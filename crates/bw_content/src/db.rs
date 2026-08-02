//! The loaded, validated content database.

use std::collections::BTreeSet;
use std::path::Path;

use bw_core::{ContentId, Interner};
use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use smol_str::SmolStr;

use crate::error::{ContentError, ContentResult};
use crate::registry::GeneratorRegistry;
use crate::schema::{
    AbilityDef, CharacterDef, EncounterDef, PropDef, RockDef, StatusDef, TerrainDef,
};

/// Maximum nesting depth of an effect tree.
///
/// Deep trees are almost always a mistake — a self-referential include or a
/// generator gone wrong — and catching it here beats a stack overflow during a
/// battle.
const MAX_EFFECT_DEPTH: usize = 16;

/// Every definition, indexed by key and by [`ContentId`].
///
/// `IndexMap` throughout, in load order, which is sorted by filename. That is
/// what makes [`ContentId`]s reproducible: the ids end up inside observation
/// vectors, so if they shifted between runs a trained policy would be reading
/// one unit's stats under another unit's name.
#[derive(Default)]
pub struct ContentDb {
    pub characters: IndexMap<SmolStr, CharacterDef>,
    pub abilities: IndexMap<SmolStr, AbilityDef>,
    pub statuses: IndexMap<SmolStr, StatusDef>,
    pub terrain: IndexMap<SmolStr, TerrainDef>,
    pub rocks: IndexMap<SmolStr, RockDef>,
    pub props: IndexMap<SmolStr, PropDef>,
    pub encounters: IndexMap<SmolStr, EncounterDef>,
    interner: Interner,
}

impl ContentDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load every `.ron` file under `root`, one subdirectory per kind.
    ///
    /// Expects `characters/`, `abilities/`, `status/`, `terrain/`, `rocks/`,
    /// `props/`, `encounters/`. A missing directory is not an error — a project
    /// with no props yet should still load.
    pub fn load_dir(root: &Path) -> ContentResult<Self> {
        let mut db = Self::new();
        db.characters = load_kind(&root.join("characters"), |d: &CharacterDef| d.key.clone())?;
        db.abilities = load_kind(&root.join("abilities"), |d: &AbilityDef| d.key.clone())?;
        db.statuses = load_kind(&root.join("status"), |d: &StatusDef| d.key.clone())?;
        db.terrain = load_kind(&root.join("terrain"), |d: &TerrainDef| d.key.clone())?;
        db.rocks = load_kind(&root.join("rocks"), |d: &RockDef| d.key.clone())?;
        db.props = load_kind(&root.join("props"), |d: &PropDef| d.key.clone())?;
        db.encounters = load_kind(&root.join("encounters"), |d: &EncounterDef| d.key.clone())?;
        db.rebuild_ids();
        Ok(db)
    }

    /// Assign [`ContentId`]s. Call after mutating the maps directly.
    pub fn rebuild_ids(&mut self) {
        let mut interner = Interner::new();
        // Order matters and must not change: characters, then abilities, then
        // statuses, then terrain, rocks, props, encounters.
        for key in self.characters.keys() {
            interner.intern(key.clone());
        }
        for key in self.abilities.keys() {
            interner.intern(key.clone());
        }
        for key in self.statuses.keys() {
            interner.intern(key.clone());
        }
        for key in self.terrain.keys() {
            interner.intern(key.clone());
        }
        for key in self.rocks.keys() {
            interner.intern(key.clone());
        }
        for key in self.props.keys() {
            interner.intern(key.clone());
        }
        for key in self.encounters.keys() {
            interner.intern(key.clone());
        }
        self.interner = interner;
    }

    pub fn id(&self, key: &str) -> ContentId {
        self.interner.get(key).unwrap_or(ContentId::NONE)
    }

    pub fn key(&self, id: ContentId) -> Option<&str> {
        self.interner.resolve(id)
    }

    /// Check every cross-reference in the database.
    ///
    /// `effect_kinds` comes from the simulation's effect registry and
    /// `generators` from the content plugins, because this crate deliberately
    /// does not know what either of them contains.
    pub fn validate(
        &self,
        effect_kinds: &BTreeSet<&str>,
        generators: &GeneratorRegistry,
    ) -> ContentResult<()> {
        for character in self.characters.values() {
            for ability in &character.abilities {
                if !self.abilities.contains_key(ability) {
                    return Err(ContentError::UnknownReference {
                        referrer: character.key.clone(),
                        kind: "ability",
                        key: ability.clone(),
                    });
                }
            }
        }

        for ability in self.abilities.values() {
            if ability.effect.depth() > MAX_EFFECT_DEPTH {
                return Err(ContentError::Invalid {
                    context: ability.key.clone(),
                    message: format!(
                        "effect tree is {} deep, limit is {MAX_EFFECT_DEPTH}",
                        ability.effect.depth()
                    ),
                });
            }
            for node in ability.effect.walk() {
                if !effect_kinds.contains(node.kind.as_str()) {
                    return Err(ContentError::UnknownEffectKind {
                        referrer: ability.key.clone(),
                        kind: node.kind.clone(),
                    });
                }
            }
        }

        for status in self.statuses.values() {
            if let Some(effect) = &status.tick_effect {
                for node in effect.walk() {
                    if !effect_kinds.contains(node.kind.as_str()) {
                        return Err(ContentError::UnknownEffectKind {
                            referrer: status.key.clone(),
                            kind: node.kind.clone(),
                        });
                    }
                }
            }
        }

        for rock in self.rocks.values() {
            match generators.rock(&rock.generator) {
                None => {
                    return Err(ContentError::UnknownGenerator {
                        referrer: rock.key.clone(),
                        key: rock.generator.clone(),
                    });
                }
                Some(generator) => generator.validate(&rock.params)?,
            }
        }

        for encounter in self.encounters.values() {
            match generators.terrain(&encounter.terrain_generator) {
                None => {
                    return Err(ContentError::UnknownGenerator {
                        referrer: encounter.key.clone(),
                        key: encounter.terrain_generator.clone(),
                    });
                }
                Some(generator) => generator.validate(&encounter.terrain_params)?,
            }
            if encounter.grid_width == 0 || encounter.grid_height == 0 {
                return Err(ContentError::Invalid {
                    context: encounter.key.clone(),
                    message: "grid dimensions must be non-zero".into(),
                });
            }
            for team in &encounter.teams {
                for entry in &team.units {
                    if !self.characters.contains_key(&entry.character) {
                        return Err(ContentError::UnknownReference {
                            referrer: encounter.key.clone(),
                            kind: "character",
                            key: entry.character.clone(),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.characters.is_empty() && self.abilities.is_empty() && self.encounters.is_empty()
    }
}

/// Read every `.ron` in `dir`, in sorted filename order.
fn load_kind<T, F>(dir: &Path, key_of: F) -> ContentResult<IndexMap<SmolStr, T>>
where
    T: DeserializeOwned,
    F: Fn(&T) -> SmolStr,
{
    let mut out = IndexMap::new();
    if !dir.is_dir() {
        return Ok(out);
    }

    // Sorted, because directory iteration order is filesystem-dependent and
    // ContentIds are derived from load order.
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|source| ContentError::Io {
            file: dir.display().to_string(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ron"))
        .collect();
    paths.sort();

    for path in paths {
        let display = path.display().to_string();
        let text = std::fs::read_to_string(&path).map_err(|source| ContentError::Io {
            file: display.clone(),
            source,
        })?;
        let parsed: T = ron::from_str(&text).map_err(|source| ContentError::Parse {
            file: display,
            source: Box::new(source),
        })?;
        let key = key_of(&parsed);
        if out.insert(key.clone(), parsed).is_some() {
            return Err(ContentError::DuplicateKey {
                kind: "definition",
                key,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectSpec;
    use crate::schema::BaseStats;

    fn stats() -> BaseStats {
        BaseStats {
            max_health: 10.0,
            move_speed: 1.0,
            attack_damage: 1.0,
            attack_range: 1.0,
            attack_cooldown_ticks: 8,
            armor: 0.0,
            radius: 0.5,
        }
    }

    fn db_with_character(abilities: Vec<SmolStr>) -> ContentDb {
        let mut db = ContentDb::new();
        db.characters.insert(
            SmolStr::new("goblin"),
            CharacterDef {
                key: SmolStr::new("goblin"),
                name: "Goblin".into(),
                stats: stats(),
                abilities,
                sprite: SmolStr::default(),
                tags: vec![],
            },
        );
        db.rebuild_ids();
        db
    }

    fn kinds(list: &[&'static str]) -> BTreeSet<&'static str> {
        list.iter().copied().collect()
    }

    #[test]
    fn ids_are_dense_and_reversible() {
        let db = db_with_character(vec![]);
        let id = db.id("goblin");
        assert_eq!(id, ContentId(0));
        assert_eq!(db.key(id), Some("goblin"));
        assert!(db.id("nonexistent").is_none());
    }

    #[test]
    fn a_character_referencing_a_missing_ability_is_rejected() {
        let db = db_with_character(vec![SmolStr::new("fireball")]);
        let err = db
            .validate(&kinds(&[]), &GeneratorRegistry::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("goblin"), "{err}");
        assert!(err.contains("fireball"), "{err}");
    }

    #[test]
    fn an_unregistered_effect_kind_is_rejected() {
        let mut db = ContentDb::new();
        db.abilities.insert(
            SmolStr::new("zap"),
            AbilityDef {
                key: SmolStr::new("zap"),
                name: "Zap".into(),
                cooldown_ticks: 8,
                cast_time_ticks: 0,
                range: 4.0,
                effect: EffectSpec::new("nonexistent_kind"),
                tags: vec![],
            },
        );
        db.rebuild_ids();
        let err = db
            .validate(&kinds(&["damage"]), &GeneratorRegistry::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("nonexistent_kind"), "{err}");
    }

    #[test]
    fn a_valid_database_passes() {
        let mut db = ContentDb::new();
        db.abilities.insert(
            SmolStr::new("zap"),
            AbilityDef {
                key: SmolStr::new("zap"),
                name: "Zap".into(),
                cooldown_ticks: 8,
                cast_time_ticks: 0,
                range: 4.0,
                effect: EffectSpec::new("damage"),
                tags: vec![],
            },
        );
        db.characters.insert(
            SmolStr::new("goblin"),
            CharacterDef {
                key: SmolStr::new("goblin"),
                name: "Goblin".into(),
                stats: stats(),
                abilities: vec![SmolStr::new("zap")],
                sprite: SmolStr::default(),
                tags: vec![],
            },
        );
        db.rebuild_ids();
        assert!(
            db.validate(&kinds(&["damage"]), &GeneratorRegistry::new())
                .is_ok()
        );
    }

    #[test]
    fn loading_a_missing_directory_yields_an_empty_database() {
        let db = ContentDb::load_dir(Path::new("/nonexistent/bw-content")).unwrap();
        assert!(db.is_empty());
    }
}

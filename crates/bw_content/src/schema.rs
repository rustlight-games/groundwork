//! Authored content definitions.
//!
//! Everything here is deserialised from RON under `assets/content/`. Durations
//! are in ticks rather than seconds so that content cannot introduce rounding,
//! and stats are `f64` at rest and converted to [`Real`] when the battle loads.

use bw_core::Real;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::effect::EffectSpec;
use crate::params::Params;

/// Combat statistics, as authored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseStats {
    pub max_health: f64,
    /// World units per second.
    pub move_speed: f64,
    pub attack_damage: f64,
    pub attack_range: f64,
    pub attack_cooldown_ticks: u32,
    /// Flat damage reduction applied before the health subtraction.
    #[serde(default)]
    pub armor: f64,
    /// Collision radius, also used by local avoidance.
    #[serde(default = "default_radius")]
    pub radius: f64,
}

fn default_radius() -> f64 {
    0.5
}

/// The same statistics after conversion, as the simulation sees them.
///
/// Produced once at load. Simulation never touches the `f64` originals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedStats {
    pub max_health: Real,
    pub move_speed: Real,
    pub attack_damage: Real,
    pub attack_range: Real,
    pub attack_cooldown_ticks: u32,
    pub armor: Real,
    pub radius: Real,
}

impl BaseStats {
    pub fn resolve(&self) -> ResolvedStats {
        ResolvedStats {
            max_health: Real::from_num(self.max_health),
            move_speed: Real::from_num(self.move_speed),
            attack_damage: Real::from_num(self.attack_damage),
            attack_range: Real::from_num(self.attack_range),
            attack_cooldown_ticks: self.attack_cooldown_ticks,
            armor: Real::from_num(self.armor),
            radius: Real::from_num(self.radius),
        }
    }
}

/// A fieldable unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterDef {
    pub key: SmolStr,
    pub name: String,
    pub stats: BaseStats,
    /// Ability keys, in the order they occupy the unit's slots. The action
    /// space the network learns over is indexed by slot, so reordering these
    /// invalidates trained policies.
    #[serde(default)]
    pub abilities: Vec<SmolStr>,
    #[serde(default)]
    pub sprite: SmolStr,
    #[serde(default)]
    pub tags: Vec<SmolStr>,
}

/// An activated ability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbilityDef {
    pub key: SmolStr,
    pub name: String,
    pub cooldown_ticks: u32,
    #[serde(default)]
    pub cast_time_ticks: u32,
    /// World units. Zero means self-cast or unlimited, per the effect tree.
    #[serde(default)]
    pub range: f64,
    pub effect: EffectSpec,
    #[serde(default)]
    pub tags: Vec<SmolStr>,
}

/// A timed modifier attached to a unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusDef {
    pub key: SmolStr,
    pub name: String,
    pub duration_ticks: u32,
    #[serde(default = "one")]
    pub max_stacks: u32,
    /// Refresh duration when reapplied, rather than adding a stack.
    #[serde(default)]
    pub refreshes: bool,
    /// Runs every tick the status is held, e.g. a damage-over-time.
    #[serde(default)]
    pub tick_effect: Option<EffectSpec>,
    #[serde(default)]
    pub modifiers: Params,
}

fn one() -> u32 {
    1
}

/// A terrain tile type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainDef {
    pub key: SmolStr,
    pub name: String,
    /// 256 is normal ground; higher is slower.
    #[serde(default = "normal_cost")]
    pub move_cost: u16,
    #[serde(default)]
    pub blocks: bool,
    /// 0..=1, scaled to the map's 0..=255 density channel.
    #[serde(default)]
    pub grass_density: f64,
    #[serde(default)]
    pub color: [u8; 3],
}

fn normal_cost() -> u16 {
    crate::terrain::NORMAL_COST
}

/// A procedurally generated rock.
///
/// Rocks and terrain are the two things generated rather than drawn — they make
/// up the landscape and its obstacles, and there are too many needed, in too
/// many sizes, for hand-authored art to keep up. Everything else in the scene
/// (trees, bushes, decoration) is a sprite placed by a [`ScatterRule`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RockDef {
    pub key: SmolStr,
    /// Registry key of the [`RockGenerator`](crate::registry::RockGenerator).
    pub generator: SmolStr,
    #[serde(default)]
    pub params: Params,
    /// Whether units path around it. Small scatter rocks should not block.
    #[serde(default)]
    pub blocks_movement: bool,
    #[serde(default)]
    pub scatter: Option<ScatterRule>,
}

/// A sprite-based prop: trees, bushes, debris.
///
/// Distinct from [`RockDef`] on purpose. These are drawn art placed into the
/// world, not generated geometry, so they carry a sprite path and a placement
/// rule and nothing else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropDef {
    pub key: SmolStr,
    pub sprite: SmolStr,
    #[serde(default)]
    pub blocks_movement: bool,
    /// Extra movement cost added to the cell, for things units push through.
    #[serde(default)]
    pub move_cost: u16,
    pub scatter: ScatterRule,
}

/// How often and where a prop or rock is placed during generation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScatterRule {
    /// Expected instances per hundred cells.
    pub density_per_100_cells: f64,
    /// Minimum world-unit separation. Enforced by dart throwing, which is what
    /// keeps scatter from clumping — see the blue-noise metric in `bw_bench`.
    #[serde(default)]
    pub min_spacing: f64,
    /// Terrain keys this may appear on. Empty means anywhere unblocked.
    #[serde(default)]
    pub allowed_terrain: Vec<SmolStr>,
    /// Inclusive elevation band, 0..=255.
    #[serde(default = "full_elevation")]
    pub elevation_range: (u8, u8),
}

fn full_elevation() -> (u8, u8) {
    (0, 255)
}

/// A battle setup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EncounterDef {
    pub key: SmolStr,
    pub name: String,
    /// Registry key of the terrain generator to build the battlefield with.
    pub terrain_generator: SmolStr,
    #[serde(default)]
    pub terrain_params: Params,
    pub grid_width: u32,
    pub grid_height: u32,
    #[serde(default)]
    pub teams: Vec<TeamRoster>,
}

/// One side's units and where they start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamRoster {
    pub team: u8,
    pub units: Vec<RosterEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterEntry {
    pub character: SmolStr,
    #[serde(default = "one")]
    pub count: u32,
    /// Spawn area centre in world units, jittered deterministically per unit.
    #[serde(default)]
    pub spawn: (f64, f64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_resolve_to_fixed_point() {
        let stats = BaseStats {
            max_health: 120.0,
            move_speed: 3.5,
            attack_damage: 9.0,
            attack_range: 1.5,
            attack_cooldown_ticks: 48,
            armor: 2.0,
            radius: 0.5,
        };
        let r = stats.resolve();
        assert_eq!(r.max_health, Real::from_num(120));
        assert_eq!(r.move_speed, Real::from_num(3.5));
        assert_eq!(r.attack_cooldown_ticks, 48);
    }

    #[test]
    fn character_defaults_fill_in() {
        let def: CharacterDef = ron::from_str(
            r#"(
                key: "goblin",
                name: "Goblin",
                stats: (
                    max_health: 40.0, move_speed: 4.0, attack_damage: 5.0,
                    attack_range: 1.0, attack_cooldown_ticks: 32,
                ),
            )"#,
        )
        .unwrap();
        assert_eq!(def.key, "goblin");
        assert!(def.abilities.is_empty());
        assert_eq!(def.stats.radius, 0.5);
        assert_eq!(def.stats.armor, 0.0);
    }

    #[test]
    fn scatter_rule_defaults_to_the_whole_elevation_band() {
        let rule: ScatterRule = ron::from_str("(density_per_100_cells: 2.0)").unwrap();
        assert_eq!(rule.elevation_range, (0, 255));
        assert!(rule.allowed_terrain.is_empty());
    }

    #[test]
    fn terrain_defaults_to_open_ground() {
        let def: TerrainDef = ron::from_str(r#"(key: "grass", name: "Grassland")"#).unwrap();
        assert_eq!(def.move_cost, crate::terrain::NORMAL_COST);
        assert!(!def.blocks);
    }
}

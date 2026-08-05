//! Bringing an older document up to the current version.
//!
//! ## The rules a migration follows
//!
//! - **One step per version.** `1 -> 2 -> 3`, never `1 -> 3`. A direct jump has
//!   to reimplement everything the intermediate steps did, and it silently rots
//!   the moment step two changes.
//! - **A migration is pure and total.** It takes a document of version *n* and
//!   returns one of version *n + 1*. It does not fail on unusual content: a
//!   document that was legal before must be legal after, or it is not a
//!   migration, it is a breaking change with a friendly name.
//! - **A migration never validates.** Validation happens once, at the end, on
//!   the canonicalised document. A migration that rejected invalid input would
//!   make an old broken document unopenable, when the whole point is to open it
//!   and show the author what is wrong.
//! - **Migrations are tested against real old documents**, kept as fixtures.
//!   A migration tested only against a synthetic input tests the test.

use crate::envelope::{CURRENT_FORMAT_VERSION, OLDEST_READABLE_VERSION};
use crate::raw::RawDocument;

/// Why a document could not be brought to the current version.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    #[error(
        "this document is format version {found}, which is newer than this build \
         understands ({current}); update the tool"
    )]
    FromTheFuture { found: u32, current: u32 },

    #[error(
        "this document is format version {found}, which is older than this build \
         can read ({oldest}); open it with an older tool and save it forward"
    )]
    TooOld { found: u32, oldest: u32 },
}

/// What a migration did, for reporting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationLog {
    pub from_version: u32,
    pub to_version: u32,
    /// One line per step taken, in order.
    pub steps: Vec<String>,
}

impl MigrationLog {
    pub fn migrated(&self) -> bool {
        self.from_version != self.to_version
    }
}

/// Bring `document` from `version` up to [`CURRENT_FORMAT_VERSION`].
pub fn migrate(
    document: RawDocument,
    version: u32,
) -> Result<(RawDocument, MigrationLog), MigrationError> {
    if version > CURRENT_FORMAT_VERSION {
        return Err(MigrationError::FromTheFuture {
            found: version,
            current: CURRENT_FORMAT_VERSION,
        });
    }
    if version < OLDEST_READABLE_VERSION {
        return Err(MigrationError::TooOld {
            found: version,
            oldest: OLDEST_READABLE_VERSION,
        });
    }

    let mut log = MigrationLog {
        from_version: version,
        to_version: version,
        steps: Vec::new(),
    };
    let mut document = document;

    // One step per version, carrying the document through each in turn.
    while log.to_version < CURRENT_FORMAT_VERSION {
        let (next, note) = step(document, log.to_version);
        document = next;
        log.steps.push(note);
        log.to_version += 1;
    }

    Ok((document, log))
}

/// One version's worth of change.
///
/// Written as a `match` with an arm per step rather than a chain of `if`s, so
/// that raising [`CURRENT_FORMAT_VERSION`] without writing the migration fails
/// loudly here rather than silently skipping a version.
fn step(document: RawDocument, from: u32) -> (RawDocument, String) {
    match from {
        1 => one_to_two(document),
        _ => unreachable!("no migration is registered from format version {from}"),
    }
}

/// Version one to version two: profiles, affinities and roles.
///
/// Three things a version-one document could not say, and all three were being
/// guessed at runtime instead:
///
/// - **Which ground profile a material uses.** It was decided by the appearance
///   key, in a table in the Blender build.
/// - **Whether grass grows on it.** It was decided by looking for the substring
///   `dirt`, `mud`, `rock`, `sand` or `gravel` in the material's key. That rule
///   is reproduced exactly here, once, at migration time — which is the right
///   place for a guess: it happens on a document an author can then correct,
///   rather than on every sample of every render forever.
/// - **What a modifier channel means.** It was decided by exact string match on
///   `soil_moisture` and `soil_compaction`. Same treatment: the conventional
///   names get their canonical roles, and a document that used a different word
///   comes through with no role and can be told so.
///
/// Nothing here fails. A material this cannot classify gets no profile and no
/// affinity, which is exactly what it had before.
fn one_to_two(mut document: RawDocument) -> (RawDocument, String) {
    let mut profiled = 0usize;
    let mut roled = 0usize;

    for material in &mut document.materials {
        if material.profile.is_none() {
            material.profile = default_profile(&material.key).map(str::to_string);
            if material.profile.is_some() {
                profiled += 1;
            }
        }
        if material.vegetation_affinity.is_none() {
            material.vegetation_affinity = Some(if grew_grass_under_the_old_rule(&material.key) {
                1.0
            } else {
                0.0
            });
        }
    }

    for channel in &mut document.modifier_channels {
        if channel.role.is_none() {
            channel.role = conventional_role(&channel.key).map(str::to_string);
            if channel.role.is_some() {
                roled += 1;
            }
        }
    }

    let note = format!(
        "1 -> 2: bound {profiled} material(s) to a ground profile, gave every material an \
         explicit vegetation affinity, and named the role of {roled} modifier channel(s)"
    );
    (document, note)
}

/// The profile a version-one material key implies.
///
/// Keyed on the material key rather than the appearance key because the material
/// key is what an author chose to describe the ground; two materials sharing
/// `surface.ground` are two different soils.
fn default_profile(key: &str) -> Option<&'static str> {
    Some(match key {
        "meadow_soil" => "materials/meadow_floor.ground.ron",
        "dirt_compacted" => "materials/compacted_loam.ground.ron",
        "grass_lush" | "grass_dry" => "materials/meadow_floor.ground.ron",
        _ => return None,
    })
}

/// The rule that used to live in the CLI, preserved exactly.
fn grew_grass_under_the_old_rule(key: &str) -> bool {
    !(key.contains("dirt")
        || key.contains("mud")
        || key.contains("rock")
        || key.contains("sand")
        || key.contains("gravel"))
}

/// The role a conventionally-named version-one channel was being read as.
fn conventional_role(key: &str) -> Option<&'static str> {
    Some(match key {
        "vegetation_density" => "VegetationDensity",
        "soil_moisture" => "SoilMoisture",
        "soil_compaction" => "SoilCompaction",
        "soil_disturbance" => "SoilDisturbance",
        "grit_abundance" => "LooseMaterial",
        "soil_dryness" | "desiccation" => "Desiccation",
        "organic_matter" => "OrganicMatter",
        "wind_exposure" => "WindExposure",
        "water_supply" => "WaterSupply",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::raw::{RawMaterial, RawModifierChannel};

    fn document() -> RawDocument {
        RawDocument {
            coordinate_system: "PlanarMetres".into(),
            root_seed: "8df782f95ce1a4d4".into(),
            metadata: Default::default(),
            materials: Vec::new(),
            modifier_channels: Vec::new(),
            sources: Vec::new(),
            layers: Vec::new(),
            populations: Vec::new(),
        }
    }

    fn material(key: &str) -> RawMaterial {
        RawMaterial {
            key: key.into(),
            display_name: String::new(),
            appearance: format!("surface.{key}"),
            profile: None,
            vegetation_affinity: None,
        }
    }

    fn channel(key: &str) -> RawModifierChannel {
        RawModifierChannel {
            key: key.into(),
            display_name: String::new(),
            range: (0.0, 1.0),
            default_value: 0.0,
            composition: "Max".into(),
            unit: "Unitless".into(),
            role: None,
        }
    }

    /// The two materials every shipped version-one document is built from.
    fn meadow_and_track() -> RawDocument {
        let mut document = document();
        document.materials = vec![material("meadow_soil"), material("dirt_compacted")];
        document.modifier_channels = vec![
            channel("vegetation_density"),
            channel("soil_moisture"),
            channel("soil_compaction"),
            channel("flower_abundance"),
        ];
        document
    }

    #[test]
    fn version_one_materials_reach_version_two_with_profiles() {
        let (migrated, log) = migrate(meadow_and_track(), 1).expect("migrates");
        assert!(log.migrated());
        assert_eq!(
            migrated.materials[0].profile.as_deref(),
            Some("materials/meadow_floor.ground.ron")
        );
        assert_eq!(
            migrated.materials[1].profile.as_deref(),
            Some("materials/compacted_loam.ground.ron")
        );
    }

    #[test]
    fn the_old_substring_rule_is_frozen_into_the_migration() {
        // The picture a version-one document produced came from this rule. It
        // has to survive the migration exactly, or every existing render moves.
        let (migrated, _) = migrate(meadow_and_track(), 1).expect("migrates");
        assert_eq!(migrated.materials[0].vegetation_affinity, Some(1.0));
        assert_eq!(migrated.materials[1].vegetation_affinity, Some(0.0));
    }

    #[test]
    fn conventional_channel_names_get_their_roles_and_others_do_not() {
        let (migrated, _) = migrate(meadow_and_track(), 1).expect("migrates");
        let roles: Vec<_> = migrated
            .modifier_channels
            .iter()
            .map(|c| c.role.as_deref())
            .collect();
        assert_eq!(
            roles,
            vec![
                Some("VegetationDensity"),
                Some("SoilMoisture"),
                Some("SoilCompaction"),
                // Not a canonical state channel. No role, and nothing invented.
                None,
            ]
        );
    }

    #[test]
    fn a_document_that_already_said_it_is_left_alone() {
        // A migration is pure and total, and that includes not overwriting an
        // answer the author gave. A version-one document written by hand may
        // already carry the new fields; the step must not second-guess it.
        let mut document = document();
        let mut material = material("dirt_compacted");
        material.profile = Some("materials/somebody_elses_soil.ground.ron".into());
        material.vegetation_affinity = Some(0.08);
        document.materials = vec![material];

        let (migrated, _) = migrate(document, 1).expect("migrates");
        assert_eq!(
            migrated.materials[0].profile.as_deref(),
            Some("materials/somebody_elses_soil.ground.ron")
        );
        assert_eq!(migrated.materials[0].vegetation_affinity, Some(0.08));
    }

    #[test]
    fn an_unrecognised_material_migrates_to_no_profile_rather_than_a_guess() {
        let mut document = document();
        document.materials = vec![material("volcanic_ash")];
        let (migrated, _) = migrate(document, 1).expect("migrates");
        assert_eq!(migrated.materials[0].profile, None);
        // Still affinity 1.0: the old rule grew grass on anything it could not
        // name, and reproducing the old picture is the point.
        assert_eq!(migrated.materials[0].vegetation_affinity, Some(1.0));
    }

    #[test]
    fn a_current_document_passes_through_untouched() {
        let (_, log) = migrate(document(), CURRENT_FORMAT_VERSION).expect("current");
        assert!(!log.migrated());
        assert!(log.steps.is_empty());
    }

    #[test]
    fn a_document_from_the_future_is_refused_by_its_own_version() {
        // Refused here, with the number in the message, rather than through
        // whatever confusing shape mismatch a newer body happens to produce.
        assert_eq!(
            migrate(document(), CURRENT_FORMAT_VERSION + 1).err(),
            Some(MigrationError::FromTheFuture {
                found: CURRENT_FORMAT_VERSION + 1,
                current: CURRENT_FORMAT_VERSION,
            })
        );
    }

    #[test]
    fn a_document_older_than_anything_readable_is_refused() {
        assert_eq!(
            migrate(document(), 0).err(),
            Some(MigrationError::TooOld {
                found: 0,
                oldest: OLDEST_READABLE_VERSION,
            })
        );
    }

    #[test]
    fn the_readable_range_is_coherent() {
        // A build that claimed to read only from a version newer than it writes
        // could never open anything. Constants today, and the check is here for
        // the commit that changes one of them without the other.
        let (oldest, current) = (OLDEST_READABLE_VERSION, CURRENT_FORMAT_VERSION);
        assert!(oldest <= current, "{oldest} > {current}");
        assert!(oldest >= 1, "version zero is not a version");
    }
}

//! Reading a ground material profile off disk.
//!
//! The same shape as a terrain document — frozen envelope, raw body, migration,
//! canonicalisation — for the same reasons, and with one difference worth
//! stating: a profile is versioned **independently** of the document that names
//! it. A soil retuned to version three must not force every document that
//! mentions it up a version, because the two files answer different questions
//! and change on completely different schedules. A document changes when a map
//! changes; a profile changes when somebody looks harder at a photograph.
//!
//! ## Why the numbers are here and not in the shader
//!
//! Every value in a profile was, until recently, a literal in a Blender node
//! graph. That works exactly until there are two soils, at which point the graph
//! grows a branch per material and the interesting numbers — the ones measured
//! off a reference plate — end up scattered through Python that nothing in Rust
//! can read, test or report on.
//!
//! So the numbers live in an asset, both halves read them, and the one place
//! they are written down is a file an author can open.
//!
//! ## A large error is fine here
//!
//! [`ProfileError`] carries a path, a parser span and — for the interesting
//! variant — every diagnostic found. Clippy objects to a wide `Result`, and the
//! objection is aimed at hot paths where the success value is small and returned
//! millions of times. A profile is read once per document. The same argument
//! `ron_io` makes, for the same reason.
#![allow(clippy::result_large_err)]

use serde::{Deserialize, Serialize};
use terrain_core::diagnostics::{DiagnosticReport, Location};
use terrain_core::document::AssetPath;
use terrain_core::ground_material::*;
use terrain_core::ids::{AppearanceKey, GroundProfileKey};

/// The `format` string every ground profile carries.
pub const GROUND_PROFILE_FORMAT: &str = "ground-material";

/// The version this build writes.
pub const CURRENT_PROFILE_VERSION: u32 = 1;

/// The oldest version this build can read.
pub const OLDEST_READABLE_PROFILE_VERSION: u32 = 1;

/// A profile, with what it is and which version of it. Frozen, like the
/// document envelope and for the same reason.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileEnvelope {
    pub format: String,
    pub format_version: u32,
    pub material: RawGroundProfile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawGroundProfile {
    pub key: String,
    #[serde(default = "surface_ground")]
    pub shader: String,
    #[serde(default)]
    pub display_name: String,
    pub optics: RawOptics,
    pub structure: RawStructure,
    #[serde(default)]
    pub ripples: Option<RawRipples>,
    #[serde(default)]
    pub cracks: Option<RawCracks>,
    pub scatter: RawScatter,
    #[serde(default)]
    pub vegetation_affinity: f32,
}

fn surface_ground() -> String {
    "surface.ground".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawOptics {
    pub dry_palette: RawPalette,
    pub wet: RawWet,
    pub roughness_dry: (f32, f32),
    pub ior: f32,
    pub region_wavelength_m: f32,
    pub region_strength: f32,
    pub patch_wavelength_m: f32,
    pub patch_strength: f32,
    pub grain_strength: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPalette {
    pub low: (f32, f32, f32),
    pub mid: (f32, f32, f32),
    pub high: (f32, f32, f32),
}

impl RawPalette {
    fn resolved(&self) -> Palette {
        Palette {
            low: rgb(self.low),
            mid: rgb(self.mid),
            high: rgb(self.high),
        }
    }
}

/// Tuples on the wire, arrays in the model.
///
/// RON writes a fixed-size array as a tuple, so `[0.05, 0.03, 0.02]` in a file
/// is a parse error with a span pointing at the bracket. Rather than teach every
/// author that, the wire type is the tuple it was always going to be.
fn rgb(value: (f32, f32, f32)) -> LinearRgb {
    [value.0, value.1, value.2]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawWet {
    pub wet_mid: (f32, f32, f32),
    pub roughness_wet: f32,
    pub saturation_flattening: f32,
    #[serde(default = "water_ior")]
    pub film_ior: f32,
}

fn water_ior() -> f32 {
    1.333
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawStructure {
    pub bands: Vec<RawBand>,
    pub cluster_wavelength_m: f32,
    pub cluster_strength: f32,
    pub cohesion: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBand {
    pub wavelength_m: f32,
    pub amplitude_m: f32,
    #[serde(default = "rounded")]
    pub shape: String,
    pub compaction_response: f32,
    #[serde(default)]
    pub clustered: bool,
}

fn rounded() -> String {
    "Rounded".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRipples {
    pub wavelength_m: f32,
    pub amplitude_m: f32,
    pub direction_deg: f32,
    pub crest_sharpness: f32,
    pub asymmetry: f32,
    pub meander_m: f32,
    pub wetness_suppression: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCracks {
    pub polygon_m: f32,
    pub width_m: f32,
    pub depth_m: f32,
    pub secondary: f32,
    pub curl_m: f32,
    pub moisture_ceiling: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawScatter {
    pub grit_per_m2: f32,
    pub pebble_per_m2: f32,
    pub fragment_radius_m: (f32, f32),
}

/// Why a profile could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("{path} is not valid RON: {source}")]
    Syntax {
        path: String,
        #[source]
        source: ron::error::SpannedError,
    },

    #[error("{path} says its format is `{found}`, not `{GROUND_PROFILE_FORMAT}`")]
    WrongFormat { path: String, found: String },

    #[error(
        "{path} is ground profile version {found}; this build reads \
         {OLDEST_READABLE_PROFILE_VERSION}..={CURRENT_PROFILE_VERSION}"
    )]
    UnreadableVersion { path: String, found: u32 },

    #[error("{path} is not a usable ground profile:\n{report}")]
    Invalid {
        path: String,
        report: Box<DiagnosticReport>,
    },
}

/// Parse and validate one profile.
///
/// The path is carried only so errors can name the file; nothing is read from
/// disk here. Bytes arrive from an [`AssetResolver`](terrain_core::AssetResolver)
/// so that a profile can live in a zip, an editor buffer or a test map.
pub fn from_str(path: &str, text: &str) -> Result<GroundMaterialProfile, ProfileError> {
    let envelope: ProfileEnvelope = ron::from_str(text).map_err(|source| ProfileError::Syntax {
        path: path.to_string(),
        source,
    })?;
    if envelope.format != GROUND_PROFILE_FORMAT {
        return Err(ProfileError::WrongFormat {
            path: path.to_string(),
            found: envelope.format,
        });
    }
    if envelope.format_version < OLDEST_READABLE_PROFILE_VERSION
        || envelope.format_version > CURRENT_PROFILE_VERSION
    {
        return Err(ProfileError::UnreadableVersion {
            path: path.to_string(),
            found: envelope.format_version,
        });
    }

    let mut report = DiagnosticReport::new();
    let raw = &envelope.material;

    let key = match GroundProfileKey::new(raw.key.clone()) {
        Ok(key) => key,
        Err(problem) => {
            report.error(
                "bad_key",
                Location::at("material.key"),
                format!("`{}` is not a usable profile key: {problem}", raw.key),
            );
            GroundProfileKey::new("unnamed").expect("literal")
        }
    };
    let shader = match AppearanceKey::new(raw.shader.clone()) {
        Ok(key) => key,
        Err(problem) => {
            report.error(
                "bad_key",
                Location::at("material.shader"),
                format!("`{}` is not a usable appearance key: {problem}", raw.shader),
            );
            AppearanceKey::new("surface.ground").expect("literal")
        }
    };
    let mut bands = Vec::with_capacity(raw.structure.bands.len());
    for (index, band) in raw.structure.bands.iter().enumerate() {
        let shape = match band.shape.as_str() {
            "Rounded" => AggregateShape::Rounded,
            "RoundedRidged" => AggregateShape::RoundedRidged,
            "Angular" => AggregateShape::Angular,
            other => {
                report.error(
                    "unknown_variant",
                    Location::at(format!("material.structure.bands[{index}].shape")),
                    format!("`{other}` is not one of Rounded, RoundedRidged, Angular"),
                );
                AggregateShape::Rounded
            }
        };
        bands.push(ReliefBand {
            wavelength_m: band.wavelength_m,
            amplitude_m: band.amplitude_m,
            shape,
            compaction_response: band.compaction_response,
            clustered: band.clustered,
        });
    }

    let profile = GroundMaterialProfile {
        key,
        shader,
        display_name: if raw.display_name.is_empty() {
            raw.key.clone()
        } else {
            raw.display_name.clone()
        },
        optics: GroundOptics {
            dry_palette: raw.optics.dry_palette.resolved(),
            wet: WetResponse {
                wet_mid: rgb(raw.optics.wet.wet_mid),
                roughness_wet: raw.optics.wet.roughness_wet,
                saturation_flattening: raw.optics.wet.saturation_flattening,
                film_ior: raw.optics.wet.film_ior,
            },
            roughness_dry: Span::new(raw.optics.roughness_dry.0, raw.optics.roughness_dry.1),
            ior: raw.optics.ior,
            region_wavelength_m: raw.optics.region_wavelength_m,
            region_strength: raw.optics.region_strength,
            patch_wavelength_m: raw.optics.patch_wavelength_m,
            patch_strength: raw.optics.patch_strength,
            grain_strength: raw.optics.grain_strength,
        },
        structure: GroundStructure {
            bands,
            cluster_wavelength_m: raw.structure.cluster_wavelength_m,
            cluster_strength: raw.structure.cluster_strength,
            cohesion: raw.structure.cohesion,
        },
        // Degrees on the wire and radians in the model. An author reasoning
        // about which way the wind blew is thinking in degrees; every consumer
        // is feeding a trigonometric function.
        ripples: raw.ripples.as_ref().map(|r| RippleProfile {
            wavelength_m: r.wavelength_m,
            amplitude_m: r.amplitude_m,
            direction_rad: r.direction_deg.to_radians(),
            crest_sharpness: r.crest_sharpness,
            asymmetry: r.asymmetry,
            meander_m: r.meander_m,
            wetness_suppression: r.wetness_suppression,
        }),
        cracks: raw.cracks.as_ref().map(|c| CrackProfile {
            polygon_m: c.polygon_m,
            width_m: c.width_m,
            depth_m: c.depth_m,
            secondary: c.secondary,
            curl_m: c.curl_m,
            moisture_ceiling: c.moisture_ceiling,
        }),
        scatter: GroundScatter {
            grit_per_m2: raw.scatter.grit_per_m2,
            pebble_per_m2: raw.scatter.pebble_per_m2,
            fragment_radius_m: Span::new(
                raw.scatter.fragment_radius_m.0,
                raw.scatter.fragment_radius_m.1,
            ),
        },
        vegetation_affinity: raw.vegetation_affinity,
    };

    for problem in profile.problems() {
        report.error(
            "out_of_range",
            Location::at(format!("material.{}", problem.field)),
            problem.message,
        );
    }

    if report.has_errors() {
        return Err(ProfileError::Invalid {
            path: path.to_string(),
            report: Box::new(report),
        });
    }
    Ok(profile)
}

/// Read every profile a document names, through the document's asset resolver.
///
/// Collected into one library rather than resolved per material, because two
/// materials naming the same soil should share one parsed profile — and because
/// a document that names four missing profiles should say so once.
pub fn load_library(
    profiles: impl IntoIterator<Item = AssetPath>,
    resolver: &dyn terrain_core::AssetResolver,
) -> (GroundProfileLibrary, Vec<ProfileError>) {
    let mut library = GroundProfileLibrary::new();
    let mut problems = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for path in profiles {
        let path = path.as_str().to_string();
        if !seen.insert(path.clone()) {
            continue;
        }
        let bytes = match resolver.read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                problems.push(ProfileError::Invalid {
                    path: path.clone(),
                    report: Box::new({
                        let mut report = DiagnosticReport::new();
                        report.error(
                            "missing_asset",
                            Location::at("materials"),
                            format!("cannot be read: {error}"),
                        );
                        report
                    }),
                });
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                problems.push(ProfileError::Invalid {
                    path: path.clone(),
                    report: Box::new({
                        let mut report = DiagnosticReport::new();
                        report.error(
                            "bad_encoding",
                            Location::at("materials"),
                            "is not valid UTF-8".to_string(),
                        );
                        report
                    }),
                });
                continue;
            }
        };
        match from_str(&path, &text) {
            Ok(profile) => library.insert(path, profile),
            Err(error) => problems.push(error),
        }
    }

    (library, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOAM: &str = r#"(
    format: "ground-material",
    format_version: 1,
    material: (
        key: "compacted_loam",
        display_name: "Compacted loam",
        optics: (
            dry_palette: (
                low:  (0.0500, 0.0315, 0.0180),
                mid:  (0.0840, 0.0530, 0.0300),
                high: (0.1550, 0.0977, 0.0558),
            ),
            wet: (
                wet_mid: (0.0294, 0.0154, 0.0069),
                roughness_wet: 0.20,
                saturation_flattening: 0.45,
            ),
            roughness_dry: (0.82, 0.96),
            ior: 1.48,
            region_wavelength_m: 2.0,
            region_strength: 0.5,
            patch_wavelength_m: 0.25,
            patch_strength: 0.9,
            grain_strength: 0.55,
        ),
        structure: (
            bands: [
                (wavelength_m: 0.0500, amplitude_m: 0.0170, shape: "RoundedRidged", compaction_response: 0.85, clustered: true),
                (wavelength_m: 0.0080, amplitude_m: 0.0030, compaction_response: 0.60, clustered: true),
                (wavelength_m: 0.0012, amplitude_m: 0.0006, compaction_response: 0.30),
            ],
            cluster_wavelength_m: 0.8,
            cluster_strength: 0.5,
            cohesion: 0.62,
        ),
        scatter: (
            grit_per_m2: 90.0,
            pebble_per_m2: 0.5,
            fragment_radius_m: (0.004, 0.02),
        ),
        vegetation_affinity: 0.0,
    ),
)"#;

    #[test]
    fn a_measured_profile_reads() {
        let profile = from_str("loam.ron", LOAM).expect("reads");
        assert_eq!(profile.key.as_str(), "compacted_loam");
        // The shader defaults rather than being repeated in every file.
        assert_eq!(profile.shader.as_str(), "surface.ground");
        assert_eq!(profile.optics.dry_palette.mid, [0.0840, 0.0530, 0.0300]);
        // And the film index defaults to water.
        assert_eq!(profile.optics.wet.film_ior, 1.333);
    }

    #[test]
    fn an_unknown_field_is_refused_rather_than_ignored() {
        let text = LOAM.replace("cohesion: 0.62,", "cohesion: 0.62, chunkiness: 0.4,");
        let error = from_str("loam.ron", &text).expect_err("refused");
        assert!(matches!(error, ProfileError::Syntax { .. }), "{error}");
    }

    #[test]
    fn a_value_outside_its_range_is_reported_with_its_field() {
        let text = LOAM.replace("ior: 1.48", "ior: 9.0");
        let error = from_str("loam.ron", &text).expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("optics.ior"), "{message}");
    }

    #[test]
    fn a_profile_from_a_future_version_is_refused_by_its_number() {
        let text = LOAM.replace("format_version: 1", "format_version: 7");
        let error = from_str("loam.ron", &text).expect_err("refused");
        assert!(
            matches!(error, ProfileError::UnreadableVersion { found: 7, .. }),
            "{error}"
        );
    }

    #[test]
    fn ripple_direction_arrives_in_radians() {
        let text = LOAM.replace(
            "        scatter: (",
            r#"        ripples: Some((
            wavelength_m: 0.12,
            amplitude_m: 0.004,
            direction_deg: 90.0,
            crest_sharpness: 0.4,
            asymmetry: 0.2,
            meander_m: 0.6,
            wetness_suppression: 0.85,
        )),
        scatter: ("#,
        );
        let profile = from_str("loam.ron", &text).expect("reads");
        let ripples = profile.ripples.expect("declared");
        assert!((ripples.direction_rad - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn a_library_parses_each_path_once() {
        let assets = terrain_core::MemoryAssets::new().with("a.ron", LOAM);
        let path = AssetPath::new("a.ron").expect("valid");
        let (library, problems) = load_library([path.clone(), path], &assets);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(library.len(), 1);
    }

    #[test]
    fn a_missing_profile_is_reported_and_the_rest_still_load() {
        let assets = terrain_core::MemoryAssets::new().with("a.ron", LOAM);
        let (library, problems) = load_library(
            [
                AssetPath::new("a.ron").expect("valid"),
                AssetPath::new("gone.ron").expect("valid"),
            ],
            &assets,
        );
        assert_eq!(library.len(), 1);
        assert_eq!(problems.len(), 1);
    }
}

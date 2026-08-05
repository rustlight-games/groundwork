//! The pinned laboratories: one hard question each.
//!
//! ## Why a laboratory and not the meadow
//!
//! `meadow_path` is the subject of the project and the wrong instrument for
//! measuring a relief band. It has a spline, two soils, a ragged boundary and a
//! macro swell across it, so a roughness measurement over it is a measurement of
//! all four at once. A scenario that isolates one band on flat authored ground
//! answers "did the five-centimetre clod field come out at five centimetres" in
//! a way nothing else can.
//!
//! The meadow is still measured — `ground_meadow_path_context` is on the list —
//! but it is measured *after* the isolated cases, so that a failure there can be
//! attributed rather than merely observed.
//!
//! ## Append, never repurpose
//!
//! A scenario name is the key its baseline is filed under. Changing what a name
//! measures silently invalidates every historical comparison filed against it,
//! and the comparison table would go on reporting deltas as though they meant
//! what they used to.

use std::sync::Arc;

use terrain_core::ground_material::{
    AggregateShape, GroundMaterialProfile, GroundOptics, GroundScatter, GroundStructure, Palette,
    ReliefBand, Span, WetResponse,
};
use terrain_core::ids::GroundProfileKey;

/// One question, asked in isolation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundScenario {
    pub name: &'static str,
    /// What the scenario is for, in one line, so a failing report says why the
    /// number matters rather than only that it moved.
    pub asks: &'static str,
    /// Side of the analysed square, metres.
    pub side_m: f64,
    /// Which bands the profile carries. See [`BandSet`].
    pub bands: BandSet,
    /// The state the ground is held at.
    pub compaction: f32,
    pub moisture: f32,
}

/// Which of the loam's relief bands a scenario keeps.
///
/// Isolating a band is the whole point of a laboratory: with three bands present
/// a spectrum shows three peaks and a failure in the middle one is a peak that
/// moved slightly, which is exactly the kind of thing that gets argued about. With
/// one band present it is a peak that is in the wrong place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandSet {
    /// No relief at all: the control that proves the analysis reports zero for
    /// a flat surface rather than reporting its own noise floor.
    Flat,
    /// The coarsest band alone: clods.
    CoarseOnly,
    /// The middle band alone: crumbs.
    CrumbOnly,
    /// The finest band alone: grain.
    GrainOnly,
    /// Everything the profile declares.
    FullLadder,
}

/// Every laboratory, in the order they are worth reading.
///
/// Appended to, never repurposed — see the module note.
pub const GROUND_SCENARIOS: &[GroundScenario] = &[
    GroundScenario {
        name: "ground_flat_card",
        asks: "does the analysis report zero roughness for a flat surface, or its own noise floor?",
        side_m: 1.0,
        bands: BandSet::Flat,
        compaction: 0.0,
        moisture: 0.0,
    },
    GroundScenario {
        name: "ground_band_coarse_only",
        asks: "does the clod band come out at the wavelength and amplitude it declares?",
        side_m: 1.0,
        bands: BandSet::CoarseOnly,
        compaction: 0.0,
        moisture: 0.0,
    },
    GroundScenario {
        name: "ground_band_crumb_only",
        asks: "same, for the middle band, where a hidden octave would show as energy below it",
        side_m: 0.5,
        bands: BandSet::CrumbOnly,
        compaction: 0.0,
        moisture: 0.0,
    },
    GroundScenario {
        name: "ground_band_grain_only",
        asks: "same, for the finest band the lattice can carry at all",
        side_m: 0.25,
        bands: BandSet::GrainOnly,
        compaction: 0.0,
        moisture: 0.0,
    },
    GroundScenario {
        name: "ground_band_full_ladder",
        asks: "with every band present, is each one's energy still where it belongs?",
        side_m: 1.0,
        bands: BandSet::FullLadder,
        compaction: 0.0,
        moisture: 0.0,
    },
    GroundScenario {
        name: "ground_compaction_sweep",
        asks: "does compaction flatten the coarse band harder than the fine one, as declared?",
        side_m: 1.0,
        bands: BandSet::FullLadder,
        compaction: 1.0,
        moisture: 0.0,
    },
    GroundScenario {
        name: "ground_moisture_sweep",
        asks: "does saturation flatten relief without changing which scales carry it?",
        side_m: 1.0,
        bands: BandSet::FullLadder,
        compaction: 0.0,
        moisture: 1.0,
    },
];

/// Find a scenario by name.
pub fn scenario(name: &str) -> Option<&'static GroundScenario> {
    GROUND_SCENARIOS.iter().find(|s| s.name == name)
}

/// The loam every laboratory is built from.
///
/// The shipped `compacted_loam` measurements, in code rather than read from the
/// asset. The laboratories test the *machinery* — does a declared band come out
/// at its declared wavelength — and a laboratory that broke because somebody
/// retuned a shipped soil would be reporting the wrong thing.
pub fn loam_profile() -> GroundMaterialProfile {
    GroundMaterialProfile {
        key: GroundProfileKey::new("bench_loam").expect("a valid key"),
        shader: terrain_core::ids::AppearanceKey::new("surface.ground").expect("a valid key"),
        display_name: "Benchmark loam".into(),
        optics: GroundOptics {
            dry_palette: Palette {
                low: [0.0090, 0.0079, 0.0074],
                mid: [0.0545, 0.0343, 0.0222],
                high: [0.2420, 0.1550, 0.0905],
            },
            wet: WetResponse {
                wet_mid: [0.0210, 0.0116, 0.0058],
                roughness_wet: 0.22,
                saturation_flattening: 0.45,
                film_ior: 1.33,
            },
            roughness_dry: Span {
                low: 0.62,
                high: 0.88,
            },
            ior: 1.5,
            // The colour fields, at the shipped loam's measured settings. They
            // do not move relief and so do not affect the topography half, but
            // they do reach the optics sweep, so leaving them at zero would
            // measure a soil nothing renders.
            region_wavelength_m: 1.6,
            region_strength: 0.35,
            patch_wavelength_m: 0.28,
            patch_strength: 0.22,
            grain_strength: 0.12,
        },
        structure: GroundStructure {
            bands: vec![
                ReliefBand {
                    wavelength_m: 0.050,
                    amplitude_m: 0.0165,
                    shape: AggregateShape::Rounded,
                    compaction_response: 0.75,
                    clustered: true,
                },
                ReliefBand {
                    wavelength_m: 0.014,
                    amplitude_m: 0.0046,
                    shape: AggregateShape::Rounded,
                    compaction_response: 0.45,
                    clustered: false,
                },
                ReliefBand {
                    wavelength_m: 0.004,
                    amplitude_m: 0.0013,
                    shape: AggregateShape::Angular,
                    compaction_response: 0.20,
                    clustered: false,
                },
            ],
            cohesion: 0.55,
            cluster_wavelength_m: 0.42,
            cluster_strength: 0.35,
        },
        scatter: GroundScatter {
            grit_per_m2: 0.0,
            pebble_per_m2: 0.0,
            fragment_radius_m: Span {
                low: 0.002,
                high: 0.008,
            },
        },
        ripples: None,
        cracks: None,
        // Bare earth. A laboratory measuring a relief band has no business
        // growing anything on it.
        vegetation_affinity: 0.0,
    }
}

/// The loam with only the bands a scenario asked for.
pub fn profile_for(scenario: &GroundScenario) -> Arc<GroundMaterialProfile> {
    let mut profile = loam_profile();
    let all = profile.structure.bands.clone();
    profile.structure.bands = match scenario.bands {
        BandSet::Flat => Vec::new(),
        BandSet::CoarseOnly => vec![all[0]],
        BandSet::CrumbOnly => vec![all[1]],
        BandSet::GrainOnly => vec![all[2]],
        BandSet::FullLadder => all,
    };
    Arc::new(profile)
}

/// The band a scenario is primarily about, if it is about one.
pub fn subject_band(scenario: &GroundScenario) -> Option<ReliefBand> {
    let all = loam_profile().structure.bands;
    match scenario.bands {
        BandSet::Flat | BandSet::FullLadder => None,
        BandSet::CoarseOnly => Some(all[0]),
        BandSet::CrumbOnly => Some(all[1]),
        BandSet::GrainOnly => Some(all[2]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_name_is_unique() {
        // A scenario name is the key its baseline is filed under. Two scenarios
        // sharing one would overwrite each other's history and the comparison
        // table would go on reporting deltas as though they meant something.
        let mut names: Vec<&str> = GROUND_SCENARIOS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    #[test]
    fn the_benchmark_loam_is_a_valid_profile() {
        // A laboratory built on a profile that would be rejected by validation
        // is measuring something no document could ever produce.
        let problems = loam_profile().problems();
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn its_bands_descend_and_stay_inside_their_own_wavelengths() {
        // The two rules the profile schema enforces, checked here as well
        // because this profile is written in code and never passes through the
        // parser that would otherwise catch a typo.
        let bands = loam_profile().structure.bands;
        for pair in bands.windows(2) {
            assert!(
                pair[0].wavelength_m > pair[1].wavelength_m,
                "bands are not strictly descending"
            );
        }
        for band in &bands {
            assert!(
                band.amplitude_m <= band.wavelength_m * 0.75,
                "a {}m band with {}m of amplitude is a spike field",
                band.wavelength_m,
                band.amplitude_m
            );
        }
    }

    #[test]
    fn isolating_a_band_leaves_exactly_that_band() {
        for (set, expected) in [
            (BandSet::Flat, 0),
            (BandSet::CoarseOnly, 1),
            (BandSet::CrumbOnly, 1),
            (BandSet::GrainOnly, 1),
            (BandSet::FullLadder, 3),
        ] {
            let scenario = GroundScenario {
                name: "x",
                asks: "",
                side_m: 1.0,
                bands: set,
                compaction: 0.0,
                moisture: 0.0,
            };
            assert_eq!(profile_for(&scenario).structure.bands.len(), expected);
        }
    }

    #[test]
    fn an_isolated_scenario_names_the_band_it_is_about() {
        let coarse = scenario("ground_band_coarse_only").expect("the scenario exists");
        let band = subject_band(coarse).expect("a coarse scenario has a subject");
        assert_eq!(band.wavelength_m, 0.050);
        // And the full ladder is about all of them, so it names none.
        assert!(subject_band(scenario("ground_band_full_ladder").expect("exists")).is_none());
    }

    #[test]
    fn every_scenario_says_what_it_asks() {
        // A failing report should say why the number matters, not only that it
        // moved. An empty `asks` is a scenario nobody can act on.
        for scenario in GROUND_SCENARIOS {
            assert!(
                scenario.asks.len() > 20,
                "{} does not say what it asks",
                scenario.name
            );
            assert!(scenario.side_m > 0.0);
        }
    }
}

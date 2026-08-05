//! Running one laboratory, end to end.
//!
//! Sample the evaluator, detrend, measure, compare two windows, and reach a
//! verdict. The order matters in one place: **composability is checked against
//! the raw samples, before any statistic is taken.** A comparison of two
//! summaries would pass whenever both windows happened to be equally rough,
//! which is most of the time even when they disagree sample for sample.

use std::sync::Arc;

use terrain_core::coords::WorldPoint;
use terrain_generators::ground::GroundEvaluator;
use terrain_generators::transition::TransitionProfile;
use terrain_scene::field::{FieldGridSpec, TerrainFieldStack};

use super::field::{AnalysisGrid, GroundField};
use super::report::{
    BenchmarkVerdict, ComparisonMetric, GateResult, GroundBenchmarkReport, SCHEMA_VERSION,
    SourceIdentity,
};
use super::scenarios::{BandSet, GroundScenario, profile_for, subject_band};
use super::{optics, psd, semivariogram, topography};

/// How many analysis samples to put across the finest band being measured.
///
/// Eight, not the generator's four. The generator's four is the threshold at
/// which a band is *representable*; measuring at exactly that rate would put
/// every band's energy at the Nyquist edge of the analysis, where the window's
/// own rolloff is strongest. Eight leaves an octave of headroom, and the alias
/// gate is what checks nothing spilled past it.
pub const ANALYSIS_SAMPLES_PER_WAVELENGTH: f64 = 8.0;

/// The seed every laboratory runs at unless a sweep says otherwise.
pub const DEFAULT_SEED: u64 = 0x5a17_e33b_0c9d_2f14;

/// The most analysis samples across one axis.
///
/// A full ladder resolves its finest band at half a millimetre, and a metre of
/// that is four million samples — twenty seconds for one scenario, which is a
/// suite nobody runs on a commit. The window is shrunk instead of the spacing
/// coarsened, because coarsening would alias the very band the spacing was
/// derived from; shrinking only reduces how many coarse wavelengths fit, and
/// seven of them is enough to measure one.
///
/// The effective side is reported, so a scenario that was shrunk says so rather
/// than quietly measuring a smaller patch than it declared.
pub const MAX_SAMPLES_PER_AXIS: usize = 640;

/// The side a scenario is actually analysed over.
pub fn effective_side_m(scenario: &GroundScenario) -> f64 {
    let spacing = spacing_for(scenario);
    scenario.side_m.min(MAX_SAMPLES_PER_AXIS as f64 * spacing)
}

/// Build the evaluator one scenario describes.
///
/// Flat authored ground, one material, one profile. Flat because a macro slope
/// would dominate every roughness measurement, and one material because a
/// boundary would put two different band sets into the same window.
fn evaluator(scenario: &GroundScenario, seed: u64, side_m: f64, spacing_m: f64) -> GroundEvaluator {
    let bounds = terrain_core::coords::WorldRect::centred(WorldPoint::ORIGIN, side_m * 1.5);
    let fields = Arc::new(TerrainFieldStack::flat(FieldGridSpec::covering(
        bounds, spacing_m,
    )));
    GroundEvaluator::for_benchmark(
        fields,
        TransitionProfile::SMOOTH,
        seed,
        vec![profile_for(scenario)],
        spacing_m as f32,
        scenario.compaction,
        scenario.moisture,
    )
}

/// The lattice spacing a scenario needs.
fn spacing_for(scenario: &GroundScenario) -> f64 {
    let profile = profile_for(scenario);
    profile
        .structure
        .bands
        .iter()
        .map(|band| band.wavelength_m as f64 / ANALYSIS_SAMPLES_PER_WAVELENGTH)
        .fold(f64::INFINITY, f64::min)
        // A flat card has no band to derive from, so it measures at the coarse
        // band's rate: enough to prove the analysis reports zero rather than
        // its own noise floor at the resolution the others run at.
        .min(0.05 / ANALYSIS_SAMPLES_PER_WAVELENGTH)
}

/// Run one laboratory and reach a verdict.
pub fn run(scenario: &GroundScenario, seed: u64) -> GroundBenchmarkReport {
    let spacing = spacing_for(scenario);
    let side = effective_side_m(scenario);
    let ground = evaluator(scenario, seed, side, spacing);
    let profile = profile_for(scenario);

    let grid = AnalysisGrid::square(WorldPoint::ORIGIN, side, spacing);
    let field = GroundField::sample(&ground, grid, 1);

    // The margin excludes the ring where derivative estimates ran off the end
    // and the spectral window saw a discontinuity the terrain never had.
    let coarsest = profile
        .structure
        .bands
        .first()
        .map(|band| band.wavelength_m as f64)
        .unwrap_or(0.05);
    let margin = GroundField::margin_for(spacing, coarsest);
    let (height, columns, rows) = GroundField::interior(&field.height_m, &grid, margin);
    let (cavity, _, _) = GroundField::interior(&field.cavity, &grid, margin);

    let lags: Vec<f64> = profile
        .structure
        .bands
        .iter()
        .flat_map(|band| {
            let w = band.wavelength_m as f64;
            [w * 0.25, w, w * 2.0]
        })
        .collect();
    let topography = topography::measure(&height, Some(&cavity), columns, rows, spacing, &lags);

    let (residual, _) = topography::detrend(&height, columns, rows, spacing);
    let (cropped, spectral_side, _) = psd::crop_to_power_of_two(&residual, columns, rows);
    let bands: Vec<psd::BandQuery> = profile
        .structure
        .bands
        .iter()
        .enumerate()
        .map(|(index, band)| psd::BandQuery::new(format!("band{index}"), band.wavelength_m as f64))
        .collect();
    let cropped_mean = cropped.iter().map(|v| *v as f64).sum::<f64>() / cropped.len().max(1) as f64;
    let cropped_variance = cropped
        .iter()
        .map(|v| (*v as f64 - cropped_mean).powi(2))
        .sum::<f64>()
        / cropped.len().max(1) as f64;
    let spectrum = psd::measure(&cropped, spectral_side, spectral_side, spacing, &bands);

    let max_lag = ((coarsest * 2.0 / spacing).ceil() as usize).min(columns / 3);
    let semivariograms: Vec<_> = [0.0, std::f64::consts::FRAC_PI_2]
        .into_iter()
        .map(|direction| {
            semivariogram::measure(&residual, columns, rows, spacing, direction, max_lag)
        })
        .collect();

    let optics = optics::measure(&profile, 9);
    let composability = compare_windows(scenario, seed, spacing);

    let gates = gates(
        scenario,
        &topography,
        &spectrum,
        &semivariograms,
        &optics,
        &composability,
        cropped_variance,
    );

    let mut counts = std::collections::BTreeMap::new();
    counts.insert("analysis_samples".to_string(), columns * rows);
    counts.insert(
        "spectral_samples".to_string(),
        spectral_side * spectral_side,
    );
    counts.insert("relief_bands".to_string(), profile.structure.bands.len());

    GroundBenchmarkReport {
        schema_version: SCHEMA_VERSION,
        source: SourceIdentity {
            scenario: scenario.name.to_string(),
            seed_hex: format!("{seed:016x}"),
            profile_digest: profile_digest(&profile),
            generator_version: crate::fingerprint::GENERATOR_VERSION,
        },
        scenario_asks: scenario.asks.to_string(),
        grid,
        topography,
        spectrum,
        semivariograms,
        optics,
        composability,
        counts,
        verdict: BenchmarkVerdict::from(gates),
    }
}

/// Compare a whole window against two half windows that tile it.
///
/// The seam gate, and the reason it compares raw samples rather than summaries:
/// two windows can be equally rough and still disagree at every point, which is
/// exactly what a window-dependent generator produces.
fn compare_windows(scenario: &GroundScenario, seed: u64, spacing: f64) -> Vec<ComparisonMetric> {
    let side = effective_side_m(scenario);
    let ground = evaluator(scenario, seed, side, spacing);
    let whole = AnalysisGrid::square(WorldPoint::ORIGIN, side, spacing);
    let half = AnalysisGrid::covering(
        terrain_core::coords::WorldRect::new(
            whole.position(0, 0),
            whole.position(whole.columns / 2, whole.rows - 1),
        ),
        spacing,
    );

    let whole_field = GroundField::sample(&ground, whole, 1);
    let half_field = GroundField::sample(&ground, half, 1);

    let planes: [(&str, &Vec<f32>, &Vec<f32>); 4] = [
        ("height_m", &whole_field.height_m, &half_field.height_m),
        (
            "displacement_m",
            &whole_field.displacement_m,
            &half_field.displacement_m,
        ),
        ("cavity", &whole_field.cavity, &half_field.cavity),
        ("moisture", &whole_field.moisture, &half_field.moisture),
    ];

    planes
        .into_iter()
        .map(|(key, big, small)| {
            let mut max_abs = 0.0f64;
            let mut sum_squared = 0.0f64;
            let mut compared = 0usize;
            let mut exact = true;
            for row in 0..half.rows.min(whole.rows) {
                for column in 0..half.columns.min(whole.columns) {
                    // Both grids are snapped to the same global lattice, so the
                    // same `(column, row)` offset from each origin is the same
                    // world point — no interpolation, and therefore no
                    // interpolation error to explain away.
                    let a = big[row * whole.columns + column];
                    let b = small[row * half.columns + column];
                    if a.to_bits() != b.to_bits() {
                        exact = false;
                    }
                    let difference = (a as f64 - b as f64).abs();
                    max_abs = max_abs.max(difference);
                    sum_squared += difference * difference;
                    compared += 1;
                }
            }
            ComparisonMetric {
                key: key.to_string(),
                max_abs,
                rms: (sum_squared / compared.max(1) as f64).sqrt(),
                bit_exact: exact,
            }
        })
        .collect()
}

/// The gates, and the reasoning behind each limit.
#[allow(clippy::too_many_arguments)]
fn gates(
    scenario: &GroundScenario,
    topography: &topography::TopographyMetrics,
    spectrum: &psd::SpectralMetrics,
    semivariograms: &[semivariogram::Semivariogram],
    optics: &optics::OpticsMetrics,
    composability: &[ComparisonMetric],
    reference_variance: f64,
) -> Vec<GateResult> {
    let mut gates = Vec::new();

    // The self-test. If the PSD is wrong by a constant factor, no band energy
    // below it means anything, so this is checked first and hard.
    gates.push(GateResult::at_most(
        "psd_parseval",
        spectrum.parseval_relative_error,
        1.0e-5,
        "PSD integral against the measured variance",
    ));

    // Two windows that address the same lattice must agree to the bit. A small
    // difference is not "close enough": in a deterministic field the only thing
    // that can make two windows disagree is a dependence on the window.
    for comparison in composability {
        gates.push(GateResult::holds(
            &format!("composable_{}", comparison.key),
            comparison.bit_exact,
            &format!(
                "{}: whole and half windows differ by at most {:.3e}",
                comparison.key, comparison.max_abs
            ),
        ));
    }

    // A deterministic field has no microscale variation, so a nugget is
    // aliasing, a discontinuity, or a bad tier handoff.
    for curve in semivariograms {
        if curve.sill_m2 <= 0.0 {
            continue;
        }
        gates.push(
            GateResult::at_most(
                &format!("nugget_fraction_{:.2}", curve.direction_rad),
                curve.nugget_m2 / curve.sill_m2,
                0.05,
                "nugget as a fraction of the sill",
            )
            .advisory(),
        );
    }

    // Energy above what the lattice can carry is aliasing by definition.
    gates.push(GateResult::at_most(
        "alias_energy",
        spectrum.alias_energy_fraction,
        0.02,
        "energy above the representable cutoff",
    ));

    match scenario.bands {
        BandSet::Flat => {
            // The control. A flat card must report zero roughness, not the
            // analysis's own noise floor.
            gates.push(GateResult::at_most(
                "flat_card_is_flat",
                topography.sq_m,
                1.0e-6,
                "Sq of a card with no relief bands",
            ));
            gates.push(GateResult::not_applicable(
                "band_dominant_wavelength",
                "a flat card declares no bands",
            ));
        }
        _ => {
            // An axis-aligned lattice leaking through is the quilted-square
            // failure, and no scalar statistic sees it. A field built from
            // golden-angle-turned noise puts very little energy on the axes.
            gates.push(
                GateResult::at_most(
                    "axis_grid_energy",
                    spectrum.axis_grid_energy_fraction,
                    0.15,
                    "energy sitting exactly on the frequency axes",
                )
                .advisory(),
            );

            if let Some(band) = subject_band(scenario)
                && let Some(measured) = spectrum.bands.first()
            {
                // Within a factor of two of the declared wavelength. Loose,
                // deliberately: a band is a noise field with a characteristic
                // scale, not a sine, so its spectral centroid sits below its
                // nominal wavelength by an amount the shape transform decides.
                // The gate is here to catch a band that came out at a *tenth*
                // of what it declared, which is what a hidden octave produces.
                let ratio = measured.dominant_wavelength_m / band.wavelength_m as f64;
                gates.push(GateResult::within(
                    "band_dominant_wavelength",
                    ratio,
                    0.4,
                    2.5,
                    "measured over declared wavelength",
                ));
            } else {
                gates.push(GateResult::not_applicable(
                    "band_dominant_wavelength",
                    "the full ladder is about every band rather than one",
                ));
            }

            // Cavity is occlusion between crumbs, so it must be high where the
            // ground is low. A shader whose tone comes from noise uncorrelated
            // with its own relief reads as painted paper however much geometry
            // the mesh carries — this is the number that says which one it is.
            gates.push(GateResult::at_most(
                "cavity_tracks_hollows",
                topography.cavity_height_spearman,
                -0.3,
                "cavity against height (Spearman, wants strongly negative)",
            ));
        }
    }

    // The wet response, from the profile rather than from pixels.
    gates.push(GateResult::holds(
        "wet_albedo_monotone",
        optics.moisture_albedo_monotone,
        "reflectance falls monotonically as the ground wets",
    ));
    gates.push(GateResult::holds(
        "wet_endpoints_declared",
        optics.endpoints_match_declaration,
        "the sweep hits the declared dry and wet mid tones at its ends",
    ));
    gates.push(GateResult::holds(
        "wet_is_not_a_grey_dimmer",
        optics.hue_ratio_span > 1.0e-4,
        "the channel ratio moves across the sweep rather than staying flat",
    ));

    // A measurement over an empty field would report every gate green.
    gates.push(GateResult::holds(
        "measured_something",
        reference_variance > 0.0 || scenario.bands == BandSet::Flat,
        "the analysed window has some relief in it",
    ));

    gates
}

/// A digest of what the profile declared, so a report says which soil it read.
fn profile_digest(profile: &terrain_core::ground_material::GroundMaterialProfile) -> String {
    use terrain_core::digest::Digest;
    let mut digest = Digest::for_domain("ground-profile");
    digest.str(profile.key.as_str());
    for band in &profile.structure.bands {
        digest
            .f32(band.wavelength_m)
            .f32(band.amplitude_m)
            .tag(band.shape as u8)
            .f32(band.compaction_response);
    }
    for channel in profile.optics.dry_palette.mid {
        digest.f32(channel);
    }
    for channel in profile.optics.wet.wet_mid {
        digest.f32(channel);
    }
    digest.finish().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ground::scenarios::GROUND_SCENARIOS;

    #[test]
    fn every_laboratory_runs_and_reaches_a_verdict() {
        // The whole harness, exercised. A scenario that panicked or produced a
        // non-finite would be discovered here rather than in a CI job at three
        // in the morning.
        for scenario in GROUND_SCENARIOS {
            let report = run(scenario, DEFAULT_SEED);
            assert_eq!(report.source.scenario, scenario.name);
            assert!(
                report.topography.sq_m.is_finite(),
                "{}: Sq is not finite",
                scenario.name
            );
            assert!(!report.verdict.gates.is_empty(), "{}", scenario.name);
            // Every gate's message says something, so a failing report is
            // actionable rather than merely red.
            for gate in &report.verdict.gates {
                assert!(!gate.message.is_empty(), "{}: {}", scenario.name, gate.key);
            }
        }
    }

    #[test]
    fn the_flat_card_is_flat() {
        // The control that proves the analysis reports zero for a flat surface
        // rather than its own noise floor. If this fails, every other Sq in the
        // suite is inflated by the same amount.
        let flat = crate::ground::scenarios::scenario("ground_flat_card").expect("exists");
        let report = run(flat, DEFAULT_SEED);
        assert!(
            report.topography.sq_m < 1.0e-6,
            "a flat card measured {} m of roughness",
            report.topography.sq_m
        );
    }

    #[test]
    fn a_cloddy_card_is_not_flat() {
        // Guards the test above from being vacuous: if the evaluator produced
        // nothing at all, both would pass.
        let coarse = crate::ground::scenarios::scenario("ground_band_coarse_only").expect("exists");
        let report = run(coarse, DEFAULT_SEED);
        assert!(
            report.topography.sq_m > 1.0e-4,
            "a card with a 5 cm clod band measured only {} m of roughness",
            report.topography.sq_m
        );
    }

    #[test]
    fn two_windows_over_one_ground_agree_to_the_bit() {
        // The seam property, which is the whole framework's central claim,
        // measured rather than asserted. Compared over raw samples: two windows
        // can be equally rough and still disagree at every point.
        for scenario in GROUND_SCENARIOS {
            let report = run(scenario, DEFAULT_SEED);
            for comparison in &report.composability {
                assert!(
                    comparison.bit_exact,
                    "{}: {} differs by up to {:.3e} between a whole and a half window",
                    scenario.name, comparison.key, comparison.max_abs
                );
            }
        }
    }

    #[test]
    fn compaction_flattens_the_ground_it_is_declared_to_flatten() {
        // The state response, end to end. Both scenarios carry the full ladder;
        // only the compaction differs, and the profile says every band responds
        // to it.
        let loose = run(
            crate::ground::scenarios::scenario("ground_band_full_ladder").expect("exists"),
            DEFAULT_SEED,
        );
        let packed = run(
            crate::ground::scenarios::scenario("ground_compaction_sweep").expect("exists"),
            DEFAULT_SEED,
        );
        assert!(
            packed.topography.sq_m < loose.topography.sq_m,
            "compaction did not flatten: {} against {}",
            packed.topography.sq_m,
            loose.topography.sq_m
        );
    }

    #[test]
    fn a_report_serialises_to_parseable_json() {
        let report = run(
            crate::ground::scenarios::scenario("ground_band_coarse_only").expect("exists"),
            DEFAULT_SEED,
        );
        let json = report.to_json();
        let text = serde_json::to_string(&json).expect("the report serialises");
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("the report parses back");
        assert_eq!(parsed["schema_version"], SCHEMA_VERSION);
        assert!(!text.contains("NaN"), "a NaN reached the JSON");
        assert!(!text.contains("Infinity"), "an infinity reached the JSON");
    }

    #[test]
    fn the_table_names_every_gate_that_did_not_pass() {
        let report = run(
            crate::ground::scenarios::scenario("ground_band_full_ladder").expect("exists"),
            DEFAULT_SEED,
        );
        let table = report.to_table();
        for gate in &report.verdict.gates {
            if gate.status == crate::ground::report::GateStatus::Pass {
                continue;
            }
            assert!(
                table.contains(&gate.key),
                "the table does not mention the failing gate {}",
                gate.key
            );
        }
    }

    #[test]
    fn running_the_same_laboratory_twice_gives_the_same_report() {
        // A benchmark that is not reproducible is not a benchmark.
        let scenario =
            crate::ground::scenarios::scenario("ground_band_crumb_only").expect("exists");
        let first = run(scenario, DEFAULT_SEED);
        let second = run(scenario, DEFAULT_SEED);
        assert_eq!(first.topography, second.topography);
        assert_eq!(first.spectrum, second.spectrum);
        assert_eq!(first.verdict, second.verdict);
    }
}

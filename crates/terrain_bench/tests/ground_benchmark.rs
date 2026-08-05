//! The ground benchmark, pinned — and honest about what it does not yet fill.
//!
//! Two jobs. The first is the ordinary one: a committed table of what every
//! laboratory currently measures, so that a change to the soil has a before and
//! an after rather than an opinion.
//!
//! The second is the one that is easy to skip. `docs/spec/ground-benchmark-report.schema.json`
//! is the durable interchange shape, and this build fills a *subset* of it. That
//! subset is enumerated here rather than left to be discovered, because the
//! failure mode of an unenumerated gap is a downstream tool that reads
//! `report["performance"]["total_median_ms"]`, finds nothing, and reports zero
//! milliseconds.

use std::collections::BTreeSet;

use terrain_bench::ground::{self, GateStatus};

/// Where the committed table lives.
fn baseline_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ground")
        .join("baseline.txt")
}

fn schema_path() -> std::path::PathBuf {
    terrain_bench::documents::in_repository("docs/spec/ground-benchmark-report.schema.json")
}

fn render() -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("// The ground benchmark, pinned. Regenerate with:\n");
    out.push_str(
        "//   TERRAIN_ACCEPT_GROUND=1 cargo test -p terrain_bench --test ground_benchmark\n",
    );
    out.push_str("// Every number here is a claim about the soil. A row that moves\n");
    out.push_str("// without a phase behind it is a regression.\n\n");
    for scenario in ground::GROUND_SCENARIOS {
        let report = ground::run(scenario, ground::DEFAULT_SEED);
        let _ = writeln!(out, "[{}]", scenario.name);
        let _ = writeln!(out, "verdict = {}", report.verdict.status.name());
        let _ = writeln!(
            out,
            "grid = {}x{} at {:.5} m",
            report.grid.columns, report.grid.rows, report.grid.spacing_m
        );
        let _ = writeln!(
            out,
            "height = Sq {:.6} m, Sa {:.6} m, skew {:+.3}, kurtosis {:.3}",
            report.topography.sq_m,
            report.topography.sa_m,
            report.topography.ssk,
            report.topography.sku
        );
        let _ = writeln!(
            out,
            "cavity = pearson {:+.3}, spearman {:+.3}",
            report.topography.cavity_height_pearson, report.topography.cavity_height_spearman
        );
        let _ = writeln!(
            out,
            "spectrum = axis {:.3}, alias {:.3}, anisotropy {:.3}",
            report.spectrum.axis_grid_energy_fraction,
            report.spectrum.above_policy_cutoff_fraction,
            report.spectrum.anisotropy
        );
        for band in &report.spectrum.bands {
            let _ = writeln!(
                out,
                "  {} = declared {:.4} m, dominant {:.4} m, {:.3} of energy",
                band.key, band.declared_wavelength_m, band.dominant_wavelength_m, band.energy_share
            );
        }
        for gate in &report.verdict.gates {
            if gate.status == GateStatus::Pass {
                continue;
            }
            let _ = writeln!(out, "  [{}] {}", gate.status.name(), gate.key);
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_pinned_ground_benchmark_is_unchanged() {
    let text = render();
    let path = baseline_path();

    if std::env::var_os("TERRAIN_ACCEPT_GROUND").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture directory is writable");
        }
        std::fs::write(&path, &text).expect("the baseline is writable");
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "no committed baseline at {}: {error}\n\
             create one with TERRAIN_ACCEPT_GROUND=1",
            path.display()
        )
    });
    if committed.replace("\r\n", "\n") != text {
        panic!(
            "the ground moved.\n\n--- committed\n{committed}\n--- measured\n{text}\n\
             If a phase meant to move these numbers, accept with \
             TERRAIN_ACCEPT_GROUND=1 and say which phase in the commit."
        );
    }
}

#[test]
fn no_laboratory_fails_a_gate() {
    // Needs-review is allowed; failure is not. The distinction is the whole
    // reason both statuses exist: a threshold that is still a bootstrap guess
    // must not be able to block a commit, or it gets deleted rather than
    // tightened.
    for scenario in ground::GROUND_SCENARIOS {
        let report = ground::run(scenario, ground::DEFAULT_SEED);
        let failures: Vec<&str> = report
            .verdict
            .gates
            .iter()
            .filter(|g| g.status == GateStatus::Fail)
            .map(|g| g.key.as_str())
            .collect();
        assert!(
            failures.is_empty(),
            "{}: {} failed",
            scenario.name,
            failures.join(", ")
        );
    }
}

/// The schema keys this build does not yet fill, and why.
///
/// Enumerated rather than discovered. A downstream tool reading a key that is
/// simply absent gets `null` and reports zero, which is indistinguishable from
/// a measurement of zero — so the gap has to be a fact somebody wrote down.
const UNFILLED: &[(&str, &str)] = &[
    (
        "relief_plan",
        "the fingerprinted tier assignment arrives with the relief-plan phase; \
         until then no band has a recorded representation owner",
    ),
    ("cracks", "no shipped profile declares a crack network yet"),
    ("ripples", "no shipped profile declares ripples yet"),
    (
        "render",
        "the render half needs Blender and runs on the visual gate rather than \
         on every commit",
    ),
];

#[test]
fn the_report_fills_what_it_claims_and_the_gap_is_written_down() {
    // The drift check. If a schema key starts being filled, or stops, this test
    // says so — which is what keeps the honest list above from becoming a stale
    // list of excuses.
    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(schema_path()).expect("the companion schema is in the tree"),
    )
    .expect("the schema is valid JSON");

    let required: BTreeSet<String> = schema["required"]
        .as_array()
        .expect("the schema declares required keys")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();

    let report = ground::run(
        ground::scenarios::scenario("ground_band_coarse_only").expect("the scenario exists"),
        ground::DEFAULT_SEED,
    );
    let json = report.to_json();
    let filled: BTreeSet<String> = json
        .as_object()
        .expect("a report is an object")
        .keys()
        .cloned()
        .collect();

    let unfilled: BTreeSet<String> = UNFILLED.iter().map(|(key, _)| key.to_string()).collect();
    let missing: Vec<&String> = required
        .iter()
        .filter(|key| !filled.contains(*key) && !unfilled.contains(*key))
        .collect();
    assert!(
        missing.is_empty(),
        "the schema requires {missing:?}, the report does not fill them, and the \
         UNFILLED list does not say why"
    );

    let stale: Vec<&String> = unfilled
        .iter()
        .filter(|key| filled.contains(*key))
        .collect();
    assert!(
        stale.is_empty(),
        "{stale:?} are now filled, but the UNFILLED list still explains why they are not"
    );
}

#[test]
fn every_report_serialises_to_json_that_parses_back() {
    // A NaN serialises as bare `NaN`, which is not JSON. A CI job reading the
    // report would fail to parse it and report a broken file rather than the
    // broken measurement that produced it.
    for scenario in ground::GROUND_SCENARIOS {
        let report = ground::run(scenario, ground::DEFAULT_SEED);
        let text = serde_json::to_string(&report.to_json()).expect("serialises");
        serde_json::from_str::<serde_json::Value>(&text).expect("parses back");
        assert!(
            !text.contains("NaN"),
            "{}: a NaN reached the JSON",
            scenario.name
        );
        assert!(
            !text.contains("Infinity"),
            "{}: an infinity reached the JSON",
            scenario.name
        );
    }
}

#[test]
fn the_moisture_sweep_flattens_by_the_profiles_declared_amount() {
    // The state response, checked against the profile rather than against a
    // previous run. `saturation_flattening` is 0.45, so a saturated ground
    // should carry about 55% of the dry ground's relief.
    let dry = ground::run(
        ground::scenarios::scenario("ground_band_full_ladder").expect("exists"),
        ground::DEFAULT_SEED,
    );
    let wet = ground::run(
        ground::scenarios::scenario("ground_moisture_sweep").expect("exists"),
        ground::DEFAULT_SEED,
    );
    let declared = 1.0
        - ground::scenarios::loam_profile()
            .optics
            .wet
            .saturation_flattening as f64;
    let measured = wet.topography.sq_m / dry.topography.sq_m;
    assert!(
        (measured - declared).abs() < 0.05,
        "saturation left {measured:.3} of the relief; the profile declares {declared:.3}"
    );
}

#[test]
fn compaction_flattens_more_than_saturation_does() {
    // Not a tuning claim — a check that the two responses are distinct. The
    // loam's bands declare compaction responses of 0.75, 0.45 and 0.20, which
    // together bite harder than one saturation flattening of 0.45.
    let dry = ground::run(
        ground::scenarios::scenario("ground_band_full_ladder").expect("exists"),
        ground::DEFAULT_SEED,
    );
    let packed = ground::run(
        ground::scenarios::scenario("ground_compaction_sweep").expect("exists"),
        ground::DEFAULT_SEED,
    );
    let wet = ground::run(
        ground::scenarios::scenario("ground_moisture_sweep").expect("exists"),
        ground::DEFAULT_SEED,
    );
    assert!(
        packed.topography.sq_m < wet.topography.sq_m,
        "compaction left {} and saturation left {}; compaction should bite harder",
        packed.topography.sq_m,
        wet.topography.sq_m
    );
    assert!(packed.topography.sq_m < dry.topography.sq_m);
}

#[test]
fn the_run_and_performance_sections_match_the_schema_they_claim() {
    // Filling a key is not the same as filling it correctly. The schema names
    // every field of these three sections as required, and a report that
    // supplied an object with two of them would satisfy the drift check above
    // and still be unreadable downstream.
    let report = ground::run(
        ground::scenarios::scenario("ground_band_coarse_only").expect("the scenario exists"),
        ground::DEFAULT_SEED,
    );
    let json = report.to_json();

    let run = json["run"].as_object().expect("run is an object");
    for key in ["run_id", "machine", "repetitions", "warmup_repetitions"] {
        assert!(run.contains_key(key), "run is missing {key}");
    }
    let machine = run["machine"].as_object().expect("machine is an object");
    for key in [
        "os",
        "arch",
        "cpu",
        "gpu",
        "rustc",
        "blender",
        "cycles_device",
    ] {
        let value = machine[key].as_str().unwrap_or_default();
        assert!(!value.is_empty(), "machine.{key} is empty");
    }
    // The renderer fields have to *say* there is no renderer rather than be
    // blank or plausible. A reader comparing two reports needs to be able to
    // tell "none, by design" from "nobody recorded it".
    for key in ["gpu", "blender", "cycles_device"] {
        assert!(
            machine[key].as_str().unwrap_or_default().contains("none"),
            "machine.{key} does not say there is no renderer: {}",
            machine[key]
        );
    }
    assert!(run["repetitions"].as_u64().unwrap_or(0) >= 1);

    let performance = json["performance"]
        .as_object()
        .expect("performance is an object");
    for key in ["total_median_ms", "total_p95_ms", "peak_bytes", "stages"] {
        assert!(
            performance.contains_key(key),
            "performance is missing {key}"
        );
    }
    let stages = performance["stages"]
        .as_array()
        .expect("stages is an array");
    assert!(
        stages.len() >= 5,
        "only {} stages timed — the analysis has more than that",
        stages.len()
    );
    for stage in stages {
        for key in ["stage", "median_ms", "mad_ms", "p95_ms", "peak_bytes"] {
            assert!(
                stage.get(key).is_some(),
                "a stage is missing {key}: {stage}"
            );
        }
        assert!(stage["median_ms"].as_f64().unwrap_or(-1.0) >= 0.0);
    }
    // The total decomposes into the parts a reader is shown. A total that did
    // not would leave "which stage got slower" unanswerable from the report.
    let summed: f64 = stages
        .iter()
        .map(|s| s["median_ms"].as_f64().unwrap_or(0.0))
        .sum();
    let total = performance["total_median_ms"].as_f64().unwrap_or(0.0);
    assert!(
        (total - summed).abs() < 1.0e-9,
        "the total is {total} and the stages sum to {summed}"
    );

    assert!(json["artifacts"].is_array(), "artifacts is not an array");
}

#[test]
fn a_timing_is_reported_beside_the_counters_it_has_to_be_read_against() {
    // The rule from `ground::performance`, asserted rather than left in a
    // comment: no speed claim is valid unless the compared runs have equal
    // content counts, and the only way a reader can check that is to have both
    // numbers in the same report.
    let report = ground::run(
        ground::scenarios::scenario("ground_band_coarse_only").expect("the scenario exists"),
        ground::DEFAULT_SEED,
    );
    for key in ["analysis_samples", "spectral_samples", "relief_bands"] {
        assert!(
            report.counts.contains_key(key),
            "the report times its stages but does not carry {key}"
        );
    }
    assert!(
        report.performance.total_median_ms > 0.0,
        "every stage measured zero milliseconds"
    );
}

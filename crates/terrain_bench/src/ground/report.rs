//! One benchmark run, as a durable record.
//!
//! ## Why the report is a format and not a printout
//!
//! A soil change that says "it looks better" is not evidence. The question the
//! benchmark exists to force is *why did the ground move* — and answering that a
//! month later needs the old numbers, in a shape a comparison tool can read,
//! filed under a name that still means what it meant.
//!
//! The Rust types here are the source of truth. `docs/spec/ground-benchmark-report.schema.json`
//! is the durable interchange shape, and the two drifting apart is a test failure
//! rather than a documentation problem: `tests/ground_benchmark.rs` reads the
//! real schema and checks that every key it requires is either filled or named
//! in a written-down list of what this build does not yet measure.
//!
//! ## Gates are pass, fail, or needs-review — never a score
//!
//! A single number lets a regression in one dimension be paid for by an
//! improvement in another, which is exactly what must not happen here: a band
//! that lost half its energy is not compensated for by a better colour match.
//! So every gate is separate, every gate names the value it saw and the limit it
//! was held to, and the verdict is the worst of them.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::optics::OpticsMetrics;
use super::psd::SpectralMetrics;
use super::semivariogram::Semivariogram;
use super::topography::TopographyMetrics;

/// The schema version this build writes.
pub const SCHEMA_VERSION: u32 = 1;

/// What one gate concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateStatus {
    Pass,
    Fail,
    /// Measured, outside the limit, and not automatically a failure.
    ///
    /// For the gates whose limits are bootstrap guesses rather than established
    /// thresholds. Marking those `Fail` would train readers to ignore failures,
    /// which is worse than the missing rigour.
    NeedsReview,
    /// The scenario cannot ask this question. A flat card has no band energy.
    NotApplicable,
}

impl GateStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NeedsReview => "needs_review",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// One thing checked, and what it saw.
#[derive(Clone, Debug, PartialEq)]
pub struct GateResult {
    pub key: String,
    pub status: GateStatus,
    /// What was measured. `None` when the gate did not apply.
    pub observed: Option<f64>,
    /// What it was held to.
    pub limit: Option<f64>,
    pub message: String,
}

impl GateResult {
    /// A gate whose observed value must not exceed a limit.
    pub fn at_most(key: &str, observed: f64, limit: f64, what: &str) -> Self {
        let status = if !observed.is_finite() {
            GateStatus::Fail
        } else if observed <= limit {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        };
        Self {
            key: key.to_string(),
            status,
            observed: Some(observed),
            limit: Some(limit),
            message: format!("{what}: {observed:.6} against a limit of {limit:.6}"),
        }
    }

    /// A gate whose observed value must fall inside a ratio band.
    pub fn within(key: &str, observed: f64, low: f64, high: f64, what: &str) -> Self {
        let status = if observed.is_finite() && observed >= low && observed <= high {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        };
        Self {
            key: key.to_string(),
            status,
            observed: Some(observed),
            limit: Some(high),
            message: format!("{what}: {observed:.6}, wanted {low:.4}..{high:.4}"),
        }
    }

    /// A gate that is simply true or false.
    pub fn holds(key: &str, held: bool, what: &str) -> Self {
        Self {
            key: key.to_string(),
            status: if held {
                GateStatus::Pass
            } else {
                GateStatus::Fail
            },
            observed: Some(if held { 1.0 } else { 0.0 }),
            limit: Some(1.0),
            message: what.to_string(),
        }
    }

    pub fn not_applicable(key: &str, why: &str) -> Self {
        Self {
            key: key.to_string(),
            status: GateStatus::NotApplicable,
            observed: None,
            limit: None,
            message: why.to_string(),
        }
    }

    /// Downgrade a failure to a review.
    ///
    /// For a gate whose limit is a bootstrap guess. The measurement is still
    /// reported; only its authority is reduced.
    pub fn advisory(mut self) -> Self {
        if self.status == GateStatus::Fail {
            self.status = GateStatus::NeedsReview;
        }
        self
    }
}

/// The verdict, and everything it was reached from.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkVerdict {
    pub status: GateStatus,
    pub gates: Vec<GateResult>,
    pub summary: String,
}

impl BenchmarkVerdict {
    /// The worst of the gates.
    ///
    /// Worst rather than an average, because averaging is how a band that lost
    /// half its energy gets paid for by a better colour match.
    pub fn from(gates: Vec<GateResult>) -> Self {
        let failed = gates
            .iter()
            .filter(|g| g.status == GateStatus::Fail)
            .count();
        let review = gates
            .iter()
            .filter(|g| g.status == GateStatus::NeedsReview)
            .count();
        let status = if failed > 0 {
            GateStatus::Fail
        } else if review > 0 {
            GateStatus::NeedsReview
        } else {
            GateStatus::Pass
        };
        let summary = match status {
            GateStatus::Pass => format!("{} gate(s) passed", gates.len()),
            GateStatus::NeedsReview => {
                format!(
                    "{review} gate(s) need review, {} passed",
                    gates.len() - review
                )
            }
            _ => format!("{failed} gate(s) failed of {}", gates.len()),
        };
        Self {
            status,
            gates,
            summary,
        }
    }
}

/// How two windows compared where they overlapped.
#[derive(Clone, Debug, PartialEq)]
pub struct ComparisonMetric {
    pub key: String,
    pub max_abs: f64,
    pub rms: f64,
    /// Whether every shared sample was bit-identical.
    ///
    /// The gate, not `max_abs`. A tiny difference in a deterministic field is
    /// not "close enough" — it means two windows disagreed, and the only reason
    /// they can disagree is that something depended on the window.
    pub bit_exact: bool,
}

/// Where a run's inputs came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceIdentity {
    pub scenario: String,
    pub seed_hex: String,
    pub profile_digest: String,
    pub generator_version: u32,
}

/// Which run this was, and on what.
///
/// Distinct from [`SourceIdentity`], which says what was *measured*. This says
/// what did the measuring, and the two move independently: the same laboratory
/// benchmarked on two machines is one source and two runs.
#[derive(Clone, Debug, PartialEq)]
pub struct RunIdentity {
    /// Derived from the inputs rather than drawn, so the same run on the same
    /// checkout has the same id and a committed report does not churn.
    pub run_id: String,
    pub notes: Vec<String>,
}

/// One complete run.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundBenchmarkReport {
    pub schema_version: u32,
    pub source: SourceIdentity,
    pub run: RunIdentity,
    pub performance: super::performance::PerformanceMetrics,
    /// Files this run wrote. Empty unless a caller asked for them.
    pub artifacts: Vec<super::performance::ArtifactRecord>,
    pub scenario_asks: String,
    pub grid: super::AnalysisGrid,
    pub topography: TopographyMetrics,
    pub spectrum: SpectralMetrics,
    pub semivariograms: Vec<Semivariogram>,
    pub optics: OpticsMetrics,
    pub composability: Vec<ComparisonMetric>,
    pub counts: BTreeMap<String, usize>,
    pub verdict: BenchmarkVerdict,
}

impl GroundBenchmarkReport {
    /// The report as JSON, in the shape the companion schema declares.
    pub fn to_json(&self) -> Value {
        json!({
            "schema_version": self.schema_version,
            "source": {
                "scenario": self.source.scenario,
                "seed": self.source.seed_hex,
                "profile_digest": self.source.profile_digest,
                "generator_version": self.source.generator_version,
            },
            "scenario": {
                "name": self.source.scenario,
                "asks": self.scenario_asks,
                "analysis_grid": {
                    "origin_index": [self.grid.origin_index.x, self.grid.origin_index.y],
                    "spacing_m": self.grid.spacing_m,
                    "columns": self.grid.columns,
                    "rows": self.grid.rows,
                    "anchor": "Edge",
                    "row_order": "BottomUp",
                },
            },
            "run": {
                "run_id": self.run.run_id,
                "machine": {
                    "os": self.performance.machine.os,
                    "arch": self.performance.machine.arch,
                    "cpu": self.performance.machine.cpu,
                    "gpu": self.performance.machine.gpu,
                    "rustc": self.performance.machine.rustc,
                    "cargo_profile": self.performance.machine.cargo_profile,
                    "blender": self.performance.machine.blender,
                    "cycles_device": self.performance.machine.cycles_device,
                    "logical_threads": self.performance.machine.logical_threads,
                },
                "repetitions": self.performance.repetitions,
                "warmup_repetitions": self.performance.warmup_repetitions,
                "notes": self.run.notes,
            },
            "performance": {
                "total_median_ms": finite(self.performance.total_median_ms),
                "total_p95_ms": finite(self.performance.total_p95_ms),
                "peak_bytes": self.performance.peak_bytes,
                "stages": self.performance.stages.iter().map(|s| json!({
                    "stage": s.stage,
                    "median_ms": finite(s.median_ms),
                    "mad_ms": finite(s.mad_ms),
                    "p95_ms": finite(s.p95_ms),
                    "peak_bytes": s.peak_bytes,
                })).collect::<Vec<_>>(),
            },
            "artifacts": self.artifacts.iter().map(|a| json!({
                "kind": a.kind,
                "path": a.path,
                "checksum": a.checksum,
                "bytes": a.bytes,
            })).collect::<Vec<_>>(),
            "counts": self.counts,
            "topography": {
                "height_m": summary_json(&self.topography.height_m),
                "sa_m": finite(self.topography.sa_m),
                "sq_m": finite(self.topography.sq_m),
                "ssk": finite(self.topography.ssk),
                "sku": finite(self.topography.sku),
                "rms_slope": finite(self.topography.rms_slope),
                "positive_area_fraction": finite(self.topography.positive_area_fraction),
                "cavity_height_pearson": finite(self.topography.cavity_height_pearson),
                "cavity_height_spearman": finite(self.topography.cavity_height_spearman),
                "detrend_plane": self.topography.detrend_plane.map(finite),
                "scale_dependent": self.topography.scale_dependent.iter().map(|m| json!({
                    "direction_rad": finite(m.direction_rad),
                    "lag_m": finite(m.lag_m),
                    "height_difference_rms_m": finite(m.height_difference_rms_m),
                    "slope_rms": finite(m.slope_rms),
                    "curvature_rms_per_m": m.curvature_rms_per_m.map(finite),
                })).collect::<Vec<_>>(),
                "semivariograms": self.semivariograms.iter().map(|v| json!({
                    "direction_rad": finite(v.direction_rad),
                    "nugget_m2": finite(v.nugget_m2),
                    "sill_m2": finite(v.sill_m2),
                    "practical_range_m": finite(v.practical_range_m),
                    "autocorrelation_1e_m": finite(v.autocorrelation_1e_m),
                    "samples": v.samples.iter().map(|s| json!({
                        "lag_m": finite(s.lag_m),
                        "gamma_m2": finite(s.gamma_m2),
                        "pair_count": s.pair_count,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            },
            "spectrum": {
                "variance_from_psd_m2": finite(self.spectrum.variance_from_psd_m2),
                "parseval_relative_error": finite(self.spectrum.parseval_relative_error),
                "axis_grid_energy_fraction": finite(self.spectrum.axis_grid_energy_fraction),
                "above_policy_cutoff_fraction": finite(self.spectrum.above_policy_cutoff_fraction),
                "anisotropy": finite(self.spectrum.anisotropy),
                "principal_wavevector_rad": finite(self.spectrum.principal_wavevector_rad),
                "bands": self.spectrum.bands.iter().map(|b| json!({
                    "key": b.key,
                    "declared_wavelength_m": finite(b.declared_wavelength_m),
                    "dominant_wavelength_m": finite(b.dominant_wavelength_m),
                    "energy_m2": finite(b.energy_m2),
                    "energy_share": finite(b.energy_share),
                    "out_of_band_fraction": finite(b.out_of_band_fraction),
                })).collect::<Vec<_>>(),
            },
            "optics": {
                "profile": self.optics.profile,
                "colours": [
                    colour_json("dry_mid", &self.optics.dry),
                    colour_json("wet_mid", &self.optics.wet),
                ],
                "moisture_albedo_monotone": self.optics.moisture_albedo_monotone,
                "finite_and_non_negative": self.optics.finite_and_non_negative,
                "endpoints_match_declaration": self.optics.endpoints_match_declaration,
                "delta_e_dry_to_wet": finite(self.optics.delta_e_dry_to_wet),
                "hue_ratio_span": finite(self.optics.hue_ratio_span),
            },
            "composability": {
                "comparisons": self.composability.iter().map(|c| json!({
                    "key": c.key,
                    "max_abs": finite(c.max_abs),
                    "rms": finite(c.rms),
                    "bit_exact": c.bit_exact,
                })).collect::<Vec<_>>(),
                "all_structural_exact": self.composability.iter().all(|c| c.bit_exact),
            },
            "verdict": {
                "status": self.verdict.status.name(),
                "summary": self.verdict.summary,
                "gates": self.verdict.gates.iter().map(|g| json!({
                    "key": g.key,
                    "status": g.status.name(),
                    "observed": g.observed.map(finite),
                    "limit": g.limit.map(finite),
                    "message": g.message,
                })).collect::<Vec<_>>(),
            },
        })
    }

    /// The report as a human-readable table.
    ///
    /// Read far more often than the JSON. A gate that failed should say what it
    /// saw and what it wanted on one line, because the first question anybody
    /// asks is "by how much".
    pub fn to_table(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{}  [{}]",
            self.source.scenario,
            self.verdict.status.name()
        );
        let _ = writeln!(out, "  {}", self.scenario_asks);
        let _ = writeln!(
            out,
            "  grid     {}x{} at {:.4} m",
            self.grid.columns, self.grid.rows, self.grid.spacing_m
        );
        let _ = writeln!(
            out,
            "  height   Sq {:.5} m, Sa {:.5} m, skew {:+.3}, kurtosis {:.3}",
            self.topography.sq_m, self.topography.sa_m, self.topography.ssk, self.topography.sku
        );
        let _ = writeln!(
            out,
            "  cavity   pearson {:+.3}, spearman {:+.3} against height",
            self.topography.cavity_height_pearson, self.topography.cavity_height_spearman
        );
        let _ = writeln!(
            out,
            "  spectrum parseval {:.2e}, axis energy {:.1}%, alias {:.1}%, anisotropy {:.3}",
            self.spectrum.parseval_relative_error,
            self.spectrum.axis_grid_energy_fraction * 100.0,
            self.spectrum.above_policy_cutoff_fraction * 100.0,
            self.spectrum.anisotropy
        );
        for band in &self.spectrum.bands {
            let _ = writeln!(
                out,
                "    {:<10} declared {:.4} m, dominant {:.4} m, {:.1}% of energy",
                band.key,
                band.declared_wavelength_m,
                band.dominant_wavelength_m,
                band.energy_share * 100.0
            );
        }
        // The timing, with the counters on the same line. A speed number without
        // a content count is not a measurement of anything — the easiest way to
        // make any of this faster is to do less of it. See `ground::performance`.
        let _ = writeln!(
            out,
            "  time     {:.1} ms median, {:.1} ms p95 over {} rep(s), {} sample(s)",
            self.performance.total_median_ms,
            self.performance.total_p95_ms,
            self.performance.repetitions,
            self.counts.get("analysis_samples").copied().unwrap_or(0),
        );
        for stage in &self.performance.stages {
            let _ = writeln!(
                out,
                "    {:<14} {:>7.2} ms  ±{:.2}",
                stage.stage, stage.median_ms, stage.mad_ms
            );
        }
        for gate in &self.verdict.gates {
            if gate.status == GateStatus::Pass {
                continue;
            }
            let _ = writeln!(
                out,
                "  [{}] {}: {}",
                gate.status.name(),
                gate.key,
                gate.message
            );
        }
        out
    }
}

/// Replace a non-finite with zero, so the JSON stays parseable.
///
/// A NaN serialises as bare `NaN`, which is not JSON — a CI job reading the
/// report fails to parse it and reports a broken file rather than the broken
/// measurement that produced it. Zero is a lie, so every gate that could produce
/// one checks `is_finite` before it checks its limit.
fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn summary_json(summary: &super::topography::ScalarSummary) -> Value {
    json!({
        "count": summary.count,
        "min": finite(summary.min),
        "max": finite(summary.max),
        "mean": finite(summary.mean),
        "median": finite(summary.median),
        "stddev": finite(summary.stddev),
        "mad": finite(summary.mad),
        "p01": finite(summary.p01),
        "p05": finite(summary.p05),
        "p95": finite(summary.p95),
        "p99": finite(summary.p99),
    })
}

fn colour_json(key: &str, colour: &super::optics::ColourMetric) -> Value {
    json!({
        "key": key,
        "linear_rgb_mean": colour.linear_rgb.map(|c| c as f64).map(finite),
        "linear_rgb_median": colour.linear_rgb.map(|c| c as f64).map(finite),
        "luminance": finite(colour.luminance),
        "g_over_r": finite(colour.g_over_r),
        "b_over_r": finite(colour.b_over_r),
        "lab_median": colour.lab.map(finite),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_is_the_worst_of_its_gates() {
        // Worst rather than an average, because averaging is how a band that
        // lost half its energy gets paid for by a better colour match.
        let pass = GateResult::holds("a", true, "");
        let fail = GateResult::holds("b", false, "");
        let review = GateResult::holds("c", false, "").advisory();

        assert_eq!(
            BenchmarkVerdict::from(vec![pass.clone()]).status,
            GateStatus::Pass
        );
        assert_eq!(
            BenchmarkVerdict::from(vec![pass.clone(), review.clone()]).status,
            GateStatus::NeedsReview
        );
        assert_eq!(
            BenchmarkVerdict::from(vec![pass, review, fail]).status,
            GateStatus::Fail
        );
    }

    #[test]
    fn a_not_applicable_gate_does_not_fail_a_run() {
        // A flat card has no band energy, and reporting that as a failure would
        // train readers to ignore failures.
        let gates = vec![
            GateResult::holds("a", true, ""),
            GateResult::not_applicable("bands", "a flat card declares no bands"),
        ];
        assert_eq!(BenchmarkVerdict::from(gates).status, GateStatus::Pass);
    }

    #[test]
    fn a_non_finite_observation_fails_rather_than_passing_a_limit_check() {
        // A NaN compares false against every limit, so a naive `>` test would
        // pass it. The failure mode is a benchmark that reports green while
        // measuring nothing.
        let gate = GateResult::at_most("x", f64::NAN, 1.0, "something");
        assert_eq!(gate.status, GateStatus::Fail);
        let infinite = GateResult::at_most("x", f64::INFINITY, 1.0, "something");
        assert_eq!(infinite.status, GateStatus::Fail);
    }

    #[test]
    fn a_within_gate_rejects_both_ends() {
        assert_eq!(
            GateResult::within("x", 1.0, 0.95, 1.05, "").status,
            GateStatus::Pass
        );
        assert_eq!(
            GateResult::within("x", 0.5, 0.95, 1.05, "").status,
            GateStatus::Fail
        );
        assert_eq!(
            GateResult::within("x", 2.0, 0.95, 1.05, "").status,
            GateStatus::Fail
        );
    }

    #[test]
    fn a_non_finite_never_reaches_the_json() {
        // A NaN serialises as bare `NaN`, which is not JSON: a CI job reading
        // the report would fail to parse it and report a broken file rather
        // than the broken measurement.
        assert_eq!(finite(f64::NAN), 0.0);
        assert_eq!(finite(f64::INFINITY), 0.0);
        assert_eq!(finite(-1.5), -1.5);
    }

    #[test]
    fn an_advisory_gate_leaves_a_passing_gate_alone() {
        let pass = GateResult::holds("a", true, "").advisory();
        assert_eq!(pass.status, GateStatus::Pass);
    }
}

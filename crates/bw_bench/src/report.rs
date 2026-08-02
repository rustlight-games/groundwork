//! Recording and comparing results.
//!
//! criterion already tracks performance over time. This exists for the
//! aesthetic metrics, which criterion has no concept of, and to give both kinds
//! one comparable format so a single command can answer "did anything get
//! worse".

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: Box<ron::error::SpannedError>,
    },
}

/// What a measurement is denominated in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Nanoseconds,
    TicksPerSecond,
    /// A dimensionless score, usually 0..=1. Most aesthetic metrics.
    Ratio,
    Count,
    Bytes,
}

/// One recorded number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// Dotted path, e.g. `rocks.boulder.compactness` or `sim.tick_throughput`.
    pub name: String,
    /// Which [`Scenario`](crate::Scenario) or seed set this came from.
    pub scenario: String,
    pub value: f64,
    pub unit: Unit,
    /// Direction of improvement. Recorded per measurement because the suite
    /// mixes both — throughput should rise, frame time should fall — and a
    /// comparison that guesses will report the wrong half as regressions.
    pub higher_is_better: bool,
}

impl Measurement {
    pub fn new(
        name: impl Into<String>,
        scenario: impl Into<String>,
        value: f64,
        unit: Unit,
        higher_is_better: bool,
    ) -> Self {
        Self {
            name: name.into(),
            scenario: scenario.into(),
            value,
            unit,
            higher_is_better,
        }
    }

    fn key(&self) -> (&str, &str) {
        (self.name.as_str(), self.scenario.as_str())
    }
}

/// A change between two runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub name: String,
    pub scenario: String,
    pub baseline: f64,
    pub current: f64,
    /// Signed fraction, positive meaning improvement.
    pub relative: f64,
}

/// A set of measurements from one run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub measurements: Vec<Measurement>,
}

impl Report {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, measurement: Measurement) -> &mut Self {
        self.measurements.push(measurement);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.measurements.is_empty()
    }

    pub fn len(&self) -> usize {
        self.measurements.len()
    }

    pub fn get(&self, name: &str, scenario: &str) -> Option<&Measurement> {
        self.measurements
            .iter()
            .find(|m| m.key() == (name, scenario))
    }

    /// Measurements that got worse by more than `tolerance`.
    ///
    /// `tolerance` is a fraction, so 0.05 allows a five percent slip. Aesthetic
    /// metrics are noisier than timings and generally want a looser tolerance
    /// than performance ones.
    ///
    /// Measurements absent from `baseline` are not reported — a new benchmark
    /// is not a regression.
    pub fn regressions_against(&self, baseline: &Report, tolerance: f64) -> Vec<Change> {
        let tolerance = tolerance.abs();
        let mut out = Vec::new();
        for current in &self.measurements {
            let Some(previous) = baseline.get(&current.name, &current.scenario) else {
                continue;
            };
            if previous.value.abs() <= f64::EPSILON {
                continue;
            }
            let delta = (current.value - previous.value) / previous.value.abs();
            let relative = if current.higher_is_better {
                delta
            } else {
                -delta
            };
            if relative < -tolerance {
                out.push(Change {
                    name: current.name.clone(),
                    scenario: current.scenario.clone(),
                    baseline: previous.value,
                    current: current.value,
                    relative,
                });
            }
        }
        // Worst first, so a truncated report still shows what matters.
        out.sort_by(|a, b| a.relative.total_cmp(&b.relative));
        out
    }

    pub fn save(&self, path: &Path) -> Result<(), ReportError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .expect("a report is always serialisable");
        std::fs::write(path, text).map_err(|source| ReportError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn load(path: &Path) -> Result<Self, ReportError> {
        let text = std::fs::read_to_string(path).map_err(|source| ReportError::Io {
            path: path.display().to_string(),
            source,
        })?;
        ron::from_str(&text).map_err(|source| ReportError::Parse {
            path: path.display().to_string(),
            source: Box::new(source),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(pairs: &[(&str, f64, bool)]) -> Report {
        let mut r = Report::new();
        for &(name, value, higher_is_better) in pairs {
            r.push(Measurement::new(
                name,
                "medium",
                value,
                Unit::Ratio,
                higher_is_better,
            ));
        }
        r
    }

    #[test]
    fn a_drop_in_a_higher_is_better_metric_is_a_regression() {
        let baseline = report(&[("rocks.compactness", 0.80, true)]);
        let current = report(&[("rocks.compactness", 0.60, true)]);
        let found = current.regressions_against(&baseline, 0.05);
        assert_eq!(found.len(), 1);
        assert!(found[0].relative < 0.0);
    }

    #[test]
    fn a_rise_in_a_lower_is_better_metric_is_a_regression() {
        // Frame time going up is bad; the comparison must know the difference.
        let baseline = report(&[("grass.frame_time", 4.0, false)]);
        let current = report(&[("grass.frame_time", 8.0, false)]);
        assert_eq!(current.regressions_against(&baseline, 0.05).len(), 1);
    }

    #[test]
    fn improvements_are_not_reported() {
        let baseline = report(&[("sim.throughput", 100.0, true)]);
        let current = report(&[("sim.throughput", 150.0, true)]);
        assert!(current.regressions_against(&baseline, 0.05).is_empty());
    }

    #[test]
    fn changes_inside_the_tolerance_are_ignored() {
        let baseline = report(&[("sim.throughput", 100.0, true)]);
        let current = report(&[("sim.throughput", 97.0, true)]);
        assert!(current.regressions_against(&baseline, 0.05).is_empty());
        assert_eq!(current.regressions_against(&baseline, 0.01).len(), 1);
    }

    #[test]
    fn a_new_measurement_is_not_a_regression() {
        let baseline = report(&[]);
        let current = report(&[("rocks.new_metric", 0.1, true)]);
        assert!(current.regressions_against(&baseline, 0.05).is_empty());
    }

    #[test]
    fn a_zero_baseline_does_not_divide_by_zero() {
        let baseline = report(&[("x", 0.0, true)]);
        let current = report(&[("x", 5.0, true)]);
        assert!(current.regressions_against(&baseline, 0.05).is_empty());
    }

    #[test]
    fn regressions_are_ordered_worst_first() {
        let baseline = report(&[("a", 100.0, true), ("b", 100.0, true)]);
        let current = report(&[("a", 90.0, true), ("b", 50.0, true)]);
        let found = current.regressions_against(&baseline, 0.01);
        assert_eq!(found[0].name, "b");
    }

    #[test]
    fn the_same_name_in_different_scenarios_is_tracked_separately() {
        let mut baseline = Report::new();
        baseline.push(Measurement::new(
            "sim.tps",
            "small",
            100.0,
            Unit::TicksPerSecond,
            true,
        ));
        baseline.push(Measurement::new(
            "sim.tps",
            "large",
            10.0,
            Unit::TicksPerSecond,
            true,
        ));

        let mut current = Report::new();
        current.push(Measurement::new(
            "sim.tps",
            "small",
            100.0,
            Unit::TicksPerSecond,
            true,
        ));
        current.push(Measurement::new(
            "sim.tps",
            "large",
            5.0,
            Unit::TicksPerSecond,
            true,
        ));

        let found = current.regressions_against(&baseline, 0.05);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].scenario, "large");
    }

    #[test]
    fn round_trips_through_ron() {
        let original = report(&[("a", 1.5, true), ("b", 2.5, false)]);
        let text = ron::ser::to_string(&original).unwrap();
        let parsed: Report = ron::from_str(&text).unwrap();
        assert_eq!(parsed.measurements, original.measurements);
    }
}

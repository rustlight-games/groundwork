//! Benchmark fixtures, metrics, and reporting.
//!
//! Benchmarking is a first-class activity in this project, not an afterthought,
//! and it covers two things that are usually kept apart.
//!
//! **Performance** is the familiar half: how many ticks per second the
//! simulation manages, how long a flow-field rebuild takes, how much of a frame
//! the grass costs. Measured with criterion in each crate's `benches/`.
//!
//! **Aesthetics** is the unusual half, and it exists because most of this game
//! is generated. A rock generator can regress in a way no unit test notices:
//! the rocks still have valid geometry, they just look worse — spikier, or all
//! the same, or clumped when scattered. [`metrics`] turns those judgements into
//! numbers that can be tracked over time. They do not replace looking at the
//! output, but they catch the drift between the times you look.
//!
//! The rule that makes any of it comparable is in [`fixtures`]: benchmarks run
//! against fixed seeds and fixed scenarios. A measurement taken against a
//! random input is not a measurement.
//!
//! See `docs/BENCHMARKS.md` for when a benchmark is required.

#![forbid(unsafe_code)]

pub mod fixtures;
pub mod metrics;
pub mod report;

pub use fixtures::{SEEDS, Scenario};
pub use metrics::{blue_noise_score, compactness, convexity, luminance_spread, silhouette_variety};
pub use report::{Measurement, Report, ReportError, Unit};

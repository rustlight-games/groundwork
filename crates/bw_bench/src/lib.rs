//! Benchmark fixtures, metrics, and reporting.
//!
//! Benchmarking is a first-class activity here, not an afterthought, and it
//! covers two things that are usually kept apart.
//!
//! **Performance** is the familiar half: how long a page takes to bake, what a
//! Cycles export costs, how fast the sampler answers a batch. Measured with
//! criterion in each crate's `benches/`.
//!
//! **Aesthetics** is the unusual half, and it exists because all of this is
//! generated. A generator can regress in a way no unit test notices: the
//! geometry is still valid, the output just looks worse — spikier, or all the
//! same, or clumped when it should be scattered. [`metrics`] turns those
//! judgements into numbers that can be tracked over time. They do not replace
//! looking at the output; they catch the drift between the times you look.
//!
//! The rule that makes any of it comparable is in [`fixtures`]: benchmarks run
//! against fixed seeds. A measurement taken against a random input is not a
//! measurement.
//!
//! ## Status
//!
//! Mid-migration, and honest about it. The seeds, the metrics and the report
//! comparison are here and tested. The *fixtures* are not: the three battle
//! scenarios that used to live here went with the simulation, and the terrain
//! fixtures that replace them — a page, a grid of pages, a view, a blend
//! boundary, a path junction — arrive with `terrain_bench`.
//!
//! See `docs/BENCHMARKS.md` for when a benchmark is required.

#![forbid(unsafe_code)]

pub mod fixtures;
pub mod metrics;
pub mod report;

pub use fixtures::SEEDS;
pub use metrics::{
    Point, blue_noise_score, compactness, convexity, luminance_spread, silhouette_variety,
};
pub use report::{Measurement, Report, ReportError, Unit};

//! The ground benchmark: six questions that must not collapse into one score.
//!
//! ```text
//! 1  Did the mathematical surface match the authored profile?
//! 2  Did representation tiering preserve the same surface signal?
//! 3  Did material state behave physically and monotonically?
//! 4  Did independent windows and trace slices agree?
//! 5  Did the Cycles image preserve the causal AOVs?
//! 6  Did a speed-up preserve all of the above?
//! ```
//!
//! A visual review is still necessary. It happens *after* the structural and
//! quantitative failures have been removed, because a reviewer looking at a
//! plate cannot tell a band that moved tiers from a band that was lost, and both
//! look like "the soil changed a bit".
//!
//! ## The split that makes this runnable
//!
//! The topography half — this module and its neighbours — needs no Blender. It
//! samples the `GroundEvaluator` directly and answers questions one to four in
//! well under a second on the compact scenario set, which means it can run on
//! every commit. The render half needs Cycles and runs on the visual gate.
//!
//! Keeping them apart is what stops the benchmark becoming something nobody
//! runs. A suite that takes twenty minutes is a suite consulted after the
//! argument rather than before it.

pub mod field;
pub mod optics;
pub mod performance;
pub mod psd;
pub mod report;
pub mod run;
pub mod scenarios;
pub mod semivariogram;
pub mod topography;

pub use field::{AnalysisGrid, GroundField};
pub use performance::{ArtifactRecord, PerformanceMetrics, Recorder};
pub use report::{BenchmarkVerdict, GateResult, GateStatus, GroundBenchmarkReport};
pub use run::{DEFAULT_SEED, run};
pub use scenarios::{GROUND_SCENARIOS, GroundScenario};

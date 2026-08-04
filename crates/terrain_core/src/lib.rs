//! The vocabulary terrain is described in.
//!
//! Four things live here, and they are here together because they are the four
//! decisions that everything else in the framework is written against. Each one
//! has a plausible alternative that works fine until it doesn't, and the failure
//! in every case is quiet: a seam, a shifted world, a cache that answers with
//! the wrong ground, an error message that arrives one at a time.
//!
//! ```text
//! coords       where things are          metres, half-open, floored
//! ids          what things are called    authored strings, never file order
//! seed         where randomness comes    addressed, never sequential
//!              from
//! digest       whether two things are    semantic content only
//!              the same
//! ```
//!
//! ## The one sentence
//!
//! **Terrain is a continuous function of world position.** Everything here
//! exists to keep that true. The isometric grid, the semantic tiles, the raster
//! masks and the output pages are all ways of *addressing* or *presenting*
//! terrain; none of them is its identity. A framework that lets any of them
//! become the identity acquires a preferred resolution, a preferred origin and a
//! preferred tiling, and then cannot answer the question it exists to answer:
//! what is the ground at this point.
//!
//! Three consequences follow, and they are worth stating because each one costs
//! something and the cost is easy to mistake for waste:
//!
//! - **World positions are `f64`.** An `f32` holds about a millimetre at ten
//!   kilometres out, which is coarser than a close-up render resolves. See
//!   [`coords`].
//! - **Randomness is addressed, not drawn.** A sequential generator makes every
//!   value depend on how many values came before it, which means a page's
//!   contents depend on where its edges were. See [`seed`].
//! - **Identity is a string the author chose.** Not a number derived from
//!   sorted filename order, which renumbers when a file is added. See [`ids`].
//!
//! ## No renderer in the graph
//!
//! This crate takes no Bevy, and neither does anything that samples terrain,
//! builds a scene, or hands one to Cycles. That boundary is the difference
//! between a terrain framework and a renderer with a terrain feature: the
//! headless half has to be usable from a command line, a test, a benchmark and a
//! dataset job, none of which want a window.

#![forbid(unsafe_code)]

pub mod coords;
pub mod diagnostics;
pub mod digest;
pub mod document;
mod document_digest;
pub mod ids;
pub mod prepare;
pub mod registry;
pub mod sample;
pub mod seed;
pub mod sources;
pub mod validate;

pub use coords::{
    CellCoord, CellGrid, Footprint, RasterTransform, RowOrder, TexelAnchor, WorldPoint, WorldRect,
    WorldVector,
};
pub use diagnostics::{Diagnostic, DiagnosticReport, Location, Severity};
pub use digest::{DIGEST_ALGORITHM_VERSION, Digest, Digestible, Fingerprint};
pub use document::{
    AssetPath, CoordinateSystem, DocumentMetadata, LayerDef, LayerOperation, Mask, MaterialDef,
    MaterialLayer, MaterialMode, ModifierChannelDef, ModifierComposition, ModifierLayer,
    ModifierUnit, ParameterObject, ParameterValue, PopulationDef, Profile, Source, SourceDef,
    TerrainDocument, ValueRange,
};
pub use ids::{
    AppearanceKey, KeyError, LayerKey, MaterialIndex, MaterialKey, ModifierIndex, ModifierKey,
    PopulationIndex, PopulationKey, RecipeKey, SourceIndex, SourceKey, StreamKey,
};
pub use prepare::{PrepareOptions, PrepareReport, PreparedTerrain, prepare};
pub use registry::{
    AssetError, AssetResolver, MemoryAssets, NoAssets, ScalarField, SourceRecipe, SourceRegistry,
};
pub use sample::{
    FeatureContext, JunctionClass, MaterialWeight, MaterialWeightSet, MicroreliefSample,
    ModifierSet, SampleChannels, SampleFootprint, SampleQuery, TerrainSample,
};
pub use seed::{
    CandidateId, PopulationHash, RandomAddress, RootSeed, SEED_ALGORITHM_VERSION, SeedContext,
};
pub use sources::{NoiseField, Spline, SplineDistanceField};
pub use validate::{KnownRecipes, validate, validate_against};

//! A paired render: one `TerrainScene`, held once, rendered twice.
//!
//! ## The pair must be one meadow
//!
//! ```text
//!   wrong                              right
//!   ─────                              ─────
//!   cheap  → generate scene A          generate scene ────┬─→ render cheap
//!   costly → generate scene B                             └─→ render costly
//! ```
//!
//! Generating twice looks safe, because placement is a pure function of world
//! coordinates and both runs would agree. It is not the agreement that matters;
//! it is that nothing can *later* make them disagree. A quality tier that
//! skipped a fork, a step count that moved a rib, an optimisation that reordered
//! a draw — any of those turns the pair into two photographs of two different
//! fields, and a network trained on that learns to hallucinate rather than to
//! reconstruct.
//!
//! The failure is silent: the loss simply stops falling, and no image in the
//! corpus looks wrong. See [`shard::RenderPair`], which enforces the rule with
//! the type: there is no constructor that takes two scenes.
//!
//! ## Known gap
//!
//! The corpus-generation job that used to live here (`dataset`, `Pair`,
//! `TracedPair`, `CorpusRequest`, `generate`) paired a cheap rasterised input
//! against a Cycles target, and both halves rasterised the shared scene —
//! `TracedPair::build` did, for its "input" side, not only the raster-only
//! fallback. With the rasteriser gone (see root `CLAUDE.md`, "Cycles is the
//! only renderer"), that job has no way left to produce its input half.
//!
//! What survives is [`shard`] — the renderer-agnostic manifest, layout and
//! `RenderPair` contract — because a redesigned corpus job still needs it. What
//! is missing is the job itself: the low-fidelity input side needs to be the
//! `TerrainFieldStack` directly (per `terrain_scene`'s own doc), not a picture
//! of it, and that pairing has not been designed yet.

#![forbid(unsafe_code)]

pub mod shard;

pub use shard::{
    ArtifactFile, RenderArtifact, RenderPair, SHARD_MANIFEST_VERSION, ShardLayout, ShardManifest,
};

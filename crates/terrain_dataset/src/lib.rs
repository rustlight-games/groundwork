//! Paired renders, for training something to do this faster.
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
//! corpus looks wrong.
//!
//! So there is no API here that accepts two generators.

#![forbid(unsafe_code)]

pub mod dataset;

pub use dataset::{CorpusReport, CorpusRequest, Pair, Render, ShardMetadata, TracedPair, generate};

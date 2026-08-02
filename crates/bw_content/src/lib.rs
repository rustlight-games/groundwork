//! Content: the data half of the game.
//!
//! The design goal here is that adding a character or a spell is a RON file,
//! not a Rust type. With hundreds of units and abilities planned, one type per
//! spell would mean the codebase grows linearly with the content, every
//! addition needs a recompile, and no non-programmer can contribute.
//!
//! So an ability is a tree of *registered primitives* — see [`effect`]. A small
//! fixed set of Rust handlers (`damage`, `heal`, `apply_status`, `knockback`,
//! and so on) is composed by data into arbitrarily many abilities.
//!
//! ## Where floats are allowed
//!
//! Authoring uses `f64`, because writing `0.35` in a RON file is much nicer
//! than writing a fixed-point bit pattern. Those values are converted to
//! [`Real`](bw_core::Real) exactly once, at load time, and simulation only ever
//! sees the converted value. This is safe: parsing a decimal literal to `f64`
//! is correctly rounded and identical on every platform, as is the conversion
//! to fixed point. What is *not* safe, and does not happen, is arithmetic on
//! floats during simulation.

#![forbid(unsafe_code)]

pub mod db;
pub mod effect;
pub mod error;
pub mod params;
pub mod registry;
pub mod schema;
pub mod terrain;

pub use db::ContentDb;
pub use effect::{EffectSpec, TargetFilter, TargetShape, TargetSort, Targeting};
pub use error::{ContentError, ContentResult};
pub use params::{Params, Value};
pub use registry::{GeneratorRegistry, RockGenerator, RockShape, TerrainGenerator};
pub use schema::{
    AbilityDef, BaseStats, CharacterDef, EncounterDef, PropDef, RockDef, ScatterRule, StatusDef,
    TerrainDef,
};
pub use terrain::TerrainMap;

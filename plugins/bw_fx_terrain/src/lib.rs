//! Terrain generation, terrain effects, and prop scatter.
//!
//! Terrain is one of the two things this project generates rather than draws
//! (rocks being the other). It produces the movement costs navigation reads,
//! the density map the grass renderer reads, and the elevation that decides
//! where props may stand.
//!
//! Sprite props — trees, bushes, debris — are *placed*, not generated. That is
//! [`scatter`]'s job: it picks positions, and the renderer draws authored
//! artwork at them. Keeping placement here rather than in the renderer means
//! the headless trainer sees the same obstacles the player does.

#![forbid(unsafe_code)]

pub mod effects;
pub mod generators;
pub mod scatter;

pub use effects::register_effects;
pub use generators::{RollingHills, register_generators};
pub use scatter::{ScatterPoint, scatter};

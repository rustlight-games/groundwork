//! What grows on the terrain.
//!
//! This crate decides **where every mark goes and what shape it is**, and hands
//! that description to whoever is drawing. It links no renderer: not Bevy, not
//! the rasteriser, not the path tracer. That absence is the whole point of the
//! crate existing — while placement and rasterisation shared a module, deciding
//! that a blade was there required linking the code that fills pixels, and there
//! was no way to state which of them owned a `Stroke`.
//!
//! Three consumers read what this produces — the cheap rasteriser, the Cycles
//! exporter, and the shadow pass — and none of them is more canonical than the
//! others.
//!
//! ## Reading order
//!
//! [`iso`] first: it is the projection everything else is written against, and
//! where the art's authoring scale is set. Then [`field`], which decides what
//! the ground is like, and [`placement`], which decides what grows on it.
//! [`stroke`] is what one mark is. [`style`] is the whole of what the generator
//! is told.
//!
//! ## The rule that is expensive to break
//!
//! **Place in world space, project at the very end.** A clump placed by screen
//! position slides when the camera moves. Everything in [`field`] is a pure
//! function of a world coordinate, which is also what lets two pages that have
//! never met agree along a shared edge.

#![forbid(unsafe_code)]

pub mod fastmath;
pub mod field;
pub mod fixtures;
pub mod geometry;
pub mod iso;
pub mod page;
pub mod placement;
pub mod population;
pub mod quality;
pub mod recipes;
pub mod rng;
pub mod scene;
pub mod stroke;
pub mod style;
pub mod sun;
pub mod tone;

pub use field::WorldField;
pub use page::Page;
pub use population::{PopulationContext, PopulationRecipe, PopulationRegistry};
pub use quality::GrassRenderQuality;
pub use recipes::{default_registry, register_all};
pub use scene::GrassScene;
pub use stroke::Stroke;
pub use style::{GrassParams, GrassStyle};
pub use sun::Key;
pub use tone::Tone;

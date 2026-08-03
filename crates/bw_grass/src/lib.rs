//! Isometric grass: a baked ground-surface cache and the renderer that draws it.
//!
//! The system is built around one decision — **bake the field, draw the cache**.
//! Grass is not thousands of objects at runtime. It is a small number of opaque
//! pages of already-composited ground, generated from world coordinates and
//! cached, with the animated part kept to the few marks that actually need to
//! move.
//!
//! ```text
//!   world fields          strokes                 surface                page
//!   (mounds, dirt,   ->   (bezier blades,  ->     (depth-composited  ->  (opaque
//!    density, tint)        leaves, mat)            light index)           texture)
//!        field.rs          stroke.rs               surface.rs             material.rs
//!                              \                      /
//!                               \--- bake.rs --------/
//! ```
//!
//! ## Reading order
//!
//! [`iso`] first — it is the contract everything else is written against, and it
//! is where the cache's pixel scale is set. Then [`field`], which decides where
//! things go, and [`stroke`], which decides what one mark looks like. [`surface`]
//! explains why compositing is a depth test rather than alpha-over, and
//! [`bake`] is the assembly. [`palette`] can be read at any point; it is short,
//! and it is measured from the reference art rather than invented.
//!
//! ## The rules that are expensive to break later
//!
//! - **Place in world space, project at the very end.** A clump placed by screen
//!   position slides when the camera moves. Everything in [`field`] is a pure
//!   function of a world coordinate, which is also what lets two pages that have
//!   never met agree along a shared edge.
//! - **Shade through a ramp, never by multiplying.** Multiplying albedo by a
//!   lambert term gives grey-green shadows. The reference's darkest pixels are
//!   still saturated green and its brightest are yellow-green paint; only a
//!   lookup reproduces that. See [`palette`].
//! - **Composite by isometric depth, not by draw order.** Alpha-over produces a
//!   collage of decals. The depth test is what gives the cache an inside.
//! - **Detail belongs to the mound, not to the pixel.** The reference is not
//!   uniformly detailed: bright crowns, dark backs and dark interiors are
//!   organised by the mound field, and grass that is uniformly busy everywhere
//!   reads as carpet however good the individual marks are.
//!
//! Iterate with `cargo run --release -p bw_grass --example grass_bake`, which
//! bakes a plate to a PNG with no window and no GPU, and
//! `cargo run -p bw_grass --example grass_sandbox` for the live renderer.

#![forbid(unsafe_code)]

pub mod bake;
pub mod field;
pub mod iso;
pub mod material;
pub mod metrics;
pub mod palette;
pub mod plugin;
pub mod rng;
pub mod stroke;
pub mod surface;

pub use bake::{BakeParams, Page, bake};
pub use field::WorldField;
pub use material::GrassSurfaceMaterial;
pub use palette::Tone;
pub use plugin::{GrassPlugin, GrassWorld, PAGE_PIXELS};

//! The Bevy integration: page cache, material, and the plugin that draws.
//!
//! The only crate in the framework that takes Bevy, and the boundary is load
//! bearing. Everything upstream of here — the document, the sampler, the scene,
//! the generator, the bakers — has to be usable from a command line, a test, a
//! benchmark and a dataset job, none of which want a window.
//!
//! What lives here is genuinely engine-shaped: an asset cache keyed by page
//! address, a material with a shader behind it, and the systems that decide
//! which pages a camera needs.

#![forbid(unsafe_code)]

pub mod cache;
pub mod material;
pub mod plugin;

pub use material::GrassSurfaceMaterial;
pub use plugin::{GrassPlugin, GrassWorld, PAGE_PIXELS, grass_camera};

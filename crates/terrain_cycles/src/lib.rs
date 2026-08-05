//! Handing an explicit scene to Blender's path tracer.
//!
//! ## Rust places, Cycles lights
//!
//! The boundary is the single most important decision in the rendering half of
//! this framework. Blender receives **explicit geometry** and never scatters
//! anything itself: every blade's position, curve and attributes are decided in
//! Rust and written out.
//!
//! Two things follow, and neither is available if Blender does its own
//! scattering. The cheap render and the expensive one are the *same ground*, so
//! a training pair is one meadow. And a corpus is reproducible from a seed
//! rather than from a backup, because nothing about the picture lives inside a
//! `.blend` file.
//!
//! ## A subprocess, not a linked library
//!
//! Cycles' standalone interface is explicitly not a stable API, and linking its
//! internals means owning Embree, OpenImageIO, OpenColorIO, OIDN and a device
//! abstraction across four GPU backends. A process boundary costs a few seconds
//! of startup and buys immunity from all of it.
//!
//! ## The projection is a mirror, and the world is what gets reflected
//!
//! A path tracer cannot be handed a left-handed camera. See
//! [`terrain_scene::projection`] for why the game's projection is one, and
//! `to_blender` for the swap that fixes it. The rule this creates is worth
//! stating loudly: **nothing may cross this boundary without the swap.** A blade
//! reflected while its sun is not would be lit from the wrong side, and it would
//! look entirely plausible.

#![forbid(unsafe_code)]

pub mod aov;
pub mod bridge;
pub mod cycles;
pub mod export;
pub mod package;
pub mod plate;
pub mod secondary;

pub use aov::{OutputPass, OutputRequest};
pub use cycles::{Camera, CyclesScene, RenderSettings, blender_path, render};
pub use export::{RenderProfile, write_package};
pub use package::{
    GeometryKind, GeometryManifest, MaterialBindingManifest, PACKAGE_VERSION, ScenePackageManifest,
};
pub use plate::{Plate, PlatePlan, PlateRequest};

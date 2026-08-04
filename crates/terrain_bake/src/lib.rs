//! The cheap raster tier: a page of ground, composited by depth.
//!
//! This is the fast renderer — fast enough to run in a frame, and unable to
//! integrate a hemisphere. It exists for three jobs, and it is worth being clear
//! that only the first is about looking at pictures:
//!
//! - A **preview**, so a change can be judged in under a second.
//! - The neural renderer's **input**, paired against a Cycles target. See
//!   `terrain_dataset`.
//! - An exact **regression oracle** for optimisations, because it is
//!   deterministic to the bit and a path tracer is not.
//!
//! ## Composite by depth, never by draw order
//!
//! Alpha-over produces a collage of decals. The depth test is what gives a page
//! an inside — see [`surface`].
//!
//! ## Shade through a ramp, never by multiplying
//!
//! Multiplying an albedo by a lambert term gives grey-green shadows. This
//! renderer's "albedo" is a position in a hand-authored ramp, and half that
//! ramp's value is *where the hue goes* as the value falls; multiplying an index
//! by 0.3 does not darken a colour, it picks a different one. So light moves a
//! surface **along** the ramp instead. See [`palette`].
//!
//! ## What is not here
//!
//! Nothing that decides where a blade goes. That is `terrain_generators`, and
//! the separation is what lets one scene be drawn by this and by Cycles and be
//! the same meadow.

#![forbid(unsafe_code)]

pub mod bake;
pub mod lighting;
pub mod painter;
pub mod palette;
pub mod request;
pub mod shadow;
pub mod surface;

pub use bake::{BakeParams, BakeRegion, Passes, PreviewRasterStyle, bake, bake_padded};
pub use painter::Painter;
pub use request::{
    BakeManifest, BakeOutput, BakeRequest, BakeResolution, MANIFEST_VERSION, MipPolicy, PageLayout,
    PageRecord,
};
pub use surface::Surface;

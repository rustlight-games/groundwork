//! Bake requests: the renderer-agnostic contract for a page of ground.
//!
//! There is no renderer in this crate. Cycles is the only one — see
//! `terrain_cycles` and root `CLAUDE.md`. What lives here is the shape of a
//! request for a page of output (`BakeRequest`, `BakeOutput`) and the
//! provenance record a page is written with (`BakeManifest`), independent of
//! whatever produced the pixels.

#![forbid(unsafe_code)]

pub mod request;

pub use request::{
    BakeManifest, BakeOutput, BakeRequest, BakeResolution, MANIFEST_VERSION, MipPolicy, PageLayout,
    PageRecord,
};

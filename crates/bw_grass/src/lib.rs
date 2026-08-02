//! The grass renderer.
//!
//! **Status: skeleton.** The module boundaries, chunking, and the resources
//! that feed the eventual shader are here and wired together; the shader itself
//! is not. Chunks currently draw as flat quads tinted by density. What exists is
//! the structure that is painful to retrofit — everything else is shader work
//! that can happen without moving any of it.
//!
//! ## The shape of the finished thing
//!
//! Grass is drawn per [`chunk`], each holding an instance buffer of blades
//! placed from a [`density`] map that terrain generation produces. Blades sway
//! by sampling a [`wind`] field in the vertex shader — analytic, so it costs no
//! bandwidth — and bend away from a low-resolution [`disturbance`] texture that
//! units write into as they move, which is what makes an army leave a visible
//! wake. Distant chunks drop blade count through [`lod`] and eventually fade to
//! a flat texture.
//!
//! Chunking is the load-bearing decision. It is what makes culling, streaming,
//! LOD, and partial rebuilds all possible; adding it after the fact would mean
//! rewriting every one of them.
//!
//! Iterate with `cargo run -p bw_grass --example grass_sandbox`, which brings up
//! the chunk grid on its own without launching the game.

#![forbid(unsafe_code)]

pub mod chunk;
pub mod density;
pub mod disturbance;
pub mod lod;
pub mod wind;

use bevy::prelude::*;

pub use chunk::{CHUNK_CELLS, GrassChunk, GrassChunks};
pub use density::DensityMap;
pub use disturbance::DisturbanceMap;
pub use lod::{GrassLod, lod_for_distance};
pub use wind::WindField;

/// Adds grass rendering.
pub struct GrassPlugin;

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindField>()
            .init_resource::<GrassChunks>()
            .init_resource::<DisturbanceMap>()
            .add_systems(Update, (wind::advance_wind, disturbance::decay_disturbance));
    }
}

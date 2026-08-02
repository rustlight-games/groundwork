//! Grass: simulation and rendering.
//!
//! The system is built around one decision — **simulate a field, not blades**.
//! A world-aligned grid holds the posture of the canopy, and the renderer
//! reconstructs however many blades it needs by sampling it. Simulation cost
//! then depends on the area of ground being disturbed, not on how much grass is
//! drawn over it, which is the only reason a battlefield's worth of grass can
//! react to a battlefield's worth of units.
//!
//! ```text
//!   units, blasts, wind                     assets/shaders/grass.wgsl
//!          |                                          |
//!          v                                          v
//!   disturbance ---> field ---> field textures ---> blade vertex shader
//!   (swept stamps)   (solver)   (theta/axis/       (centreline, isometric
//!                                compaction)        projection, depth)
//! ```
//!
//! ## Reading order
//!
//! [`field`] first — it explains what a cell remembers and why there are six
//! channels rather than one. Then [`iso`], which is the contract the shader
//! must match. [`disturbance`] and [`wind`] are the two force sources, and
//! [`blade`] and [`material`] are how it all reaches the screen.
//!
//! ## The rules that are expensive to break later
//!
//! - **Simulate in world space, project at the very end.** A blade shoved west
//!   must behave exactly like a blade shoved north. Simulating in screen space
//!   makes the response depend on the camera, which is wrong in a way players
//!   feel without being able to name.
//! - **Roots do not move.** Every deformation is weighted to zero at the base.
//!   Grass whose roots slide reads instantly as a texture being dragged over
//!   the ground.
//! - **Bend in three dimensions, then project.** Blades reconstruct a virtual
//!   centreline through `(X, Y, Z)` and preserve their arc length, so leaning
//!   shortens the silhouette. Shearing a sprite instead is what makes grass
//!   look like rubber.
//! - **Keep neighbour coupling weak.** Grass clumps are not sewn together.
//!   Large-scale coherence comes from the shared wind field.
//!
//! Iterate with `cargo run -p bw_grass --example grass_sandbox`, which brings
//! the field and the renderer up on their own without launching the game.

#![forbid(unsafe_code)]

pub mod blade;
pub mod chunk;
pub mod density;
pub mod disturbance;
pub mod field;
pub mod iso;
pub mod lod;
pub mod material;
pub mod noise;
pub mod params;
pub mod scene;
pub mod wind;

use bevy::prelude::*;
use bevy::sprite_render::Material2dPlugin;

pub use chunk::{CHUNK_CELLS, GrassChunk, GrassChunks};
pub use density::DensityMap;
pub use disturbance::{GrassEvents, GrassInteractor, Shockwave};
pub use field::GrassField;
pub use lod::{GrassLod, lod_for_distance};
pub use material::GrassMaterial;
pub use params::GrassParams;
pub use scene::{GrassScene, GrassScenePlugin};
pub use wind::WindField;

/// Ordering within a frame.
///
/// Spelled out rather than inferred, because the order is a correctness
/// property: stamping after the solve would apply every disturbance a frame
/// late, and uploading before it would show the previous frame's grass.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GrassSet {
    /// Advance wind and age events.
    Sources,
    /// Write disturbances into the field.
    Stamp,
    /// Step the solver.
    Simulate,
    /// Push field state to the GPU.
    Upload,
}

/// Grass simulation and rendering.
///
/// Does not place any grass. Add [`GrassScenePlugin`] for that, or drive
/// [`chunk`] yourself from generated terrain.
pub struct GrassPlugin;

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<GrassMaterial>::default())
            .init_resource::<WindField>()
            .init_resource::<GrassField>()
            // After the field: it sizes its textures from the field's resolution.
            .init_resource::<material::GrassTextures>()
            .init_resource::<GrassEvents>()
            .init_resource::<GrassChunks>()
            .configure_sets(
                Update,
                (
                    GrassSet::Sources,
                    GrassSet::Stamp,
                    GrassSet::Simulate,
                    GrassSet::Upload,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    (wind::advance_wind, disturbance::advance_events).in_set(GrassSet::Sources),
                    disturbance::stamp_disturbances.in_set(GrassSet::Stamp),
                    field::step_field.in_set(GrassSet::Simulate),
                    material::upload_field.in_set(GrassSet::Upload),
                ),
            );
    }
}

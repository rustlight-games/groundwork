//! Drawing the battle.
//!
//! This crate is the only place fixed-point simulation state becomes `f32`, and
//! the conversion is strictly one-way. Nothing here writes back into the
//! simulation — if it did, the renderer's frame rate would start influencing
//! battle outcomes, and the headless trainer would stop agreeing with the game.
//!
//! ## Why interpolation
//!
//! The simulation runs at a fixed 64 Hz while the display runs at whatever the
//! monitor does. Drawing units at their raw tick position makes movement stutter
//! on any refresh rate that is not a multiple of 64. [`interpolate`] draws
//! between the previous and current tick instead, which costs one extra stored
//! position per unit and removes the problem entirely.

#![forbid(unsafe_code)]

pub mod camera;
pub mod debug;
pub mod interpolate;

use bevy::prelude::*;

pub use camera::{BattleCamera, CameraPlugin};
pub use interpolate::{PreviousPosition, RenderInterpolation};

/// Everything needed to see a battle.
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((CameraPlugin, interpolate::InterpolationPlugin))
            .add_plugins(debug::DebugDrawPlugin);
    }
}

/// Ordering for presentation systems.
///
/// Separate from the simulation's own sets: these run in Bevy's `Update`, after
/// the simulation has stepped, and only ever read.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderSet {
    /// Copy simulation state into render components.
    Sync,
    /// Interpolate between ticks and set transforms.
    Interpolate,
    /// Depth-sort sprites.
    Sort,
    /// Gizmos and overlays.
    Debug,
}

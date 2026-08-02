//! The battle camera.

use bevy::prelude::*;

/// Marks the camera that frames the battle.
#[derive(Component, Clone, Copy, Debug)]
pub struct BattleCamera {
    /// World units visible vertically. Zoom is expressed this way rather than
    /// as a scale factor so that framing rules can be written in the same units
    /// as the battlefield.
    pub view_height: f32,
}

impl Default for BattleCamera {
    fn default() -> Self {
        Self { view_height: 40.0 }
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((Camera2d, BattleCamera::default()));
}

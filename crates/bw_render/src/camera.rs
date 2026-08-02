//! The battle camera.

use bevy::camera::ScalingMode;
use bevy::prelude::*;

/// Marks the camera that frames the battle.
#[derive(Component, Clone, Copy, Debug)]
pub struct BattleCamera {
    /// World metres visible vertically.
    ///
    /// Zoom is expressed this way rather than as a scale factor so framing
    /// rules can be written in the same units as the battlefield — and so that
    /// changing the window size changes how much you see, never how big things
    /// are. A scale factor would make the game play differently on a bigger
    /// monitor.
    pub view_height: f32,
}

impl Default for BattleCamera {
    fn default() -> Self {
        // Close enough that knee-high grass is legible. The battlefield view
        // will pull back once there is a battle to frame.
        Self { view_height: 9.0 }
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, apply_zoom);
    }
}

fn spawn_camera(mut commands: Commands) {
    let camera = BattleCamera::default();
    commands.spawn((Camera2d, projection_for(camera.view_height), camera));
}

/// An orthographic projection framing `view_height` metres vertically.
pub fn projection_for(view_height: f32) -> Projection {
    Projection::Orthographic(OrthographicProjection {
        scaling_mode: ScalingMode::FixedVertical {
            viewport_height: view_height.max(0.01),
        },
        ..OrthographicProjection::default_2d()
    })
}

/// Keep the projection in step with [`BattleCamera::view_height`].
fn apply_zoom(mut cameras: Query<(&BattleCamera, &mut Projection), Changed<BattleCamera>>) {
    for (camera, mut projection) in &mut cameras {
        *projection = projection_for(camera.view_height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The framed height, or `None` if the projection is not what we asked for.
    fn framed_height(view_height: f32) -> Option<f32> {
        let Projection::Orthographic(orthographic) = projection_for(view_height) else {
            return None;
        };
        match orthographic.scaling_mode {
            ScalingMode::FixedVertical { viewport_height } => Some(viewport_height),
            _ => None,
        }
    }

    #[test]
    fn the_projection_frames_the_requested_height() {
        assert_eq!(framed_height(12.0), Some(12.0));
    }

    #[test]
    fn a_degenerate_zoom_does_not_produce_a_zero_sized_view() {
        assert!(framed_height(0.0).is_some_and(|h| h > 0.0));
        assert!(framed_height(-5.0).is_some_and(|h| h > 0.0));
    }

    #[test]
    fn the_depth_range_spans_both_directions() {
        // Grass writes a depth derived from world position, and half of the
        // field sits behind the origin. A near plane at zero would clip it.
        let Projection::Orthographic(orthographic) = projection_for(9.0) else {
            panic!("expected orthographic");
        };
        assert!(orthographic.near < 0.0);
        assert!(orthographic.far > 0.0);
    }
}

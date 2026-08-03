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
        // The projection is orthographic and 2:1 dimetric, so a `view_height` of
        // h shows h metres vertically and 16h/9 horizontally on a widescreen
        // window — h × 16h/9 square metres of ground.
        //
        // The genres this framing has to serve do not agree with each other.
        // Isometric farming and exploration games sit close: Stardew Valley
        // shows about fifteen metres vertically, and its relatives cluster
        // between fourteen and twenty. Real-time strategy sits much further
        // back — Warcraft III's melee camera shows roughly 24 × 13 tiles, about
        // twenty-six metres at the conventional two metres to a tile, and
        // plenty of RTS cameras go wider still.
        //
        // Fifty-five: about 98 × 55 metres of ground, which is a strategy camera
        // rather than a farming one. The deciding measurement is not how much
        // ground is visible but how big a *person* is on screen, because that is
        // the thing the eye actually calibrates against. At this height on a
        // 1080-pixel window a 1.8-metre figure stands about thirty-five pixels
        // tall, which is where Warcraft III and Age of Empires sit and is the
        // size a unit has to be for a formation to read as a formation. At
        // thirty-five metres the same figure is fifty-six pixels and the view
        // feels intimate — closer to an action game looking over one character
        // than to a commander watching a battle.
        //
        // What the width costs is worth writing down, because it is the kind of
        // thing someone later "fixes" without knowing it was decided. The ground
        // is a baked cache at ninety-six pixels to the metre and is displayed at
        // `window_height / view_height / 96` — here, a fifth. A blade of grass is
        // roughly a quarter of a metre, so it lands at about five screen pixels,
        // and the tip highlights average down to well under half the share that
        // was baked. At this framing the grass is a *texture*. It is not a field
        // of legible blades and no amount of per-blade work will make it one.
        //
        // Which settles an argument rather than losing one. Both art critiques of
        // this surface asked for the same things — larger regional shapes, calmer
        // ground between the clumps, less uniform micro-detail — and at a close
        // camera those are matters of taste. At this one they are forced:
        // anything below about ten screen pixels cannot be seen as detail, only
        // as noise, so everything the eye is going to read has to be built at the
        // scale of metres. Tune the ground against a `--view 55 --ruler` render,
        // never against a 1:1 plate.
        //
        // `BW_VIEW` overrides it at run time — see `bw_app`.
        Self { view_height: 55.0 }
    }
}

/// Zoom policy for the battle camera.
///
/// Deliberately does **not** spawn one. The camera that frames the battle is
/// also the camera that draws grass into its pixel canvas, and that bundle
/// belongs to `bw_grass`; a camera spawned here as well would render the scene
/// twice, once straight to the window with none of the pixel pipeline applied.
/// The composition root spawns it — see `bw_app`.
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, apply_zoom);
    }
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

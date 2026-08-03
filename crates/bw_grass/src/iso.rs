//! The isometric projection, and the one place the cache's pixel scale is set.
//!
//! Everything about the grass is decided in world ground coordinates and only
//! becomes isometric here. That ordering is load-bearing: a clump placed by
//! screen position would slide as the camera moved, and a mound shaped in screen
//! space would change shape when the view scrolled. Place in world space,
//! project once, and both problems stop existing.
//!
//! ## Two coordinate systems, both called "screen"
//!
//! [`project`] returns **screen metres** — Bevy 2D world space, +Y up. That is
//! what a mesh vertex wants.
//!
//! The baker works in **cache pixels** — image space, +Y down, at a fixed
//! [`PX_PER_METRE`]. That is what a rasteriser wants. [`to_cache`] converts.
//!
//! The cache is baked at one fixed zoom on purpose. Every art constant in this
//! crate — blade length, stroke width, mound diameter — is expressed in cache
//! pixels, because that is the unit the reference art is authored in. Zoom is
//! then the camera's business: it scales the finished page like any other
//! sprite, and the mip chain handles the rest.

use bevy::prelude::*;

/// Half the screen width of one world metre of X−Y separation.
///
/// One at the projection level, so a screen metre is a world metre and the
/// camera's `view_height` can be reasoned about in battlefield units.
pub const HALF_TILE_W: f32 = 1.0;

/// Half the screen height of one world metre of X+Y depth.
///
/// Half of [`HALF_TILE_W`], which is what makes a ground tile draw as the
/// familiar 2:1 dimetric diamond. It also makes the projection area-preserving:
/// a square metre of ground covers a square metre of screen.
pub const HALF_TILE_H: f32 = 0.5;

/// Screen metres per world metre of height.
///
/// Equal to [`HALF_TILE_W`]: a metre straight up is as long on screen as a metre
/// along the projected X axis, so a cube looks like a cube.
pub const Z_SCALE: f32 = 1.0;

/// Depth contributed per metre of ground distance from the camera.
pub const DEPTH_PER_GROUND: f32 = 0.5;

/// Depth contributed per metre of height.
///
/// Positive because an isometric camera looks down as well as along: raising a
/// point moves it *toward* the viewer. This is the whole reason a tall blade
/// rooted in front of a unit can draw over that unit's feet while a blade rooted
/// behind it cannot, with no per-blade sorting anywhere.
pub const DEPTH_PER_HEIGHT: f32 = 0.5;

/// Cache pixels per screen metre.
///
/// Sets how much detail a baked page holds. A blade of grass is about a quarter
/// of a metre, so at 96 it is roughly 24 pixels long — which is the stroke
/// length the reference art is drawn at, and the reason this number is 96 rather
/// than a rounder one.
pub const PX_PER_METRE: f32 = 96.0;

/// Project a world point onto the screen plane, in screen metres.
///
/// `world` is `(X, Y, Z)` with X and Y on the ground and Z up. The result is
/// Bevy 2D world space, where +Y is up the screen.
#[inline]
pub fn project(world: Vec3) -> Vec2 {
    Vec2::new(
        (world.x - world.y) * HALF_TILE_W,
        -(world.x + world.y) * HALF_TILE_H + world.z * Z_SCALE,
    )
}

/// Depth of a world point, larger being nearer the camera.
///
/// Used two ways, and they must agree: Bevy's 2D pipeline depth-tests with
/// `GreaterEqual` so a larger value wins on screen, and the baker's height
/// compositing resolves overlapping strokes the same way. Grass that sorted one
/// way against itself and another way against units would be worse than grass
/// that sorted badly in both.
#[inline]
pub fn depth(world: Vec3) -> f32 {
    (world.x + world.y) * DEPTH_PER_GROUND + world.z * DEPTH_PER_HEIGHT
}

/// Invert the projection back onto the ground plane at `Z = 0`.
///
/// Only the ground plane is recoverable — a screen point is a whole ray in world
/// space — and `Z = 0` is the choice that means "the ground under this pixel".
/// The baker leans on this heavily: a page is a rectangle in screen space, and
/// this is what tells it which patch of world it has to grow grass on.
#[inline]
pub fn unproject_ground(screen: Vec2) -> Vec2 {
    let difference = screen.x / HALF_TILE_W;
    let sum = -screen.y / HALF_TILE_H;
    Vec2::new((sum + difference) * 0.5, (sum - difference) * 0.5)
}

/// Project straight to cache pixels: image space, +Y down, origin at the world
/// origin.
#[inline]
pub fn to_cache(world: Vec3) -> Vec2 {
    let screen = project(world);
    Vec2::new(screen.x * PX_PER_METRE, -screen.y * PX_PER_METRE)
}

/// Invert [`to_cache`] onto the ground plane.
#[inline]
pub fn from_cache_ground(cache: Vec2) -> Vec2 {
    unproject_ground(Vec2::new(cache.x / PX_PER_METRE, -cache.y / PX_PER_METRE))
}

/// Metres of world height per cache pixel of screen rise.
pub const METRES_PER_PX_UP: f32 = 1.0 / (Z_SCALE * PX_PER_METRE);

/// How much cache one screenful covers, and how far it is scaled down to be
/// seen.
///
/// The two numbers this reconciles are set a long way apart in the codebase and
/// neither knows about the other. `bw_render::BattleCamera::view_height` is
/// world metres visible vertically; [`PX_PER_METRE`] is how many cache pixels a
/// screen metre is baked at. The ratio between them — how much a finished page
/// is shrunk before anyone sees it — belonged to neither, so until this existed
/// it was written down nowhere and was routinely assumed to be one.
///
/// It is not close to one. At the default 26-metre camera on a 1080-pixel
/// window the ground shows at about **43 percent**; at 35 metres, under a third.
/// A judgement made on a 1:1 plate is a judgement made at more than twice the
/// size the plate will ever be presented at, which is exactly the size at which
/// "richly detailed" and "busy" are hardest to tell apart.
///
/// Returns the cache-pixel extent to bake, and the scale it is shown at.
pub fn view_pixels(view_height: f32, screen: (usize, usize)) -> (usize, usize, f32) {
    let (screen_w, screen_h) = (screen.0.max(1), screen.1.max(1));
    let metres = view_height.max(0.01);
    // The projection is area-preserving and 2:1 dimetric, so a screen metre is a
    // world metre and the horizontal extent is just the aspect times the
    // vertical one.
    let across = metres * screen_w as f32 / screen_h as f32;
    let width = ((across * PX_PER_METRE).round() as usize).max(1);
    let height = ((metres * PX_PER_METRE).round() as usize).max(1);
    (width, height, screen_h as f32 / height as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn the_origin_projects_to_the_origin() {
        assert_eq!(project(Vec3::ZERO), Vec2::ZERO);
        assert_eq!(depth(Vec3::ZERO), 0.0);
        assert_eq!(to_cache(Vec3::ZERO), Vec2::ZERO);
    }

    #[test]
    fn unprojecting_a_projection_returns_the_ground_point() {
        for ground in [
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, -7.0),
            Vec2::new(-12.5, 4.25),
            Vec2::new(100.0, 100.0),
        ] {
            let back = unproject_ground(project(ground.extend(0.0)));
            assert!(
                close(back.x, ground.x) && close(back.y, ground.y),
                "{ground:?}"
            );
            let cached = from_cache_ground(to_cache(ground.extend(0.0)));
            assert!(
                close(cached.x, ground.x) && close(cached.y, ground.y),
                "{ground:?}"
            );
        }
    }

    #[test]
    fn height_moves_a_point_up_the_screen_without_shifting_it_sideways() {
        let ground = project(Vec3::new(2.0, 3.0, 0.0));
        let raised = project(Vec3::new(2.0, 3.0, 1.0));
        assert!(raised.y > ground.y);
        assert!(close(raised.x, ground.x));
    }

    #[test]
    fn the_two_ground_axes_separate_on_screen() {
        // Were +X and +Y to project the same way the view would be a side-on
        // elevation rather than an isometric one.
        let along_x = project(Vec3::X);
        let along_y = project(Vec3::Y);
        assert!(along_x.x > 0.0 && along_y.x < 0.0);
        assert!(along_x.y < 0.0 && along_y.y < 0.0);
    }

    #[test]
    fn moving_toward_the_camera_increases_depth() {
        assert!(depth(Vec3::X) > depth(Vec3::ZERO));
        assert!(depth(Vec3::Y) > depth(Vec3::ZERO));
        assert!(depth(Vec3::Z) > depth(Vec3::ZERO));
    }

    #[test]
    fn the_projection_preserves_ground_area() {
        // A square metre of ground has to cover a square metre of page, or the
        // baker's blades-per-square-metre would mean something different in one
        // direction than the other.
        let a = project(Vec3::X) - project(Vec3::ZERO);
        let b = project(Vec3::Y) - project(Vec3::ZERO);
        assert!(close((a.x * b.y - a.y * b.x).abs(), 1.0));
    }

    #[test]
    fn a_screenful_is_baked_much_larger_than_it_is_shown() {
        // The number the whole snapshot suite rests on. If this ever came back
        // near 1.0 the views would be being judged at the wrong scale and every
        // similarity figure would be measuring a picture nobody sees.
        let (width, height, scale) = view_pixels(26.0, (1920, 1080));
        assert_eq!(height, 2496);
        assert_eq!(width, 4437);
        assert!((scale - 0.4327).abs() < 1.0e-3, "{scale}");

        // Zooming out shrinks it further, never the other way.
        let (_, _, far) = view_pixels(48.0, (1920, 1080));
        assert!(far < scale);
    }

    #[test]
    fn a_view_keeps_the_screens_aspect_ratio() {
        let (width, height, _) = view_pixels(20.0, (1600, 900));
        assert!(((width as f32 / height as f32) - 16.0 / 9.0).abs() < 1.0e-2);
    }

    #[test]
    fn projection_is_linear_in_the_ground_plane() {
        // Relied on everywhere: a blade projects its root once and then adds
        // projected offsets rather than re-projecting each sample from scratch.
        let (a, b) = (Vec3::new(1.0, 2.0, 0.5), Vec3::new(-3.0, 0.5, 1.5));
        let sum = project(a + b);
        let parts = project(a) + project(b);
        assert!(close(sum.x, parts.x) && close(sum.y, parts.y));
    }
}

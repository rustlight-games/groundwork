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
//! The baker works in **cache pixels** — image space, +Y down. That is what a
//! rasteriser wants. [`to_cache`] converts, and [`to_cache_at`] does it at a
//! chosen scale.
//!
//! [`PX_PER_METRE`] is where the art is *authored*: every art constant in this
//! crate — blade length, stroke width, mound diameter — is expressed against it,
//! because that is the unit the reference art is drawn in. It used to be the
//! only scale anything was baked at as well, and that is the part that was
//! wrong. The camera decides how much of a metre reaches the screen, and at the
//! height this game ships at that is about a fifth, so a page baked at the
//! authoring scale did twenty-four pixels of work for every pixel anyone saw.
//!
//! So the authoring scale and the bake scale are now two different things. See
//! [`crate::page::Page::detail`] for what has to be carried through to move
//! between them — the short version is that a length in metres scales itself and
//! a length in cache pixels does not.
//!
//! ## This is the fast path, not the definition
//!
//! [`terrain_scene::projection::Projection`] is where the projection is
//! *defined*. It is `f64`, it is parameterised, and it is what the Cycles
//! camera, the scene IR, the tile layout and the frame resolver are all written
//! against — because the projection is a contract between renderers rather than
//! one renderer's detail.
//!
//! What lives here is the `f32` inner loop: a blade's rasteriser projects a
//! point per rib per blade, several million times a plate, and doing that in
//! `f64` through a struct is measurably slower for a result that rounds to the
//! same pixel.
//!
//! The two must agree, and "must agree" is worthless unless something checks.
//! [`tests`] compares them point for point — ground, elevated, negative, and far
//! from the origin — and pins each constant here against its `f64` counterpart.
//! The one thing that must *not* happen is this file being rewritten to call
//! through to the `f64` path: identical values are not the requirement, bitwise
//! identical *results* are, and re-associating this arithmetic moves every
//! pinned fingerprint in the repository.

use glam::{Vec2, Vec3};

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
    to_cache_at(world, PX_PER_METRE)
}

/// Invert [`to_cache`] onto the ground plane.
#[inline]
pub fn from_cache_ground(cache: Vec2) -> Vec2 {
    from_cache_ground_at(cache, PX_PER_METRE)
}

/// [`to_cache`] at a chosen cache scale.
///
/// [`PX_PER_METRE`] is the scale the art is *authored* at, and for a long time
/// it was the only scale anything was baked at. It is not the scale a page has
/// to be baked at. The camera decides how much of a metre reaches the screen,
/// and at the height this game ships at that is about a fifth — so a page baked
/// at the authoring scale spends twenty-four pixels of work on every pixel the
/// player sees, and then throws twenty-three of them away in the minification
/// filter.
///
/// Baking at the scale the page will be *shown* at is therefore not a quality
/// setting, it is the removal of a mistake. See [`crate::page::Page::detail`]
/// for what has to travel with it: every length the art expresses in cache
/// pixels has to shrink by the same factor, or the blades keep their pixel width
/// while the field shrinks around them and the grass turns to bristles.
#[inline]
pub fn to_cache_at(world: Vec3, px_per_metre: f32) -> Vec2 {
    let screen = project(world);
    Vec2::new(screen.x * px_per_metre, -screen.y * px_per_metre)
}

/// Invert [`to_cache_at`] onto the ground plane.
#[inline]
pub fn from_cache_ground_at(cache: Vec2, px_per_metre: f32) -> Vec2 {
    unproject_ground(Vec2::new(cache.x / px_per_metre, -cache.y / px_per_metre))
}

/// The world direction that runs to the right of the screen, unit length.
///
/// One of three axes that turn image space into world space and back. They earn
/// their place because the renderer has always described its key light in
/// *image* coordinates — right, down, and toward the viewer — and a surface
/// normal is unavoidably a world quantity. Shading one against the other
/// requires the bridge to be written down once rather than approximated at each
/// call site.
///
/// Derived rather than chosen: [`project`] maps a world step to
/// `(x − y, −(x + y)/2 + z)`, so the world direction that moves a point purely
/// rightwards is the one with `x + y = 0` and no height, which is `(1, −1, 0)`.
pub const VIEW_RIGHT: Vec3 = Vec3::new(
    std::f32::consts::FRAC_1_SQRT_2,
    -std::f32::consts::FRAC_1_SQRT_2,
    0.0,
);

/// The world direction that runs **down** the screen, unit length.
///
/// Down, because the baker works in image space where +Y counts downward. It
/// must move a point down the screen without moving it sideways and without
/// changing its depth, which pins it to `(1, 1, −2)`: the depth of that step is
/// `(1 + 1)/2 + (−2)/2 = 0`, and the screen rise is `−(1 + 1)/2 + (−2) = −3`.
///
/// It has a negative height component, and that is the whole reason an
/// isometric camera cannot treat "down the screen" and "along the ground" as the
/// same thing.
const DOWN_SCALE: f32 = 0.408_248_3; // 1/√6
pub const VIEW_DOWN: Vec3 = Vec3::new(DOWN_SCALE, DOWN_SCALE, -2.0 * DOWN_SCALE);

/// The world direction pointing out of the screen at the viewer, unit length.
///
/// `(1, 1, 1)`, and it is the gradient of [`depth`] — which is the definition of
/// "toward the camera" in a projection with no perspective. A world step along
/// it changes nothing on screen and everything about what is in front of what.
const VIEWER_SCALE: f32 = 0.577_350_3; // 1/√3
pub const TOWARD_VIEWER: Vec3 = Vec3::new(VIEWER_SCALE, VIEWER_SCALE, VIEWER_SCALE);

/// An image-space direction as a world one.
///
/// The renderer's key light is authored as `(right, down, toward the viewer)`,
/// which is convenient for deciding which side of a stroke gets its dark
/// under-mark and useless for asking whether a leaf faces the sun. This is that
/// conversion, and it comes with a warning worth stating loudly:
///
/// **Image `+Z` is not up.** It is toward the camera, and this camera looks down
/// at about 35°, so a light with a large image `z` is a light that is somewhat
/// up and substantially *behind the viewer's shoulder*. Every lighting term in
/// the old baker that treated `light.z` as an up-ness was really reading
/// toward-the-viewer-ness, which is a large part of why the field had tone
/// variation and no sense of a sun.
#[inline]
pub fn image_to_world(image: Vec3) -> Vec3 {
    VIEW_RIGHT * image.x + VIEW_DOWN * image.y + TOWARD_VIEWER * image.z
}

/// A world direction as an image-space one. The inverse of [`image_to_world`].
#[inline]
pub fn world_to_image(world: Vec3) -> Vec3 {
    Vec3::new(
        world.dot(VIEW_RIGHT),
        world.dot(VIEW_DOWN),
        world.dot(TOWARD_VIEWER),
    )
}

/// How high a world direction stands above the ground plane, radians.
#[inline]
pub fn elevation_of(world: Vec3) -> f32 {
    world.normalize_or(Vec3::Z).z.clamp(-1.0, 1.0).asin()
}

/// Metres of world height per cache pixel of screen rise.
pub const METRES_PER_PX_UP: f32 = 1.0 / (Z_SCALE * PX_PER_METRE);

/// How much cache one screenful covers, and how far it is scaled down to be
/// seen.
///
/// The two numbers this reconciles are set a long way apart in the codebase and
/// neither knows about the other. `terrain_bench::scenarios::RTS_VIEW_M` is
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
    fn the_view_basis_is_orthonormal() {
        for axis in [VIEW_RIGHT, VIEW_DOWN, TOWARD_VIEWER] {
            assert!((axis.length() - 1.0).abs() < 1.0e-5, "{axis:?}");
        }
        assert!(VIEW_RIGHT.dot(VIEW_DOWN).abs() < 1.0e-5);
        assert!(VIEW_RIGHT.dot(TOWARD_VIEWER).abs() < 1.0e-5);
        assert!(VIEW_DOWN.dot(TOWARD_VIEWER).abs() < 1.0e-5);
    }

    #[test]
    fn the_view_axes_do_what_they_are_named() {
        // Each has to move a point exactly one way on screen and no other way,
        // or the image-to-world bridge silently mixes the light's components.
        let right = to_cache(VIEW_RIGHT) - to_cache(Vec3::ZERO);
        assert!(right.x > 0.0 && right.y.abs() < 1.0e-4, "{right:?}");
        let down = to_cache(VIEW_DOWN) - to_cache(Vec3::ZERO);
        assert!(down.y > 0.0 && down.x.abs() < 1.0e-4, "{down:?}");
        let toward = to_cache(TOWARD_VIEWER) - to_cache(Vec3::ZERO);
        assert!(toward.length() < 1.0e-4, "{toward:?} moved on screen");
        assert!(depth(TOWARD_VIEWER) > depth(Vec3::ZERO));
    }

    #[test]
    fn the_image_and_world_bridges_invert_each_other() {
        for image in [
            Vec3::new(-0.42, -0.40, 0.81).normalize(),
            Vec3::new(0.3, 0.6, 0.74).normalize(),
            Vec3::Z,
            Vec3::X,
        ] {
            let back = world_to_image(image_to_world(image));
            assert!((back - image).length() < 1.0e-5, "{image:?} → {back:?}");
        }
    }

    #[test]
    fn image_z_is_not_up() {
        // The trap this bridge exists to remove, asserted so nobody has to
        // rediscover it. A light straight out of the screen is a light 35°
        // above the horizon, not one overhead — the camera is tilted, and every
        // term that read `light.z` as an up-ness was reading something else.
        let overhead = elevation_of(image_to_world(Vec3::Z)).to_degrees();
        assert!(
            (overhead - 35.264).abs() < 0.05,
            "straight out of the screen is {overhead}° above the ground"
        );
        // And the field's own key, which reads as high in image space, is not
        // as high as it looks.
        let key = image_to_world(Vec3::new(-0.42, -0.40, 0.81).normalize());
        let elevation = elevation_of(key).to_degrees();
        assert!(
            (40.0..65.0).contains(&elevation),
            "the field's key sits at {elevation}° above the ground"
        );
    }

    /// A spread of points chosen to catch the ways two projections drift apart:
    /// the origin, each axis alone, height alone, negative ground, and a point
    /// far enough out that `f32` has started to lose the low bits.
    fn parity_points() -> [Vec3; 9] {
        [
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(3.0, -7.0, 0.0),
            Vec3::new(-12.5, 4.25, 0.31),
            Vec3::new(-2852.0, 1136.0, 0.0),
            Vec3::new(8192.0, 8192.0, 1.5),
            Vec3::new(0.001, -0.001, 0.0),
        ]
    }

    fn canonical() -> terrain_scene::projection::Projection {
        terrain_scene::projection::Projection::DIMETRIC_2_1
    }

    #[test]
    fn every_constant_here_matches_the_projection_it_is_a_fast_path_for() {
        // The cheapest half of the parity check, and the one that fails first if
        // somebody retunes one file and not the other.
        let canonical = canonical();
        assert_eq!(HALF_TILE_W as f64, canonical.half_width);
        assert_eq!(HALF_TILE_H as f64, canonical.half_height);
        assert_eq!(Z_SCALE as f64, canonical.height_scale);
        assert_eq!(DEPTH_PER_GROUND as f64, canonical.depth_per_ground);
        assert_eq!(DEPTH_PER_HEIGHT as f64, canonical.depth_per_height);
    }

    #[test]
    fn this_projection_and_the_scenes_agree_point_for_point() {
        // The check that lets the tile layout, the frame resolver and the Cycles
        // camera be written against the `f64` projection while the rasteriser's
        // inner loop stays in `f32`. Without it, "they are the same projection"
        // is a comment rather than a fact.
        let canonical = canonical();
        for point in parity_points() {
            let fast = project(point);
            let exact = canonical.project(terrain_scene::projection::ScenePoint::new(
                point.x as f64,
                point.y as f64,
                point.z as f64,
            ));
            // A relative tolerance, because the far point is eight thousand
            // metres out and an absolute one there would be either meaningless
            // or unsatisfiable.
            let scale = (exact.x_m.abs().max(exact.y_m.abs())).max(1.0);
            assert!(
                (fast.x as f64 - exact.x_m).abs() < 1.0e-5 * scale
                    && (fast.y as f64 - exact.y_m).abs() < 1.0e-5 * scale,
                "{point:?}: {fast:?} against ({}, {})",
                exact.x_m,
                exact.y_m
            );

            let exact_depth = canonical.depth(terrain_scene::projection::ScenePoint::new(
                point.x as f64,
                point.y as f64,
                point.z as f64,
            ));
            assert!(
                (depth(point) as f64 - exact_depth).abs() < 1.0e-5 * scale,
                "{point:?}: depth {} against {exact_depth}",
                depth(point)
            );
        }
    }

    #[test]
    fn both_inversions_land_on_the_same_ground() {
        let canonical = canonical();
        for point in parity_points() {
            let screen = project(point);
            let fast = unproject_ground(screen);
            let exact = canonical.unproject_ground(terrain_scene::projection::ScreenPoint::new(
                screen.x as f64,
                screen.y as f64,
            ));
            let scale = (exact.u_m.abs().max(exact.v_m.abs())).max(1.0);
            assert!(
                (fast.x as f64 - exact.u_m).abs() < 1.0e-5 * scale
                    && (fast.y as f64 - exact.v_m).abs() < 1.0e-5 * scale,
                "{point:?}: {fast:?} against {exact}"
            );
        }
    }

    #[test]
    fn the_view_axes_agree_across_the_two_projections() {
        // The image-to-world bridge is written in `f32` here and derived in
        // `f64` there. A disagreement would light the two renderers' grass from
        // slightly different directions, which reads as a material difference.
        let canonical = canonical();
        let right = canonical.view_right();
        assert!((VIEW_RIGHT.x as f64 - right[0]).abs() < 1.0e-6);
        assert!((VIEW_RIGHT.y as f64 - right[1]).abs() < 1.0e-6);
        assert!((VIEW_RIGHT.z as f64 - right[2]).abs() < 1.0e-6);

        let toward = canonical.toward_viewer();
        assert!((TOWARD_VIEWER.x as f64 - toward[0]).abs() < 1.0e-6);
        assert!((TOWARD_VIEWER.y as f64 - toward[1]).abs() < 1.0e-6);
        assert!((TOWARD_VIEWER.z as f64 - toward[2]).abs() < 1.0e-6);
    }

    #[test]
    fn a_square_world_tile_projects_to_a_two_to_one_diamond() {
        // The property the nine-tile framing is fitted against, asserted on the
        // side of the projection that actually rasterises it. A tile of side S
        // is 2S across and S tall on screen, so three by three is 6S × 3S.
        for side in [1.0f32, 4.0, 7.5] {
            let corners = [
                project(Vec3::new(0.0, 0.0, 0.0)),
                project(Vec3::new(side, 0.0, 0.0)),
                project(Vec3::new(side, side, 0.0)),
                project(Vec3::new(0.0, side, 0.0)),
            ];
            let (mut low, mut high) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
            for corner in corners {
                low = low.min(corner);
                high = high.max(corner);
            }
            assert!(close(high.x - low.x, 2.0 * side), "{side}");
            assert!(close(high.y - low.y, side), "{side}");
        }
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

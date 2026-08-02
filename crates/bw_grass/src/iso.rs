//! The isometric projection.
//!
//! Grass is simulated on the world's flat X,Y ground plane and only becomes
//! isometric here, at the last moment before it is drawn. That ordering is the
//! entire reason a blade pushed west behaves like a blade pushed north: the
//! simulation never sees the projection, so the projection cannot bias it. Do
//! the reverse — simulate in screen space — and the same shove produces a
//! different result depending on which way the player happens to be facing,
//! which is the sort of wrongness you feel before you can name it.
//!
//! ## Why blades reconstruct a virtual third dimension
//!
//! A blade is not a sprite that shears. It is a curve through `(X, Y, Z)` that
//! happens to be drawn on a flat screen. Bending it in three dimensions and
//! then projecting gives, for free, the three things that sell the effect: the
//! silhouette shortens as the blade leans, the tip travels along an ellipse
//! rather than a straight line, and grass laid flat toward the camera covers
//! more ground than grass laid flat away from it. Shearing a sprite gives none
//! of those, which is why sheared grass reads as rubber.
//!
//! ## Screen units are metres
//!
//! [`HALF_TILE_W`] and friends are deliberately around one, so that a screen
//! unit is roughly a world metre and the camera's `view_height` can be reasoned
//! about in the same units as the battlefield. Zoom is the camera's business,
//! not the projection's.
//!
//! The same constants are duplicated in `assets/shaders/grass.wgsl`, which runs
//! this projection per vertex on the GPU. That duplication is a real risk, so
//! `shader_constants_match_this_module` reads the shader source and fails if
//! the two ever drift apart.

use bevy::prelude::*;

/// Half the screen width of one world metre of X-Y separation.
///
/// With [`HALF_TILE_H`] at half this, one ground tile draws as the familiar
/// 2:1 dimetric diamond.
pub const HALF_TILE_W: f32 = 1.0;

/// Half the screen height of one world metre of X+Y depth.
pub const HALF_TILE_H: f32 = 0.5;

/// Screen units per world metre of height.
///
/// Equal to [`HALF_TILE_W`]: a metre straight up is as long on screen as a
/// metre along the projected X axis, which is what makes a cube look like a
/// cube rather than a squashed box.
pub const Z_SCALE: f32 = 1.0;

/// Depth contributed per metre of ground distance from the camera.
pub const DEPTH_PER_GROUND: f32 = 0.5;

/// Depth contributed per metre of height.
///
/// Positive because a true isometric camera looks down as well as along: a
/// point raised straight up moves *toward* the viewer. This is what lets a tall
/// blade in front of a unit draw over that unit's legs while a blade rooted
/// behind it does not, with no per-blade sorting anywhere.
pub const DEPTH_PER_HEIGHT: f32 = 0.5;

/// Project a world point onto the screen plane.
///
/// `world` is `(X, Y, Z)` with X and Y on the ground and Z up. The result is in
/// Bevy's 2D world space, where +Y is up the screen.
pub fn project(world: Vec3) -> Vec2 {
    Vec2::new(
        (world.x - world.y) * HALF_TILE_W,
        -(world.x + world.y) * HALF_TILE_H + world.z * Z_SCALE,
    )
}

/// Depth of a world point, larger being nearer the camera.
///
/// Bevy's 2D pipeline depth-tests with `GreaterEqual`, so a larger value wins.
/// Feeding this through the mesh's vertex Z is what gives grass per-fragment
/// sorting against itself and, later, against units — rather than the
/// per-sprite ordering that breaks the moment a blade leans across a
/// neighbour.
pub fn depth(world: Vec3) -> f32 {
    (world.x + world.y) * DEPTH_PER_GROUND + world.z * DEPTH_PER_HEIGHT
}

/// Project to the position a vertex should carry: screen X, screen Y, depth.
pub fn project_to_vertex(world: Vec3) -> Vec3 {
    let screen = project(world);
    screen.extend(depth(world))
}

/// Invert the projection back onto the ground plane at `Z = 0`.
///
/// What a mouse click needs. Only the ground plane is recoverable — a screen
/// point corresponds to a whole ray in world space, and picking `Z = 0` is the
/// choice that means "the ground under the cursor".
pub fn unproject_ground(screen: Vec2) -> Vec2 {
    // s_x = (X - Y) * HALF_TILE_W        =>  X - Y = s_x / HALF_TILE_W
    // s_y = -(X + Y) * HALF_TILE_H       =>  X + Y = -s_y / HALF_TILE_H
    let difference = screen.x / HALF_TILE_W;
    let sum = -screen.y / HALF_TILE_H;
    Vec2::new((sum + difference) * 0.5, (sum - difference) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn the_origin_projects_to_the_origin() {
        assert_eq!(project(Vec3::ZERO), Vec2::ZERO);
        assert_eq!(depth(Vec3::ZERO), 0.0);
    }

    #[test]
    fn unprojecting_a_projection_returns_the_ground_point() {
        for ground in [
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, -7.0),
            Vec2::new(-12.5, 4.25),
            Vec2::new(100.0, 100.0),
        ] {
            let screen = project(ground.extend(0.0));
            let back = unproject_ground(screen);
            assert!(
                close(back.x, ground.x) && close(back.y, ground.y),
                "{ground:?} -> {screen:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn height_moves_a_point_up_the_screen() {
        let ground = project(Vec3::new(2.0, 3.0, 0.0));
        let raised = project(Vec3::new(2.0, 3.0, 1.0));
        assert!(raised.y > ground.y);
        assert!(close(raised.x, ground.x), "height must not shift sideways");
    }

    #[test]
    fn the_two_ground_axes_separate_on_screen() {
        // If +X and +Y projected to the same screen direction the view would be
        // a side-on elevation, not an isometric one.
        let along_x = project(Vec3::X);
        let along_y = project(Vec3::Y);
        assert!(along_x.x > 0.0 && along_y.x < 0.0);
        assert!(along_x.y < 0.0 && along_y.y < 0.0);
    }

    #[test]
    fn moving_toward_the_camera_increases_depth() {
        // Both ground axes point away from the camera as they decrease.
        assert!(depth(Vec3::new(1.0, 0.0, 0.0)) > depth(Vec3::ZERO));
        assert!(depth(Vec3::new(0.0, 1.0, 0.0)) > depth(Vec3::ZERO));
        // And raising a point brings it nearer, so a tall blade can cover what
        // is rooted behind it.
        assert!(depth(Vec3::new(0.0, 0.0, 1.0)) > depth(Vec3::ZERO));
    }

    #[test]
    fn projection_is_linear_in_the_ground_plane() {
        // Relied on by the shader, which projects a blade's root once and then
        // adds projected offsets rather than re-projecting every segment.
        let a = Vec3::new(1.0, 2.0, 0.5);
        let b = Vec3::new(-3.0, 0.5, 1.5);
        let sum = project(a + b);
        let parts = project(a) + project(b);
        assert!(close(sum.x, parts.x) && close(sum.y, parts.y));
    }

    #[test]
    fn a_world_rotation_is_not_a_screen_rotation() {
        // The property that makes simulating in world space necessary: equal
        // world displacements in different directions do *not* map to equal
        // screen displacements, so screen-space grass would respond to a shove
        // differently depending on its direction.
        let east = project(Vec3::X).length();
        let north = project(Vec3::Y).length();
        let diagonal = project(Vec3::new(1.0, 1.0, 0.0).normalize()).length();
        assert!(close(east, north), "the two ground axes are symmetric");
        assert!(
            !close(east, diagonal),
            "but a diagonal is foreshortened differently: {east} vs {diagonal}"
        );
    }

    /// The projection lives in two places — here and in the vertex shader — and
    /// nothing but this test stops them drifting apart. A mismatch is
    /// particularly nasty because it looks fine until grass and units disagree
    /// about where the ground is.
    #[test]
    fn shader_constants_match_this_module() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/grass.wgsl"
        );
        let source = std::fs::read_to_string(path).expect("the grass shader must exist");

        for (name, value) in [
            ("HALF_TILE_W", HALF_TILE_W),
            ("HALF_TILE_H", HALF_TILE_H),
            ("Z_SCALE", Z_SCALE),
            ("DEPTH_PER_GROUND", DEPTH_PER_GROUND),
            ("DEPTH_PER_HEIGHT", DEPTH_PER_HEIGHT),
        ] {
            let needle = format!("const {name}: f32 = {value:?};");
            assert!(
                source.contains(&needle),
                "grass.wgsl must declare `{needle}` to stay in step with iso.rs"
            );
        }
    }
}

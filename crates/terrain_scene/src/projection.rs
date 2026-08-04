//! How ground becomes screen.
//!
//! This used to live inside the grass crate, and moving it is not tidying. The
//! projection is a **contract between renderers**: the rasteriser draws through
//! it, the Cycles camera is derived from it, the dataset's two halves have to
//! register pixel for pixel, and a debug overlay has to land on the ground it is
//! annotating. Four consumers, one transform, and any disagreement between them
//! is an artefact nobody can attribute.
//!
//! ## The projection is orthogonal, anisotropic, and left-handed
//!
//! Three properties, each of which has cost a wasted render to discover.
//!
//! **Orthogonal.** `screen.x = (u − v) · half_width` and
//! `screen.y = −(u + v) · half_height + z · height_scale`. Written as basis
//! vectors those are `r = (1, −1, 0)` and `up = (−½, −½, 1)`, and `r · up = 0`.
//! So this really is an orthographic view, down the axis `r × up`.
//!
//! **Anisotropic.** Their *lengths* differ: `|r| = √2` and `|up| = √3/2`. The
//! projection therefore stretches horizontally by `2/√3 ≈ 1.1547` relative to a
//! rigid orthographic view, and that factor is the entire difference between the
//! 2:1 dimetric diamond this draws and true isometric. No camera transform can
//! express it, because a transform that scales one screen axis is not a
//! rotation — which is why a path tracer has to carry it as a non-square pixel.
//! See [`Projection::pixel_aspect`].
//!
//! **Left-handed.** Take the basis at face value and `r × up` points *below* the
//! ground looking up. That is not a sign slip; it is a property of the
//! convention. A physical camera above `+u+v+z` sees `+u` go left, and this
//! sends `+u` right. Tile-based projections are picked to suit the tile grid
//! rather than a physical camera, and they are self-consistent because
//! everything inside them agrees.
//!
//! A path tracer cannot be handed a mirrored camera, so the *world* is reflected
//! instead — across the plane `u = v`, which is a swap of the two ground axes.
//! See [`Projection::to_right_handed`]. The rule this creates is worth stating
//! as loudly as possible: **nothing may cross that boundary without the swap.**
//! A blade reflected while its sun is not would be lit from the wrong side, and
//! it would look entirely plausible.
//!
//! ## Depth is height-aware, and that is what makes grass work
//!
//! [`Projection::depth`] adds a positive term for height, because an isometric
//! camera looks *down* as well as along: raising a point moves it toward the
//! viewer. That single sign is why a tall blade rooted in front of an object can
//! draw over that object's feet while one rooted behind it cannot, with no
//! per-mark sorting anywhere.

use terrain_core::coords::{WorldPoint, WorldRect};

/// A world position with height, in metres.
///
/// Distinct from [`WorldPoint`] because the terrain is a function of the ground
/// plane and a mark is not: a blade has a root height, an arc that leaves the
/// ground, and a tip somewhere above both.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ScenePoint {
    pub u_m: f64,
    pub v_m: f64,
    pub z_m: f64,
}

impl ScenePoint {
    pub const fn new(u_m: f64, v_m: f64, z_m: f64) -> Self {
        Self { u_m, v_m, z_m }
    }

    pub fn on_ground(point: WorldPoint) -> Self {
        Self::new(point.u_m, point.v_m, 0.0)
    }

    pub fn ground(self) -> WorldPoint {
        WorldPoint::new(self.u_m, self.v_m)
    }

    pub fn raised(self, by_m: f64) -> Self {
        Self::new(self.u_m, self.v_m, self.z_m + by_m)
    }
}

/// A position on the screen plane, in screen metres.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ScreenPoint {
    pub x_m: f64,
    /// Positive up the screen.
    pub y_m: f64,
}

impl ScreenPoint {
    pub const fn new(x_m: f64, y_m: f64) -> Self {
        Self { x_m, y_m }
    }
}

/// The transform from ground to screen.
///
/// Parameterised rather than hard-coded, so that a document can eventually
/// choose a different tile ratio — but with the defaults pinned, because the
/// existing art is authored against them and every accepted snapshot assumes
/// them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    /// Half the screen width of one metre of `u − v` separation.
    ///
    /// One, so a screen metre is a world metre and a camera height can be
    /// reasoned about in ground units.
    pub half_width: f64,
    /// Half the screen height of one metre of `u + v` depth.
    ///
    /// Half of [`Projection::half_width`], which makes a ground tile draw as the
    /// familiar 2:1 dimetric diamond *and* makes the projection
    /// area-preserving: a square metre of ground covers a square metre of
    /// screen. That second property is quietly load-bearing — it is what lets a
    /// population's density be stated per square metre of ground and mean the
    /// same thing per square metre of plate.
    pub half_height: f64,
    /// Screen metres per metre of height.
    ///
    /// Equal to [`Projection::half_width`], so a cube looks like a cube.
    pub height_scale: f64,
    /// Depth contributed per metre of ground distance from the camera.
    pub depth_per_ground: f64,
    /// Depth contributed per metre of height.
    ///
    /// Positive. See the module note: this is the sign that makes a tall mark
    /// rooted in front draw over what is behind it.
    pub depth_per_height: f64,
}

impl Default for Projection {
    fn default() -> Self {
        Self::DIMETRIC_2_1
    }
}

impl Projection {
    /// The 2:1 dimetric projection everything here is authored against.
    pub const DIMETRIC_2_1: Self = Self {
        half_width: 1.0,
        half_height: 0.5,
        height_scale: 1.0,
        depth_per_ground: 0.5,
        depth_per_height: 0.5,
    };

    /// Project a scene point onto the screen plane.
    #[inline]
    pub fn project(self, point: ScenePoint) -> ScreenPoint {
        ScreenPoint::new(
            (point.u_m - point.v_m) * self.half_width,
            -(point.u_m + point.v_m) * self.half_height + point.z_m * self.height_scale,
        )
    }

    /// How near the camera a point is. Larger wins.
    ///
    /// Used two ways that must agree: a rasteriser resolves overlapping marks
    /// with it, and a GPU pipeline depth-tests with it. Marks that sorted one
    /// way against each other and another way against everything else would be
    /// worse than marks that sorted badly in both.
    #[inline]
    pub fn depth(self, point: ScenePoint) -> f64 {
        (point.u_m + point.v_m) * self.depth_per_ground + point.z_m * self.depth_per_height
    }

    /// Invert the projection onto the ground plane at `z = 0`.
    ///
    /// Only the ground plane is recoverable — a screen point is a whole ray —
    /// and `z = 0` is the choice that means "the ground under this pixel".
    #[inline]
    pub fn unproject_ground(self, screen: ScreenPoint) -> WorldPoint {
        let difference = screen.x_m / self.half_width;
        let sum = -screen.y_m / self.half_height;
        WorldPoint::new((sum + difference) * 0.5, (sum - difference) * 0.5)
    }

    /// The screen rectangle a ground rectangle projects into.
    ///
    /// The ground rectangle's four corners, projected and bounded. Note that a
    /// ground rectangle does **not** project to a screen rectangle — it projects
    /// to a diamond — so this is the bounding box of that diamond and is
    /// genuinely larger than the ground it came from.
    pub fn screen_bounds(self, ground: WorldRect) -> (ScreenPoint, ScreenPoint) {
        let corners = [
            ScenePoint::new(ground.min.u_m, ground.min.v_m, 0.0),
            ScenePoint::new(ground.max.u_m, ground.min.v_m, 0.0),
            ScenePoint::new(ground.min.u_m, ground.max.v_m, 0.0),
            ScenePoint::new(ground.max.u_m, ground.max.v_m, 0.0),
        ];
        let mut low = ScreenPoint::new(f64::INFINITY, f64::INFINITY);
        let mut high = ScreenPoint::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for corner in corners {
            let screen = self.project(corner);
            low.x_m = low.x_m.min(screen.x_m);
            low.y_m = low.y_m.min(screen.y_m);
            high.x_m = high.x_m.max(screen.x_m);
            high.y_m = high.y_m.max(screen.y_m);
        }
        (low, high)
    }

    /// The anisotropy, as a pixel taller than it is wide.
    ///
    /// What a path tracer's camera has to carry, because a rigid orthographic
    /// view cannot stretch one screen axis. `2/√3` for the default projection.
    pub fn pixel_aspect(self) -> f64 {
        let horizontal = (self.half_width * 2.0f64.sqrt()) / self.half_width;
        let vertical = {
            // |up| for the basis (−half_height, −half_height, height_scale),
            // normalised by the screen scale it produces.
            let up = (self.half_height * self.half_height * 2.0
                + self.height_scale * self.height_scale)
                .sqrt();
            up / self.height_scale
        };
        horizontal / vertical
    }

    /// The world direction that runs to the right of the screen, unit length.
    ///
    /// Derived rather than chosen: the world direction that moves a point purely
    /// rightwards is the one with `u + v = 0` and no height.
    pub fn view_right(self) -> [f64; 3] {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        [s, -s, 0.0]
    }

    /// The world direction pointing out of the screen at the viewer.
    ///
    /// The gradient of [`Projection::depth`], which is the definition of "toward
    /// the camera" in a projection with no perspective.
    pub fn toward_viewer(self) -> [f64; 3] {
        let s = 1.0 / 3.0f64.sqrt();
        [s, s, s]
    }

    /// Reflect a scene point into a right-handed space.
    ///
    /// A swap of the two ground axes, and the *only* sanctioned way across the
    /// boundary to a physical renderer. See the module note: a point reflected
    /// while its light is not is lit from the wrong side, and it looks plausible.
    pub fn to_right_handed(self, point: ScenePoint) -> ScenePoint {
        ScenePoint::new(point.v_m, point.u_m, point.z_m)
    }

    /// Reflect a world-space bearing, measured from `+u` toward `+v`.
    ///
    /// The companion to [`Projection::to_right_handed`] for directions. A
    /// reflection across `u = v` sends a bearing `θ` to `π/2 − θ`.
    pub fn bearing_to_right_handed(self, bearing_rad: f64) -> f64 {
        std::f64::consts::FRAC_PI_2 - bearing_rad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1.0e-9
    }

    #[test]
    fn the_origin_projects_to_the_origin() {
        let projection = Projection::default();
        assert_eq!(
            projection.project(ScenePoint::default()),
            ScreenPoint::default()
        );
        assert_eq!(projection.depth(ScenePoint::default()), 0.0);
    }

    #[test]
    fn unprojecting_a_projection_returns_the_ground_point() {
        let projection = Projection::default();
        for ground in [
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(3.0, -7.0),
            WorldPoint::new(-12.5, 4.25),
            WorldPoint::new(1000.0, 1000.0),
        ] {
            let back =
                projection.unproject_ground(projection.project(ScenePoint::on_ground(ground)));
            assert!(
                close(back.u_m, ground.u_m) && close(back.v_m, ground.v_m),
                "{ground}"
            );
        }
    }

    #[test]
    fn height_moves_a_point_up_the_screen_without_shifting_it_sideways() {
        let projection = Projection::default();
        let ground = projection.project(ScenePoint::new(2.0, 3.0, 0.0));
        let raised = projection.project(ScenePoint::new(2.0, 3.0, 1.0));
        assert!(raised.y_m > ground.y_m);
        assert!(close(raised.x_m, ground.x_m));
    }

    #[test]
    fn the_two_ground_axes_separate_on_screen() {
        // Were +u and +v to project the same way, the view would be a side-on
        // elevation rather than an isometric one.
        let projection = Projection::default();
        let along_u = projection.project(ScenePoint::new(1.0, 0.0, 0.0));
        let along_v = projection.project(ScenePoint::new(0.0, 1.0, 0.0));
        assert!(along_u.x_m > 0.0 && along_v.x_m < 0.0);
        assert!(along_u.y_m < 0.0 && along_v.y_m < 0.0);
    }

    #[test]
    fn the_projection_preserves_ground_area() {
        // Quietly load-bearing: it is what lets a population's density be stated
        // per square metre of ground and mean the same per square metre of plate.
        let projection = Projection::default();
        let origin = projection.project(ScenePoint::default());
        let a = projection.project(ScenePoint::new(1.0, 0.0, 0.0));
        let b = projection.project(ScenePoint::new(0.0, 1.0, 0.0));
        let (ax, ay) = (a.x_m - origin.x_m, a.y_m - origin.y_m);
        let (bx, by) = (b.x_m - origin.x_m, b.y_m - origin.y_m);
        assert!(close((ax * by - ay * bx).abs(), 1.0));
    }

    #[test]
    fn raising_a_point_brings_it_toward_the_camera() {
        // The sign that makes a tall mark rooted in front draw over what is
        // behind it, with no per-mark sorting anywhere.
        let projection = Projection::default();
        assert!(
            projection.depth(ScenePoint::new(0.0, 0.0, 1.0))
                > projection.depth(ScenePoint::default())
        );
        assert!(
            projection.depth(ScenePoint::new(1.0, 0.0, 0.0))
                > projection.depth(ScenePoint::default())
        );
        assert!(
            projection.depth(ScenePoint::new(0.0, 1.0, 0.0))
                > projection.depth(ScenePoint::default())
        );
    }

    #[test]
    fn projection_is_linear() {
        // Relied on everywhere: a mark projects its root once and then adds
        // projected offsets rather than re-projecting each sample.
        let projection = Projection::default();
        let (a, b) = (
            ScenePoint::new(1.0, 2.0, 0.5),
            ScenePoint::new(-3.0, 0.5, 1.5),
        );
        let sum = projection.project(ScenePoint::new(a.u_m + b.u_m, a.v_m + b.v_m, a.z_m + b.z_m));
        let parts = projection.project(a);
        let other = projection.project(b);
        assert!(close(sum.x_m, parts.x_m + other.x_m));
        assert!(close(sum.y_m, parts.y_m + other.y_m));
    }

    #[test]
    fn the_anisotropy_is_the_two_over_root_three_a_path_tracer_has_to_carry() {
        // The entire difference between the 2:1 dimetric diamond this draws and
        // true isometric. A camera transform cannot express it.
        let aspect = Projection::default().pixel_aspect();
        assert!((aspect - 2.0 / 3.0f64.sqrt()).abs() < 1.0e-9, "{aspect}");
    }

    #[test]
    fn the_view_axes_do_what_they_are_named() {
        let projection = Projection::default();
        let right = projection.view_right();
        let moved = projection.project(ScenePoint::new(right[0], right[1], right[2]));
        assert!(moved.x_m > 0.0 && close(moved.y_m, 0.0), "{moved:?}");

        // Toward the viewer moves nothing on screen and everything in depth.
        let toward = projection.toward_viewer();
        let point = ScenePoint::new(toward[0], toward[1], toward[2]);
        let projected = projection.project(point);
        assert!(
            close(projected.x_m, 0.0) && close(projected.y_m, 0.0),
            "{projected:?}"
        );
        assert!(projection.depth(point) > 0.0);
    }

    #[test]
    fn the_reflection_is_its_own_inverse() {
        // So a round trip across the renderer boundary cannot accumulate a
        // half-swap.
        let projection = Projection::default();
        let point = ScenePoint::new(3.0, -7.0, 0.25);
        assert_eq!(
            projection.to_right_handed(projection.to_right_handed(point)),
            point
        );
        let bearing = 0.7;
        assert!(close(
            projection.bearing_to_right_handed(projection.bearing_to_right_handed(bearing)),
            bearing
        ));
    }

    #[test]
    fn the_reflection_agrees_with_the_projection() {
        // The check that makes the swap correct rather than merely plausible:
        // after reflecting, a physical right-handed basis has to reproduce this
        // projection's screen coordinates exactly.
        let projection = Projection::default();
        // The physical camera's basis, above the ground looking down the
        // isometric axis.
        let right = [-1.0, 1.0, 0.0];
        let right_len = 2.0f64.sqrt();
        let up = [-1.0, -1.0, 2.0];
        let up_len = 6.0f64.sqrt();

        for point in [
            ScenePoint::new(1.0, 0.0, 0.0),
            ScenePoint::new(0.0, 1.0, 0.0),
            ScenePoint::new(0.0, 0.0, 1.0),
            ScenePoint::new(3.0, -7.0, 0.25),
        ] {
            let screen = projection.project(point);
            let reflected = projection.to_right_handed(point);
            let p = [reflected.u_m, reflected.v_m, reflected.z_m];

            let along_right = (p[0] * right[0] + p[1] * right[1] + p[2] * right[2]) / right_len;
            let along_up = (p[0] * up[0] + p[1] * up[1] + p[2] * up[2]) / up_len;

            // The physical basis is normalised and the projection's is not, so
            // the two agree up to the axis lengths the camera carries as
            // `ortho_scale` and `pixel_aspect`.
            assert!(
                close(along_right * right_len / 2.0, screen.x_m / 2.0),
                "{point:?}: right {along_right} against screen.x {}",
                screen.x_m
            );
            assert!(
                close(along_up * up_len / 2.0, screen.y_m),
                "{point:?}: up {along_up} against screen.y {}",
                screen.y_m
            );
        }
    }

    #[test]
    fn a_ground_rectangle_projects_to_a_diamond_larger_than_itself() {
        // Worth asserting, because assuming the screen bounds are the ground
        // bounds is how a bake clips its own corners.
        let projection = Projection::default();
        let ground = WorldRect::new(WorldPoint::new(0.0, 0.0), WorldPoint::new(4.0, 4.0));
        let (low, high) = projection.screen_bounds(ground);
        assert_eq!(low.x_m, -4.0);
        assert_eq!(high.x_m, 4.0);
        assert_eq!(low.y_m, -4.0);
        assert_eq!(high.y_m, 0.0);
        assert!(
            high.x_m - low.x_m > ground.width_m(),
            "the diamond is not wider"
        );
    }
}

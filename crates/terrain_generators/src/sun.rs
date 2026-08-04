//! Where the sun is.
//!
//! Lifted out of `lab.rs`, because a sun is not a property of the laboratory
//! plate. It reaches placement — the mound field shades its own domes
//! analytically and has to know which way the light is — so it sits on the
//! generator's side of the boundary, and having it inside a measurement tool
//! meant the parameter model depended on one.
//!
//! Two angles rather than a vector, and that is the whole design. The two things
//! a look change needs to sweep are exactly these: turn the sun around the
//! compass and watch the lit side follow, drop it toward the horizon and watch
//! the shadows lengthen. A `Vec3` makes both an arithmetic exercise at every
//! call site, and an error-prone one — see [`Key::direction`] for the specific
//! mistake it prevents.

use glam::{Vec2, Vec3};

use crate::iso;

/// How the key light is pointed at the plate.
///
/// Two angles rather than a vector, because the two things a look change needs
/// to sweep are exactly these: turn the sun around the compass and see the lit
/// side follow, drop it toward the horizon and see the shadows lengthen. A
/// `Vec3` makes both of those an arithmetic exercise at every call site.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    /// Compass bearing of the light on the screen plane, radians. Zero points
    /// the light along −X in image space, which is where the field's own key
    /// sits, and it turns anticlockwise from there.
    pub azimuth: f32,
    /// Height of the light above the ground plane, radians.
    pub elevation: f32,
}

impl Key {
    /// Where the light sits on the screen plane, normalised.
    ///
    /// The field's own bearing, turned by [`Key::azimuth`]. Held fixed against
    /// the elevation, because it is the thing the picture is composed around:
    /// it decides which way every shadow falls on screen, which side of every
    /// mound is lit, and which flank of a mark carries its dark under-stroke.
    /// [`crate::field::LIGHT_PLANE`] is the same statement made where the mound
    /// field can read it.
    pub fn plane(self) -> Vec2 {
        let (sin_a, cos_a) = self.azimuth.sin_cos();
        let plane = crate::field::LIGHT_PLANE;
        Vec2::new(
            plane.x * cos_a - plane.y * sin_a,
            plane.x * sin_a + plane.y * cos_a,
        )
    }

    /// The direction toward the light, in the image space
    /// [`crate::style::GrassParams::light`] uses: +X right, +Y **down**, +Z toward
    /// the viewer.
    ///
    /// ## Elevation is a world angle, and image `+Z` is not up
    ///
    /// This looks like it should be `(plane · cos θ, sin θ)` and that is exactly
    /// the mistake it exists to avoid. Image `+Z` points at the **viewer**, and
    /// this camera looks down at 35°, so putting `sin θ` there produces a light
    /// somewhere behind the viewer's shoulder rather than one `θ` above the
    /// horizon. Built that way, a "35° sun" came out at nearly 55° of real
    /// elevation — and the shadow guard band, which is sized from one over the
    /// tangent, came out a third short of what the field actually casts.
    ///
    /// So the sun is placed in the **world** and converted, with the screen
    /// bearing as the constraint rather than the construction.
    ///
    /// Its height is fixed by `θ`, which leaves a circle of candidates. The
    /// bearing pins that to a line — the projections of the candidates onto the
    /// screen plane have to be parallel to [`Key::plane`] — and a line meets a
    /// circle twice. Both roots project onto the same screen *line*; only one
    /// projects along it in the right *direction*, and the other is a light that
    /// is very nearly straight at the camera. Picking by alignment rather than
    /// by sign is what makes this survive the bearing being turned right round.
    ///
    /// Not every elevation is reachable at every bearing: with the screen
    /// bearing pinned, a sun that would have to be both high and down-screen
    /// does not exist. Those cases have no root at all, and fall back to the
    /// highest the bearing allows.
    pub fn direction(self) -> Vec3 {
        let plane = self.plane();
        let up = self.elevation.sin();
        let flat = self.elevation.cos();

        // The candidates are the world vectors of height `up` whose screen
        // projection is parallel to `plane`: `right·plane.y − down·plane.x = 0`,
        // written out and cleared of denominators.
        const ROOT3: f32 = 1.732_050_8;
        let normal = Vec2::new(ROOT3 * plane.y - plane.x, -(ROOT3 * plane.y + plane.x));
        let offset = -2.0 * up * plane.x;
        let length = normal.length();
        if length < 1.0e-6 {
            return Vec3::new(plane.x, plane.y, 0.0).normalize_or(Vec3::Z);
        }

        // Where that line comes closest to the origin, and how far along it the
        // circle of radius `flat` is met.
        let foot = normal * (offset / (length * length));
        let half = (flat * flat - (offset * offset) / (length * length))
            .max(0.0)
            .sqrt();
        let along = Vec2::new(-normal.y, normal.x) / length;

        // Of the two, the one whose screen projection runs *along* the bearing
        // rather than against it.
        let mut best = Vec3::new(plane.x * flat, plane.y * flat, up);
        let mut best_score = f32::NEG_INFINITY;
        for side in [-1.0f32, 1.0] {
            let ground = foot + along * (half * side);
            let candidate = Vec3::new(ground.x, ground.y, up);
            let image = iso::world_to_image(candidate);
            let score = Vec2::new(image.x, image.y).dot(plane);
            if score > best_score {
                best_score = score;
                best = candidate;
            }
        }
        iso::world_to_image(best.normalize_or(Vec3::Z)).normalize_or(Vec3::Z)
    }

    /// The direction toward the light in world space.
    pub fn world(self) -> Vec3 {
        iso::image_to_world(self.direction()).normalize_or(Vec3::Z)
    }
}

impl Default for Key {
    fn default() -> Self {
        Self {
            azimuth: 0.0,
            elevation: DEFAULT_ELEVATION,
        }
    }
}

/// The sun height the renderer is built for, radians.
///
/// Thirty-five degrees. Low enough that a blade casts a shadow one and a half
/// times its own height and the field reads as lit from a direction rather than
/// from above; high enough that the guard band a page needs stays inside one
/// page width, which is what keeps a shadow from being present on one side of a
/// join and missing on the other.
pub const DEFAULT_ELEVATION: f32 = 35.0 * std::f32::consts::PI / 180.0;

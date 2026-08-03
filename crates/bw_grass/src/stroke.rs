//! Blades and leaves: how one mark gets made.
//!
//! A blade is not a sprite that shears. It is a curve through `(X, Y, Z)` in
//! world space that happens to be drawn on a flat screen, and bending it in
//! three dimensions before projecting is what gives, for free, the three things
//! that sell the effect: the silhouette shortens as the blade leans, the tip
//! travels along an ellipse rather than a straight line, and grass laid toward
//! the camera covers more ground than grass laid away from it.
//!
//! ## The stroke language
//!
//! Reproducing the reference is mostly a matter of getting one mark right, then
//! making ten thousand of them. Four properties do the work:
//!
//! - **The centreline is an arc of constant length.** Bend is an angle from
//!   vertical that grows along the blade, integrated rather than interpolated,
//!   so a blade that leans hard genuinely gets shorter on screen.
//! - **The tip is brighter than the root, sharply.** This is the single largest
//!   contributor to the look, larger than the lateral term below. The reference
//!   has bright tips everywhere and bright *blades* almost nowhere.
//! - **One lateral edge catches the light.** A pseudo-cylindrical normal across
//!   the width makes one side face the key and the other face away. Without it
//!   the field reads as flat ribbon; with it, every mark has a round side.
//! - **A darker stroke sits behind, offset away from the light.** This is the
//!   painterly half. It is the thing that separates two overlapping blades of
//!   the same colour, and no amount of runtime lighting substitutes for it.

use bevy::prelude::*;

use crate::iso;
use crate::palette::Tone;
use crate::surface::{SUPERSAMPLE, Surface};

/// How width varies from root to tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Wide at the root, tapering to a point. Grass.
    Tapered,
    /// Narrow at both ends, widest in the middle. A leaflet.
    Oval,
    /// Nearly constant. Stems and the odd reed.
    Stem,
}

impl Profile {
    #[inline]
    fn width_at(self, s: f32) -> f32 {
        match self {
            // 1.2 rather than 1.0: the reference's blades hold their width for
            // the first third and then give it up quickly, which is what makes
            // them read as blades rather than as triangles.
            Profile::Tapered => (1.0 - s).powf(1.2),
            Profile::Oval => (s * (1.0 - s) * 4.0).powf(0.55),
            Profile::Stem => (1.0 - s * 0.55).powf(0.7),
        }
    }
}

/// One mark, described in world space and cache pixels.
#[derive(Clone, Copy, Debug)]
pub struct Stroke {
    /// Where it grows from, world metres.
    pub root: Vec3,
    /// Ground direction it leans toward, world radians.
    pub azimuth: f32,
    /// Arc length, world metres.
    pub length: f32,
    /// Bend away from vertical at the tip, radians. Past `PI/2` the tip falls.
    pub bend: f32,
    /// Extra bend concentrated in the last third — the hook.
    pub curl: f32,
    /// Lateral drift of the lean direction along the blade, radians. Makes S
    /// curves, which the reference has a great many of.
    pub sway: f32,
    /// An abrupt change of bend partway along, radians.
    ///
    /// The difference between a mark and a curve. Every smooth arc in a field of
    /// smooth arcs advertises the function that drew it; an elbow does not,
    /// because there is no continuous parameter that produces one. The reference
    /// is full of strokes that change their mind halfway.
    pub kink: f32,
    /// Where along the blade the kink happens, `0..1`.
    pub kink_at: f32,
    /// Sideways component of the kink, radians.
    pub kink_turn: f32,
    /// Half-width at the root, in cache pixels.
    pub width: f32,
    /// Width the tip never falls below, cache pixels. Keeps thin marks from
    /// disintegrating into dashes under supersampling.
    pub tip_width: f32,
    pub profile: Profile,
    pub tone: Tone,
    /// Light index at the root.
    pub base_light: f32,
    /// A gentle lift toward the tip, spread over the whole blade.
    pub tip_light: f32,
    /// A sharp catch of light in the last fifth, and only on chosen marks.
    ///
    /// Separate from [`Stroke::tip_light`] because they do different jobs and
    /// the reference uses them at different rates. Lighting every blade along
    /// its length gives the field a wet, varnished sheen; the reference reserves
    /// its brightest paint for scattered tips and bends, and leaves most marks
    /// with no highlight at all.
    pub glint: f32,
    /// Strength of the one-sided lateral shading.
    pub side_light: f32,
    /// Width of the dark stroke offset behind this one, cache pixels.
    pub under: f32,
    /// Pushed this far behind its own root in depth.
    ///
    /// A mark drawn at a bias loses to its neighbours wherever they overlap, so
    /// only fragments of it survive. That is how the reference gets strokes that
    /// disappear into the mass and reappear — and it is much closer to how the
    /// paint actually behaves than drawing fewer, fully visible blades.
    pub depth_bias: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Self {
            root: Vec3::ZERO,
            azimuth: 0.0,
            length: 0.22,
            bend: 0.5,
            curl: 0.0,
            sway: 0.0,
            kink: 0.0,
            kink_at: 0.5,
            kink_turn: 0.0,
            width: 1.6,
            tip_width: 0.35,
            profile: Profile::Tapered,
            tone: Tone::Grass,
            base_light: 0.42,
            tip_light: 0.16,
            glint: 0.0,
            side_light: 0.16,
            under: 1.1,
            depth_bias: 0.0,
        }
    }
}

/// Rasterises strokes into one page's [`Surface`].
pub struct Painter<'a> {
    surface: &'a mut Surface,
    /// Cache-pixel position of the page's top-left corner.
    origin: Vec2,
    /// Direction toward the key light, in image space: +X right, +Y *down*,
    /// +Z toward the viewer.
    light: Vec3,
    /// The light's direction on the screen plane, normalised.
    light_plane: Vec2,
}

impl<'a> Painter<'a> {
    pub fn new(surface: &'a mut Surface, origin: Vec2, light: Vec3) -> Self {
        let plane = Vec2::new(light.x, light.y);
        Self {
            surface,
            origin,
            light,
            light_plane: plane.normalize_or_zero(),
        }
    }

    /// World point to supersampled page pixel.
    #[inline]
    pub fn to_page(&self, world: Vec3) -> Vec2 {
        (iso::to_cache(world) - self.origin) * SUPERSAMPLE as f32
    }

    /// Supersampled page pixel back to the ground plane.
    #[inline]
    pub fn to_ground(&self, page: Vec2) -> Vec2 {
        iso::from_cache_ground(page / SUPERSAMPLE as f32 + self.origin)
    }

    pub fn surface(&self) -> &Surface {
        self.surface
    }

    pub fn surface_mut(&mut self) -> &mut Surface {
        self.surface
    }

    #[inline]
    fn plot(&mut self, x: f32, y: f32, depth: f32, light: f32, tone: Tone, top: f32) {
        // `as` truncates toward zero, so a negative coordinate would fold back
        // onto the page's left edge as a bright smear. Reject before casting.
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.surface.width || y >= self.surface.height {
            return;
        }
        let index = self.surface.index(x, y);
        self.surface.write(index, depth, light, tone, top);
    }

    /// Draw one stroke.
    pub fn draw(&mut self, stroke: &Stroke) {
        let scale = SUPERSAMPLE as f32;
        // Sample the centreline finely enough that consecutive ribs overlap:
        // half a supersampled pixel apart leaves no gaps at any angle, and the
        // cost is linear in a quantity that is already small.
        let screen_length = stroke.length * iso::PX_PER_METRE * scale;
        let steps = (screen_length * 2.0).clamp(6.0, 512.0) as usize;
        let inverse = 1.0 / steps as f32;

        // Walk the arc, integrating rather than interpolating, so the blade
        // keeps its length however far it bends.
        let mut position = stroke.root;
        let segment = stroke.length * inverse;
        let mut previous_page = self.to_page(position);

        for step in 0..=steps {
            let s = step as f32 * inverse;
            // Smooth arc, plus an elbow. The smoothstep is narrow on purpose:
            // spread it out and the kink becomes just more curvature.
            let elbow = {
                let t = ((s - stroke.kink_at) / 0.10).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            let angle = stroke.bend * s.powf(1.55) + stroke.curl * s.powi(4) + stroke.kink * elbow;
            let heading = stroke.azimuth + stroke.sway * s * s + stroke.kink_turn * elbow;
            let (sin_heading, cos_heading) = heading.sin_cos();
            let (sin_angle, cos_angle) = angle.sin_cos();

            let page = self.to_page(position);
            let tangent = (page - previous_page).normalize_or(Vec2::NEG_Y);
            previous_page = page;

            let half = (stroke.width * stroke.profile.width_at(s) + stroke.tip_width) * scale;
            let depth = iso::depth(position) - stroke.depth_bias;
            let top = position.z * iso::Z_SCALE * iso::PX_PER_METRE;
            self.rib(stroke, page, tangent, half, s, depth, top);

            // Advance along the arc. Past a right angle `cos` goes negative and
            // the tip starts descending, which is exactly the hook shape the
            // reference is full of.
            position += Vec3::new(
                segment * sin_angle * cos_heading,
                segment * sin_angle * sin_heading,
                segment * cos_angle,
            );
        }
    }

    /// One perpendicular slice across the stroke.
    #[allow(clippy::too_many_arguments)]
    fn rib(
        &mut self,
        stroke: &Stroke,
        centre: Vec2,
        tangent: Vec2,
        half: f32,
        s: f32,
        depth: f32,
        top: f32,
    ) {
        let perpendicular = Vec2::new(-tangent.y, tangent.x);
        // The under-stroke goes on the side facing away from the light, which is
        // what makes it read as the blade's own shadow rather than as an
        // outline.
        let away = if perpendicular.dot(self.light_plane) > 0.0 {
            -1.0
        } else {
            1.0
        };
        let under = stroke.under * SUPERSAMPLE as f32;
        let (low, high) = if away < 0.0 {
            (-half - under, half)
        } else {
            (-half, half + under)
        };

        // Two terms doing different jobs, and — crucially — carried by two
        // different fields so they can be dealt out at different rates. The
        // gentle one lifts the whole blade a little. The sharp one is the glint,
        // and it does not wake up until the last fifth.
        // Eighth power, not fifth. The glint has to be a catch on the last
        // few pixels of a mark, not a pale upper half: spread it further and
        // the marks read as bright ribbons rather than as dark grass with
        // something bright riding on top of it.
        let tip = stroke.tip_light * s.powf(1.4) + stroke.glint * s.powi(8);
        // Roots sit in their own shadow. Without this every blade glows at the
        // base and the canopy loses its floor.
        let root_shade = 0.085 * (1.0 - s).powf(2.5);

        let span = high - low;
        let steps = (span.ceil() as usize).max(1);
        let step = span / steps as f32;

        for i in 0..=steps {
            let offset = low + step * i as f32;
            let point = centre + perpendicular * offset;

            let (light, depth) = if offset < -half || offset > half {
                // Under-stroke: darker by a fixed amount rather than by a
                // fraction, so a bright blade and a dim one cast the same
                // weight of shadow, and pushed a hair behind the body so a
                // neighbouring blade at the same depth still wins the pixel.
                ((stroke.base_light - 0.22).max(0.0), depth - 1.0e-4)
            } else {
                let r = (offset / half.max(1.0e-3)).clamp(-1.0, 1.0);
                // Pseudo-cylindrical: one edge faces the key, the other faces
                // away, and the middle faces the viewer.
                // Low on purpose. A blade lit as a full cylinder reads as a
                // fleshy tube; the reference's marks are nearly flat, with the
                // roundness only just enough to say which edge faces the light.
                const ROUNDNESS: f32 = 0.72;
                let lateral = ROUNDNESS * r;
                let normal = Vec3::new(
                    lateral * perpendicular.x,
                    lateral * perpendicular.y,
                    (1.0 - lateral * lateral).max(0.0).sqrt(),
                );
                let lambert = normal.dot(self.light).max(0.0);
                // Centred on the mean rather than added: a stroke's average
                // brightness is its own business, and the lateral term is only
                // meant to say which *side* of it is lit.
                let side = stroke.side_light * (lambert - 0.62);
                (stroke.base_light + tip + side - root_shade, depth)
            };

            self.plot(point.x, point.y, depth, light, stroke.tone, top);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width: usize, height: usize) -> (Surface, Vec2) {
        (
            Surface::new(width, height),
            Vec2::new(-(width as f32) / 2.0, -(height as f32) * 0.75),
        )
    }

    #[test]
    fn a_stroke_marks_the_page() {
        let (mut surface, origin) = page(64, 64);
        let mut painter = Painter::new(
            &mut surface,
            origin,
            Vec3::new(-0.4, -0.4, 0.82).normalize(),
        );
        painter.draw(&Stroke {
            root: painter.to_ground(Vec2::new(96.0, 140.0)).extend(0.0),
            ..default()
        });
        let (heights, _) = surface.height_maps(64, 64);
        assert!(heights.iter().any(|&h| h > 1.0), "nothing was drawn");
    }

    #[test]
    fn bending_shortens_the_silhouette() {
        // The property that makes this a projected curve rather than a sheared
        // sprite. Two blades of equal length, one upright and one laid over,
        // must not reach the same height on screen.
        let measure = |bend: f32| {
            let (mut surface, origin) = page(64, 64);
            let mut painter = Painter::new(&mut surface, origin, Vec3::Z);
            let root = painter.to_ground(Vec2::new(96.0, 160.0)).extend(0.0);
            painter.draw(&Stroke {
                root,
                bend,
                length: 0.3,
                ..default()
            });
            let (heights, _) = surface.height_maps(64, 64);
            heights.iter().cloned().fold(0.0f32, f32::max)
        };
        // Bend is the angle at the *tip*, and it grows along the blade, so a
        // blade bent 1.2 radians spends most of its length nearer upright than
        // that and loses only a fifth of its height. Two radians is where the
        // arc genuinely lies over.
        let upright = measure(0.0);
        let leaning = measure(2.0);
        assert!(
            upright > leaning * 1.3,
            "upright {upright} vs leaning {leaning}"
        );
    }

    #[test]
    fn the_tip_is_brighter_than_the_root() {
        let (mut surface, origin) = page(48, 64);
        let mut painter = Painter::new(&mut surface, origin, Vec3::new(0.0, 0.0, 1.0));
        let root = painter.to_ground(Vec2::new(72.0, 170.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            bend: 0.0,
            length: 0.35,
            tip_light: 0.3,
            ..default()
        });

        let mut brightest_high = 0.0f32;
        let mut brightest_low = 0.0f32;
        for y in 0..surface.height {
            for x in 0..surface.width {
                let index = surface.index(x, y);
                if surface.top_at(index) <= 0.0 {
                    continue;
                }
                let (light, _) = surface.pixel(index);
                if surface.top_at(index) > 24.0 {
                    brightest_high = brightest_high.max(light);
                } else if surface.top_at(index) < 6.0 {
                    brightest_low = brightest_low.max(light);
                }
            }
        }
        assert!(
            brightest_high > brightest_low + 0.1,
            "{brightest_high} vs {brightest_low}"
        );
    }

    #[test]
    fn a_stroke_off_the_page_does_not_wrap_onto_it() {
        // `as` truncates toward zero, so a negative page coordinate would land
        // on column zero. That reads as a bright vertical smear down the page
        // edge, and it is the sort of thing that only shows up once the field
        // is tiled.
        let (mut surface, origin) = page(32, 32);
        let mut painter = Painter::new(&mut surface, origin, Vec3::Z);
        let root = painter.to_ground(Vec2::new(-40.0, 48.0)).extend(0.0);
        painter.draw(&Stroke { root, ..default() });
        let (heights, _) = surface.height_maps(32, 32);
        for y in 0..32 {
            assert_eq!(heights[y * 32], 0.0, "row {y} picked up a wrapped stroke");
        }
    }

    #[test]
    fn every_profile_is_widest_where_it_should_be() {
        assert!(Profile::Tapered.width_at(0.0) > Profile::Tapered.width_at(0.9));
        assert!(Profile::Oval.width_at(0.5) > Profile::Oval.width_at(0.05));
        assert!(Profile::Oval.width_at(0.5) > Profile::Oval.width_at(0.95));
        assert!(Profile::Stem.width_at(0.9) > Profile::Tapered.width_at(0.9));
    }
}

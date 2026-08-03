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

use std::sync::{LazyLock, OnceLock};

use bevy::prelude::*;

use crate::fastmath;
use crate::iso;
use crate::palette::Tone;
use crate::surface::Surface;

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
    /// The taper, written the obvious way.
    ///
    /// Not on the rasteriser's path — [`Profile::width_from_logs`] is — but kept
    /// as the definition the fast one is checked against, which is the only
    /// thing that makes the fast one reviewable.
    #[cfg(test)]
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

    /// [`Profile::width_at`], reusing logarithms the caller already has.
    ///
    /// Every one of these is a power of `s` or of `1 - s`, and so are three more
    /// terms the same loop needs. Taking the two logarithms once and reusing
    /// them turns six transcendentals per rib into two — see [`fastmath`] for
    /// why that was worth restructuring the signature for.
    #[inline]
    fn width_from_logs(self, s: f32, log_s: f32, log_rest: f32) -> f32 {
        match self {
            Profile::Tapered => fastmath::pow_from_log2(log_rest, 1.2),
            // log2(4·s·(1−s)) — the product becomes a sum, so this one shares
            // both logarithms rather than needing a third.
            Profile::Oval => fastmath::pow_from_log2(log_s + log_rest + 2.0, 0.55),
            // The only base that is neither `s` nor `1 - s`, and the rarest
            // profile in the vocabulary, so it pays for its own logarithm.
            Profile::Stem => fastmath::pow(1.0 - s * 0.55, 0.7),
        }
    }
}

/// Normalised centreline terms shared by every stroke with the same step count.
///
/// A blade's world-space parameters differ, but its `s` positions, powers and
/// width-profile values do not. Computing these inside every blade repeated two
/// logarithms and several exponentials millions of times per page. The table is
/// built with the exact same `f32` operations the loop used, then reused without
/// changing a single raster input.
#[derive(Clone, Copy)]
struct StepSample {
    s: f32,
    s4: f32,
    s8: f32,
    bend: f32,
    tip: f32,
    root_shade: f32,
    widths: [f32; 3],
}

static STEP_TABLES: LazyLock<[OnceLock<Box<[StepSample]>>; 513]> =
    LazyLock::new(|| std::array::from_fn(|_| OnceLock::new()));

fn step_table(steps: usize) -> &'static [StepSample] {
    STEP_TABLES[steps].get_or_init(|| {
        let inverse = 1.0 / steps as f32;
        (0..=steps)
            .map(|step| {
                let s = step as f32 * inverse;
                let log_s = fastmath::log2(s);
                let log_rest = fastmath::log2(1.0 - s);
                StepSample {
                    s,
                    s4: s.powi(4),
                    s8: s.powi(8),
                    bend: fastmath::pow_from_log2(log_s, 1.55),
                    tip: fastmath::pow_from_log2(log_s, 1.4),
                    root_shade: 0.085 * fastmath::pow_from_log2(log_rest, 2.5),
                    widths: [
                        Profile::Tapered.width_from_logs(s, log_s, log_rest),
                        Profile::Oval.width_from_logs(s, log_s, log_rest),
                        Profile::Stem.width_from_logs(s, log_s, log_rest),
                    ],
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    })
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
    /// This page's cache pixels per world metre.
    px_per_metre: f32,
    /// That, as a fraction of the scale the art is authored at.
    ///
    /// Every width on a [`Stroke`] is in *reference* cache pixels — the units
    /// the reference art is drawn in — and is multiplied by this on the way to
    /// the page. Keeping the conversion here rather than at the two dozen places
    /// a stroke is built means a mark is described once and drawn correctly at
    /// any scale.
    detail: f32,
    /// The surface's supersampling factor, cached as a float because every
    /// world-to-page conversion multiplies by it.
    scale: f32,
    /// Page-local fast path around the atomic read in each global lazy table.
    step_tables: [Option<&'static [StepSample]>; 513],
}

impl<'a> Painter<'a> {
    /// A painter for a page baked at the authoring scale.
    pub fn new(surface: &'a mut Surface, origin: Vec2, light: Vec3) -> Self {
        Self::at_scale(surface, origin, light, iso::PX_PER_METRE)
    }

    /// A painter for a page baked at `px_per_metre` cache pixels to the metre.
    pub fn at_scale(
        surface: &'a mut Surface,
        origin: Vec2,
        light: Vec3,
        px_per_metre: f32,
    ) -> Self {
        let plane = Vec2::new(light.x, light.y);
        let scale = surface.supersample() as f32;
        Self {
            surface,
            origin,
            light,
            light_plane: plane.normalize_or_zero(),
            px_per_metre,
            detail: px_per_metre / iso::PX_PER_METRE,
            scale,
            step_tables: [None; 513],
        }
    }

    /// Supersampled pixels per final pixel on this page.
    #[inline]
    pub fn supersample(&self) -> f32 {
        self.scale
    }

    /// World point to supersampled page pixel.
    #[inline]
    pub fn to_page(&self, world: Vec3) -> Vec2 {
        (iso::to_cache_at(world, self.px_per_metre) - self.origin) * self.scale
    }

    /// Supersampled page pixel back to the ground plane.
    #[inline]
    pub fn to_ground(&self, page: Vec2) -> Vec2 {
        iso::from_cache_ground_at(page / self.scale + self.origin, self.px_per_metre)
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

    /// A bound, in cache pixels, on how far this stroke's marks can land from
    /// its own root.
    ///
    /// Conservative and cheap, and the cheapness is the point: it is evaluated
    /// once per stroke to decide whether to rasterise the stroke at all, and
    /// most pages reject about two marks in three this way. The bound has to be
    /// genuinely an upper bound — a stroke wrongly rejected is a mark present on
    /// one side of a page join and missing on the other — so it is derived
    /// rather than estimated, and
    /// [`crate::bake::tests::the_stroke_reach_bound_is_never_beaten`] sweeps the
    /// vocabulary against it.
    ///
    /// An arc of length `L` cannot displace its own tip further than `L` in a
    /// straight line. The projection turns a world displacement `(dx, dy, dz)`
    /// into `((dx - dy), (dx + dy)/2 - dz)` cache-pixel units, and maximising
    /// each of those over the sphere of radius `L` gives `√2 L` across and
    /// `√1.5 L` down. So `1.42` covers both, and the rib's own half-width and
    /// under-stroke are added on top because those measure from the centreline
    /// rather than from the root.
    #[inline]
    pub fn reach(&self, stroke: &Stroke) -> f32 {
        const SPREAD: f32 = 1.4143;
        stroke.length.abs() * self.px_per_metre * SPREAD
            + (stroke.width.abs() + stroke.tip_width.abs() + stroke.under.abs()) * self.detail
            + 1.0
    }

    /// Draw one stroke.
    pub fn draw(&mut self, stroke: &Stroke) {
        let scale = self.scale;
        // Sample the centreline finely enough that consecutive ribs overlap:
        // half a supersampled pixel apart leaves no gaps at any angle, and the
        // cost is linear in a quantity that is already small.
        //
        // And it is linear in the *page's* scale, which is the whole of why a
        // distant page is cheap: the same blade of grass, drawn onto a page
        // baked at a quarter scale, is a quarter as long in pixels and so walks
        // a quarter of the ribs — each of which is a quarter as wide.
        let screen_length = stroke.length * self.px_per_metre * scale;
        let steps = (screen_length * 2.0).clamp(6.0, 512.0) as usize;
        let inverse = 1.0 / steps as f32;
        let samples = if let Some(samples) = self.step_tables[steps] {
            samples
        } else {
            let samples = step_table(steps);
            self.step_tables[steps] = Some(samples);
            samples
        };

        // Walk the arc, integrating rather than interpolating, so the blade
        // keeps its length however far it bends.
        let mut position = stroke.root;
        let segment = stroke.length * inverse;
        let mut previous_page = self.to_page(position);

        // Hoisted out of the rib loop, all of it constant per stroke. The three
        // widths are authored in reference pixels and land on the page in this
        // page's own.
        let pen = scale * self.detail;
        let width = stroke.width * pen;
        let tip_width = stroke.tip_width * pen;
        let under = stroke.under * pen;
        let under_light = (stroke.base_light - 0.22).max(0.0);
        // The heading only turns if this mark has an S in it or an elbow that
        // twists; most of the vocabulary has neither, and a straight lean lets
        // its sine and cosine be taken once for the whole blade rather than
        // once per rib.
        let turns = stroke.sway != 0.0 || stroke.kink_turn != 0.0;
        let kinks = stroke.kink != 0.0 || stroke.kink_turn != 0.0;
        let profile = stroke.profile as usize;
        let fixed_heading = fastmath::sin_cos(stroke.azimuth);

        for sample in samples {
            let s = sample.s;
            // Smooth arc, plus an elbow. The smoothstep is narrow on purpose:
            // spread it out and the kink becomes just more curvature.
            let elbow = if kinks {
                let t = ((s - stroke.kink_at) / 0.10).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            } else {
                0.0
            };
            let angle = stroke.bend * sample.bend + stroke.curl * sample.s4 + stroke.kink * elbow;
            let (sin_heading, cos_heading) = if turns {
                fastmath::sin_cos(stroke.azimuth + stroke.sway * s * s + stroke.kink_turn * elbow)
            } else {
                fixed_heading
            };
            let (sin_angle, cos_angle) = fastmath::sin_cos(angle);

            let page = self.to_page(position);
            let tangent = (page - previous_page).normalize_or(Vec2::NEG_Y);
            previous_page = page;

            let half = width * sample.widths[profile] + tip_width;
            let depth = iso::depth(position) - stroke.depth_bias;
            // In *reference* pixels at every page scale, on purpose. Every
            // shading term downstream keys on how tall the canopy stands, and
            // grass of a given world height has to mean the same thing to those
            // terms whether the page holding it was baked at full detail or a
            // quarter of it.
            let top = position.z * iso::Z_SCALE * iso::PX_PER_METRE;

            // Two terms doing different jobs, and — crucially — carried by two
            // different fields so they can be dealt out at different rates. The
            // gentle one lifts the whole blade a little. The sharp one is the
            // glint, and it does not wake up until the last fifth.
            // Eighth power, not fifth. The glint has to be a catch on the last
            // few pixels of a mark, not a pale upper half: spread it further and
            // the marks read as bright ribbons rather than as dark grass with
            // something bright riding on top of it.
            let tip = stroke.tip_light * sample.tip + stroke.glint * sample.s8;
            // Roots sit in their own shadow. Without this every blade glows at
            // the base and the canopy loses its floor.
            let root_shade = sample.root_shade;

            self.rib(
                stroke,
                Rib {
                    centre: page,
                    tangent,
                    half,
                    under,
                    depth,
                    top,
                    body_light: stroke.base_light + tip - root_shade,
                    under_light,
                },
            );

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
    fn rib(&mut self, stroke: &Stroke, rib: Rib) {
        let Rib {
            centre,
            tangent,
            half,
            under,
            depth,
            top,
            body_light,
            under_light,
        } = rib;
        let perpendicular = Vec2::new(-tangent.y, tangent.x);
        // The under-stroke goes on the side facing away from the light, which is
        // what makes it read as the blade's own shadow rather than as an
        // outline.
        let away = perpendicular.dot(self.light_plane) > 0.0;
        let (low, high) = if away {
            (-half - under, half)
        } else {
            (-half, half + under)
        };

        let span = high - low;
        let steps = (span.ceil() as usize).max(1);
        let step = span / steps as f32;

        // Whole ribs fall outside the page — the guard band exists so that
        // strokes rooted off the edge still lean in, and the parts of them that
        // do not lean in are most of their length. One rectangle test here
        // replaces a bounds check on every pixel of the rib.
        let extent = perpendicular.abs() * high.abs().max(low.abs());
        if centre.x + extent.x < 0.0
            || centre.y + extent.y < 0.0
            || centre.x - extent.x >= self.surface.width as f32
            || centre.y - extent.y >= self.surface.height as f32
        {
            return;
        }

        // The lateral shading, without building a vector per pixel. The normal
        // leans along `perpendicular` by the lateral term and stands up by
        // whatever is left of a unit vector, so its dot with the key light is
        // two constants and one square root.
        let plane_dot = perpendicular.x * self.light.x + perpendicular.y * self.light.y;
        let up_dot = self.light.z;
        let inverse_half = 1.0 / half.max(1.0e-3);
        // Pushed a hair behind the body so a neighbouring blade at the same
        // depth still wins the pixel.
        let under_depth = depth - 1.0e-4;

        for i in 0..=steps {
            let offset = low + step * i as f32;
            let point = centre + perpendicular * offset;

            let (light, depth) = if offset < -half || offset > half {
                // Under-stroke: darker by a fixed amount rather than by a
                // fraction, so a bright blade and a dim one cast the same
                // weight of shadow.
                (under_light, under_depth)
            } else {
                let r = (offset * inverse_half).clamp(-1.0, 1.0);
                // Pseudo-cylindrical: one edge faces the key, the other faces
                // away, and the middle faces the viewer.
                // Low on purpose. A blade lit as a full cylinder reads as a
                // fleshy tube; the reference's marks are nearly flat, with the
                // roundness only just enough to say which edge faces the light.
                const ROUNDNESS: f32 = 0.72;
                let lateral = ROUNDNESS * r;
                let lambert = (lateral * plane_dot
                    + (1.0 - lateral * lateral).max(0.0).sqrt() * up_dot)
                    .max(0.0);
                // Centred on the mean rather than added: a stroke's average
                // brightness is its own business, and the lateral term is only
                // meant to say which *side* of it is lit.
                (body_light + stroke.side_light * (lambert - 0.62), depth)
            };

            self.plot(point.x, point.y, depth, light, stroke.tone, top);
        }
    }
}

/// One slice across a stroke, with everything the slice needs already worked
/// out.
///
/// A struct rather than nine arguments, and the terms in it are the ones that
/// used to be recomputed per rib and are now computed per stroke or hoisted out
/// of the pixel loop.
struct Rib {
    centre: Vec2,
    tangent: Vec2,
    half: f32,
    under: f32,
    depth: f32,
    top: f32,
    /// `base_light + tip - root_shade`: everything but the lateral term.
    body_light: f32,
    under_light: f32,
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
    fn cached_step_terms_are_bit_exact() {
        for steps in [6usize, 17, 64, 127, 256, 512] {
            let inverse = 1.0 / steps as f32;
            for (step, sample) in step_table(steps).iter().enumerate() {
                let s = step as f32 * inverse;
                let log_s = fastmath::log2(s);
                let log_rest = fastmath::log2(1.0 - s);
                assert_eq!(sample.s.to_bits(), s.to_bits());
                assert_eq!(sample.s4.to_bits(), s.powi(4).to_bits());
                assert_eq!(sample.s8.to_bits(), s.powi(8).to_bits());
                assert_eq!(
                    sample.bend.to_bits(),
                    fastmath::pow_from_log2(log_s, 1.55).to_bits()
                );
                assert_eq!(
                    sample.tip.to_bits(),
                    fastmath::pow_from_log2(log_s, 1.4).to_bits()
                );
                assert_eq!(
                    sample.root_shade.to_bits(),
                    (0.085 * fastmath::pow_from_log2(log_rest, 2.5)).to_bits()
                );
                for (profile, cached) in [Profile::Tapered, Profile::Oval, Profile::Stem]
                    .into_iter()
                    .zip(sample.widths)
                {
                    assert_eq!(
                        cached.to_bits(),
                        profile.width_from_logs(s, log_s, log_rest).to_bits()
                    );
                }
            }
        }
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
        let heights = surface.height_map(64, 64);
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
            let heights = surface.height_map(64, 64);
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
        let heights = surface.height_map(32, 32);
        for y in 0..32 {
            assert_eq!(heights[y * 32], 0.0, "row {y} picked up a wrapped stroke");
        }
    }

    #[test]
    fn the_fast_taper_is_the_taper() {
        // The rasteriser stopped calling `width_at` and started calling a
        // version that shares its logarithms with five other terms. If the two
        // ever disagree, every blade in the field changes shape and no test
        // that looks at one blade would notice.
        for profile in [Profile::Tapered, Profile::Oval, Profile::Stem] {
            let mut worst = 0.0f32;
            for step in 0..=2000 {
                let s = step as f32 / 2000.0;
                let (log_s, log_rest) = (fastmath::log2(s), fastmath::log2(1.0 - s));
                let fast = profile.width_from_logs(s, log_s, log_rest);
                worst = worst.max((fast - profile.width_at(s)).abs());
            }
            assert!(worst < 1.0e-6, "{profile:?} drifts by {worst}");
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

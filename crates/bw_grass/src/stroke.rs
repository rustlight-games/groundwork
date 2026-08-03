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
//! making ten thousand of them. Five properties do the work:
//!
//! - **The centreline is an arc of constant length.** Bend is an angle from
//!   vertical that grows along the blade, integrated rather than interpolated,
//!   so a blade that leans hard genuinely gets shorter on screen.
//! - **The tip is brighter than the root, sharply.** The reference has bright
//!   tips everywhere and bright *blades* almost nowhere.
//! - **The blade has a real cross-section.** A shallow trough with a raised
//!   midrib, carried on a world-space frame that twists along the blade. This is
//!   what replaced the old pseudo-cylindrical fudge, and the difference is not
//!   subtlety — the fudge was computed in screen space, so moving the sun
//!   changed nothing at all.
//! - **The width profile is a leaf, not a needle.** Narrow where it attaches,
//!   broadest a third of the way up, then a long taper to a quick point.
//! - **A darker stroke sits behind, offset away from the light.** The painterly
//!   half: it separates two overlapping blades of the same colour. Its job
//!   narrows once real shadows exist, but it does not go away.
//!
//! ## Where the shading happens
//!
//! At the rib, against a world-space sun, using the world-space normal of the
//! cross-section. [`crate::iso::image_to_world`] is the bridge, and it is worth
//! reading its warning: the key light is authored in image coordinates where
//! `+Z` points at the viewer rather than at the sky.

use std::sync::{LazyLock, OnceLock};

use bevy::prelude::*;

use crate::fastmath;
use crate::geometry::{self, Frame, TipProfile};
use crate::iso;
use crate::palette::Tone;
use crate::surface::Surface;

pub use crate::geometry::Profile;

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
    s8: f32,
    bend: f32,
    tip: f32,
    root_shade: f32,
    /// How far the twist has run by here, as a fraction of the total.
    twist: f32,
    widths: [f32; 4],
}

/// How the twist is distributed along a blade.
///
/// Above one, so the surface turns slowly near the sheath and quickly toward the
/// tip. A linear twist reads as a machined helix; grass is nearly flat where it
/// leaves the ground and does most of its turning in the free upper half.
const TWIST_CURVE: f32 = 1.45;

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
                    s8: s.powi(8),
                    bend: fastmath::pow_from_log2(log_s, 1.55),
                    tip: fastmath::pow_from_log2(log_s, 1.4),
                    root_shade: 0.085 * fastmath::pow_from_log2(log_rest, 2.5),
                    twist: fastmath::pow_from_log2(log_s, TWIST_CURVE),
                    widths: [
                        Profile::Tapered.width_from_logs(s, log_s, log_rest),
                        Profile::Oval.width_from_logs(s, log_s, log_rest),
                        Profile::Stem.width_from_logs(s, log_s, log_rest),
                        Profile::Leaf.width_from_logs(s, log_s, log_rest),
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
    /// How far the blade's face rotates about its own axis, root to tip,
    /// radians.
    ///
    /// The cheapest thing in the whole vocabulary and close to the most
    /// valuable. Without it every blade in a tuft presents the same face to the
    /// sun, every highlight lands in the same place, and the tuft reads as a
    /// comb however varied its shapes are. With it, the lit edge appears and
    /// disappears along each blade on its own schedule.
    pub twist: f32,
    /// How far the centre of the blade stands proud of its edges, as a fraction
    /// of the half-width. See [`geometry::RIDGE`].
    pub ridge: f32,
    /// What happens at the end of it.
    pub tip: TipProfile,
    /// How old and how established this mark is, `0..1`.
    ///
    /// Not a shape parameter — every shape decision has already been made by the
    /// time this is set. It rides along so the *material* can know: a mature
    /// blade is broader, a little drier at the tip and a little less saturated
    /// through the body, and a new shoot is the opposite. Carrying it on the
    /// mark rather than re-deriving it at shading time is what lets the tiller
    /// grade its own leaves by age.
    pub maturity: f32,
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
    /// Strength of the form shading — how much the blade's own facing moves its
    /// light index.
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

impl Stroke {
    /// A bound, in cache pixels, on how far this mark's paint can land from its
    /// own root when drawn at `px_per_metre`.
    ///
    /// Conservative and cheap, and the cheapness is the point: it is evaluated
    /// once per mark to decide whether to rasterise it at all, and most pages
    /// reject about two marks in three this way. The bound has to be genuinely
    /// an upper bound — a mark wrongly rejected is present on one side of a page
    /// join and missing on the other — so it is derived rather than estimated,
    /// and [`crate::bake::tests::the_stroke_reach_bound_is_never_beaten`] sweeps
    /// the vocabulary against it.
    ///
    /// An arc of length `L` cannot displace its own tip further than `L` in a
    /// straight line. The projection turns a world displacement `(dx, dy, dz)`
    /// into `((dx - dy), (dx + dy)/2 - dz)` cache-pixel units, and maximising
    /// each of those over the sphere of radius `L` gives `√2 L` across and
    /// `√1.5 L` down. So `1.42` covers both, and the rib's own half-width and
    /// under-stroke are added on top because those measure from the centreline
    /// rather than from the root.
    ///
    /// A forked tip continues past the parent rather than replacing what is
    /// there, so it genuinely lengthens the mark and
    /// [`TipProfile::extra_reach`] is added to the arc before scaling.
    ///
    /// On the mark rather than on the painter, because placement has to ask it
    /// before there is a painter — a scene is built and only then drawn.
    #[inline]
    pub fn reach(&self, px_per_metre: f32) -> f32 {
        const SPREAD: f32 = 1.4143;
        let detail = px_per_metre / iso::PX_PER_METRE;
        let arc = self.length.abs() * (1.0 + self.tip.extra_reach());
        arc * px_per_metre * SPREAD
            + (self.width.abs() + self.tip_width.abs() + self.under.abs()) * detail
            + 1.0
    }
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
            profile: Profile::Leaf,
            twist: 0.0,
            ridge: geometry::RIDGE,
            tip: TipProfile::Pointed,
            maturity: 0.5,
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

/// Where the arc integration has got to.
///
/// A blade with a forked tip is walked as a parent and then two children that
/// *continue* from where the parent stopped, so the walk has to be able to hand
/// its state over. Keeping it in one value is also what stopped the tangent
/// being derived from the previous projected point, which was a subtle
/// dependency on rasterisation order in something that is purely geometry.
#[derive(Clone, Copy)]
struct Cursor {
    position: Vec3,
    /// Angle from vertical, radians.
    angle: f32,
    /// World heading, radians.
    heading: f32,
    /// Twist accumulated so far, radians.
    twist: f32,
}

/// Rasterises strokes into one page's [`Surface`].
pub struct Painter<'a> {
    surface: &'a mut Surface,
    /// Cache-pixel position of the page's top-left corner.
    origin: Vec2,
    /// Direction toward the key light in **world** space, which is the only
    /// space a surface normal exists in. Converted once, at construction, from
    /// the image-space vector the rest of the renderer authors it as.
    sun: Vec3,
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
    /// Ribs per supersampled pixel of blade length.
    ribs_per_pixel: f32,
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
            sun: iso::image_to_world(light).normalize_or(Vec3::Z),
            light_plane: plane.normalize_or_zero(),
            px_per_metre,
            detail: px_per_metre / iso::PX_PER_METRE,
            scale,
            ribs_per_pixel: 2.0,
            step_tables: [None; 513],
        }
    }

    /// How finely centrelines are walked, in ribs per supersampled pixel.
    pub fn with_ribs_per_pixel(mut self, ribs: f32) -> Self {
        self.ribs_per_pixel = ribs.max(0.5);
        self
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

    /// A world *direction* as a page-pixel direction.
    ///
    /// The projection is linear, so a direction maps without the origin term.
    /// Written out rather than differencing two projected points because the
    /// difference of two nearly equal projections is where precision goes to
    /// die, and the rib's width axis is exactly that case.
    #[inline]
    fn page_direction(&self, world: Vec3) -> Vec2 {
        let unit = self.px_per_metre * self.scale;
        Vec2::new(
            (world.x - world.y) * unit,
            ((world.x + world.y) * 0.5 - world.z) * unit,
        )
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
    /// its own root. See [`Stroke::reach`].
    #[inline]
    pub fn reach(&self, stroke: &Stroke) -> f32 {
        stroke.reach(self.px_per_metre)
    }

    /// Draw one stroke, tip and all.
    pub fn draw(&mut self, stroke: &Stroke) {
        // How long the mark is on this page decides two things: how finely the
        // centreline is walked, and whether a fork is drawable at all.
        //
        // And it is linear in the *page's* scale, which is the whole of why a
        // distant page is cheap: the same blade of grass, drawn onto a page
        // baked at a quarter scale, is a quarter as long in pixels and so walks
        // a quarter of the ribs — each of which is a quarter as wide.
        let screen_length = stroke.length * self.px_per_metre * self.scale;

        match stroke.tip.resolved_at(self.child_pixels(stroke)) {
            TipProfile::Forked {
                split_at,
                opening,
                long,
                short,
            } => {
                // The parent stops at the split and hands its state over. The
                // children continue from exactly there, which is what makes a
                // fork read as one blade separating rather than as two small
                // blades glued onto the end of a big one.
                // The parent keeps the *whole* blade's width profile even
                // though it stops early, so it arrives at the split at its
                // natural width rather than tapering to a point there.
                let cursor = self.body(stroke, 0.0, split_at, screen_length, (0.0, 1.0), None);
                // Asymmetric, and deliberately so: the long child keeps most of
                // the parent's heading and the short one does the turning. Two
                // children mirrored about the parent is a tuning fork, and the
                // eye finds the symmetry immediately.
                let half = opening * 0.5;
                self.child(stroke, cursor, split_at, -half * 1.3, long, screen_length);
                self.child(stroke, cursor, split_at, half * 0.7, short, screen_length);
            }
            TipProfile::Notched { depth } => {
                // A notch is a blade that stops a little short and blunt. The
                // width floor is what makes it read as torn rather than as
                // tapered, and it is the shape a fork averages to once it is too
                // small to draw.
                let end = (1.0 - depth).clamp(0.2, 1.0);
                self.body(stroke, 0.0, end, screen_length, (0.0, 1.0), None);
            }
            TipProfile::Pointed => {
                self.body(stroke, 0.0, 1.0, screen_length, (0.0, 1.0), None);
            }
        }
    }

    /// How many final page pixels one fork child would span.
    #[inline]
    fn child_pixels(&self, stroke: &Stroke) -> f32 {
        match stroke.tip {
            TipProfile::Forked { long, short, .. } => {
                long.min(short) * stroke.length * self.px_per_metre
            }
            _ => f32::INFINITY,
        }
    }

    /// One child of a forked tip.
    fn child(
        &mut self,
        stroke: &Stroke,
        from: Cursor,
        split_at: f32,
        turn: f32,
        length: f32,
        screen_length: f32,
    ) {
        // A child has to *continue* the parent's width, not restart from the
        // parent's root width — otherwise the blade visibly swells at the split,
        // which is the one thing a fork must never do. So the parent's own taper
        // is evaluated where the split happens and the children divide what is
        // left of it.
        //
        // `FORK_SHARE` is above a half on purpose: two children of exactly half
        // the width read as a blade that has been sliced, and a real split leaf
        // has a little more material than that because the two halves curl apart
        // rather than lying flat.
        const FORK_SHARE: f32 = 0.62;
        let at_split = stroke.profile.width_at(split_at).max(0.05);
        let child = Stroke {
            width: stroke.width * at_split * FORK_SHARE,
            tip_width: stroke.tip_width * 0.55,
            // The children carry the tip lift and the glint, because the whole
            // reason to draw a fork is that the reference's split ends are its
            // brightest, thinnest paint.
            tip_light: stroke.tip_light * 1.15,
            tip: TipProfile::Pointed,
            ..*stroke
        };
        let mut start = from;
        start.heading += turn;
        // Re-parameterised onto its own `0..1`, so a child tapers to its own
        // point rather than inheriting whichever fraction of the parent's taper
        // it happens to occupy.
        self.body(
            &child,
            split_at,
            split_at + length,
            screen_length,
            (split_at, split_at + length),
            Some(start),
        );
    }

    /// Walk part of a centreline and rasterise it.
    ///
    /// `from`/`to` are the arc parameters this segment walks. `profile` is the
    /// arc range the *width profile* is stretched across, which is a different
    /// question and has to be asked separately: a parent that stops at the split
    /// still wants the whole blade's taper, so it arrives there at its natural
    /// width; a child wants its own `0..1` so it tapers to its own point.
    ///
    /// Returns where the walk ended, so a fork's children can continue from it.
    fn body(
        &mut self,
        stroke: &Stroke,
        from: f32,
        to: f32,
        screen_length: f32,
        profile_range: (f32, f32),
        start: Option<Cursor>,
    ) -> Cursor {
        let span = (to - from).max(0.0);
        // Sample the centreline finely enough that consecutive ribs overlap:
        // half a supersampled pixel apart leaves no gaps at any angle, and the
        // cost is linear in a quantity that is already small.
        let steps = (screen_length * span * self.ribs_per_pixel).clamp(4.0, 512.0) as usize;
        let inverse = 1.0 / steps as f32;
        let samples = if let Some(samples) = self.step_tables[steps] {
            samples
        } else {
            let samples = step_table(steps);
            self.step_tables[steps] = Some(samples);
            samples
        };

        // Hoisted out of the rib loop, all of it constant per stroke. The three
        // widths are authored in reference pixels and land on the page in this
        // page's own.
        let pen = self.scale * self.detail;
        let width = stroke.width * pen;
        let tip_width = stroke.tip_width * pen;
        let under = stroke.under * pen;
        let under_light = (stroke.base_light - 0.22).max(0.0);
        let kinks = stroke.kink != 0.0 || stroke.kink_turn != 0.0;
        let profile_index = stroke.profile as usize;
        let (profile_from, profile_to) = profile_range;
        let profile_span = (profile_to - profile_from).max(1.0e-4);

        // Most marks are walked whole, and for those the step table's cached
        // powers are exactly right — the segment parameter *is* the arc
        // parameter. A fork's children walk a slice of it and have to pay for
        // their own, which is affordable because they are a minority of a
        // minority.
        let whole = from == 0.0 && span == 1.0;

        let mut cursor = start.unwrap_or(Cursor {
            position: stroke.root,
            angle: 0.0,
            heading: stroke.azimuth,
            twist: 0.0,
        });
        let segment = stroke.length * span * inverse;

        for sample in samples {
            // Where this rib sits on the parent's arc, and where it sits on the
            // width profile — two different parameters once a fork is involved.
            let s = from + sample.s * span;
            let profile_s = ((s - profile_from) / profile_span).clamp(0.0, 1.0);
            // Smooth arc, plus an elbow. The smoothstep is narrow on purpose:
            // spread it out and the kink becomes just more curvature.
            let elbow = if kinks {
                let t = ((s - stroke.kink_at) / 0.10).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            } else {
                0.0
            };
            let (bend_power, twist_power) = if whole {
                (sample.bend, sample.twist)
            } else {
                let log_s = fastmath::log2(s.max(0.0));
                (
                    fastmath::pow_from_log2(log_s, 1.55),
                    fastmath::pow_from_log2(log_s, TWIST_CURVE),
                )
            };
            let s2 = s * s;
            cursor.angle = stroke.bend * bend_power + stroke.curl * s2 * s2 + stroke.kink * elbow;
            cursor.heading = stroke.azimuth + stroke.sway * s2 + stroke.kink_turn * elbow;
            if let Some(started) = start {
                // A child inherits the turn its parent handed it on top of
                // whatever the shared centreline says.
                cursor.heading += started.heading - stroke.azimuth - stroke.sway * from * from;
            }
            cursor.twist = stroke.twist * twist_power;

            let (sin_heading, cos_heading) = fastmath::sin_cos(cursor.heading);
            let (sin_angle, cos_angle) = fastmath::sin_cos(cursor.angle);
            let frame = Frame::build(sin_heading, cos_heading, sin_angle, cos_angle, cursor.twist);

            // The rib runs along the blade's own width axis, projected. That is
            // not quite perpendicular to the projected centreline once the blade
            // twists, and the difference is the point: a twisted blade presents
            // a skewed cross-section, which is exactly what the eye reads as a
            // surface turning.
            let across = self
                .page_direction(frame.binormal)
                .normalize_or(Vec2::new(1.0, 0.0));

            let index = ((profile_s * (samples.len() - 1) as f32) as usize).min(samples.len() - 1);
            let shape = samples[index];
            let half = (width * shape.widths[profile_index] + tip_width)
                * geometry::foreshorten(frame.normal);
            let depth = iso::depth(cursor.position) - stroke.depth_bias;
            // In *reference* pixels at every page scale, on purpose. Every
            // shading term downstream keys on how tall the canopy stands, and
            // grass of a given world height has to mean the same thing to those
            // terms whether the page holding it was baked at full detail or a
            // quarter of it.
            let top = cursor.position.z * iso::Z_SCALE * iso::PX_PER_METRE;

            // Two terms doing different jobs, and — crucially — carried by two
            // different fields so they can be dealt out at different rates. The
            // gentle one lifts the whole blade a little. The sharp one is the
            // glint, and it does not wake up until the last fifth.
            let tip = stroke.tip_light * shape.tip + stroke.glint * shape.s8;
            // Roots sit in their own shadow. Without this every blade glows at
            // the base and the canopy loses its floor.
            let root_shade = shape.root_shade;

            self.rib(
                stroke,
                &frame,
                Rib {
                    centre: self.to_page(cursor.position),
                    across,
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
            cursor.position += frame.tangent * segment;
        }
        cursor
    }

    /// One slice across the stroke.
    fn rib(&mut self, stroke: &Stroke, frame: &Frame, rib: Rib) {
        let Rib {
            centre,
            across,
            half,
            under,
            depth,
            top,
            body_light,
            under_light,
        } = rib;
        // The under-stroke goes on the side facing away from the light, which is
        // what makes it read as the blade's own shadow rather than as an
        // outline.
        let away = across.dot(self.light_plane) > 0.0;
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
        let extent = across.abs() * high.abs().max(low.abs());
        if centre.x + extent.x < 0.0
            || centre.y + extent.y < 0.0
            || centre.x - extent.x >= self.surface.width as f32
            || centre.y - extent.y >= self.surface.height as f32
        {
            return;
        }

        let inverse_half = 1.0 / half.max(1.0e-3);
        // Pushed a hair behind the body so a neighbouring blade at the same
        // depth still wins the pixel.
        let under_depth = depth - 1.0e-4;

        for i in 0..=steps {
            let offset = low + step * i as f32;
            let point = centre + across * offset;

            let (light, depth) = if offset < -half || offset > half {
                // Under-stroke: darker by a fixed amount rather than by a
                // fraction, so a bright blade and a dim one cast the same
                // weight of shadow.
                (under_light, under_depth)
            } else {
                let u = (offset * inverse_half).clamp(-1.0, 1.0);
                // The real thing, at last: the world-space normal of the raised
                // cross-section, dotted against the world-space sun. This is
                // what the old pseudo-cylindrical term was imitating, and the
                // difference is that this one knows which way the blade points.
                let normal = frame.across(u, stroke.ridge);
                let lambert = form_light(normal, self.sun);
                // Centred on the mean rather than added: a stroke's average
                // brightness is its own business, and the form term is only
                // meant to say which *side* of it is lit.
                (
                    body_light + stroke.side_light * (lambert - FORM_MEAN),
                    depth,
                )
            };

            self.plot(point.x, point.y, depth, light, stroke.tone, top);
        }
    }
}

/// How much light a thin two-sided surface shows, `0..1`.
///
/// Wrapped rather than clamped, and the wrap is not a cheat. A grass blade is a
/// few cells thick, so the face turned away from the sun is not black — it is
/// lit by what came through the blade plus what bounced off the canopy below.
/// A hard `max(N·L, 0)` gives every blade a terminator and a dead back, which is
/// what makes procedural vegetation read as moulded plastic.
///
/// The absolute value is the two-sidedness: which face of a leaf you happen to
/// be looking at should not decide whether it is lit, because the leaf is thin
/// enough that both faces are.
#[inline]
fn form_light(normal: Vec3, sun: Vec3) -> f32 {
    const WRAP: f32 = 0.45;
    let facing = normal.dot(sun).abs();
    ((facing + WRAP) / (1.0 + WRAP)).clamp(0.0, 1.0)
}

/// What [`form_light`] averages to over a sphere of normals.
///
/// Subtracted so the form term redistributes light rather than adding any. A
/// blade whose average brightness moved when its shading model changed would
/// need every other constant in the baker retuned around it.
const FORM_MEAN: f32 = 0.655;

/// One slice across a stroke, with everything the slice needs already worked
/// out.
struct Rib {
    centre: Vec2,
    /// Unit page direction the rib runs along — the blade's own width axis,
    /// projected.
    across: Vec2,
    half: f32,
    under: f32,
    depth: f32,
    top: f32,
    /// `base_light + tip - root_shade`: everything but the form term.
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
                assert_eq!(sample.s8.to_bits(), s.powi(8).to_bits());
                assert_eq!(
                    sample.bend.to_bits(),
                    fastmath::pow_from_log2(log_s, 1.55).to_bits()
                );
                assert_eq!(
                    sample.twist.to_bits(),
                    fastmath::pow_from_log2(log_s, TWIST_CURVE).to_bits()
                );
                for (profile, cached) in Profile::ALL.into_iter().zip(sample.widths) {
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

    /// The brightest light index anywhere a blade painted, and how much of the
    /// page it covered.
    fn measure(light: Vec3, twist: f32) -> (f32, f32, f32) {
        let (mut surface, origin) = page(64, 64);
        let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
        let root = painter.to_ground(Vec2::new(96.0, 150.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            azimuth: 0.4,
            bend: 0.35,
            length: 0.3,
            width: 3.0,
            twist,
            side_light: 0.5,
            under: 0.0,
            ..default()
        });
        let (mut low, mut high, mut count) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f32);
        for index in 0..surface.width * surface.height {
            if surface.top_at(index) <= 0.0 {
                continue;
            }
            let (value, _) = surface.pixel(index);
            low = low.min(value);
            high = high.max(value);
            count += 1.0;
        }
        (low, high, count)
    }

    #[test]
    fn turning_the_sun_moves_the_light_on_a_blade() {
        // The gate for the whole phase. The old lateral term was computed in
        // screen space against a screen-space light, so it produced a lit edge
        // that never moved when the key did — four bearings ninety degrees apart
        // gave four identical plates.
        let elevation: f32 = 0.9;
        let bearing = |degrees: f32| {
            let a = degrees.to_radians();
            Vec3::new(
                a.cos() * elevation.cos(),
                a.sin() * elevation.cos(),
                elevation.sin(),
            )
        };
        let a = measure(bearing(0.0), 0.0);
        let b = measure(bearing(90.0), 0.0);
        let c = measure(bearing(180.0), 0.0);
        // Not just "something changed" — the *span* of light across the blade
        // has to differ, which is what says the shading is reading a direction
        // rather than a magnitude.
        let span = |m: (f32, f32, f32)| m.1 - m.0;
        assert!(
            (span(a) - span(c)).abs() > 0.02 || (a.1 - c.1).abs() > 0.02,
            "the sun turned 180° and the blade did not notice: {a:?} vs {c:?}"
        );
        assert!(
            (a.1 - b.1).abs() > 0.01 || (span(a) - span(b)).abs() > 0.01,
            "the sun turned 90° and the blade did not notice: {a:?} vs {b:?}"
        );
    }

    #[test]
    fn a_blade_is_lit_across_its_width() {
        // One edge toward the key, one away. Without this the mark is a flat
        // ribbon whatever else is done to it.
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let (low, high, _) = measure(light, 0.0);
        assert!(
            high - low > 0.05,
            "the blade shades flat across its width: {low} to {high}"
        );
    }

    #[test]
    fn twisting_a_blade_narrows_it() {
        // Foreshortening, which is what makes the twist legible in the
        // silhouette rather than only in the shading.
        //
        // Measured at a half turn rather than a quarter, and the reason is
        // worth recording because it looks like a bug and is not. Width does
        // not fall off monotonically with total twist: the twist runs as
        // `s^1.45`, so a blade with a quarter turn at its tip spends nearly all
        // of its length barely turned at all, and its area is indistinguishable
        // from flat. A half turn is the first angle at which the *middle* of the
        // blade sits near edge-on, which is where the area actually goes.
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let flat = measure(light, 0.0).2;
        let turned = measure(light, std::f32::consts::PI).2;
        assert!(
            turned < flat * 0.95,
            "a half-twisted blade covered {turned} px against {flat} flat"
        );
        // But it must not vanish. A field whose blades wink out as they turn is
        // far worse than one that never narrows at all.
        assert!(turned > flat * 0.55, "the twisted blade nearly disappeared");
    }

    #[test]
    fn a_forked_blade_paints_more_than_a_pointed_one() {
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let paint = |tip: TipProfile| {
            let (mut surface, origin) = page(96, 96);
            let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
            let root = painter.to_ground(Vec2::new(144.0, 240.0)).extend(0.0);
            painter.draw(&Stroke {
                root,
                bend: 0.3,
                length: 0.34,
                width: 3.0,
                tip,
                ..default()
            });
            surface.painted_map(96, 96).iter().sum::<f32>()
        };
        let pointed = paint(TipProfile::Pointed);
        let forked = paint(TipProfile::Forked {
            split_at: 0.78,
            opening: 0.35,
            long: 0.3,
            short: 0.18,
        });
        assert!(
            forked > pointed,
            "the fork added no paint: {forked} vs {pointed}"
        );
    }

    #[test]
    fn a_forks_children_start_where_the_parent_stopped() {
        // The property that makes it one blade separating rather than two glued
        // on: there must be no gap in the paint at the split.
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let (mut surface, origin) = page(96, 96);
        let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
        let root = painter.to_ground(Vec2::new(144.0, 250.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            azimuth: 0.0,
            bend: 0.2,
            length: 0.36,
            width: 3.5,
            tip: TipProfile::Forked {
                split_at: 0.75,
                opening: 0.30,
                long: 0.30,
                short: 0.20,
            },
            ..default()
        });
        // Walk up the painted column and check there is no empty row between the
        // lowest and highest paint — a gap is a fork whose children begin
        // somewhere other than where the parent ended.
        let painted = surface.painted_map(96, 96);
        let rows: Vec<bool> = (0..96)
            .map(|y| (0..96).any(|x| painted[y * 96 + x] > 0.0))
            .collect();
        let first = rows.iter().position(|p| *p).expect("nothing painted");
        let last = rows.iter().rposition(|p| *p).unwrap();
        assert!(
            rows[first..=last].iter().all(|p| *p),
            "the fork left a gap between rows {first} and {last}"
        );
    }

    #[test]
    fn a_fork_too_small_to_draw_becomes_a_notch() {
        // A page baked for a distant camera cannot resolve a fork, and two
        // subpixel children flicker independently as the ground slides under
        // the sampling grid. This is the collapse, exercised through the
        // rasteriser rather than only through `TipProfile`.
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let mut surface = Surface::new(64, 64);
        // An eighth of the authoring scale: a fork child is well under a pixel.
        let mut painter =
            Painter::at_scale(&mut surface, Vec2::ZERO, light, iso::PX_PER_METRE * 0.125);
        let root = painter.to_ground(Vec2::new(96.0, 120.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            length: 0.3,
            tip: TipProfile::Forked {
                split_at: 0.8,
                opening: 0.4,
                long: 0.22,
                short: 0.12,
            },
            ..default()
        });
        // It still draws something — collapsing must not delete the blade.
        assert!(surface.painted_map(64, 64).iter().any(|p| *p > 0.0));
    }
}

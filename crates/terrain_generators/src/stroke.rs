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

use glam::Vec3;

use crate::fastmath;
use crate::geometry::{self, Frame, TipProfile};
use crate::iso;
use crate::tone::Tone;

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

/// How sharply the tip lift gathers toward the end of a blade.
///
/// Raised from 1.4, and the reason is a critique rather than a measurement: a
/// gentle lift spread over the upper half makes the whole blade pale, so a tuft
/// of them is a *bright object* rather than a green object with lit tips. The
/// reference art has bright tips everywhere and bright blades almost nowhere,
/// and the difference between those two readings is entirely this exponent.
const TIP_CURVE: f32 = 2.4;

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
                    tip: fastmath::pow_from_log2(log_s, TIP_CURVE),
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
    /// and `bake`'s `the_stroke_reach_bound_is_never_beaten` sweeps
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

/// One sampled point on a blade's centreline, with everything a rasteriser
/// needs and nothing about where it is being drawn.
#[derive(Clone, Copy, Debug)]
pub struct BladeSample {
    /// World position of the centreline.
    pub position: Vec3,
    /// The orthonormal frame there, twist included.
    pub frame: Frame,
    /// Half-width in **reference** cache pixels, before foreshortening.
    pub half_reference: f32,
    /// Root-to-tip position on this segment, `0..1`.
    pub along: f32,
    /// The tip lift and glint at this point, already combined.
    pub tip_light: f32,
    /// How much the root's own shadow darkens this point.
    pub root_shade: f32,
}

/// Walk a blade's whole centreline, forks and all.
///
/// The one place the shape of a blade is decided, and it takes no view, no
/// surface and no light. That is the whole point: the camera pass and the shadow
/// pass have to rasterise **the same geometry**, and the only way to guarantee
/// that is for both to call this rather than each having its own idea of where a
/// blade goes. Regenerating the shape twice would nearly work — it is
/// deterministic — and "nearly" is exactly the failure that produces shadows
/// which do not quite belong to the blades casting them.
///
/// `arc_pixels` is how many target pixels the blade's arc spans, which sets how
/// finely it is walked. `tip` arrives already resolved, because whether a fork
/// can be drawn is a question about the target's resolution and this function
/// has no opinion about targets.
pub fn walk_blade(
    stroke: &Stroke,
    arc_pixels: f32,
    ribs_per_pixel: f32,
    tip: TipProfile,
    emit: &mut impl FnMut(BladeSample),
) {
    match tip {
        TipProfile::Forked {
            split_at,
            opening,
            long,
            short,
        } => {
            // The parent stops at the split and hands its state over. The
            // children continue from exactly there, which is what makes a fork
            // read as one blade separating rather than as two small blades glued
            // onto the end of a big one.
            //
            // It keeps the *whole* blade's width profile even though it stops
            // early, so it arrives at the split at its natural width rather than
            // tapering to a point there.
            let cursor = walk_segment(
                stroke,
                0.0,
                split_at,
                arc_pixels,
                ribs_per_pixel,
                (0.0, 1.0),
                None,
                emit,
            );
            // Asymmetric, and deliberately so: the long child keeps most of the
            // parent's heading and the short one does the turning. Two children
            // mirrored about the parent is a tuning fork, and the eye finds the
            // symmetry immediately.
            let half = opening * 0.5;
            walk_child(
                stroke,
                cursor,
                split_at,
                -half * 1.3,
                long,
                arc_pixels,
                ribs_per_pixel,
                emit,
            );
            walk_child(
                stroke,
                cursor,
                split_at,
                half * 0.7,
                short,
                arc_pixels,
                ribs_per_pixel,
                emit,
            );
        }
        TipProfile::Notched { depth } => {
            // A notch is a blade that stops a little short and blunt. Stopping
            // before the profile reaches its point is what makes it read as torn
            // rather than as tapered, and it is the shape a fork averages to once
            // it is too small to draw.
            let end = (1.0 - depth).clamp(0.2, 1.0);
            walk_segment(
                stroke,
                0.0,
                end,
                arc_pixels,
                ribs_per_pixel,
                (0.0, 1.0),
                None,
                emit,
            );
        }
        TipProfile::Pointed => {
            walk_segment(
                stroke,
                0.0,
                1.0,
                arc_pixels,
                ribs_per_pixel,
                (0.0, 1.0),
                None,
                emit,
            );
        }
    }
}

/// How much of the parent's width a fork's two children share between them.
///
/// Above a half on purpose: two children of exactly half the width read as a
/// blade that has been sliced, and a real split leaf has a little more material
/// than that because the two halves curl apart rather than lying flat.
const FORK_SHARE: f32 = 0.62;

/// One child of a forked tip.
#[allow(clippy::too_many_arguments)]
fn walk_child(
    stroke: &Stroke,
    from: Cursor,
    split_at: f32,
    turn: f32,
    length: f32,
    arc_pixels: f32,
    ribs_per_pixel: f32,
    emit: &mut impl FnMut(BladeSample),
) {
    // A child has to *continue* the parent's width, not restart from the
    // parent's root width — otherwise the blade visibly swells at the split,
    // which is the one thing a fork must never do. So the parent's own taper is
    // evaluated where the split happens and the children divide what is left.
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
    // Re-parameterised onto its own `0..1`, so a child tapers to its own point
    // rather than inheriting whichever fraction of the parent's taper it happens
    // to occupy.
    walk_segment(
        &child,
        split_at,
        split_at + length,
        arc_pixels,
        ribs_per_pixel,
        (split_at, split_at + length),
        Some(start),
        emit,
    );
}

/// Walk part of a centreline, emitting a sample per rib.
///
/// `from`/`to` are the arc parameters this segment walks. `profile_range` is the
/// arc range the *width profile* is stretched across, which is a different
/// question and has to be asked separately: a parent that stops at the split
/// still wants the whole blade's taper, so it arrives there at its natural
/// width; a child wants its own `0..1` so it tapers to its own point.
///
/// Returns where the walk ended, so a fork's children can continue from it.
#[allow(clippy::too_many_arguments)]
fn walk_segment(
    stroke: &Stroke,
    from: f32,
    to: f32,
    arc_pixels: f32,
    ribs_per_pixel: f32,
    profile_range: (f32, f32),
    start: Option<Cursor>,
    emit: &mut impl FnMut(BladeSample),
) -> Cursor {
    let span = (to - from).max(0.0);
    // Sample the centreline finely enough that consecutive ribs overlap: half a
    // target pixel apart leaves no gaps at any angle, and the cost is linear in
    // a quantity that is already small.
    let steps = (arc_pixels * span * ribs_per_pixel).clamp(4.0, 512.0) as usize;
    let inverse = 1.0 / steps as f32;
    let samples = step_table(steps);

    let kinks = stroke.kink != 0.0 || stroke.kink_turn != 0.0;
    let profile_index = stroke.profile as usize;
    let (profile_from, profile_to) = profile_range;
    let profile_span = (profile_to - profile_from).max(1.0e-4);
    // Most marks are walked whole, and for those the step table's cached powers
    // are exactly right — the segment parameter *is* the arc parameter. A fork's
    // children walk a slice of it and pay for their own, which is affordable
    // because they are a minority of a minority.
    let whole = from == 0.0 && span == 1.0;

    let mut cursor = start.unwrap_or(Cursor {
        position: stroke.root,
        angle: 0.0,
        heading: stroke.azimuth,
        twist: 0.0,
    });
    let segment = stroke.length * span * inverse;
    let handed = start.map(|s| s.heading - stroke.azimuth - stroke.sway * from * from);

    for sample in samples {
        // Where this rib sits on the parent's arc, and where it sits on the
        // width profile — two different parameters once a fork is involved.
        let s = from + sample.s * span;
        let profile_s = ((s - profile_from) / profile_span).clamp(0.0, 1.0);
        // Smooth arc, plus an elbow. The smoothstep is narrow on purpose: spread
        // it out and the kink becomes just more curvature.
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
        if let Some(offset) = handed {
            // A child inherits the turn its parent handed it on top of whatever
            // the shared centreline says.
            cursor.heading += offset;
        }
        cursor.twist = stroke.twist * twist_power;

        let (sin_heading, cos_heading) = fastmath::sin_cos(cursor.heading);
        let (sin_angle, cos_angle) = fastmath::sin_cos(cursor.angle);
        let frame = Frame::build(sin_heading, cos_heading, sin_angle, cos_angle, cursor.twist);

        let index = ((profile_s * (samples.len() - 1) as f32) as usize).min(samples.len() - 1);
        let shape = samples[index];

        emit(BladeSample {
            position: cursor.position,
            frame,
            half_reference: stroke.width * shape.widths[profile_index] + stroke.tip_width,
            along: s.clamp(0.0, 1.0),
            // Two terms doing different jobs, and carried by two different
            // fields so they can be dealt out at different rates. The gentle one
            // lifts the whole blade a little; the glint does not wake up until
            // the last fifth.
            tip_light: stroke.tip_light * shape.tip + stroke.glint * shape.s8,
            // Roots sit in their own shadow. Without this every blade glows at
            // the base and the canopy loses its floor.
            root_shade: shape.root_shade,
        });

        // Advance along the arc. Past a right angle `cos` goes negative and the
        // tip starts descending, which is exactly the hook shape the reference is
        // full of.
        cursor.position += frame.tangent * segment;
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fastmath;

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
}

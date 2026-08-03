//! How a surface turns into a light index.
//!
//! Everything here takes world-space normals and a world-space sun. That sounds
//! obvious and it is the thing the old renderer could not do: its key light was
//! authored in image coordinates where `+Z` points at the viewer, and every term
//! that read `light.z` as an up-ness was reading toward-the-viewer-ness instead.
//! [`crate::iso::image_to_world`] is the bridge and carries the warning.
//!
//! ## Three normals, blended, not chosen
//!
//! A meadow has form at three scales and they are not the same shape:
//!
//! ```text
//!   ground normal    metres      which way the terrain faces
//!   canopy normal    centimetres the crown of a tuft against the valley beside it
//!   blade normal     millimetres which edge of this leaf catches the sun
//! ```
//!
//! Shading against only the blade normal gives a field of individually correct
//! leaves with no larger shape — every tuft equally lit, no sense of a mound
//! having a lit face. Shading against only the canopy normal gives a smooth
//! velvet dune with no leaves in it. The blend is what makes a bright crown made
//! of individually shaded blades, which is what the reference art is.
//!
//! The weights lean on the blade because that is what the eye reads first at the
//! scale this is drawn, but the two coarser terms carry the composition, and a
//! composition that disappears when you squint is the failure mode this is
//! guarding against.
//!
//! ## Nothing here clamps to zero
//!
//! A grass blade is a few cells thick. The face turned away from the sun is not
//! black — it is lit by what came through the leaf and what bounced off the
//! canopy underneath. A hard `max(N·L, 0)` gives every blade a terminator and a
//! dead back, which is most of what makes procedural vegetation read as moulded
//! plastic. Every diffuse term here is wrapped, and the back of a leaf gets its
//! own transmission term on top.

use bevy::prelude::*;

use crate::iso;

/// How far past the terminator a wrapped diffuse keeps giving light.
///
/// Not a fudge factor. Grass is thin and translucent and sits over more of
/// itself, so the transition from lit to unlit happens over a much wider angle
/// than an opaque surface's does. Below about a third the field grows hard
/// terminators across every blade; above about a half the form stops reading at
/// all, because everything is lit.
pub const WRAP: f32 = 0.45;

/// What a wrapped, two-sided diffuse averages to over all normals.
///
/// Subtracted wherever the form term is applied, so it *redistributes* light
/// rather than adding any. A shading model whose mean brightness moved when it
/// changed would need every other constant in the baker retuned around it, and
/// the retune would be indistinguishable from the improvement.
pub const FORM_MEAN: f32 = 0.655;

/// How much light a thin two-sided surface shows, `0..1`.
///
/// The absolute value is the two-sidedness: which face of a leaf the camera
/// happens to see should not decide whether it is lit, because the leaf is thin
/// enough that both faces are. The underside is darkened separately, by
/// [`underside_fill`], because it is darker for a different reason — it faces
/// the ground rather than the sky.
#[inline]
pub fn wrapped(normal: Vec3, sun: Vec3) -> f32 {
    let facing = normal.dot(sun).abs();
    ((facing + WRAP) / (1.0 + WRAP)).clamp(0.0, 1.0)
}

/// How the three scales of form are weighted against each other.
#[derive(Clone, Copy, Debug)]
pub struct FormWeights {
    pub ground: f32,
    pub canopy: f32,
    pub blade: f32,
}

impl Default for FormWeights {
    fn default() -> Self {
        // Fifteen, forty, forty-five. The canopy takes a share off the blade,
        // and the reason is a critique the first split earned: with the blade
        // leading, every tuft was lit the same amount and the field read as a
        // carpet with brighter patches rather than as plants. A tuft's *crown*
        // is the thing that has a lit flank and a dark one, and describing that
        // is worth more than describing which edge of each leaf inside it
        // catches — because the crown is what survives being squinted at, and
        // the leaf is not.
        //
        // The ground stays small: terrain in this art is a rhythm rather than a
        // subject.
        Self {
            ground: 0.15,
            canopy: 0.40,
            blade: 0.45,
        }
    }
}

/// The form term: how lit this surface is, before shadow and occlusion.
///
/// Returns a value centred on zero — positive where the surface faces the sun,
/// negative where it faces away — so a caller adds it to a light index without
/// having to subtract a mean it would otherwise have to know.
#[inline]
pub fn form(weights: FormWeights, ground: Vec3, canopy: Vec3, blade: Vec3, sun: Vec3) -> f32 {
    weights.ground * wrapped(ground, sun)
        + weights.canopy * wrapped(canopy, sun)
        + weights.blade * wrapped(blade, sun)
        - (weights.ground + weights.canopy + weights.blade) * FORM_MEAN
}

/// Light that has come *through* a leaf rather than off it.
///
/// The single term that separates lit grass from cut-out grass, and it is
/// strongest exactly where the reflected term is weakest — a leaf edge-on to the
/// sun with the sun behind it glows, because a few cells of leaf is not opaque.
///
/// Raised to a power so it is a rim rather than a wash: a transmission term
/// spread evenly over a blade makes the whole field look backlit, which is a
/// different and equally wrong picture. `thinness` is how little material there
/// is to get through — a tip transmits, a root does not.
#[inline]
pub fn transmission(normal: Vec3, sun: Vec3, thinness: f32) -> f32 {
    let behind = (-normal.dot(sun)).max(0.0);
    behind * behind * thinness
}

/// How much darker the back of a leaf runs than its face.
///
/// Small, and it is a *fill* rather than a shadow. The underside of a leaf sees
/// the ground and the canopy below it rather than the sky, so it loses ambient
/// light — but it is the same leaf, so it must not lose its colour.
#[inline]
pub fn underside_fill(underside: bool) -> f32 {
    if underside { -0.055 } else { 0.0 }
}

/// A broad waxy highlight along the leaf.
///
/// Blinn-Phong against the half vector rather than a mirror reflection, and
/// deliberately broad: grass has a soft sheen, not a specular pinprick. A narrow
/// lobe on geometry this fine produces exactly the scintillating pixels that
/// cannot be filtered and therefore crawl whenever the ground moves under the
/// sampling grid.
///
/// The camera has no perspective, so the view direction is a constant and the
/// half vector can be built once per bake.
#[inline]
pub fn half_vector(sun: Vec3) -> Vec3 {
    (sun + iso::TOWARD_VIEWER).normalize_or(iso::TOWARD_VIEWER)
}

/// The sheen at one surface point, `0..1`.
#[inline]
pub fn sheen(normal: Vec3, half: Vec3, power: f32) -> f32 {
    let facing = normal.dot(half).abs();
    crate::fastmath::pow(facing.clamp(0.0, 1.0), power)
}

/// The world normal of a height field sampled on the page grid.
///
/// `slope` is how fast the height rises per page pixel, in **reference** pixels,
/// on each axis. The conversion is exact rather than approximate, and it has to
/// be: a page pixel is not a square of ground. Stepping one pixel across the
/// screen and one pixel down it move a point along two different, non-orthogonal
/// world directions, because the world axes run along the screen's diagonals.
///
/// Working it through — a page step of `(1, 0)` moves the ground by
/// `(0.5, -0.5) / px` and a step of `(0, 1)` moves it by `(1, 1) / px` — the
/// cross product of the two tangents collapses to this.
#[inline]
pub fn height_field_normal(slope: Vec2, detail: f32) -> Vec3 {
    let a = slope.x * detail;
    let b = slope.y * detail;
    Vec3::new(-(a + 0.5 * b), a - 0.5 * b, 1.0).normalize_or(Vec3::Z)
}

/// Where a canopy's own shape stops the sky reaching it, `0..1`.
///
/// Horizon-based rather than a difference of blurs, and the distinction is what
/// the offline budget buys. A blur difference answers "is this lower than its
/// neighbourhood", which is the same answer in every direction — so it darkens a
/// hollow and a narrow slot between two crowns by the same amount, when the slot
/// is far more occluded. Scanning outward along real directions and keeping the
/// steepest horizon in each answers "how much sky can this point actually see",
/// which is the question.
///
/// The radii do three jobs at once and are chosen to span them: the near ones
/// are one blade against the blade behind it, the middle ones are the inside of
/// a tuft, and the far ones are a crown against the valley beside it. Sampling a
/// geometric spread rather than a uniform one is what lets five taps cover two
/// orders of magnitude of scale.
pub fn horizon_occlusion(
    heights: &[f32],
    width: usize,
    height: usize,
    directions: usize,
    radii: &[f32],
) -> Vec<f32> {
    let mut occlusion = vec![0.0f32; width * height];
    if directions == 0 || radii.is_empty() {
        return occlusion;
    }
    // Precomputed, because the inner loop runs a hundred million times on a
    // page and a sine in it would be most of the cost.
    let steps: Vec<Vec2> = (0..directions)
        .map(|i| {
            let angle = i as f32 / directions as f32 * std::f32::consts::TAU;
            Vec2::new(angle.cos(), angle.sin())
        })
        .collect();

    for y in 0..height {
        for x in 0..width {
            let base = heights[y * width + x];
            let mut total = 0.0f32;
            for step in &steps {
                // The steepest thing this direction has to climb over.
                let mut horizon = 0.0f32;
                for radius in radii {
                    let sample = Vec2::new(x as f32, y as f32) + *step * *radius;
                    if sample.x < 0.0 || sample.y < 0.0 {
                        continue;
                    }
                    let (sx, sy) = (sample.x as usize, sample.y as usize);
                    if sx >= width || sy >= height {
                        continue;
                    }
                    let rise = heights[sy * width + sx] - base;
                    horizon = horizon.max(rise / radius.max(1.0));
                }
                // The sine of the horizon angle, without the trigonometry: a
                // slope of one is 45° and hides half the sky in that direction.
                total += horizon / (1.0 + horizon);
            }
            occlusion[y * width + x] = total / directions as f32;
        }
    }
    occlusion
}

/// How dark a stack of overlapping leaves gets, `0..1`.
///
/// Beer–Lambert against the count of fragments that passed through a pixel,
/// winner and loser alike. This is the term the horizon scan cannot supply: the
/// canopy height field says how high the surface is, and says nothing at all
/// about whether there is one blade there or fifteen. The inside of a tuft is
/// dark because it is *full*, not because it is low.
#[inline]
pub fn optical_occlusion(depth: f32, density: f32) -> f32 {
    1.0 - (-density * depth).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_diffuse_never_reaches_black() {
        // The property that keeps the shaded side of a leaf a colour rather than
        // an absence. A hard `max(N·L, 0)` gives every blade a terminator.
        let sun = Vec3::new(0.0, 0.6, 0.8).normalize();
        let mut lowest = f32::INFINITY;
        for i in 0..64 {
            for j in 0..64 {
                let (a, b) = (
                    i as f32 / 64.0 * std::f32::consts::TAU,
                    j as f32 / 64.0 * std::f32::consts::PI,
                );
                let normal = Vec3::new(b.sin() * a.cos(), b.sin() * a.sin(), b.cos());
                lowest = lowest.min(wrapped(normal, sun));
            }
        }
        assert!(lowest > 0.25, "the shaded side bottoms out at {lowest}");
    }

    #[test]
    fn the_form_term_costs_no_exposure() {
        // It redistributes light; it must not add or remove any. Otherwise every
        // constant tuned against the old shading would need moving, and the
        // retune would be indistinguishable from the improvement.
        let sun = Vec3::new(0.1, 0.6, 0.79).normalize();
        let weights = FormWeights::default();
        let mut total = 0.0f64;
        let mut count = 0usize;
        for i in 0..96 {
            for j in 0..96 {
                let (a, b) = (
                    i as f32 / 96.0 * std::f32::consts::TAU,
                    (j as f32 + 0.5) / 96.0 * std::f32::consts::PI,
                );
                // Weighted by the solid angle, so this is a genuine spherical
                // average rather than one biased toward the poles.
                let weight = b.sin() as f64;
                let normal = Vec3::new(b.sin() * a.cos(), b.sin() * a.sin(), b.cos());
                total += form(weights, normal, normal, normal, sun) as f64 * weight;
                count += 1;
                let _ = count;
            }
        }
        let mean = total / (96.0 * 96.0 * 2.0 / std::f32::consts::PI as f64);
        assert!(
            mean.abs() < 0.02,
            "the form term shifts the mean light index by {mean:.4}"
        );
    }

    #[test]
    fn turning_the_sun_turns_the_form() {
        let normal = Vec3::new(0.6, 0.0, 0.8).normalize();
        let weights = FormWeights::default();
        let toward = form(weights, normal, normal, normal, normal);
        let across = form(
            weights,
            normal,
            normal,
            normal,
            Vec3::new(-0.8, 0.0, 0.6).normalize(),
        );
        assert!(
            toward > across + 0.1,
            "facing the sun is worth only {toward} against {across}"
        );
    }

    #[test]
    fn a_flat_canopy_faces_straight_up() {
        assert!((height_field_normal(Vec2::ZERO, 1.0) - Vec3::Z).length() < 1.0e-6);
    }

    #[test]
    fn a_rising_canopy_leans_away_from_the_rise() {
        // Page +x is screen right, which is world `(+X, −Y)`. A canopy rising
        // that way has to tilt its normal the other way, toward `(−X, +Y)`.
        let normal = height_field_normal(Vec2::new(0.4, 0.0), 1.0);
        assert!(normal.x < 0.0 && normal.y > 0.0, "{normal:?}");
        assert!(normal.z > 0.0, "the canopy normal turned upside down");
        // And steeper ground tilts further.
        let steeper = height_field_normal(Vec2::new(1.2, 0.0), 1.0);
        assert!(steeper.z < normal.z);
    }

    #[test]
    fn every_normal_the_height_field_produces_is_unit_length() {
        for i in -8..=8 {
            for j in -8..=8 {
                let normal = height_field_normal(Vec2::new(i as f32 * 0.4, j as f32 * 0.4), 1.3);
                assert!((normal.length() - 1.0).abs() < 1.0e-5, "{normal:?}");
            }
        }
    }

    #[test]
    fn a_pit_is_more_occluded_than_a_plain() {
        // The basic claim, and the one a difference of blurs also makes.
        const W: usize = 33;
        let mut heights = vec![0.0f32; W * W];
        // A ring of tall canopy around the middle.
        for y in 0..W {
            for x in 0..W {
                let d = ((x as f32 - 16.0).powi(2) + (y as f32 - 16.0).powi(2)).sqrt();
                if (6.0..12.0).contains(&d) {
                    heights[y * W + x] = 20.0;
                }
            }
        }
        let ao = horizon_occlusion(&heights, W, W, 8, &[2.0, 4.0, 8.0]);
        let pit = ao[16 * W + 16];
        let outside = ao[16 * W + 30];
        assert!(
            pit > outside * 2.0,
            "the pit is {pit:.3} occluded and the open ground {outside:.3}"
        );
    }

    #[test]
    fn a_hollow_is_darker_than_a_slot_of_the_same_depth() {
        // The claim a difference of blurs *cannot* make, and the reason this is
        // a directional scan rather than a cheaper one.
        //
        // Both points sit the same distance below the canopy around them, so any
        // measure of "how far below my neighbourhood am I" rates them equally.
        // They are not equal: the slot has open sky along its length and the
        // hollow has walls in every direction, and the difference between those
        // two is most of what separates the gap between two crowns from the
        // inside of a tuft.
        const W: usize = 41;
        let ridge = |slot: bool| {
            let mut heights = vec![0.0f32; W * W];
            for y in 0..W {
                for x in 0..W {
                    let far = (x as f32 - 20.0).abs();
                    // A slot: two walls either side, open along the other axis.
                    // A hollow: walls all round.
                    let inside = if slot {
                        far > 3.0 && far < 9.0
                    } else {
                        let d = ((x as f32 - 20.0).powi(2) + (y as f32 - 20.0).powi(2)).sqrt();
                        (3.0..9.0).contains(&d)
                    };
                    if inside {
                        heights[y * W + x] = 24.0;
                    }
                }
            }
            horizon_occlusion(&heights, W, W, 16, &[2.0, 4.0, 8.0])[20 * W + 20]
        };
        let (slot, hollow) = (ridge(true), ridge(false));
        assert!(
            hollow > slot * 1.25,
            "a hollow ({hollow:.3}) is no darker than a slot ({slot:.3}), so the \
             scan is not reading direction"
        );
        // And both are genuinely occluded — a test that passed by rating the
        // slot at nothing would be measuring a broken scan.
        assert!(slot > 0.2, "the slot reads as open ground: {slot:.3}");
    }

    #[test]
    fn flat_ground_is_not_occluded() {
        let heights = vec![7.0f32; 24 * 24];
        let ao = horizon_occlusion(&heights, 24, 24, 8, &[2.0, 5.0]);
        assert!(ao.iter().all(|v| *v < 1.0e-5), "flat ground shaded itself");
    }

    #[test]
    fn stacked_leaves_darken_and_saturate() {
        // One layer is barely anything; a dozen is opaque and a dozen more
        // changes nothing, which is why the counter saturates in a byte.
        let one = optical_occlusion(1.0, 0.14);
        let dozen = optical_occlusion(12.0, 0.14);
        let many = optical_occlusion(40.0, 0.14);
        assert!(one < 0.2, "a single leaf occludes {one:.3}");
        assert!(dozen > 0.7, "a dozen leaves occlude only {dozen:.3}");
        assert!(many - dozen < 0.3, "the term has not saturated by forty");
        assert!(optical_occlusion(0.0, 0.14).abs() < 1.0e-6);
    }

    #[test]
    fn transmission_is_a_rim_rather_than_a_wash() {
        let sun = Vec3::new(0.0, 0.6, 0.8).normalize();
        // Facing the sun: nothing comes through, because nothing is behind.
        assert!(transmission(sun, sun, 1.0) < 1.0e-6);
        // Facing directly away: the most.
        assert!(transmission(-sun, sun, 1.0) > 0.9);
        // Edge-on: nearly nothing, which is what makes it a rim. A linear term
        // would still be at a half here and the whole field would look backlit.
        let edge = Vec3::new(0.8, -0.48, 0.36).normalize();
        assert!(transmission(edge, sun, 1.0) < 0.25, "the rim is a wash");
    }
}

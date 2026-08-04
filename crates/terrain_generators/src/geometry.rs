//! What shape a blade is, in three dimensions.
//!
//! The old renderer drew a blade as a centreline plus a scalar width, laid down
//! as a one-pixel-at-a-time span across the screen. Its lateral shading —
//! "pretend the cross-section is a cylinder, one edge faces the key" — was a
//! good approximation of the thing this module now computes properly, and it had
//! one fatal limitation: the cylinder was pretended in *screen* space, so it had
//! no idea which way the blade actually pointed. Turn the sun ninety degrees and
//! nothing moved.
//!
//! So a blade now carries a real frame.
//!
//! ## The frame
//!
//! At every point along the centreline there are three orthonormal world
//! directions:
//!
//! ```text
//!   T  tangent    along the blade, the direction the arc is travelling
//!   B  binormal   across the blade, rotated about T by the twist
//!   N  normal     out of the blade's face, = T × B
//! ```
//!
//! `B` starts horizontal and perpendicular to the blade's own heading — which is
//! exactly what a grass blade does, since it grows out of a sheath and its flat
//! face begins level. The twist then rotates `B` and `N` together about `T`,
//! which is the cheapest thing in this module and the one that buys the most:
//! without it every blade in a tuft presents the same face to the sun and the
//! tuft reads as a comb.
//!
//! ## The cross-section
//!
//! A blade is not a flat ribbon and not a cylinder. It is a shallow trough with
//! a raised central ridge, and that shape is what produces the reading the
//! reference art has everywhere — a lit edge, a darker opposite edge, and a
//! highlight that runs along the middle at some orientations and not at others.
//!
//! Across the width, at `u ∈ [−1, 1]`:
//!
//! ```text
//!   lift(u)  = ridge · (1 − u²)          the centre stands proud
//!   normal   = normalise(N + 2·ridge·u·B)
//! ```
//!
//! That falls straight out of differentiating the cross-section curve, and it is
//! worth having in closed form rather than as three tessellated strips: a strip
//! count is a quality setting that changes the silhouette, and this changes only
//! how the surface is shaded.
//!
//! ## Foreshortening
//!
//! A real ribbon seen edge-on is invisible, and a field of grass whose blades
//! vanish and reappear as they twist is worse than one that never narrows at
//! all. So width responds to the viewing angle but never all the way to zero —
//! see [`foreshorten`].

use glam::{Vec2, Vec3};

use crate::fastmath;
use crate::iso;

/// How width varies from root to tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Profile {
    /// Wide at the root, tapering to a point. The old grass profile.
    Tapered = 0,
    /// Narrow at both ends, widest in the middle. A leaflet.
    Oval = 1,
    /// Nearly constant. Stems and the odd reed.
    Stem = 2,
    /// Narrow where it attaches, broadest a third of the way up, then a long
    /// taper and a quick point.
    ///
    /// The profile actual grass has, and the one the old renderer did not. A
    /// blade that is widest at the root and only ever narrows reads as a needle
    /// — the eye picks up "triangle" long before it picks up "leaf" — and no
    /// amount of curvature or colour repairs it, because the silhouette is
    /// wrong at every size.
    ///
    /// Three factors, each doing one job: a shoulder that opens over the first
    /// third, a gentle body taper, and a sharp collapse over the last seventh
    /// that gives the tip a point rather than a stub.
    Leaf = 3,
}

impl Profile {
    /// Every profile, indexed by its discriminant.
    pub const ALL: [Profile; 4] = [
        Profile::Tapered,
        Profile::Oval,
        Profile::Stem,
        Profile::Leaf,
    ];

    /// The taper, written the obvious way.
    ///
    /// Not on the rasteriser's path — [`Profile::width_from_logs`] is — but kept
    /// as the definition the fast one is checked against, which is the only
    /// thing that makes the fast one reviewable.
    #[inline]
    pub fn width_at(self, s: f32) -> f32 {
        match self {
            // 1.2 rather than 1.0: blades hold their width for the first third
            // and then give it up quickly, which is what makes them read as
            // blades rather than as triangles.
            Profile::Tapered => (1.0 - s).powf(1.2),
            Profile::Oval => (s * (1.0 - s) * 4.0).powf(0.55),
            Profile::Stem => (1.0 - s * 0.55).powf(0.7),
            Profile::Leaf => {
                (LEAF_ROOT + (1.0 - LEAF_ROOT) * smoothstep(0.0, LEAF_SHOULDER, s))
                    * (1.0 - s).powf(LEAF_TAPER)
                    * (1.0 - smoothstep(LEAF_POINT, 1.0, s) * LEAF_COLLAPSE)
            }
        }
    }

    /// [`Profile::width_at`], reusing logarithms the caller already has.
    ///
    /// Every one of these is a power of `s` or of `1 - s`, and so are three more
    /// terms the same loop needs. Taking the two logarithms once and reusing
    /// them turns six transcendentals per rib into two — see
    /// [`crate::fastmath`] for why that was worth restructuring the signature
    /// for.
    #[inline]
    pub fn width_from_logs(self, s: f32, log_s: f32, log_rest: f32) -> f32 {
        match self {
            Profile::Tapered => fastmath::pow_from_log2(log_rest, 1.2),
            // log2(4·s·(1−s)) — the product becomes a sum, so this one shares
            // both logarithms rather than needing a third.
            Profile::Oval => fastmath::pow_from_log2(log_s + log_rest + 2.0, 0.55),
            // The only base that is neither `s` nor `1 - s`, and the rarest
            // profile in the vocabulary, so it pays for its own logarithm.
            Profile::Stem => fastmath::pow(1.0 - s * 0.55, 0.7),
            // Two smoothsteps and a shared logarithm. The smoothsteps are
            // polynomials, so the leaf profile costs no more transcendentals
            // than the taper it replaces.
            Profile::Leaf => {
                (LEAF_ROOT + (1.0 - LEAF_ROOT) * smoothstep(0.0, LEAF_SHOULDER, s))
                    * fastmath::pow_from_log2(log_rest, LEAF_TAPER)
                    * (1.0 - smoothstep(LEAF_POINT, 1.0, s) * LEAF_COLLAPSE)
            }
        }
    }
}

/// How wide a leaf blade is where it attaches, as a fraction of its widest.
const LEAF_ROOT: f32 = 0.52;
/// Where the leaf profile finishes opening out.
const LEAF_SHOULDER: f32 = 0.34;
/// The body taper's exponent. Gentler than [`Profile::Tapered`]'s, because the
/// shoulder is already taking width out of the root end.
const LEAF_TAPER: f32 = 0.62;
/// Where the final collapse to a point begins.
const LEAF_POINT: f32 = 0.86;
/// How much of the remaining width the final collapse removes.
const LEAF_COLLAPSE: f32 = 0.75;

#[inline]
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What happens at the end of a blade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TipProfile {
    /// It comes to a point. Most blades.
    Pointed,
    /// It is torn or blunt. Cheap, and the right thing for a fork that has
    /// become too small to resolve — see [`TipProfile::resolved_at`].
    Notched {
        /// How deep the notch cuts, as a fraction of the blade.
        depth: f32,
    },
    /// It splits in two.
    ///
    /// The single most visible piece of morphology in the target art, and the
    /// one that needs the most restraint. Fork every blade and a meadow becomes
    /// antlers; fork the broad mature minority and it reads as grass that has
    /// been alive for a while.
    Forked {
        /// Where along the parent the split begins, `0..1`. Late — a fork that
        /// starts halfway up is two blades glued together at the root.
        split_at: f32,
        /// Total angle between the two children, radians.
        opening: f32,
        /// Length of the longer child, as a fraction of the whole blade.
        long: f32,
        /// Length of the shorter child, likewise. Asymmetry is most of what
        /// stops a fork reading as a tuning fork.
        short: f32,
    },
}

impl TipProfile {
    /// The tip this one becomes when the page cannot resolve it.
    ///
    /// A fork whose children are narrower than a page pixel does not read as a
    /// fork. It reads as a flickering pair of dashes, because a feature below
    /// the sampling rate cannot be filtered, only sampled — and the two children
    /// wink independently as the ground slides under the grid. Collapsing to the
    /// notch it would have averaged to is both cheaper and more stable.
    ///
    /// `child_pixels` is how many final page pixels the shorter child spans.
    pub fn resolved_at(self, child_pixels: f32) -> Self {
        match self {
            TipProfile::Forked { short, .. } if child_pixels < FORK_RESOLVES_ABOVE => {
                TipProfile::Notched {
                    depth: short.min(0.2),
                }
            }
            other => other,
        }
    }

    /// How far past the parent's own length this tip can reach, as a fraction.
    ///
    /// The guard band has to know. A fork's children continue from the split
    /// rather than replacing what is past it, so a forked blade is genuinely
    /// longer than its nominal arc and a band sized for the arc alone would clip
    /// the outer child on one side of a page join and not the other.
    pub fn extra_reach(self) -> f32 {
        match self {
            TipProfile::Forked {
                split_at,
                long,
                short,
                ..
            } => (split_at + long.max(short) - 1.0).max(0.0),
            _ => 0.0,
        }
    }
}

/// Below this many final page pixels, a fork's child stops being drawn as one.
///
/// Three. Two children of one and a half pixels each, separated by a gap of
/// under one, is not a shape — it is a dotted line that moves.
pub const FORK_RESOLVES_ABOVE: f32 = 3.0;

/// The orthonormal world frame at a point on a blade.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Along the blade, unit length.
    pub tangent: Vec3,
    /// Across the blade, unit length, already twisted.
    pub binormal: Vec3,
    /// Out of the blade's face, unit length, already twisted.
    pub normal: Vec3,
}

impl Frame {
    /// Build the frame from the blade's heading, its angle from vertical, and
    /// how far the surface has twisted by this point.
    ///
    /// `heading` is the world azimuth the blade leans toward and `angle` is how
    /// far it has bent from straight up, which is exactly the pair the arc
    /// integration already carries — so the frame costs a rotation and no extra
    /// state.
    #[inline]
    pub fn build(
        sin_heading: f32,
        cos_heading: f32,
        sin_angle: f32,
        cos_angle: f32,
        twist: f32,
    ) -> Self {
        // The direction of travel, which is what the integrator steps along.
        let tangent = Vec3::new(sin_angle * cos_heading, sin_angle * sin_heading, cos_angle);
        // Level and across the lean. Perpendicular to the tangent for free, at
        // any bend: their dot product is `sinθ·cosφ·(−sinφ) + sinθ·sinφ·cosφ`,
        // which cancels exactly.
        let flat = Vec3::new(-sin_heading, cos_heading, 0.0);
        let up = tangent.cross(flat);
        // Rodrigues about the tangent, with the term along it dropped because
        // `flat` is already perpendicular to it.
        let (sin_twist, cos_twist) = fastmath::sin_cos(twist);
        Self {
            tangent,
            binormal: flat * cos_twist + up * sin_twist,
            normal: up * cos_twist - flat * sin_twist,
        }
    }

    /// The surface normal partway across the blade, `u` running edge to edge.
    ///
    /// The closed form of differentiating the raised cross-section. `ridge` is
    /// how far the centre stands proud as a fraction of the half-width, so the
    /// shape is scale-free and a wide blade and a narrow one curve alike.
    #[inline]
    pub fn across(&self, u: f32, ridge: f32) -> Vec3 {
        (self.normal + self.binormal * (2.0 * ridge * u)).normalize_or(self.normal)
    }
}

/// How far a leaf's centre stands proud of its edges, as a fraction of the
/// half-width.
///
/// A third. Grass has a pronounced midrib and a shallow trough either side of
/// it, and this is what turns "one edge is lighter" into "the blade has a
/// shape". Push it much past a half and the blade starts reading as a tube;
/// below about a fifth the ridge highlight stops being findable.
pub const RIDGE: f32 = 0.34;

/// How much of a blade's screen width is lost when it turns edge-on.
///
/// Not all of it, on purpose. A ribbon seen exactly edge-on covers no pixels,
/// which is correct and unusable: twist makes every blade pass through that
/// angle somewhere along its length, and a field whose blades wink out in the
/// middle is far worse than one that never narrows. Half is enough that the
/// twist is legible in the silhouette and not so much that anything vanishes.
pub const FORESHORTEN: f32 = 0.5;

/// How much narrower this surface looks from the camera, `1 − FORESHORTEN` to 1.
///
/// The camera has no perspective, so "how edge-on is this" is one dot product
/// against a fixed direction — see [`iso::TOWARD_VIEWER`].
#[inline]
pub fn foreshorten(normal: Vec3) -> f32 {
    let facing = normal.dot(iso::TOWARD_VIEWER).abs();
    // Smooth rather than linear, so the narrowing happens over the middle of
    // the range instead of concentrating all of it at the grazing angles where
    // the blade is already hard to see.
    (1.0 - FORESHORTEN) + FORESHORTEN * smoothstep(0.0, 0.62, facing)
}

/// How far a blade's shadow reaches along the ground, per unit of its height.
///
/// The number the guard band is sized from, and it is a function of the sun
/// alone: `|L.xy| / L.z`, which is one over the tangent of the elevation. At
/// 35° it is 1.43; at 20° it would be 2.75, and the band — and the cost of every
/// page — would nearly double with it.
#[inline]
pub fn reach_per_height(sun: Vec3) -> f32 {
    let plane = Vec2::new(sun.x, sun.y).length();
    plane / sun.z.abs().max(1.0e-3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(heading: f32, angle: f32, twist: f32) -> Frame {
        let (sin_h, cos_h) = heading.sin_cos();
        let (sin_a, cos_a) = angle.sin_cos();
        Frame::build(sin_h, cos_h, sin_a, cos_a, twist)
    }

    #[test]
    fn the_frame_is_orthonormal_at_every_pose() {
        for heading in 0..12 {
            for angle in 0..8 {
                for twist in 0..6 {
                    let f = frame(
                        heading as f32 / 12.0 * std::f32::consts::TAU,
                        angle as f32 / 8.0 * std::f32::consts::PI,
                        twist as f32 / 6.0 * std::f32::consts::TAU,
                    );
                    for axis in [f.tangent, f.binormal, f.normal] {
                        assert!((axis.length() - 1.0).abs() < 1.0e-4, "{axis:?}");
                        assert!(axis.is_finite());
                    }
                    assert!(f.tangent.dot(f.binormal).abs() < 1.0e-4);
                    assert!(f.tangent.dot(f.normal).abs() < 1.0e-4);
                    assert!(f.binormal.dot(f.normal).abs() < 1.0e-4);
                }
            }
        }
    }

    #[test]
    fn an_upright_untwisted_blade_has_a_level_binormal() {
        // The claim the frame is built on: a blade leaves its sheath with its
        // flat face level, so the width axis starts horizontal.
        let f = frame(0.7, 0.0, 0.0);
        assert!(f.binormal.z.abs() < 1.0e-5, "{:?}", f.binormal);
    }

    #[test]
    fn a_quarter_twist_swaps_the_face_for_the_edge() {
        let flat = frame(0.0, 0.3, 0.0);
        let turned = frame(0.0, 0.3, std::f32::consts::FRAC_PI_2);
        // A quarter turn puts the old width axis where the old normal was.
        assert!(
            turned.binormal.dot(flat.normal).abs() > 0.99,
            "{:?} vs {:?}",
            turned.binormal,
            flat.normal
        );
    }

    #[test]
    fn the_two_edges_of_a_blade_face_opposite_ways() {
        // The whole reason a blade has a lit side and a dark side.
        let f = frame(0.4, 0.5, 0.2);
        let left = f.across(-1.0, RIDGE);
        let right = f.across(1.0, RIDGE);
        assert!(left.dot(f.binormal) < 0.0);
        assert!(right.dot(f.binormal) > 0.0);
        // And the middle looks straight out of the face.
        assert!(f.across(0.0, RIDGE).dot(f.normal) > 0.999);
    }

    #[test]
    fn a_flat_blade_has_the_same_normal_everywhere() {
        let f = frame(0.4, 0.5, 0.2);
        for u in [-1.0, -0.3, 0.0, 0.6, 1.0] {
            assert!((f.across(u, 0.0) - f.normal).length() < 1.0e-5);
        }
    }

    #[test]
    fn the_leaf_profile_is_broadest_partway_up() {
        // The property that makes it a leaf rather than a needle.
        let widest = (0..=100)
            .map(|i| i as f32 / 100.0)
            .fold((0.0f32, 0.0f32), |best, s| {
                let w = Profile::Leaf.width_at(s);
                if w > best.1 { (s, w) } else { best }
            });
        assert!(
            (0.20..=0.50).contains(&widest.0),
            "the leaf is broadest at s = {}",
            widest.0
        );
        // Narrower where it attaches than at its widest, and to a point at the
        // end.
        assert!(Profile::Leaf.width_at(0.0) < widest.1 * 0.75);
        assert!(Profile::Leaf.width_at(1.0) < 1.0e-4);
        // And the last seventh collapses faster than the body taper.
        let body = Profile::Leaf.width_at(0.70) - Profile::Leaf.width_at(0.80);
        let point = Profile::Leaf.width_at(0.88) - Profile::Leaf.width_at(0.98);
        assert!(point > body, "the tip does not come to a point");
    }

    #[test]
    fn every_profile_stays_positive_and_finite() {
        for profile in Profile::ALL {
            for i in 0..=200 {
                let w = profile.width_at(i as f32 / 200.0);
                assert!(w.is_finite() && w >= 0.0, "{profile:?} at {i}: {w}");
            }
        }
    }

    #[test]
    fn the_fast_taper_is_the_taper() {
        // The rasteriser calls a version that shares its logarithms with five
        // other terms. If the two ever disagree, every blade in the field
        // changes shape and no test that looks at one blade would notice.
        for profile in Profile::ALL {
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
    fn foreshortening_never_reaches_zero() {
        // A blade that vanishes is worse than one that never narrows.
        for i in 0..64 {
            for j in 0..64 {
                let (a, b) = (
                    i as f32 / 64.0 * std::f32::consts::TAU,
                    j as f32 / 64.0 * std::f32::consts::PI,
                );
                let normal = Vec3::new(b.sin() * a.cos(), b.sin() * a.sin(), b.cos());
                let width = foreshorten(normal);
                assert!(
                    (1.0 - FORESHORTEN - 1.0e-5..=1.0 + 1.0e-5).contains(&width),
                    "{width} at {a} {b}"
                );
            }
        }
        // Face-on to the camera is full width; edge-on is the floor.
        assert!((foreshorten(iso::TOWARD_VIEWER) - 1.0).abs() < 1.0e-5);
        assert!((foreshorten(iso::VIEW_RIGHT) - (1.0 - FORESHORTEN)).abs() < 1.0e-5);
    }

    #[test]
    fn an_unresolvable_fork_becomes_a_notch() {
        let fork = TipProfile::Forked {
            split_at: 0.8,
            opening: 0.2,
            long: 0.24,
            short: 0.14,
        };
        assert!(matches!(fork.resolved_at(1.0), TipProfile::Notched { .. }));
        assert_eq!(fork.resolved_at(12.0), fork);
        // A tip that was never a fork is left alone at any size.
        assert_eq!(TipProfile::Pointed.resolved_at(0.1), TipProfile::Pointed);
    }

    #[test]
    fn a_fork_declares_how_far_past_its_parent_it_reaches() {
        // The guard band reads this. A fork continues from the split rather than
        // replacing what is past it, so a forked blade is genuinely longer than
        // its own arc and a band sized for the arc alone clips the outer child.
        let fork = TipProfile::Forked {
            split_at: 0.85,
            opening: 0.3,
            long: 0.28,
            short: 0.12,
        };
        assert!((fork.extra_reach() - 0.13).abs() < 1.0e-5);
        assert_eq!(TipProfile::Pointed.extra_reach(), 0.0);
        // A fork that stays inside its parent asks for nothing.
        let tucked = TipProfile::Forked {
            split_at: 0.8,
            opening: 0.3,
            long: 0.15,
            short: 0.1,
        };
        assert_eq!(tucked.extra_reach(), 0.0);
    }
}

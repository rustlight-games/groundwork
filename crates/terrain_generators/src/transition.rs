//! Turning a smooth authored ramp into the boundary the eye expects.
//!
//! ## What the authored weights give you, and why it is not enough
//!
//! A `SmoothBand` over a spline distance produces a clean monotone ramp from
//! grass to dirt. Rendered directly, it reads as an airbrushed decal: the
//! boundary is a smooth curve at the band's own scale, and nothing in nature has
//! one. Look at `docs/references/grass_to_mud_transition.jpg` and the boundary
//! is broken into islands and peninsulas a few centimetres across, sitting
//! inside a band tens of centimetres wide.
//!
//! Those are two different scales and they are separately authored:
//!
//! - the **band** is where the ground is changing, and the document says so
//!   through a mask;
//! - the **raggedness** is how the change is realised inside that band, and it
//!   is a property of the pair of materials meeting there.
//!
//! ## The mechanism, and the useful thing that falls out of it
//!
//! Each material's score is perturbed by its own noise field before the scores
//! are normalised:
//!
//! ```text
//! w        = normalise(score)
//! contest_k = 4 · w_k · (1 − w_k)          // 1 where evenly split, 0 at either end
//! w_k'     = max(0, w_k + amplitude · (noise_k(p) − ½) · contest_k)
//! weights  = normalise(w')
//! ```
//!
//! Per material rather than one shared field, so the lobes of one interpenetrate
//! the lobes of another instead of the whole boundary wobbling as a unit.
//!
//! The **contest** term is what makes the extremes safe, and it replaced a plain
//! clamp that was wrong in two ways. Perturbing a lone material at weight one
//! can drive it to zero, leaving ground made of nothing; perturbing a material
//! at weight zero can conjure mud into the middle of a clean meadow. Scaling by
//! `4w(1−w)` removes both: the noise has full authority where two materials are
//! evenly matched and none at all where one already owns the ground.
//!
//! What falls out is the relationship between the two references, and it is why
//! this formulation was chosen over displacing the boundary curve directly. The
//! contour where two weights cross moves by roughly `amplitude / |∇score|`
//! *metres*. So a wide, gentle band gets big islands and a tight band gets a
//! crisp edge — from the **same** raggedness setting. Reference plate one is the
//! wide case and plate two the narrow one, and an author gets both by moving the
//! band rather than by retuning the noise.
//!
//! ## It has to be evaluated, not sampled
//!
//! The realisation is a pure function of a world point, and it is deliberately
//! *not* baked into the field stack. Two reasons, and the second is the one that
//! matters:
//!
//! 1. The lobes are finer than a sensible grid spacing. Baking them would cap
//!    the detail at the matrix resolution, and the matrix is sized for the
//!    macro fields.
//! 2. **Ownership and ground shading must consult the same answer.** A candidate
//!    asks "is this point grass?" and a texel asks the same question a
//!    millimetre away; if one reads a baked plane and the other evaluates the
//!    function, tufts sit slightly off the mud they are supposed to be standing
//!    beside. One function, called by both.
//!
//! ## Fully one thing, and fully the other
//!
//! At `w_k = 1` with every other weight zero, `contest_k` is zero and no
//! amplitude of noise makes the ground anything but material `k`. At `w_k = 0`
//! it is zero again and nothing conjures material `k` into being. So a document
//! that says "the middle of this track is bare" gets bare, one that says "no
//! dirt here" gets none, and the raggedness only ever acts where two materials
//! genuinely overlap — which is the property that lets an author reason about
//! the extremes.

use terrain_core::coords::WorldPoint;
use terrain_core::ids::MaterialIndex;
use terrain_core::sample::{MaterialWeightSet, WEIGHT_EPSILON};

use crate::rng::{Stream, fbm, scramble};

/// The version the transition solver stamps on itself.
pub const TRANSITION_VERSION: u32 = 1;

/// How a pair of materials realise the join between them.
///
/// Carried per document rather than per pair to begin with. A pair-specific
/// profile is a reasonable later refinement and a poor starting point: with one
/// profile an author tunes a look, and with a matrix of them an author tunes
/// `n²` looks and none of them is the one they were looking at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionProfile {
    /// How far a boundary wanders, in units of material score.
    ///
    /// Zero is the raw authored ramp. Around a third gives the interpenetrating
    /// islands of the reference plates. Above about one the boundary stops being
    /// a boundary and becomes a mottle, because the noise dominates the ramp
    /// everywhere rather than only where the ramp is level.
    pub raggedness: f32,
    /// The size of the largest lobes, in metres.
    pub feature_m: f32,
    /// How many octaves of detail the raggedness carries.
    ///
    /// Three is enough to stop the lobes reading as a single frequency. More
    /// costs a noise evaluation per octave per material per query, and this is
    /// called once per candidate and once per ground texel.
    pub octaves: u32,
}

impl Default for TransitionProfile {
    fn default() -> Self {
        Self {
            raggedness: 0.30,
            feature_m: 0.18,
            octaves: 3,
        }
    }
}

impl TransitionProfile {
    /// A boundary with no raggedness: exactly the authored ramp.
    pub const SMOOTH: Self = Self {
        raggedness: 0.0,
        feature_m: 0.2,
        octaves: 1,
    };

    pub fn is_well_formed(&self) -> bool {
        self.raggedness.is_finite()
            && self.raggedness >= 0.0
            && self.feature_m.is_finite()
            && self.feature_m > 0.0
    }
}

/// The realised substrate at a point.
///
/// Held as a small fixed array rather than a `Vec`, because this is produced
/// once per candidate and once per ground texel and an allocation there is the
/// whole cost. Ground made of more than this many materials at one point is not
/// a blend, it is a mistake — and the excess is dropped by weight, lowest first.
pub const MAX_REALISED_MATERIALS: usize = 4;

/// Substrate weights after the boundary has been realised.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RealisedSubstrate {
    entries: [(MaterialIndex, f32); MAX_REALISED_MATERIALS],
    count: u8,
}

impl RealisedSubstrate {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn iter(&self) -> impl Iterator<Item = (MaterialIndex, f32)> + '_ {
        self.entries[..self.count as usize].iter().copied()
    }

    /// One material's realised weight.
    pub fn weight_of(&self, material: MaterialIndex) -> f32 {
        self.iter()
            .find(|(index, _)| *index == material)
            .map(|(_, weight)| weight)
            .unwrap_or(0.0)
    }

    /// The material with the greatest weight, if any.
    pub fn dominant(&self) -> Option<(MaterialIndex, f32)> {
        self.iter().fold(
            None,
            |best: Option<(MaterialIndex, f32)>, entry| match best {
                Some(current) if current.1 >= entry.1 => Some(current),
                _ => Some(entry),
            },
        )
    }

    /// How mixed the ground is, `0..1`.
    ///
    /// The same mapping the point sampler and the field stack use, so all three
    /// agree about where a transition is.
    pub fn blend(&self) -> f32 {
        match self.dominant() {
            None => 0.0,
            Some((_, weight)) => ((1.0 - weight) * 2.0).clamp(0.0, 1.0),
        }
    }
}

/// Realise the boundary at a point.
///
/// `scores` are the authored weights — normalised or not; they are renormalised
/// here regardless, which is what lets a caller pass a raw
/// [`MaterialWeightSet`] or a slice read from field planes without thinking
/// about it.
pub fn realise(
    scores: impl IntoIterator<Item = (MaterialIndex, f32)>,
    at: WorldPoint,
    profile: &TransitionProfile,
    root_seed: u64,
) -> RealisedSubstrate {
    let mut entries = [(MaterialIndex(0), 0.0f32); MAX_REALISED_MATERIALS];
    let mut count = 0usize;

    for (material, score) in scores {
        if !score.is_finite() || score <= 0.0 {
            continue;
        }
        insert(&mut entries, &mut count, material, score);
    }

    // Normalised *before* the perturbation, because the contest term is defined
    // against a weight rather than a raw score. A caller passing scores of 8 and
    // 2 means the same ground as one passing 0.8 and 0.2, and the boundary must
    // ragged the same way for both.
    normalise(&mut entries, &mut count);

    if profile.raggedness > 0.0 && profile.is_well_formed() && count > 1 {
        let frequency = 1.0 / profile.feature_m;
        for entry in entries[..count].iter_mut() {
            let (material, weight) = *entry;
            // Zero at either extreme, one where two materials are evenly
            // matched. See the module note: without it, loud noise either
            // empties pure ground or conjures a material the author excluded.
            let contest = 4.0 * weight * (1.0 - weight);
            if contest <= 0.0 {
                continue;
            }
            // Each material gets its own noise field, keyed by its index, so
            // two materials' lobes interpenetrate rather than the whole
            // boundary translating as one.
            let seed =
                scramble(root_seed ^ ((material.0 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)));
            let value = fbm(
                seed,
                Stream::Boundary,
                at.u_m as f32 * frequency,
                at.v_m as f32 * frequency,
                profile.octaves.max(1),
            );
            entry.1 = (weight + profile.raggedness * (value - 0.5) * contest).max(0.0);
        }
        normalise(&mut entries, &mut count);
    }

    RealisedSubstrate {
        entries,
        count: count as u8,
    }
}

/// Realise from a sampled weight set.
pub fn realise_set(
    weights: &MaterialWeightSet,
    at: WorldPoint,
    profile: &TransitionProfile,
    root_seed: u64,
) -> RealisedSubstrate {
    realise(
        weights.iter().map(|w| (w.material, w.weight)),
        at,
        profile,
        root_seed,
    )
}

/// Keep the largest weights, dropping the smallest when the array is full.
fn insert(
    entries: &mut [(MaterialIndex, f32); MAX_REALISED_MATERIALS],
    count: &mut usize,
    material: MaterialIndex,
    score: f32,
) {
    if *count < MAX_REALISED_MATERIALS {
        entries[*count] = (material, score);
        *count += 1;
        return;
    }
    // Full: replace the weakest, but only if this one beats it. Dropping the
    // weakest rather than refusing the new one keeps the result independent of
    // the order the caller happened to supply.
    let (weakest, weight) =
        entries
            .iter()
            .enumerate()
            .fold((0usize, f32::INFINITY), |acc, (index, entry)| {
                if entry.1 < acc.1 {
                    (index, entry.1)
                } else {
                    acc
                }
            });
    if score > weight {
        entries[weakest] = (material, score);
    }
}

/// Normalise to one, prune below the epsilon, and order by material index.
///
/// The same three invariants [`MaterialWeightSet`] enforces, for the same
/// reasons: a consumer never has to ask whether to divide by the sum, a boundary
/// does not carry a long tail of materials at `1e-9`, and two realisations of
/// the same ground compare equal regardless of the order they were built in.
fn normalise(entries: &mut [(MaterialIndex, f32); MAX_REALISED_MATERIALS], count: &mut usize) {
    let total: f32 = entries[..*count].iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        *count = 0;
        return;
    }
    let mut kept = 0usize;
    for index in 0..*count {
        let weight = entries[index].1 / total;
        if weight >= WEIGHT_EPSILON {
            entries[kept] = (entries[index].0, weight);
            kept += 1;
        }
    }
    *count = kept;

    // Renormalise after the prune, so the survivors still sum to one.
    let total: f32 = entries[..*count].iter().map(|(_, weight)| weight).sum();
    if total > 0.0 {
        for entry in entries[..*count].iter_mut() {
            entry.1 /= total;
        }
    }
    entries[..*count].sort_by_key(|(material, _)| material.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u64 = 0x5a17_e33b_0c9d_2f14;

    fn profile() -> TransitionProfile {
        TransitionProfile::default()
    }

    fn at(u: f64, v: f64) -> WorldPoint {
        WorldPoint::new(u, v)
    }

    #[test]
    fn realised_weights_always_sum_to_one() {
        let profile = profile();
        for step in 0..64 {
            let t = step as f32 / 63.0;
            let point = at(step as f64 * 0.031, step as f64 * 0.017);
            let realised = realise(
                [(MaterialIndex(0), 1.0 - t), (MaterialIndex(1), t)],
                point,
                &profile,
                SEED,
            );
            if realised.is_empty() {
                continue;
            }
            let sum: f32 = realised.iter().map(|(_, weight)| weight).sum();
            assert!((sum - 1.0).abs() < 1.0e-5, "sum was {sum} at t={t}");
        }
    }

    #[test]
    fn pure_ground_stays_pure_however_loud_the_noise() {
        // The property that lets an author reason about the extremes: a track
        // whose middle is stated to be bare comes out bare.
        let loud = TransitionProfile {
            raggedness: 5.0,
            ..profile()
        };
        for step in 0..40 {
            let point = at(step as f64 * 0.07, step as f64 * -0.05);
            let realised = realise([(MaterialIndex(1), 1.0)], point, &loud, SEED);
            assert_eq!(realised.len(), 1);
            assert_eq!(realised.weight_of(MaterialIndex(1)), 1.0);
        }
    }

    #[test]
    fn raggedness_never_conjures_a_material_the_author_excluded() {
        // The other half of the contest term. Mud must not appear in the middle
        // of a clean meadow because the noise happened to peak there — an
        // author who wrote no dirt here gets no dirt here.
        let loud = TransitionProfile {
            raggedness: 5.0,
            ..profile()
        };
        for step in 0..60 {
            let point = at(step as f64 * 0.043, step as f64 * 0.061);
            let realised = realise(
                [(MaterialIndex(0), 1.0), (MaterialIndex(1), 0.0)],
                point,
                &loud,
                SEED,
            );
            assert_eq!(
                realised.weight_of(MaterialIndex(1)),
                0.0,
                "a material with no authored weight appeared at {point:?}"
            );
        }
    }

    #[test]
    fn a_smooth_profile_returns_the_authored_ramp_unchanged() {
        let realised = realise(
            [(MaterialIndex(0), 0.7), (MaterialIndex(1), 0.3)],
            at(1.23, -4.56),
            &TransitionProfile::SMOOTH,
            SEED,
        );
        assert!((realised.weight_of(MaterialIndex(0)) - 0.7).abs() < 1.0e-5);
        assert!((realised.weight_of(MaterialIndex(1)) - 0.3).abs() < 1.0e-5);
    }

    #[test]
    fn raggedness_moves_the_boundary_without_moving_the_band() {
        // Walk a straight line across a linear ramp and find where the
        // dominant material flips. With raggedness the crossing wanders; the
        // *average* crossing stays where the ramp put it.
        let profile = profile();
        let crossing = |v: f64, profile: &TransitionProfile| -> f64 {
            let mut last = 0.0;
            for step in 0..400 {
                let u = -1.0 + step as f64 * 0.005;
                // A ramp from all-of-0 to all-of-1 across u in [-0.5, 0.5].
                let t = ((u + 0.5) / 1.0).clamp(0.0, 1.0) as f32;
                let realised = realise(
                    [(MaterialIndex(0), 1.0 - t), (MaterialIndex(1), t)],
                    at(u, v),
                    profile,
                    SEED,
                );
                if let Some((material, _)) = realised.dominant() {
                    if material == MaterialIndex(1) {
                        return u;
                    }
                }
                last = u;
            }
            last
        };

        let smooth: Vec<f64> = (0..16)
            .map(|row| crossing(row as f64 * 0.13, &TransitionProfile::SMOOTH))
            .collect();
        let ragged: Vec<f64> = (0..16)
            .map(|row| crossing(row as f64 * 0.13, &profile))
            .collect();

        // The smooth ramp crosses at the same place on every row: a straight
        // line, which is exactly the decal look.
        let smooth_spread = smooth.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b))
            - smooth.iter().fold(f64::INFINITY, |a, b| a.min(*b));
        assert!(
            smooth_spread < 0.02,
            "a smooth ramp should cross in a straight line, spread {smooth_spread}"
        );

        // The ragged one does not.
        let ragged_spread = ragged.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b))
            - ragged.iter().fold(f64::INFINITY, |a, b| a.min(*b));
        assert!(
            ragged_spread > 0.05,
            "raggedness should break the boundary up, spread {ragged_spread}"
        );

        // And it stays a boundary rather than becoming a mottle: the mean
        // crossing is still near the middle of the band.
        let mean = ragged.iter().sum::<f64>() / ragged.len() as f64;
        assert!(
            mean.abs() < 0.15,
            "the boundary drifted off the band, mean crossing {mean}"
        );
    }

    #[test]
    fn a_wide_band_gets_wider_islands_than_a_narrow_one() {
        // The relationship between the two reference plates, asserted. The same
        // raggedness setting produces big islands on a gentle ramp and a crisp
        // edge on a steep one, because the contour moves by amplitude over
        // gradient.
        let profile = profile();
        let spread_for = |band_m: f64| -> f64 {
            let crossing = |v: f64| -> f64 {
                for step in 0..800 {
                    let u = -1.0 + step as f64 * 0.0025;
                    let t = ((u / band_m) + 0.5).clamp(0.0, 1.0) as f32;
                    let realised = realise(
                        [(MaterialIndex(0), 1.0 - t), (MaterialIndex(1), t)],
                        at(u, v),
                        &profile,
                        SEED,
                    );
                    if let Some((material, _)) = realised.dominant() {
                        if material == MaterialIndex(1) {
                            return u;
                        }
                    }
                }
                1.0
            };
            let values: Vec<f64> = (0..24).map(|row| crossing(row as f64 * 0.11)).collect();
            values.iter().fold(f64::NEG_INFINITY, |a, b| a.max(*b))
                - values.iter().fold(f64::INFINITY, |a, b| a.min(*b))
        };

        let wide = spread_for(1.2);
        let narrow = spread_for(0.1);
        assert!(
            wide > narrow * 2.0,
            "a wide band should ragged further than a narrow one: {wide} vs {narrow}"
        );
    }

    #[test]
    fn realisation_is_a_pure_function_of_the_point() {
        // Ownership and ground shading both call this, from different loops, at
        // nearly the same points. If it were not pure the tufts would sit off
        // the mud they stand beside.
        let profile = profile();
        let point = at(0.317, -1.902);
        let scores = [(MaterialIndex(0), 0.55), (MaterialIndex(2), 0.45)];
        let once = realise(scores, point, &profile, SEED);
        let again = realise(scores, point, &profile, SEED);
        assert_eq!(once, again);
        // And supplying the same scores in the other order is the same ground.
        let reversed = realise(
            [(MaterialIndex(2), 0.45), (MaterialIndex(0), 0.55)],
            point,
            &profile,
            SEED,
        );
        assert_eq!(once, reversed);
    }

    #[test]
    fn a_different_seed_is_a_different_boundary() {
        let profile = profile();
        let point = at(0.2, 0.2);
        let scores = [(MaterialIndex(0), 0.5), (MaterialIndex(1), 0.5)];
        let a = realise(scores, point, &profile, SEED);
        let b = realise(scores, point, &profile, SEED ^ 0xffff);
        assert_ne!(a, b);
    }

    #[test]
    fn more_materials_than_the_array_holds_keeps_the_largest() {
        let realised = realise(
            [
                (MaterialIndex(0), 0.40),
                (MaterialIndex(1), 0.30),
                (MaterialIndex(2), 0.20),
                (MaterialIndex(3), 0.09),
                (MaterialIndex(4), 0.01),
            ],
            at(0.0, 0.0),
            &TransitionProfile::SMOOTH,
            SEED,
        );
        assert!(realised.len() <= MAX_REALISED_MATERIALS);
        // The smallest was dropped, the largest kept, and it still sums to one.
        assert!(realised.weight_of(MaterialIndex(0)) > 0.0);
        assert_eq!(realised.weight_of(MaterialIndex(4)), 0.0);
        let sum: f32 = realised.iter().map(|(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1.0e-5);
    }
}

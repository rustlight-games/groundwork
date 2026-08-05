//! Which recipe gets a candidate, when several want it.
//!
//! ## One candidate, one owner
//!
//! This is the second half of the mechanism in [`crate::domain`]. The domain
//! decides *whether* something grows at a point; this decides *what*. Keeping
//! them apart is what stops a transition doubling its density, because the count
//! of marks is fixed by acceptance before any material is consulted — a
//! 70/30 boundary emits the same number of things as the pure ground on either
//! side, and only their identity changes.
//!
//! Getting this wrong is the characteristic terrain-blend failure and it is
//! worth naming precisely. Two populations each scattering at their own weight
//! put down 0.7 of one and 0.3 of the other and the *positions* do not overlap,
//! so the boundary carries 1.0 of grass positions plus 1.0 of dirt positions
//! thinned to 0.3 — visibly busier than either side, in a stripe that follows
//! the join.
//!
//! ## The score, and why it is a product
//!
//! ```text
//! owner_score_k = substrate_affinity_k · abundance_k · profile_weight_k · boundary_k
//! ```
//!
//! A product rather than a sum, because every term is a veto. A recipe with no
//! affinity for the ground under it should get the candidate *never*, not
//! rarely — and a sum lets a large abundance drown a zero affinity, which is how
//! grass ends up growing on bare rock at low density instead of not at all.
//!
//! ## The draw is categorical, and it is the candidate's own
//!
//! One value in `0..1`, addressed to the candidate on the `owner` stream, walked
//! against the normalised scores in owner order. So ownership is stable under a
//! rebuild, independent of the order recipes were registered in, and independent
//! of which region was generated.
//!
//! Ownership *does* move when a neighbouring owner's score changes, and that is
//! inherent to a categorical draw rather than a defect: the alternative is a
//! per-owner threshold, which either leaves candidates unowned or gives one to
//! two recipes. What is preserved — and what matters for popping — is that the
//! candidate's **position and latent attributes do not move** when ownership
//! changes hands. A boundary nudged by a centimetre reassigns a handful of marks
//! and leaves every other one exactly where it was.

// See `terrain_scene::field`: `!(x > 0.0)` is true for NaN and `x <= 0.0` is
// not, and this is a guard whose job is to catch one.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use terrain_core::ids::StreamKey;
use terrain_core::seed::{RandomAddress, SeedContext};

use crate::domain::DomainCandidate;

/// One recipe's claim on a candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnerOption {
    /// Which population this is, as an index into the compiler's own list.
    ///
    /// An index rather than a key, because this is walked once per candidate and
    /// there can be a million of them. The compiler owns the table.
    pub owner: u16,
    /// How strongly it wants this candidate. Non-negative; zero is a refusal.
    pub score: f32,
}

/// Choose one owner, or none.
///
/// `options` may arrive in any order — it is sorted by owner index here, so two
/// callers building the list differently get the same answer. Returns `None`
/// when nothing wants the candidate, which is a normal outcome rather than an
/// error: bare ground is ground where every recipe scored zero.
pub fn assign(
    candidate: &DomainCandidate,
    options: &mut [OwnerOption],
    seeds: &SeedContext,
) -> Option<u16> {
    options.sort_by_key(|option| option.owner);

    let total: f32 = options
        .iter()
        .map(|option| option.score.max(0.0))
        .filter(|score| score.is_finite())
        .sum();
    if !(total > 0.0) {
        return None;
    }

    let stream = StreamKey::new("owner").expect("valid");
    let draw = seeds.unit(&RandomAddress::new(candidate.id, &stream)) as f32 * total;

    let mut accumulated = 0.0f32;
    for option in options.iter() {
        let score = option.score.max(0.0);
        if !score.is_finite() || score <= 0.0 {
            continue;
        }
        accumulated += score;
        if draw < accumulated {
            return Some(option.owner);
        }
    }
    // Only reachable through floating-point drift, when the accumulated sum
    // falls a hair short of the total. The last positive option is the honest
    // answer rather than `None`, which would leave a hole in otherwise solid
    // ground.
    options
        .iter()
        .rev()
        .find(|option| option.score > 0.0 && option.score.is_finite())
        .map(|option| option.owner)
}

/// The product form of an owner's score.
///
/// Written once so that four recipes do not each decide which terms multiply and
/// which add. Every argument is clamped non-negative, because a negative term
/// would flip the sign of the product and turn a refusal into an enthusiastic
/// claim.
pub fn score(substrate_affinity: f32, abundance: f32, profile_weight: f32, boundary: f32) -> f32 {
    let clamp = |value: f32| {
        if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        }
    };
    clamp(substrate_affinity) * clamp(abundance) * clamp(profile_weight) * clamp(boundary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::coords::{CellCoord, WorldPoint};
    use terrain_core::seed::{CandidateId, PopulationHash, RootSeed};

    fn seeds() -> SeedContext {
        SeedContext::new(RootSeed::new(0x0c9d_2f14_5a17_e33b), 1)
    }

    fn candidate(rank: u16) -> DomainCandidate {
        DomainCandidate {
            id: CandidateId::new(
                PopulationHash::from_bits(0xabcd_ef01),
                CellCoord::new(3, -7),
                rank,
            ),
            position: WorldPoint::new(0.5, 0.25),
            priority: 0.5,
            footprint_radius_m: 0.0,
        }
    }

    #[test]
    fn nothing_wanting_a_candidate_leaves_it_unowned() {
        let mut options = [
            OwnerOption {
                owner: 0,
                score: 0.0,
            },
            OwnerOption {
                owner: 1,
                score: 0.0,
            },
        ];
        assert_eq!(assign(&candidate(0), &mut options, &seeds()), None);
    }

    #[test]
    fn a_sole_claimant_always_wins() {
        for rank in 0..64 {
            let mut options = [OwnerOption {
                owner: 3,
                score: 0.01,
            }];
            assert_eq!(assign(&candidate(rank), &mut options, &seeds()), Some(3));
        }
    }

    #[test]
    fn a_zero_score_never_wins_however_large_the_others_are() {
        // The reason the score is a product: a refusal has to be absolute, or
        // grass grows on bare rock at low density instead of not at all.
        for rank in 0..200 {
            let mut options = [
                OwnerOption {
                    owner: 0,
                    score: 0.0,
                },
                OwnerOption {
                    owner: 1,
                    score: 5.0,
                },
            ];
            assert_eq!(assign(&candidate(rank), &mut options, &seeds()), Some(1));
        }
    }

    #[test]
    fn ownership_follows_the_scores_over_many_candidates() {
        // A 3:1 split should come out near three quarters, or the boundary is
        // biased and no author could reason about it.
        let seeds = seeds();
        let mut first = 0usize;
        let total = 4000;
        for rank in 0..total {
            let mut options = [
                OwnerOption {
                    owner: 0,
                    score: 0.75,
                },
                OwnerOption {
                    owner: 1,
                    score: 0.25,
                },
            ];
            if assign(&candidate(rank as u16), &mut options, &seeds) == Some(0) {
                first += 1;
            }
        }
        let share = first as f64 / total as f64;
        assert!(
            (share - 0.75).abs() < 0.03,
            "a three-to-one split came out at {share}"
        );
    }

    #[test]
    fn the_order_options_are_supplied_in_does_not_change_the_owner() {
        // Two callers building the list differently must agree, or ownership
        // depends on registration order.
        let seeds = seeds();
        for rank in 0..200 {
            let mut forward = [
                OwnerOption {
                    owner: 0,
                    score: 0.4,
                },
                OwnerOption {
                    owner: 1,
                    score: 0.35,
                },
                OwnerOption {
                    owner: 2,
                    score: 0.25,
                },
            ];
            let mut backward = [
                OwnerOption {
                    owner: 2,
                    score: 0.25,
                },
                OwnerOption {
                    owner: 1,
                    score: 0.35,
                },
                OwnerOption {
                    owner: 0,
                    score: 0.4,
                },
            ];
            assert_eq!(
                assign(&candidate(rank), &mut forward, &seeds),
                assign(&candidate(rank), &mut backward, &seeds)
            );
        }
    }

    #[test]
    fn a_candidate_has_the_same_owner_every_time_it_is_asked() {
        let seeds = seeds();
        let build = || {
            [
                OwnerOption {
                    owner: 0,
                    score: 0.5,
                },
                OwnerOption {
                    owner: 1,
                    score: 0.5,
                },
            ]
        };
        for rank in 0..100 {
            let (mut a, mut b) = (build(), build());
            assert_eq!(
                assign(&candidate(rank), &mut a, &seeds),
                assign(&candidate(rank), &mut b, &seeds)
            );
        }
    }

    #[test]
    fn the_score_is_a_veto_in_every_term() {
        assert_eq!(score(0.0, 9.0, 9.0, 9.0), 0.0);
        assert_eq!(score(9.0, 0.0, 9.0, 9.0), 0.0);
        assert_eq!(score(9.0, 9.0, 0.0, 9.0), 0.0);
        assert_eq!(score(9.0, 9.0, 9.0, 0.0), 0.0);
        // And a negative or non-finite term is a refusal rather than a sign flip.
        assert_eq!(score(-1.0, 1.0, 1.0, 1.0), 0.0);
        assert_eq!(score(f32::NAN, 1.0, 1.0, 1.0), 0.0);
        assert!((score(0.5, 2.0, 1.0, 1.0) - 1.0).abs() < 1.0e-6);
    }
}

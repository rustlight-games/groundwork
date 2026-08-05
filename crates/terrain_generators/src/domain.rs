//! Shared candidate domains: one lattice of potential content, many owners.
//!
//! ## The problem this exists to solve
//!
//! Ask two populations to fill the same ground and a transition gets both of
//! them. Grass at 70% emits a full grass population scaled to 0.7, dirt detail
//! at 30% emits a full one scaled to 0.3, and where they meet there are *more*
//! marks than on either side — a busy stripe down the boundary, which is the
//! most recognisable failure a terrain blend has. Scaling the two by weight does
//! not fix it, because two independent scatters at 0.7 and 0.3 still put down
//! 1.0 of *positions* and the clumps of one interleave with the clumps of the
//! other.
//!
//! So the candidate field is shared. One lattice, one acceptance decision, and
//! then a separate draw deciding *which recipe* gets each accepted candidate.
//! A transition emits one mark per accepted candidate, exactly as the pure
//! ground on either side does, and the only thing that changes across the
//! boundary is which recipe drew it.
//!
//! ## Capacity is fixed; density is an acceptance probability
//!
//! The tempting design is to size the lattice from the density the author asked
//! for. It is wrong, and the reason is worth stating because the symptom is
//! confusing: changing a density then changes the *cell size*, which changes
//! every candidate's cell and rank, which changes every candidate's address, and
//! the whole meadow is redrawn. An author nudging density from 400 to 420 sees
//! every blade move.
//!
//! Instead a domain declares a fixed capacity — a cell size and a number of
//! candidates per cell — and a target density becomes a threshold:
//!
//! ```text
//! p_accept = target_density / max_density
//! accept if unit(candidate, "accept") < p_accept
//! ```
//!
//! The draw belongs to the candidate, so raising the density can only *add*
//! candidates and lowering it can only remove them. Survivors never move. That
//! is what makes a density field paintable, and it is what makes a boundary
//! moveable without the terrain popping.
//!
//! ## Conflict thinning is by stated priority, not by order
//!
//! Pure jitter clumps. For tuft anchors, stones and flowers the clumping is the
//! wrong kind — real ones exclude each other — so a candidate is dropped when a
//! higher-priority neighbour lies inside its exclusion radius.
//!
//! The test is deliberately **non-recursive**: a candidate is compared against
//! every neighbour, not against the neighbours that *survived*. A recursive test
//! gives a slightly better packing and is order-dependent to compute, which
//! would make the result depend on which region was walked first — and two
//! neighbouring plates would then disagree along their join, which is the one
//! failure this framework refuses to accept. Thinning slightly harder is the
//! cheaper mistake by a wide margin.
//!
//! For that to hold across regions, the neighbourhood query has to see
//! candidates that lie outside the region being generated. [`generate`] expands
//! its own working area by the domain's maximum exclusion radius before
//! thinning, so a candidate near the edge is judged against the same neighbours
//! whichever window asked for it.

use terrain_core::coords::{CellCoord, CellGrid, WorldPoint, WorldRect};
use terrain_core::ids::{DomainKey, StreamKey};
use terrain_core::seed::{CandidateId, PopulationHash, RandomAddress, SeedContext};

/// The version the domain lattice and its acceptance stamp on themselves.
///
/// Separate from any recipe's version: a change here moves every candidate in
/// every domain, and a change to a recipe moves only what that recipe drew.
/// Conflating them would make a grass tweak invalidate the stones.
pub const DOMAIN_ALGORITHM_VERSION: u32 = 2;

/// How a domain spaces its content.
///
/// ## Two exclusion policies, and why they are not one
///
/// The old `Exclusion { max_radius_m }` was documented as a per-candidate
/// footprint radius and implemented as a fixed centre-to-centre distance. Those
/// are different quantities: two objects of radius `r` do not overlap when their
/// centres are `2r` apart, so reading the old number as a footprint radius would
/// have silently *halved* the spacing of every domain that used it.
///
/// So there are two policies. `PriorityDistance` preserves the old meaning
/// exactly, and everything that already used it keeps it. `PriorityFootprints`
/// is the new one: each candidate carries its own physical radius and a pair
/// conflicts when their disks overlap after clearance. Stones use it, because a
/// stone's exclusion radius *is* its footprint; grass does not, because a tuft's
/// exclusion radius is about root competition rather than about the clump's
/// physical extent.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum SpacingPolicy {
    /// Jittered within the cell, and nothing more.
    ///
    /// What high-density filler wants. Grit and fine grass have no business
    /// excluding each other, and the thinning pass is the expensive part.
    Jittered,
    /// No two survivors closer than a fixed centre-to-centre distance.
    PriorityDistance { minimum_centre_distance_m: f64 },
    /// No two survivors whose physical footprints overlap after clearance.
    ///
    /// `‖xᵢ − xⱼ‖ < rᵢ + rⱼ + c`. A **sum** of radii, because the radii are
    /// occupied object disks rather than desired sample spacings — the variable
    /// radius literature also offers max, min, prior-point and current-point
    /// rules, and the sum is the one with a sphere-packing meaning.
    ///
    /// Symmetric, so permuting the generation order cannot change whether a pair
    /// conflicts.
    PriorityFootprints {
        radius: CandidateRadiusPolicy,
        clearance_m: f64,
    },
}

/// How big a candidate's own footprint is.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum CandidateRadiusPolicy {
    Fixed {
        radius_m: f64,
    },
    /// Drawn from the candidate's own address, in a named stream.
    ///
    /// Addressed rather than drawn sequentially, for the reason everything else
    /// here is: a candidate's radius has to be the same whichever window
    /// computed it, or two neighbouring plates thin their shared boundary
    /// differently.
    Uniform {
        min_m: f64,
        max_m: f64,
    },
}

impl CandidateRadiusPolicy {
    /// The largest radius this policy can produce.
    ///
    /// What the halo is sized from, so it has to be a genuine bound rather than
    /// a typical value.
    pub fn maximum_m(&self) -> f64 {
        match self {
            Self::Fixed { radius_m } => radius_m.max(0.0),
            Self::Uniform { min_m, max_m } => min_m.max(*max_m).max(0.0),
        }
    }

    /// One candidate's radius, from its own address.
    pub fn radius_for(&self, candidate_id: CandidateId, seeds: &SeedContext) -> f64 {
        match self {
            Self::Fixed { radius_m } => radius_m.max(0.0),
            Self::Uniform { min_m, max_m } => {
                let low = min_m.min(*max_m).max(0.0);
                let high = min_m.max(*max_m).max(0.0);
                let unit = seeds.unit(&RandomAddress::new(candidate_id, &stream("footprint")));
                low + (high - low) * unit
            }
        }
    }
}

impl SpacingPolicy {
    /// How far outside a region the thinning pass has to look.
    ///
    /// The proof this has to satisfy: for a candidate `i` inside the requested
    /// bounds, every candidate that *can* conflict with it lies within
    /// `rᵢ + Rmax + c`, which is at most `2·Rmax + c`. Expanding the working
    /// area by that means every window containing `i` compares it against the
    /// same complete conflict set and reaches the same decision.
    pub fn conflict_reach_m(&self) -> f64 {
        match self {
            Self::Jittered => 0.0,
            Self::PriorityDistance {
                minimum_centre_distance_m,
            } => minimum_centre_distance_m.max(0.0),
            Self::PriorityFootprints {
                radius,
                clearance_m,
            } => 2.0 * radius.maximum_m() + clearance_m.max(0.0),
        }
    }

    /// The largest footprint a candidate in this domain can carry.
    pub fn maximum_radius_m(&self) -> f64 {
        match self {
            Self::Jittered | Self::PriorityDistance { .. } => 0.0,
            Self::PriorityFootprints { radius, .. } => radius.maximum_m(),
        }
    }
}

/// A domain: a named lattice of potential content, with a fixed capacity.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateDomainDef {
    pub key: DomainKey,
    /// Side of one addressing cell, metres.
    pub cell_m: f64,
    /// How many candidates each cell offers.
    ///
    /// More than one, always. A lattice offering exactly one candidate per cell
    /// shows its own grid however hard the position is jittered, because the
    /// *count* is uniform even when the placement is not — and the eye finds a
    /// uniform count faster than it finds a uniform position.
    pub candidates_per_cell: u16,
    pub spacing: SpacingPolicy,
}

impl CandidateDomainDef {
    /// The most content this domain can put down, per square metre.
    ///
    /// The denominator of every acceptance probability, so an author asking for
    /// more than this gets the lattice saturated rather than silently rounded —
    /// which validation reports rather than leaving to be discovered in a
    /// picture.
    pub fn max_density_per_m2(&self) -> f64 {
        if self.cell_m <= 0.0 {
            return 0.0;
        }
        self.candidates_per_cell as f64 / (self.cell_m * self.cell_m)
    }

    /// The stable hash every candidate in this domain is addressed under.
    ///
    /// Mixed with a domain tag so that a domain and a population sharing a name
    /// cannot share an address. They are different lattices and an accidental
    /// collision between them would be invisible: the content would simply be
    /// correlated in a way nobody asked for.
    pub fn hash(&self) -> PopulationHash {
        // An arbitrary constant, and its only job is to be different from the
        // nothing a population hash mixes in.
        const DOMAIN_TAG: u64 = 0xD0_A1_11_5E_ED_C0_DE_01;
        PopulationHash::from_bits(terrain_core::seed::mix(self.key.seed_hash() ^ DOMAIN_TAG))
    }

    pub fn is_well_formed(&self) -> bool {
        self.cell_m.is_finite() && self.cell_m > 0.0 && self.candidates_per_cell > 0
    }
}

/// One potential piece of content, before anything decides what it is.
///
/// The identity exists whether or not it is accepted and whether or not any
/// recipe wants it, which is the property everything else rests on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomainCandidate {
    pub id: CandidateId,
    /// Where it sits, jittered inside its own cell.
    pub position: WorldPoint,
    /// Its stable place in the conflict order, `0..1`.
    ///
    /// Addressed rather than drawn, so a candidate's priority is the same
    /// whichever region computed it — without which two neighbouring plates
    /// would thin differently along their join.
    pub priority: f32,
    /// How much ground this candidate physically occupies, metres.
    ///
    /// Belongs to the *shared domain* rather than to whichever recipe ends up
    /// owning it, and that ordering is load-bearing: acceptance happens before
    /// ownership, so every claimant sharing a lattice has to agree about how
    /// much room each candidate takes. Zero under the policies that space by
    /// centre distance rather than by footprint.
    pub footprint_radius_m: f64,
}

impl DomainCandidate {
    /// The strict total order two candidates are compared in.
    ///
    /// Priority first, then the complete candidate address. Rank alone is not
    /// enough — different cells share ranks, so two candidates in adjacent cells
    /// with equal priority would compare equal and both survive or both die
    /// depending on which was walked first. Exact 32-bit priority collisions are
    /// rare; determinism contracts are not probabilistic.
    ///
    /// The field order is pinned by [`DOMAIN_ALGORITHM_VERSION`].
    fn priority_key(&self) -> (u32, u64, i64, i64, u16) {
        (
            // Canonical bits: `-0.0` and `0.0` must not order differently, and a
            // NaN priority must not make the order intransitive.
            if self.priority.is_nan() {
                0
            } else if self.priority == 0.0 {
                0.0f32.to_bits()
            } else {
                self.priority.to_bits()
            },
            self.id.population.bits(),
            self.id.cell.y,
            self.id.cell.x,
            self.id.rank,
        )
    }

    /// A named random value in `0..1` belonging to this candidate.
    ///
    /// Every latent attribute a recipe wants — azimuth, scale, maturity, hue
    /// drift, bend, cluster phase — comes through here rather than being stored,
    /// for two reasons. It costs nothing for a candidate nobody grows, and
    /// adding a new attribute is a new stream name rather than a new field,
    /// which means it cannot disturb the values already drawn.
    pub fn latent(&self, seeds: &SeedContext, stream: &StreamKey) -> f32 {
        seeds.unit(&RandomAddress::new(self.id, stream)) as f32
    }

    /// A named random value in a range.
    pub fn latent_range(
        &self,
        seeds: &SeedContext,
        stream: &StreamKey,
        low: f32,
        high: f32,
    ) -> f32 {
        low + (high - low) * self.latent(seeds, stream)
    }
}

/// What a caller wants from one domain.
pub struct DomainRequest<'a> {
    pub definition: &'a CandidateDomainDef,
    /// The ground candidates should be returned for, halo included.
    pub bounds: WorldRect,
    pub seeds: SeedContext,
}

/// Stream names this module draws on.
///
/// Named constants rather than literals at the call sites, because a typo in one
/// of them is a silent correlation between two decisions rather than an error.
fn stream(name: &str) -> StreamKey {
    StreamKey::new(name).expect("domain stream names are valid by construction")
}

/// Generate, accept-agnostic, and thin one domain over a region.
///
/// Returns every candidate that survives conflict thinning, in cell-then-rank
/// order. Acceptance against a density field is a *separate* step — see
/// [`accepts`] — because the two answer different questions and the caller needs
/// the ordering between them: a candidate rejected for density still occupies
/// space against its neighbours, or lowering a density would let stones creep
/// toward each other.
pub fn generate(request: &DomainRequest<'_>) -> Vec<DomainCandidate> {
    let definition = request.definition;
    if !definition.is_well_formed() || request.bounds.is_empty() {
        return Vec::new();
    }

    let reach = definition.spacing.conflict_reach_m();
    // Judged against the same neighbours whichever window asked, so a candidate
    // on the edge of one plate and in the middle of the next survives or dies
    // identically. See `SpacingPolicy::conflict_reach_m` for the bound.
    let working = request.bounds.expanded(reach);
    let all = lay_out(definition, working, &request.seeds);

    match definition.spacing {
        SpacingPolicy::Jittered => all
            .into_iter()
            .filter(|candidate| request.bounds.contains(candidate.position))
            .collect(),
        SpacingPolicy::PriorityDistance {
            minimum_centre_distance_m,
        } => thin(all, request.bounds, |a, b| {
            let limit = minimum_centre_distance_m;
            squared_distance(a, b) < limit * limit
        }),
        SpacingPolicy::PriorityFootprints {
            radius,
            clearance_m,
        } => {
            let bucket = radius.maximum_m().max(1.0e-6);
            let clearance = clearance_m.max(0.0);
            thin_bucketed(all, request.bounds, bucket, clearance, |a, b| {
                let limit = a.footprint_radius_m + b.footprint_radius_m + clearance;
                squared_distance(a, b) < limit * limit
            })
        }
    }
}

/// Squared centre-to-centre distance between two candidates.
fn squared_distance(a: &DomainCandidate, b: &DomainCandidate) -> f64 {
    let du = b.position.u_m - a.position.u_m;
    let dv = b.position.v_m - a.position.v_m;
    du * du + dv * dv
}

/// Every candidate the lattice offers over a rectangle, unthinned.
fn lay_out(
    definition: &CandidateDomainDef,
    bounds: WorldRect,
    seeds: &SeedContext,
) -> Vec<DomainCandidate> {
    let domain = definition.hash();
    let grid = CellGrid::new(definition.cell_m);
    let jitter_u = stream("candidate_u");
    let jitter_v = stream("candidate_v");
    let priority = stream("priority");
    let radius_policy = match definition.spacing {
        SpacingPolicy::PriorityFootprints { radius, .. } => Some(radius),
        _ => None,
    };

    let mut out = Vec::new();
    for cell in grid.cells_over(bounds) {
        let rect = grid.cell_rect(cell);
        for rank in 0..definition.candidates_per_cell {
            let id = CandidateId::new(domain, cell, rank);
            let u = seeds.unit(&RandomAddress::new(id, &jitter_u));
            let v = seeds.unit(&RandomAddress::new(id, &jitter_v));
            out.push(DomainCandidate {
                id,
                position: WorldPoint::new(
                    rect.min.u_m + u * rect.width_m(),
                    rect.min.v_m + v * rect.height_m(),
                ),
                priority: seeds.unit(&RandomAddress::new(id, &priority)) as f32,
                footprint_radius_m: radius_policy
                    .map(|policy| policy.radius_for(id, seeds))
                    .unwrap_or(0.0),
            });
        }
    }
    out
}

/// Drop every candidate that a higher-priority neighbour excludes.
///
/// Non-recursive: a candidate is compared against every *raw proposal* in its
/// conflict neighbourhood, not against the ones that survived. A recursive rule
/// packs slightly better and needs a dependency traversal whose boundary can
/// extend unpredictably toward the window edge, which is exactly what a finite
/// halo cannot guarantee. Thinning slightly harder is the cheaper mistake by a
/// wide margin.
fn thin(
    all: Vec<DomainCandidate>,
    keep: WorldRect,
    conflicts: impl Fn(&DomainCandidate, &DomainCandidate) -> bool,
) -> Vec<DomainCandidate> {
    all.iter()
        .filter(|candidate| keep.contains(candidate.position))
        .filter(|candidate| {
            !all.iter().any(|rival| {
                !std::ptr::eq(*candidate, rival)
                    && rival.priority_key() > candidate.priority_key()
                    && conflicts(candidate, rival)
            })
        })
        .copied()
        .collect()
}

/// The same rule, with a bucket index so the neighbour query is bounded.
///
/// The bucket side is the largest footprint, so a candidate of radius `rᵢ` has
/// to inspect `⌈(rᵢ + Rmax + c)/b⌉` buckets in every direction. For a
/// fixed-radius domain with no clearance that is two, giving a five-by-five
/// neighbourhood — where the old fixed-distance rule needed three-by-three,
/// because it treated its one number as the whole centre-to-centre inhibition
/// distance rather than as a physical radius.
fn thin_bucketed(
    all: Vec<DomainCandidate>,
    keep: WorldRect,
    bucket_m: f64,
    clearance_m: f64,
    conflicts: impl Fn(&DomainCandidate, &DomainCandidate) -> bool,
) -> Vec<DomainCandidate> {
    let buckets = CellGrid::new(bucket_m);
    let mut index: std::collections::BTreeMap<CellCoord, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (slot, candidate) in all.iter().enumerate() {
        index
            .entry(buckets.cell_at(candidate.position))
            .or_default()
            .push(slot);
    }

    let max_radius = all
        .iter()
        .map(|c| c.footprint_radius_m)
        .fold(0.0f64, f64::max);

    let mut kept = Vec::new();
    for (slot, candidate) in all.iter().enumerate() {
        if !keep.contains(candidate.position) {
            continue;
        }
        let search = candidate.footprint_radius_m + max_radius + clearance_m;
        let cells = ((search / bucket_m).ceil() as i64).max(1);
        let home = buckets.cell_at(candidate.position);
        let mut excluded = false;
        'search: for dy in -cells..=cells {
            for dx in -cells..=cells {
                let Some(neighbours) = index.get(&home.offset(dx, dy)) else {
                    continue;
                };
                for other in neighbours {
                    if *other == slot {
                        continue;
                    }
                    let rival = &all[*other];
                    if rival.priority_key() <= candidate.priority_key() {
                        continue;
                    }
                    if conflicts(candidate, rival) {
                        excluded = true;
                        break 'search;
                    }
                }
            }
        }
        if !excluded {
            kept.push(*candidate);
        }
    }
    kept
}

/// Whether a candidate is accepted at a local target density.
///
/// The draw belongs to the candidate, so this is monotone in `target_per_m2`:
/// raising a density can only add candidates, and every one already accepted
/// stays exactly where it was.
pub fn accepts(
    candidate: &DomainCandidate,
    definition: &CandidateDomainDef,
    seeds: &SeedContext,
    target_per_m2: f64,
) -> bool {
    let capacity = definition.max_density_per_m2();
    if capacity <= 0.0 || !target_per_m2.is_finite() || target_per_m2 <= 0.0 {
        return false;
    }
    let probability = (target_per_m2 / capacity).clamp(0.0, 1.0);
    // `<` rather than `<=`, so a probability of exactly zero accepts nothing
    // even for a candidate whose draw came out at zero.
    (seeds.unit(&RandomAddress::new(candidate.id, &stream("accept")))) < probability
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::seed::RootSeed;

    fn domain(cell_m: f64, per_cell: u16, spacing: SpacingPolicy) -> CandidateDomainDef {
        CandidateDomainDef {
            key: DomainKey::new("vegetation.tuft_anchor").expect("valid"),
            cell_m,
            candidates_per_cell: per_cell,
            spacing,
        }
    }

    fn seeds() -> SeedContext {
        SeedContext::new(
            RootSeed::new(0x5a17_e33b_0c9d_2f14),
            DOMAIN_ALGORITHM_VERSION,
        )
    }

    fn rect(min: (f64, f64), max: (f64, f64)) -> WorldRect {
        WorldRect::new(WorldPoint::new(min.0, min.1), WorldPoint::new(max.0, max.1))
    }

    #[test]
    fn capacity_is_what_the_lattice_can_hold() {
        let d = domain(0.2, 8, SpacingPolicy::Jittered);
        // Eight per four-hundredth of a square metre is two hundred a metre.
        assert!((d.max_density_per_m2() - 200.0).abs() < 1.0e-9);
    }

    #[test]
    fn lowering_a_density_removes_candidates_without_moving_the_survivors() {
        // The property the whole fixed-capacity design exists for, and the one
        // an author feels immediately: nudging a density must not redraw the
        // meadow.
        let d = domain(0.2, 8, SpacingPolicy::Jittered);
        let seeds = seeds();
        let candidates = generate(&DomainRequest {
            definition: &d,
            bounds: rect((0.0, 0.0), (4.0, 4.0)),
            seeds,
        });

        let dense: Vec<_> = candidates
            .iter()
            .filter(|c| accepts(c, &d, &seeds, 120.0))
            .collect();
        let sparse: Vec<_> = candidates
            .iter()
            .filter(|c| accepts(c, &d, &seeds, 40.0))
            .collect();

        assert!(!sparse.is_empty() && sparse.len() < dense.len());
        // Every survivor of the thinner field is present in the denser one, at
        // the identical position. A subset, not a resample.
        for candidate in &sparse {
            let twin = dense
                .iter()
                .find(|other| other.id == candidate.id)
                .expect("a candidate accepted at low density must survive at high");
            assert_eq!(twin.position, candidate.position);
        }
    }

    #[test]
    fn density_zero_accepts_nothing_and_saturation_accepts_everything() {
        let d = domain(0.25, 4, SpacingPolicy::Jittered);
        let seeds = seeds();
        let candidates = generate(&DomainRequest {
            definition: &d,
            bounds: rect((0.0, 0.0), (2.0, 2.0)),
            seeds,
        });
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|c| !accepts(c, &d, &seeds, 0.0)));
        // Asked for more than the lattice holds, every candidate is taken —
        // saturated rather than silently scaled.
        let capacity = d.max_density_per_m2();
        assert!(
            candidates
                .iter()
                .all(|c| accepts(c, &d, &seeds, capacity * 2.0))
        );
    }

    #[test]
    fn thinning_keeps_no_two_candidates_closer_than_the_exclusion_radius() {
        let radius = 0.15;
        let d = domain(
            0.1,
            4,
            SpacingPolicy::PriorityDistance {
                minimum_centre_distance_m: radius,
            },
        );
        let kept = generate(&DomainRequest {
            definition: &d,
            bounds: rect((0.0, 0.0), (2.0, 2.0)),
            seeds: seeds(),
        });
        assert!(kept.len() > 10, "the region should still hold content");
        for (index, a) in kept.iter().enumerate() {
            for b in &kept[index + 1..] {
                let distance = a.position.distance(b.position);
                assert!(
                    distance >= radius - 1.0e-9,
                    "two survivors are {distance} apart, closer than {radius}"
                );
            }
        }
    }

    #[test]
    fn thinning_agrees_across_a_join() {
        // The reason the test is non-recursive and the working area is grown.
        // A candidate near the edge of one window is in the middle of another,
        // and it must live or die the same way in both.
        let d = domain(
            0.1,
            4,
            SpacingPolicy::PriorityDistance {
                minimum_centre_distance_m: 0.15,
            },
        );
        let seeds = seeds();

        let whole = generate(&DomainRequest {
            definition: &d,
            bounds: rect((0.0, 0.0), (4.0, 2.0)),
            seeds,
        });
        let left = generate(&DomainRequest {
            definition: &d,
            bounds: rect((0.0, 0.0), (2.0, 2.0)),
            seeds,
        });
        let right = generate(&DomainRequest {
            definition: &d,
            bounds: rect((2.0, 0.0), (4.0, 2.0)),
            seeds,
        });

        let mut halves: Vec<_> = left.iter().chain(right.iter()).map(|c| c.id).collect();
        halves.sort();
        let mut together: Vec<_> = whole.iter().map(|c| c.id).collect();
        together.sort();
        assert_eq!(
            together, halves,
            "the join produced different content from the whole"
        );
    }

    #[test]
    fn generating_twice_gives_the_same_candidates() {
        let d = domain(
            0.15,
            6,
            SpacingPolicy::PriorityDistance {
                minimum_centre_distance_m: 0.1,
            },
        );
        let request = || DomainRequest {
            definition: &d,
            bounds: rect((-1.0, -1.0), (1.0, 1.0)),
            seeds: seeds(),
        };
        assert_eq!(generate(&request()), generate(&request()));
    }

    #[test]
    fn two_domains_with_different_names_do_not_share_a_lattice() {
        // If they did, stones would sit exactly where tufts do.
        let tufts = domain(0.2, 8, SpacingPolicy::Jittered);
        let mut stones = tufts.clone();
        stones.key = DomainKey::new("rock.large").expect("valid");
        assert_ne!(tufts.hash(), stones.hash());

        let seeds = seeds();
        let bounds = rect((0.0, 0.0), (2.0, 2.0));
        let a = generate(&DomainRequest {
            definition: &tufts,
            bounds,
            seeds,
        });
        let b = generate(&DomainRequest {
            definition: &stones,
            bounds,
            seeds,
        });
        assert_ne!(a[0].position, b[0].position);
    }

    #[test]
    fn a_degenerate_domain_produces_nothing_rather_than_panicking() {
        // Reached from authored data, so it has to be reported upstream rather
        // than crashing several layers down.
        let mut d = domain(0.0, 8, SpacingPolicy::Jittered);
        assert!(!d.is_well_formed());
        assert!(
            generate(&DomainRequest {
                definition: &d,
                bounds: rect((0.0, 0.0), (1.0, 1.0)),
                seeds: seeds(),
            })
            .is_empty()
        );
        d.cell_m = 0.2;
        d.candidates_per_cell = 0;
        assert!(!d.is_well_formed());
    }
}

#[cfg(test)]
mod footprint_tests {
    use super::*;
    use terrain_core::seed::RootSeed;

    fn seeds() -> SeedContext {
        SeedContext::new(
            RootSeed::new(0x5a17_e33b_0c9d_2f14),
            DOMAIN_ALGORITHM_VERSION,
        )
    }

    fn rect(min: (f64, f64), max: (f64, f64)) -> WorldRect {
        WorldRect::new(WorldPoint::new(min.0, min.1), WorldPoint::new(max.0, max.1))
    }

    fn stones(min_m: f64, max_m: f64, clearance_m: f64) -> CandidateDomainDef {
        CandidateDomainDef {
            key: DomainKey::new("rock.large").expect("valid"),
            cell_m: 0.5,
            candidates_per_cell: 4,
            spacing: SpacingPolicy::PriorityFootprints {
                radius: CandidateRadiusPolicy::Uniform { min_m, max_m },
                clearance_m,
            },
        }
    }

    #[test]
    fn no_two_survivors_overlap_their_own_footprints() {
        // The rule, checked against the survivors rather than against the
        // algorithm. Two stones interpenetrating is the one artefact in this
        // family that is unmistakable.
        let definition = stones(0.03, 0.12, 0.01);
        let kept = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((0.0, 0.0), (6.0, 6.0)),
            seeds: seeds(),
        });
        assert!(kept.len() > 20, "only {} survivors", kept.len());
        for (index, a) in kept.iter().enumerate() {
            for b in &kept[index + 1..] {
                let limit = a.footprint_radius_m + b.footprint_radius_m + 0.01;
                let distance = a.position.distance(b.position);
                assert!(
                    distance >= limit - 1.0e-9,
                    "two stones of {:.3} and {:.3} m sit {distance:.3} m apart",
                    a.footprint_radius_m,
                    b.footprint_radius_m
                );
            }
        }
    }

    #[test]
    fn a_variable_radius_domain_produces_a_range_of_sizes() {
        // Guards the test above from being vacuous: if every radius came out
        // the same, the sum rule would reduce to a fixed distance and the
        // variable half would be untested.
        let definition = stones(0.03, 0.12, 0.0);
        let kept = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((0.0, 0.0), (6.0, 6.0)),
            seeds: seeds(),
        });
        let low = kept
            .iter()
            .map(|c| c.footprint_radius_m)
            .fold(f64::INFINITY, f64::min);
        let high = kept
            .iter()
            .map(|c| c.footprint_radius_m)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(high - low > 0.05, "radii span only {low}..{high}");
        assert!(low >= 0.03 - 1.0e-9 && high <= 0.12 + 1.0e-9);
    }

    #[test]
    fn the_bucketed_search_agrees_with_brute_force() {
        // The strongest defence against a missed bucket-range case, which is
        // the one bug in this algorithm that produces a *nearly* correct
        // result: a few overlapping pairs in a field of thousands.
        let definition = stones(0.03, 0.12, 0.01);
        let bounds = rect((0.0, 0.0), (3.0, 3.0));
        let bucketed = generate(&DomainRequest {
            definition: &definition,
            bounds,
            seeds: seeds(),
        });

        // The same rule, evaluated against every pair.
        let all = lay_out(
            &definition,
            bounds.expanded(definition.spacing.conflict_reach_m()),
            &seeds(),
        );
        let brute = thin(all, bounds, |a, b| {
            let limit = a.footprint_radius_m + b.footprint_radius_m + 0.01;
            squared_distance(a, b) < limit * limit
        });

        let ids = |set: &[DomainCandidate]| {
            let mut out: Vec<_> = set.iter().map(|c| c.id).collect();
            out.sort();
            out
        };
        assert_eq!(ids(&bucketed), ids(&brute));
    }

    #[test]
    fn footprint_thinning_agrees_across_a_join() {
        // The seam property, for the variable-radius rule. A candidate near the
        // edge of one window is in the middle of another and must live or die
        // the same way in both.
        let definition = stones(0.03, 0.12, 0.01);
        let seeds = seeds();
        let whole = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((0.0, 0.0), (4.0, 2.0)),
            seeds,
        });
        let left = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((0.0, 0.0), (2.0, 2.0)),
            seeds,
        });
        let right = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((2.0, 0.0), (4.0, 2.0)),
            seeds,
        });

        let mut halves: Vec<_> = left.iter().chain(right.iter()).map(|c| c.id).collect();
        halves.sort();
        let mut together: Vec<_> = whole.iter().map(|c| c.id).collect();
        together.sort();
        assert_eq!(together, halves);
    }

    #[test]
    fn a_candidates_radius_is_the_same_whichever_window_computed_it() {
        // Addressed rather than drawn. If a radius depended on traversal, two
        // neighbouring plates would thin their shared boundary differently and
        // the join would show.
        let definition = stones(0.03, 0.12, 0.0);
        let seeds = seeds();
        let left = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((0.0, 0.0), (2.0, 2.0)),
            seeds,
        });
        let wide = generate(&DomainRequest {
            definition: &definition,
            bounds: rect((-2.0, -2.0), (4.0, 4.0)),
            seeds,
        });
        for candidate in &left {
            let twin = wide
                .iter()
                .find(|other| other.id == candidate.id)
                .expect("a survivor of the small window survives the large one");
            assert_eq!(
                twin.footprint_radius_m.to_bits(),
                candidate.footprint_radius_m.to_bits()
            );
        }
    }

    #[test]
    fn the_priority_key_is_a_strict_total_order() {
        // Rank alone is not enough: different cells share ranks, so two
        // candidates in adjacent cells with equal priority would compare equal
        // and both survive or both die depending on which was walked first.
        let definition = stones(0.05, 0.05, 0.0);
        let all = lay_out(&definition, rect((0.0, 0.0), (2.0, 2.0)), &seeds());
        let mut keys: Vec<_> = all.iter().map(|c| c.priority_key()).collect();
        keys.sort();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "two candidates share a priority key");
    }

    #[test]
    fn a_clearance_pushes_survivors_further_apart() {
        let bounds = rect((0.0, 0.0), (4.0, 4.0));
        let close = generate(&DomainRequest {
            definition: &stones(0.05, 0.05, 0.0),
            bounds,
            seeds: seeds(),
        });
        let spaced = generate(&DomainRequest {
            definition: &stones(0.05, 0.05, 0.08),
            bounds,
            seeds: seeds(),
        });
        assert!(
            spaced.len() < close.len(),
            "a clearance of 8 cm kept {} where none kept {}",
            spaced.len(),
            close.len()
        );
    }
}

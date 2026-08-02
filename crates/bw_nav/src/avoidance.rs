//! Local avoidance.
//!
//! Flow fields say where the crowd is going; they say nothing about units
//! standing on each other. That is this module's job, and it is deliberately
//! steering rather than rigid collision resolution — an auto-battler wants
//! units that flow around each other, not a physics solver fighting the
//! pathfinder for control of the same position.

use bw_core::{Real, UnitId, Vec2Fx, ceil_div_to_int, floor_div_to_int, real_from_int};
use indexmap::IndexMap;

/// A unit as seen by the avoidance system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neighbor {
    pub id: UnitId,
    pub position: Vec2Fx,
    pub radius: Real,
}

/// A uniform grid over unit positions, rebuilt each tick.
///
/// Rebuilding beats incremental maintenance here: every unit moves every tick,
/// so an incremental structure would process the same number of updates while
/// also carrying bookkeeping. A fresh build is a single linear pass.
#[derive(Debug, Default)]
pub struct SpatialHash {
    cell_size: Real,
    buckets: IndexMap<(i32, i32), Vec<u32>>,
    entries: Vec<Neighbor>,
}

impl SpatialHash {
    /// Build over `units`.
    ///
    /// `cell_size` should be around the largest query radius: much smaller and
    /// a query touches many buckets, much larger and each bucket holds units
    /// that are nowhere near the query.
    pub fn build(cell_size: Real, units: impl IntoIterator<Item = Neighbor>) -> Self {
        let cell_size = if cell_size > Real::ZERO {
            cell_size
        } else {
            real_from_int(1)
        };
        let entries: Vec<Neighbor> = units.into_iter().collect();
        let mut buckets: IndexMap<(i32, i32), Vec<u32>> = IndexMap::new();
        for (i, entry) in entries.iter().enumerate() {
            buckets
                .entry(cell_of(entry.position, cell_size))
                .or_default()
                .push(i as u32);
        }
        Self {
            cell_size,
            buckets,
            entries,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Neighbours within `radius` of `center`, excluding `exclude`.
    ///
    /// Results come back sorted by [`UnitId`], so a caller accumulating a
    /// steering force adds them in the same order every run. Fixed-point
    /// addition is not associative in general, and without the sort a bucket
    /// reshuffle would perturb the sum.
    pub fn query(
        &self,
        center: Vec2Fx,
        radius: Real,
        exclude: Option<UnitId>,
        out: &mut Vec<Neighbor>,
    ) {
        out.clear();
        if self.entries.is_empty() {
            return;
        }
        let radius_sq = radius * radius;
        let span = ceil_div_to_int(radius, self.cell_size);
        let (cx, cy) = cell_of(center, self.cell_size);

        for gy in (cy - span)..=(cy + span) {
            for gx in (cx - span)..=(cx + span) {
                let Some(bucket) = self.buckets.get(&(gx, gy)) else {
                    continue;
                };
                for &i in bucket {
                    let entry = self.entries[i as usize];
                    if Some(entry.id) == exclude {
                        continue;
                    }
                    if entry.position.distance_squared(center) <= radius_sq {
                        out.push(entry);
                    }
                }
            }
        }
        out.sort_unstable_by_key(|n| n.id);
    }
}

fn cell_of(position: Vec2Fx, cell_size: Real) -> (i32, i32) {
    (
        floor_div_to_int(position.x, cell_size),
        floor_div_to_int(position.y, cell_size),
    )
}

/// Push-apart force from overlapping neighbours.
///
/// Weighted by how deep the overlap is, so units barely touching are nudged and
/// units genuinely stacked are pushed hard. Returns a direction of unit length
/// or less; the caller scales it by the unit's speed.
///
/// `neighbors` must be in a deterministic order — [`SpatialHash::query`]
/// guarantees that.
pub fn separation(me: Neighbor, neighbors: &[Neighbor]) -> Vec2Fx {
    let mut push = Vec2Fx::ZERO;
    for other in neighbors {
        if other.id == me.id {
            continue;
        }
        let offset = me.position - other.position;
        let min_distance = me.radius + other.radius;
        let distance_sq = offset.length_squared();
        if distance_sq >= min_distance * min_distance {
            continue;
        }
        let distance = offset.length();
        if distance == Real::ZERO {
            // Exactly coincident. Break the symmetry by unit id rather than
            // randomly, so the outcome is reproducible.
            let sign = if me.id < other.id {
                real_from_int(1)
            } else {
                real_from_int(-1)
            };
            push += Vec2Fx::new(sign, Real::ZERO);
            continue;
        }
        let overlap = (min_distance - distance) / min_distance;
        push += offset.normalize_or_zero() * overlap;
    }
    push.clamp_length(real_from_int(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: u32, x: i32, y: i32) -> Neighbor {
        Neighbor {
            id: UnitId(id),
            position: Vec2Fx::from_ints(x, y),
            radius: real_from_int(1) / real_from_int(2),
        }
    }

    #[test]
    fn finds_neighbors_inside_the_radius_only() {
        let hash = SpatialHash::build(
            real_from_int(2),
            [unit(0, 0, 0), unit(1, 1, 0), unit(2, 10, 10)],
        );
        let mut out = Vec::new();
        hash.query(Vec2Fx::ZERO, real_from_int(2), None, &mut out);
        let ids: Vec<_> = out.iter().map(|n| n.id.0).collect();
        assert_eq!(ids, [0, 1]);
    }

    #[test]
    fn exclusion_removes_the_querying_unit() {
        let hash = SpatialHash::build(real_from_int(2), [unit(0, 0, 0), unit(1, 1, 0)]);
        let mut out = Vec::new();
        hash.query(Vec2Fx::ZERO, real_from_int(2), Some(UnitId(0)), &mut out);
        assert_eq!(out.iter().map(|n| n.id.0).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn results_are_sorted_regardless_of_insertion_order() {
        let forward = SpatialHash::build(
            real_from_int(2),
            [unit(2, 1, 1), unit(0, 0, 0), unit(1, 1, 0)],
        );
        let backward = SpatialHash::build(
            real_from_int(2),
            [unit(1, 1, 0), unit(0, 0, 0), unit(2, 1, 1)],
        );
        let mut a = Vec::new();
        let mut b = Vec::new();
        forward.query(Vec2Fx::ZERO, real_from_int(4), None, &mut a);
        backward.query(Vec2Fx::ZERO, real_from_int(4), None, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn negative_coordinates_bucket_correctly() {
        let hash = SpatialHash::build(real_from_int(2), [unit(0, -5, -5), unit(1, -4, -5)]);
        let mut out = Vec::new();
        hash.query(Vec2Fx::from_ints(-5, -5), real_from_int(2), None, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn empty_hash_queries_cleanly() {
        let hash = SpatialHash::build(real_from_int(2), []);
        let mut out = vec![unit(9, 9, 9)];
        hash.query(Vec2Fx::ZERO, real_from_int(4), None, &mut out);
        assert!(out.is_empty(), "query must clear the output buffer");
    }

    #[test]
    fn separation_pushes_apart_only_when_overlapping() {
        let me = unit(0, 0, 0);
        let touching = Neighbor {
            position: Vec2Fx::from_ints(2, 0),
            ..unit(1, 2, 0)
        };
        assert_eq!(separation(me, &[touching]), Vec2Fx::ZERO);

        let overlapping = Neighbor {
            id: UnitId(1),
            position: Vec2Fx::new(real_from_int(1) / real_from_int(4), Real::ZERO),
            radius: real_from_int(1) / real_from_int(2),
        };
        let push = separation(me, &[overlapping]);
        assert!(
            push.x < Real::ZERO,
            "should be pushed away from the neighbour"
        );
    }

    #[test]
    fn coincident_units_separate_deterministically() {
        // Two units at exactly the same point have no direction to push along.
        // Falling back to unit-id order keeps it reproducible instead of NaN or
        // a coin flip.
        let a = unit(0, 5, 5);
        let b = unit(1, 5, 5);
        let push_a = separation(a, &[b]);
        let push_b = separation(b, &[a]);
        assert_ne!(push_a, Vec2Fx::ZERO);
        assert_eq!(push_a, -push_b);
    }

    #[test]
    fn separation_is_bounded() {
        let me = unit(0, 0, 0);
        let crowd: Vec<_> = (1..20).map(|i| unit(i, 0, 0)).collect();
        assert!(separation(me, &crowd).length() <= real_from_int(1));
    }
}

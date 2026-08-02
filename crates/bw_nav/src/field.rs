//! Integration and flow fields.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use bw_core::{Grid, GridPos, Real, Vec2Fx, real_from_int};
use indexmap::IndexMap;

use crate::cost::CostField;

/// Distance value for a cell the goal cannot reach.
pub const UNREACHABLE: u32 = u32::MAX;

/// Diagonal movement costs sqrt(2) times an orthogonal step. Expressed as a
/// 256ths multiplier so the whole search stays in integers — 362/256 = 1.4141,
/// within 0.01% of sqrt(2) and identical on every machine.
const DIAGONAL_NUMERATOR: u32 = 362;
const DIAGONAL_DENOMINATOR: u32 = 256;

/// Accumulated cost from every cell to the nearest goal.
#[derive(Clone, Debug)]
pub struct IntegrationField {
    grid: Grid,
    distance: Vec<u32>,
}

impl IntegrationField {
    /// Dijkstra outward from `goals` across `costs`.
    ///
    /// A binary heap keyed on `(distance, cell_index)` rather than distance
    /// alone. The index is not a tie-breaking nicety — `BinaryHeap` gives no
    /// ordering guarantee among equal keys, so without it two runs could expand
    /// equidistant cells in different orders and produce different flow
    /// directions from identical input.
    pub fn compute(costs: &CostField, goals: &[GridPos]) -> Self {
        let grid = *costs.grid();
        let mut distance = vec![UNREACHABLE; grid.cell_count()];
        let mut heap: BinaryHeap<Reverse<(u32, usize)>> = BinaryHeap::new();

        for &goal in goals {
            if let Some(index) = grid.index(goal) {
                if costs.blocked_at_index(index) {
                    continue;
                }
                if distance[index] != 0 {
                    distance[index] = 0;
                    heap.push(Reverse((0, index)));
                }
            }
        }

        while let Some(Reverse((dist, index))) = heap.pop() {
            // Stale entry: we already found a cheaper route to this cell.
            if dist > distance[index] {
                continue;
            }
            let from = grid.from_index(index);
            for (n, neighbor) in from.neighbors8().into_iter().enumerate() {
                let Some(n_index) = grid.index(neighbor) else {
                    continue;
                };
                if costs.blocked_at_index(n_index) {
                    continue;
                }
                let step = costs.cost_at_index(n_index) as u32;
                // neighbors8 lists the four orthogonals first, then diagonals.
                let step = if n < 4 {
                    step
                } else {
                    if !Self::diagonal_is_open(costs, &grid, from, neighbor) {
                        continue;
                    }
                    step * DIAGONAL_NUMERATOR / DIAGONAL_DENOMINATOR
                };
                let next = dist.saturating_add(step);
                if next < distance[n_index] {
                    distance[n_index] = next;
                    heap.push(Reverse((next, n_index)));
                }
            }
        }

        Self { grid, distance }
    }

    /// Whether a diagonal step is legal.
    ///
    /// Both shared orthogonal neighbours must be open, otherwise units cut
    /// through the corner where two walls meet — visually wrong, and it lets
    /// them slip through gaps a body could not fit down.
    fn diagonal_is_open(costs: &CostField, grid: &Grid, from: GridPos, to: GridPos) -> bool {
        let horizontal = GridPos::new(to.x, from.y);
        let vertical = GridPos::new(from.x, to.y);
        let open = |p: GridPos| grid.index(p).is_some_and(|i| !costs.blocked_at_index(i));
        open(horizontal) && open(vertical)
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn distance(&self, pos: GridPos) -> u32 {
        self.grid
            .index(pos)
            .map_or(UNREACHABLE, |i| self.distance[i])
    }

    pub fn is_reachable(&self, pos: GridPos) -> bool {
        self.distance(pos) != UNREACHABLE
    }

    pub(crate) fn distance_at_index(&self, index: usize) -> u32 {
        self.distance[index]
    }
}

/// One movement direction per cell, pointing downhill toward the goal.
///
/// Directions are stored as an index into [`GridPos::neighbors8`] order rather
/// than as a vector, which keeps the field at one byte per cell — a 256x256
/// battlefield is 64 KB, small enough to rebuild often and cache several of.
#[derive(Clone, Debug)]
pub struct FlowField {
    grid: Grid,
    direction: Vec<u8>,
}

/// Marker for "no direction": the goal itself, or an unreachable cell.
const NO_DIRECTION: u8 = u8::MAX;

impl FlowField {
    pub fn from_integration(field: &IntegrationField) -> Self {
        let grid = *field.grid();
        let mut direction = vec![NO_DIRECTION; grid.cell_count()];

        for (index, slot) in direction.iter_mut().enumerate() {
            let here = field.distance_at_index(index);
            if here == UNREACHABLE || here == 0 {
                continue;
            }
            let from = grid.from_index(index);
            let mut best = here;
            let mut best_dir = NO_DIRECTION;
            // Strictly-less-than, walking neighbours in their fixed order, so
            // equidistant neighbours always resolve to the same one — and
            // orthogonals win over diagonals because they come first.
            for (n, neighbor) in from.neighbors8().into_iter().enumerate() {
                let Some(n_index) = grid.index(neighbor) else {
                    continue;
                };
                let candidate = field.distance_at_index(n_index);
                if candidate < best {
                    best = candidate;
                    best_dir = n as u8;
                }
            }
            *slot = best_dir;
        }

        Self { grid, direction }
    }

    pub fn compute(costs: &CostField, goals: &[GridPos]) -> Self {
        Self::from_integration(&IntegrationField::compute(costs, goals))
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Unit direction to move from `pos`, or `None` at the goal or on an
    /// unreachable cell.
    pub fn direction(&self, pos: GridPos) -> Option<Vec2Fx> {
        let index = self.grid.index(pos)?;
        let dir = self.direction[index];
        if dir == NO_DIRECTION {
            return None;
        }
        let target = pos.neighbors8()[dir as usize];
        Some(
            Vec2Fx::new(
                real_from_int(target.x - pos.x),
                real_from_int(target.y - pos.y),
            )
            .normalize_or_zero(),
        )
    }

    /// The neighbouring cell a unit on `pos` should step to.
    pub fn next_cell(&self, pos: GridPos) -> Option<GridPos> {
        let index = self.grid.index(pos)?;
        let dir = self.direction[index];
        (dir != NO_DIRECTION).then(|| pos.neighbors8()[dir as usize])
    }
}

/// Which goal a cached field was built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GoalKey(pub GridPos);

/// Flow fields kept between ticks.
///
/// Units share destinations, so the same field usually answers for a whole
/// team. Terrain changes invalidate everything at once via [`invalidate_all`],
/// which is the honest thing to do — a mud patch can alter distances arbitrarily
/// far away, so there is no sound way to patch a field in place.
///
/// [`invalidate_all`]: FlowFieldCache::invalidate_all
#[derive(Default)]
pub struct FlowFieldCache {
    fields: IndexMap<GoalKey, FlowField>,
    /// Cache statistics, for the navigation benchmark to report against.
    hits: u64,
    misses: u64,
}

impl FlowFieldCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch or build the field for `goal`.
    pub fn get_or_build(&mut self, costs: &CostField, goal: GridPos) -> &FlowField {
        let key = GoalKey(goal);
        if self.fields.contains_key(&key) {
            self.hits += 1;
        } else {
            self.misses += 1;
            self.fields.insert(key, FlowField::compute(costs, &[goal]));
        }
        &self.fields[&key]
    }

    pub fn get(&self, goal: GridPos) -> Option<&FlowField> {
        self.fields.get(&GoalKey(goal))
    }

    /// Drop every cached field. Call when the cost field changes.
    pub fn invalidate_all(&mut self) {
        self.fields.clear();
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn hit_rate(&self) -> Real {
        let total = self.hits + self.misses;
        if total == 0 {
            return Real::ZERO;
        }
        real_from_int(self.hits as i32) / real_from_int(total as i32)
    }

    /// Evict the oldest fields until at most `max` remain.
    pub fn trim_to(&mut self, max: usize) {
        while self.fields.len() > max {
            self.fields.shift_remove_index(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use bw_core::{GridDims, Vec2Fx, real_from_int};

    use super::*;

    fn costs(width: u32, height: u32) -> CostField {
        CostField::new(Grid::new(
            GridDims::new(width, height),
            real_from_int(1),
            Vec2Fx::ZERO,
        ))
    }

    #[test]
    fn goal_has_zero_distance_and_no_direction() {
        let c = costs(8, 8);
        let goal = GridPos::new(4, 4);
        let integration = IntegrationField::compute(&c, &[goal]);
        assert_eq!(integration.distance(goal), 0);
        assert!(
            FlowField::from_integration(&integration)
                .direction(goal)
                .is_none()
        );
    }

    #[test]
    fn distance_grows_with_separation() {
        let c = costs(8, 8);
        let f = IntegrationField::compute(&c, &[GridPos::new(0, 0)]);
        assert!(f.distance(GridPos::new(3, 0)) > f.distance(GridPos::new(1, 0)));
    }

    #[test]
    fn every_open_cell_flows_downhill_to_the_goal() {
        // The property that matters: follow the arrows from anywhere and you
        // arrive, without looping.
        let c = costs(12, 12);
        let goal = GridPos::new(11, 7);
        let flow = FlowField::compute(&c, &[goal]);
        for start in c.grid().iter_cells() {
            let mut at = start;
            let mut steps = 0;
            while at != goal {
                let Some(next) = flow.next_cell(at) else {
                    panic!("no direction at {at:?} starting from {start:?}");
                };
                at = next;
                steps += 1;
                assert!(steps < 400, "did not converge from {start:?}");
            }
        }
    }

    #[test]
    fn walls_are_not_entered_and_do_not_stop_the_sweep() {
        let mut c = costs(9, 9);
        for y in 0..8 {
            c.set_blocked(GridPos::new(4, y), true);
        }
        let goal = GridPos::new(8, 4);
        let flow = FlowField::compute(&c, &[goal]);
        let mut at = GridPos::new(0, 4);
        let mut steps = 0;
        while at != goal {
            at = flow
                .next_cell(at)
                .expect("path should route around the wall");
            assert!(!c.is_blocked(at), "path entered a wall at {at:?}");
            steps += 1;
            assert!(steps < 400, "did not converge around the wall");
        }
    }

    #[test]
    fn fully_enclosed_regions_are_unreachable_rather_than_wrong() {
        let mut c = costs(7, 7);
        for p in GridPos::new(1, 1).neighbors8() {
            c.set_blocked(p, true);
        }
        let f = IntegrationField::compute(&c, &[GridPos::new(5, 5)]);
        assert!(!f.is_reachable(GridPos::new(1, 1)));
        assert!(f.is_reachable(GridPos::new(5, 4)));
    }

    #[test]
    fn diagonals_do_not_cut_through_corners() {
        // Two walls meeting at a corner must not be squeezed through.
        let mut c = costs(5, 5);
        c.set_blocked(GridPos::new(2, 1), true);
        c.set_blocked(GridPos::new(1, 2), true);
        let f = IntegrationField::compute(&c, &[GridPos::new(1, 1)]);
        let direct = f.distance(GridPos::new(2, 2));
        let around = 2 * 256;
        assert!(
            direct > around,
            "corner was cut: {direct} should exceed a two-step detour"
        );
    }

    #[test]
    fn expensive_terrain_is_routed_around() {
        let mut c = costs(9, 3);
        for y in 0..3 {
            if y != 0 {
                c.set_cost(GridPos::new(4, y), 5000);
            }
        }
        let f = IntegrationField::compute(&c, &[GridPos::new(8, 1)]);
        assert!(f.distance(GridPos::new(0, 0)) < f.distance(GridPos::new(0, 2)) + 5000);
    }

    #[test]
    fn identical_inputs_produce_identical_fields() {
        // Guards the heap tie-break. Without the index in the sort key this can
        // pass by luck, so the field is compared cell by cell.
        let mut c = costs(16, 16);
        c.set_blocked(GridPos::new(8, 8), true);
        c.set_cost(GridPos::new(3, 3), 900);
        let build = || {
            FlowField::compute(&c, &[GridPos::new(15, 15)])
                .direction
                .clone()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn cache_serves_repeat_goals_and_clears_on_terrain_change() {
        let c = costs(8, 8);
        let mut cache = FlowFieldCache::new();
        let goal = GridPos::new(7, 7);
        cache.get_or_build(&c, goal);
        cache.get_or_build(&c, goal);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.hit_rate(), real_from_int(1) / real_from_int(2));

        cache.invalidate_all();
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_trims_to_a_bound() {
        let c = costs(8, 8);
        let mut cache = FlowFieldCache::new();
        for x in 0..5 {
            cache.get_or_build(&c, GridPos::new(x, 0));
        }
        cache.trim_to(2);
        assert_eq!(cache.len(), 2);
        // The oldest goals are the ones evicted.
        assert!(cache.get(GridPos::new(0, 0)).is_none());
        assert!(cache.get(GridPos::new(4, 0)).is_some());
    }
}

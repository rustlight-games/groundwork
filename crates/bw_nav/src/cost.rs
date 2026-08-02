//! Movement costs over the grid.

use bw_content::TerrainMap;
use bw_content::terrain::NORMAL_COST;
use bw_core::{Grid, GridPos};

/// Per-cell traversal cost, derived from terrain and then modified in place by
/// terrain effects such as mud or fire.
///
/// Separate from [`TerrainMap`] because it changes on a different schedule: the
/// terrain map is generated once per battle, while costs are patched whenever
/// an effect lands, and every patch invalidates cached flow fields.
#[derive(Clone, Debug)]
pub struct CostField {
    grid: Grid,
    cost: Vec<u16>,
    blocked: Vec<bool>,
}

impl CostField {
    pub fn new(grid: Grid) -> Self {
        let n = grid.cell_count();
        Self {
            grid,
            cost: vec![NORMAL_COST; n],
            blocked: vec![false; n],
        }
    }

    pub fn from_terrain(map: &TerrainMap) -> Self {
        Self {
            grid: *map.grid(),
            cost: map.move_costs().to_vec(),
            blocked: map.blocked_flags().to_vec(),
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn cell_count(&self) -> usize {
        self.cost.len()
    }

    /// Cost of entering `pos`. Out of bounds is treated as blocking.
    pub fn cost(&self, pos: GridPos) -> u16 {
        self.grid.index(pos).map_or(u16::MAX, |i| self.cost[i])
    }

    pub fn is_blocked(&self, pos: GridPos) -> bool {
        self.grid.index(pos).is_none_or(|i| self.blocked[i])
    }

    pub fn is_passable(&self, pos: GridPos) -> bool {
        !self.is_blocked(pos)
    }

    pub fn set_cost(&mut self, pos: GridPos, cost: u16) {
        if let Some(i) = self.grid.index(pos) {
            self.cost[i] = cost.max(1);
        }
    }

    /// Raise a cell's cost without ever lowering it, so overlapping terrain
    /// effects compose in any order.
    pub fn add_cost(&mut self, pos: GridPos, extra: u16) {
        if let Some(i) = self.grid.index(pos) {
            self.cost[i] = self.cost[i].saturating_add(extra);
        }
    }

    pub fn set_blocked(&mut self, pos: GridPos, blocked: bool) {
        if let Some(i) = self.grid.index(pos) {
            self.blocked[i] = blocked;
        }
    }

    pub(crate) fn cost_at_index(&self, index: usize) -> u16 {
        self.cost[index]
    }

    pub(crate) fn blocked_at_index(&self, index: usize) -> bool {
        self.blocked[index]
    }
}

#[cfg(test)]
mod tests {
    use bw_core::{GridDims, Vec2Fx, real_from_int};

    use super::*;

    fn field() -> CostField {
        CostField::new(Grid::new(
            GridDims::new(4, 4),
            real_from_int(1),
            Vec2Fx::ZERO,
        ))
    }

    #[test]
    fn defaults_to_normal_open_ground() {
        let f = field();
        assert_eq!(f.cost(GridPos::new(1, 1)), NORMAL_COST);
        assert!(f.is_passable(GridPos::new(1, 1)));
    }

    #[test]
    fn out_of_bounds_is_blocked() {
        let f = field();
        assert!(f.is_blocked(GridPos::new(-1, 0)));
        assert_eq!(f.cost(GridPos::new(4, 0)), u16::MAX);
    }

    #[test]
    fn cost_never_drops_below_one() {
        // A zero-cost cell would make Dijkstra's distances non-increasing and
        // let a unit cross the map for free.
        let mut f = field();
        f.set_cost(GridPos::new(0, 0), 0);
        assert_eq!(f.cost(GridPos::new(0, 0)), 1);
    }

    #[test]
    fn added_costs_are_order_independent_and_saturate() {
        let p = GridPos::new(2, 2);
        let mut a = field();
        a.add_cost(p, 300);
        a.add_cost(p, 40);
        let mut b = field();
        b.add_cost(p, 40);
        b.add_cost(p, 300);
        assert_eq!(a.cost(p), b.cost(p));

        let mut c = field();
        c.add_cost(p, u16::MAX);
        assert_eq!(c.cost(p), u16::MAX);
    }
}

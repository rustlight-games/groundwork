//! The generated terrain map.
//!
//! Terrain is the shared substrate three systems read: navigation turns it into
//! movement costs, grass turns it into density, and rendering turns it into
//! tiles. Keeping it as plain data here — rather than inside `bw_nav` or the
//! renderer — is what lets the headless trainer generate and simulate over
//! terrain without linking a renderer.

use bw_core::{Grid, GridPos};

use crate::params::Params;

/// Per-cell terrain, parallel arrays over a [`Grid`] in row-major order.
///
/// Parallel arrays rather than an array of structs because the consumers want
/// different fields: navigation reads only `move_cost` and `blocked`, and
/// walking a dense `u16` slice is considerably kinder to the cache than
/// striding over a wider struct.
#[derive(Clone, Debug)]
pub struct TerrainMap {
    grid: Grid,
    /// Index into the content database's terrain definitions.
    tile: Vec<u16>,
    /// Movement cost multiplier, 256 = normal. Integer to keep pathfinding exact.
    move_cost: Vec<u16>,
    blocked: Vec<bool>,
    /// Grass density, 0..=255.
    grass_density: Vec<u8>,
    /// Surface height, 0..=255. Drives shading and prop scatter, not movement.
    elevation: Vec<u8>,
}

/// Movement cost of ordinary open ground.
pub const NORMAL_COST: u16 = 256;

impl TerrainMap {
    /// An empty map of open ground.
    pub fn new(grid: Grid) -> Self {
        let n = grid.cell_count();
        Self {
            grid,
            tile: vec![0; n],
            move_cost: vec![NORMAL_COST; n],
            blocked: vec![false; n],
            grass_density: vec![0; n],
            elevation: vec![0; n],
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn cell_count(&self) -> usize {
        self.tile.len()
    }

    pub fn tile(&self, pos: GridPos) -> u16 {
        self.grid.index(pos).map_or(0, |i| self.tile[i])
    }

    /// Movement cost, saturating to blocking outside the map so units cannot
    /// path off the edge of the world.
    pub fn move_cost(&self, pos: GridPos) -> u16 {
        self.grid.index(pos).map_or(u16::MAX, |i| self.move_cost[i])
    }

    pub fn is_blocked(&self, pos: GridPos) -> bool {
        self.grid.index(pos).is_none_or(|i| self.blocked[i])
    }

    pub fn grass_density(&self, pos: GridPos) -> u8 {
        self.grid.index(pos).map_or(0, |i| self.grass_density[i])
    }

    pub fn elevation(&self, pos: GridPos) -> u8 {
        self.grid.index(pos).map_or(0, |i| self.elevation[i])
    }

    pub fn set(&mut self, pos: GridPos, cell: TerrainCell) {
        if let Some(i) = self.grid.index(pos) {
            self.tile[i] = cell.tile;
            self.move_cost[i] = cell.move_cost;
            self.blocked[i] = cell.blocked;
            self.grass_density[i] = cell.grass_density;
            self.elevation[i] = cell.elevation;
        }
    }

    /// Raise the movement cost of a cell without lowering it.
    ///
    /// Terrain *effects* — mud, fire, a crater — layer onto generated terrain,
    /// and they should never make ground cheaper than the generator decided.
    /// Saturating rather than assigning means two overlapping effects compose
    /// predictably regardless of which is applied first.
    pub fn add_cost(&mut self, pos: GridPos, extra: u16) {
        if let Some(i) = self.grid.index(pos) {
            self.move_cost[i] = self.move_cost[i].saturating_add(extra);
        }
    }

    pub fn set_blocked(&mut self, pos: GridPos, blocked: bool) {
        if let Some(i) = self.grid.index(pos) {
            self.blocked[i] = blocked;
        }
    }

    /// Raw cost slice, for navigation to build a cost field over.
    pub fn move_costs(&self) -> &[u16] {
        &self.move_cost
    }

    pub fn blocked_flags(&self) -> &[bool] {
        &self.blocked
    }

    pub fn grass_densities(&self) -> &[u8] {
        &self.grass_density
    }
}

/// One cell's worth of terrain, as produced by a generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainCell {
    pub tile: u16,
    pub move_cost: u16,
    pub blocked: bool,
    pub grass_density: u8,
    pub elevation: u8,
}

impl Default for TerrainCell {
    fn default() -> Self {
        Self {
            tile: 0,
            move_cost: NORMAL_COST,
            blocked: false,
            grass_density: 0,
            elevation: 0,
        }
    }
}

/// Context handed to a terrain generator.
#[derive(Clone, Debug)]
pub struct TerrainGenContext<'a> {
    pub grid: Grid,
    pub params: &'a Params,
    /// Salt so two generators sharing a battle seed do not produce correlated
    /// output.
    pub salt: u64,
}

#[cfg(test)]
mod tests {
    use bw_core::{GridDims, Vec2Fx, real_from_int};

    use super::*;

    fn map() -> TerrainMap {
        TerrainMap::new(Grid::new(
            GridDims::new(4, 4),
            real_from_int(1),
            Vec2Fx::ZERO,
        ))
    }

    #[test]
    fn new_map_is_open_ground() {
        let m = map();
        assert_eq!(m.move_cost(GridPos::new(1, 1)), NORMAL_COST);
        assert!(!m.is_blocked(GridPos::new(1, 1)));
    }

    #[test]
    fn outside_the_map_is_blocked_and_impassable() {
        // Units must not be able to path off the edge of the world.
        let m = map();
        assert!(m.is_blocked(GridPos::new(-1, 0)));
        assert_eq!(m.move_cost(GridPos::new(99, 0)), u16::MAX);
    }

    #[test]
    fn added_costs_compose_regardless_of_order() {
        let (a, b) = (GridPos::new(2, 2), GridPos::new(2, 2));
        let mut first = map();
        first.add_cost(a, 100);
        first.add_cost(b, 50);
        let mut second = map();
        second.add_cost(b, 50);
        second.add_cost(a, 100);
        assert_eq!(first.move_cost(a), second.move_cost(a));
        assert_eq!(first.move_cost(a), NORMAL_COST + 150);
    }

    #[test]
    fn added_cost_saturates_rather_than_wrapping() {
        let mut m = map();
        m.add_cost(GridPos::new(0, 0), u16::MAX);
        assert_eq!(m.move_cost(GridPos::new(0, 0)), u16::MAX);
    }

    #[test]
    fn writes_outside_the_map_are_ignored_not_panics() {
        let mut m = map();
        m.set(GridPos::new(-5, -5), TerrainCell::default());
        m.add_cost(GridPos::new(99, 99), 10);
        assert_eq!(m.cell_count(), 16);
    }
}

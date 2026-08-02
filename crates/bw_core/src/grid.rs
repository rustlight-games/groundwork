//! The spatial grid shared by terrain, navigation, and grass.
//!
//! Terrain generation writes tiles into it, navigation builds cost and flow
//! fields over it, and grass samples density from it. Keeping one grid
//! definition here means those three never disagree about where cell `(4, 7)`
//! is, which is the sort of off-by-half-a-cell bug that is miserable to find
//! once it is spread across three crates.

use serde::{Deserialize, Serialize};

use crate::fx::{Real, Vec2Fx, floor_div_to_int, real_from_int};

/// Integer cell coordinate. May be outside a given grid's bounds — use
/// [`Grid::contains`] before indexing.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct GridPos {
    pub x: i32,
    pub y: i32,
}

impl GridPos {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// The four orthogonal neighbours, in a fixed order.
    pub fn neighbors4(self) -> [Self; 4] {
        [
            Self::new(self.x + 1, self.y),
            Self::new(self.x, self.y + 1),
            Self::new(self.x - 1, self.y),
            Self::new(self.x, self.y - 1),
        ]
    }

    /// All eight neighbours, in a fixed order. Diagonals come last so that
    /// cost-equal orthogonal moves win ties, which keeps paths tidier.
    pub fn neighbors8(self) -> [Self; 8] {
        [
            Self::new(self.x + 1, self.y),
            Self::new(self.x, self.y + 1),
            Self::new(self.x - 1, self.y),
            Self::new(self.x, self.y - 1),
            Self::new(self.x + 1, self.y + 1),
            Self::new(self.x - 1, self.y + 1),
            Self::new(self.x - 1, self.y - 1),
            Self::new(self.x + 1, self.y - 1),
        ]
    }

    /// Chebyshev distance — the number of moves when diagonals are allowed.
    pub fn chebyshev_distance(self, other: Self) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }

    /// Manhattan distance.
    pub fn manhattan_distance(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }
}

/// Width and height of a grid in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridDims {
    pub width: u32,
    pub height: u32,
}

impl GridDims {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn cell_count(self) -> usize {
        (self.width as usize) * (self.height as usize)
    }
}

/// A grid placed in world space.
///
/// `origin` is the world position of the *corner* of cell `(0, 0)`, not its
/// centre. Cell centres are offset by half a cell, which [`cell_center`] does
/// for you.
///
/// [`cell_center`]: Grid::cell_center
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub dims: GridDims,
    pub cell_size: Real,
    pub origin: Vec2Fx,
}

impl Grid {
    pub fn new(dims: GridDims, cell_size: Real, origin: Vec2Fx) -> Self {
        debug_assert!(cell_size > Real::ZERO, "cell_size must be positive");
        Self {
            dims,
            cell_size,
            origin,
        }
    }

    /// A grid centred on the world origin.
    pub fn centered(dims: GridDims, cell_size: Real) -> Self {
        let half = Vec2Fx::new(
            cell_size * real_from_int(dims.width as i32) / real_from_int(2),
            cell_size * real_from_int(dims.height as i32) / real_from_int(2),
        );
        Self::new(dims, cell_size, -half)
    }

    pub fn contains(&self, pos: GridPos) -> bool {
        pos.x >= 0
            && pos.y >= 0
            && (pos.x as u32) < self.dims.width
            && (pos.y as u32) < self.dims.height
    }

    /// Row-major index, or `None` when out of bounds.
    pub fn index(&self, pos: GridPos) -> Option<usize> {
        self.contains(pos)
            .then(|| (pos.y as usize) * (self.dims.width as usize) + (pos.x as usize))
    }

    pub fn from_index(&self, index: usize) -> GridPos {
        let w = self.dims.width as usize;
        GridPos::new((index % w) as i32, (index / w) as i32)
    }

    /// The cell containing `world`.
    ///
    /// Uses floor division so that coordinates below the origin map correctly —
    /// truncating division would fold `-0.5` and `+0.5` into the same cell and
    /// make the row and column at the origin twice as wide as every other.
    pub fn world_to_cell(&self, world: Vec2Fx) -> GridPos {
        let local = world - self.origin;
        GridPos::new(
            floor_div_to_int(local.x, self.cell_size),
            floor_div_to_int(local.y, self.cell_size),
        )
    }

    /// World position of the centre of `pos`.
    pub fn cell_center(&self, pos: GridPos) -> Vec2Fx {
        let half = self.cell_size / real_from_int(2);
        Vec2Fx::new(
            self.origin.x + real_from_int(pos.x) * self.cell_size + half,
            self.origin.y + real_from_int(pos.y) * self.cell_size + half,
        )
    }

    /// Clamp a cell into bounds.
    pub fn clamp(&self, pos: GridPos) -> GridPos {
        GridPos::new(
            pos.x.clamp(0, self.dims.width.saturating_sub(1) as i32),
            pos.y.clamp(0, self.dims.height.saturating_sub(1) as i32),
        )
    }

    pub fn cell_count(&self) -> usize {
        self.dims.cell_count()
    }

    /// Iterate every cell in row-major order.
    pub fn iter_cells(&self) -> impl Iterator<Item = GridPos> + '_ {
        (0..self.dims.height as i32)
            .flat_map(move |y| (0..self.dims.width as i32).map(move |x| GridPos::new(x, y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Grid {
        Grid::new(GridDims::new(8, 4), real_from_int(2), Vec2Fx::ZERO)
    }

    #[test]
    fn index_round_trips() {
        let g = grid();
        for cell in g.iter_cells() {
            assert_eq!(g.from_index(g.index(cell).unwrap()), cell);
        }
    }

    #[test]
    fn out_of_bounds_has_no_index() {
        let g = grid();
        assert!(g.index(GridPos::new(-1, 0)).is_none());
        assert!(g.index(GridPos::new(8, 0)).is_none());
        assert!(g.index(GridPos::new(0, 4)).is_none());
    }

    #[test]
    fn cell_center_maps_back_to_its_own_cell() {
        let g = grid();
        for cell in g.iter_cells() {
            assert_eq!(g.world_to_cell(g.cell_center(cell)), cell);
        }
    }

    #[test]
    fn coordinates_below_the_origin_floor_rather_than_truncate() {
        // The bug this guards: truncation puts -1.0 and +1.0 both in cell 0,
        // making the row and column at the origin twice as wide as the rest.
        //
        // Note the origin is zero here, so these are genuinely negative *local*
        // coordinates. An earlier version of this test placed the origin at
        // (-8, -8), which made every local coordinate non-negative and let a
        // broken floor_div pass unnoticed.
        let g = Grid::new(GridDims::new(8, 8), real_from_int(2), Vec2Fx::ZERO);
        assert_eq!(
            g.world_to_cell(Vec2Fx::from_ints(-5, -5)),
            GridPos::new(-3, -3)
        );
        assert_eq!(
            g.world_to_cell(Vec2Fx::from_ints(-4, -4)),
            GridPos::new(-2, -2)
        );
        assert_eq!(
            g.world_to_cell(Vec2Fx::from_ints(-3, -3)),
            GridPos::new(-2, -2)
        );
        assert_eq!(
            g.world_to_cell(Vec2Fx::from_ints(-1, -1)),
            GridPos::new(-1, -1)
        );
        assert_eq!(g.world_to_cell(Vec2Fx::ZERO), GridPos::new(0, 0));
        assert_eq!(g.world_to_cell(Vec2Fx::from_ints(3, 3)), GridPos::new(1, 1));
    }

    #[test]
    fn cells_below_the_origin_round_trip_too() {
        let g = Grid::new(GridDims::new(8, 8), real_from_int(2), Vec2Fx::ZERO);
        for cell in [
            GridPos::new(-3, -1),
            GridPos::new(-1, -4),
            GridPos::new(-7, -7),
        ] {
            assert_eq!(g.world_to_cell(g.cell_center(cell)), cell);
        }
    }

    #[test]
    fn centered_grid_straddles_the_origin() {
        let g = Grid::centered(GridDims::new(4, 4), real_from_int(2));
        assert_eq!(g.origin, Vec2Fx::from_ints(-4, -4));
        assert_eq!(g.world_to_cell(Vec2Fx::ZERO), GridPos::new(2, 2));
    }

    #[test]
    fn neighbor_order_is_fixed() {
        // Iteration order feeds into flow-field tie-breaking, so it is part of
        // the determinism contract rather than an implementation detail.
        let n = GridPos::new(0, 0).neighbors4();
        assert_eq!(
            n,
            [
                GridPos::new(1, 0),
                GridPos::new(0, 1),
                GridPos::new(-1, 0),
                GridPos::new(0, -1)
            ]
        );
    }

    #[test]
    fn clamp_pulls_into_bounds() {
        let g = grid();
        assert_eq!(g.clamp(GridPos::new(-5, 99)), GridPos::new(0, 3));
    }
}

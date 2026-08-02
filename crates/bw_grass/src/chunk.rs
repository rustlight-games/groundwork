//! Chunking.
//!
//! Grass is spatially uniform and enormously numerous, which makes it the
//! textbook case for chunking: one draw call per chunk, one visibility test per
//! chunk, and a rebuild that touches only the chunks that changed.

use bevy::prelude::*;
use bw_core::{Grid, GridPos};

/// Terrain cells per chunk edge.
///
/// Thirty-two is a compromise. Smaller chunks cull more precisely but multiply
/// draw calls; larger ones waste blade budget on off-screen grass. At one world
/// unit per cell this is a 32x32 patch, which is a sensible fraction of a
/// screen at typical zoom.
pub const CHUNK_CELLS: i32 = 32;

/// One chunk of grass.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrassChunk {
    /// Chunk coordinate, not cell coordinate.
    pub coord: GridPos,
    /// Blades currently allocated, after LOD.
    pub blade_count: u32,
}

/// Which chunks exist and which need rebuilding.
#[derive(Resource, Default, Debug)]
pub struct GrassChunks {
    loaded: Vec<GridPos>,
    dirty: Vec<GridPos>,
}

impl GrassChunks {
    pub fn loaded(&self) -> &[GridPos] {
        &self.loaded
    }

    pub fn dirty(&self) -> &[GridPos] {
        &self.dirty
    }

    pub fn is_loaded(&self, coord: GridPos) -> bool {
        self.loaded.contains(&coord)
    }

    pub fn load(&mut self, coord: GridPos) {
        if !self.is_loaded(coord) {
            self.loaded.push(coord);
            self.mark_dirty(coord);
        }
    }

    pub fn unload(&mut self, coord: GridPos) {
        self.loaded.retain(|&c| c != coord);
        self.dirty.retain(|&c| c != coord);
    }

    /// Flag a chunk for rebuild. Idempotent, so a terrain effect touching forty
    /// cells in one chunk queues one rebuild rather than forty.
    pub fn mark_dirty(&mut self, coord: GridPos) {
        if self.is_loaded(coord) && !self.dirty.contains(&coord) {
            self.dirty.push(coord);
        }
    }

    /// Take the dirty list, clearing it.
    pub fn take_dirty(&mut self) -> Vec<GridPos> {
        std::mem::take(&mut self.dirty)
    }

    pub fn len(&self) -> usize {
        self.loaded.len()
    }

    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }
}

/// The chunk containing a terrain cell.
///
/// Uses floor division so cells left of and below the origin land in the
/// correct chunk rather than all collapsing into chunk zero.
pub fn chunk_of(cell: GridPos) -> GridPos {
    GridPos::new(
        cell.x.div_euclid(CHUNK_CELLS),
        cell.y.div_euclid(CHUNK_CELLS),
    )
}

/// The range of cells a chunk covers, as inclusive corners.
pub fn cells_in_chunk(coord: GridPos) -> (GridPos, GridPos) {
    let min = GridPos::new(coord.x * CHUNK_CELLS, coord.y * CHUNK_CELLS);
    let max = GridPos::new(min.x + CHUNK_CELLS - 1, min.y + CHUNK_CELLS - 1);
    (min, max)
}

/// Every chunk needed to cover `grid`.
pub fn chunks_covering(grid: &Grid) -> Vec<GridPos> {
    let max_cell = GridPos::new(grid.dims.width as i32 - 1, grid.dims.height as i32 - 1);
    let last = chunk_of(max_cell);
    let mut out = Vec::new();
    for y in 0..=last.y {
        for x in 0..=last.x {
            out.push(GridPos::new(x, y));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use bw_core::{GridDims, Vec2Fx, real_from_int};

    use super::*;

    #[test]
    fn cells_map_into_the_expected_chunk() {
        assert_eq!(chunk_of(GridPos::new(0, 0)), GridPos::new(0, 0));
        assert_eq!(chunk_of(GridPos::new(31, 31)), GridPos::new(0, 0));
        assert_eq!(chunk_of(GridPos::new(32, 0)), GridPos::new(1, 0));
    }

    #[test]
    fn negative_cells_do_not_collapse_into_chunk_zero() {
        assert_eq!(chunk_of(GridPos::new(-1, -1)), GridPos::new(-1, -1));
        assert_eq!(chunk_of(GridPos::new(-32, 0)), GridPos::new(-1, 0));
        assert_eq!(chunk_of(GridPos::new(-33, 0)), GridPos::new(-2, 0));
    }

    #[test]
    fn chunk_bounds_round_trip() {
        for coord in [GridPos::new(0, 0), GridPos::new(3, -2)] {
            let (min, max) = cells_in_chunk(coord);
            assert_eq!(chunk_of(min), coord);
            assert_eq!(chunk_of(max), coord);
            assert_eq!(max.x - min.x + 1, CHUNK_CELLS);
        }
    }

    #[test]
    fn coverage_spans_the_whole_grid() {
        let grid = Grid::new(GridDims::new(64, 96), real_from_int(1), Vec2Fx::ZERO);
        let chunks = chunks_covering(&grid);
        assert_eq!(chunks.len(), 2 * 3);
        assert!(chunks.contains(&chunk_of(GridPos::new(63, 95))));
    }

    #[test]
    fn coverage_rounds_up_for_partial_chunks() {
        let grid = Grid::new(GridDims::new(33, 1), real_from_int(1), Vec2Fx::ZERO);
        assert_eq!(chunks_covering(&grid).len(), 2);
    }

    #[test]
    fn loading_a_chunk_marks_it_dirty_once() {
        let mut chunks = GrassChunks::default();
        chunks.load(GridPos::new(0, 0));
        chunks.load(GridPos::new(0, 0));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks.dirty().len(), 1);
    }

    #[test]
    fn marking_dirty_repeatedly_queues_one_rebuild() {
        // A terrain effect touching many cells in a chunk must not queue many
        // rebuilds of that chunk.
        let mut chunks = GrassChunks::default();
        chunks.load(GridPos::new(1, 1));
        chunks.take_dirty();
        for _ in 0..40 {
            chunks.mark_dirty(GridPos::new(1, 1));
        }
        assert_eq!(chunks.take_dirty(), vec![GridPos::new(1, 1)]);
    }

    #[test]
    fn unloaded_chunks_cannot_be_marked_dirty() {
        let mut chunks = GrassChunks::default();
        chunks.mark_dirty(GridPos::new(5, 5));
        assert!(chunks.dirty().is_empty());
    }

    #[test]
    fn unloading_clears_any_pending_rebuild() {
        let mut chunks = GrassChunks::default();
        chunks.load(GridPos::new(2, 2));
        chunks.unload(GridPos::new(2, 2));
        assert!(chunks.is_empty());
        assert!(chunks.dirty().is_empty());
    }
}

//! Where grass grows.
//!
//! Density comes from terrain generation rather than being generated here, so
//! that a mud patch or a scorched crater changes the grass without the grass
//! system needing to know what mud or fire are.

use bw_content::TerrainMap;
use bw_core::{Grid, GridPos};

/// Per-cell grass density, 0..=255.
#[derive(Clone, Debug)]
pub struct DensityMap {
    grid: Grid,
    density: Vec<u8>,
}

impl DensityMap {
    pub fn from_terrain(map: &TerrainMap) -> Self {
        Self {
            grid: *map.grid(),
            density: map.grass_densities().to_vec(),
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Out of bounds is bare ground, not an error — chunk edges routinely
    /// sample past the map.
    pub fn at(&self, cell: GridPos) -> u8 {
        self.grid.index(cell).map_or(0, |i| self.density[i])
    }

    pub fn set(&mut self, cell: GridPos, value: u8) {
        if let Some(i) = self.grid.index(cell) {
            self.density[i] = value;
        }
    }

    /// Blades to place in a cell at full detail.
    pub fn blades_at(&self, cell: GridPos, blades_per_full_cell: u32) -> u32 {
        (self.at(cell) as u32 * blades_per_full_cell) / 255
    }
}

#[cfg(test)]
mod tests {
    use bw_content::terrain::{TerrainCell, TerrainMap};
    use bw_core::{GridDims, Vec2Fx, real_from_int};

    use super::*;

    fn map() -> TerrainMap {
        TerrainMap::new(Grid::new(
            GridDims::new(8, 8),
            real_from_int(1),
            Vec2Fx::ZERO,
        ))
    }

    #[test]
    fn density_follows_the_terrain() {
        let mut terrain = map();
        terrain.set(
            GridPos::new(2, 2),
            TerrainCell {
                grass_density: 200,
                ..Default::default()
            },
        );
        let density = DensityMap::from_terrain(&terrain);
        assert_eq!(density.at(GridPos::new(2, 2)), 200);
        assert_eq!(density.at(GridPos::new(0, 0)), 0);
    }

    #[test]
    fn outside_the_map_is_bare() {
        let density = DensityMap::from_terrain(&map());
        assert_eq!(density.at(GridPos::new(-1, -1)), 0);
        assert_eq!(density.at(GridPos::new(99, 0)), 0);
    }

    #[test]
    fn blade_count_scales_with_density() {
        let mut terrain = map();
        terrain.set(
            GridPos::new(1, 1),
            TerrainCell {
                grass_density: 255,
                ..Default::default()
            },
        );
        terrain.set(
            GridPos::new(1, 2),
            TerrainCell {
                grass_density: 128,
                ..Default::default()
            },
        );
        let density = DensityMap::from_terrain(&terrain);
        assert_eq!(density.blades_at(GridPos::new(1, 1), 40), 40);
        assert_eq!(density.blades_at(GridPos::new(1, 2), 40), 20);
        assert_eq!(density.blades_at(GridPos::new(7, 7), 40), 0);
    }
}

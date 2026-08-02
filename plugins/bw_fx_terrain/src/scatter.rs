//! Sprite prop placement.
//!
//! Trees, bushes and debris are authored artwork, not generated geometry, so
//! this module only decides *where* they go. Dart throwing with a minimum
//! spacing rather than independent random placement: uniform random points
//! clump visibly, and a scattered forest that clumps reads as a mistake rather
//! than as nature. `bw_bench` scores the result with a blue-noise metric.

use bw_content::ScatterRule;
use bw_content::terrain::TerrainMap;
use bw_core::{GridPos, Real, Vec2Fx};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// A placed prop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScatterPoint {
    pub cell: GridPos,
    pub position: Vec2Fx,
}

/// How many placement attempts to make per wanted point before giving up.
///
/// Dart throwing has no upper bound on its own: once the map is dense, most
/// candidates are rejected and the loop can run indefinitely. Capping attempts
/// trades exact density for a guaranteed finish, which is the right way round.
const ATTEMPTS_PER_POINT: u32 = 24;

/// Place props according to `rule`.
///
/// Deterministic given `rng`. Results are sorted by cell so downstream
/// consumers see a stable order.
pub fn scatter(map: &TerrainMap, rule: &ScatterRule, rng: &mut ChaCha8Rng) -> Vec<ScatterPoint> {
    let grid = *map.grid();
    let cells = grid.cell_count() as f64;
    let wanted = ((cells / 100.0) * rule.density_per_100_cells)
        .round()
        .max(0.0) as usize;
    if wanted == 0 {
        return Vec::new();
    }

    let min_spacing = Real::from_num(rule.min_spacing.max(0.0));
    let min_spacing_sq = min_spacing * min_spacing;
    let (low, high) = rule.elevation_range;

    let mut placed: Vec<ScatterPoint> = Vec::with_capacity(wanted);
    let width = grid.dims.width as i32;
    let height = grid.dims.height as i32;

    for _ in 0..(wanted as u32 * ATTEMPTS_PER_POINT) {
        if placed.len() >= wanted {
            break;
        }
        let cell = GridPos::new(rng.random_range(0..width), rng.random_range(0..height));
        if map.is_blocked(cell) {
            continue;
        }
        let elevation = map.elevation(cell);
        if elevation < low || elevation > high {
            continue;
        }
        let position = grid.cell_center(cell);
        if min_spacing > Real::ZERO
            && placed
                .iter()
                .any(|p| p.position.distance_squared(position) < min_spacing_sq)
        {
            continue;
        }
        placed.push(ScatterPoint { cell, position });
    }

    placed.sort_unstable_by_key(|p| (p.cell.y, p.cell.x));
    placed
}

#[cfg(test)]
mod tests {
    use bw_content::terrain::TerrainCell;
    use bw_core::{Grid, GridDims, real_from_int};
    use rand::SeedableRng;

    use super::*;

    fn map() -> TerrainMap {
        let grid = Grid::centered(GridDims::new(40, 40), real_from_int(1));
        TerrainMap::new(grid)
    }

    fn rule(density: f64, spacing: f64) -> ScatterRule {
        ScatterRule {
            density_per_100_cells: density,
            min_spacing: spacing,
            allowed_terrain: vec![],
            elevation_range: (0, 255),
        }
    }

    #[test]
    fn places_roughly_the_requested_density() {
        let points = scatter(&map(), &rule(5.0, 0.0), &mut ChaCha8Rng::seed_from_u64(1));
        // 1600 cells at 5 per 100 is 80.
        assert_eq!(points.len(), 80);
    }

    #[test]
    fn respects_minimum_spacing() {
        let spacing = 3.0;
        let points = scatter(
            &map(),
            &rule(4.0, spacing),
            &mut ChaCha8Rng::seed_from_u64(2),
        );
        let min_sq = Real::from_num(spacing) * Real::from_num(spacing);
        for (i, a) in points.iter().enumerate() {
            for b in &points[i + 1..] {
                assert!(
                    a.position.distance_squared(b.position) >= min_sq,
                    "{a:?} and {b:?} are closer than the minimum spacing"
                );
            }
        }
    }

    #[test]
    fn terminates_when_spacing_makes_the_density_impossible() {
        // Asking for 80 points that are 50 units apart on a 40-unit map cannot
        // be satisfied. It must return fewer points, not hang.
        let points = scatter(&map(), &rule(5.0, 50.0), &mut ChaCha8Rng::seed_from_u64(3));
        assert!(points.len() < 80);
        assert!(!points.is_empty());
    }

    #[test]
    fn never_places_on_blocked_ground() {
        let mut m = map();
        let blocked: Vec<_> = m.grid().iter_cells().take(1200).collect();
        for cell in blocked {
            m.set_blocked(cell, true);
        }
        let points = scatter(&m, &rule(10.0, 0.0), &mut ChaCha8Rng::seed_from_u64(4));
        assert!(points.iter().all(|p| !m.is_blocked(p.cell)));
    }

    #[test]
    fn honours_the_elevation_band() {
        let mut m = map();
        let cells: Vec<_> = m.grid().iter_cells().collect();
        for (i, cell) in cells.into_iter().enumerate() {
            m.set(
                cell,
                TerrainCell {
                    elevation: (i % 256) as u8,
                    ..TerrainCell::default()
                },
            );
        }
        let mut r = rule(5.0, 0.0);
        r.elevation_range = (200, 255);
        let points = scatter(&m, &r, &mut ChaCha8Rng::seed_from_u64(5));
        assert!(points.iter().all(|p| m.elevation(p.cell) >= 200));
    }

    #[test]
    fn is_reproducible_and_stably_ordered() {
        let run = || scatter(&map(), &rule(3.0, 1.5), &mut ChaCha8Rng::seed_from_u64(6));
        let points = run();
        assert_eq!(points, run());
        assert!(
            points
                .windows(2)
                .all(|w| (w[0].cell.y, w[0].cell.x) <= (w[1].cell.y, w[1].cell.x))
        );
    }

    #[test]
    fn zero_density_places_nothing() {
        assert!(scatter(&map(), &rule(0.0, 0.0), &mut ChaCha8Rng::seed_from_u64(7)).is_empty());
    }
}

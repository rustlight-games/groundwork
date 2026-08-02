//! Terrain generators.

use bw_content::registry::{GeneratorRegistry, TerrainGenerator};
use bw_content::terrain::{NORMAL_COST, TerrainCell, TerrainGenContext, TerrainMap};
use bw_content::{ContentError, ContentResult, Params};
use bw_core::GridPos;
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Register every terrain generator.
pub fn register_generators(registry: &mut GeneratorRegistry) {
    registry.add_terrain(RollingHills);
}

/// Open ground with soft elevation, patches of rough going, and a scattering of
/// impassable outcrops.
///
/// Skeleton implementation: value noise smoothed a few times, which is enough
/// to exercise every downstream consumer — costs, density, elevation, blocking
/// — without pretending to be a finished world generator.
///
/// Parameters: `roughness` (0..1, default 0.35), `outcrop_chance` (0..1,
/// default 0.04), `smoothing` (integer passes, default 3).
pub struct RollingHills;

impl TerrainGenerator for RollingHills {
    fn key(&self) -> &'static str {
        "rolling_hills"
    }

    fn validate(&self, params: &Params) -> ContentResult<()> {
        for key in ["roughness", "outcrop_chance"] {
            if params.contains(key) {
                let value = params.real("rolling_hills", key)?.to_num::<f64>();
                if !(0.0..=1.0).contains(&value) {
                    return Err(ContentError::Invalid {
                        context: "rolling_hills".into(),
                        message: format!("{key} must be between 0 and 1, found {value}"),
                    });
                }
            }
        }
        Ok(())
    }

    fn generate(&self, ctx: &TerrainGenContext<'_>, rng: &mut ChaCha8Rng, out: &mut TerrainMap) {
        let roughness = ctx
            .params
            .real_or("rolling_hills", "roughness", num(0.35))
            .unwrap_or(num(0.35));
        let outcrop = ctx
            .params
            .real_or("rolling_hills", "outcrop_chance", num(0.04))
            .unwrap_or(num(0.04));
        let smoothing = ctx
            .params
            .int_or("rolling_hills", "smoothing", 3)
            .unwrap_or(3)
            .max(0);

        let width = ctx.grid.dims.width as usize;
        let height = ctx.grid.dims.height as usize;
        let mut field: Vec<u8> = (0..width * height)
            .map(|_| rng.random_range(0..=255u8))
            .collect();

        // Box-blur the noise a few times. Cheap, and turns white noise into
        // something with recognisable landforms.
        for _ in 0..smoothing {
            field = smooth(&field, width, height);
        }

        let roughness_cutoff = (255.0 * (1.0 - roughness.to_num::<f64>())) as u8;
        let outcrop_cutoff = outcrop.to_num::<f64>();

        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let pos = GridPos::new(x, y);
                let elevation = field[(y as usize) * width + (x as usize)];

                let rough = elevation >= roughness_cutoff;
                let is_outcrop = rough && rng.random_range(0.0..1.0f64) < outcrop_cutoff;

                out.set(
                    pos,
                    TerrainCell {
                        tile: u16::from(rough),
                        // Rough ground costs more to cross but is still passable;
                        // only outcrops block, so the map cannot fragment into
                        // unreachable pockets.
                        move_cost: if rough { NORMAL_COST * 2 } else { NORMAL_COST },
                        blocked: is_outcrop,
                        // Grass thins out on high, rough ground.
                        grass_density: if is_outcrop {
                            0
                        } else if rough {
                            90
                        } else {
                            200u8.saturating_sub(elevation / 4)
                        },
                        elevation,
                    },
                );
            }
        }
    }
}

fn num(v: f64) -> bw_core::Real {
    bw_core::Real::from_num(v)
}

/// Average each cell with its neighbours, clamping at the edges.
fn smooth(field: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; field.len()];
    for y in 0..height {
        for x in 0..width {
            let mut total = 0u32;
            let mut count = 0u32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    total += field[(ny as usize) * width + (nx as usize)] as u32;
                    count += 1;
                }
            }
            out[y * width + x] = (total / count.max(1)) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use bw_core::{Grid, GridDims, real_from_int};
    use rand::SeedableRng;

    use super::*;

    fn generate(seed: u64, params: &Params) -> TerrainMap {
        let grid = Grid::centered(GridDims::new(32, 32), real_from_int(1));
        let mut map = TerrainMap::new(grid);
        let ctx = TerrainGenContext {
            grid,
            params,
            salt: 0,
        };
        RollingHills.generate(&ctx, &mut ChaCha8Rng::seed_from_u64(seed), &mut map);
        map
    }

    #[test]
    fn the_same_seed_produces_the_same_map() {
        let params = Params::new();
        let a = generate(7, &params);
        let b = generate(7, &params);
        for cell in a.grid().iter_cells() {
            assert_eq!(a.move_cost(cell), b.move_cost(cell));
            assert_eq!(a.elevation(cell), b.elevation(cell));
            assert_eq!(a.is_blocked(cell), b.is_blocked(cell));
        }
    }

    #[test]
    fn different_seeds_produce_different_maps() {
        let params = Params::new();
        let a = generate(1, &params);
        let b = generate(2, &params);
        assert!(
            a.grid()
                .iter_cells()
                .any(|c| a.elevation(c) != b.elevation(c))
        );
    }

    #[test]
    fn most_of_the_map_stays_walkable() {
        // A generator that blocks everything is deterministic and useless.
        let map = generate(3, &Params::new());
        let blocked = map
            .grid()
            .iter_cells()
            .filter(|&c| map.is_blocked(c))
            .count();
        assert!(
            blocked * 4 < map.cell_count(),
            "{blocked} of {} blocked",
            map.cell_count()
        );
    }

    #[test]
    fn out_of_range_parameters_are_rejected_at_load() {
        let mut params = Params::new();
        params.insert("roughness", bw_content::Value::Num(4.0));
        assert!(RollingHills.validate(&params).is_err());
        assert!(RollingHills.validate(&Params::new()).is_ok());
    }

    #[test]
    fn smoothing_reduces_variation_between_neighbours() {
        let noisy: Vec<u8> = (0..64).map(|i| if i % 2 == 0 { 0 } else { 255 }).collect();
        let smoothed = smooth(&noisy, 8, 8);
        let spread = |v: &[u8]| {
            v.windows(2)
                .map(|w| w[0].abs_diff(w[1]) as u32)
                .sum::<u32>()
        };
        assert!(spread(&smoothed) < spread(&noisy));
    }
}

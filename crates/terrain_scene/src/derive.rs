//! Filling the field stack, and the fields that follow from the ones filled.
//!
//! Two halves of building one stack, in one module because they share the grid
//! and because the second half is meaningless without the first.
//!
//! ## Sampled once, at a footprint
//!
//! [`sample_fields`] is the only place in the framework that walks
//! [`PreparedTerrain`] on a lattice. Everything downstream interpolates *this*
//! rather than asking the terrain again, and that is the whole point: two
//! consumers sampling the same path edge at their own rates disagree about where
//! it is by a fraction of a texel, which is invisible in every test and is a
//! seam in the picture.
//!
//! Each sample carries a footprint of half the grid spacing, so a mask read at a
//! lattice point antialiases against the ground it actually covers instead of
//! aliasing at a mathematical point. Without it a path edge staircases at the
//! sampling rate — and the edge is the one thing on the plate the eye is
//! guaranteed to look at.
//!
//! ## Derived once, for the same reason
//!
//! Slope, curvature, exposure and flow are computed here and carried, rather
//! than recomputed by each recipe and each renderer. The characteristic failure
//! is not that one of them is wrong; it is that two of them are *slightly
//! different*, so a population that thinned on a slope and a renderer that
//! shaded it disagree about where the slope was.
//!
//! Every derivation is a finite difference over the **combined** structural
//! surface — elevation plus microrelief — rather than a mixture of analytic
//! gradients and differenced ones. The analytic microrelief gradient is more
//! accurate in isolation and would be the wrong choice: the renderer differences
//! the grid, so a placement derived analytically would sit on a surface with a
//! slightly different normal from the one it is drawn against.
//!
//! ## Determinism under threads
//!
//! Rows are sampled in parallel and stitched in index order, so the result does
//! not depend on how many cores ran it. The flow solver orders its cells by
//! height with the sample index as the tie-break, for the same reason: two cells
//! at exactly equal height are common on ground that is mostly flat, and without
//! a stated tie-break they resolve in whatever order the sort happened to leave
//! them.

use rayon::prelude::*;

use terrain_core::ids::MaterialIndex;
use terrain_core::prepare::PreparedTerrain;
use terrain_core::sample::{SampleChannels, SampleFootprint, SampleQuery};

use crate::field::{
    DerivedFieldSet, FieldDescriptor, FieldGridSpec, FieldUnit, MaterialPlane, ModifierPlane,
    ScalarPlane, TerrainFieldStack, VectorPlane,
};

/// Which derived fields to compute.
///
/// Requested rather than always-on, because they are not all cheap and not all
/// wanted. The exposure scan is eight rays per sample and a corpus job that
/// never reads it should not pay for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DerivedFieldRequest {
    /// Ground normal, slope and aspect.
    pub surface: bool,
    /// Mean curvature: hollows and crests.
    pub curvature: bool,
    /// Sky exposure, by horizon scan.
    pub exposure: bool,
    /// Flow accumulation and direction.
    pub flow: bool,
    /// Dominant pair, blend amount and boundary tangent.
    pub boundary: bool,
}

impl DerivedFieldRequest {
    /// Everything.
    pub const ALL: Self = Self {
        surface: true,
        curvature: true,
        exposure: true,
        flow: true,
        boundary: true,
    };

    /// Nothing.
    pub const NONE: Self = Self {
        surface: false,
        curvature: false,
        exposure: false,
        flow: false,
        boundary: false,
    };

    /// What placing content needs: the surface it stands on and the boundary it
    /// may be sorted by. Not the flow solver, which only wetness and debris
    /// sorting read.
    pub const PLACEMENT: Self = Self {
        surface: true,
        curvature: true,
        exposure: false,
        flow: false,
        boundary: true,
    };
}

impl Default for DerivedFieldRequest {
    fn default() -> Self {
        Self::ALL
    }
}

/// One row's worth of sampled planes, before stitching.
struct SampledRow {
    elevation: Vec<f32>,
    microrelief: Vec<f32>,
    /// One row per declared material, each `samples_across` long.
    materials: Vec<Vec<f32>>,
    /// One row per declared modifier channel.
    modifiers: Vec<Vec<f32>>,
}

/// Sample a prepared terrain onto a grid.
///
/// Substrate planes are allocated for every declared material and then **pruned
/// to those that appear**, because a document may declare twenty materials of
/// which a given nine-tile plate contains two, and a renderer iterating eighteen
/// all-zero planes per texel is the inner loop of everything.
pub fn sample_fields(prepared: &PreparedTerrain, grid: FieldGridSpec) -> TerrainFieldStack {
    let across = grid.samples_across();
    let down = grid.samples_down();
    let material_count = prepared.materials().len();
    let channel_count = prepared.channels().len();

    // Half the spacing: the radius of the ground one lattice sample stands for.
    let footprint = SampleFootprint::circle(grid.spacing_m * 0.5);

    let rows: Vec<SampledRow> = (0..down)
        .into_par_iter()
        .map(|row| {
            let mut out = SampledRow {
                elevation: vec![0.0; across],
                microrelief: vec![0.0; across],
                materials: vec![vec![0.0; across]; material_count],
                modifiers: vec![vec![0.0; across]; channel_count],
            };
            for column in 0..across {
                let at = grid.sample_position(column as u32, row as u32);
                let sample = prepared.sample(
                    &SampleQuery::at(at)
                        .with_footprint(footprint)
                        .with_channels(SampleChannels::SURFACE),
                );
                out.elevation[column] = sample.elevation_m;
                out.microrelief[column] = sample.microrelief.displacement_m;
                for weight in sample.material_weights.iter() {
                    if let Some(plane) = out.materials.get_mut(weight.material.index()) {
                        plane[column] = weight.weight;
                    }
                }
                for (index, plane) in out.modifiers.iter_mut().enumerate() {
                    plane[column] = sample
                        .modifiers
                        .as_slice()
                        .get(index)
                        .copied()
                        .unwrap_or(0.0);
                }
            }
            out
        })
        .collect();

    let count = grid.sample_count();
    let mut elevation = Vec::with_capacity(count);
    let mut microrelief = Vec::with_capacity(count);
    let mut materials = vec![Vec::with_capacity(count); material_count];
    let mut modifiers = vec![Vec::with_capacity(count); channel_count];
    for row in rows {
        elevation.extend_from_slice(&row.elevation);
        microrelief.extend_from_slice(&row.microrelief);
        for (plane, source) in materials.iter_mut().zip(row.materials.iter()) {
            plane.extend_from_slice(source);
        }
        for (plane, source) in modifiers.iter_mut().zip(row.modifiers.iter()) {
            plane.extend_from_slice(source);
        }
    }

    let substrates = materials
        .into_iter()
        .enumerate()
        .filter(|(_, values)| values.iter().any(|weight| *weight > 0.0))
        .map(|(index, values)| {
            let key = prepared
                .material_key(MaterialIndex(index as u16))
                .map(|key| format!("substrate.{key}"))
                .unwrap_or_else(|| format!("substrate.{index}"));
            MaterialPlane {
                material: MaterialIndex(index as u16),
                weights: ScalarPlane {
                    values,
                    descriptor: FieldDescriptor::scalar(key, FieldUnit::Fraction)
                        .with_range(0.0, 1.0),
                },
            }
        })
        .collect();

    let modifiers = modifiers
        .into_iter()
        .enumerate()
        .map(|(index, values)| {
            let definition = &prepared.channels()[index];
            let key = prepared
                .channel_key(terrain_core::ids::ModifierIndex(index as u16))
                .map(|key| format!("modifier.{key}"))
                .unwrap_or_else(|| format!("modifier.{index}"));
            ModifierPlane {
                channel: terrain_core::ids::ModifierIndex(index as u16),
                values: ScalarPlane {
                    values,
                    descriptor: FieldDescriptor::scalar(key, FieldUnit::Unitless)
                        .with_range(definition.range.low, definition.range.high),
                },
            }
        })
        .collect();

    TerrainFieldStack {
        elevation_m: ScalarPlane {
            values: elevation,
            descriptor: FieldDescriptor::scalar("elevation_m", FieldUnit::Metres),
        },
        microrelief_m: ScalarPlane {
            values: microrelief,
            descriptor: FieldDescriptor::scalar("microrelief_m", FieldUnit::Metres),
        },
        grid,
        substrates,
        modifiers,
        covers: Vec::new(),
        derived: DerivedFieldSet::default(),
    }
}

/// How far a horizon scan looks, in metres.
///
/// A metre and a half. This is surface-scale shelter — the hollow a tuft sits
/// in, the lee of a stone — not landscape occlusion, and a scan long enough for
/// the latter would cost proportionally more for a term nothing currently reads
/// at that range.
const EXPOSURE_REACH_M: f64 = 1.5;

/// How many directions the horizon scan takes.
const EXPOSURE_DIRECTIONS: usize = 8;

/// How many steps along each direction.
const EXPOSURE_STEPS: usize = 12;

/// Compute the requested derived fields into a stack.
///
/// Idempotent: calling it twice recomputes rather than accumulating, so a caller
/// that changed a cover depth can re-derive without rebuilding the stack.
pub fn derive_fields(stack: &mut TerrainFieldStack, request: DerivedFieldRequest) {
    let grid = stack.grid;
    let across = grid.samples_across();
    let down = grid.samples_down();
    let spacing = grid.spacing_m as f32;

    // The combined structural surface, differenced by everything below. Built
    // once rather than per derivation.
    let surface: Vec<f32> = (0..grid.sample_count())
        .map(|index| {
            stack.elevation_m.values.get(index).copied().unwrap_or(0.0)
                + stack
                    .microrelief_m
                    .values
                    .get(index)
                    .copied()
                    .unwrap_or(0.0)
        })
        .collect();

    let height_at = |column: usize, row: usize| -> f32 {
        let column = column.min(across - 1);
        let row = row.min(down - 1);
        surface[row * across + column]
    };
    // Central difference, one-sided at the border. One-sided rather than
    // wrapping or mirroring: the border of the *generated* region is inside the
    // halo, so a slightly worse gradient there is never seen, whereas a mirrored
    // one invents a ridge that is.
    let gradient_at = |column: usize, row: usize| -> [f32; 2] {
        let left = height_at(column.saturating_sub(1), row);
        let right = height_at(column + 1, row);
        let down_v = height_at(column, row.saturating_sub(1));
        let up_v = height_at(column, row + 1);
        let span_u = if column == 0 || column + 1 >= across {
            spacing
        } else {
            spacing * 2.0
        };
        let span_v = if row == 0 || row + 1 >= down {
            spacing
        } else {
            spacing * 2.0
        };
        [(right - left) / span_u, (up_v - down_v) / span_v]
    };

    if request.surface {
        let mut normal = VectorPlane::filled(
            &grid,
            FieldDescriptor::scalar("ground_normal", FieldUnit::Unitless),
            true,
        );
        let mut slope = ScalarPlane::filled(
            &grid,
            0.0,
            FieldDescriptor::scalar("slope", FieldUnit::PerMetre),
        );
        let mut aspect = ScalarPlane::filled(
            &grid,
            0.0,
            FieldDescriptor::scalar("aspect", FieldUnit::Radians),
        );
        for row in 0..down {
            for column in 0..across {
                let index = row * across + column;
                let g = gradient_at(column, row);
                // A height field's normal is (-dh/du, -dh/dv, 1), normalised.
                // Only the horizontal pair is stored; the vertical component is
                // implied and always positive.
                let length = (g[0] * g[0] + g[1] * g[1] + 1.0).sqrt();
                normal.u[index] = -g[0] / length;
                normal.v[index] = -g[1] / length;
                slope.values[index] = (g[0] * g[0] + g[1] * g[1]).sqrt();
                // Downslope: the direction water would run, which is what every
                // consumer of an aspect actually wants.
                aspect.values[index] = (-g[1]).atan2(-g[0]);
            }
        }
        stack.derived.normal = Some(normal);
        stack.derived.slope = Some(slope);
        stack.derived.aspect = Some(aspect);
    }

    if request.curvature {
        let mut curvature = ScalarPlane::filled(
            &grid,
            0.0,
            FieldDescriptor::scalar("curvature", FieldUnit::PerMetre),
        );
        let step = spacing * spacing;
        for row in 0..down {
            for column in 0..across {
                let index = row * across + column;
                let centre = height_at(column, row);
                let laplacian = height_at(column.saturating_sub(1), row)
                    + height_at(column + 1, row)
                    + height_at(column, row.saturating_sub(1))
                    + height_at(column, row + 1)
                    - 4.0 * centre;
                // Negated, so the sign reads the way the field is named:
                // negative in a hollow, positive on a crest.
                curvature.values[index] = -laplacian / step;
            }
        }
        stack.derived.curvature = Some(curvature);
    }

    if request.exposure {
        let mut exposure = ScalarPlane::filled(
            &grid,
            1.0,
            FieldDescriptor::scalar("exposure", FieldUnit::Fraction).with_range(0.0, 1.0),
        );
        let step_m = (EXPOSURE_REACH_M / EXPOSURE_STEPS as f64).max(grid.spacing_m);
        let values: Vec<f32> = (0..down)
            .into_par_iter()
            .flat_map_iter(|row| {
                (0..across).map(move |column| {
                    let origin = height_at(column, row);
                    let mut blocked = 0.0f32;
                    for direction in 0..EXPOSURE_DIRECTIONS {
                        let angle =
                            std::f64::consts::TAU * direction as f64 / EXPOSURE_DIRECTIONS as f64;
                        let (du, dv) = (angle.cos(), angle.sin());
                        let mut highest = 0.0f32;
                        for step in 1..=EXPOSURE_STEPS {
                            let distance = step as f64 * step_m;
                            let cu = column as f64 + du * distance / grid.spacing_m;
                            let cv = row as f64 + dv * distance / grid.spacing_m;
                            if cu < 0.0 || cv < 0.0 {
                                break;
                            }
                            let (cu, cv) = (cu as usize, cv as usize);
                            if cu >= across || cv >= down {
                                break;
                            }
                            let rise = height_at(cu, cv) - origin;
                            if rise > 0.0 {
                                // The tangent of the horizon angle in this
                                // direction, kept as a tangent for the same
                                // reason slope is.
                                highest = highest.max(rise / distance as f32);
                            }
                        }
                        // A horizon at 45 degrees blocks half the sky in that
                        // direction; the mapping is deliberately gentle,
                        // because this is shelter rather than an occlusion
                        // term a renderer would multiply light by.
                        blocked += highest / (1.0 + highest);
                    }
                    (1.0 - blocked / EXPOSURE_DIRECTIONS as f32).clamp(0.0, 1.0)
                })
            })
            .collect();
        exposure.values = values;
        stack.derived.exposure = Some(exposure);
    }

    if request.flow {
        let (accumulation, direction) = solve_flow(&surface, &grid);
        stack.derived.flow_accumulation = Some(accumulation);
        stack.derived.flow_direction = Some(direction);
    }

    if request.boundary {
        derive_boundary(stack, &grid);
    }
}

/// Flow accumulation and direction over the structural surface.
///
/// Multiple-flow-direction rather than steepest-descent: a D8 solver routes all
/// of a cell's water down one of eight compass directions, which on nearly flat
/// ground produces hard channels along the lattice axes — an artefact that reads
/// as a grid in anything the field is used to sort.
///
/// Deterministic under threads because the order is stated: descending height,
/// with the sample index breaking exact ties. On terrain that is mostly flat
/// exact ties are the common case rather than the rare one, so the tie-break is
/// load-bearing rather than defensive.
fn solve_flow(surface: &[f32], grid: &FieldGridSpec) -> (ScalarPlane, VectorPlane) {
    let across = grid.samples_across();
    let down = grid.samples_down();
    let count = grid.sample_count();
    let cell_area = (grid.spacing_m * grid.spacing_m) as f32;

    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|a, b| {
        surface[*b]
            .partial_cmp(&surface[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });

    let mut accumulation = vec![cell_area; count];
    let mut flow_u = vec![0.0f32; count];
    let mut flow_v = vec![0.0f32; count];

    // The eight neighbours, with their distances in cells.
    const NEIGHBOURS: [(i64, i64); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    let mut weights = [0.0f32; 8];
    for &index in &order {
        let column = (index % across) as i64;
        let row = (index / across) as i64;
        let height = surface[index];
        let mut total = 0.0f32;
        for (slot, (dx, dy)) in NEIGHBOURS.iter().enumerate() {
            let nx = column + dx;
            let ny = row + dy;
            weights[slot] = 0.0;
            if nx < 0 || ny < 0 || nx >= across as i64 || ny >= down as i64 {
                continue;
            }
            let neighbour = ny as usize * across + nx as usize;
            let drop = height - surface[neighbour];
            if drop <= 0.0 {
                continue;
            }
            // Distance-corrected, so a diagonal is not treated as though it
            // were a cell away. Without the correction the field develops a
            // bias toward the diagonals that shows as a forty-five degree
            // grain.
            let distance = if *dx != 0 && *dy != 0 {
                std::f32::consts::SQRT_2
            } else {
                1.0
            };
            let weight = (drop / distance).powf(1.1);
            weights[slot] = weight;
            total += weight;
        }
        if total <= 0.0 {
            continue;
        }
        let share = accumulation[index];
        for (slot, (dx, dy)) in NEIGHBOURS.iter().enumerate() {
            if weights[slot] <= 0.0 {
                continue;
            }
            let fraction = weights[slot] / total;
            let neighbour = (row + dy) as usize * across + (column + dx) as usize;
            accumulation[neighbour] += share * fraction;
            let length = ((dx * dx + dy * dy) as f32).sqrt();
            flow_u[index] += fraction * *dx as f32 / length;
            flow_v[index] += fraction * *dy as f32 / length;
        }
    }

    (
        ScalarPlane {
            values: accumulation,
            descriptor: FieldDescriptor::scalar("flow_accumulation", FieldUnit::Unitless)
                // Accumulated area runs to the size of the region, so a tenth of
                // a square millimetre of digest resolution is meaningless here.
                .with_steps(100.0),
        },
        VectorPlane {
            u: flow_u,
            v: flow_v,
            descriptor: FieldDescriptor::scalar("flow_direction", FieldUnit::Unitless),
            unit_length: true,
        },
    )
}

/// The dominant substrate pair, how mixed it is, and which way the boundary runs.
fn derive_boundary(stack: &mut TerrainFieldStack, grid: &FieldGridSpec) {
    let count = grid.sample_count();
    let across = grid.samples_across();
    let down = grid.samples_down();

    let mut dominant = vec![-1.0f32; count];
    let mut secondary = vec![-1.0f32; count];
    let mut blend = vec![0.0f32; count];

    for index in 0..count {
        let mut best = (-1.0f32, -1.0f32);
        let mut runner = (-1.0f32, -1.0f32);
        for plane in &stack.substrates {
            let weight = plane.weights.values.get(index).copied().unwrap_or(0.0);
            let entry = (weight, plane.material.0 as f32);
            if weight > best.0 {
                runner = best;
                best = entry;
            } else if weight > runner.0 {
                runner = entry;
            }
        }
        if best.0 > 0.0 {
            dominant[index] = best.1;
            // The same mapping [`terrain_core::MaterialWeightSet::blend`] uses:
            // zero where one material owns the ground, one at an even split.
            // Stated the same way in both places on purpose — a boundary that
            // measured "mixed" differently in the matrix and in a point sample
            // would put the transition in two places.
            blend[index] = ((1.0 - best.0) * 2.0).clamp(0.0, 1.0);
        }
        if runner.0 > 0.0 {
            secondary[index] = runner.1;
        }
    }

    // The boundary frame comes from whichever substrate is changing fastest
    // here. Its gradient points toward more of that substrate, so the outward
    // normal is the negative of it, and the tangent runs along the edge.
    let mut tangent = VectorPlane::filled(
        grid,
        FieldDescriptor::scalar("boundary_tangent", FieldUnit::Unitless),
        true,
    );
    let spacing = grid.spacing_m as f32;
    for row in 0..down {
        for column in 0..across {
            let index = row * across + column;
            let mut strongest = 0.0f32;
            let mut chosen = [0.0f32, 0.0f32];
            for plane in &stack.substrates {
                let at = |c: usize, r: usize| -> f32 {
                    let c = c.min(across - 1);
                    let r = r.min(down - 1);
                    plane
                        .weights
                        .values
                        .get(r * across + c)
                        .copied()
                        .unwrap_or(0.0)
                };
                let du =
                    (at(column + 1, row) - at(column.saturating_sub(1), row)) / (2.0 * spacing);
                let dv =
                    (at(column, row + 1) - at(column, row.saturating_sub(1))) / (2.0 * spacing);
                let magnitude = (du * du + dv * dv).sqrt();
                if magnitude > strongest {
                    strongest = magnitude;
                    chosen = [du, dv];
                }
            }
            if strongest > 0.0 {
                // Perpendicular to the gradient: along the edge rather than
                // across it.
                tangent.u[index] = -chosen[1] / strongest;
                tangent.v[index] = chosen[0] / strongest;
            }
        }
    }

    stack.derived.dominant_material = Some(ScalarPlane {
        values: dominant,
        descriptor: FieldDescriptor::categorical("dominant_material"),
    });
    stack.derived.secondary_material = Some(ScalarPlane {
        values: secondary,
        descriptor: FieldDescriptor::categorical("secondary_material"),
    });
    stack.derived.blend = Some(ScalarPlane {
        values: blend,
        descriptor: FieldDescriptor::scalar("blend", FieldUnit::Fraction).with_range(0.0, 1.0),
    });
    stack.derived.boundary_tangent = Some(tangent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::FieldGridSpec;
    use terrain_core::coords::{WorldPoint, WorldRect};

    fn grid(side_m: f64, spacing: f64) -> FieldGridSpec {
        FieldGridSpec::covering(
            WorldRect::new(WorldPoint::ORIGIN, WorldPoint::new(side_m, side_m)),
            spacing,
        )
    }

    /// A stack with a stated surface, for testing derivations without a
    /// document in the loop.
    fn stack_with_surface(
        grid: FieldGridSpec,
        height: impl Fn(f64, f64) -> f32,
    ) -> TerrainFieldStack {
        let mut stack = TerrainFieldStack::flat(grid);
        for row in 0..grid.samples_down() {
            for column in 0..grid.samples_across() {
                let at = grid.sample_position(column as u32, row as u32);
                stack.elevation_m.values[row * grid.samples_across() + column] =
                    height(at.u_m, at.v_m);
            }
        }
        stack
    }

    #[test]
    fn a_flat_surface_has_no_slope_and_full_exposure() {
        let grid = grid(2.0, 0.25);
        let mut stack = stack_with_surface(grid, |_, _| 0.0);
        derive_fields(&mut stack, DerivedFieldRequest::ALL);
        let slope = stack.derived.slope.as_ref().expect("slope");
        assert!(slope.values.iter().all(|s| s.abs() < 1.0e-6));
        let exposure = stack.derived.exposure.as_ref().expect("exposure");
        assert!(exposure.values.iter().all(|e| (*e - 1.0).abs() < 1.0e-6));
    }

    #[test]
    fn a_ramp_has_the_slope_it_was_built_with() {
        // A one-in-four ramp along +u.
        let grid = grid(2.0, 0.25);
        let mut stack = stack_with_surface(grid, |u, _| (u * 0.25) as f32);
        derive_fields(&mut stack, DerivedFieldRequest::PLACEMENT);
        let slope = stack.derived.slope.as_ref().expect("slope");
        // Sample away from the border, where the difference is central.
        let index = 3 * grid.samples_across() + 3;
        assert!(
            (slope.values[index] - 0.25).abs() < 1.0e-4,
            "slope was {}",
            slope.values[index]
        );
        // And the normal leans away from the rise.
        let normal = stack.derived.normal.as_ref().expect("normal");
        assert!(normal.u[index] < 0.0, "normal should lean down-slope");
    }

    #[test]
    fn curvature_is_negative_in_a_hollow_and_positive_on_a_crest() {
        let grid = grid(2.0, 0.1);
        let centre = 1.0;
        // A bowl: height rises with distance from the middle.
        let mut bowl = stack_with_surface(grid, |u, v| {
            (((u - centre).powi(2) + (v - centre).powi(2)) * 0.5) as f32
        });
        derive_fields(&mut bowl, DerivedFieldRequest::PLACEMENT);
        let index = grid.samples_down() / 2 * grid.samples_across() + grid.samples_across() / 2;
        let hollow = bowl.derived.curvature.as_ref().expect("curvature").values[index];
        assert!(hollow < 0.0, "a hollow should read negative, got {hollow}");

        let mut mound = stack_with_surface(grid, |u, v| {
            (-((u - centre).powi(2) + (v - centre).powi(2)) * 0.5) as f32
        });
        derive_fields(&mut mound, DerivedFieldRequest::PLACEMENT);
        let crest = mound.derived.curvature.as_ref().expect("curvature").values[index];
        assert!(crest > 0.0, "a crest should read positive, got {crest}");
    }

    #[test]
    fn a_hollow_is_more_sheltered_than_a_crest() {
        let grid = grid(3.0, 0.1);
        let centre = 1.5;
        let mut bowl = stack_with_surface(grid, |u, v| {
            (((u - centre).powi(2) + (v - centre).powi(2)) * 0.4) as f32
        });
        derive_fields(&mut bowl, DerivedFieldRequest::ALL);
        let middle = grid.samples_down() / 2 * grid.samples_across() + grid.samples_across() / 2;
        let sheltered = bowl.derived.exposure.as_ref().expect("exposure").values[middle];
        assert!(
            sheltered < 0.95,
            "the bottom of a bowl should be sheltered, got {sheltered}"
        );
    }

    #[test]
    fn flow_runs_downhill_and_conserves_what_it_started_with() {
        let grid = grid(2.0, 0.1);
        let mut stack = stack_with_surface(grid, |u, _| (1.0 - u * 0.3) as f32);
        derive_fields(&mut stack, DerivedFieldRequest::ALL);

        let flow = stack.derived.flow_accumulation.as_ref().expect("flow");
        // Every cell starts with its own area and passes it downhill, so the
        // low edge must hold more than the high one.
        let across = grid.samples_across();
        let row = grid.samples_down() / 2;
        let high = flow.values[row * across + 1];
        let low = flow.values[row * across + across - 2];
        assert!(
            low > high,
            "flow should accumulate downhill: {high} -> {low}"
        );

        // The direction points the way the ground falls: +u here.
        let direction = stack.derived.flow_direction.as_ref().expect("direction");
        assert!(direction.u[row * across + across / 2] > 0.5);
    }

    #[test]
    fn the_dominant_pair_and_blend_follow_the_substrate_weights() {
        use crate::field::{FieldDescriptor, FieldUnit, MaterialPlane, ScalarPlane};
        use terrain_core::ids::MaterialIndex;

        let grid = grid(1.0, 0.5);
        let count = grid.sample_count();
        let mut stack = TerrainFieldStack::flat(grid);
        // An even split everywhere between materials 0 and 1.
        for material in 0..2u16 {
            stack.substrates.push(MaterialPlane {
                material: MaterialIndex(material),
                weights: ScalarPlane {
                    values: vec![0.5; count],
                    descriptor: FieldDescriptor::scalar(
                        format!("substrate.{material}"),
                        FieldUnit::Fraction,
                    ),
                },
            });
        }
        derive_fields(&mut stack, DerivedFieldRequest::PLACEMENT);

        let blend = stack.derived.blend.as_ref().expect("blend");
        // An even split is maximally mixed.
        assert!(blend.values.iter().all(|b| (*b - 1.0).abs() < 1.0e-5));
        let dominant = stack.derived.dominant_material.as_ref().expect("dominant");
        let secondary = stack
            .derived
            .secondary_material
            .as_ref()
            .expect("secondary");
        assert!(dominant.values.iter().all(|d| *d >= 0.0));
        assert!(secondary.values.iter().all(|s| *s >= 0.0));
        assert!(
            dominant.values[0] != secondary.values[0],
            "a pair must name two different materials"
        );
    }

    #[test]
    fn deriving_twice_gives_the_same_answer() {
        // Idempotence, so a caller that changed a cover and re-derived does not
        // accumulate anything.
        let grid = grid(2.0, 0.2);
        let mut stack = stack_with_surface(grid, |u, v| ((u * 0.2) + (v * 0.1)) as f32);
        derive_fields(&mut stack, DerivedFieldRequest::ALL);
        let once = stack.fingerprint();
        derive_fields(&mut stack, DerivedFieldRequest::ALL);
        assert_eq!(once, stack.fingerprint());
    }
}

//! The ground, sampled onto an analysis lattice.
//!
//! Everything downstream — topography, spectrum, semivariogram, optics — reads
//! this one structure rather than calling the evaluator itself. Two reasons, and
//! the second is the one that matters:
//!
//! - a metric that sampled the evaluator on its own schedule would measure a
//!   slightly different surface from the one beside it, and the whole point of
//!   running six analyses is that they describe the same ground;
//! - the lattice is **integer-addressed**, so a plate analysed as one window and
//!   as four quadrants reads the same world points. That is what makes the
//!   composability gate a comparison rather than an interpolation.
//!
//! ## Why the margin is excluded rather than the window made larger
//!
//! Roughness statistics near the edge of a sampled field are wrong in a specific
//! way: the derivative estimates run off the end, and the spectral window sees a
//! discontinuity that was never in the terrain. Making the window larger does not
//! fix it — it moves the bad ring outward. So the field carries an explicit
//! analysis margin, every scalar statistic is taken over the interior, and the
//! margin is sized from the coarsest feature being analysed rather than picked.

use glam::Vec2;
use terrain_core::coords::{CellCoord, WorldPoint, WorldRect};
use terrain_generators::ground::{GroundEvaluator, GroundSample};

/// Where the analysis lattice sits, addressed by integer.
///
/// The same discipline as `terrain_scene::field::FieldGridSpec`: a grid is a
/// world origin *in cells* plus a spacing, so two windows that overlap sample
/// identical points rather than points that are nearly the same.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalysisGrid {
    /// The lattice cell the sample at `(0, 0)` sits on.
    pub origin_index: CellCoord,
    pub spacing_m: f64,
    pub columns: usize,
    pub rows: usize,
}

impl AnalysisGrid {
    /// A grid covering `bounds`, snapped outward to the global lattice.
    ///
    /// Snapped rather than started at the rectangle's corner, because a corner
    /// is an arbitrary real number and two windows asking for overlapping
    /// rectangles would then sample two interleaved lattices that never
    /// coincide.
    pub fn covering(bounds: WorldRect, spacing_m: f64) -> Self {
        let low_u = (bounds.min.u_m / spacing_m).floor() as i64;
        let low_v = (bounds.min.v_m / spacing_m).floor() as i64;
        let high_u = (bounds.max.u_m / spacing_m).ceil() as i64;
        let high_v = (bounds.max.v_m / spacing_m).ceil() as i64;
        Self {
            origin_index: CellCoord::new(low_u, low_v),
            spacing_m,
            columns: (high_u - low_u + 1).max(1) as usize,
            rows: (high_v - low_v + 1).max(1) as usize,
        }
    }

    /// A square grid of a given side, snapped to the lattice.
    pub fn square(centre: WorldPoint, side_m: f64, spacing_m: f64) -> Self {
        Self::covering(WorldRect::centred(centre, side_m), spacing_m)
    }

    pub fn sample_count(&self) -> usize {
        self.columns * self.rows
    }

    /// The world position of sample `(column, row)`.
    pub fn position(&self, column: usize, row: usize) -> WorldPoint {
        WorldPoint::new(
            (self.origin_index.x + column as i64) as f64 * self.spacing_m,
            (self.origin_index.y + row as i64) as f64 * self.spacing_m,
        )
    }

    /// The ground this grid covers.
    pub fn bounds(&self) -> WorldRect {
        WorldRect::new(
            self.position(0, 0),
            self.position(self.columns - 1, self.rows - 1),
        )
    }

    /// Whether two grids address the same lattice, so their overlap can be
    /// compared sample for sample.
    pub fn shares_lattice_with(&self, other: &Self) -> bool {
        (self.spacing_m - other.spacing_m).abs() < 1.0e-12
    }
}

/// Every plane the benchmark measures, over one grid.
///
/// Row-major, `rows × columns`. Held as separate planes rather than as a vector
/// of samples because every analysis walks one channel at a time, and a struct
/// of arrays is what a strided read wants.
#[derive(Clone, Debug)]
pub struct GroundField {
    pub grid: AnalysisGrid,
    /// Final surface height including profile relief, metres.
    pub height_m: Vec<f32>,
    /// Profile geometry displacement alone, metres.
    pub displacement_m: Vec<f32>,
    pub cavity: Vec<f32>,
    pub moisture: Vec<f32>,
    pub compaction: Vec<f32>,
    pub wet_film: Vec<f32>,
    /// Realised weight per material index, one plane each.
    pub weights: Vec<Vec<f32>>,
    /// How much each point supports plants.
    pub vegetation_support: Vec<f32>,
}

impl GroundField {
    /// Sample an evaluator over a grid.
    pub fn sample(ground: &GroundEvaluator, grid: AnalysisGrid, materials: usize) -> Self {
        let count = grid.sample_count();
        let mut field = Self {
            grid,
            height_m: Vec::with_capacity(count),
            displacement_m: Vec::with_capacity(count),
            cavity: Vec::with_capacity(count),
            moisture: Vec::with_capacity(count),
            compaction: Vec::with_capacity(count),
            wet_film: Vec::with_capacity(count),
            weights: vec![Vec::with_capacity(count); materials],
            vegetation_support: Vec::with_capacity(count),
        };
        for row in 0..grid.rows {
            for column in 0..grid.columns {
                let at = grid.position(column, row);
                let flat = Vec2::new(at.u_m as f32, at.v_m as f32);
                let sample: GroundSample = ground.sample(flat);
                field.height_m.push(ground.final_surface_z_m(flat));
                field.displacement_m.push(sample.displacement_m);
                field.cavity.push(sample.cavity);
                field.moisture.push(sample.state.moisture);
                field.compaction.push(sample.state.compaction);
                field.wet_film.push(sample.wet_film);
                field.vegetation_support.push(sample.vegetation_support);
                for (index, plane) in field.weights.iter_mut().enumerate() {
                    plane.push(
                        sample
                            .substrates
                            .weight_of(terrain_core::ids::MaterialIndex(index as u16)),
                    );
                }
            }
        }
        field
    }

    /// The interior of a plane, with `margin` samples removed from each edge.
    ///
    /// Returns the values and the interior's own dimensions. Statistics taken
    /// over the whole plane include a ring where the derivative estimates ran
    /// off the end and the spectral window saw a discontinuity the terrain never
    /// had; excluding it is cheaper and more honest than correcting for it.
    pub fn interior(plane: &[f32], grid: &AnalysisGrid, margin: usize) -> (Vec<f32>, usize, usize) {
        if margin * 2 >= grid.columns || margin * 2 >= grid.rows {
            return (plane.to_vec(), grid.columns, grid.rows);
        }
        let columns = grid.columns - margin * 2;
        let rows = grid.rows - margin * 2;
        let mut out = Vec::with_capacity(columns * rows);
        for row in margin..grid.rows - margin {
            let start = row * grid.columns + margin;
            out.extend_from_slice(&plane[start..start + columns]);
        }
        (out, columns, rows)
    }

    /// The analysis margin a set of wavelengths implies, in samples.
    ///
    /// At least one coarsest wavelength, so no statistic is taken over a point
    /// whose neighbourhood was clamped. Derived rather than chosen, because a
    /// constant margin is either wasteful on a fine lattice or insufficient on a
    /// coarse one.
    pub fn margin_for(spacing_m: f64, coarsest_wavelength_m: f64) -> usize {
        ((coarsest_wavelength_m / spacing_m).ceil() as usize).max(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> AnalysisGrid {
        AnalysisGrid::covering(
            WorldRect::new(WorldPoint::new(0.0, 0.0), WorldPoint::new(1.0, 1.0)),
            0.25,
        )
    }

    #[test]
    fn two_overlapping_windows_address_the_same_lattice() {
        // The property the composability gate rests on. If the grids were
        // started at their own rectangles' corners, an overlap would sample two
        // interleaved lattices that never coincide and every comparison would be
        // an interpolation error rather than a measurement.
        let left = AnalysisGrid::covering(
            WorldRect::new(WorldPoint::new(0.13, 0.0), WorldPoint::new(1.0, 1.0)),
            0.25,
        );
        let right = AnalysisGrid::covering(
            WorldRect::new(WorldPoint::new(0.37, 0.0), WorldPoint::new(1.4, 1.0)),
            0.25,
        );
        assert!(left.shares_lattice_with(&right));
        // Every sample of one that lies inside the other's span is at an exact
        // position the other also samples.
        for column in 0..left.columns {
            let at = left.position(column, 0);
            let offset = (at.u_m - right.position(0, 0).u_m) / right.spacing_m;
            assert!(
                (offset - offset.round()).abs() < 1.0e-9,
                "{at:?} is not on the right window's lattice"
            );
        }
    }

    #[test]
    fn a_grid_snaps_outward_so_it_covers_what_was_asked_for() {
        let grid = AnalysisGrid::covering(
            WorldRect::new(WorldPoint::new(0.1, 0.1), WorldPoint::new(0.9, 0.9)),
            0.25,
        );
        let bounds = grid.bounds();
        assert!(bounds.min.u_m <= 0.1 + 1.0e-9);
        assert!(bounds.max.u_m >= 0.9 - 1.0e-9);
    }

    #[test]
    fn the_interior_drops_a_ring_of_the_requested_width() {
        let grid = grid();
        let plane: Vec<f32> = (0..grid.sample_count()).map(|i| i as f32).collect();
        let (interior, columns, rows) = GroundField::interior(&plane, &grid, 1);
        assert_eq!(columns, grid.columns - 2);
        assert_eq!(rows, grid.rows - 2);
        assert_eq!(interior.len(), columns * rows);
        // The first interior value is the sample one in from each edge.
        assert_eq!(interior[0], (grid.columns + 1) as f32);
    }

    #[test]
    fn a_margin_that_would_leave_nothing_returns_the_whole_plane() {
        // Rather than an empty vector, which every downstream statistic would
        // then report as zero — indistinguishable from a perfectly flat ground.
        let grid = grid();
        let plane: Vec<f32> = vec![1.0; grid.sample_count()];
        let (interior, columns, rows) = GroundField::interior(&plane, &grid, 99);
        assert_eq!((columns, rows), (grid.columns, grid.rows));
        assert_eq!(interior.len(), plane.len());
    }

    #[test]
    fn the_margin_is_at_least_one_coarsest_wavelength() {
        assert_eq!(GroundField::margin_for(0.01, 0.05), 5);
        // And never less than two samples, however fine the feature.
        assert_eq!(GroundField::margin_for(0.01, 0.001), 2);
    }
}

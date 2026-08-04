//! The surface everything stands on, sampled onto a grid.
//!
//! ## Why the ground is a grid when the terrain is a function
//!
//! [`terrain_core::PreparedTerrain`] answers at any point. A renderer cannot ask
//! it per shading sample: Cycles would need a callback across a process
//! boundary, and a rasteriser would pay a full layer composition per pixel. So
//! the scene carries the terrain **already sampled**, on a grid, and every
//! renderer interpolates the same numbers.
//!
//! That is a lossy step and it is the right one, because the alternative is each
//! renderer sampling the terrain *itself* — at its own rate, with its own
//! filter — and the two halves of a training pair then disagreeing about where
//! the path edge is. One grid, sampled once, interpolated identically.
//!
//! ## Edge-anchored, and the reason is seams
//!
//! The lattice samples its own **corners**, not its cell centres. So a grid over
//! a rectangle shares its entire boundary row with the grid of the neighbouring
//! rectangle, and two independently built scenes agree exactly along the join.
//! Centre-anchored samples do not: the last sample of one and the first of the
//! next sit half a cell apart, and the interpolated surface between them is
//! nobody's, which is a visible ridge at every page boundary.
//!
//! The cost is one extra row and column — `columns + 1` samples across
//! `columns` cells. That is the cheapest seam insurance in the framework.
//!
//! ## Material channels are dense here even though a sample is sparse
//!
//! A [`terrain_core::MaterialWeightSet`] is a pruned list because most points are
//! one or two materials. A *grid* of them is a different problem: a renderer
//! wants to interpolate channel `k` across four neighbours, and it cannot do
//! that against four differently-shaped lists. So the grid is dense per
//! material, and materials that are zero everywhere in this region are dropped
//! wholesale rather than per texel.

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::digest::{Digest, Digestible};
use terrain_core::ids::{MaterialIndex, ModifierIndex};

/// One material's coverage across the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundMaterialChannel {
    pub material: MaterialIndex,
    /// Row-major, `(rows + 1) × (columns + 1)` samples.
    pub weights: Vec<f32>,
}

/// One modifier channel across the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundModifierChannel {
    pub channel: ModifierIndex,
    pub values: Vec<f32>,
}

/// The terrain, sampled onto a lattice.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundSurface {
    /// The world point sample `(0, 0)` sits at: the grid's minimum corner.
    pub origin: WorldPoint,
    /// Metres between lattice points.
    pub spacing_m: f64,
    /// Cells down. There are `rows + 1` samples.
    pub rows: u32,
    /// Cells across. There are `columns + 1` samples.
    pub columns: u32,
    /// Height above the datum, metres, row-major.
    pub elevation: Vec<f32>,
    /// Fine displacement from the elevation surface, metres.
    pub microrelief: Vec<f32>,
    /// Materials that appear anywhere in this region.
    pub material_channels: Vec<GroundMaterialChannel>,
    /// Declared modifier channels, in document order.
    pub modifier_channels: Vec<GroundModifierChannel>,
}

impl GroundSurface {
    /// Samples across one row.
    pub fn samples_across(&self) -> usize {
        self.columns as usize + 1
    }

    /// Samples down one column.
    pub fn samples_down(&self) -> usize {
        self.rows as usize + 1
    }

    /// Total samples in each channel.
    pub fn sample_count(&self) -> usize {
        self.samples_across() * self.samples_down()
    }

    /// The ground this grid covers.
    pub fn bounds(&self) -> WorldRect {
        WorldRect::from_size(
            self.origin,
            terrain_core::coords::WorldVector::new(
                self.columns as f64 * self.spacing_m,
                self.rows as f64 * self.spacing_m,
            ),
        )
    }

    /// The world point one lattice sample sits at.
    pub fn sample_position(&self, column: u32, row: u32) -> WorldPoint {
        WorldPoint::new(
            self.origin.u_m + column as f64 * self.spacing_m,
            self.origin.v_m + row as f64 * self.spacing_m,
        )
    }

    /// Whether every channel is the length the grid says it should be.
    ///
    /// Checked rather than assumed, because a channel one row short is a
    /// renderer reading past the end of an array or — worse, in a language that
    /// does not check — reading the next channel's first row as this one's last.
    pub fn is_well_formed(&self) -> bool {
        let expected = self.sample_count();
        self.spacing_m.is_finite()
            && self.spacing_m > 0.0
            && self.elevation.len() == expected
            && self.microrelief.len() == expected
            && self
                .material_channels
                .iter()
                .all(|channel| channel.weights.len() == expected)
            && self
                .modifier_channels
                .iter()
                .all(|channel| channel.values.len() == expected)
    }

    /// An empty surface over a region, for a scene with no ground yet.
    pub fn flat(origin: WorldPoint, spacing_m: f64, columns: u32, rows: u32) -> Self {
        let count = (columns as usize + 1) * (rows as usize + 1);
        Self {
            origin,
            spacing_m,
            rows,
            columns,
            elevation: vec![0.0; count],
            microrelief: vec![0.0; count],
            material_channels: Vec::new(),
            modifier_channels: Vec::new(),
        }
    }
}

/// Height quantisation for the digest, in steps per metre.
///
/// A tenth of a millimetre. Ground heights fall out of a long chain of
/// transcendental functions, and the last bit of one of those is arithmetic
/// noise rather than a decision anybody made — but a tenth of a millimetre over
/// relief that runs to a quarter of a metre is four parts in ten thousand, so a
/// real change cannot hide under it.
const HEIGHT_STEPS: f64 = 10_000.0;

/// Weight quantisation, in steps per unit.
const WEIGHT_STEPS: f64 = 10_000.0;

impl Digestible for GroundSurface {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .f64(self.origin.u_m)
            .f64(self.origin.v_m)
            .f64(self.spacing_m)
            .u32(self.rows)
            .u32(self.columns);
        digest.slice(&self.elevation, |d, value| {
            d.quantised(*value as f64, HEIGHT_STEPS);
        });
        digest.slice(&self.microrelief, |d, value| {
            d.quantised(*value as f64, HEIGHT_STEPS);
        });
        digest.slice(&self.material_channels, |d, channel| {
            d.u32(channel.material.0 as u32);
            d.slice(&channel.weights, |d, weight| {
                d.quantised(*weight as f64, WEIGHT_STEPS);
            });
        });
        digest.slice(&self.modifier_channels, |d, channel| {
            d.u32(channel.channel.0 as u32);
            d.slice(&channel.values, |d, value| {
                d.quantised(*value as f64, WEIGHT_STEPS);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_holds_one_more_sample_than_it_has_cells() {
        // Edge-anchored. The extra row and column are what make two
        // independently built scenes agree along a shared boundary.
        let surface = GroundSurface::flat(WorldPoint::ORIGIN, 0.25, 4, 2);
        assert_eq!(surface.samples_across(), 5);
        assert_eq!(surface.samples_down(), 3);
        assert_eq!(surface.sample_count(), 15);
        assert_eq!(surface.elevation.len(), 15);
    }

    #[test]
    fn neighbouring_grids_share_their_boundary_samples_exactly() {
        // The seam property, stated directly. The last column of one grid and
        // the first of the next are the same world points, so two scenes built
        // by different processes agree there without having to negotiate.
        let left = GroundSurface::flat(WorldPoint::ORIGIN, 0.5, 4, 4);
        let right = GroundSurface::flat(WorldPoint::new(2.0, 0.0), 0.5, 4, 4);
        for row in 0..=4 {
            let last = left.sample_position(4, row);
            let first = right.sample_position(0, row);
            assert_eq!(last, first, "row {row} does not meet");
        }
    }

    #[test]
    fn a_grids_bounds_are_the_ground_it_covers() {
        let surface = GroundSurface::flat(WorldPoint::new(-1.0, -2.0), 0.5, 4, 8);
        let bounds = surface.bounds();
        assert_eq!(bounds.min, WorldPoint::new(-1.0, -2.0));
        assert_eq!(bounds.max, WorldPoint::new(1.0, 2.0));
        // And the corner samples land on the corners.
        assert_eq!(surface.sample_position(0, 0), bounds.min);
        assert_eq!(surface.sample_position(4, 8), bounds.max);
    }

    #[test]
    fn a_channel_of_the_wrong_length_is_caught() {
        // A channel one row short is a renderer reading the next channel's first
        // row as this one's last.
        let mut surface = GroundSurface::flat(WorldPoint::ORIGIN, 0.5, 2, 2);
        assert!(surface.is_well_formed());

        surface.material_channels.push(GroundMaterialChannel {
            material: MaterialIndex(0),
            weights: vec![1.0; surface.sample_count()],
        });
        assert!(surface.is_well_formed());

        surface.material_channels[0].weights.pop();
        assert!(!surface.is_well_formed());
    }

    #[test]
    fn a_degenerate_spacing_is_caught() {
        let mut surface = GroundSurface::flat(WorldPoint::ORIGIN, 0.5, 2, 2);
        surface.spacing_m = 0.0;
        assert!(!surface.is_well_formed());
        surface.spacing_m = f64::NAN;
        assert!(!surface.is_well_formed());
    }

    #[test]
    fn the_digest_moves_with_the_surface_and_not_with_arithmetic_noise() {
        let base = GroundSurface::flat(WorldPoint::ORIGIN, 0.5, 2, 2);
        let reference = base.fingerprint("ground");

        // A micron is noise from six transcendental functions and does not move
        // the digest.
        let mut nudged = base.clone();
        nudged.elevation[3] += 1.0e-8;
        assert_eq!(reference, nudged.fingerprint("ground"));

        // A millimetre is a decision and does.
        let mut moved = base.clone();
        moved.elevation[3] += 0.001;
        assert_ne!(reference, moved.fingerprint("ground"));

        // As does the geometry of the grid itself.
        let mut resized = base.clone();
        resized.spacing_m = 0.25;
        assert_ne!(reference, resized.fingerprint("ground"));
        let mut moved_origin = base.clone();
        moved_origin.origin = WorldPoint::new(1.0, 0.0);
        assert_ne!(reference, moved_origin.fingerprint("ground"));
    }

    #[test]
    fn adding_a_channel_moves_the_digest() {
        let base = GroundSurface::flat(WorldPoint::ORIGIN, 0.5, 2, 2);
        let mut with_material = base.clone();
        with_material.material_channels.push(GroundMaterialChannel {
            material: MaterialIndex(0),
            weights: vec![1.0; base.sample_count()],
        });
        assert_ne!(
            base.fingerprint("ground"),
            with_material.fingerprint("ground")
        );

        // And the same weights under a different material are different ground.
        let mut relabelled = with_material.clone();
        relabelled.material_channels[0].material = MaterialIndex(1);
        assert_ne!(
            with_material.fingerprint("ground"),
            relabelled.fingerprint("ground")
        );
    }
}

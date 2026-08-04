//! Where things are, in metres.
//!
//! Six conventions are fixed here, and they are fixed *here* rather than being
//! rediscovered at each call site because every one of them has a plausible
//! opposite that produces a seam. A seam is the characteristic failure of a
//! terrain framework: two pieces of ground that were computed separately and
//! disagree by a pixel along the line where they meet. Nothing about a seam is
//! subtle once you see it, and nothing about the arithmetic that produces one is
//! obvious before you do.
//!
//! 1. **Metres are the unit.** Not tiles, not texels, not cells. Those are all
//!    ways of *addressing* terrain and none of them is the terrain's identity.
//! 2. **Rectangles and cells are half-open**: `min <= p < max`. So a point on a
//!    shared edge belongs to exactly one of the two rectangles that meet there,
//!    and tiling a region produces no overlaps and no gaps.
//! 3. **Division floors mathematically**, not toward zero. Rust's `/` on
//!    integers truncates, which puts `-1 / 4` and `0 / 4` in the same cell and
//!    makes cell zero twice as wide as every other cell. Every generator keyed
//!    on a cell index would then produce a visible stripe through the world
//!    origin.
//! 4. **A raster says whether its coordinates mean texel centres or texel
//!    edges.** Half a texel, every time, on every mask in the project.
//! 5. **Boundary ownership is deterministic**, and follows from (2) and (3): the
//!    cell that owns an exact boundary is the one the boundary is the *minimum*
//!    of.
//! 6. **Axis and row orientation are stated, never assumed.** `+V` is one
//!    direction in the world and image rows run in some direction on disk, and
//!    those two facts are independent.
//!
//! ## `f64` out here, `f32` only at the very end
//!
//! World positions are `f64`. That is not caution, it is arithmetic: `f32` has
//! about seven significant decimal digits, so at ten kilometres from the origin
//! its spacing is roughly a millimetre — and a millimetre is larger than the
//! detail a close-up render resolves. Terrain that quietly loses precision as
//! you walk away from the origin is terrain with a home field advantage.
//!
//! Render geometry still wants `f32`, because that is what a GPU and a mesh
//! format take. The conversion happens *after* subtracting a stable local
//! origin — see [`WorldPoint::to_local_f32`] — so the `f32` only ever carries an
//! offset of a few metres and its precision is spent where it is needed.

use std::fmt;

use crate::ids::LayerKey;

/// A point on the ground, in metres.
///
/// `u` and `v` rather than `x` and `y`, deliberately. The terrain is a
/// continuous function of a planar coordinate and has no opinion about which way
/// a camera looks at it; `x`/`y` would invite the reader to assume screen axes,
/// and the projection that turns ground into screen lives in `terrain_scene`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorldPoint {
    pub u_m: f64,
    pub v_m: f64,
}

/// A displacement on the ground, in metres.
///
/// Distinct from [`WorldPoint`] because adding two positions is meaningless and
/// the type system may as well say so. It catches the class of bug where an
/// offset is accumulated into an absolute coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorldVector {
    pub du_m: f64,
    pub dv_m: f64,
}

impl WorldPoint {
    pub const ORIGIN: Self = Self { u_m: 0.0, v_m: 0.0 };

    pub const fn new(u_m: f64, v_m: f64) -> Self {
        Self { u_m, v_m }
    }

    pub fn offset(self, by: WorldVector) -> Self {
        Self::new(self.u_m + by.du_m, self.v_m + by.dv_m)
    }

    /// The displacement from `other` to `self`.
    pub fn from_point(self, other: Self) -> WorldVector {
        WorldVector::new(self.u_m - other.u_m, self.v_m - other.v_m)
    }

    pub fn distance(self, other: Self) -> f64 {
        self.from_point(other).length()
    }

    /// This point relative to a local origin, narrowed to `f32`.
    ///
    /// The one sanctioned way world coordinates become `f32`. The subtraction
    /// has to happen in `f64` and has to happen *first*: narrowing and then
    /// subtracting gives back exactly the precision the local origin existed to
    /// buy, and the symptom is vertices that jitter as the camera moves rather
    /// than anything that looks like a numerical problem.
    pub fn to_local_f32(self, origin: Self) -> [f32; 2] {
        let offset = self.from_point(origin);
        [offset.du_m as f32, offset.dv_m as f32]
    }

    pub fn is_finite(self) -> bool {
        self.u_m.is_finite() && self.v_m.is_finite()
    }
}

impl WorldVector {
    pub const ZERO: Self = Self {
        du_m: 0.0,
        dv_m: 0.0,
    };

    pub const fn new(du_m: f64, dv_m: f64) -> Self {
        Self { du_m, dv_m }
    }

    pub fn length(self) -> f64 {
        self.du_m.hypot(self.dv_m)
    }

    pub fn scaled(self, by: f64) -> Self {
        Self::new(self.du_m * by, self.dv_m * by)
    }

    /// Unit length, or `None` for a vector too short to have a direction.
    pub fn normalised(self) -> Option<Self> {
        let length = self.length();
        (length > 0.0 && length.is_finite()).then(|| self.scaled(1.0 / length))
    }

    pub fn is_finite(self) -> bool {
        self.du_m.is_finite() && self.dv_m.is_finite()
    }
}

impl fmt::Display for WorldPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:.3}, {:.3}) m", self.u_m, self.v_m)
    }
}

/// A half-open rectangle of ground: `min <= p < max` on both axes.
///
/// Half-open is the whole design. A closed rectangle shares its edge with its
/// neighbour, so a point on the join is in both — and every quantity computed
/// per-rectangle is then computed twice there, which is how a tiled bake grows a
/// brighter line down each seam. Half-open makes tiling a partition.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorldRect {
    pub min: WorldPoint,
    pub max: WorldPoint,
}

impl WorldRect {
    /// A rectangle from two corners, in either order.
    pub fn new(a: WorldPoint, b: WorldPoint) -> Self {
        Self {
            min: WorldPoint::new(a.u_m.min(b.u_m), a.v_m.min(b.v_m)),
            max: WorldPoint::new(a.u_m.max(b.u_m), a.v_m.max(b.v_m)),
        }
    }

    /// A rectangle from a corner and a size.
    pub fn from_size(min: WorldPoint, size: WorldVector) -> Self {
        Self::new(min, min.offset(size))
    }

    /// A square of `side` metres centred on `centre`.
    pub fn centred(centre: WorldPoint, side: f64) -> Self {
        let half = side.abs() * 0.5;
        Self {
            min: WorldPoint::new(centre.u_m - half, centre.v_m - half),
            max: WorldPoint::new(centre.u_m + half, centre.v_m + half),
        }
    }

    pub fn size(self) -> WorldVector {
        self.max.from_point(self.min)
    }

    pub fn width_m(self) -> f64 {
        self.max.u_m - self.min.u_m
    }

    pub fn height_m(self) -> f64 {
        self.max.v_m - self.min.v_m
    }

    pub fn area_m2(self) -> f64 {
        self.width_m() * self.height_m()
    }

    pub fn centre(self) -> WorldPoint {
        WorldPoint::new(
            (self.min.u_m + self.max.u_m) * 0.5,
            (self.min.v_m + self.max.v_m) * 0.5,
        )
    }

    pub fn is_empty(self) -> bool {
        !(self.width_m() > 0.0 && self.height_m() > 0.0)
    }

    /// Half-open containment: `min <= p < max`.
    pub fn contains(self, point: WorldPoint) -> bool {
        point.u_m >= self.min.u_m
            && point.u_m < self.max.u_m
            && point.v_m >= self.min.v_m
            && point.v_m < self.max.v_m
    }

    /// Grown by `margin` metres on every side.
    ///
    /// The halo a bake needs so that a mark rooted just outside the rectangle
    /// still reaches into it. A negative margin shrinks, and may empty it.
    pub fn expanded(self, margin_m: f64) -> Self {
        Self {
            min: WorldPoint::new(self.min.u_m - margin_m, self.min.v_m - margin_m),
            max: WorldPoint::new(self.max.u_m + margin_m, self.max.v_m + margin_m),
        }
    }

    /// The overlap of two rectangles, or `None` if they do not meet.
    ///
    /// Two rectangles that merely *touch* do not overlap, which follows from
    /// half-openness and is what a caller wants: a shared edge is not shared
    /// area.
    pub fn intersection(self, other: Self) -> Option<Self> {
        let rect = Self {
            min: WorldPoint::new(
                self.min.u_m.max(other.min.u_m),
                self.min.v_m.max(other.min.v_m),
            ),
            max: WorldPoint::new(
                self.max.u_m.min(other.max.u_m),
                self.max.v_m.min(other.max.v_m),
            ),
        };
        (!rect.is_empty()).then_some(rect)
    }

    /// The smallest rectangle holding both.
    pub fn union(self, other: Self) -> Self {
        Self {
            min: WorldPoint::new(
                self.min.u_m.min(other.min.u_m),
                self.min.v_m.min(other.min.v_m),
            ),
            max: WorldPoint::new(
                self.max.u_m.max(other.max.u_m),
                self.max.v_m.max(other.max.v_m),
            ),
        }
    }

    pub fn is_finite(self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }
}

/// An integer cell address.
///
/// The unit of *procedural addressing*: what a candidate's random draw is keyed
/// on. Signed and 64-bit, so a world can extend in every direction without a
/// wrap and without a special case near the origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CellCoord {
    pub x: i64,
    pub y: i64,
}

impl CellCoord {
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    pub fn offset(self, dx: i64, dy: i64) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

impl fmt::Display for CellCoord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}]", self.x, self.y)
    }
}

/// A square lattice over the world, for procedural addressing.
///
/// The lattice a population scatters its candidates on. It is deliberately not
/// tied to any output resolution: a candidate's identity must not change because
/// somebody rendered at a different size, and the surest way to guarantee that
/// is for the addressing lattice to know nothing about pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CellGrid {
    /// Side of one cell, in metres. Always positive.
    cell_m: f64,
    /// World point that cell `[0, 0]`'s minimum corner sits at.
    origin: WorldPoint,
}

impl CellGrid {
    /// A lattice of `cell_m`-metre cells with cell `[0, 0]` cornered at the
    /// world origin.
    ///
    /// A non-finite or non-positive size is clamped to a small positive one
    /// rather than panicking: this is reached from authored data, and a document
    /// that says `0.0` should be reported by validation rather than by a crash
    /// several layers down.
    pub fn new(cell_m: f64) -> Self {
        Self::anchored(cell_m, WorldPoint::ORIGIN)
    }

    pub fn anchored(cell_m: f64, origin: WorldPoint) -> Self {
        Self {
            cell_m: if cell_m.is_finite() && cell_m > 0.0 {
                cell_m
            } else {
                f64::MIN_POSITIVE
            },
            origin,
        }
    }

    pub fn cell_m(self) -> f64 {
        self.cell_m
    }

    pub fn origin(self) -> WorldPoint {
        self.origin
    }

    /// Which cell owns a point.
    ///
    /// Floors, so the lattice is uniform across the world origin. Truncation
    /// toward zero — which is what `as i64` does — would make cell zero twice as
    /// wide as its neighbours and put a visible stripe through `u = 0` and
    /// `v = 0` in every population keyed on this.
    ///
    /// A point exactly on a cell boundary belongs to the cell the boundary is
    /// the *minimum* of, which is the half-open rule and is what makes
    /// [`CellGrid::cell_rect`] tile without overlap.
    pub fn cell_at(self, point: WorldPoint) -> CellCoord {
        CellCoord::new(
            floor_div(point.u_m - self.origin.u_m, self.cell_m),
            floor_div(point.v_m - self.origin.v_m, self.cell_m),
        )
    }

    /// The half-open rectangle a cell covers.
    pub fn cell_rect(self, cell: CellCoord) -> WorldRect {
        let min = WorldPoint::new(
            self.origin.u_m + cell.x as f64 * self.cell_m,
            self.origin.v_m + cell.y as f64 * self.cell_m,
        );
        WorldRect::from_size(min, WorldVector::new(self.cell_m, self.cell_m))
    }

    /// Every cell that meets `rect`, in a stable row-major order.
    ///
    /// Ordered, and the order is part of the contract. A population that walks
    /// its cells in whatever order a hash set produced would emit its marks in a
    /// different sequence run to run, and painter order is picture order wherever
    /// two marks tie.
    pub fn cells_over(self, rect: WorldRect) -> Vec<CellCoord> {
        if rect.is_empty() || !rect.is_finite() {
            return Vec::new();
        }
        let low = self.cell_at(rect.min);
        // The maximum is exclusive, so the last cell is the one containing the
        // point just inside it. Asking `cell_at(rect.max)` directly would include
        // a whole row and column of cells the rectangle only touches.
        let high = CellCoord::new(
            floor_div_exclusive(rect.max.u_m - self.origin.u_m, self.cell_m),
            floor_div_exclusive(rect.max.v_m - self.origin.v_m, self.cell_m),
        );
        let mut cells = Vec::new();
        for y in low.y..=high.y {
            for x in low.x..=high.x {
                cells.push(CellCoord::new(x, y));
            }
        }
        cells
    }
}

/// Mathematical floor division of a real by a positive real.
///
/// Saturating rather than wrapping at the extremes, because the alternative is a
/// coordinate a billion light years out silently addressing a cell next to the
/// origin.
fn floor_div(value: f64, by: f64) -> i64 {
    let quotient = (value / by).floor();
    if quotient >= i64::MAX as f64 {
        i64::MAX
    } else if quotient <= i64::MIN as f64 {
        i64::MIN
    } else {
        quotient as i64
    }
}

/// [`floor_div`] for an *exclusive* upper bound.
///
/// A rectangle ending exactly on a cell boundary does not reach into the cell
/// beyond it, so the last cell covered is one lower than the floor would say.
fn floor_div_exclusive(value: f64, by: f64) -> i64 {
    let scaled = value / by;
    let floored = scaled.floor();
    let index = if scaled == floored {
        floored - 1.0
    } else {
        floored
    };
    if index >= i64::MAX as f64 {
        i64::MAX
    } else if index <= i64::MIN as f64 {
        i64::MIN
    } else {
        index as i64
    }
}

/// Whether a raster's coordinates name texel centres or texel edges.
///
/// The half-texel that ruins a mask. A 64×64 mask over a 64-metre square either
/// samples the world at `0.5, 1.5, 2.5…` or at `0, 1, 2…`, and a framework that
/// leaves it implicit will get it one way in the sampler and the other way in
/// the debug view — where it looks like a rounding difference and is actually
/// half a texel of everything the mask controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TexelAnchor {
    /// Texel `(0, 0)` is sampled at half a texel in from the raster's corner.
    /// What an image *is*: a grid of area samples.
    #[default]
    Centre,
    /// Texel `(0, 0)` is sampled exactly at the raster's corner. What a height
    /// grid usually is: point samples on a lattice, shared between neighbours.
    Edge,
}

/// Which way image rows run against the world's `+V` axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RowOrder {
    /// Row 0 is at the rectangle's `max.v`, rows descending — the usual image
    /// convention, where the first row of a PNG is the top of the picture.
    #[default]
    TopDown,
    /// Row 0 is at the rectangle's `min.v`, rows ascending.
    BottomUp,
}

/// How a raster maps onto the world.
///
/// Everything a painted mask needs to become a function of a world point, stated
/// rather than assumed: where it sits, how big it is, whether its coordinates
/// mean centres or edges, and which way its rows run.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RasterTransform {
    /// The ground the raster covers.
    pub bounds: WorldRect,
    pub columns: u32,
    pub rows: u32,
    pub anchor: TexelAnchor,
    pub row_order: RowOrder,
}

impl RasterTransform {
    pub fn new(bounds: WorldRect, columns: u32, rows: u32) -> Self {
        Self {
            bounds,
            columns,
            rows,
            anchor: TexelAnchor::Centre,
            row_order: RowOrder::TopDown,
        }
    }

    pub fn with_anchor(mut self, anchor: TexelAnchor) -> Self {
        self.anchor = anchor;
        self
    }

    pub fn with_row_order(mut self, row_order: RowOrder) -> Self {
        self.row_order = row_order;
        self
    }

    /// Metres per texel on each axis.
    pub fn texel_size(self) -> WorldVector {
        // Edge-anchored samples sit on a lattice with one *fewer* interval than
        // it has points: 64 edge samples span 63 gaps. Centre-anchored samples
        // each own a texel, so 64 of them span 64.
        let (across, down) = match self.anchor {
            TexelAnchor::Centre => (self.columns.max(1) as f64, self.rows.max(1) as f64),
            TexelAnchor::Edge => (
                (self.columns.max(2) - 1) as f64,
                (self.rows.max(2) - 1) as f64,
            ),
        };
        WorldVector::new(
            self.bounds.width_m() / across,
            self.bounds.height_m() / down,
        )
    }

    /// The world point a texel samples.
    pub fn texel_to_world(self, column: u32, row: u32) -> WorldPoint {
        let size = self.texel_size();
        let shift = match self.anchor {
            TexelAnchor::Centre => 0.5,
            TexelAnchor::Edge => 0.0,
        };
        let u = self.bounds.min.u_m + (column as f64 + shift) * size.du_m;
        let along_v = (row as f64 + shift) * size.dv_m;
        let v = match self.row_order {
            RowOrder::TopDown => self.bounds.max.v_m - along_v,
            RowOrder::BottomUp => self.bounds.min.v_m + along_v,
        };
        WorldPoint::new(u, v)
    }

    /// Continuous texel coordinates for a world point.
    ///
    /// Returned as reals so a caller can filter between texels. Outside the
    /// raster the values simply go negative or past the extent; clamping or
    /// wrapping is the sampler's decision, not the transform's, because
    /// "outside" means different things for a tiling noise mask and a painted
    /// region.
    pub fn world_to_texel(self, point: WorldPoint) -> (f64, f64) {
        let size = self.texel_size();
        let shift = match self.anchor {
            TexelAnchor::Centre => 0.5,
            TexelAnchor::Edge => 0.0,
        };
        let column = (point.u_m - self.bounds.min.u_m) / size.du_m - shift;
        let along_v = match self.row_order {
            RowOrder::TopDown => self.bounds.max.v_m - point.v_m,
            RowOrder::BottomUp => point.v_m - self.bounds.min.v_m,
        };
        (column, along_v / size.dv_m - shift)
    }
}

/// A named region of ground, for reporting and debugging.
#[derive(Clone, Debug, PartialEq)]
pub struct Footprint {
    pub layer: LayerKey,
    pub bounds: WorldRect,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(u: f64, v: f64) -> WorldPoint {
        WorldPoint::new(u, v)
    }

    #[test]
    fn a_rectangle_is_half_open_on_both_axes() {
        let rect = WorldRect::new(point(0.0, 0.0), point(4.0, 4.0));
        assert!(
            rect.contains(point(0.0, 0.0)),
            "the minimum corner is inside"
        );
        assert!(
            !rect.contains(point(4.0, 2.0)),
            "the maximum edge is outside"
        );
        assert!(
            !rect.contains(point(2.0, 4.0)),
            "the maximum edge is outside"
        );
        assert!(!rect.contains(point(4.0, 4.0)));
    }

    #[test]
    fn tiling_a_region_covers_every_point_exactly_once() {
        // The property half-openness exists for, asserted directly. A point
        // owned by two tiles is a quantity computed twice; a point owned by none
        // is a hole.
        let tiles: Vec<WorldRect> = (0..3)
            .flat_map(|y| {
                (0..3).map(move |x| {
                    WorldRect::from_size(
                        point(x as f64 * 2.0, y as f64 * 2.0),
                        WorldVector::new(2.0, 2.0),
                    )
                })
            })
            .collect();
        // Sample densely, including every tile boundary exactly.
        for step_u in 0..=60 {
            for step_v in 0..=60 {
                let p = point(step_u as f64 * 0.1, step_v as f64 * 0.1);
                let owners = tiles.iter().filter(|t| t.contains(p)).count();
                let inside = p.u_m < 6.0 && p.v_m < 6.0;
                assert_eq!(
                    owners,
                    usize::from(inside),
                    "{p} is owned by {owners} tiles"
                );
            }
        }
    }

    #[test]
    fn cells_do_not_widen_at_the_world_origin() {
        // The bug truncation produces, asserted so nobody reintroduces `as i64`.
        // With truncation, -0.5 and +0.5 both land in cell 0 and cell zero is
        // twice as wide as every other — a stripe through the origin in every
        // population keyed on this.
        let grid = CellGrid::new(1.0);
        assert_eq!(grid.cell_at(point(-1.5, 0.0)).x, -2);
        assert_eq!(grid.cell_at(point(-0.5, 0.0)).x, -1);
        assert_eq!(grid.cell_at(point(-0.001, 0.0)).x, -1);
        assert_eq!(grid.cell_at(point(0.0, 0.0)).x, 0);
        assert_eq!(grid.cell_at(point(0.5, 0.0)).x, 0);
        assert_eq!(grid.cell_at(point(1.0, 0.0)).x, 1);
    }

    #[test]
    fn every_cell_is_the_same_width_across_the_origin() {
        let grid = CellGrid::new(0.25);
        let widths: Vec<f64> = (-8..8)
            .map(|x| grid.cell_rect(CellCoord::new(x, 0)).width_m())
            .collect();
        for width in &widths {
            assert!((width - 0.25).abs() < 1.0e-12, "{width}");
        }
    }

    #[test]
    fn a_point_on_a_cell_boundary_belongs_to_the_cell_above_it() {
        // Deterministic boundary ownership, and it has to agree with
        // `cell_rect`'s half-openness or a bake would place a mark in one cell
        // and look for it in another.
        let grid = CellGrid::new(2.0);
        for boundary in [-4.0, -2.0, 0.0, 2.0, 4.0] {
            let cell = grid.cell_at(point(boundary, boundary));
            assert!(
                grid.cell_rect(cell).contains(point(boundary, boundary)),
                "{boundary} is not inside the cell that claims it"
            );
        }
    }

    #[test]
    fn cells_over_a_rectangle_stop_at_its_exclusive_edge() {
        // A rectangle ending exactly on a boundary must not pull in the row
        // beyond it. Getting this wrong costs a whole extra row and column of
        // candidate generation on every page, and shows up as a performance
        // mystery rather than as a wrong picture.
        let grid = CellGrid::new(1.0);
        let cells = grid.cells_over(WorldRect::new(point(0.0, 0.0), point(2.0, 2.0)));
        assert_eq!(cells.len(), 4, "{cells:?}");
        assert!(cells.contains(&CellCoord::new(0, 0)));
        assert!(cells.contains(&CellCoord::new(1, 1)));
        assert!(!cells.contains(&CellCoord::new(2, 0)));
        assert!(!cells.contains(&CellCoord::new(0, 2)));
    }

    #[test]
    fn cells_over_a_rectangle_are_row_major_and_stable() {
        let grid = CellGrid::new(1.0);
        let cells = grid.cells_over(WorldRect::new(point(-1.5, -1.5), point(0.5, 0.5)));
        assert_eq!(
            cells,
            vec![
                CellCoord::new(-2, -2),
                CellCoord::new(-1, -2),
                CellCoord::new(0, -2),
                CellCoord::new(-2, -1),
                CellCoord::new(-1, -1),
                CellCoord::new(0, -1),
                CellCoord::new(-2, 0),
                CellCoord::new(-1, 0),
                CellCoord::new(0, 0),
            ]
        );
    }

    #[test]
    fn every_cell_a_rectangle_touches_is_listed() {
        // The two directions of the same claim: nothing listed that is not
        // touched, nothing touched that is not listed.
        let grid = CellGrid::new(0.7);
        let rect = WorldRect::new(point(-2.3, -1.1), point(1.9, 3.4));
        let cells = grid.cells_over(rect);
        for cell in &cells {
            assert!(
                grid.cell_rect(*cell).intersection(rect).is_some(),
                "{cell} was listed but does not meet the rectangle"
            );
        }
        for x in -10..10 {
            for y in -10..10 {
                let cell = CellCoord::new(x, y);
                if grid.cell_rect(cell).intersection(rect).is_some() {
                    assert!(
                        cells.contains(&cell),
                        "{cell} meets the rectangle and was not listed"
                    );
                }
            }
        }
    }

    #[test]
    fn an_empty_rectangle_covers_no_cells() {
        let grid = CellGrid::new(1.0);
        assert!(
            grid.cells_over(WorldRect::new(point(1.0, 1.0), point(1.0, 5.0)))
                .is_empty()
        );
        assert!(
            grid.cells_over(WorldRect::new(point(f64::NAN, 0.0), point(1.0, 1.0)))
                .is_empty()
        );
    }

    #[test]
    fn touching_rectangles_do_not_intersect() {
        let left = WorldRect::new(point(0.0, 0.0), point(1.0, 1.0));
        let right = WorldRect::new(point(1.0, 0.0), point(2.0, 1.0));
        assert_eq!(left.intersection(right), None);
        assert_eq!(
            left.union(right),
            WorldRect::new(point(0.0, 0.0), point(2.0, 1.0))
        );
    }

    #[test]
    fn expanding_a_rectangle_adds_a_halo_on_every_side() {
        let rect = WorldRect::new(point(0.0, 0.0), point(4.0, 2.0));
        let grown = rect.expanded(0.5);
        assert_eq!(grown.min, point(-0.5, -0.5));
        assert_eq!(grown.max, point(4.5, 2.5));
        assert_eq!(grown.expanded(-0.5), rect);
    }

    #[test]
    fn a_local_origin_buys_back_the_precision_far_from_the_world_origin() {
        // The reason world positions are f64. Ten kilometres out an f32's
        // spacing is about a millimetre, so a millimetre of detail is right at
        // the edge of what it can hold — and what comes back is not the
        // millimetre but the nearest representable neighbour of it.
        let origin = point(10_000.0, 10_000.0);
        let nudged = point(10_000.001, 10_000.0);

        // Subtract first, then narrow: the f32 only ever carries the offset, so
        // it spends its precision where the detail is.
        let kept = nudged.to_local_f32(origin)[0];
        assert!(
            ((kept as f64) - 0.001).abs() < 1.0e-9,
            "a local origin lost the millimetre: {kept}"
        );

        // Narrow first, then subtract — the mistake the API exists to prevent.
        // It does not vanish; it comes back over two percent wrong, which is
        // far more dangerous than vanishing because it still looks like a
        // measurement.
        let naive = (nudged.u_m as f32) - (origin.u_m as f32);
        let error = ((naive as f64) - 0.001).abs() / 0.001;
        assert!(
            error > 0.02,
            "narrowing before subtracting was unexpectedly accurate ({naive})"
        );
    }

    #[test]
    fn centre_and_edge_anchors_differ_by_half_a_texel() {
        // The half-texel that ruins a mask.
        let bounds = WorldRect::new(point(0.0, 0.0), point(64.0, 64.0));
        let centred = RasterTransform::new(bounds, 64, 64);
        let edged = RasterTransform::new(bounds, 64, 64).with_anchor(TexelAnchor::Edge);
        assert_eq!(centred.texel_to_world(0, 0).u_m, 0.5);
        assert_eq!(edged.texel_to_world(0, 0).u_m, 0.0);
        // And the edge lattice spans the full extent with its last sample.
        assert_eq!(edged.texel_to_world(63, 0).u_m, 64.0);
        assert_eq!(centred.texel_to_world(63, 0).u_m, 63.5);
    }

    #[test]
    fn row_order_decides_which_end_of_the_world_row_zero_is() {
        let bounds = WorldRect::new(point(0.0, 0.0), point(8.0, 8.0));
        let top_down = RasterTransform::new(bounds, 8, 8);
        let bottom_up = RasterTransform::new(bounds, 8, 8).with_row_order(RowOrder::BottomUp);
        assert_eq!(top_down.texel_to_world(0, 0).v_m, 7.5);
        assert_eq!(bottom_up.texel_to_world(0, 0).v_m, 0.5);
    }

    #[test]
    fn texel_and_world_coordinates_invert_each_other() {
        let bounds = WorldRect::new(point(-32.0, -32.0), point(32.0, 32.0));
        for anchor in [TexelAnchor::Centre, TexelAnchor::Edge] {
            for order in [RowOrder::TopDown, RowOrder::BottomUp] {
                let transform = RasterTransform::new(bounds, 64, 48)
                    .with_anchor(anchor)
                    .with_row_order(order);
                for (column, row) in [(0, 0), (7, 3), (63, 47), (31, 24)] {
                    let world = transform.texel_to_world(column, row);
                    let (back_c, back_r) = transform.world_to_texel(world);
                    assert!(
                        (back_c - column as f64).abs() < 1.0e-9
                            && (back_r - row as f64).abs() < 1.0e-9,
                        "{anchor:?}/{order:?} texel ({column}, {row}) -> {world} -> \
                         ({back_c}, {back_r})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_degenerate_cell_size_does_not_panic() {
        // Reached from authored data. A document saying `0.0` is validation's
        // problem to report, not a crash's problem to announce.
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let grid = CellGrid::new(bad);
            assert!(grid.cell_m() > 0.0);
            let _ = grid.cell_at(WorldPoint::ORIGIN);
        }
    }

    #[test]
    fn a_coordinate_past_the_end_of_the_world_saturates() {
        // Rather than wrapping to a cell beside the origin, which would silently
        // reuse another region's candidate identities.
        let grid = CellGrid::new(1.0);
        assert_eq!(grid.cell_at(point(1.0e30, 0.0)).x, i64::MAX);
        assert_eq!(grid.cell_at(point(-1.0e30, 0.0)).x, i64::MIN);
    }

    #[test]
    fn vectors_normalise_or_report_that_they_cannot() {
        assert_eq!(WorldVector::new(3.0, 4.0).length(), 5.0);
        let unit = WorldVector::new(3.0, 4.0)
            .normalised()
            .expect("a direction");
        assert!((unit.length() - 1.0).abs() < 1.0e-12);
        assert_eq!(WorldVector::ZERO.normalised(), None);
        assert_eq!(WorldVector::new(f64::NAN, 0.0).normalised(), None);
    }
}

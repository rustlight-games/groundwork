//! The canonical low-fidelity matrix: a typed, multi-channel, top-down stack.
//!
//! ## Why this is not a low-resolution picture
//!
//! An RGB preview cannot tell the next stage what a dark patch *means*. Lower
//! ground, wet dirt, shadow, dense grass, a different substrate and a painted
//! colour variation all darken a texel, and geometry, ownership and lighting
//! each need a different one of those answers. So the thing that travels between
//! stages is a stack of **named planes with declared units**, and the cheap RGB
//! picture is a derivative of it rather than the other way round.
//!
//! That is the whole reason this module exists. Everything below follows from
//! it.
//!
//! ## Four groups, and they compose differently
//!
//! The groups are kept apart because their arithmetic is genuinely different,
//! and a stack that mixed them would need every consumer to remember which rule
//! applied to which plane:
//!
//! - **Structural** — elevation and microrelief. Metres, added.
//! - **Substrate** — what the continuous ground is made of. Mutually exclusive,
//!   normalised to one, because a square metre of ground is entirely something.
//! - **Cover** — what lies *over* the substrate. Snow, litter, water. Depth and
//!   coverage, **not** normalised against the substrate: snow over grass leaves
//!   the grass semantically present underneath, and a third normalised weight
//!   would erase it. See [`CoverPlane`].
//! - **Modifier** — authored control fields. Density, moisture, compaction,
//!   dryness. Independent, each with its own composition rule from the document.
//!
//! A population is none of these. Populations are countable things and live in
//! the scene, not in the matrix; a plane says *how much* grows, never *which
//! blades*.
//!
//! ## Edge-anchored, and snapped to a global lattice
//!
//! Two properties, and both are seam insurance:
//!
//! 1. The lattice samples its own **corners**. A grid over `columns × rows`
//!    cells holds `(columns + 1) × (rows + 1)` samples, so the last column of
//!    one grid *is* the first column of its neighbour rather than sitting half a
//!    cell away from it.
//! 2. The origin is **snapped down to a multiple of the spacing**, not set to
//!    the caller's rectangle. This is the one that is easy to miss. Edge
//!    anchoring alone only makes two grids agree if they happen to share a
//!    lattice — and two requests with different bounds do not, so their samples
//!    interleave and the interpolated surface between them is nobody's. Snapping
//!    means every grid at a given spacing is a window onto *one* world lattice,
//!    and two regions compiled in different processes agree exactly wherever
//!    they overlap.
//!
//! [`FieldGridSpec::covering`] is the only constructor that a compiler should
//! use, precisely so that property cannot be opted out of by accident.
//!
//! ## Every plane declares how to read it
//!
//! A [`FieldDescriptor`] carries the unit, the legal range, the filter and the
//! border rule. That is not bookkeeping: **a categorical plane must not be
//! bilinearly interpolated**, because the average of material 2 and material 4
//! is not material 3, and a direction plane must be renormalised after averaging
//! or it shortens wherever two directions disagree. Carrying the rule with the
//! data is what stops each consumer inventing its own answer — the failure mode
//! the whole stack exists to prevent, since a population placing by one slope
//! and a renderer shading by a different one is invisible until it is a seam.

// Clippy reads `!(x > 0.0)` as a negated comparison and suggests `x <= 0.0`.
// They are not the same predicate. `!(x > 0.0)` is **true** for NaN and
// `x <= 0.0` is false, and every one of these is a guard whose whole job is to
// catch a NaN before it poisons an accumulator. The awkward spelling is the
// point.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::digest::{Digest, Digestible, Fingerprint};
use terrain_core::ids::{MaterialIndex, ModifierIndex};

use crate::ground::{GroundMaterialChannel, GroundModifierChannel, GroundSurface};

/// The version this stack's construction stamps on itself.
///
/// Its own domain, separate from the generator and the document. A change to
/// how a plane is derived must move field-stack cache keys without pretending
/// the meadow moved, and a change to the meadow must not invalidate a cached
/// slope that is still correct.
pub const FIELD_STACK_VERSION: u32 = 1;

/// What a plane's numbers mean.
///
/// Checked rather than decorative: a validator that knows a plane is in metres
/// can say a snow depth of minus four is wrong, and one that knows a plane is
/// categorical can refuse to interpolate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldUnit {
    /// A bare multiplier or score.
    Unitless,
    /// A proportion, `0..1`.
    Fraction,
    Metres,
    Radians,
    /// A gradient: change per metre travelled.
    PerMetre,
    /// An area, in square metres. What flow accumulation gathers.
    SquareMetres,
    /// An identity. Never interpolated.
    Categorical,
}

impl FieldUnit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unitless => "unitless",
            Self::Fraction => "fraction",
            Self::Metres => "metres",
            Self::Radians => "radians",
            Self::PerMetre => "per_metre",
            Self::SquareMetres => "square_metres",
            Self::Categorical => "categorical",
        }
    }
}

/// How a plane is read between its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FieldFilter {
    /// Weighted by the four surrounding samples. What a continuous field wants.
    #[default]
    Bilinear,
    /// The closest sample, unmodified. What a categorical field *requires* —
    /// the average of two identities is not a third identity.
    Nearest,
}

/// What a plane reads as outside its own grid.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum FieldBorder {
    /// The nearest edge sample continues outward.
    ///
    /// The default because it is the only choice that does not invent a step at
    /// the boundary — and a step at the boundary of the *generated* region lands
    /// inside the halo, where every neighbourhood term reads it.
    #[default]
    Clamp,
    /// A stated value.
    Value(f32),
}

/// Everything a consumer needs in order to read a plane correctly.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDescriptor {
    /// The name the plane is addressed by in debug plates, AOVs and the CLI.
    pub key: String,
    pub unit: FieldUnit,
    /// The legal range, when the channel declares one.
    pub range: Option<(f32, f32)>,
    pub filter: FieldFilter,
    pub border: FieldBorder,
    /// Digest quantisation, in steps per unit.
    ///
    /// Per plane rather than global, because the noise floor is not: a height in
    /// metres and a fraction in `0..1` need different step sizes for "a change
    /// somebody made" to be distinguishable from "the last bit of six
    /// transcendental functions".
    pub digest_steps_per_unit: f64,
}

impl FieldDescriptor {
    /// A continuous scalar plane with sensible defaults.
    pub fn scalar(key: impl Into<String>, unit: FieldUnit) -> Self {
        Self {
            key: key.into(),
            unit,
            range: None,
            filter: FieldFilter::Bilinear,
            border: FieldBorder::Clamp,
            digest_steps_per_unit: 10_000.0,
        }
    }

    /// A plane whose values are identities.
    pub fn categorical(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            unit: FieldUnit::Categorical,
            range: None,
            filter: FieldFilter::Nearest,
            border: FieldBorder::Clamp,
            digest_steps_per_unit: 1.0,
        }
    }

    pub fn with_range(mut self, low: f32, high: f32) -> Self {
        self.range = Some((low, high));
        self
    }

    pub fn with_border(mut self, border: FieldBorder) -> Self {
        self.border = border;
        self
    }

    pub fn with_steps(mut self, steps_per_unit: f64) -> Self {
        self.digest_steps_per_unit = steps_per_unit;
        self
    }

    /// Whether this descriptor's filter is legal for its unit.
    ///
    /// Two combinations are always a bug, and both are silent:
    ///
    /// - a **categorical** plane read bilinearly, because the average of
    ///   material 2 and material 4 is not material 3;
    /// - a **wrapped angle** read bilinearly, because `+179` and `-179` degrees
    ///   both point nearly west and average to zero, which points east. An
    ///   aspect field interpolated across its own branch cut sends water up the
    ///   hill for one texel in every wrap.
    ///
    /// Reported rather than silently corrected, because the fix depends on
    /// which of the two the author meant — a direction that needs interpolating
    /// belongs in a [`VectorPlane`], where there is no branch cut to cross.
    pub fn filter_is_legal(&self) -> bool {
        !matches!(
            (self.unit, self.filter),
            (FieldUnit::Categorical, FieldFilter::Bilinear)
                | (FieldUnit::Radians, FieldFilter::Bilinear)
        )
    }
}

impl Digestible for FieldDescriptor {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(&self.key).str(self.unit.name());
        match self.range {
            Some((low, high)) => {
                digest.tag(1).f32(low).f32(high);
            }
            None => {
                digest.tag(0);
            }
        }
        digest.u32(self.filter as u32);
        match self.border {
            FieldBorder::Clamp => {
                digest.tag(0);
            }
            FieldBorder::Value(value) => {
                digest.tag(1).f32(value);
            }
        }
        digest.f64(self.digest_steps_per_unit);
    }
}

/// The lattice a stack is sampled on.
///
/// Edge-anchored and snapped to a global lattice — see the module note for why
/// both are load-bearing.
///
/// ## Why the origin is an integer
///
/// Storing the origin as a world point is the obvious design and it loses the
/// property the whole grid exists for. Two windows that both cover a point
/// reach it by different arithmetic — `-3.0 + 53 × 0.05` in one and
/// `-0.4 + 1 × 0.05` in the other — and those differ in the last bit. A noise
/// source evaluated at two coordinates that differ by an ulp returns values
/// that differ by an ulp, so the two windows disagree everywhere they overlap
/// by an amount too small to see and too large to be equal.
///
/// Addressing the lattice by **integer index** removes the arithmetic
/// difference rather than tolerating it: a sample's world position is
/// `index × spacing` on both sides, so two windows agree bit for bit. An
/// approximate seam guarantee is not one — the whole point is that a caller can
/// compare for equality and be right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldGridSpec {
    /// The global lattice index of sample `(0, 0)`.
    pub origin_index: [i64; 2],
    /// Metres between lattice points.
    pub spacing_m: f64,
    /// Cells across. There are `columns + 1` samples.
    pub columns: u32,
    /// Cells down. There are `rows + 1` samples.
    pub rows: u32,
}

/// The smallest spacing a grid may be built at, in metres.
///
/// A guard rather than a judgement. Half a millimetre over a nine-tile layout is
/// already more samples than any machine will hold, and the value exists so a
/// bad divisor produces a diagnosable grid instead of an allocation the size of
/// the address space.
pub const MIN_SPACING_M: f64 = 0.0005;

/// The largest lattice index magnitude a grid may address.
///
/// Two thirds of the way to the point where consecutive `i64` values stop being
/// distinguishable as `f64` (2^53). Beyond it, samples 0 and 1 would round to
/// the same world position and the grid would silently stop being a lattice, so
/// the limit is enforced rather than documented.
pub const LATTICE_INDEX_LIMIT: f64 = 1.0e15;

/// The most samples one plane may hold.
///
/// Sixteen million, which is a 4096-square grid — far beyond the resolutions in
/// the spec's tiers and comfortably below the point where a stack of twenty
/// planes stops fitting in memory.
pub const MAX_SAMPLES_PER_PLANE: usize = 16_777_216;

impl FieldGridSpec {
    /// The grid covering `bounds` at `spacing_m`, snapped to the world lattice.
    ///
    /// The origin is the largest multiple of the spacing at or below the
    /// rectangle's minimum corner, and the grid is extended to cover the maximum
    /// corner. So the grid is always at least as large as asked for, never
    /// smaller, and two calls with different rectangles produce grids whose
    /// samples coincide exactly wherever they overlap.
    ///
    /// That last property is what makes a nine-tile plate and a single-tile
    /// re-render of its middle agree, and it is why this — rather than a
    /// struct literal — is what a compiler calls.
    pub fn covering(bounds: WorldRect, spacing_m: f64) -> Self {
        let spacing_m = if spacing_m.is_finite() && spacing_m >= MIN_SPACING_M {
            spacing_m
        } else {
            MIN_SPACING_M
        };
        let origin_index = [
            lattice_index_below(bounds.min.u_m, spacing_m),
            lattice_index_below(bounds.min.v_m, spacing_m),
        ];
        // Spanned in *index* space rather than by dividing a float extent.
        // `bounds.max - origin` is not exact — for a rectangle from -0.2 to
        // -3 x 0.05 it comes out a hair under one cell, so a float span drops
        // the cell that should contain the far edge. Two floored indices and a
        // subtraction cannot.
        let far = [
            lattice_index_below(bounds.max.u_m, spacing_m),
            lattice_index_below(bounds.max.v_m, spacing_m),
        ];
        // One past the cell the far edge falls in, so `bounds.max` is strictly
        // inside the grid rather than on its open boundary.
        let columns = cells_between(origin_index[0], far[0]);
        let rows = cells_between(origin_index[1], far[1]);
        Self {
            origin_index,
            spacing_m,
            columns,
            rows,
        }
    }

    /// The world point sample `(0, 0)` sits at.
    pub fn origin(&self) -> WorldPoint {
        self.sample_position(0, 0)
    }

    pub fn samples_across(&self) -> usize {
        self.columns as usize + 1
    }

    pub fn samples_down(&self) -> usize {
        self.rows as usize + 1
    }

    pub fn sample_count(&self) -> usize {
        self.samples_across() * self.samples_down()
    }

    /// The ground this grid covers.
    pub fn bounds(&self) -> WorldRect {
        WorldRect::new(self.origin(), self.sample_position(self.columns, self.rows))
    }

    /// The world point one lattice sample sits at.
    ///
    /// Computed from the *global* index rather than by walking from a stored
    /// origin, so that two windows covering the same point produce the identical
    /// coordinate rather than one an ulp apart. See the type's note.
    pub fn sample_position(&self, column: u32, row: u32) -> WorldPoint {
        WorldPoint::new(
            (self.origin_index[0] + column as i64) as f64 * self.spacing_m,
            (self.origin_index[1] + row as i64) as f64 * self.spacing_m,
        )
    }

    /// The index of a sample in a row-major plane.
    pub fn index(&self, column: u32, row: u32) -> usize {
        row as usize * self.samples_across() + column as usize
    }

    /// Whether the grid is usable: positive spacing, and not absurdly large.
    pub fn is_well_formed(&self) -> bool {
        self.spacing_m.is_finite()
            && self.spacing_m >= MIN_SPACING_M
            && self.origin().is_finite()
            && self.sample_count() <= MAX_SAMPLES_PER_PLANE
            && self.origin_index.iter().all(|index| {
                (*index as f64).abs() <= LATTICE_INDEX_LIMIT
            })
            // Far from the origin, consecutive indices round to the same `f64`
            // and the grid quietly stops being a lattice: two samples at one
            // world position, interpolating over a zero-width cell. Checked
            // rather than assumed, because nothing else downstream would notice.
            && self.sample_position(0, 0) != self.sample_position(1, 0)
            && self.sample_position(0, 0) != self.sample_position(0, 1)
    }

    /// Where a world point falls in sample coordinates, clamped to the grid.
    ///
    /// Returns the lower sample index on each axis and the fraction toward the
    /// next one. Clamping here rather than in each reader is what implements
    /// [`FieldBorder::Clamp`] for the position; a border *value* is applied by
    /// the reader, which knows whether the point was outside.
    /// Where a world point falls, in cell index and fraction.
    ///
    /// The fraction is computed from the **global** lattice — `position /
    /// spacing`, floored — rather than from a stored origin. Subtracting a
    /// window's own origin first would make the fraction depend on which window
    /// asked, so two overlapping stacks holding identical lattice values would
    /// still interpolate to answers an ulp apart. Deriving it globally and then
    /// converting to a local index by *integer* subtraction keeps a read
    /// window-independent all the way through the filter, which is what lets a
    /// caller compare two regions for equality rather than for closeness.
    fn locate(&self, at: WorldPoint) -> ([u32; 2], [f32; 2]) {
        let axis = |value: f64, origin_index: i64, cells: u32| -> (u32, f32) {
            let cells = cells.max(1);
            let Some((index, fraction)) = lattice_split(value, self.spacing_m) else {
                return (0, 0.0);
            };
            let local = index.saturating_sub(origin_index);
            if local < 0 {
                return (0, 0.0);
            }
            if local >= cells as i64 {
                // The last *cell*, fully toward its far sample, so a point on
                // the far edge reads the edge sample rather than falling off
                // the end.
                return (cells - 1, 1.0);
            }
            (local as u32, fraction as f32)
        };
        let (cu, tu) = axis(at.u_m, self.origin_index[0], self.columns);
        let (cv, tv) = axis(at.v_m, self.origin_index[1], self.rows);
        ([cu, cv], [tu, tv])
    }

    /// Whether a point lies inside the grid's closed sample domain.
    ///
    /// **Closed on both ends, deliberately unlike [`WorldRect::contains`]**,
    /// which is half-open. The two answer different questions: a rectangle is
    /// half-open so that tiling a region produces no overlaps and no gaps, but a
    /// lattice genuinely *has* a sample on its far edge, and a read there should
    /// return that sample rather than a border value. Calling the difference out
    /// here because two meanings of "inside the grid" is otherwise the kind of
    /// thing that is discovered from a picture.
    #[allow(rustdoc::broken_intra_doc_links)]
    fn contains(&self, at: WorldPoint) -> bool {
        let bounds = self.bounds();
        at.u_m >= bounds.min.u_m
            && at.u_m <= bounds.max.u_m
            && at.v_m >= bounds.min.v_m
            && at.v_m <= bounds.max.v_m
    }
}

/// Split a coordinate into its lattice cell and the fraction across it.
///
/// The subtlety this exists for: **division does not invert multiplication.**
/// The canonical position of lattice index 3 at a spacing of 0.05 is
/// `0.15000000000000002`, and dividing that back by 0.05 gives
/// `3.0000000000000004` rather than 3. Flooring the quotient is therefore
/// correct almost everywhere and wrong exactly on the lattice lines — where it
/// matters most, because that is where two windows meet.
///
/// The consequence was not theoretical. One window ending at index 3 clamped
/// the point to its last cell and returned sample 3 exactly; a longer window
/// read it as cell 3 at a fraction of `4.4e-16` and returned a blend of samples
/// 3 and 4. Two overlapping stacks disagreed at their shared edge — the one
/// place the whole design exists to make agree.
///
/// So a point is tested against the *canonical* position of its nearest index
/// before the quotient is trusted. Returns `None` for a coordinate that has no
/// meaningful lattice position at all.
fn lattice_split(value: f64, step: f64) -> Option<(i64, f64)> {
    if !value.is_finite() || !(step > 0.0) {
        return None;
    }
    let global = value / step;
    if !global.is_finite() {
        return None;
    }
    let nearest = global.round();
    if nearest.abs() <= LATTICE_INDEX_LIMIT && nearest * step == value {
        // Exactly on a lattice line, whatever the quotient says.
        return Some((nearest as i64, 0.0));
    }
    let floor = global.floor();
    if floor.abs() > LATTICE_INDEX_LIMIT {
        return None;
    }
    Some((floor as i64, global - floor))
}

/// The index of the largest lattice point at or below `value`.
fn lattice_index_below(value: f64, step: f64) -> i64 {
    lattice_split(value, step)
        .map(|(index, _)| index)
        .unwrap_or(0)
}

/// Cells needed to reach one past the cell `far` falls in.
///
/// Integer throughout, so a rectangle whose far edge lands exactly on a lattice
/// point still gets the cell beyond it and `bounds.max` is strictly inside the
/// grid rather than on its open boundary.
fn cells_between(origin: i64, far: i64) -> u32 {
    far.saturating_sub(origin)
        .saturating_add(1)
        .clamp(1, u32::MAX as i64) as u32
}

impl Digestible for FieldGridSpec {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .i64(self.origin_index[0])
            .i64(self.origin_index[1])
            .f64(self.spacing_m)
            .u32(self.columns)
            .u32(self.rows);
    }
}

/// One scalar field across the grid, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarPlane {
    pub values: Vec<f32>,
    pub descriptor: FieldDescriptor,
}

impl ScalarPlane {
    /// A plane filled with one value.
    pub fn filled(grid: &FieldGridSpec, value: f32, descriptor: FieldDescriptor) -> Self {
        Self {
            values: vec![value; grid.sample_count()],
            descriptor,
        }
    }

    /// Read the plane at a world point, honouring its declared filter and
    /// border.
    pub fn sample(&self, grid: &FieldGridSpec, at: WorldPoint) -> f32 {
        if let FieldBorder::Value(outside) = self.descriptor.border
            && !grid.contains(at)
        {
            return outside;
        }
        let (cell, t) = grid.locate(at);
        match self.descriptor.filter {
            FieldFilter::Nearest => {
                let column = cell[0] + u32::from(t[0] >= 0.5);
                let row = cell[1] + u32::from(t[1] >= 0.5);
                self.at(grid, column, row)
            }
            FieldFilter::Bilinear => {
                let (c, r) = (cell[0], cell[1]);
                let v00 = self.at(grid, c, r);
                let v10 = self.at(grid, c + 1, r);
                let v01 = self.at(grid, c, r + 1);
                let v11 = self.at(grid, c + 1, r + 1);
                let low = v00 + (v10 - v00) * t[0];
                let high = v01 + (v11 - v01) * t[0];
                low + (high - low) * t[1]
            }
        }
    }

    /// One sample, with indices clamped into the grid.
    pub fn at(&self, grid: &FieldGridSpec, column: u32, row: u32) -> f32 {
        let column = column.min(grid.columns);
        let row = row.min(grid.rows);
        self.values
            .get(grid.index(column, row))
            .copied()
            .unwrap_or(0.0)
    }

    pub fn is_well_formed(&self, grid: &FieldGridSpec) -> bool {
        self.values.len() == grid.sample_count()
            && self.descriptor.filter_is_legal()
            && self.values.iter().all(|v| v.is_finite())
    }

    /// The smallest and largest value, for a debug plate's own scaling.
    pub fn extent(&self) -> (f32, f32) {
        self.values
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(*v), hi.max(*v))
            })
    }
}

impl Digestible for ScalarPlane {
    fn absorb(&self, digest: &mut Digest) {
        self.descriptor.absorb(digest);
        let steps = self.descriptor.digest_steps_per_unit;
        digest.slice(&self.values, |d, value| {
            d.quantised(*value as f64, steps);
        });
    }
}

/// A two-component field: a direction, a gradient, a boundary frame axis.
///
/// Its own type rather than two scalar planes because the two components are not
/// independent. Averaging them separately is what shortens a direction wherever
/// its neighbours disagree, and a consumer holding two unrelated planes has
/// nothing telling it to renormalise. [`VectorPlane::sample_unit`] is the read
/// that cannot get it wrong.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorPlane {
    pub u: Vec<f32>,
    pub v: Vec<f32>,
    pub descriptor: FieldDescriptor,
    /// Whether a sample should be renormalised to unit length after filtering.
    pub unit_length: bool,
}

impl VectorPlane {
    pub fn filled(grid: &FieldGridSpec, descriptor: FieldDescriptor, unit_length: bool) -> Self {
        Self {
            u: vec![0.0; grid.sample_count()],
            v: vec![0.0; grid.sample_count()],
            descriptor,
            unit_length,
        }
    }

    /// Read both components, renormalised if the plane says so.
    pub fn sample_unit(&self, grid: &FieldGridSpec, at: WorldPoint) -> [f32; 2] {
        let (cell, t) = grid.locate(at);
        let (c, r) = (cell[0], cell[1]);
        let read = |values: &[f32]| -> f32 {
            let get = |column: u32, row: u32| -> f32 {
                let column = column.min(grid.columns);
                let row = row.min(grid.rows);
                values.get(grid.index(column, row)).copied().unwrap_or(0.0)
            };
            match self.descriptor.filter {
                // Honoured rather than assumed. A vector plane may carry
                // something that must not be averaged — a per-sample choice of
                // frame, say — and silently blending it would produce a
                // direction that is nobody's.
                FieldFilter::Nearest => get(c + u32::from(t[0] >= 0.5), r + u32::from(t[1] >= 0.5)),
                FieldFilter::Bilinear => {
                    let low = get(c, r) + (get(c + 1, r) - get(c, r)) * t[0];
                    let high = get(c, r + 1) + (get(c + 1, r + 1) - get(c, r + 1)) * t[0];
                    low + (high - low) * t[1]
                }
            }
        };
        let mut out = [read(&self.u), read(&self.v)];
        if self.unit_length {
            let length = (out[0] * out[0] + out[1] * out[1]).sqrt();
            if length > 1.0e-6 {
                out[0] /= length;
                out[1] /= length;
            }
        }
        out
    }

    pub fn is_well_formed(&self, grid: &FieldGridSpec) -> bool {
        // A scalar border value cannot say what a two-component field reads as
        // outside itself, so a vector plane may not declare one. Clamping is the
        // only border it supports, and saying so is better than quietly
        // ignoring what an author wrote.
        matches!(self.descriptor.border, FieldBorder::Clamp)
            && self.descriptor.filter_is_legal()
            && self.u.len() == grid.sample_count()
            && self.v.len() == grid.sample_count()
            && self
                .u
                .iter()
                .chain(self.v.iter())
                .all(|value| value.is_finite())
    }
}

impl Digestible for VectorPlane {
    fn absorb(&self, digest: &mut Digest) {
        self.descriptor.absorb(digest);
        let steps = self.descriptor.digest_steps_per_unit;
        digest.bool(self.unit_length);
        digest.slice(&self.u, |d, value| {
            d.quantised(*value as f64, steps);
        });
        digest.slice(&self.v, |d, value| {
            d.quantised(*value as f64, steps);
        });
    }
}

/// One substrate's coverage across the grid.
///
/// Dense per material even though a point sample is a pruned list, for the
/// reason in [`crate::ground`]: a renderer interpolating channel `k` across four
/// neighbours cannot do it against four differently shaped lists.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialPlane {
    pub material: MaterialIndex,
    pub weights: ScalarPlane,
}

/// One declared modifier channel across the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct ModifierPlane {
    pub channel: ModifierIndex,
    pub values: ScalarPlane,
}

/// A continuous cover lying over the substrate.
///
/// Depth **and** coverage, and the pair is the point. Depth alone cannot say
/// whether two centimetres of snow is a continuous sheet or a scattering of
/// patches between grass blades, and coverage alone cannot say how far the
/// surface stands proud of the ground. Both are needed to render dusting, and
/// dusting is the interesting half of the range.
///
/// Not normalised against the substrate weights. A cover sits *over* ground that
/// remains semantically what it was, which is what lets shallow snow reveal the
/// dirt it lies on and what a third normalised weight would destroy.
#[derive(Clone, Debug, PartialEq)]
pub struct CoverPlane {
    /// The cover's index in the scene's cover binding table.
    pub cover: u16,
    /// How deep the cover stands above the ground surface, metres.
    pub depth_m: ScalarPlane,
    /// How much of the ground it hides, `0..1`.
    pub coverage: ScalarPlane,
    /// How packed it is, `0..1`. Drives roughness and load-bearing response.
    pub compaction: ScalarPlane,
    /// How wet it is, `0..1`. Drives darkening and melt.
    pub wetness: ScalarPlane,
}

/// Fields the compiler computes once from the sampled ones.
///
/// Derived here rather than in each consumer, because the characteristic failure
/// is two consumers deriving the same quantity slightly differently: a
/// population placing by one slope and a renderer shading by another produces a
/// disagreement that is invisible in every unit test and obvious in the picture.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DerivedFieldSet {
    /// Ground normal, as its two horizontal components. The vertical one is
    /// implied, since a height field's normal always points up.
    pub normal: Option<VectorPlane>,
    /// Slope magnitude, as a tangent — rise over run, not an angle.
    ///
    /// A tangent rather than radians because every consumer compares it against
    /// another tangent (an angle of repose, a growth limit) and an `atan` per
    /// sample buys nothing.
    pub slope: Option<ScalarPlane>,
    /// Which way the slope faces, radians from `+u` toward `+v`.
    pub aspect: Option<ScalarPlane>,
    /// Mean curvature. Negative in hollows, positive on crests.
    pub curvature: Option<ScalarPlane>,
    /// How sheltered a point is from the open sky, `0..1`.
    pub exposure: Option<ScalarPlane>,
    /// How much upslope ground drains through here, in square metres.
    pub flow_accumulation: Option<ScalarPlane>,
    /// Which way water leaves, unit length.
    pub flow_direction: Option<VectorPlane>,
    /// The dominant substrate's index, as a number. Categorical.
    pub dominant_material: Option<ScalarPlane>,
    /// The runner-up substrate's index. Categorical.
    pub secondary_material: Option<ScalarPlane>,
    /// How mixed the ground is here, `0..1`. Zero is pure, one is an even split.
    pub blend: Option<ScalarPlane>,
    /// Which way the substrate boundary runs, unit length.
    ///
    /// What lets a stone settle along an edge or a rut align to a track without
    /// each recipe recovering the boundary geometry for itself.
    pub boundary_tangent: Option<VectorPlane>,
}

impl DerivedFieldSet {
    /// Every scalar plane present, with its key. For debug plates and AOVs.
    pub fn scalar_planes(&self) -> Vec<(&str, &ScalarPlane)> {
        let mut out = Vec::new();
        for plane in [
            &self.slope,
            &self.aspect,
            &self.curvature,
            &self.exposure,
            &self.flow_accumulation,
            &self.dominant_material,
            &self.secondary_material,
            &self.blend,
        ]
        .into_iter()
        .flatten()
        {
            out.push((plane.descriptor.key.as_str(), plane));
        }
        out
    }

    /// Every vector plane present, with its key.
    pub fn vector_planes(&self) -> Vec<(&str, &VectorPlane)> {
        let mut out = Vec::new();
        for plane in [&self.normal, &self.flow_direction, &self.boundary_tangent]
            .into_iter()
            .flatten()
        {
            out.push((plane.descriptor.key.as_str(), plane));
        }
        out
    }
}

impl Digestible for DerivedFieldSet {
    fn absorb(&self, digest: &mut Digest) {
        let scalars = self.scalar_planes();
        digest.slice(&scalars, |d, (key, plane)| {
            d.str(key);
            plane.absorb(d);
        });
        let vectors = self.vector_planes();
        digest.slice(&vectors, |d, (key, plane)| {
            d.str(key);
            plane.absorb(d);
        });
    }
}

/// The canonical low-fidelity representation of a piece of ground.
///
/// One of these is built per render, over the *generated* bounds rather than the
/// visible ones, and every later stage reads from it: the cover solvers, the
/// candidate samplers, both renderers, and the neural corpus. Nothing downstream
/// calls [`terrain_core::PreparedTerrain`] again at its own rate, which is what
/// keeps the two halves of a training pair from disagreeing about where a path
/// edge is.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainFieldStack {
    pub grid: FieldGridSpec,
    pub elevation_m: ScalarPlane,
    pub microrelief_m: ScalarPlane,
    /// Substrates present anywhere in this region. Normalised across the set.
    pub substrates: Vec<MaterialPlane>,
    /// Declared modifier channels, in document order.
    pub modifiers: Vec<ModifierPlane>,
    /// Continuous covers over the substrate.
    pub covers: Vec<CoverPlane>,
    pub derived: DerivedFieldSet,
}

/// The domain a field stack's fingerprint is taken in.
pub const FIELD_DIGEST_DOMAIN: &str = "terrain-fields";

impl TerrainFieldStack {
    /// An empty stack over a grid: flat ground, no substrates, no covers.
    pub fn flat(grid: FieldGridSpec) -> Self {
        Self {
            elevation_m: ScalarPlane::filled(
                &grid,
                0.0,
                FieldDescriptor::scalar("elevation_m", FieldUnit::Metres),
            ),
            microrelief_m: ScalarPlane::filled(
                &grid,
                0.0,
                FieldDescriptor::scalar("microrelief_m", FieldUnit::Metres),
            ),
            grid,
            substrates: Vec::new(),
            modifiers: Vec::new(),
            covers: Vec::new(),
            derived: DerivedFieldSet::default(),
        }
    }

    pub fn sample_count(&self) -> usize {
        self.grid.sample_count()
    }

    pub fn bounds(&self) -> WorldRect {
        self.grid.bounds()
    }

    /// The structural surface height at a sample: elevation plus microrelief.
    pub fn surface_at(&self, column: u32, row: u32) -> f32 {
        self.elevation_m.at(&self.grid, column, row)
            + self.microrelief_m.at(&self.grid, column, row)
    }

    /// The structural surface height at a world point.
    pub fn surface_height(&self, at: WorldPoint) -> f32 {
        self.elevation_m.sample(&self.grid, at) + self.microrelief_m.sample(&self.grid, at)
    }

    /// One substrate's weight at a world point.
    pub fn substrate_weight(&self, material: MaterialIndex, at: WorldPoint) -> f32 {
        self.substrates
            .iter()
            .find(|plane| plane.material == material)
            .map(|plane| plane.weights.sample(&self.grid, at))
            .unwrap_or(0.0)
    }

    /// One modifier channel's value at a world point, or a fallback if the
    /// document does not declare it.
    pub fn modifier(&self, channel: ModifierIndex, at: WorldPoint, fallback: f32) -> f32 {
        self.modifiers
            .iter()
            .find(|plane| plane.channel == channel)
            .map(|plane| plane.values.sample(&self.grid, at))
            .unwrap_or(fallback)
    }

    /// Every substrate weight at a world point, in plane order.
    ///
    /// Allocation-free into a caller's buffer, because this is called once per
    /// candidate and there can be a hundred thousand of them.
    pub fn substrate_weights_into(&self, at: WorldPoint, into: &mut Vec<(MaterialIndex, f32)>) {
        into.clear();
        for plane in &self.substrates {
            let weight = plane.weights.sample(&self.grid, at);
            if weight > 0.0 {
                into.push((plane.material, weight));
            }
        }
    }

    /// Total cover depth at a world point, over every cover.
    pub fn cover_depth(&self, at: WorldPoint) -> f32 {
        self.covers
            .iter()
            .map(|cover| cover.depth_m.sample(&self.grid, at))
            .sum()
    }

    /// The greatest coverage any single cover has here, `0..1`.
    ///
    /// A maximum rather than a sum: two covers each hiding half the ground do
    /// not between them hide all of it, and a sum would say they did.
    pub fn cover_coverage(&self, at: WorldPoint) -> f32 {
        self.covers
            .iter()
            .map(|cover| cover.coverage.sample(&self.grid, at))
            .fold(0.0, f32::max)
    }

    /// The visible surface: ground plus whatever lies over it.
    pub fn visible_height(&self, at: WorldPoint) -> f32 {
        self.surface_height(at) + self.cover_depth(at)
    }

    /// Slope at a world point, as a tangent, or zero if it was not derived.
    pub fn slope(&self, at: WorldPoint) -> f32 {
        self.derived
            .slope
            .as_ref()
            .map(|plane| plane.sample(&self.grid, at))
            .unwrap_or(0.0)
    }

    /// Curvature at a world point, or zero if it was not derived.
    pub fn curvature(&self, at: WorldPoint) -> f32 {
        self.derived
            .curvature
            .as_ref()
            .map(|plane| plane.sample(&self.grid, at))
            .unwrap_or(0.0)
    }

    /// Sky exposure at a world point, or fully exposed if it was not derived.
    pub fn exposure(&self, at: WorldPoint) -> f32 {
        self.derived
            .exposure
            .as_ref()
            .map(|plane| plane.sample(&self.grid, at))
            .unwrap_or(1.0)
    }

    /// How mixed the substrate is at a world point, `0..1`.
    pub fn blend(&self, at: WorldPoint) -> f32 {
        self.derived
            .blend
            .as_ref()
            .map(|plane| plane.sample(&self.grid, at))
            .unwrap_or(0.0)
    }

    /// Every plane length matches the grid, and no plane holds a bad number.
    ///
    /// Checked rather than assumed: a plane one row short is a reader taking the
    /// next plane's first row as this one's last, which produces a picture that
    /// is wrong in a way no bounds check reports.
    pub fn is_well_formed(&self) -> bool {
        self.grid.is_well_formed()
            && self.elevation_m.is_well_formed(&self.grid)
            && self.microrelief_m.is_well_formed(&self.grid)
            && self
                .substrates
                .iter()
                .all(|plane| plane.weights.is_well_formed(&self.grid))
            && self
                .modifiers
                .iter()
                .all(|plane| plane.values.is_well_formed(&self.grid))
            && self.covers.iter().all(|cover| {
                cover.depth_m.is_well_formed(&self.grid)
                    && cover.coverage.is_well_formed(&self.grid)
                    && cover.compaction.is_well_formed(&self.grid)
                    && cover.wetness.is_well_formed(&self.grid)
            })
            // Derived planes are checked too. They are the ones a consumer is
            // most likely to read without thinking, and a truncated flow
            // direction would otherwise pass validation and be sampled as zeros.
            && self
                .derived
                .scalar_planes()
                .iter()
                .all(|(_, plane)| plane.is_well_formed(&self.grid))
            && self
                .derived
                .vector_planes()
                .iter()
                .all(|(_, plane)| plane.is_well_formed(&self.grid))
    }

    /// The ground normal at a world point, as a three-component unit vector.
    ///
    /// The stored plane holds only the two horizontal components; the vertical
    /// one is reconstructed here because a height field's normal always points
    /// up, so its sign is never in question and storing it would be a third of
    /// the memory for no information.
    ///
    /// Reconstructed rather than renormalised in two dimensions, which is the
    /// mistake this replaced: scaling the horizontal pair to unit length asserts
    /// the ground is vertical, so every gentle slope read as a cliff.
    pub fn ground_normal(&self, at: WorldPoint) -> [f32; 3] {
        let Some(plane) = self.derived.normal.as_ref() else {
            return [0.0, 0.0, 1.0];
        };
        let horizontal = plane.sample_unit(&self.grid, at);
        let flat = horizontal[0] * horizontal[0] + horizontal[1] * horizontal[1];
        let up = (1.0 - flat).max(0.0).sqrt();
        [horizontal[0], horizontal[1], up]
    }

    /// The largest deviation from one across all substrate sums.
    ///
    /// Zero substrates is reported as zero rather than one: a region with no
    /// materials is empty ground, not ground that fails normalisation.
    pub fn worst_substrate_sum_error(&self) -> f32 {
        if self.substrates.is_empty() {
            return 0.0;
        }
        let mut worst = 0.0f32;
        for index in 0..self.sample_count() {
            let sum: f32 = self
                .substrates
                .iter()
                .map(|plane| plane.weights.values.get(index).copied().unwrap_or(0.0))
                .sum();
            worst = worst.max((sum - 1.0).abs());
        }
        worst
    }

    /// The structural subset, in the shape the Cycles exporter takes.
    ///
    /// A conversion rather than a second stored copy. The exporter wants a
    /// [`GroundSurface`] and building one costs a clone of the structural planes
    /// once per render, which is nothing beside a path trace — and storing both
    /// would be two things that can disagree.
    pub fn to_ground_surface(&self) -> GroundSurface {
        GroundSurface {
            origin: self.grid.origin(),
            spacing_m: self.grid.spacing_m,
            rows: self.grid.rows,
            columns: self.grid.columns,
            elevation: self.elevation_m.values.clone(),
            microrelief: self.microrelief_m.values.clone(),
            material_channels: self
                .substrates
                .iter()
                .map(|plane| GroundMaterialChannel {
                    material: plane.material,
                    weights: plane.weights.values.clone(),
                })
                .collect(),
            modifier_channels: self
                .modifiers
                .iter()
                .map(|plane| GroundModifierChannel {
                    channel: plane.channel,
                    values: plane.values.values.clone(),
                })
                .collect(),
        }
    }

    /// This stack's fingerprint.
    pub fn fingerprint(&self) -> Fingerprint {
        let mut digest = Digest::for_domain(FIELD_DIGEST_DOMAIN);
        digest.u32(FIELD_STACK_VERSION);
        self.absorb(&mut digest);
        digest.finish()
    }
}

impl Digestible for TerrainFieldStack {
    fn absorb(&self, digest: &mut Digest) {
        self.grid.absorb(digest);
        self.elevation_m.absorb(digest);
        self.microrelief_m.absorb(digest);
        digest.slice(&self.substrates, |d, plane| {
            d.u32(plane.material.0 as u32);
            plane.weights.absorb(d);
        });
        digest.slice(&self.modifiers, |d, plane| {
            d.u32(plane.channel.0 as u32);
            plane.values.absorb(d);
        });
        digest.slice(&self.covers, |d, cover| {
            d.u32(cover.cover as u32);
            cover.depth_m.absorb(d);
            cover.coverage.absorb(d);
            cover.compaction.absorb(d);
            cover.wetness.absorb(d);
        });
        self.derived.absorb(digest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(min: (f64, f64), max: (f64, f64)) -> WorldRect {
        WorldRect::new(WorldPoint::new(min.0, min.1), WorldPoint::new(max.0, max.1))
    }

    #[test]
    fn a_grid_holds_one_more_sample_than_it_has_cells() {
        let grid = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: 0.25,
            columns: 4,
            rows: 2,
        };
        assert_eq!(grid.samples_across(), 5);
        assert_eq!(grid.samples_down(), 3);
        assert_eq!(grid.sample_count(), 15);
    }

    #[test]
    fn grids_at_one_spacing_share_a_world_lattice() {
        // The seam property, and the one that edge anchoring alone does not
        // give: two requests with unrelated bounds must still sample the same
        // world points wherever they overlap.
        let a = FieldGridSpec::covering(rect((-3.1, -3.1), (3.1, 3.1)), 0.05);
        let b = FieldGridSpec::covering(rect((0.37, -1.02), (5.0, 2.5)), 0.05);

        // Both origins are multiples of the spacing, so both lattices are
        // subsets of the same global one.
        // Both are integer offsets on one world lattice, so a point covered by
        // both has the *identical* coordinate in each rather than one an ulp
        // apart. Compared for exact equality on purpose: an approximate seam
        // guarantee is not a seam guarantee.
        let shift = (b.origin_index[0] - a.origin_index[0]) as u32;
        for column in 0..(a.columns - shift).min(b.columns) {
            assert_eq!(
                a.sample_position(column + shift, 0).u_m,
                b.sample_position(column, 0).u_m,
                "column {column} lands on a different world point"
            );
        }
    }

    #[test]
    fn a_grid_covers_the_rectangle_it_was_asked_for() {
        let bounds = rect((-1.0, -2.0), (1.0, 2.0));
        let grid = FieldGridSpec::covering(bounds, 0.5);
        let covered = grid.bounds();
        assert!(covered.min.u_m <= bounds.min.u_m);
        assert!(covered.min.v_m <= bounds.min.v_m);
        // Strictly greater, so the half-open rectangle contains its own far
        // edge rather than excluding the last row.
        assert!(covered.max.u_m > bounds.max.u_m);
        assert!(covered.max.v_m > bounds.max.v_m);
        assert!(covered.contains(bounds.max));
    }

    #[test]
    fn a_bilinear_read_interpolates_and_a_nearest_read_does_not() {
        let grid = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: 1.0,
            columns: 1,
            rows: 1,
        };
        let smooth = ScalarPlane {
            values: vec![0.0, 10.0, 0.0, 10.0],
            descriptor: FieldDescriptor::scalar("smooth", FieldUnit::Unitless),
        };
        let mid = WorldPoint::new(0.5, 0.5);
        assert!((smooth.sample(&grid, mid) - 5.0).abs() < 1.0e-5);

        // The same numbers read as identities must not average to five, which
        // is not one of them.
        let ids = ScalarPlane {
            values: vec![0.0, 10.0, 0.0, 10.0],
            descriptor: FieldDescriptor::categorical("ids"),
        };
        let value = ids.sample(&grid, mid);
        assert!(value == 0.0 || value == 10.0, "categorical read blended");
    }

    #[test]
    fn a_point_on_a_lattice_line_reads_its_own_sample_from_any_window() {
        // The counterexample that broke the guarantee. Lattice index 3 at a
        // spacing of 0.05 sits at 0.15000000000000002, and dividing that back
        // gives 3.0000000000000004 — so a naive floor put the point a hair into
        // cell 3 in a long grid and clamped it to cell 2 in a short one. With
        // samples of 0 and 1 either side, the two windows returned 0 and
        // 4.4e-16 for the same world point.
        let spacing = 0.05;
        let short = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: spacing,
            columns: 3,
            rows: 1,
        };
        let long = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: spacing,
            columns: 6,
            rows: 1,
        };
        let plane_for = |grid: &FieldGridSpec| ScalarPlane {
            // Zero up to sample 3, then one, so any leak past the line shows.
            values: (0..grid.sample_count())
                .map(|index| {
                    let column = index % grid.samples_across();
                    if column <= 3 { 0.0 } else { 1.0 }
                })
                .collect(),
            descriptor: FieldDescriptor::scalar("step", FieldUnit::Unitless),
        };

        let at = WorldPoint::new(3.0 * spacing, 0.0);
        let a = plane_for(&short).sample(&short, at);
        let b = plane_for(&long).sample(&long, at);
        assert_eq!(a, 0.0, "the short window did not return its own sample");
        assert_eq!(b, 0.0, "the long window leaked past the lattice line");
        assert_eq!(a, b, "two windows disagreed on a shared lattice point");
    }

    #[test]
    fn a_grid_contains_a_far_edge_that_lands_on_a_lattice_line() {
        // The other half of the same arithmetic. From -0.2 to exactly -3 x 0.05,
        // the float extent comes out a hair under one cell, so a span computed
        // by division dropped the cell holding the far edge — and the snapped
        // origin index came out one too low for the same reason.
        let spacing = 0.05;
        let bounds = rect((-0.2, -0.2), (-3.0 * spacing, -3.0 * spacing));
        let grid = FieldGridSpec::covering(bounds, spacing);
        let covered = grid.bounds();
        assert!(
            covered.min.u_m <= bounds.min.u_m,
            "origin {} is above the requested minimum {}",
            covered.min.u_m,
            bounds.min.u_m
        );
        assert!(
            covered.max.u_m > bounds.max.u_m,
            "grid maximum {} does not strictly contain {}",
            covered.max.u_m,
            bounds.max.u_m
        );
        assert!(covered.contains(bounds.max));
    }

    #[test]
    fn a_lattice_too_far_from_the_origin_to_resolve_is_refused() {
        // Past 2^53 consecutive indices round to one `f64`, so two samples land
        // on the same world point and the cell between them has no width.
        let grid = FieldGridSpec {
            origin_index: [1 << 60, 0],
            spacing_m: 0.05,
            columns: 4,
            rows: 4,
        };
        assert!(!grid.is_well_formed());
    }

    #[test]
    fn a_vector_plane_honours_the_filter_it_declares() {
        let grid = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: 1.0,
            columns: 1,
            rows: 1,
        };
        // Two orthogonal directions. Averaged they rotate to something that is
        // neither; read as nearest they stay one of the two.
        let build = |filter: FieldFilter| VectorPlane {
            u: vec![1.0, 0.0, 1.0, 0.0],
            v: vec![0.0, 1.0, 0.0, 1.0],
            descriptor: FieldDescriptor {
                filter,
                ..FieldDescriptor::scalar("dir", FieldUnit::Unitless)
            },
            unit_length: true,
        };
        let at = WorldPoint::new(0.25, 0.0);
        let nearest = build(FieldFilter::Nearest).sample_unit(&grid, at);
        assert_eq!(nearest, [1.0, 0.0], "a nearest plane was blended anyway");
        let bilinear = build(FieldFilter::Bilinear).sample_unit(&grid, at);
        assert!(bilinear[1] > 0.0, "a bilinear plane should blend");
    }

    #[test]
    fn a_wrapped_angle_may_not_be_read_bilinearly() {
        // +179 and -179 degrees both point nearly west; averaged they point
        // east. A plane of angles has to say it is read nearest.
        let mut descriptor = FieldDescriptor::scalar("aspect", FieldUnit::Radians);
        assert!(!descriptor.filter_is_legal());
        descriptor.filter = FieldFilter::Nearest;
        assert!(descriptor.filter_is_legal());
    }

    #[test]
    fn a_categorical_plane_may_not_be_read_bilinearly() {
        let mut descriptor = FieldDescriptor::categorical("ids");
        assert!(descriptor.filter_is_legal());
        descriptor.filter = FieldFilter::Bilinear;
        assert!(!descriptor.filter_is_legal());
    }

    #[test]
    fn reading_outside_the_grid_clamps_or_returns_the_declared_border() {
        let grid = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: 1.0,
            columns: 1,
            rows: 1,
        };
        let clamped = ScalarPlane {
            values: vec![1.0, 2.0, 3.0, 4.0],
            descriptor: FieldDescriptor::scalar("clamped", FieldUnit::Unitless),
        };
        // Far outside, and the nearest edge sample continues outward.
        assert_eq!(clamped.sample(&grid, WorldPoint::new(-50.0, -50.0)), 1.0);

        let bordered = ScalarPlane {
            values: vec![1.0, 2.0, 3.0, 4.0],
            descriptor: FieldDescriptor::scalar("bordered", FieldUnit::Unitless)
                .with_border(FieldBorder::Value(-1.0)),
        };
        assert_eq!(bordered.sample(&grid, WorldPoint::new(-50.0, -50.0)), -1.0);
        // Inside, the border rule does not apply.
        assert_eq!(bordered.sample(&grid, WorldPoint::ORIGIN), 1.0);
    }

    #[test]
    fn a_direction_stays_unit_length_across_a_disagreement() {
        // Two neighbouring samples pointing opposite ways average to nothing.
        // A plane that renormalises returns a direction anyway; one that did
        // not would hand a recipe a zero-length "direction" and let it divide
        // by the length.
        let grid = FieldGridSpec {
            origin_index: [0, 0],
            spacing_m: 1.0,
            columns: 1,
            rows: 1,
        };
        let plane = VectorPlane {
            u: vec![1.0, 0.6, 1.0, 0.6],
            v: vec![0.0, 0.8, 0.0, 0.8],
            descriptor: FieldDescriptor::scalar("dir", FieldUnit::Unitless),
            unit_length: true,
        };
        let sample = plane.sample_unit(&grid, WorldPoint::new(0.5, 0.5));
        let length = (sample[0] * sample[0] + sample[1] * sample[1]).sqrt();
        assert!((length - 1.0).abs() < 1.0e-5, "length was {length}");
    }

    #[test]
    fn the_fingerprint_moves_with_the_field_and_not_with_arithmetic_noise() {
        let grid = FieldGridSpec::covering(rect((0.0, 0.0), (1.0, 1.0)), 0.5);
        let base = TerrainFieldStack::flat(grid);
        let reference = base.fingerprint();

        let mut noise = base.clone();
        noise.elevation_m.values[2] += 1.0e-9;
        assert_eq!(reference, noise.fingerprint());

        let mut real = base.clone();
        real.elevation_m.values[2] += 0.01;
        assert_ne!(reference, real.fingerprint());
    }

    #[test]
    fn the_structural_subset_round_trips_into_a_ground_surface() {
        let grid = FieldGridSpec::covering(rect((0.0, 0.0), (2.0, 1.0)), 0.5);
        let mut stack = TerrainFieldStack::flat(grid);
        stack.substrates.push(MaterialPlane {
            material: MaterialIndex(3),
            weights: ScalarPlane::filled(
                &grid,
                1.0,
                FieldDescriptor::scalar("substrate.3", FieldUnit::Fraction),
            ),
        });
        let ground = stack.to_ground_surface();
        assert!(ground.is_well_formed());
        assert_eq!(ground.columns, grid.columns);
        assert_eq!(ground.rows, grid.rows);
        assert_eq!(ground.sample_count(), stack.sample_count());
        assert_eq!(ground.material_channels[0].material, MaterialIndex(3));
    }

    #[test]
    fn a_plane_of_the_wrong_length_is_caught() {
        let grid = FieldGridSpec::covering(rect((0.0, 0.0), (2.0, 2.0)), 0.5);
        let mut stack = TerrainFieldStack::flat(grid);
        assert!(stack.is_well_formed());
        stack.elevation_m.values.pop();
        assert!(!stack.is_well_formed());
    }
}

//! One thing that grows on the ground, as a value.
//!
//! ## A mark is a description, not geometry
//!
//! A page carries tens of thousands of marks, and a mark stays a hundred bytes
//! of parameters rather than becoming vertices. That is not premature
//! optimisation — a scene of descriptions is a megabyte, a scene of tessellated
//! ribbons is hundreds — but the memory is the smaller half of the argument.
//!
//! The larger half is that **the same mark is drawn more than once, at different
//! budgets**. Into a shadow map from the sun, into the camera's surface, and
//! into a Cycles export at a different rib count. Every one of those needs to
//! see exactly what the others saw, and geometry tessellated at one budget
//! cannot be re-tessellated at another without regenerating it — at which point
//! the two passes are two meadows and nothing says so.
//!
//! ## Four primitives, not four content types
//!
//! The renderer has no `render_grass` and no `render_wildflowers`. It has
//! ribbons, curves, analytic shapes and stamps, and *content recipes decide how
//! to use them*. A grass blade is a ribbon; so is a leaf. A wildflower stem is a
//! curve; so is a twig. That boundary is what stops the renderer growing a
//! method per ecological content type, which is the shape every one of these
//! systems degenerates into if the line is not held.
//!
//! ## Order is semantic and total
//!
//! [`PainterOrder`] is derived from what a mark *is* and where it *is*, never
//! from the sequence it happened to be generated in. Two marks that overlap must
//! resolve the same way in every renderer and on every run, and a sort key built
//! from generation order cannot promise that across a threaded build.

use terrain_core::digest::{Digest, Digestible};
use terrain_core::ids::MaterialIndex;
use terrain_core::seed::CandidateId;

use crate::projection::{Projection, ScenePoint};

/// A mark's stable identity.
///
/// Derived from the candidate that produced it, so the same mark has the same id
/// however many times the scene is rebuilt and whichever page it was built as
/// part of. Two things depend on that: a cryptomatte pass that has to name the
/// same blade twice, and a fingerprint that has to survive a reordering of the
/// generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MarkId(pub u64);

impl MarkId {
    /// The id of a candidate's `part`-th mark.
    ///
    /// A part index rather than a running counter, because a counter would make
    /// a mark's id depend on how many marks came before it — which is the same
    /// mistake as a sequential random stream, with the same consequence.
    pub fn of(candidate: CandidateId, part: u16) -> Self {
        // Chained rather than xored. Xoring several mixed values together loses
        // information wherever two of them share bits, and the collisions it
        // produces are not rare — a plain xor of `mix(x)` and a rotated `mix(y)`
        // collides within a sixteen-by-sixteen grid of cells. Feeding each value
        // through the mixer in sequence is one more multiply per field and has
        // no such structure.
        let mut state = terrain_core::seed::mix(candidate.population.bits());
        state = terrain_core::seed::mix(state ^ candidate.cell.x as u64);
        state = terrain_core::seed::mix(state ^ candidate.cell.y as u64);
        state = terrain_core::seed::mix(state ^ candidate.rank as u64);
        Self(terrain_core::seed::mix(state ^ part as u64))
    }
}

/// Which placement group in the scene's anchor table.
///
/// A dense index rather than a `CandidateId`, because every mark carries one and
/// a scene holds hundreds of thousands of marks. The table it indexes is small —
/// one entry per accepted candidate that emitted anything — and it is what turns
/// "these six primitives" into "this one plant".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AnchorIndex(pub u32);

impl AnchorIndex {
    /// The group a mark belongs to when nothing grouped it.
    ///
    /// Zero, and it is a real entry rather than a sentinel: the scene builder
    /// seeds its anchor table with one ungrouped placeholder so that a mark
    /// pushed without an anchor still indexes something valid. Validation
    /// reports marks that land here in a compile that was supposed to group
    /// everything, rather than letting a dangling index reach a renderer.
    pub const UNGROUPED: Self = Self(0);

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Which broad band of the picture a mark belongs to.
///
/// Coarse, and deliberately so. It exists to keep things that are *conceptually*
/// under or over other things in the right relationship even where their depths
/// nearly tie — a thatch mat is under the canopy whatever its root height says.
/// Everything finer than this is decided by depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[non_exhaustive]
pub enum Stratum {
    /// Lying on the ground: scuffs, fallen leaves, the mat under a canopy.
    Ground = 0,
    /// The bulk of a canopy.
    #[default]
    Canopy = 1,
    /// Standing above the canopy: seed heads, flowers, tall stems.
    Emergent = 2,
}

/// A total order for drawing.
///
/// Packed into one integer so a sort is a single comparison, and built from
/// semantic values so it is the same on every run:
///
/// ```text
///  bits 62..64   stratum
///  bits 22..62   quantised depth, nearer is larger
///  bits 16..22   sublayer within the mark
///  bits  0..16   stable id, to break exact ties
/// ```
///
/// The tie-break is the part that is easy to leave out and expensive to add
/// later. Two marks at genuinely equal depth are common — a fork's two children,
/// a tuft's blades sharing a root — and without a deterministic break they
/// resolve by whatever order the sort happened to leave them in, which is not
/// stable across a threaded build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PainterOrder(u64);

/// Depth quantisation, in steps per metre.
///
/// A tenth of a millimetre over the ±2^39-step range the field allows, which
/// covers a world about fifty kilometres across. Fine enough that no two marks
/// tie by accident; coarse enough that a depth difference below it is genuinely
/// below what any renderer resolves.
const DEPTH_STEPS: f64 = 10_000.0;

/// The zero point of the depth field, so negative depths stay orderable.
const DEPTH_BIAS: i64 = 1 << 39;

impl PainterOrder {
    /// Build an order from what a mark is and where it is.
    pub fn new(stratum: Stratum, depth: f64, sublayer: u8, id: MarkId) -> Self {
        // Clamped before the bias is added, not after: a depth of 1e30 saturates
        // the cast to `i64::MAX`, and adding the bias to that overflows. The
        // clamp has to bound the value while it can still be represented.
        let steps = if depth.is_finite() {
            (depth * DEPTH_STEPS)
                .round()
                .clamp(-(DEPTH_BIAS as f64), (DEPTH_BIAS - 1) as f64) as i64
        } else {
            0
        };
        let biased = (steps + DEPTH_BIAS).clamp(0, (1 << 40) - 1) as u64;
        Self(
            ((stratum as u64) << 62)
                | (biased << 22)
                | (((sublayer & 0x3f) as u64) << 16)
                | (id.0 & 0xffff),
        )
    }

    /// Build an order by projecting a mark's root.
    pub fn at(
        stratum: Stratum,
        projection: Projection,
        root: ScenePoint,
        sublayer: u8,
        id: MarkId,
    ) -> Self {
        Self::new(stratum, projection.depth(root), sublayer, id)
    }

    pub fn bits(self) -> u64 {
        self.0
    }

    pub fn stratum(self) -> u8 {
        (self.0 >> 62) as u8
    }
}

/// How a ribbon's width varies from root to tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum WidthProfile {
    /// Widest at the root, tapering to a point.
    Tapered,
    /// Narrow at both ends, widest in the middle.
    Oval,
    /// Nearly constant.
    Stem,
    /// Narrow where it attaches, broadest a third of the way up, then a long
    /// taper and a quick point. What actual grass does.
    #[default]
    Leaf,
}

/// What happens at the end of a ribbon.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[non_exhaustive]
pub enum TipShape {
    /// It comes to a point. Most ribbons.
    #[default]
    Pointed,
    /// Torn or blunt.
    Notched { depth: f32 },
    /// Split in two.
    Forked {
        split_at: f32,
        opening_rad: f32,
        long: f32,
        short: f32,
    },
}

/// A ribbon's centreline and cross-section, in world units.
///
/// Everything here is metres or radians. The rasteriser's widths used to be in
/// cache pixels, which tied the description of a blade to the resolution it
/// happened to be drawn at — so the same scene handed to a renderer at a
/// different scale produced blades of the wrong thickness, and the fix was a
/// fudge factor at the boundary. World units remove the fudge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonGeometry {
    /// Arc length, metres.
    pub length_m: f32,
    /// Ground direction it leans toward, radians from `+u` toward `+v`.
    pub azimuth_rad: f32,
    /// Bend away from vertical at the tip, radians. Past `π/2` the tip falls.
    pub bend_rad: f32,
    /// Extra bend concentrated in the last third — the hook.
    pub curl_rad: f32,
    /// Lateral drift of the lean along the ribbon, radians. Makes S curves.
    pub sway_rad: f32,
    /// An abrupt change of bend partway along, radians.
    ///
    /// The difference between a mark and a curve. Every smooth arc in a field of
    /// smooth arcs advertises the function that drew it; an elbow does not,
    /// because no continuous parameter produces one.
    pub kink_rad: f32,
    /// Where along the ribbon the kink happens, `0..1`.
    pub kink_at: f32,
    /// Sideways component of the kink, radians.
    pub kink_turn_rad: f32,
    /// How far the face rotates about its own axis, root to tip, radians.
    ///
    /// Cheap and close to the most valuable thing in the vocabulary. Without it
    /// every ribbon in a tuft presents the same face to the sun, every highlight
    /// lands in the same place, and the tuft reads as a comb however varied its
    /// shapes are.
    pub twist_rad: f32,
    /// Half-width at the root, metres.
    pub width_m: f32,
    /// Width the tip never falls below, metres.
    pub tip_width_m: f32,
    pub profile: WidthProfile,
    pub tip: TipShape,
    /// How far the centre stands proud of the edges, as a fraction of the
    /// half-width.
    pub ridge: f32,
}

impl Default for RibbonGeometry {
    fn default() -> Self {
        Self {
            length_m: 0.22,
            azimuth_rad: 0.0,
            bend_rad: 0.5,
            curl_rad: 0.0,
            sway_rad: 0.0,
            kink_rad: 0.0,
            kink_at: 0.5,
            kink_turn_rad: 0.0,
            twist_rad: 0.0,
            width_m: 0.004,
            tip_width_m: 0.001,
            profile: WidthProfile::Leaf,
            tip: TipShape::Pointed,
            ridge: 0.34,
        }
    }
}

impl RibbonGeometry {
    /// A bound, in metres, on how far this ribbon's geometry reaches from its
    /// own root.
    ///
    /// Genuinely an upper bound, and it has to be: a mark wrongly rejected is
    /// present on one side of a page join and missing on the other. An arc of
    /// length `L` cannot displace its tip further than `L`, and a forked tip
    /// continues past the parent rather than replacing it, so its extra reach is
    /// added before the width.
    pub fn reach_m(&self) -> f32 {
        let extra = match self.tip {
            TipShape::Forked {
                split_at,
                long,
                short,
                ..
            } => (split_at + long.max(short) - 1.0).max(0.0),
            _ => 0.0,
        };
        self.length_m.abs() * (1.0 + extra) + self.width_m.abs() + self.tip_width_m.abs()
    }
}

/// What a mark is made of and what it looks like, apart from its shape.
///
/// **Intrinsic only.** How old this blade is, how wet the ground under it is,
/// which way it faces — all properties of the plant. What is *not* here is
/// anything about the current light: how much this face catches the sun is a
/// renderer's question, and putting it on the mark would mean the scene had to
/// be regenerated to change the time of day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkAttributes {
    /// How established this mark is, `0..1`.
    pub maturity: f32,
    /// How wet the ground it grew from is, `0..1`.
    pub moisture: f32,
    /// How exposed it is to the sky, `0..1`. A property of the canopy around it
    /// rather than of any particular sun.
    pub exposure: f32,
    /// A per-mark colour drift, `-1..1`, within its material's own family.
    pub tint: f32,
    /// A free intrinsic axis a recipe may use for anything, `0..1`.
    ///
    /// Present because the alternative is a recipe adding a field to this struct
    /// every time it wants one, and every renderer then having to carry it.
    pub variation: f32,
}

impl Default for MarkAttributes {
    fn default() -> Self {
        Self {
            maturity: 0.5,
            moisture: 0.5,
            exposure: 1.0,
            tint: 0.0,
            variation: 0.5,
        }
    }
}

/// An axis-aligned box in scene space, metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: ScenePoint,
    pub max: ScenePoint,
}

impl Aabb3 {
    /// A box around a point, reaching `radius_m` in every direction.
    pub fn around(centre: ScenePoint, radius_m: f64) -> Self {
        Self {
            min: ScenePoint::new(
                centre.u_m - radius_m,
                centre.v_m - radius_m,
                centre.z_m - radius_m,
            ),
            max: ScenePoint::new(
                centre.u_m + radius_m,
                centre.v_m + radius_m,
                centre.z_m + radius_m,
            ),
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: ScenePoint::new(
                self.min.u_m.min(other.min.u_m),
                self.min.v_m.min(other.min.v_m),
                self.min.z_m.min(other.min.z_m),
            ),
            max: ScenePoint::new(
                self.max.u_m.max(other.max.u_m),
                self.max.v_m.max(other.max.v_m),
                self.max.z_m.max(other.max.z_m),
            ),
        }
    }

    /// The tallest point in the box.
    pub fn ceiling_m(self) -> f64 {
        self.max.z_m
    }
}

/// Which renderer-side material a mark uses.
///
/// An index into the scene's own binding table rather than a document material
/// index, because a mark's *appearance* and the ground's *composition* are
/// different questions: a blade of grass growing on ground that is 70% grass and
/// 30% dirt is still made of grass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneMaterialIndex(pub u16);

/// A tapered ribbon: a blade, a leaf, a strap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonMark {
    pub stable_id: MarkId,
    /// The placement group this mark belongs to.
    ///
    /// A flower is a stem and a head; a rosette is several leaves. They are one
    /// *plant*, and a trace slice that kept the stem and dropped the head would
    /// render half a flower. See [`crate::scene::PlacementAnchor`].
    pub anchor: AnchorIndex,
    pub order: PainterOrder,
    pub material: SceneMaterialIndex,
    pub root: ScenePoint,
    pub geometry: RibbonGeometry,
    pub attributes: MarkAttributes,
    pub bounds: Aabb3,
}

/// A curve with a round cross-section: a stem, a twig, a runner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveMark {
    pub stable_id: MarkId,
    /// The placement group this mark belongs to.
    ///
    /// A flower is a stem and a head; a rosette is several leaves. They are one
    /// *plant*, and a trace slice that kept the stem and dropped the head would
    /// render half a flower. See [`crate::scene::PlacementAnchor`].
    pub anchor: AnchorIndex,
    pub order: PainterOrder,
    pub material: SceneMaterialIndex,
    pub root: ScenePoint,
    pub length_m: f32,
    pub azimuth_rad: f32,
    pub bend_rad: f32,
    pub radius_m: f32,
    pub tip_radius_m: f32,
    pub attributes: MarkAttributes,
    pub bounds: Aabb3,
}

/// An analytic shape lying on or near the ground: a pebble, a scuff, a patch.
///
/// Analytic rather than meshed because at the sizes these are used, a mesh is
/// more data than the thing it describes and a renderer can evaluate the shape
/// more accurately than any tessellation of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalyticMark {
    pub stable_id: MarkId,
    /// The placement group this mark belongs to.
    ///
    /// A flower is a stem and a head; a rosette is several leaves. They are one
    /// *plant*, and a trace slice that kept the stem and dropped the head would
    /// render half a flower. See [`crate::scene::PlacementAnchor`].
    pub anchor: AnchorIndex,
    pub order: PainterOrder,
    pub material: SceneMaterialIndex,
    pub centre: ScenePoint,
    /// Semi-axes on the ground, metres.
    pub radius_m: [f32; 2],
    /// How high the shape stands at its centre, metres. Zero is flat.
    pub height_m: f32,
    pub rotation_rad: f32,
    pub attributes: MarkAttributes,
    pub bounds: Aabb3,
}

/// An authored image, placed on the ground or standing on it.
///
/// The escape hatch for silhouettes that are recognisable rather than
/// procedural: a specific flower head, a maple leaf. Procedural marks are better
/// where the eye reads texture; a stamp is better where the eye reads *what the
/// thing is*, and no amount of parameterisation gets a procedural generator to a
/// recognisable species.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StampMark {
    pub stable_id: MarkId,
    /// The placement group this mark belongs to.
    ///
    /// A flower is a stem and a head; a rosette is several leaves. They are one
    /// *plant*, and a trace slice that kept the stem and dropped the head would
    /// render half a flower. See [`crate::scene::PlacementAnchor`].
    pub anchor: AnchorIndex,
    pub order: PainterOrder,
    pub material: SceneMaterialIndex,
    /// Which stamp in the scene's stamp table.
    pub stamp: u16,
    /// Where on the ground the stamp sits.
    ///
    /// Named `centre` rather than `anchor` since marks gained a placement
    /// group: two different meanings of the same word on one struct is the
    /// kind of thing that reads fine until somebody wires the wrong one.
    pub centre: ScenePoint,
    /// Size on the ground, metres.
    pub size_m: [f32; 2],
    pub rotation_rad: f32,
    /// Whether the stamp lies on the ground or faces the camera.
    pub upright: bool,
    pub attributes: MarkAttributes,
    pub bounds: Aabb3,
}

/// One thing in the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum SceneMark {
    Ribbon(RibbonMark),
    Curve(CurveMark),
    Analytic(AnalyticMark),
    Stamp(StampMark),
}

impl SceneMark {
    pub fn stable_id(&self) -> MarkId {
        match self {
            Self::Ribbon(m) => m.stable_id,
            Self::Curve(m) => m.stable_id,
            Self::Analytic(m) => m.stable_id,
            Self::Stamp(m) => m.stable_id,
        }
    }

    pub fn order(&self) -> PainterOrder {
        match self {
            Self::Ribbon(m) => m.order,
            Self::Curve(m) => m.order,
            Self::Analytic(m) => m.order,
            Self::Stamp(m) => m.order,
        }
    }

    pub fn material(&self) -> SceneMaterialIndex {
        match self {
            Self::Ribbon(m) => m.material,
            Self::Curve(m) => m.material,
            Self::Analytic(m) => m.material,
            Self::Stamp(m) => m.material,
        }
    }

    pub fn bounds(&self) -> Aabb3 {
        match self {
            Self::Ribbon(m) => m.bounds,
            Self::Curve(m) => m.bounds,
            Self::Analytic(m) => m.bounds,
            Self::Stamp(m) => m.bounds,
        }
    }

    /// Where the mark meets the ground.
    pub fn root(&self) -> ScenePoint {
        match self {
            Self::Ribbon(m) => m.root,
            Self::Curve(m) => m.root,
            Self::Analytic(m) => m.centre,
            Self::Stamp(m) => m.centre,
        }
    }

    pub fn attributes(&self) -> MarkAttributes {
        match self {
            Self::Ribbon(m) => m.attributes,
            Self::Curve(m) => m.attributes,
            Self::Analytic(m) => m.attributes,
            Self::Stamp(m) => m.attributes,
        }
    }

    /// The placement group this mark belongs to.
    pub fn anchor(&self) -> AnchorIndex {
        match self {
            Self::Ribbon(m) => m.anchor,
            Self::Curve(m) => m.anchor,
            Self::Analytic(m) => m.anchor,
            Self::Stamp(m) => m.anchor,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Ribbon(_) => "ribbon",
            Self::Curve(_) => "curve",
            Self::Analytic(_) => "analytic",
            Self::Stamp(_) => "stamp",
        }
    }

    fn tag(&self) -> u8 {
        match self {
            Self::Ribbon(_) => 0,
            Self::Curve(_) => 1,
            Self::Analytic(_) => 2,
            Self::Stamp(_) => 3,
        }
    }
}

fn absorb_point(digest: &mut Digest, point: ScenePoint) {
    digest.f64(point.u_m).f64(point.v_m).f64(point.z_m);
}

fn absorb_attributes(digest: &mut Digest, attributes: MarkAttributes) {
    digest
        .f32(attributes.maturity)
        .f32(attributes.moisture)
        .f32(attributes.exposure)
        .f32(attributes.tint)
        .f32(attributes.variation);
}

impl Digestible for SceneMark {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .tag(self.tag())
            .u64(self.stable_id().0)
            .u64(self.order().bits())
            .u32(self.material().0 as u32);
        absorb_point(digest, self.root());
        absorb_attributes(digest, self.attributes());

        match self {
            Self::Ribbon(mark) => {
                let g = &mark.geometry;
                digest
                    .f32(g.length_m)
                    .f32(g.azimuth_rad)
                    .f32(g.bend_rad)
                    .f32(g.curl_rad)
                    .f32(g.sway_rad)
                    .f32(g.kink_rad)
                    .f32(g.kink_at)
                    .f32(g.kink_turn_rad)
                    .f32(g.twist_rad)
                    .f32(g.width_m)
                    .f32(g.tip_width_m)
                    .tag(g.profile as u8)
                    .f32(g.ridge);
                match g.tip {
                    TipShape::Pointed => {
                        digest.tag(0);
                    }
                    TipShape::Notched { depth } => {
                        digest.tag(1).f32(depth);
                    }
                    TipShape::Forked {
                        split_at,
                        opening_rad,
                        long,
                        short,
                    } => {
                        digest
                            .tag(2)
                            .f32(split_at)
                            .f32(opening_rad)
                            .f32(long)
                            .f32(short);
                    }
                }
            }
            Self::Curve(mark) => {
                digest
                    .f32(mark.length_m)
                    .f32(mark.azimuth_rad)
                    .f32(mark.bend_rad)
                    .f32(mark.radius_m)
                    .f32(mark.tip_radius_m);
            }
            Self::Analytic(mark) => {
                digest
                    .f32(mark.radius_m[0])
                    .f32(mark.radius_m[1])
                    .f32(mark.height_m)
                    .f32(mark.rotation_rad);
            }
            Self::Stamp(mark) => {
                digest
                    .u32(mark.stamp as u32)
                    .f32(mark.size_m[0])
                    .f32(mark.size_m[1])
                    .f32(mark.rotation_rad)
                    .bool(mark.upright);
            }
        }
    }
}

/// The renderer-side appearance a scene material binds to.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneMaterialBinding {
    /// The stable appearance key: `plant.grass_blade`, `rock.granite`.
    pub appearance: terrain_core::ids::AppearanceKey,
    /// The document material this appearance stands for, when there is one.
    ///
    /// Optional because a mark's appearance need not correspond to a *ground*
    /// material at all: a wildflower head is a material in the renderer's sense
    /// and not in the terrain's.
    pub terrain_material: Option<MaterialIndex>,
}

impl Digestible for SceneMaterialBinding {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(self.appearance.as_str());
        match self.terrain_material {
            Some(index) => {
                digest.tag(1).u32(index.0 as u32);
            }
            None => {
                digest.tag(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::coords::CellCoord;
    use terrain_core::seed::PopulationHash;

    fn candidate(x: i64, y: i64, rank: u16) -> CandidateId {
        CandidateId::new(
            PopulationHash::from_bits(0x1234_5678),
            CellCoord::new(x, y),
            rank,
        )
    }

    #[test]
    fn a_marks_id_is_a_function_of_its_candidate() {
        // The property a cryptomatte pass and the fingerprint both need: the
        // same mark has the same id however the scene was built.
        assert_eq!(
            MarkId::of(candidate(3, -7, 2), 0),
            MarkId::of(candidate(3, -7, 2), 0)
        );
        assert_ne!(
            MarkId::of(candidate(3, -7, 2), 0),
            MarkId::of(candidate(4, -7, 2), 0)
        );
        assert_ne!(
            MarkId::of(candidate(3, -7, 2), 0),
            MarkId::of(candidate(3, -6, 2), 0)
        );
        assert_ne!(
            MarkId::of(candidate(3, -7, 2), 0),
            MarkId::of(candidate(3, -7, 3), 0)
        );
        assert_ne!(
            MarkId::of(candidate(3, -7, 2), 0),
            MarkId::of(candidate(3, -7, 2), 1)
        );
    }

    #[test]
    fn ids_do_not_collide_across_a_grid_of_candidates() {
        let mut seen = std::collections::BTreeSet::new();
        for x in -8..8i64 {
            for y in -8..8i64 {
                for rank in 0..4u16 {
                    assert!(
                        seen.insert(MarkId::of(candidate(x, y, rank), 0).0),
                        "collision at [{x}, {y}] #{rank}"
                    );
                }
            }
        }
    }

    #[test]
    fn nearer_marks_sort_after_further_ones() {
        // Larger is nearer, matching the projection's depth, so a plain sort
        // puts the painter's order in the right sequence.
        let far = PainterOrder::new(Stratum::Canopy, 1.0, 0, MarkId(0));
        let near = PainterOrder::new(Stratum::Canopy, 5.0, 0, MarkId(0));
        assert!(near > far);
    }

    #[test]
    fn a_stratum_outranks_depth() {
        // A thatch mat is under the canopy whatever its root height says.
        let ground_near = PainterOrder::new(Stratum::Ground, 1000.0, 0, MarkId(0));
        let canopy_far = PainterOrder::new(Stratum::Canopy, -1000.0, 0, MarkId(0));
        assert!(canopy_far > ground_near);
    }

    #[test]
    fn exact_ties_break_deterministically_by_id() {
        // Two marks at genuinely equal depth are common — a fork's children, a
        // tuft's blades sharing a root — and without this they resolve by
        // whatever order the sort happened to leave them in.
        let first = PainterOrder::new(Stratum::Canopy, 2.0, 0, MarkId(0x1111));
        let second = PainterOrder::new(Stratum::Canopy, 2.0, 0, MarkId(0x2222));
        assert_ne!(first, second);
        assert!(second > first);

        let mut orders = vec![second, first];
        orders.sort();
        assert_eq!(orders, vec![first, second]);
    }

    #[test]
    fn a_sublayer_orders_within_one_mark() {
        let under = PainterOrder::new(Stratum::Canopy, 2.0, 0, MarkId(0));
        let over = PainterOrder::new(Stratum::Canopy, 2.0, 1, MarkId(0));
        assert!(over > under);
    }

    #[test]
    fn negative_depths_stay_orderable() {
        // Half the world has a negative depth, and an unbiased field would wrap
        // it to the far side of the order.
        let below = PainterOrder::new(Stratum::Canopy, -5.0, 0, MarkId(0));
        let above = PainterOrder::new(Stratum::Canopy, -1.0, 0, MarkId(0));
        let positive = PainterOrder::new(Stratum::Canopy, 1.0, 0, MarkId(0));
        assert!(above > below);
        assert!(positive > above);
    }

    #[test]
    fn an_absurd_depth_clamps_rather_than_wrapping() {
        // A wrapped depth would put a mark at the wrong end of the order, which
        // reads as one blade drawn over everything.
        let huge = PainterOrder::new(Stratum::Canopy, 1.0e30, 0, MarkId(0));
        let ordinary = PainterOrder::new(Stratum::Canopy, 1.0, 0, MarkId(0));
        assert!(huge > ordinary);
        let nan = PainterOrder::new(Stratum::Canopy, f64::NAN, 0, MarkId(0));
        assert!(nan.bits() > 0);
    }

    #[test]
    fn ordering_from_a_projected_root_agrees_with_the_projection() {
        let projection = Projection::default();
        let near = ScenePoint::new(5.0, 5.0, 0.0);
        let far = ScenePoint::new(-5.0, -5.0, 0.0);
        assert!(
            PainterOrder::at(Stratum::Canopy, projection, near, 0, MarkId(0))
                > PainterOrder::at(Stratum::Canopy, projection, far, 0, MarkId(0))
        );
    }

    #[test]
    fn a_ribbons_reach_bounds_its_own_tip() {
        // Genuinely an upper bound: a mark wrongly rejected is present on one
        // side of a page join and missing on the other.
        let plain = RibbonGeometry::default();
        assert!(plain.reach_m() >= plain.length_m);

        let forked = RibbonGeometry {
            tip: TipShape::Forked {
                split_at: 0.8,
                opening_rad: 0.5,
                long: 0.4,
                short: 0.2,
            },
            ..plain
        };
        // The fork continues past the parent, so it genuinely reaches further.
        assert!(forked.reach_m() > plain.reach_m());
    }

    #[test]
    fn every_ribbon_parameter_reaches_the_digest() {
        // The maintenance contract: a parameter added to the vocabulary and not
        // digested is a parameter no fingerprint can prove was preserved.
        let base = SceneMark::Ribbon(RibbonMark {
            stable_id: MarkId(1),
            anchor: AnchorIndex::UNGROUPED,
            order: PainterOrder::new(Stratum::Canopy, 0.0, 0, MarkId(1)),
            material: SceneMaterialIndex(0),
            root: ScenePoint::default(),
            geometry: RibbonGeometry::default(),
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(ScenePoint::default(), 1.0),
        });
        let reference = base.fingerprint("mark");

        type Nudge = (&'static str, fn(&mut RibbonGeometry));
        let nudges: [Nudge; 13] = [
            ("length_m", |g| g.length_m += 1.0),
            ("azimuth_rad", |g| g.azimuth_rad += 1.0),
            ("bend_rad", |g| g.bend_rad += 1.0),
            ("curl_rad", |g| g.curl_rad += 1.0),
            ("sway_rad", |g| g.sway_rad += 1.0),
            ("kink_rad", |g| g.kink_rad += 1.0),
            ("kink_at", |g| g.kink_at += 1.0),
            ("kink_turn_rad", |g| g.kink_turn_rad += 1.0),
            ("twist_rad", |g| g.twist_rad += 1.0),
            ("width_m", |g| g.width_m += 1.0),
            ("tip_width_m", |g| g.tip_width_m += 1.0),
            ("profile", |g| g.profile = WidthProfile::Oval),
            ("ridge", |g| g.ridge += 1.0),
        ];
        for (name, nudge) in nudges {
            let SceneMark::Ribbon(mut ribbon) = base else {
                unreachable!()
            };
            nudge(&mut ribbon.geometry);
            assert_ne!(
                reference,
                SceneMark::Ribbon(ribbon).fingerprint("mark"),
                "{name} does not reach the digest"
            );
        }

        // The tip, whose variants must be told apart by their tags.
        let SceneMark::Ribbon(mut notched) = base else {
            unreachable!()
        };
        notched.geometry.tip = TipShape::Notched { depth: 0.0 };
        assert_ne!(reference, SceneMark::Ribbon(notched).fingerprint("mark"));
    }

    #[test]
    fn a_marks_attributes_reach_the_digest() {
        let make = |attributes: MarkAttributes| {
            SceneMark::Ribbon(RibbonMark {
                stable_id: MarkId(1),
                anchor: AnchorIndex::UNGROUPED,
                order: PainterOrder::new(Stratum::Canopy, 0.0, 0, MarkId(1)),
                material: SceneMaterialIndex(0),
                root: ScenePoint::default(),
                geometry: RibbonGeometry::default(),
                attributes,
                bounds: Aabb3::around(ScenePoint::default(), 1.0),
            })
            .fingerprint("mark")
        };
        let reference = make(MarkAttributes::default());
        for nudge in [
            |a: &mut MarkAttributes| a.maturity += 0.1,
            |a: &mut MarkAttributes| a.moisture += 0.1,
            |a: &mut MarkAttributes| a.exposure -= 0.1,
            |a: &mut MarkAttributes| a.tint += 0.1,
            |a: &mut MarkAttributes| a.variation += 0.1,
        ] {
            let mut attributes = MarkAttributes::default();
            nudge(&mut attributes);
            assert_ne!(reference, make(attributes));
        }
    }

    #[test]
    fn the_four_primitives_are_told_apart() {
        let id = MarkId(7);
        let order = PainterOrder::new(Stratum::Canopy, 0.0, 0, id);
        let point = ScenePoint::default();
        let bounds = Aabb3::around(point, 1.0);
        let material = SceneMaterialIndex(0);
        let attributes = MarkAttributes::default();

        let marks = [
            SceneMark::Ribbon(RibbonMark {
                stable_id: id,
                anchor: AnchorIndex::UNGROUPED,
                order,
                material,
                root: point,
                geometry: RibbonGeometry::default(),
                attributes,
                bounds,
            }),
            SceneMark::Curve(CurveMark {
                stable_id: id,
                anchor: AnchorIndex::UNGROUPED,
                order,
                material,
                root: point,
                length_m: 0.0,
                azimuth_rad: 0.0,
                bend_rad: 0.0,
                radius_m: 0.0,
                tip_radius_m: 0.0,
                attributes,
                bounds,
            }),
            SceneMark::Analytic(AnalyticMark {
                stable_id: id,
                anchor: AnchorIndex::UNGROUPED,
                order,
                material,
                centre: point,
                radius_m: [0.0, 0.0],
                height_m: 0.0,
                rotation_rad: 0.0,
                attributes,
                bounds,
            }),
            SceneMark::Stamp(StampMark {
                stable_id: id,
                anchor: AnchorIndex::UNGROUPED,
                order,
                material,
                stamp: 0,
                centre: point,
                size_m: [0.0, 0.0],
                rotation_rad: 0.0,
                upright: false,
                attributes,
                bounds,
            }),
        ];
        let mut seen = Vec::new();
        for mark in &marks {
            let fingerprint = mark.fingerprint("mark");
            assert!(
                !seen.contains(&fingerprint),
                "{} collided",
                mark.kind_name()
            );
            seen.push(fingerprint);
        }
    }

    #[test]
    fn a_box_around_a_point_reports_its_own_ceiling() {
        let box_ = Aabb3::around(ScenePoint::new(0.0, 0.0, 0.5), 0.25);
        assert_eq!(box_.ceiling_m(), 0.75);
        let wider = box_.union(Aabb3::around(ScenePoint::new(0.0, 0.0, 2.0), 0.5));
        assert_eq!(wider.ceiling_m(), 2.5);
        assert_eq!(wider.min.z_m, 0.25);
    }
}

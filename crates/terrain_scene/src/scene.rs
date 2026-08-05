//! The scene: what to render, once, for every renderer.
//!
//! ## Built once, rendered many times
//!
//! This is the contract the whole framework is arranged around. A
//! [`TerrainScene`] is built from a prepared terrain and then handed —
//! unchanged, by reference — to the path tracer, the debug plate and the
//! dataset exporter.
//!
//! Regenerating instead would nearly work. Placement is a pure function of world
//! position, so two runs would agree. It is not the agreement that matters; it
//! is that nothing can *later* make them disagree. A quality tier that skipped a
//! fork, a rib count that moved a vertex, an optimisation that reordered a draw
//! — any of those turns a training pair into two photographs of two different
//! meadows, and the failure is silent: the loss stops falling and no image in
//! the corpus looks wrong.
//!
//! So the API makes the wrong thing hard to write. There is no function here
//! that takes two generators.
//!
//! ## What may not be in a scene
//!
//! No Bevy entity ids or handles. No Blender objects. No GPU bind groups. No
//! camera components. Nothing that names a renderer, because the moment one
//! appears the scene stops being the thing all of them consume and becomes one
//! renderer's input that the others adapt from.
//!
//! Positions are `f64` scene metres, orientations are radians, sizes are metres.
//! A renderer narrows to whatever it wants at the boundary.

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::digest::{Digest, Digestible, Fingerprint};

use terrain_core::seed::CandidateId;

use crate::ground::GroundSurface;
use crate::instance::{PrototypeBinding, PrototypeIndex, PrototypeInstance};
use crate::mark::{Aabb3, AnchorIndex, MarkId, SceneMark, SceneMaterialBinding};
use crate::projection::{Projection, ScenePoint, ScreenRect};

/// What was asked for.
///
/// Carried on the scene rather than passed alongside it, because every consumer
/// needs it and a scene separated from its request is a scene nobody can frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneRequest {
    /// The ground the output covers, not counting the halo.
    ///
    /// What is *visible*, in world terms. For a tile layout it is exactly the
    /// union of the tiles.
    pub bounds: WorldRect,
    /// The camera's window on the screen plane, in screen metres.
    ///
    /// Separate from [`SceneRequest::bounds`] because the two answer different
    /// questions and are not the same shape. `bounds` says which ground the
    /// render is *about*; the viewport says what rectangle the camera
    /// photographs. A ground rectangle projects to a diamond, so a frame derived
    /// from the ground bounds can only ever be that diamond's bounding box —
    /// which leaves no way to say "put the subject tile in the middle and leave
    /// five percent of background around the outside".
    ///
    /// Both renderers read this, so the cheap plate and the traced plate
    /// register pixel for pixel.
    pub viewport: ScreenRect,
    pub projection: Projection,
    /// The output raster's size, in pixels.
    pub output_size: [u32; 2],
    /// Pixels per world metre the output is shown at.
    ///
    /// Redundant with [`SceneRequest::viewport`] and the output size, and kept
    /// because a great deal of downstream code asks for a scale rather than for
    /// a window. [`SceneRequest::viewport_pixels_per_metre`] is the number the
    /// viewport implies; the two agreeing is asserted rather than assumed.
    pub pixels_per_metre: f32,
    /// Level of detail: zero is full, each step is half the linear resolution.
    pub lod: u8,
    /// How far beyond [`SceneRequest::bounds`] to generate.
    ///
    /// A mark rooted outside the rectangle still casts a shadow and occludes
    /// inward, so a scene generated to its own edge has a bright seam there.
    /// This is what a bake sizes from a population's reach and a source's.
    pub halo_m: f64,
}

impl SceneRequest {
    /// A request over a square of ground at a chosen scale.
    ///
    /// A square *window*, on the ground point at the centre — which is not the
    /// same as the square of ground's own projected extent, and deliberately
    /// so: this is the laboratory plate it always was, and its framing has not
    /// moved. A caller that wants the diamond to fill the frame asks the frame
    /// resolver for it.
    pub fn square(centre: WorldPoint, side_m: f64, pixels_per_metre: f32) -> Self {
        let bounds = WorldRect::centred(centre, side_m);
        let projection = Projection::default();
        let pixels = (side_m * pixels_per_metre as f64).round().max(1.0) as u32;
        Self {
            bounds,
            viewport: ScreenRect::around(
                projection.project(ScenePoint::on_ground(centre)),
                side_m,
                side_m,
            ),
            projection,
            output_size: [pixels, pixels],
            pixels_per_metre,
            lod: 0,
            halo_m: 0.0,
        }
    }

    /// The scale the viewport and the output size imply.
    ///
    /// Should equal [`SceneRequest::pixels_per_metre`]. Two ways of saying the
    /// same thing is a place for them to drift, so this is what a test compares
    /// against rather than a number anyone maintains by hand.
    pub fn viewport_pixels_per_metre(&self) -> f32 {
        let width = self.viewport.width_m();
        if width <= 0.0 {
            return 0.0;
        }
        self.output_size[0] as f32 / width as f32
    }

    /// A request framed by an explicit camera window.
    pub fn with_viewport(mut self, viewport: ScreenRect) -> Self {
        self.viewport = viewport;
        self
    }

    pub fn with_halo(mut self, halo_m: f64) -> Self {
        self.halo_m = halo_m;
        self
    }

    pub fn with_lod(mut self, lod: u8) -> Self {
        self.lod = lod;
        self
    }

    /// The ground actually generated, including the halo.
    pub fn generated_bounds(&self) -> WorldRect {
        self.bounds.expanded(self.halo_m)
    }

    /// Pixels per metre after the level of detail is applied.
    pub fn effective_pixels_per_metre(&self) -> f32 {
        self.pixels_per_metre / (1u32 << self.lod.min(16)) as f32
    }
}

impl Digestible for SceneRequest {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .f64(self.bounds.min.u_m)
            .f64(self.bounds.min.v_m)
            .f64(self.bounds.max.u_m)
            .f64(self.bounds.max.v_m)
            .f64(self.viewport.min.x_m)
            .f64(self.viewport.min.y_m)
            .f64(self.viewport.max.x_m)
            .f64(self.viewport.max.y_m)
            .f64(self.projection.half_width)
            .f64(self.projection.half_height)
            .f64(self.projection.height_scale)
            .f64(self.projection.depth_per_ground)
            .f64(self.projection.depth_per_height)
            .u32(self.output_size[0])
            .u32(self.output_size[1])
            .f32(self.pixels_per_metre)
            .u32(self.lod as u32)
            .f64(self.halo_m);
    }
}

/// A stamp image the scene refers to.
#[derive(Clone, Debug, PartialEq)]
pub struct StampBinding {
    /// The stamp's document-relative asset path.
    pub asset: String,
}

/// One accepted candidate, and everything it grew.
///
/// ## Why a plant needs a name of its own
///
/// A flower is a curved stem plus a head instance plus a whorl of petals. A
/// rosette is several leaves sharing a crown. Each of those primitives has its
/// own [`MarkId`], its own painter order and its own bounding box — and a trace
/// slice that selected them independently would keep the stem and drop the head,
/// or classify the stem as halo geometry and the head as camera geometry. The
/// result is not a missing flower; it is half a flower, which is worse.
///
/// So every primitive names the placement it came from, and the slice selector
/// classifies *placements*. Four consequences follow, and all four are things
/// the framework previously had no way to express:
///
/// - a group is kept or dropped whole;
/// - a report can count plants rather than only primitives;
/// - an interaction footprint can name the object that caused it;
/// - the fingerprint records the grouping and not merely its members.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacementAnchor {
    /// The candidate that was accepted here.
    pub candidate: CandidateId,
    /// Where it meets the ground, at the final rendered surface height.
    pub root: ScenePoint,
}

/// A deterministic obstacle that content can grow around.
///
/// Emitted by the recipe that placed the obstacle, not derived afterwards from
/// its geometry. That direction matters: a stone's *physical* footprint is a
/// decision its recipe made — a conservative ellipse it guarantees to stay
/// inside — and recovering one from a mesh after the fact would be an estimate
/// that is sometimes smaller than the object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionPrimitive {
    /// The mark that caused it.
    pub source: MarkId,
    pub anchor: AnchorIndex,
    pub centre: WorldPoint,
    pub shape: InteractionShape,
    /// Inside this, a root is refused outright.
    pub hard_clearance_m: f32,
    /// How far past the hard boundary the smooth response reaches.
    ///
    /// Beyond `hard_clearance_m + response_reach_m` the field is *exactly*
    /// nothing — not a small number — so that removing the obstacle leaves
    /// distant content bit-identical.
    pub response_reach_m: f32,
    /// Which passes are affected.
    pub channels: InteractionChannels,
}

/// The footprint an obstacle occupies on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum InteractionShape {
    /// An oriented ellipse.
    ///
    /// The only shape version one needs, and a deliberate choice rather than a
    /// placeholder: a conservative ground-plane footprint for a stone is an
    /// ellipse, and the query is evaluated at every prospective grass root —
    /// millions per plate — so it has to be a handful of multiplies.
    Ellipse {
        semi_u_m: f32,
        semi_v_m: f32,
        yaw_rad: f32,
    },
}

/// Which kinds of content an obstacle pushes around.
///
/// A bitfield rather than a set, because it is read inside the placement loop.
/// Thatch, for instance, is excluded from a stone's footprint but barely bends
/// around it — a mat lies over whatever is there — so the channels differ per
/// pass rather than being one boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InteractionChannels(pub u8);

impl InteractionChannels {
    pub const THATCH: Self = Self(1 << 0);
    pub const FINE: Self = Self(1 << 1);
    pub const TUFT: Self = Self(1 << 2);
    pub const BROADLEAF: Self = Self(1 << 3);
    /// Everything the tuned generator plants.
    pub const ALL_TUNED: Self = Self(0b1111);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl Digestible for InteractionPrimitive {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .u64(self.source.0)
            .u32(self.anchor.0)
            .f64(self.centre.u_m)
            .f64(self.centre.v_m);
        match self.shape {
            InteractionShape::Ellipse {
                semi_u_m,
                semi_v_m,
                yaw_rad,
            } => {
                digest.tag(0).f32(semi_u_m).f32(semi_v_m).f32(yaw_rad);
            }
        }
        digest
            .f32(self.hard_clearance_m)
            .f32(self.response_reach_m)
            .u32(self.channels.0 as u32);
    }
}

impl Digestible for PlacementAnchor {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .u64(self.candidate.population.bits())
            .i64(self.candidate.cell.x)
            .i64(self.candidate.cell.y)
            .u32(self.candidate.rank as u32)
            .f64(self.root.u_m)
            .f64(self.root.v_m)
            .f64(self.root.z_m);
    }
}

/// Everything to render, for one request.
#[derive(Clone, Debug)]
pub struct TerrainScene {
    pub request: SceneRequest,
    pub ground: GroundSurface,
    /// Every mark, in painter order.
    pub marks: Vec<SceneMark>,
    pub instances: Vec<PrototypeInstance>,
    pub materials: Vec<SceneMaterialBinding>,
    pub stamps: Vec<StampBinding>,
    /// One entry per accepted candidate that grew something.
    ///
    /// Index zero is always the ungrouped placeholder, so
    /// [`AnchorIndex::UNGROUPED`] is a valid index into a scene that never
    /// bound an anchor at all.
    pub anchors: Vec<PlacementAnchor>,
    /// The geometry instances refer to, canonical and deduplicated.
    pub prototypes: Vec<PrototypeBinding>,
    /// Obstacles other content grows around.
    pub interactions: Vec<InteractionPrimitive>,
    /// The prepared terrain's document digest, so a scene can say what it came
    /// from without holding the terrain alive.
    pub document_digest: Fingerprint,
    /// The version of the generator that built it.
    pub generator_version: u32,
}

/// The domain every scene fingerprint is taken in.
pub const SCENE_DIGEST_DOMAIN: &str = "terrain-scene";

impl TerrainScene {
    /// An empty scene for a request.
    pub fn empty(
        request: SceneRequest,
        document_digest: Fingerprint,
        generator_version: u32,
    ) -> Self {
        let generated = request.generated_bounds();
        // A metre lattice by default: enough to hold a scene together and
        // coarse enough that an empty scene costs nothing.
        Self {
            ground: GroundSurface::flat(
                generated.min,
                1.0,
                generated.width_m().ceil().max(1.0) as u32,
                generated.height_m().ceil().max(1.0) as u32,
            ),
            request,
            marks: Vec::new(),
            instances: Vec::new(),
            materials: Vec::new(),
            stamps: Vec::new(),
            // Seeded with the ungrouped placeholder, so index zero is always a
            // real entry and a mark that named no group still indexes something
            // rather than dangling. Its candidate address is deliberately the
            // zero address: nothing the domain lattice can produce, so a real
            // placement can never collide with it.
            anchors: vec![PlacementAnchor {
                candidate: CandidateId::new(
                    terrain_core::seed::PopulationHash::from_bits(0),
                    terrain_core::coords::CellCoord::new(0, 0),
                    0,
                ),
                root: ScenePoint::default(),
            }],
            prototypes: Vec::new(),
            interactions: Vec::new(),
            document_digest,
            generator_version,
        }
    }

    /// Every mark belonging to one placement group.
    ///
    /// A linear scan. The alternative is a per-anchor index built at
    /// `build` time, which is the right answer once slice selection is doing
    /// this per anchor per slice; for now the caller that needs it walks the
    /// marks once and buckets them, which is the same work done once instead of
    /// once per group.
    pub fn marks_for_anchor(&self, anchor: AnchorIndex) -> impl Iterator<Item = &SceneMark> {
        self.marks.iter().filter(move |m| m.anchor() == anchor)
    }

    /// Every instance belonging to one placement group.
    pub fn instances_for_anchor(
        &self,
        anchor: AnchorIndex,
    ) -> impl Iterator<Item = &PrototypeInstance> {
        self.instances.iter().filter(move |i| i.anchor == anchor)
    }

    /// Every interaction belonging to one placement group.
    pub fn interactions_for_anchor(
        &self,
        anchor: AnchorIndex,
    ) -> impl Iterator<Item = &InteractionPrimitive> {
        self.interactions.iter().filter(move |i| i.anchor == anchor)
    }

    /// A conservative bound around everything one placement grew.
    ///
    /// What slice selection classifies. Returns `None` for a group that grew
    /// nothing, which the ungrouped placeholder normally is.
    pub fn group_bounds(&self, anchor: AnchorIndex) -> Option<Aabb3> {
        let mut bounds: Option<Aabb3> = None;
        for mark in self.marks_for_anchor(anchor) {
            bounds = Some(match bounds {
                None => mark.bounds(),
                Some(current) => current.union(mark.bounds()),
            });
        }
        for instance in self.instances_for_anchor(anchor) {
            bounds = Some(match bounds {
                None => instance.bounds,
                Some(current) => current.union(instance.bounds),
            });
        }
        bounds
    }

    /// Bounds for every placement group that grew something, by anchor index.
    ///
    /// One pass over the marks and instances rather than one pass per group,
    /// which is what a slice selector actually wants: it classifies all of them
    /// and the quadratic version is minutes rather than milliseconds on a plate
    /// with a hundred thousand placements.
    pub fn all_group_bounds(&self) -> Vec<(AnchorIndex, Aabb3)> {
        let mut bounds: Vec<Option<Aabb3>> = vec![None; self.anchors.len()];
        let mut widen = |anchor: AnchorIndex, box3: Aabb3| {
            if let Some(slot) = bounds.get_mut(anchor.index()) {
                *slot = Some(match *slot {
                    None => box3,
                    Some(current) => current.union(box3),
                });
            }
        };
        for mark in &self.marks {
            widen(mark.anchor(), mark.bounds());
        }
        for instance in &self.instances {
            widen(instance.anchor, instance.bounds);
        }
        bounds
            .into_iter()
            .enumerate()
            .filter_map(|(index, box3)| box3.map(|b| (AnchorIndex(index as u32), b)))
            .collect()
    }

    /// How many placements grew something.
    pub fn placement_count(&self) -> usize {
        // Minus the ungrouped placeholder, which is not a placement.
        self.anchors.len().saturating_sub(1)
    }

    pub fn mark_count(&self) -> usize {
        self.marks.len()
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty() && self.instances.is_empty()
    }

    /// Put every mark into painter order.
    ///
    /// Called once after building. A stable sort, so that two marks with the
    /// same order key — which cannot happen, since the key includes the stable
    /// id, but which the sort must not rely on — keep their relative sequence.
    pub fn sort_marks(&mut self) {
        self.marks.sort_by_key(|mark| mark.order());
    }

    /// Whether the marks are in order.
    pub fn is_sorted(&self) -> bool {
        self.marks.windows(2).all(|w| w[0].order() <= w[1].order())
    }

    /// A bound on everything in the scene.
    pub fn bounds(&self) -> Option<Aabb3> {
        let mut bounds: Option<Aabb3> = None;
        for mark in &self.marks {
            bounds = Some(match bounds {
                None => mark.bounds(),
                Some(current) => current.union(mark.bounds()),
            });
        }
        for instance in &self.instances {
            bounds = Some(match bounds {
                None => instance.bounds,
                Some(current) => current.union(instance.bounds),
            });
        }
        bounds
    }

    /// How high anything in the scene stands, in metres above the datum.
    ///
    /// What a shadow pass sizes its light-space volume from, and it has to be a
    /// genuine bound rather than an estimate: a caster clipped out of the depth
    /// map is a shadow that simply is not there.
    pub fn canopy_ceiling_m(&self) -> f64 {
        self.bounds().map(|b| b.ceiling_m()).unwrap_or(0.0)
    }

    /// The scene's fingerprint.
    ///
    /// Everything that decides what a renderer would draw: the request, the
    /// ground, every mark in order, every instance, and the binding tables. Not
    /// capacities, not pointers, not the order things were appended in before
    /// the sort.
    pub fn fingerprint(&self) -> Fingerprint {
        let mut digest = Digest::for_domain(SCENE_DIGEST_DOMAIN);
        digest
            .u32(self.generator_version)
            .digest(self.document_digest);
        self.request.absorb(&mut digest);
        self.ground.absorb(&mut digest);
        digest.slice(&self.materials, |d, binding| binding.absorb(d));
        digest.slice(&self.stamps, |d, stamp| {
            d.str(&stamp.asset);
        });
        // Grouping is semantic, not bookkeeping: two scenes with identical
        // primitives that grouped them differently would render the same still
        // and slice differently, and a fingerprint that could not tell them
        // apart would call that no change at all.
        digest.slice(&self.anchors, |d, anchor| anchor.absorb(d));
        digest.slice(&self.prototypes, |d, prototype| prototype.absorb(d));
        digest.slice(&self.marks, |d, mark| mark.absorb(d));
        digest.slice(&self.instances, |d, instance| instance.absorb(d));
        digest.slice(&self.interactions, |d, interaction| interaction.absorb(d));
        digest.finish()
    }

    /// Every mark whose root lies inside the requested bounds.
    ///
    /// The halo's marks are generated and then *not* counted as belonging to
    /// this scene's ground — they exist to shade and occlude inward. A caller
    /// tallying density has to use this rather than the mark count, or every
    /// measurement is inflated by the halo's area.
    pub fn marks_inside(&self) -> impl Iterator<Item = &SceneMark> {
        let bounds = self.request.bounds;
        self.marks.iter().filter(move |mark| {
            let root = mark.root();
            bounds.contains(WorldPoint::new(root.u_m, root.v_m))
        })
    }

    /// Marks per square metre of the requested ground.
    ///
    /// The quality counter-metric every speed claim carries: an optimisation
    /// that got faster by generating fewer marks is a quality-tier change, and
    /// this is the number that says so.
    pub fn mark_density(&self) -> f64 {
        let area = self.request.bounds.area_m2();
        if area <= 0.0 {
            return 0.0;
        }
        self.marks_inside().count() as f64 / area
    }
}

/// A scene under construction.
///
/// Separate from [`TerrainScene`] so that a finished scene cannot be appended
/// to. That matters more than it looks: a scene is sorted into painter order
/// once, and a mark pushed afterwards would sit in the wrong place with nothing
/// to report it. The builder consumes itself to produce the scene, so there is
/// no path from "finished" back to "being built".
pub struct SceneBuilder {
    scene: TerrainScene,
}

impl SceneBuilder {
    pub fn new(
        request: SceneRequest,
        document_digest: Fingerprint,
        generator_version: u32,
    ) -> Self {
        Self {
            scene: TerrainScene::empty(request, document_digest, generator_version),
        }
    }

    pub fn request(&self) -> &SceneRequest {
        &self.scene.request
    }

    pub fn set_ground(&mut self, ground: GroundSurface) -> &mut Self {
        self.scene.ground = ground;
        self
    }

    /// Register an appearance and return the index marks refer to it by.
    ///
    /// Idempotent: registering the same appearance twice returns the same index,
    /// so a recipe can call it per mark without building a table of duplicates.
    pub fn bind_material(
        &mut self,
        binding: SceneMaterialBinding,
    ) -> crate::mark::SceneMaterialIndex {
        if let Some(index) = self
            .scene
            .materials
            .iter()
            .position(|existing| existing.appearance == binding.appearance)
        {
            return crate::mark::SceneMaterialIndex(index as u16);
        }
        self.scene.materials.push(binding);
        crate::mark::SceneMaterialIndex((self.scene.materials.len() - 1) as u16)
    }

    /// Register a stamp asset and return its index.
    pub fn bind_stamp(&mut self, asset: impl Into<String>) -> u16 {
        let asset = asset.into();
        if let Some(index) = self.scene.stamps.iter().position(|s| s.asset == asset) {
            return index as u16;
        }
        self.scene.stamps.push(StampBinding { asset });
        (self.scene.stamps.len() - 1) as u16
    }

    /// Register a placement group and return the index its primitives name.
    ///
    /// Not idempotent, and deliberately not: two accepted candidates at the
    /// same position are two plants, and folding them would make one of them
    /// vanish from every per-group count while both still rendered.
    pub fn bind_anchor(&mut self, anchor: PlacementAnchor) -> AnchorIndex {
        self.scene.anchors.push(anchor);
        AnchorIndex((self.scene.anchors.len() - 1) as u32)
    }

    /// Register a prototype and return the index instances refer to it by.
    ///
    /// Canonical and idempotent: equal bindings return the same index, so a
    /// recipe can name its prototype per instance without building a table with
    /// a thousand copies of one rock in it.
    ///
    /// Equality is on the *whole binding* rather than on the key. Two bindings
    /// under one key with different parameters are two different shapes, and
    /// resolving that by "last one wins" would make the geometry depend on
    /// traversal order — which is the one thing this framework will not do. The
    /// compiler reports the collision instead; see `validate_scene`.
    pub fn bind_prototype(&mut self, binding: PrototypeBinding) -> PrototypeIndex {
        if let Some(index) = self
            .scene
            .prototypes
            .iter()
            .position(|existing| *existing == binding)
        {
            return PrototypeIndex(index as u16);
        }
        self.scene.prototypes.push(binding);
        PrototypeIndex((self.scene.prototypes.len() - 1) as u16)
    }

    pub fn push_interaction(&mut self, interaction: InteractionPrimitive) -> &mut Self {
        self.scene.interactions.push(interaction);
        self
    }

    pub fn push_mark(&mut self, mark: SceneMark) -> &mut Self {
        self.scene.marks.push(mark);
        self
    }

    pub fn push_instance(&mut self, instance: PrototypeInstance) -> &mut Self {
        self.scene.instances.push(instance);
        self
    }

    pub fn mark_count(&self) -> usize {
        self.scene.marks.len()
    }

    /// Sort and finish.
    pub fn build(mut self) -> TerrainScene {
        self.scene.sort_marks();
        self.scene.instances.sort_by_key(|instance| instance.order);
        // By the causing mark rather than by insertion, so the query field
        // built from these has an order that does not depend on which recipe
        // emitted first. `MarkId` is a total order derived from the candidate
        // address, so this is stable across windows.
        self.scene
            .interactions
            .sort_by_key(|interaction| interaction.source);
        self.scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mark::*;
    use crate::projection::ScreenPoint;
    use terrain_core::ids::AppearanceKey;

    fn appearance(key: &str) -> AppearanceKey {
        AppearanceKey::new(key).expect("valid")
    }

    fn request() -> SceneRequest {
        SceneRequest::square(WorldPoint::ORIGIN, 4.0, 96.0)
    }

    fn ribbon(id: u64, depth: f64, root: ScenePoint) -> SceneMark {
        SceneMark::Ribbon(RibbonMark {
            stable_id: MarkId(id),
            anchor: AnchorIndex::UNGROUPED,
            order: PainterOrder::new(Stratum::Canopy, depth, 0, MarkId(id)),
            material: SceneMaterialIndex(0),
            root,
            geometry: RibbonGeometry::default(),
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(root, 0.25),
        })
    }

    fn builder() -> SceneBuilder {
        SceneBuilder::new(request(), Fingerprint::from_u128(0x1234), 1)
    }

    #[test]
    fn a_built_scene_is_in_painter_order() {
        let mut builder = builder();
        builder.push_mark(ribbon(1, 5.0, ScenePoint::default()));
        builder.push_mark(ribbon(2, 1.0, ScenePoint::default()));
        builder.push_mark(ribbon(3, 3.0, ScenePoint::default()));
        let scene = builder.build();
        assert!(scene.is_sorted());
        assert_eq!(
            scene
                .marks
                .iter()
                .map(|m| m.stable_id().0)
                .collect::<Vec<_>>(),
            [2, 3, 1]
        );
    }

    #[test]
    fn the_order_marks_were_pushed_in_does_not_reach_the_fingerprint() {
        // The property that lets a scene be built by several threads and
        // still fingerprint identically: the sort is what decides the order,
        // and the sort is total.
        let mut forward = builder();
        forward.push_mark(ribbon(1, 5.0, ScenePoint::default()));
        forward.push_mark(ribbon(2, 1.0, ScenePoint::default()));
        forward.push_mark(ribbon(3, 3.0, ScenePoint::default()));

        let mut backward = builder();
        backward.push_mark(ribbon(3, 3.0, ScenePoint::default()));
        backward.push_mark(ribbon(2, 1.0, ScenePoint::default()));
        backward.push_mark(ribbon(1, 5.0, ScenePoint::default()));

        assert_eq!(
            forward.build().fingerprint(),
            backward.build().fingerprint()
        );
    }

    #[test]
    fn a_scene_fingerprints_the_same_way_twice() {
        let scene = builder().build();
        assert_eq!(scene.fingerprint(), scene.fingerprint());
    }

    #[test]
    fn every_part_of_a_scene_reaches_its_fingerprint() {
        let base = {
            let mut builder = builder();
            builder.bind_material(SceneMaterialBinding {
                appearance: appearance("plant.grass_blade"),
                terrain_material: None,
            });
            builder.push_mark(ribbon(1, 1.0, ScenePoint::default()));
            builder.build()
        };
        let reference = base.fingerprint();

        let mut moved = base.clone();
        moved.request.halo_m = 0.5;
        assert_ne!(reference, moved.fingerprint(), "request");

        let mut reframed = base.clone();
        reframed.request.viewport = ScreenRect::around(ScreenPoint::new(1.0, 2.0), 3.0, 4.0);
        assert_ne!(reference, reframed.fingerprint(), "viewport");

        let mut reground = base.clone();
        reground.ground.elevation[0] += 1.0;
        assert_ne!(reference, reground.fingerprint(), "ground");

        let mut remarked = base.clone();
        remarked.marks.push(ribbon(2, 2.0, ScenePoint::default()));
        assert_ne!(reference, remarked.fingerprint(), "marks");

        let mut rebound = base.clone();
        rebound.materials[0].appearance = appearance("plant.wildflower_head");
        assert_ne!(reference, rebound.fingerprint(), "materials");

        let mut restamped = base.clone();
        restamped.stamps.push(StampBinding {
            asset: "stamps/flowers/daisy.png".into(),
        });
        assert_ne!(reference, restamped.fingerprint(), "stamps");

        let mut reversioned = base.clone();
        reversioned.generator_version = 2;
        assert_ne!(reference, reversioned.fingerprint(), "generator version");

        let mut redocumented = base.clone();
        redocumented.document_digest = Fingerprint::from_u128(0x5678);
        assert_ne!(reference, redocumented.fingerprint(), "document digest");
    }

    #[test]
    fn binding_the_same_appearance_twice_returns_one_index() {
        // So a recipe can call it per mark without building a table of
        // duplicates, which would then be a table Cycles has to build materials
        // for.
        let mut builder = builder();
        let first = builder.bind_material(SceneMaterialBinding {
            appearance: appearance("plant.grass_blade"),
            terrain_material: None,
        });
        let second = builder.bind_material(SceneMaterialBinding {
            appearance: appearance("plant.grass_blade"),
            terrain_material: None,
        });
        let other = builder.bind_material(SceneMaterialBinding {
            appearance: appearance("surface.dirt_compacted"),
            terrain_material: None,
        });
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert_eq!(builder.build().materials.len(), 2);
    }

    #[test]
    fn binding_the_same_stamp_twice_returns_one_index() {
        let mut builder = builder();
        assert_eq!(builder.bind_stamp("stamps/flowers/daisy.png"), 0);
        assert_eq!(builder.bind_stamp("stamps/flowers/daisy.png"), 0);
        assert_eq!(builder.bind_stamp("stamps/leaves/maple.png"), 1);
    }

    #[test]
    fn only_marks_rooted_inside_the_request_count_toward_density() {
        // The halo's marks exist to shade and occlude inward. Counting them
        // would inflate every density measurement by the halo's area.
        let mut builder = SceneBuilder::new(request().with_halo(1.0), Fingerprint::from_u128(0), 1);
        // Inside the 4×4 square centred on the origin.
        builder.push_mark(ribbon(1, 0.0, ScenePoint::new(0.0, 0.0, 0.0)));
        builder.push_mark(ribbon(2, 0.0, ScenePoint::new(1.5, 1.5, 0.0)));
        // In the halo.
        builder.push_mark(ribbon(3, 0.0, ScenePoint::new(2.5, 0.0, 0.0)));
        let scene = builder.build();

        assert_eq!(scene.mark_count(), 3);
        assert_eq!(scene.marks_inside().count(), 2);
        assert!((scene.mark_density() - 2.0 / 16.0).abs() < 1.0e-9);
    }

    #[test]
    fn the_canopy_ceiling_bounds_every_mark() {
        // A caster clipped out of a shadow map is a shadow that simply is not
        // there, so this has to be a genuine bound.
        let mut builder = builder();
        builder.push_mark(ribbon(1, 0.0, ScenePoint::new(0.0, 0.0, 0.0)));
        builder.push_mark(ribbon(2, 0.0, ScenePoint::new(0.0, 0.0, 1.0)));
        let scene = builder.build();
        let ceiling = scene.canopy_ceiling_m();
        for mark in &scene.marks {
            assert!(
                mark.bounds().ceiling_m() <= ceiling + 1.0e-9,
                "a mark reaches past the ceiling"
            );
        }
        assert!(ceiling >= 1.0);
    }

    #[test]
    fn an_empty_scene_reports_nothing_rather_than_guessing() {
        let scene = builder().build();
        assert!(scene.is_empty());
        assert_eq!(scene.bounds(), None);
        assert_eq!(scene.canopy_ceiling_m(), 0.0);
        assert_eq!(scene.mark_density(), 0.0);
        // And still fingerprints, because an empty scene is a real answer.
        assert_eq!(scene.fingerprint(), builder().build().fingerprint());
    }

    #[test]
    fn a_halo_widens_the_ground_that_gets_generated() {
        let request = request().with_halo(0.5);
        assert_eq!(request.bounds.width_m(), 4.0);
        assert_eq!(request.generated_bounds().width_m(), 5.0);
    }

    #[test]
    fn the_viewport_and_the_pixel_scale_say_the_same_thing() {
        // Two ways of writing one number, which is a place for them to drift.
        // Every request the resolver builds is checked against this.
        let request = request();
        assert!(
            (request.viewport_pixels_per_metre() - request.pixels_per_metre).abs() < 1.0e-3,
            "{} against {}",
            request.viewport_pixels_per_metre(),
            request.pixels_per_metre
        );
    }

    #[test]
    fn a_level_of_detail_halves_the_resolution_per_step() {
        let request = request();
        assert_eq!(request.effective_pixels_per_metre(), 96.0);
        assert_eq!(request.with_lod(1).effective_pixels_per_metre(), 48.0);
        assert_eq!(request.with_lod(2).effective_pixels_per_metre(), 24.0);
    }
}

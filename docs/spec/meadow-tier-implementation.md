# Groundwork Meadow Tier
## Implementation specification for the next production phase

**Status:** Proposed implementation contract  
**Audience:** the implementing agent and reviewers  
**Repository baseline:** `BackseatWarlord-main-2026-08-05T12-12-51-499Z-941c7582.xml`, generated 2026-08-05  
**Product brief:** `groundwork-meadow-tier-spec.md`  
**Primary output:** one continuous, deterministic, 3×3 isometric meadow scene rendered by Blender Cycles  
**Companion benchmark schema:** `ground-benchmark-report.schema.json`  
**Scope boundary:** complete the meadow content tier before freezing the neural-renderer conditioning contract

---

## 0. How to read this document

This specification has three kinds of statement, kept deliberately separate.

1. **Verified current state** describes code that exists in the supplied repository. It is not a proposal.
2. **Required design** is the implementation contract for this phase. Normative words such as **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are intentional.
3. **Research basis** explains why a difficult algorithm has the proposed form. It is evidence, not an instruction to transplant a paper literally.

The central constraint is unchanged from the existing project:

> Terrain is one continuous function of world position. A render window, world tile, trace slice, cache page, or output crop is never a generation boundary.

Everything below is designed to preserve that property while adding the meadow content that is currently computed but not rendered.

---

# Part I — Decisions

## 1. Executive implementation decisions

The next phase is not “write some flower and rock geometry.” It is a controlled merge of two existing paths:

- the **tuned grass path**, which already produces the correct meadow canopy and is the visual quality bar;
- the **semantic scene compiler**, which already resolves documents, fields, shared candidate domains, ownership, and generic marks, but whose output is discarded by the production renderer.

The following decisions settle the architecture.

### D1. Preserve the tuned grass generator

`terrain_generators::{field, placement, scene, stroke, style}` remains the only source of production grass tufts, fine grass, thatch, and broadleaf geometry.

The generic grass families in `terrain_generators::families` **MUST NOT** be wired into the production Cycles render. They are materially lower fidelity and would double the meadow density if rendered beside the tuned generator.

### D2. Extend the active Cycles path now; do not migrate to `write_package` in this phase

The active `CyclesScene`/`scene.json` path will be extended to carry secondary ribbons, curves, prototypes, instances, and their materials beside the existing tuned blade buffers.

The generic `write_package` format remains the long-term destination, but moving production onto it is a separate, fingerprint-preserving migration after it can represent the exact tuned blade vocabulary. Combining that migration with new content would make any visual regression impossible to attribute.

### D3. Compile one meadow domain once

The semantic compiler runs once over the complete generated bounds for the nine-tile plate, including the derived halo. It produces:

- one `TerrainFieldStack`;
- one shared `GroundEvaluator`;
- one secondary `TerrainScene`;
- one deterministic `InteractionField` derived from accepted obstacles;
- one set of tuned-population controls;
- one compilation report.

Trace slices only **select and lower** already-compiled secondary content. They never regenerate it.

The tuned grass path may continue to generate its strokes per trace slice because its placement is world-addressed and already proven slice-stable. It must, however, read the same shared ground evaluator, tuned-population controls, and interaction field.

### D4. Make render ownership explicit

Every population recipe declares one of three render classes:

```text
Tuned(pass)       document controls an existing tuned pass; generic marks do not render
Secondary         recipe emits geometry or instances that the hybrid Cycles path renders
Deferred          recipe is compiled only for diagnostics/experiments and does not render yet
```

Initial mapping:

| Recipe | Render class |
| --- | --- |
| `population.grass_tuft` | `Tuned(Tuft)` |
| `population.grass_fine` | `Tuned(Fine)` |
| `population.ground_thatch` | `Tuned(Thatch)` |
| `population.meadow_broadleaf` | `Tuned(Broadleaf)` — new semantic recipe |
| `population.meadow_flowers` | `Secondary` |
| `population.field_stones` | `Secondary` |
| `population.meadow_undergrowth` | `Secondary` — new recipe |
| `population.dirt_clods` | `Deferred` |

`dirt_clods` is deferred because the ground evaluator already generates clod-scale relief. Rendering a second clod population now would double-count the same physical signal.

### D5. Move final-ground evaluation into scene compilation

Secondary roots currently use `TerrainFieldStack::surface_height`, while the active ground mesh adds profile-derived relief through `GroundEvaluator`. That means a stone or flower can be vertically registered to a different surface from the one Cycles renders.

The compiler will construct the one `GroundEvaluator` before emitting secondary content and will give every recipe the final surface height:

```text
final_surface_z(p)
    = authored_elevation(p)
    + authored_microrelief(p)
    + profile_geometry_displacement(p)
```

The compiler returns that evaluator. The CLI and tuned overlay reuse it; they do not reconstruct a second evaluator.

### D6. Add deterministic prototype instances

Rust decides every stochastic and semantic fact:

- candidate acceptance;
- owner;
- prototype identity;
- scale;
- yaw and tilt;
- burial depth;
- tint and material variation;
- interaction footprint.

Blender builds each declared prototype once and creates linked instances from explicit transforms. Blender performs no terrain scattering and no random selection.

### D7. Generalise exclusion to variable physical footprints

The current exclusion pass is a fixed-radius, order-independent priority thinning, closely related to a Matérn type-II hard-core process. It will be extended so each domain candidate carries an addressed footprint radius.

A pair conflicts when their physical footprint disks overlap after clearance:

\[
\lVert x_i-x_j\rVert < r_i+r_j+c.
\]

Candidate `i` survives if no conflicting candidate has a strictly greater deterministic priority key. This preserves:

- traversal-order independence;
- exact overlap-window agreement;
- monotone density thinning;
- stable candidate identities.

Sequential dart throwing or ordinary Bridson sampling is not acceptable here because a candidate's result would depend on what had already been generated in that particular window.

### D8. Derive a bounded stone interaction field

Accepted stones produce deterministic elliptical interaction primitives. The tuned grass generator queries this field at a prospective root.

Grass inside a stone is rejected. Grass close to a stone is shortened and bent away from it. The response is bounded, smooth, and local.

This is part of tuned placement, not a post-process. A post-process can cut a hole but cannot make the surviving plants grow around the obstacle.

### D9. Fix semantic bareness and expose per-pass controls

Authored vegetation abundance must affect both the number of plants and how much earth the tuned generator exposes.

For vegetation support `s∈[0,1]` and authored global abundance `a`:

\[
q = s\,\operatorname{clamp}(a,0,1)
\]
\[
b_{semantic}=1-q^\gamma,\qquad \gamma=1\text{ for version 1.}
\]

Then:

```text
ground.density *= s * max(a, 0)
ground.bare     = max(style_bare, b_semantic)
```

Values above one may increase density but may not create “negative bare ground.”

Tuft, fine, thatch, and broadleaf passes also receive independent spatial controls compiled from their declared population affinity, abundance channel, and target-density calibration.

### D10. Complete three genuinely missing content systems

The meadow tier adds:

- flowers with curved stems, explicit petal whorls, and grouped placement identity;
- buried stone/fragment prototypes with grass interaction;
- low broad-leaved undergrowth below the canopy.

Thatch and broadleaf already exist visually; this phase makes them author-controllable rather than creating lower-quality replacements.

### D11. Resolve the two-soil ambiguity by measurement

`meadow_floor` and `compacted_loam` currently describe what appears to be the same physical soil under different visibility and lighting conditions. The phase will test a shared physical loam profile, with compaction/moisture/disturbance and canopy occlusion carrying the state difference.

This is a measured visual experiment, not a silent refactor. The old and new results remain side by side until the acceptance metrics and reference comparison justify the change.

### D12. Do not start the neural corpus yet

The corpus contract is frozen only after all meadow content types render and the derived conditioning planes are known. Generating a corpus before then would encode a meadow vocabulary without flowers, stones, undergrowth, or interaction occupancy and would have to be discarded.

### D13. Put the ground on one mathematical relief contract and benchmark it as a system

The current ground has one semantic evaluator but two different procedural relief implementations: Rust constructs mesh-scale bands from addressed value noise and a monotonic aggregate transform, while the Blender material reconstructs sub-mesh bands with Blender Noise and a folded ridge transform. State propagation is also incomplete: `compaction` and `wet_film` are exported, but the active ground graph does not consume them, and unresolved-band roughness is a profile-level constant rather than a state-dependent field.

This phase therefore adds a **ground coherence gate** before final meadow calibration:

1. every profile band is assigned to geometry, bump, or microfacet exactly once by a recorded `GroundReliefPlan`;
2. geometry and bump evaluate the same Rust-owned band basis, phase, aggregate transform, clustering, and state response;
3. Blender samples Rust-authored bump and micro-roughness fields rather than inventing new relief;
4. moisture controls the substrate response, while the derived `wet_film` field controls the Principled coat layer;
5. the entire soil system is benchmarked morphologically, spectrally, optically, compositionally, deterministically, and for performance.

A soil change without the ground benchmark report is incomplete, even if the beauty render looks better.

---

## 2. Verified current state

The repository already contains most of the semantic machinery required for this phase.

### 2.1 Working pipeline

```text
TerrainDocument + ground profiles
    ↓ parse / migrate / validate / prepare
PreparedTerrain
    ↓ edge-anchored matrix sampling
TerrainFieldStack
    ↓ slope / curvature / flow / exposure / boundary derivation
Derived fields
    ↓ shared domains / acceptance / ownership
TerrainScene + compilation report
```

Separately, the active render path does this:

```text
TerrainFieldStack
    ↓ GroundEvaluator + SemanticOverlay
WorldField
    ↓ tuned placement for one trace page
GrassScene
    ↓ blade and ground lowering
CyclesScene
    ↓ Blender Cycles
image
```

The missing connection is between `TerrainScene` and `CyclesScene`.

### 2.2 What is already load-bearing

The implementation already has:

- half-open world rectangles, mathematical floor division, and `f64` world coordinates;
- addressed random values keyed by world cell, candidate rank, population/domain, recipe version, and named stream;
- separate seed and semantic-digest algorithms;
- immutable prepared terrain and total sampling;
- an edge-addressed `TerrainFieldStack` with typed descriptors and units;
- material-score composition and normalised, pruned weight sets;
- derived slope, aspect, normal, curvature, exposure, flow, blend, and boundary tangent;
- ragged transition realisation that cannot create an excluded material on pure ground;
- shared candidate domains with acceptance before ownership;
- one `GroundEvaluator` abstraction with a geometry/bump/roughness relief split;
- tuned grass placement with tufts, fine grass, thatch, and broadleaf clusters;
- Cycles-only rendering;
- a generic scene vocabulary of ribbon, curve, analytic, stamp, and prototype-instance types;
- generic package manifest types, although the production Blender path does not consume them.

These are foundations, not work to redo.

### 2.3 What the production image currently contains

| Content | Tuned generator | Generic family | Production Cycles image |
| --- | ---: | ---: | ---: |
| Grass tufts | yes | yes | yes, tuned only |
| Fine grass | yes | yes | yes, tuned only |
| Thatch | yes | yes | yes, tuned only |
| Broadleaf clusters | yes | no | yes, tuned only |
| Flowers | no | yes | no |
| Stones | no | yes | no |
| Dirt clods | no | yes | no as discrete marks; relief exists |
| Meadow undergrowth | no | no | no |

### 2.4 Exact production blocker

The CLI calls `compile_scene`, receives `compiled.scene`, and uses it for only:

- a scene fingerprint in the report;
- mark-density reporting.

It then independently constructs a `GroundEvaluator`, a `SemanticOverlay`, a `WorldField`, and asks `plate::trace` to build a tuned `GrassScene` for every trace slice. `plate::trace` passes only that `GrassScene` into `CyclesScene::build`.

Therefore every accepted flower, stone, and generic mark is computed, counted, fingerprinted, and discarded.

### 2.5 Additional integration hazards found in the code

These are not all stated in the overview, but follow directly from the supplied implementation.

#### H1. A naive bridge doubles the meadow

The generic scene contains generic grass, fine grass, and thatch as well as flowers and stones. Rendering every scene mark would draw a second, lower-quality canopy over the tuned one.

#### H2. Secondary roots can be below or above the rendered soil

Generic recipes receive `fields.surface_height(candidate.position)`. The rendered ground additionally uses profile-derived geometry displacement. The two surfaces can differ by centimetres, which is enough to float a pebble or bury a stem.

#### H3. Per-mark trace visibility can split one plant

A flower stem is rooted at the ground, while its head is an analytic mark centred at the tip. If trace-slice selection treats marks independently, a stem can be classified as halo geometry while its head is classified as camera geometry, or one half can be omitted. Selection must operate on a shared placement anchor before marks are lowered.

#### H4. Prototype bindings are not a complete scene table

`PrototypeBinding` and `PrototypeInstance` exist, but `TerrainScene` does not expose a complete prototype binding table or a `bind_prototype` path comparable to materials and stamps. The active package writer also does not lower prototypes or instances.

#### H5. Exclusion documentation and implementation differ

The exclusion documentation speaks about per-candidate radii, but `domain.rs` uses one `max_radius_m` for every candidate. Its exact-priority tie break compares only `rank`; a total ordering should include the complete candidate identity.

#### H6. Authored density does not currently expose ground

The overlay multiplies tuned density by vegetation support and abundance, but sets `bare` only from vegetation support. Lowering vegetation density over fertile soil produces holes in an otherwise lush canopy instead of a genuinely sparse stand with the tuned bare-ground responses engaged.

#### H7. Population-specific controls are collapsed into one global factor

The tuned passes all read one `Ground` density. A document can declare `tuft_density`, `fine_density`, or `thatch_density`, but those channels do not independently control the corresponding tuned pass.

#### H8. Rendering dirt clods immediately would double relief

The ground profile already contributes aggregate-scale geometry. The generic dirt-clod family is a second representation of coarse soil structure and needs a later ownership decision, not automatic inclusion in the meadow bridge.

#### H9. Mesh relief and shader relief are currently different surfaces

The mesh path evaluates each band in Rust as two rotated copies of a single-frequency addressed value-noise field, then applies a monotonic power transform, per-band compaction, moisture flattening, and clustering. The Blender bump path instead uses Blender Noise with internal detail, applies a non-monotonic folded ridge transform, and only applies moisture flattening. A band moving across the geometry/bump threshold therefore changes its phase, spectrum, morphology, and response to state.

That violates the intended three-tier ladder: a quality or resolution change can change *what the ground is*, rather than only which representation carries it.

#### H10. Ground state is exported but not fully consumed

`GroundSurface` contains `moisture`, `compaction`, `wet_film`, and `cavity`. The current Blender graph creates attributes only for moisture and cavity. It darkens and smooths from moisture directly, does not flatten shader bands by compaction, and does not use `wet_film` to create a surface coat. The unresolved-band roughness term is also constant per profile, so a fully compacted or saturated grain field can remain just as micro-rough as a loose dry one.

#### H11. The supplied snapshot references a benchmark crate that is absent from the tree

The workspace, CLI, documentation, and generator fixtures refer to `crates/terrain_bench`, but the supplied directory snapshot does not contain that crate. The implementing agent must recover it from source control if it exists elsewhere, or recreate the minimal benchmark crate and committed-baseline structure before claiming benchmark coverage. Silently deleting the workspace dependency is not an acceptable fix.

---

## 3. Definition of done

The meadow tier is complete only when all requirements in this section hold.

### 3.1 Functional acceptance

1. A valid terrain document can declare and render:
   - tuned grass tufts;
   - tuned fine grass;
   - tuned thatch;
   - tuned broadleaf;
   - secondary flowers;
   - secondary stones/fragments;
   - secondary undergrowth.
2. `terrain compile assets/terrain/documents/meadow_path.terrain.ron` produces one image containing the declared secondary content.
3. Blender performs no placement randomness.
4. Secondary roots use the same final displaced surface as the rendered ground.
5. Stones are visibly buried and grass responds around them.
6. Flower stems and heads remain one coherent object across trace slices.
7. Lowering one tuned population control affects only that tuned pass and its local interactions.
8. Setting a secondary population density to zero removes that population without moving any other secondary population.

### 3.2 Determinism acceptance

For a fixed document, seed, compiler version, recipe versions, and render geometry profile:

- candidate identity is independent of traversal order;
- variable-radius exclusion is independent of traversal order;
- two overlapping compile windows make identical decisions for every candidate in their overlap;
- lowering density removes candidates without moving survivors;
- prototype choice and all transforms are addressed by named streams;
- trace slicing does not move, rotate, resize, recolour, or reassign any secondary object;
- a whole render and the union of its independently selected trace slices have equal secondary entity IDs;
- every scene-affecting value reaches the scene fingerprint;
- changing render-only sampling or denoising does not move the scene fingerprint.

### 3.3 Interaction acceptance

For each accepted stone:

- no tuned stem root lies inside its hard footprint unless that pass explicitly opts out;
- response outside the configured influence radius is bit-identical to a scene without that stone;
- within the response band, grass bends predominantly away from the nearest stone and shortens smoothly;
- the response is continuous at the outer boundary;
- the field gives the same answer regardless of trace slice;
- removing stones is permitted to reveal or unbend latent grass only inside the declared interaction radius. The latent grass address and random attributes must remain unchanged.

The last clause is the precise interpretation of “turning stones off moves nothing else.” Physical interaction necessarily changes nearby visible grass; it must not perturb unrelated grass or any addressed random draw.

### 3.4 Visual acceptance

Across the committed seed set and at least these views:

- full 3×3 meadow plate;
- centre-tile crop;
- close crop of a flower patch;
- close crop of stones in grass;
- sparse path fringe;
- high-density meadow;

reviewers must see:

- no obvious lattice or diagonal rhythm;
- no doubled grass density at substrate transitions;
- no uniformly sprinkled “gravel field” stone pattern;
- no floating stones;
- no circular bald holes around stones;
- no disconnected flower heads;
- no undergrowth taller than the canopy role permits;
- no hard population cutoff at the path edge;
- no trace-tile seams or shadow discontinuities;
- no lower-quality generic grass mixed into the tuned canopy.

### 3.5 Performance acceptance

Every performance report must pair time/memory with quality counters:

- candidates generated;
- candidates after priority thinning;
- candidates accepted;
- candidates owned/unowned;
- marks and instances per population;
- tuned strokes per pass;
- prototype count and instance count;
- scene-package bytes;
- Blender object count;
- render time;
- weakest-seed visual score;
- join error.

A faster result produced by fewer flowers, stones, blades, or shadow casters is a quality-tier change, not an optimisation.

### 3.6 Ground/soil acceptance

The ground system additionally passes all of these gates:

1. **Tier partition:** every relief band, ripple field, and crack contribution has one recorded representation owner. Nothing is omitted and nothing is counted twice.
2. **Cross-tier identity:** a canonical band evaluated as geometry and as a Rust-authored bump field has the same world phase, aggregate shape, clustering, and state scale at shared sample positions.
3. **Resolution invariance:** moving a band from geometry to bump, or bump to microfacet, preserves dominant wavelength and integrated relief energy within the declared handoff tolerance.
4. **State response:** measured RMS height and spectral band energy follow the profile's compaction and saturation response; unresolved slope variance falls with the same state.
5. **Wet-film separation:** moisture changes substrate albedo and pore-scale response; `wet_film` controls a distinct dielectric coat with the profile's film IOR. A wet coat may not appear on a dry crown merely because the substrate moisture channel is nonzero.
6. **Morphology:** clod, crumb, grain, crack, and ripple laboratories satisfy their wavelength, amplitude, anisotropy, and network targets.
7. **Composability:** overlapping ground grids, bump fields, material AOVs, and state fields are bit-identical on the supported platform wherever they address the same world samples.
8. **Evidence:** every intentional ground look change includes the ground benchmark report, raw AOVs, PSD/semivariogram plots, state sweeps, performance breakdown, and weakest-seed render.

---

## 4. Non-goals

This phase does **not** include:

- replacing the tuned grass generator;
- writing a raster renderer;
- moving production wholesale onto `write_package`;
- neural-renderer training;
- dataset generation;
- snow or a general cover solver;
- cliffs, tile elevation steps, or camera pitch;
- puddles or standing-water meshes;
- path ruts requiring populated `FeatureContext`;
- a universal botany simulator;
- interactive Blender Geometry Nodes scattering;
- a general-purpose arbitrary 3D asset importer;
- rendering the generic dirt-clod population before coarse-relief ownership is resolved.

---

# Part II — Target architecture

## 5. End-to-end target flow

```text
TerrainDocument + profiles
        │
        ▼
PreparedTerrain
        │
        ├── sample one edge-addressed TerrainFieldStack over generated bounds
        │
        ├── derive slope / curvature / flow / exposure / boundary frame
        │
        ├── construct ONE GroundEvaluator
        │       └── final surface, realised substrate, state, relief, cavity, wet film
        │
        ├── compile population declarations
        │       ├── Tuned controls: tuft / fine / thatch / broadleaf
        │       ├── Secondary scene: flowers / stones / undergrowth
        │       └── Deferred: dirt clods
        │
        ├── build InteractionField from accepted stone footprints
        │
        ▼
MeadowCompilation
        │
        ├── for each trace slice:
        │       ├── tuned GrassScene reads shared evaluator, controls, interactions
        │       ├── secondary selector reads the one compiled scene
        │       └── HybridCyclesScene lowers both without regenerating either
        │
        ▼
scene.json v2 + binary buffers
        │
        ▼
Blender: build prototypes once, link explicit instances, render Cycles
        │
        ▼
logical 3×3 plate
```

## 6. Proposed top-level type

The existing `SceneCompilation` should become the authoritative compilation product rather than one half of a split pipeline.

```rust
pub struct SceneCompilation {
    pub scene: Arc<TerrainScene>,
    pub fields: Arc<TerrainFieldStack>,
    pub ground: Arc<GroundEvaluator>,
    pub tuned: Arc<TunedPopulationSet>,
    pub interactions: Arc<InteractionField>,
    pub report: SceneCompileReport,
}
```

Naming may remain `SceneCompilation`; no separate `MeadowCompilation` type is required if the broader type is documented accurately.

### Required ownership rules

- `fields` is constructed exactly once.
- `ground` owns or shares `fields`; callers do not construct another evaluator.
- `scene` is built against `ground.final_surface_z`.
- `interactions` is derived only from accepted, owned secondary emissions in `scene`.
- `tuned` is compiled from the same population declarations and field indices as `scene`.
- every member is immutable and `Send + Sync` after construction.

## 7. Render classes

Add an explicit recipe method:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeRenderClass {
    Tuned(TunedPass),
    Secondary,
    Deferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TunedPass {
    Tuft,
    Fine,
    Thatch,
    Broadleaf,
}

pub trait TerrainRecipe: Send + Sync {
    fn key(&self) -> RecipeKey;
    fn render_class(&self) -> RecipeRenderClass;
    // existing methods...
}
```

### Compiler behaviour by class

#### `Tuned(pass)`

- validate the population;
- compile a `TunedPopulationControl`;
- do not emit its generic marks into the production secondary scene;
- reject two enabled populations claiming the same tuned pass in version 1.

The one-population restriction is deliberate. The tuned generator has one pass identity, so silently folding two authored populations into it would destroy persistent population identity and make density semantics ambiguous. A future version may define a stable merge; version 1 reports an error.

#### `Secondary`

- participate in candidate domains, acceptance, ownership, and recipe emission;
- emit grouped marks, prototype instances, and interaction primitives;
- lower through the hybrid Cycles bridge.

#### `Deferred`

- validate and report the declaration;
- do not emit production geometry;
- print a specific note, not a silent omission.

## 8. Tuned population controls

A tuned pass requires a spatial evaluator, not a scalar toggle.

```rust
pub struct TunedPopulationControl {
    pub population: PopulationKey,
    pub pass: TunedPass,
    pub material_affinity: Vec<(MaterialIndex, f32)>,
    pub abundance_channel: Option<ModifierIndex>,
    pub target_density_per_m2: f64,
    pub reference_density_per_m2: f64,
}

pub struct TunedPopulationSet {
    controls: BTreeMap<TunedPass, TunedPopulationControl>,
}
```

At world position `p`, using the already realised substrate `W_m(p)`:

\[
A_{mat}(p)=
\begin{cases}
1, & \text{if the affinity table is empty},\\
\sum_m W_m(p)\,a_m, & \text{otherwise.}
\end{cases}
\]

Let `A_channel(p)` be the declared abundance-channel value, or one if absent. Let

\[
A_{density}=\frac{\rho_{document}}{\rho_{reference}}.
\]

Then the initial pass factor is

\[
F_{pass}(p)=\max(0,A_{mat}(p))\max(0,A_{channel}(p))\max(0,A_{density}).
\]

`reference_density_per_m2` is pinned per tuned pass to the existing `GrassStyle` defaults:

| Tuned pass | Initial reference density |
| --- | ---: |
| Tuft | 50 tuft anchors/m² |
| Fine | 3800 blades/m² |
| Thatch | 395 strokes/m² |
| Broadleaf | 4 clusters/m² |

The document defaults should be migrated or calibrated so a standard meadow gives `A_density≈1`. Do not interpret the current generic-family defaults as already equivalent: generic tufts and tuned tuft anchors have different morphology and mark count.

### Placement integration

Change tuned `scatter` to accept a pass control:

```rust
fn scatter(
    ...,
    pass: TunedPass,
    per_square_metre: f32,
    style_weight: impl Fn(&Ground) -> f32,
    ...,
) {
    ...
    let semantic = bed.field.population_factor(pass, root);
    let probability = ground.density
        * coverage
        * style_weight(&ground)
        * semantic;
    if !draw.chance(probability.min(1.0)) { continue; }
    ...
}
```

No existing `Draw` sequence is shifted. The pass factor is a pure field query and a multiplication before the existing acceptance draw.

## 9. Placement anchors and grouped emissions

Every accepted candidate that emits one or more primitives receives one group anchor.

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacementAnchor {
    pub candidate: CandidateId,
    pub root: WorldPoint,
}
```

Every `SceneMark`, `PrototypeInstance`, and `InteractionPrimitive` stores either the anchor directly or an index into a stable anchor table.

This solves four problems:

- trace-slice selection keeps a whole flower together;
- reports can count candidate groups rather than only primitive marks;
- interaction geometry can name the object that caused it;
- fingerprints preserve grouping semantics.

The existing `MarkId(candidate, part)` remains the primitive identity. `PlacementAnchor` is not a replacement; it is the shared parent.

## 10. Scene vocabulary additions

### 10.1 Prototype table

Add to `TerrainScene`:

```rust
pub struct TerrainScene {
    // existing fields...
    pub anchors: Vec<PlacementAnchor>,
    pub prototypes: Vec<PrototypeBinding>,
    pub interactions: Vec<InteractionPrimitive>,
}
```

Add to `SceneBuilder`:

```rust
pub fn bind_prototype(&mut self, binding: PrototypeBinding) -> PrototypeIndex;
pub fn bind_anchor(&mut self, anchor: PlacementAnchor) -> AnchorIndex;
pub fn push_interaction(&mut self, interaction: InteractionPrimitive);
```

Bindings are canonical and idempotent: equal bindings return the same index. Canonical table order is binding insertion order from deterministic recipe traversal, or key order if bindings are precollected; it may never depend on a hash map's iteration order.

### 10.2 Interaction primitives

```rust
pub struct InteractionPrimitive {
    pub source: MarkId,
    pub anchor: AnchorIndex,
    pub centre: WorldPoint,
    pub shape: InteractionShape,
    pub hard_clearance_m: f32,
    pub response_reach_m: f32,
    pub channels: InteractionChannels,
}

pub enum InteractionShape {
    Ellipse {
        semi_u_m: f32,
        semi_v_m: f32,
        yaw_rad: f32,
    },
}
```

Version 1 needs only an oriented ellipse. It is a conservative ground-plane footprint for a stone or fragment and is cheap enough to query at every prospective grass root.

### 10.3 Recipe emissions

Replace a mark-only sink with typed emissions:

```rust
pub enum RecipeEmission {
    Mark(EmittedMark),
    Instance(EmittedInstance),
    Interaction(EmittedInteraction),
}

pub trait RecipeOutput {
    fn emit(&mut self, emission: RecipeEmission);
}
```

An `EmittedInstance` names a prototype binding by semantic key plus explicit parameters. The scene builder binds it deterministically and writes an instance using the resulting dense index.

---

# Part III — Deterministic sampling and interaction science

## 11. Why ordinary Poisson-disk generation is not the core algorithm

Classical Poisson-disk algorithms solve a related but different problem: produce a well-spaced random set inside one requested domain. Bridson's grid-accelerated algorithm, for example, grows an active list and proposes candidates around previously accepted points. It is efficient and visually useful, but the accepted set is history-dependent.

Groundwork requires a stronger property:

> The life or death of a candidate must be derivable from the candidate and a bounded world neighbourhood, without knowing which render window or traversal generated it.

That is why the current non-recursive priority thinning is the correct family of algorithm. It is equivalent in spirit to assigning every proposal a time/priority mark and retaining it only when no earlier/higher-priority proposal lies in its inhibition neighbourhood—a Matérn type-II hard-core construction.

This process is not necessarily maximal: a rejected high-priority point can reject a lower-priority point even if the high-priority point is itself rejected by someone else. That conservatism is intentional. Recursive “only survivors may reject” rules require a dependency traversal whose boundary can extend unpredictably, undermining finite-halo composability.

## 12. Candidate-specific physical radius

### 12.1 Domain model

Replace fixed exclusion radius with an addressed radius policy:

```rust
pub enum SpacingPolicy {
    Jittered,
    PriorityExclusion {
        radius: CandidateRadiusPolicy,
        clearance_m: f64,
    },
}

pub enum CandidateRadiusPolicy {
    Fixed { radius_m: f64 },
    Uniform {
        min_m: f64,
        max_m: f64,
        stream: StreamKey,
    },
    // Later: discrete authored classes, never an arbitrary closure in a file format.
}

pub struct DomainCandidate {
    pub id: CandidateId,
    pub position: WorldPoint,
    pub priority: f32,
    pub footprint_radius_m: f32,
}
```

The radius belongs to the **shared domain candidate**, not to its eventual owner. Acceptance must remain before ownership, so all claimants sharing a domain must agree about the candidate footprint.

For stones, candidate radius is also the primary physical horizontal radius. The recipe may derive non-uniform semi-axes around it without changing the conservative exclusion disk.

### 12.2 Symmetric conflict rule

For candidates `i` and `j` with centres `x_i,x_j`, footprint radii `r_i,r_j`, and authored clearance `c≥0`:

\[
\operatorname{conflict}(i,j)
\iff
\lVert x_i-x_j\rVert^2 < (r_i+r_j+c)^2.
\]

A sum is chosen because `r_i` and `r_j` represent physical object footprints. The variable-radius sampling literature distinguishes prior-point, current-point, maximum-radius, minimum-radius, and sum-of-radii rules. The sum has the direct sphere-packing interpretation needed here: two occupied footprint disks plus clearance do not overlap.

The rule is symmetric, so permuting generation order cannot change whether a pair conflicts.

### 12.3 Total priority order

Define a strict total key:

```rust
fn priority_key(c: &DomainCandidate) -> PriorityKey {
    PriorityKey {
        priority_bits: canonical_f32_bits(c.priority),
        domain_hash: c.id.population.bits(),
        cell_y: c.id.cell.y,
        cell_x: c.id.cell.x,
        rank: c.id.rank,
    }
}
```

Lexicographically greater wins. The exact field order is pinned by `DOMAIN_ALGORITHM_VERSION`.

Using only `rank` as the tie-break is insufficient because different cells can share a rank. Exact 32-bit priority collisions are rare, but determinism contracts are not probabilistic.

### 12.4 Survival rule

\[
\operatorname{keep}(i)
\iff
\nexists j:\operatorname{conflict}(i,j)\land K(j)>K(i).
\]

This is non-recursive. A candidate's status depends on all raw proposals in its bounded conflict neighbourhood, not on the survival status of those proposals.

### 12.5 Spatial index

Let

\[
R_{max}=\max_i r_i,
\quad
H=2R_{max}+c.
\]

Any candidate that can conflict with `i` is at most `r_i+Rmax+c≤H` away.

Use a bucket side `b=Rmax` when `Rmax>0`. For a candidate of radius `r_i`, inspect

\[
n_i=\left\lceil\frac{r_i+R_{max}+c}{b}\right\rceil
\]

buckets in every direction. For a fixed-radius domain with zero clearance, this reduces to a 5×5 neighbourhood under footprint-sum semantics. The current 3×3 search corresponds to treating its single radius as the complete centre-to-centre inhibition distance, not as a physical footprint radius.

For sparse stone domains the slightly larger query is negligible. Correct semantics are more important than retaining a 3×3 constant.

### 12.6 Pseudocode

```rust
fn generate_domain(req: &DomainRequest) -> Vec<DomainCandidate> {
    let max_radius = req.definition.spacing.maximum_candidate_radius();
    let clearance = req.definition.spacing.clearance_m();
    let halo = match req.definition.spacing {
        Jittered => 0.0,
        PriorityExclusion { .. } => 2.0 * max_radius + clearance,
    };

    let working = req.bounds.expanded(halo);
    let all = lay_out_with_addressed_radius(req.definition, working, req.seeds);

    match req.definition.spacing {
        Jittered => filter_half_open(all, req.bounds),
        PriorityExclusion { .. } => priority_thin_variable(
            &all,
            req.bounds,
            max_radius,
            clearance,
        ),
    }
}

fn priority_thin_variable(
    all: &[DomainCandidate],
    keep_bounds: WorldRect,
    max_radius: f64,
    clearance: f64,
) -> Vec<DomainCandidate> {
    if max_radius <= 0.0 {
        return all.iter().copied()
            .filter(|c| keep_bounds.contains(c.position))
            .collect();
    }

    let bucket_side = max_radius;
    let grid = CellGrid::new(bucket_side);
    let mut buckets: BTreeMap<CellCoord, Vec<usize>> = BTreeMap::new();

    for (slot, candidate) in all.iter().enumerate() {
        buckets.entry(grid.cell_at(candidate.position))
            .or_default()
            .push(slot);
    }

    let mut out = Vec::new();
    for (slot, candidate) in all.iter().enumerate() {
        if !keep_bounds.contains(candidate.position) { continue; }

        let search_m = candidate.footprint_radius_m as f64
            + max_radius
            + clearance;
        let cells = (search_m / bucket_side).ceil() as i64;
        let home = grid.cell_at(candidate.position);
        let mut excluded = false;

        'neighbours: for dy in -cells..=cells {
            for dx in -cells..=cells {
                let Some(slots) = buckets.get(&home.offset(dx, dy)) else {
                    continue;
                };
                for &other_slot in slots {
                    if other_slot == slot { continue; }
                    let rival = &all[other_slot];
                    if priority_key(rival) <= priority_key(candidate) { continue; }

                    let limit = candidate.footprint_radius_m as f64
                        + rival.footprint_radius_m as f64
                        + clearance;
                    if squared_distance(candidate.position, rival.position)
                        < limit * limit
                    {
                        excluded = true;
                        break 'neighbours;
                    }
                }
            }
        }

        if !excluded { out.push(*candidate); }
    }
    out
}
```

### 12.7 Seam proof

Let the visible/requested region be `B`. The generator lays out all raw candidates in the expanded region

\[
W=B\oplus H,\qquad H=2R_{max}+c.
\]

For any candidate `i∈B`, every candidate capable of conflicting with it is within `r_i+Rmax+c≤H`, hence lies in `W`. Candidate position, radius, and priority are pure functions of its address. Therefore every compile window containing `i` and expanded by `H` compares `i` against the same complete conflict set and reaches the same decision.

Half-open filtering then gives an exact owner to a candidate on a shared boundary, so the union of adjacent windows contains the candidate once, not zero or twice.

### 12.8 Density semantics

Priority exclusion lowers the number of available candidates. Computing an exact post-exclusion density normaliser from the current render window would make acceptance depend on window size, which is forbidden.

For version 1:

- recipe `density` remains an **offered density** relative to the domain's raw lattice capacity;
- `accepts` remains a candidate-addressed threshold before ownership;
- the compiler report records raw, post-exclusion, and accepted densities separately;
- authored defaults are calibrated against measured post-exclusion output for the committed domain definition.

A future domain may carry a versioned, stationary retention calibration, but no local count may be used as an acceptance denominator.

## 13. Interaction field

### 13.1 Purpose

The interaction field is a deterministic query structure over accepted obstacles. It does not generate anything. It answers:

```text
At this world point, is a plant root blocked?
If not, how strongly is it influenced by the nearest obstacle?
Which direction is away from that obstacle?
```

### 13.2 Elliptical footprint

For an ellipse with centre `c`, yaw rotation matrix `R`, and semi-axes `a,b`, transform point `p` into local coordinates:

\[
q=R^T(p-c).
\]

Define the normalised elliptical radius

\[
\rho=\sqrt{(q_x/a)^2+(q_y/b)^2}.
\]

A stable approximate signed clearance is

\[
d=(\rho-1)\min(a,b).
\]

This is exact on the minor-axis scale and conservative enough for root exclusion. An exact Euclidean ellipse distance requires an iterative solve and buys no visible benefit at the centimetre tolerances involved.

The unnormalised outward gradient in local space is

\[
g_l=(q_x/a^2,\ q_y/b^2).
\]

Rotate and normalise:

\[
n=\operatorname{normalize}(Rg_l).
\]

If `g_l` is numerically zero at the centre, use an addressed fallback direction derived from the obstacle ID; never return a zero direction.

### 13.3 Smooth influence

Let hard clearance be `h≥0`, response reach be `L>0`, and `d` the signed ellipse clearance. Define distance beyond the hard boundary:

\[
u=\max(0,d-h).
\]

Then

\[
w=1-\operatorname{smoothstep}(0,L,u).
\]

Properties:

- `w=1` at and inside the hard boundary;
- `w=0` at and beyond `h+L`;
- first derivative is zero at both endpoints;
- no visible kink appears where the response turns on or off.

### 13.4 Spatial index

Use a deterministic bucket index. Each interaction is inserted into every bucket touched by its influence AABB, expanded by `h+L`. Queries then inspect only the point's bucket.

```rust
pub struct InteractionField {
    bucket_m: f64,
    buckets: BTreeMap<CellCoord, Vec<InteractionIndex>>,
    primitives: Vec<InteractionPrimitive>,
}
```

Within each bucket, indices are sorted by stable source ID. Query order therefore cannot change tie handling.

### 13.5 Query result

```rust
pub struct InteractionSample {
    pub blocked: bool,
    pub influence: f32,
    pub away: [f32; 2],
    pub clearance_m: f32,
    pub source: Option<MarkId>,
}
```

Choose the primitive with minimum signed clearance as the directional source. `influence` is the maximum influence among eligible primitives. Do not sum outward directions from several stones: approximately opposite stones can cancel to zero and make a root between them lean nowhere. The nearest-boundary primitive gives the most physically legible response.

### 13.6 Pseudocode

```rust
fn sample_interactions(
    field: &InteractionField,
    p: WorldPoint,
    pass: TunedPass,
) -> InteractionSample {
    let cell = field.grid.cell_at(p);
    let Some(indices) = field.buckets.get(&cell) else {
        return InteractionSample::none();
    };

    let mut best: Option<(f32, f32, [f32; 2], f32, MarkId)> = None;
    for &index in indices {
        let obstacle = &field.primitives[index.index()];
        if !obstacle.channels.contains(pass) { continue; }

        let (clearance, away) = ellipse_clearance_and_normal(obstacle, p);
        let u = (clearance - obstacle.hard_clearance_m).max(0.0);
        let influence = 1.0 - smoothstep(
            0.0,
            obstacle.response_reach_m.max(1.0e-6),
            u,
        );

        match best {
            None => best = Some((
                clearance,
                influence,
                away,
                obstacle.hard_clearance_m,
                obstacle.source,
            )),
            Some((best_clearance, _, _, _, best_id))
                if clearance < best_clearance
                || (clearance == best_clearance && obstacle.source < best_id) =>
            {
                best = Some((
                    clearance,
                    influence,
                    away,
                    obstacle.hard_clearance_m,
                    obstacle.source,
                ));
            }
            _ => {}
        }
    }

    match best {
        None => InteractionSample::none(),
        Some((clearance, influence, away, hard_clearance, source)) => InteractionSample {
            blocked: clearance <= hard_clearance,
            influence,
            away,
            clearance_m: clearance,
            source: Some(source),
        },
    }
}
```

### 13.7 Tuned grass response

Each tuned pass declares response coefficients:

```rust
pub struct ObstacleResponse {
    pub hard_exclusion: bool,
    pub direction_strength: f32,
    pub shortening: f32,
    pub extra_bend_rad: f32,
}
```

Suggested starting values:

| Pass | Direction strength | Max shortening | Extra bend | Hard exclusion |
| --- | ---: | ---: | ---: | ---: |
| Tuft | 0.75 | 0.25 | 0.35 rad | yes |
| Fine | 0.55 | 0.18 | 0.25 rad | yes |
| Broadleaf | 0.80 | 0.30 | 0.30 rad | yes |
| Thatch | 0.10 | 0.15 | 0.05 rad | yes, footprint only |

These are calibration seeds, not claimed biological constants.

For original horizontal growth direction `v0`, outward normal `n`, and influence `w`:

\[
v'=\operatorname{normalize}\big((1-\beta w)v_0+\beta w n\big)
\]

where `β` is direction strength. Length becomes

\[
L'=L(1-\alpha_L w),
\]

and bend becomes

\[
\theta'=\theta+\alpha_\theta w.
\]

If the blended vector is near zero, use `n` rather than normalising noise.

The response is applied after latent random morphology has been derived, so removing a stone reveals the same latent plant rather than a new random plant.

---
# Part IV — Final surface and content geometry

## 14. One final surface

### 14.1 Required evaluator API

Add an explicit final-height method:

```rust
impl GroundEvaluator {
    pub fn final_surface_z_m(&self, world: WorldPoint) -> f32 {
        let p = Vec2::new(world.u_m as f32, world.v_m as f32);
        self.fields.surface_height(world) + self.displacement(p)
    }
}
```

The exact internal method names may differ, but the semantic split must remain visible:

- authored elevation is macro shape;
- authored microrelief is document-level fine shape;
- profile geometry displacement is physical material relief resolved at the selected lattice spacing.

Do not add shader-only bump to object roots. A stone rests on geometry, not on a subpixel normal perturbation.

### 14.2 Compiler order

Reorder `compile_scene`:

```rust
fn compile_scene(...) -> Result<SceneCompilation, SceneCompileError> {
    let resolved = resolve_populations_and_domains(...)?;
    let halo = derive_halo(&resolved, terrain, options);
    let fields = Arc::new(sample_and_derive_fields(...)?);

    let band_spacing = BandSplit::spacing_for(loaded_profiles)
        .unwrap_or(options.fallback_ground_spacing_m);
    let ground = Arc::new(GroundEvaluator::new(
        terrain,
        Arc::clone(&fields),
        options.transition,
        band_spacing,
    ));

    let tuned = Arc::new(compile_tuned_controls(&resolved, terrain, &fields)?);
    let scene = Arc::new(compile_secondary_scene(
        &resolved,
        terrain,
        &fields,
        &ground,
        request,
    )?);
    let interactions = Arc::new(InteractionField::from_scene(&scene));

    Ok(SceneCompilation {
        scene,
        fields,
        ground,
        tuned,
        interactions,
        report,
    })
}
```

### 14.3 Recipe context

Change:

```rust
surface_z_m: fields.surface_height(candidate.position)
```

to:

```rust
surface_z_m: ground.final_surface_z_m(candidate.position)
```

Also give recipes access to the resolved ground state without requiring another transition call:

```rust
pub struct RecipeContext<'a> {
    pub fields: &'a TerrainFieldStack,
    pub ground: &'a GroundEvaluator,
    pub ground_sample: &'a GroundSample,
    pub seeds: SeedContext,
    pub parameters: &'a ParameterObject,
    pub substrate: RealisedSubstrate,
    pub surface_z_m: f32,
    pub root_seed: u64,
}
```

The compiler computes one `GroundSample` per accepted candidate and reuses it for affinity, ownership, recipe geometry, burial, tint, and interaction state.

## 15. Flower system

### 15.1 Content model

A flower candidate is one plant group:

```text
placement anchor
  ├── one curved stem
  ├── optional one or two small stem leaves
  ├── one disk/receptacle
  └── N petals in one or two whorls
```

The first production flower need not model a particular species. It must read as a small meadow flower at the target isometric scale and expose enough parameters for later authored variants.

### 15.2 Authored parameters

```text
density                  offered plants per m²
stem_length_m            median / base length
stem_radius_m            stem tube radius
stem_bend_rad             maximum total bend
head_radius_m             central disk radius
petal_count               integer range, default 5..8
petal_length_m            radial petal length
petal_width_m             maximum petal width
petal_cup_rad              tip lift/cup
cluster_scale_m            low-frequency abundance scale, optional in document
palette                    appearance/material selection
```

Unknown parameters remain errors.

### 15.3 Exact planar circular-arc stem

A stem begins vertical and bends by total angle `θ` toward horizontal unit direction `d=(d_x,d_y)`. Let normalised arc coordinate `s∈[0,1]`, arc length `L`, and curvature `κ=θ/L`.

For `|θ|>ε`:

\[
h(s)=\frac{L}{\theta}\left(1-\cos(\theta s)\right),
\]

\[
z(s)=\frac{L}{\theta}\sin(\theta s).
\]

The centreline is

\[
p(s)=p_0+h(s)(d_x,d_y,0)+z(s)(0,0,1).
\]

Its unit tangent is

\[
t(s)=\sin(\theta s)(d_x,d_y,0)+\cos(\theta s)(0,0,1).
\]

For `|θ|≤ε`, use the analytic limit:

\[
p(s)=p_0+Ls(0,0,1),\qquad t(s)=(0,0,1).
\]

Do not evaluate the divided formula at a tiny angle and hope floating-point cancellation is harmless.

### 15.4 Adaptive stem tessellation

Let maximum axial segment length be `ℓ_max` and maximum tangent turn per segment be `θ_max`. Choose

\[
n=\max\left(2,
\left\lceil\frac{L}{\ell_{max}}\right\rceil,
\left\lceil\frac{|\theta|}{\theta_{max}}\right\rceil
\right).
\]

Initial production values:

```text
ℓ_max = 0.035 m
θ_max = 10 degrees
radial sides = 5 at ordinary quality, 7 at high quality
```

The quality tier may increase tessellation; it may not remove the stem, petals, or leaves.

### 15.5 Rotation-minimising frames

Frenet frames are unsuitable for general plant curves because curvature can approach zero and the normal can flip at inflections. Use a rotation-minimising frame (RMF) to sweep tubes and oriented head planes.

For sampled positions `x_i`, unit tangents `t_i`, and one perpendicular frame axis `r_i`, the double-reflection update is:

```text
v1 = x[i+1] - x[i]
c1 = dot(v1, v1)
rL = r[i] - 2 * dot(v1, r[i]) / c1 * v1
tL = t[i] - 2 * dot(v1, t[i]) / c1 * v1

v2 = t[i+1] - tL
c2 = dot(v2, v2)
r[i+1] = rL - 2 * dot(v2, rL) / c2 * v2
s[i+1] = normalize(cross(t[i+1], r[i+1]))
r[i+1] = normalize(cross(s[i+1], t[i+1]))
```

Robust implementation:

```rust
fn advance_rmf(
    x0: Vec3,
    x1: Vec3,
    t0: Vec3,
    t1: Vec3,
    r0: Vec3,
) -> Frame {
    let v1 = x1 - x0;
    let c1 = v1.length_squared();
    if c1 < 1.0e-12 {
        return orthonormalise(t1, r0);
    }

    let r_l = r0 - v1 * (2.0 * v1.dot(r0) / c1);
    let t_l = t0 - v1 * (2.0 * v1.dot(t0) / c1);
    let v2 = t1 - t_l;
    let c2 = v2.length_squared();

    let r1 = if c2 < 1.0e-12 {
        r_l
    } else {
        r_l - v2 * (2.0 * v2.dot(r_l) / c2)
    };
    orthonormalise(t1, r1)
}
```

For the initial planar circular stem, a fixed binormal is mathematically sufficient. Implementing the reusable RMF now is still recommended because undergrowth leaves, branching stems, and future curved marks need it, and one tested sweep path is safer than several special cases.

### 15.6 Stem tube

At ring `i` with frame axes `(r_i,s_i,t_i)`, radius `R_i`, and radial angle `φ_j=2πj/m`:

\[
v_{i,j}=x_i+R_i(\cos\phi_j\,r_i+\sin\phi_j\,s_i).
\]

Taper slightly toward the head:

\[
R_i=R_0\left(1-0.25s_i\right).
\]

Normals are the radial frame direction. Connect adjacent rings with quads, split to triangles only if the binary format requires triangles.

### 15.7 Petal whorl

For `N` petals:

\[
\phi_k=\phi_0+\frac{2\pi k}{N}+\delta_k,
\]

where `φ_0` and each bounded jitter `δ_k` come from named streams and child path `[petal_index]`.

A petal is a short tapered ribbon in the flower-head plane. Let `u∈[0,1]` run from disk to tip:

\[
r(u)=R_{disk}+L_pu,
\]

\[
w(u)=W_p\sin(\pi u)^{p_w},
\]

\[
z(u)=C_p\sin(\pi u)-D_pu^2.
\]

`C_p` gives a gentle cup; `D_p` gives tip droop. Initial `p_w=0.75` gives a rounded petal that tapers at both ends.

Construct centreline direction in the head frame:

\[
e_k=\cos\phi_k\,r_{head}+\sin\phi_k\,s_{head}.
\]

The petal centre is

\[
P_k(u)=P_{head}+r(u)e_k+z(u)t_{head}.
\]

The ribbon sides are offset along

\[
e_k^\perp=-\sin\phi_k\,r_{head}+\cos\phi_k\,s_{head}.
\]

This creates explicit silhouette geometry and real self-shadowing. A single ellipsoid head does not.

### 15.8 Head disk

Use a shallow oblate superellipsoid or low-sided beveled disk aligned to the tip frame. The disk is a reusable prototype when several plants share head morphology; petals may either be part of the head prototype or emitted as grouped ribbons.

Version 1 recommendation:

- stem remains a curve/tube because length and bend vary continuously;
- head and petal whorl are selected from 4–8 reusable prototypes per appearance family;
- instance scale and rotation carry small variation;
- prototype choice is addressed by `flower_head_prototype`.

This greatly reduces Blender object/vertex creation while retaining visible diversity.

### 15.9 Flower emission pseudocode

```rust
fn emit_flower(
    candidate: &DomainCandidate,
    ctx: &RecipeContext,
    out: &mut dyn RecipeOutput,
) {
    let anchor = PlacementAnchor {
        candidate: candidate.id,
        root: candidate.position,
    };

    let length = parameter("stem_length_m")
        * lerp(0.78, 1.24, latent("stem_length"));
    let bend = parameter("stem_bend_rad")
        * lerp(0.25, 1.0, latent("stem_bend"));
    let azimuth = TAU * latent("stem_azimuth");
    let direction = vec2(cos(azimuth), sin(azimuth));

    let curve = circular_stem_curve(
        root = vec3(candidate.position, ctx.surface_z_m),
        direction,
        length,
        bend,
    );

    out.emit(Mark(Curve {
        anchor,
        part: 0,
        curve,
        radius_m: parameter("stem_radius_m"),
        appearance: "plant.flower_stem",
        attributes: addressed_flower_attributes(...),
    }));

    let tip = curve.tip();
    let prototype = choose_head_prototype(candidate, ctx);
    out.emit(Instance {
        anchor,
        part: 1,
        prototype,
        transform: frame_transform(
            translation = tip.position,
            tangent = tip.tangent,
            roll = TAU * latent("head_roll"),
            scale = head_scale(...),
        ),
        attributes: ...,
    });
}
```

### 15.10 Clustering

Flower clustering should remain authored through fields and abundance channels, not hidden inside a recipe-wide sequential process.

Recommended document pattern:

- a low-frequency noise source or painted scalar source;
- a ramp to select rich meadow patches;
- `flower_abundance` composed by `Max`;
- candidate acceptance multiplied by the sampled channel.

This preserves the compiler's causal model: the matrix says where flowers are likely; the candidate domain says which individual plants exist.

## 16. Stone and fragment prototypes

### 16.1 Why prototypes

A meadow stone needs enough silhouette variety to avoid repetition but does not need a unique mesh per instance. A small explicit prototype library gives:

- stable scene size;
- linked Blender data;
- real cast shadows;
- controllable burial geometry;
- deterministic variation through transform and material attributes.

Initial library:

```text
stone.rounded
stone.fractured
stone.flat
stone.elongated
fragment.shell
fragment.organic_dark
```

### 16.2 Superellipsoid base family

Use signed power

\[
\operatorname{spow}(x,e)=\operatorname{sign}(x)|x|^e.
\]

For latitude `η∈[-π/2,π/2]` and longitude `ω∈[-π,π)`:

\[
x=a\,\operatorname{spow}(\cos\eta,\epsilon_1)
       \operatorname{spow}(\cos\omega,\epsilon_2),
\]

\[
y=b\,\operatorname{spow}(\cos\eta,\epsilon_1)
       \operatorname{spow}(\sin\omega,\epsilon_2),
\]

\[
z=c\,\operatorname{spow}(\sin\eta,\epsilon_1).
\]

Useful exponent ranges:

```text
ε ≈ 1.0         ellipsoidal
ε < 1.0         squarer / fuller shoulders
ε > 1.0         pinched / diamond-like
```

Do not expose raw exponents as the only authoring language. Prototype recipes should name visual families and keep the numerical exponents in code or in a versioned prototype asset.

Suggested families:

| Prototype | `a:b:c` | `ε1` | `ε2` | Additional treatment |
| --- | --- | ---: | ---: | --- |
| Rounded | 1.0:0.85:0.60 | 0.9 | 0.9 | smooth low-order deformation |
| Fractured | 1.0:0.80:0.65 | 0.65 | 0.65 | planar cuts / hard normals |
| Flat | 1.0:0.85:0.28 | 0.85 | 0.8 | broad clipped base |
| Elongated | 1.0:0.52:0.48 | 0.8 | 0.75 | yaw variation |

### 16.3 Deterministic deformation

Avoid high-frequency displacement, which turns small stones into noisy potatoes. Apply a low-order radial field:

\[
r'(\hat v)=r(\hat v)\left[1+
\sum_{k=1}^{K} A_k N_k(\hat v)\right],
\]

where `K≤3`, wavelengths span the whole object, and `A_k` is small (`0.02..0.10`). Coefficients and phases are explicit prototype parameters derived in Rust or fixed in the binding; Blender does not draw random values.

For fractured prototypes, add two or three deterministic clipping planes:

\[
n_j\cdot x\le d_j.
\]

Clip or project vertices past the planes, then use split normals on the resulting faces. The planes are part of the prototype binding and therefore part of its fingerprint.

### 16.4 Base clipping and burial

A stone should intersect the soil. Prototype-local geometry may extend below local `z=0`; the instance translation controls burial.

Let unscaled prototype height be `H_p`, vertical scale be `s_z`, and burial fraction `b∈[0,1]`. Set instance origin:

\[
z_{instance}=z_{surface}-bH_ps_z.
\]

Suggested burial:

```text
rounded stone       0.20..0.38
fractured stone     0.16..0.32
flat pebble         0.28..0.48
shell fragment      0.12..0.28
organic fragment    0.18..0.40
```

Correlate burial weakly with object flatness and size. Very small fragments sit deeper; a perfectly constant burial fraction is visible as a common horizon line.

Burial is addressed by `burial_fraction`. It must not be sampled in Blender.

### 16.5 Physical footprint

Given candidate conservative radius `r`, derive ellipse axes:

\[
a=r\,s_a,\qquad b=r\,s_b,
\]

with `s_a,s_b` addressed but bounded so `max(a,b)≤r` if `r` is the candidate exclusion radius. Alternatively define candidate radius as `max(a,b)` exactly and derive the smaller axis only.

Recommended version-1 rule:

```text
major axis = candidate.footprint_radius_m
minor axis = major * lerp(0.55, 0.95, latent("axis_ratio"))
```

This guarantees the exclusion disk is conservative.

### 16.6 Stone placement score

Stones should not be uniformly spread merely because their affinity is empty. Their target density already reads `stone_abundance`; additionally, the recipe may use deterministic environmental weights that are causal and matrix-derived:

\[
S_{stone}=A_{channel}
           \cdot f_{slope}
           \cdot f_{blend}
           \cdot f_{loose}.
\]

Version 1 should keep these neutral unless the document declares them. Hidden procedural preference makes authoring difficult. A low-frequency `stone_abundance` source is the primary clustering mechanism.

### 16.7 Orientation and settlement

Yaw is addressed uniformly. Small tilt should be aligned partly with local ground normal and partly with the object's own addressed lean.

For ground normal `n_g`, build a tangent frame. Apply tilt vector in that frame with magnitude limited to roughly `0..12°` for ordinary stones and `0..20°` for fragments. Do not orient the object's local up exactly to a noisy micro-normal; use the geometry-scale ground normal or a smoothed normal, otherwise pebbles jitter like confetti.

### 16.8 Stone emission pseudocode

```rust
fn emit_stone(candidate: &DomainCandidate, ctx: &RecipeContext, out: &mut dyn RecipeOutput) {
    let major = candidate.footprint_radius_m;
    let axis_ratio = lerp(0.55, 0.95, latent("axis_ratio"));
    let minor = major * axis_ratio;
    let height = major * lerp(0.45, 1.10, latent("height_ratio"));

    let prototype = choose_weighted(candidate, "stone_prototype", [
        rounded, fractured, flat, elongated, shell, organic,
    ]);
    let yaw = TAU * latent("yaw");
    let burial = burial_range(prototype).sample(latent("burial"));
    let z = ctx.surface_z_m - burial * prototype.unit_height * height;

    let anchor = PlacementAnchor { candidate: candidate.id, root: candidate.position };
    let mark_id = MarkId::from_candidate(candidate.id, 0);

    out.emit(Instance {
        anchor,
        part: 0,
        prototype,
        transform: Transform3 {
            translation: [candidate.position.u_m, candidate.position.v_m, z],
            yaw_rad: yaw,
            tilt: addressed_tilt(...),
            scale: [major, minor, height],
        },
        attributes: addressed_stone_attributes(...),
    });

    out.emit(Interaction {
        source: mark_id,
        anchor,
        centre: candidate.position,
        shape: Ellipse { semi_u_m: major, semi_v_m: minor, yaw_rad: yaw },
        hard_clearance_m: parameter("root_clearance_m", 0.008),
        response_reach_m: parameter("grass_response_m", 0.11)
            * lerp(0.8, 1.25, major / reference_radius),
        channels: TUFT | FINE | BROADLEAF | THATCH,
    });
}
```

## 17. Meadow undergrowth

### 17.1 Role

Undergrowth is not another grass pass. It is a low, broad-leaved layer below or between the tuned canopy, visible primarily in sparse patches and at path fringes.

Version 1 uses rosette/cluster candidates. One candidate emits several leaves sharing a crown.

### 17.2 Parameters

```text
density                  clusters per m²
leaves                    integer range, default 3..8
leaf_length_m             default 0.06..0.18
leaf_width_m              default 0.018..0.055
crown_radius_m            root spread
rise_m                    middle lift
droop_m                   tip droop
height_bias               below-canopy control
```

### 17.3 Leaf ribbon

For leaf index `j`, choose

\[
\phi_j=\phi_0+\frac{2\pi j}{N}+\delta_j.
\]

Let radial direction `e_j=(cosφ_j,sinφ_j)`. For `u∈[0,1]`:

\[
r(u)=R_c(1-u)+Lu,
\]

\[
z(u)=z_0+H\sin(\pi u)-Du^q,
\]

\[
w(u)=W\sin(\pi u)^{p}.
\]

Initial values:

```text
q = 2
p = 0.65..0.9
H = 0.15..0.35 of leaf length
D = 0.05..0.25 of leaf length
```

Add a small lateral curl using a second orthogonal sinusoid, but keep it below about 10% of leaf length.

The ribbon needs a central ridge attribute for lighting and a slightly darker underside. Existing ribbon geometry and material attributes should be reused where possible.

### 17.4 Colony and flow alignment

A perfect radial rosette repeated everywhere is procedural. Blend each leaf azimuth toward the tuned flow direction:

\[
e'_j=\operatorname{normalize}((1-\lambda)e_j+\lambda f),
\]

where `λ` varies by cluster in `0.1..0.45`. Preserve enough radial structure that it remains a plant rather than a comb.

The cluster's overall abundance is driven by its document channel. Internal leaf count and morphology are addressed child decisions.

### 17.5 Obstacle response

Before emitting each leaf:

- reject the whole crown if its root is blocked;
- rotate the cluster's open side away from the nearest stone;
- shorten individual leaves that would cross deeply into the hard footprint;
- do not run geometry-geometry collision.

An inexpensive leaf-tip probe is sufficient:

```rust
let tip = root + leaf_direction * leaf_length;
let root_i = interactions.sample(root, Broadleaf);
let tip_i = interactions.sample(tip, Broadleaf);
let shortening = max(root_i.influence, tip_i.influence * 0.75);
```

### 17.6 Emission pseudocode

```rust
fn emit_undergrowth(candidate: &DomainCandidate, ctx: &RecipeContext, out: &mut dyn RecipeOutput) {
    let anchor = PlacementAnchor { candidate: candidate.id, root: candidate.position };
    let count = addressed_integer("leaf_count", min_leaves, max_leaves);
    let phase = TAU * latent("cluster_phase");
    let flow = ctx.fields.flow_direction(candidate.position).unwrap_or([1.0, 0.0]);

    for j in 0..count {
        let jitter = child_latent(j, "azimuth_jitter") * max_jitter - max_jitter * 0.5;
        let radial = unit_angle(phase + TAU * j as f32 / count as f32 + jitter);
        let flow_mix = lerp(0.1, 0.45, child_latent(j, "flow_mix"));
        let direction = normalize(lerp(radial, flow, flow_mix));

        let length = base_length * lerp(0.65, 1.25, child_latent(j, "length"));
        let interaction = ctx.interactions.sample(candidate.position, Broadleaf);
        let final_direction = turn_away(direction, interaction, response);
        let final_length = length * (1.0 - response.shortening * interaction.influence);

        out.emit(Mark(Ribbon {
            anchor,
            part: j,
            root: [candidate.position.u_m, candidate.position.v_m, ctx.surface_z_m],
            geometry: undergrowth_leaf(final_direction, final_length, ...),
            appearance: "plant.undergrowth_leaf",
            attributes: ...,
        }));
    }
}
```

## 18. Semantic bareness correction

### 18.1 Current failure

The current overlay computes:

```text
density *= vegetation_support * abundance
bare     = max(bare, 1 - vegetation_support)
```

Therefore a fertile meadow with abundance `0.1` grows one tenth as much grass but reports no semantic bare ground.

### 18.2 Required version-1 mapping

Let:

- `s=vegetation_support(p)`;
- `a=global vegetation abundance(p)`;
- `q=s*clamp(a,0,1)`.

Then:

```rust
let semantic_cover = support * abundance.clamp(0.0, 1.0);
let semantic_bare = 1.0 - semantic_cover.powf(BARE_RESPONSE_GAMMA);

ground.density *= support * abundance.max(0.0);
ground.bare = ground.bare.max(semantic_bare);
```

Set `BARE_RESPONSE_GAMMA=1.0` and pin it in a version constant. A later measured curve may change `γ`, but it must be a deliberate look change.

### 18.3 Exposed share

`WorldField::exposed_share` must return the same semantic quantity used by `sample`, not a separate complement of support:

```rust
pub fn exposed_share(&self, world: Vec2) -> f32 {
    self.overlay.as_ref()
        .map_or(0.0, |o| o.semantic_bare(world))
}
```

One function computes `semantic_bare`; both call it. Duplicate formulas will drift.

### 18.4 Why not infer exact canopy coverage

A physically exact uncovered fraction would require calibrated projected footprints and overlap statistics for every tuned pass. The current tuned generator's `bare` field is an artistic semantic control, not a measured Boolean union of blade shadows. Pretending otherwise would add false precision.

The linear version-1 mapping is explicit, monotone, testable, and easy to calibrate. It can later be replaced by a measured response curve without changing the document model.

## 19. Ground relief, shading, and state coherence contract

This section fixes the hardest ground-system problem before new content is calibrated against it. The goal is not to make Rust a renderer. The goal is to make Rust the owner of the **causal surface fields**, so Cycles shades the same ground the compiler, mesh, interactions, AOVs, and eventual neural corpus describe.

### 19.1 One relief plan per render

Add a resolved, fingerprinted plan:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReliefTier {
    Geometry,
    Bump,
    Microfacet,
}

pub struct PlannedReliefBand {
    pub profile: GroundProfileKey,
    pub band_index: u16,
    pub wavelength_m: f32,
    pub amplitude_m: f32,
    pub tier: ReliefTier,
}

pub struct GroundReliefPlan {
    pub geometry_spacing_m: f32,
    pub bump_spacing_m: Option<f32>,
    pub traced_pixel_m: f32,
    pub bands: Vec<PlannedReliefBand>,
    pub ripples: Vec<PlannedDirectionalRelief>,
    pub cracks: Vec<PlannedCrackRelief>,
    pub fingerprint: Fingerprint,
}
```

The current representation thresholds remain the initial policy:

```text
Geometry:   λ ≥ 4Δg
Bump:       λ < 4Δg and λ ≥ 2p
Microfacet: λ < 2p
```

where `Δg` is the fitted ground-mesh lattice spacing and `p` is the traced world-space pixel footprint. The bump field itself must sample every assigned band at no coarser than four samples per wavelength:

\[
\Delta_b \le \min_{b\in B}\frac{\lambda_b}{4}.
\]

If a memory budget forces `Δb` coarser, bands that no longer satisfy this condition are reclassified to microfacet and that decision is written into the plan. The field is never silently undersampled.

The hard tier split preserves the existing invariant that one band has one owner. Similarity at the handoff is obtained by calibrating the microfacet transfer to the bump reference and testing the resolution ladder, not by drawing the same band twice.

### 19.2 Make the Rust band basis a public internal contract

Refactor the current `GroundEvaluator` implementation into functions that can be reused by both geometry and derived render fields:

```rust
pub struct ReliefBandContext<'a> {
    pub profile: &'a GroundMaterialProfile,
    pub profile_key: &'a GroundProfileKey,
    pub band_index: usize,
    pub band: &'a ReliefBand,
    pub state: GroundState,
    pub world: Vec2,
}

pub fn unit_band_value(ctx: &ReliefBandContext<'_>, root_seed: u64) -> f32;
pub fn band_state_scale(ctx: &ReliefBandContext<'_>) -> f32;
pub fn band_height_m(ctx: &ReliefBandContext<'_>, root_seed: u64) -> f32;
```

`unit_band_value` owns all of the following:

- addressed seed and named stream;
- the two fixed rotated frames;
- one frequency and no hidden octave;
- the distribution-restoring scale;
- the monotonic aggregate-shape transform;
- zero-mean correction.

`band_state_scale` owns:

\[
q_b(c,m)=
\operatorname{clamp}(1-r_b c,0,1)
\operatorname{clamp}(1-f_w m,0,1)
q_{cluster}(x),
\]

where `r_b` is the band's compaction response and `f_w` is the profile's saturation flattening. Geometry, bump, cavity, slope variance, and debug AOVs must call the same functions. The Python material must contain no second implementation of noise or aggregate morphology.

### 19.3 Rust-authored bump field

The exporter samples the final sub-mesh relief over an integer-addressed world lattice:

```rust
pub struct GroundBumpField {
    pub grid: FieldGridSpec,
    /// Final realised sub-mesh height after profile weights and state, metres.
    pub height_m: ScalarPlane,
    /// Fine-scale cavity derived from that exact height field.
    pub cavity: ScalarPlane,
    /// RMS unresolved slope or calibrated microfacet parameter per sample.
    pub micro_slope_rms: ScalarPlane,
}
```

The final bump height at position `x` is

\[
h_B(x)=\sum_k w_k(x)
\left[
\sum_{b\in B_k} q_b(x)A_b u_b(x)
+h_{ripple,B,k}(x)-h_{crack,B,k}(x)
\right],
\]

using the **realised** substrate weights from the same `GroundEvaluator`. This is important: exporting one independent image per semantic material and blending later invites a second boundary realization. A single final field is the least ambiguous contract.

The field is written as a versioned single-channel `f32` plane with:

- integer world origin;
- spacing in metres;
- columns and rows;
- edge/centre anchor;
- row order;
- checksum;
- relief-plan fingerprint.

Blender creates a float image, uploads the values in bulk, marks it non-colour data, and maps world lattice sample `(i,j)` to texel centre:

\[
(u,v)=\left(\frac{i+1/2}{N_x},\frac{j+1/2}{N_y}\right).
\]

The exported halo must be at least one bump texel plus the support/reach of every represented contribution, so bilinear filtering never clamps inside camera-visible ground.

### 19.4 Fine cavity comes from the same height

For the bump tier, cavity is not independent noise. A first implementation derives it from locally normalised relief:

\[
c_B(x)=\operatorname{clamp}
\left(
\frac{\mu_{r}(x)-h_B(x)}{2\,a_r(x)+\varepsilon}+\frac12,
0,1
\right),
\]

where `μr` is a low-pass local mean and `ar` is a robust local half-range or RMS amplitude over a window tied to the coarsest bump wavelength. A cheaper profile-amplitude normalisation is acceptable for version 1 if the benchmark demonstrates that cavity remains positively correlated with geometric hollows and does not create halos at material boundaries.

The beauty shader may combine mesh cavity and fine cavity, but both must originate from their actual relief fields.

### 19.5 Microfacet transfer from unresolved slope, not an arbitrary amplitude constant

For each unresolved band, height amplitude shrinks linearly with state and slope variance shrinks quadratically. Let

\[
\sigma_s^2 = E\left[\left(\frac{\partial h}{\partial u}\right)^2+
                         \left(\frac{\partial h}{\partial v}\right)^2\right].
\]

For independent unresolved bands,

\[
\sigma_{s,total}^2 \approx \sum_b q_b^2\sigma_{s,b}^2.
\]

The band constants `σ²s,b` are measured once from the exact Rust basis over a large pinned world window; they are not guessed from `A/λ`. At runtime the evaluator combines them with state and realised profile weights to export `micro_slope_rms`.

Blender's Principled roughness is not numerically equal to RMS height slope, so use a calibrated transfer function:

\[
r_{micro}=L(\sigma_s),
\]

where `L` is a monotone LUT fitted in the BRDF calibration laboratory. The bootstrap guess may use `alpha≈σs` and Blender's `alpha≈roughness²`, hence `roughness≈sqrt(σs)`, but that approximation is not the release calibration.

The LUT is fitted by rendering explicit high-resolution microgeometry and a flat Principled patch at a grid of incident/view angles, then solving

\[
r^*(\sigma_s)=\arg\min_r
\sum_j \omega_j
\left[
\log(L_{micro,j}+\epsilon)-\log(L_{GGX,j}(r)+\epsilon)
\right]^2.
\]

Store the fitted knots and their digest in the render profile. This makes moving a band from bump to microfacet a measured representation change rather than a magic multiplier.

### 19.6 Separate substrate moisture from surface film

The shader receives all four canonical ground-state attributes:

```text
moisture       compaction       wet_film       cavity
```

Their responsibilities are distinct:

- `moisture` drives the profile's wet albedo curve, saturation flattening, and base-substrate roughness response;
- `compaction` has already altered every relief tier in Rust and may additionally affect dry base roughness only if the profile explicitly declares that response;
- `wet_film` drives **Principled Coat Weight**;
- the profile supplies **Coat Roughness** and **Coat IOR**, with water-like film IOR near the declared `film_ior`;
- `cavity` places tonal occlusion in actual hollows.

The required node relationship is conceptually:

```text
Base Color       = blended_profile_albedo(tone, moisture, cavity)
Base Roughness   = blended_profile_base_roughness(tone, moisture, micro_slope_rms)
Coat Weight      = wet_film
Coat Roughness   = blended_profile_film_roughness
Coat IOR         = blended_profile_film_ior
Normal           = one bump from ground_bump.height_m
```

A film is not a puddle. Standing water remains a separate future mesh/cover solver.

### 19.7 Ripple and crack ownership also follows the plan

Ripples and cracks may not bypass the tier ladder merely because they are not stored in `structure.bands`.

For ripples, use `wavelength_m` exactly like a relief band. The same Rust ripple function supplies geometry or bump. Below the visible threshold, its unresolved directional slope contributes anisotropic roughness only if the material model can represent it; otherwise report that the directional signal was dropped rather than hiding it in isotropic noise.

For cracks, the controlling representable scale is the crack/shoulder width, not the polygon diameter:

```text
geometry if crack width and curl shoulder are mesh-resolved;
bump if they are pixel-visible but not mesh-resolved;
sub-pixel contribution only to roughness/cavity if neither is visible.
```

The crack network identity and signed-distance field are generated once in Rust. Geometry and bump are two evaluations of that same field.

### 19.8 Required ground render package additions

Add to the active scene package:

```text
ground_relief_plan.json
ground_bump.f32
ground_bump_manifest.json
micro_slope_rms.f32        # may share the bump grid
```

The package manifest records:

- field dimensions and mapping;
- every assigned contribution and tier;
- profile and render-profile digests;
- checksums;
- state-channel names;
- calibration-LUT digest;
- any budget-driven reclassification.

A package whose plan claims a bump band but lacks the bump plane is invalid. A profile band appearing in two tiers is invalid. A band in no tier is invalid.

---

## 20. Soil-profile decision experiment

### 20.1 Hypothesis

The meadow floor and compacted path are likely one loam composition in different states and visibility contexts. A profile brightness difference of roughly several-fold risks double-counting canopy shadow because Cycles already computes occlusion.

### 20.2 Experimental variants

Render the pinned scenarios under identical geometry and light:

- **A — current:** `meadow_floor` and `compacted_loam` as separate profiles.
- **B — shared profile:** both semantic materials bind one `meadow_loam` profile; compaction, moisture, vegetation, loose material, and cavity carry the difference.
- **C — shared composition plus organic state:** one profile, with an `OrganicMatter` channel for genuinely darker enriched soil where reference evidence supports it.

### 20.3 Measurements

For every variant and seed record:

- median linear RGB of exposed path core;
- median linear RGB of interstitial meadow floor;
- G/R and B/R ratios;
- roughness distribution;
- visible-ground fraction;
- cavity-darkening correlation;
- path/grass edge contrast;
- visual score against references.

### 20.4 Decision rule

Choose a shared profile unless separate profiles demonstrate a composition signal not explainable by state or occlusion—such as stable hue, aggregate-shape, cohesion, or organic-content differences.

Do not retain two profiles whose only substantive distinction is brightness under different cover.

---

# Part V — Hybrid Cycles bridge

## 21. Hybrid scene representation

Extend the active `CyclesScene` without changing the tuned blade buffers.

```rust
pub struct CyclesScene {
    // Existing tuned data, unchanged.
    pub points: Vec<[f32; 3]>,
    pub blade_attributes: ...,
    pub ground: ...,
    pub surface: ...,

    // New secondary data.
    pub secondary_ribbons: Vec<SecondaryRibbonVertex>,
    pub secondary_curves: Vec<SecondaryCurve>,
    pub prototypes: Vec<CyclesPrototype>,
    pub instances: Vec<CyclesInstance>,
    pub secondary_materials: Vec<CyclesMaterialBinding>,

    pub camera: Camera,
    pub settings: RenderSettings,
}
```

A separate `HybridCyclesScene` name is acceptable, but avoid maintaining two almost-identical writers. The active scene format should have one versioned header and optional secondary sections.

## 22. Slice selection

### 22.1 Never regenerate secondary content

`plate::trace` receives the complete `SceneCompilation` or an immutable `SecondarySceneView`:

```rust
pub fn trace(
    request: &PlateRequest,
    params: &GrassParams,
    field: &WorldField,
    compiled: &SceneCompilation,
    progress: &mut dyn FnMut(Progress),
) -> io::Result<Plate>
```

For each trace slice:

1. build the tuned `GrassScene` for the slice using `field`;
2. compute the slice camera-visible world footprint;
3. expand it by secondary geometry and shadow reach;
4. select complete placement groups from `compiled.scene`;
5. lower tuned strokes and selected secondary groups into one `CyclesScene`;
6. render.

### 22.2 Group classification

For each `PlacementAnchor`, compute a conservative group AABB from all of its marks and instances.

Classify:

- `camera_visible` if the group AABB intersects the unexpanded slice frustum/ground footprint;
- `halo_only` if it misses camera visibility but intersects the expanded caster/geometry region;
- omitted otherwise.

All members of one anchor get the same class.

Halo-only objects are present for shadow, diffuse, glossy, and transmission rays as appropriate but invisible to camera rays. This mirrors the existing tuned-blade visible/halo split.

### 22.3 Secondary halo

The compiler already derives recipe reach. The slice selector requires the maximum of:

```text
horizontal primitive reach
projected canopy/flower height reach under the sun
interaction reach is not needed for rendering selection if tuned grass was already generated with it
prototype AABB reach
```

For a maximum object height `H` and sun direction with horizontal magnitude `|s_xy|` and vertical magnitude `s_z>0`, conservative shadow reach is

\[
R_{shadow}=H\frac{|s_{xy}|}{s_z}.
\]

Multiply by a small pinned safety factor such as `1.0625`, following the existing tuned guard convention.

### 22.4 Selection pseudocode

```rust
fn select_secondary(
    scene: &TerrainScene,
    visible: WorldRect,
    halo_m: f64,
) -> SecondarySelection {
    let caster = visible.expanded(halo_m);
    let mut groups = Vec::new();

    for anchor in scene.anchors() {
        let bounds = scene.group_bounds(anchor.index);
        let class = if bounds.intersects_world_rect(visible) {
            VisibilityClass::Camera
        } else if bounds.intersects_world_rect(caster) {
            VisibilityClass::Halo
        } else {
            continue;
        };
        groups.push((anchor.index, class));
    }

    groups.sort_by_key(|(anchor, _)| scene.anchor(*anchor).candidate);
    SecondarySelection { groups }
}
```

## 23. Active scene format version 2

Keep existing tuned files and add explicit optional files.

```text
scene.json
blades.bin                 existing
attributes.bin             existing
ground.bin                 existing
ground-*.bin               existing weight/state planes
secondary-ribbons.bin      new
secondary-curves.bin       new
instances.bin              new
prototypes.json            new or embedded in scene.json
secondary-attributes.bin   new
```

### 23.1 Header sketch

```json
{
  "format": "groundwork-cycles-scene",
  "version": 2,
  "camera": { "...": "..." },
  "settings": { "...": "..." },
  "blades": { "count": 123, "...": "unchanged" },
  "ground": { "...": "unchanged" },
  "secondary": {
    "ribbons": {
      "path": "secondary-ribbons.bin",
      "vertex_count": 0,
      "index_count": 0
    },
    "curves": {
      "path": "secondary-curves.bin",
      "curve_count": 0,
      "point_count": 0
    },
    "prototypes": [
      {
        "key": "stone.rounded.v1",
        "kind": "superellipsoid",
        "parameters": { "...": "..." }
      }
    ],
    "instances": {
      "path": "instances.bin",
      "count": 0
    },
    "materials": [
      { "key": "plant.flower_stem", "shader": "plant.stem" }
    ]
  }
}
```

The exact JSON representation may use the existing serializer conventions. Requirements:

- version is mandatory;
- unknown fields are errors in the Blender reader during development;
- counts and byte strides are explicit;
- every binary file is length-checked before use;
- matrix/ground plane order is recorded;
- all paths are scene-directory relative;
- no implicit Blender defaults decide geometry identity.

## 24. Binary records

### 24.1 Instance record

Use a fixed little-endian record:

```rust
#[repr(C)]
struct InstanceRecordV1 {
    prototype: u32,
    material_variant: u16,
    visibility: u8,       // camera or halo
    flags: u8,
    translation: [f32; 3],
    rotation_xyzw: [f32; 4],
    scale: [f32; 3],
    tint: [f32; 3],
    variation: f32,
}
```

Do not rely on Rust struct memory layout when writing. Write fields explicitly or use a bytemuck-safe, compile-time asserted representation in a crate that permits it. The repository currently forbids unsafe code; explicit byte writing is the safest default.

### 24.2 Curve record

Store curves as offsets into a point table:

```text
CurveHeader: point_offset, point_count, radius0, radius1, material, visibility, attrs
PointTable: x,y,z per point
```

Blender may lower them to:

- bevelled Curve objects for small counts; or
- one combined tube mesh using RMF for predictable performance.

Version 1 recommendation: lower to combined meshes in Python using NumPy and `foreach_set`, matching the existing bulk blade construction style. Avoid thousands of Blender objects for stems.

### 24.3 Ribbon record

Store already-tessellated positions, normals, UV/along-width coordinates, material index, and visibility class, or store compact semantic ribbon parameters and tessellate in Python.

For determinism and a future non-Blender consumer, prefer tessellation in Rust. Python should transfer explicit vertices, not reinterpret plant morphology.

## 25. Blender lowering

### 25.1 General rule

Blender is a deterministic geometry consumer and path tracer. It does not own terrain semantics.

### 25.2 Prototypes

For each prototype binding:

1. validate its version and parameters;
2. build one mesh datablock;
3. assign the prototype material slots;
4. cache by prototype table index;
5. create linked objects for each explicit instance.

Linked duplicates share mesh data but retain independent transforms. This is the intended memory model.

### 25.3 Bulk instance creation

Initial implementation can create linked objects in a loop because stone counts are low. Record object creation time separately.

If object count later becomes a bottleneck, the allowed optimisation is a single Geometry Nodes or collection-instance carrier that consumes the explicit instance table. The node graph may instantiate; it may not scatter, randomise, or choose ownership.

### 25.4 Camera and halo visibility

For halo-only objects:

```python
obj.visible_camera = False
obj.visible_shadow = True
obj.visible_diffuse = True
obj.visible_glossy = True
obj.visible_transmission = True
```

Use the exact Blender/Cycles API supported by the pinned Blender 5.2 LTS build and assert it in a smoke test. The semantic requirement is per-ray visibility, not a specific Python property spelling.

### 25.5 Secondary materials

Add shader bindings:

| Appearance | Initial shader behaviour |
| --- | --- |
| `plant.flower_stem` | rough green dielectric, slight longitudinal variation |
| `plant.flower_petal` | thin two-sided diffuse/translucent leaf-like shader, restrained subsurface/transmission |
| `plant.flower_disk` | rough warm central disk with small normal variation |
| `plant.undergrowth_leaf` | two-sided broadleaf with darker underside and ridge response |
| `surface.stone` | rough dielectric, broad regional colour variation, low-frequency normal detail |
| `surface.shell_fragment` | pale rough dielectric, optional layered tint |
| `surface.organic_fragment` | dark rough fibrous/crumb material |

Do not encode placement or random hue in shader noise alone. Per-instance tint/variation attributes must be supplied from Rust so the image and conditioning metadata share the same cause.

### 25.6 Colour space

All authored material colours remain linear RGB until the configured view transform. Instance tint should be multiplicative or a bounded interpolation in linear light, not an sRGB arithmetic operation.

### 25.7 Object grounding

Before rendering, Python asserts for each instance:

- finite transform;
- positive scale;
- valid prototype index;
- world AABB within the scene package's declared generated bounds plus halo;
- visibility class valid;
- material variant valid.

A bad instance fails the render with the source ID in the message. It never silently defaults to the origin.

## 26. Cycles bridge pseudocode

```rust
pub fn build_hybrid_cycles_scene(
    tuned: &GrassScene,
    compiled: &SceneCompilation,
    selection: &SecondarySelection,
    settings: RenderSettings,
) -> CyclesScene {
    let mut out = CyclesScene::build_tuned(
        tuned,
        &compiled.ground,
        settings,
    );

    let mut prototype_map = BTreeMap::new();
    for (anchor, visibility) in &selection.groups {
        for mark in compiled.scene.marks_for_anchor(*anchor) {
            match mark {
                SceneMark::Ribbon(r) => lower_secondary_ribbon(r, *visibility, &mut out),
                SceneMark::Curve(c) => lower_secondary_curve(c, *visibility, &mut out),
                SceneMark::Analytic(a) => lower_supported_analytic(a, *visibility, &mut out),
                SceneMark::Stamp(_) => {
                    // Version 1: error unless an explicit lowering exists.
                    return Err(UnsupportedSecondaryPrimitive::Stamp(mark.id()));
                }
            }
        }

        for instance in compiled.scene.instances_for_anchor(*anchor) {
            let cycles_proto = bind_cycles_prototype(
                instance.prototype,
                &compiled.scene,
                &mut prototype_map,
                &mut out,
            );
            lower_instance(instance, cycles_proto, *visibility, &mut out);
        }
    }

    out.canonicalise_secondary_tables();
    out
}
```

Unsupported secondary primitives are errors. Silently omitting a flower head is the failure this phase is intended to remove.

---

# Part VI — Versioning, identity, and validation

## 27. Version constants

Add or deliberately bump:

```text
COMPILER_VERSION
DOMAIN_ALGORITHM_VERSION
GROUND_EVALUATOR_VERSION              if final surface semantics change
TUNED_CONTROL_VERSION                 new
INTERACTION_FIELD_VERSION             new
PROTOTYPE_ALGORITHM_VERSION           new
CYCLES_SCENE_FORMAT_VERSION           new, value 2
FLOWER_RECIPE_VERSION
STONE_RECIPE_VERSION
UNDERGROWTH_RECIPE_VERSION
```

Do not bump `SEED_ALGORITHM_VERSION` merely because new named streams are added. Existing addressed values remain stable precisely because stream names isolate decisions.

Bump the seed algorithm only if the address mixer or field ordering changes.

## 28. Fingerprint coverage

### 28.1 Terrain scene fingerprint

The scene fingerprint must absorb, in canonical order:

- scene request/generated bounds;
- ground surface fingerprint;
- material bindings;
- stamp bindings;
- placement anchors;
- prototype bindings;
- marks;
- instances;
- interaction primitives;
- compiler version;
- recipe keys and versions that contributed.

### 28.2 Final render identity

The final Cycles package digest must include:

```text
document digest
root seed
field-stack fingerprint
scene fingerprint
tuned-control fingerprint
interaction-field fingerprint
GroundEvaluator/BandSplit version
GrassParams geometry-affecting fields
render profile geometry-affecting fields
Cycles scene format version
```

It must exclude execution preferences that do not alter content:

```text
thread count
progress reporting
output directory
temporary filename
CPU versus GPU, if expected to render the same scene
```

Sampling, denoising, and device may belong to a **render profile digest** because they alter pixels, but not to the semantic scene fingerprint.

### 28.3 Prototype fingerprint

A prototype binding fingerprint includes:

- semantic key;
- algorithm version;
- family;
- base dimensions;
- superellipsoid exponents;
- deformation coefficients;
- clipping planes;
- tessellation tier if it changes geometry;
- material slot layout.

Two prototypes with the same semantic key but different geometry parameters are a validation error, not “last one wins.”

## 29. Validation rules

Add collected diagnostics for:

- duplicate tuned-pass claim;
- tuned recipe with unsupported parameters;
- secondary recipe whose emitted appearance is unbound;
- prototype key collision with unequal binding;
- instance with invalid prototype index;
- interaction source that does not exist;
- interaction ellipse with nonpositive axis;
- response reach below zero;
- candidate radius range invalid or non-finite;
- clearance below zero;
- domain claimants disagreeing about one shared domain definition;
- secondary primitive unsupported by active Cycles lowering;
- deferred recipe declared in a production compile, reported as a note or warning according to policy;
- flower petal-count bounds invalid;
- undergrowth leaf-count bounds invalid;
- a root or AABB containing non-finite coordinates;
- a prototype or mark whose conservative reach exceeds the recipe's declared maximum.

### Reach self-check

Every recipe should have a debug/test method that computes actual emitted bounds for a sweep of latent extremes. Assert:

\[
\operatorname{actual\_reach}\le\operatorname{declared\_maximum\_reach}+\epsilon.
\]

This closes the existing unverified stroke-reach gap for new content and should later be applied to tuned strokes.

## 30. Scene-format validation

Before Blender runs, Rust validates:

- every binary byte length equals count × stride;
- every index is in range;
- every float is finite;
- every quaternion is near unit length or is normalised deterministically;
- every scale is positive;
- camera/halo visibility values are known;
- every path is relative and contained;
- prototype and material tables are canonical;
- scene header version is supported.

Blender repeats cheap range checks. A corrupted package must fail before producing a plausible but wrong image.

---
# Part VII — Repository implementation plan

## 31. Crate-by-crate changes

### 31.1 `terrain_core`

Expected changes are small.

- No new randomness API is required; use existing named streams and child paths.
- No seed/digest merger is permitted.
- Add reusable validation helpers only if they remain renderer-independent.
- If prototype assets become authored files later, their semantic profile belongs in a versioned core type and their parser remains in `terrain_format`. Version 1 may keep procedural prototype bindings in generator code.

### 31.2 `terrain_scene`

Files likely affected:

```text
src/scene.rs
src/mark.rs
src/instance.rs
src/validate.rs or equivalent scene validation
src/lib.rs
```

Required work:

1. Add `PlacementAnchor` and `AnchorIndex`.
2. Add an anchor reference to every mark and instance.
3. Add a canonical prototype table to `TerrainScene`.
4. Add `SceneBuilder::bind_prototype`.
5. Add `InteractionPrimitive` and `InteractionShape`.
6. Add group-bound construction.
7. Add iterators `marks_for_anchor`, `instances_for_anchor`, `interactions_for_anchor`.
8. Absorb all new semantic fields into the scene fingerprint.
9. Validate prototype, anchor, and interaction references.
10. Preserve stable painter order for ribbons while making anchor selection independent of painter order.

#### Suggested types

```rust
pub struct AnchorIndex(pub u32);

pub struct Anchored<T> {
    pub anchor: AnchorIndex,
    pub value: T,
}

pub struct PrototypeBinding {
    pub key: PrototypeKey,
    pub algorithm_version: u32,
    pub family: PrototypeFamily,
    pub parameters: PrototypeParameters,
    pub materials: Vec<SceneMaterialIndex>,
    pub local_bounds: Aabb3,
}
```

Do not identify prototypes by load order or filename sort. The persistent identity is the authored/registered key plus semantic parameters.

### 31.3 `terrain_generators::domain`

Required work:

1. Replace `SpacingPolicy::Exclusion { max_radius_m }` with the versioned variable-radius form.
2. Add candidate `footprint_radius_m`.
3. Address the radius from its own named stream.
4. Replace rank-only priority tie break with a complete total key.
5. Expand working bounds by the proven maximum conflict distance.
6. Generalise bucket-neighbour range.
7. Extend reports with:
   - raw candidates;
   - candidates after spacing/thinning;
   - offered density;
   - final accepted density.
8. Bump `DOMAIN_ALGORITHM_VERSION` because the candidate set changes.

#### Backward mapping

For existing fixed-centre-distance domains, migration must preserve the old meaning rather than reinterpret the old number as a physical object radius.

Use two explicit policies if needed:

```rust
PriorityDistance { minimum_centre_distance_m }
PriorityFootprints { radius_policy, clearance_m }
```

Then:

- existing grass/flower domains can retain `PriorityDistance` during migration;
- stones use `PriorityFootprints`;
- a later measured change can move flowers to physical footprint semantics.

This avoids silently doubling all existing fixed exclusion distances.

### 31.4 `terrain_generators::recipe`

Required work:

- add `RecipeRenderClass`;
- replace mark-only output with `RecipeEmission`;
- give emissions an anchor/group identity;
- expose prototype bindings;
- make every recipe version explicit;
- add maximum-height as well as maximum-horizontal-reach if slice shadow reach needs it.

Suggested trait additions:

```rust
fn render_class(&self) -> RecipeRenderClass;
fn maximum_height_m(&self, parameters: &ParameterObject) -> f64;
fn prototype_bindings(&self, parameters: &ParameterObject) -> Vec<PrototypeBinding>;
```

Prototype bindings may instead be emitted lazily, but all equal bindings must canonicalise to one table entry.

### 31.5 `terrain_generators::compiler`

This is the centre of the phase.

Required refactor:

1. resolve populations and render classes;
2. validate one claimant per tuned pass;
3. compute halo from:
   - source reach;
   - derived-field reach;
   - candidate conflict reach;
   - recipe geometry reach;
   - recipe maximum height projected through the sun when relevant;
4. sample and derive fields once;
5. construct one `GroundEvaluator`;
6. compile tuned controls;
7. generate only secondary/deferred domains as appropriate;
8. sample one final ground state per accepted candidate;
9. emit secondary groups at final surface height;
10. canonicalise scene;
11. build interactions from the final accepted scene;
12. return all shared products.

#### Important optimisation

Tuned populations do not need generic candidate generation merely to control an existing tuned pass. Compile their affinity/channel/density evaluator directly. This avoids generating millions of generic fine-grass candidates that will never render.

Shared domains involving both tuned and secondary recipes are forbidden in version 1 unless an explicit semantic is designed. Report a diagnostic rather than quietly changing acceptance counts.

### 31.6 `terrain_generators::ground`

Required work:

- expose final geometry height;
- expose one sampled ground record that includes realised substrates, geometry displacement, cavity, state, and final normal as needed;
- make it cheap to reuse a sample inside the compiler;
- include a version constant if the final-height composition is newly canonical;
- add a method to build the final `GroundSurface` lattice from the evaluator for the future generic package path;
- introduce the fingerprinted `GroundReliefPlan` and include bands, ripples, and cracks;
- factor the addressed unit-band basis, monotonic aggregate transform, cluster field, and state scale into reusable Rust functions;
- sample `GroundBumpField` and `micro_slope_rms` from the same evaluator;
- eliminate the profile-level constant `micro_roughness` heuristic after the calibrated transfer is available;
- add forced-tier debug/test hooks that cannot reach production documents accidentally.

### 31.7 `terrain_generators::field`

Required work:

1. extend `SemanticOverlay`:

```rust
pub struct SemanticOverlay {
    pub ground: Arc<GroundEvaluator>,
    pub tuned: Arc<TunedPopulationSet>,
    pub interactions: Arc<InteractionField>,
}
```

2. implement one `semantic_bare(world)` method;
3. use it in both `sample` and `exposed_share`;
4. expose `population_factor(pass, world)`;
5. expose `interaction(pass, world)`;
6. keep style fields untouched.

### 31.8 `terrain_generators::placement`

Required work:

- thread `TunedPass` into each scatter call;
- multiply existing acceptance by the compiled pass factor;
- query interactions only after the cheap `reaches_page` test and before expensive stroke emission;
- reject blocked roots;
- morph latent geometry by the interaction sample;
- retain every existing named/random stream; add new streams without changing old draw ordering where `Draw` remains sequential within a cell;
- ideally migrate obstacle-response attributes to addressed names rather than extra positional `Draw` calls.

#### Sequential `Draw` caution

The tuned generator's existing `Draw` is cell-addressed but uses sequential draws within a pass. Inserting a new draw before existing morphology changes downstream attributes for every plant in that pass.

For interaction response, do not consume new draws. It is a deterministic field transform. For any new random parameter, either:

- append it after all existing draws and accept a versioned tuned-look change; or
- introduce named substreams for the new decision without shifting existing draws.

The second is preferred where the API permits it.

### 31.9 `terrain_generators::families`

Required work:

- mark current grass/fine/thatch families as `Tuned`;
- add semantic `MeadowBroadleaf` recipe with `Tuned(Broadleaf)` and no generic production emission;
- replace flower ellipsoid head with curved stem + prototype head or explicit petals;
- replace analytic stone with prototype instance + interaction emission;
- add `MeadowUndergrowth` secondary recipe;
- mark `DirtClods` deferred;
- add recipe-specific versions;
- add exact validation and maximum-reach calculations.

### 31.10 `terrain_cycles`

Files likely affected:

```text
src/cycles.rs
src/plate.rs
src/package.rs
src/export.rs
src/lib.rs
```

Required work:

1. add active scene format version 2;
2. keep tuned blade writer byte-compatible inside v2;
3. add secondary buffers and prototype table;
4. add group-based slice selection;
5. accept `SceneCompilation` in `plate::trace`;
6. lower secondary ribbons, curves, and instances;
7. validate unsupported primitives as errors;
8. include secondary geometry in package/render digests;
9. preserve halo-only ray visibility;
10. add debug counts per primitive and prototype;
11. export `ground_relief_plan.json`, the Rust-authored bump plane, fine cavity, and micro-slope field;
12. record exact field transforms, anchors, checksums, and any budget-driven tier reclassification;
13. replace the constant per-profile micro-roughness calculation with the calibrated, state-dependent field;
14. expose ground benchmark AOVs without changing the production beauty path when debug passes are disabled.

`write_package` should not be deleted. Reuse compatible manifest concepts where useful, but do not route production through incomplete lowering merely to avoid a temporary second schema.

### 31.11 `tools/blender_cycles/render.py`

Required work:

- parse format v2 and retain v1 only if backward compatibility is valuable;
- bulk-create secondary ribbon and curve meshes;
- generate prototype meshes deterministically;
- create linked explicit instances;
- bind per-instance attributes;
- configure camera/halo ray visibility;
- add materials;
- emit meaningful source IDs in failures;
- verify counts and byte lengths;
- remove procedural Blender Noise and the Python folded-ridge function from active ground relief;
- load the Rust-authored float bump field in bulk and map its integer world lattice to texel centres exactly;
- read `compaction`, `wet_film`, fine cavity, and micro-slope attributes/fields;
- drive Principled Coat Weight from `wet_film`, with profile coat roughness and film IOR;
- expose ground debug AOVs and validate that all claimed fields are connected;
- produce optional debug collections:
  - `Groundwork/TunedGrass`;
  - `Groundwork/Flowers`;
  - `Groundwork/Stones`;
  - `Groundwork/Undergrowth`;
  - `Groundwork/Halo`.

Debug collections are organisational only and must not change rendering.

### 31.12 `terrain_cli`

Required work:

- stop reconstructing `GroundEvaluator` after `compile_scene`;
- create `SemanticOverlay` from `compiled.ground`, `compiled.tuned`, and `compiled.interactions`;
- pass `compiled` to `plate::trace`;
- print secondary counts and prototype counts;
- show deferred populations clearly;
- add optional debug commands or flags:

```text
terrain inspect <doc> --at U,V --tuned
terrain inspect <doc> --at U,V --interaction
terrain compile <doc> --secondary-only
terrain compile <doc> --no-secondary
terrain compile <doc> --debug-aov population_id
```

Debug flags may alter output intentionally and must reach a debug render profile digest.

### 31.13 `terrain_bench`

The supplied snapshot does not include this crate even though the workspace and CLI depend on it. Recover it from repository history or recreate it first. The restored/new crate must provide at least:

```text
src/fixtures.rs
src/scenarios.rs
src/ground/{mod,topography,semivariogram,psd,cracks,ripples,optics}.rs
src/render_metrics.rs
src/report.rs
src/baseline.rs
```

Add meadow scenarios:

```text
meadow_secondary_empty
meadow_flowers_sparse
meadow_flowers_clustered
meadow_stones_sparse
meadow_stones_large_radius_mix
meadow_stone_grass_interaction
meadow_undergrowth_sparse
meadow_all_content
meadow_path_all_content
```

Add every ground laboratory listed in §38.2, the resolution ladder, state sweeps, the high-resolution analytic oracle, and committed report comparison. Append scenarios; do not repurpose existing baseline names.

### 31.14 Assets

Add:

```text
assets/terrain/documents/meadow_full.terrain.ron
assets/terrain/documents/stone_interaction_lab.terrain.ron
assets/terrain/documents/flower_lab.terrain.ron
assets/terrain/documents/undergrowth_lab.terrain.ron
assets/terrain/materials/meadow_loam.ground.ron       experiment candidate
```

The laboratory documents isolate one hard feature at a time. `meadow_full` proves composition.

---

## 32. Example document additions

A representative extension to `meadow_path.terrain.ron`:

```ron
(
    modifier_channels: [
        // existing channels...
        (
            key: "broadleaf_abundance",
            display_name: "Broadleaf abundance",
            range: (0.0, 1.5),
            default_value: 0.45,
            composition: "Max",
            unit: "Unitless",
        ),
        (
            key: "undergrowth_abundance",
            display_name: "Undergrowth abundance",
            range: (0.0, 1.5),
            default_value: 0.18,
            composition: "Max",
            unit: "Unitless",
        ),
    ],

    populations: [
        // existing tuned populations...
        (
            key: "meadow_broadleaf",
            recipe: "population.meadow_broadleaf",
            seed_stream: "broadleaf",
            material_affinity: [
                ("meadow_soil", 1.0),
                ("dirt_compacted", 0.05),
            ],
            abundance_channel: Some("broadleaf_abundance"),
            parameters: [
                ("density", Number(4.0)),
            ],
        ),
        (
            key: "meadow_undergrowth",
            recipe: "population.meadow_undergrowth",
            seed_stream: "undergrowth",
            material_affinity: [
                ("meadow_soil", 1.0),
                ("dirt_compacted", 0.08),
            ],
            abundance_channel: Some("undergrowth_abundance"),
            parameters: [
                ("density", Number(1.8)),
                ("leaf_length_m", Number(0.12)),
                ("leaf_width_m", Number(0.035)),
                ("min_leaves", Integer(3)),
                ("max_leaves", Integer(7)),
            ],
        ),
    ],
)
```

Exact syntax must follow the current raw-format schema. The example communicates semantic intent, not permission to add unvalidated free-form parameters.

---

# Part VIII — Tests and proofs

## 33. Unit and property tests

### 33.1 Variable-radius domain tests

Add at minimum:

```text
variable_footprints_never_overlap
variable_exclusion_is_independent_of_input_order
whole_window_equals_two_half_windows
whole_window_equals_four_quadrants
priority_ties_have_exactly_one_winner
negative_world_cells_match_positive_world_rules
lower_density_is_a_subset_of_higher_density
changing_radius_stream_moves_only_the_versioned_domain
working_halo_contains_every_possible_conflict
bucket_query_matches_brute_force
```

#### Brute-force oracle

For small candidate sets, compare bucketed thinning with `O(n²)` all-pairs thinning using the same total priority rule. This is the strongest defence against a missed bucket-range case.

### 33.2 Interaction-field tests

```text
ellipse_clearance_is_zero_on_axes_boundary
ellipse_clearance_sign_is_correct
rotating_ellipse_rotates_normal
centre_uses_stable_fallback_direction
influence_is_one_at_hard_boundary
influence_is_zero_at_outer_boundary
influence_derivative_is_zero_at_endpoints
bucket_query_matches_brute_force
nearest_boundary_wins_direction_ties_stably
outside_all_reaches_returns_exact_none
whole_field_equals_sliced_fields
```

### 33.3 Final-surface tests

```text
recipe_root_equals_ground_evaluator_final_surface
stone_burial_is_measured_from_final_surface
changing_shader_only_bump_does_not_move_roots
changing_geometry_band_moves_roots_and_scene_fingerprint
compiler_and_cli_share_the_same_arc_evaluator
```

The last test may use `Arc::ptr_eq` in an integration harness.

### 33.4 Tuned-control tests

```text
one_population_per_tuned_pass_is_enforced
zero_tuft_control_removes_tufts_only
zero_fine_control_removes_fine_only
zero_thatch_control_removes_thatch_only
zero_broadleaf_control_removes_broadleaf_only
material_affinity_is_a_veto
abundance_channel_modulates_only_its_pass
target_density_scaling_is_monotone
pass_factor_uses_realised_not_raw_substrate
```

### 33.5 Bareness tests

```text
abundance_one_preserves_previous_semantic_bare
abundance_zero_exposes_supported_soil
abundance_above_one_does_not_make_negative_bare
support_zero_is_fully_semantically_bare
sample_and_exposed_share_use_identical_mapping
semantic_bare_is_monotone_in_abundance
```

A committed before/after scene fingerprint is expected because this is a deliberate look change.

### 33.6 Flower geometry tests

```text
zero_bend_stem_is_vertical
stem_arc_has_requested_length
stem_endpoint_matches_closed_form
stem_tip_tangent_matches_closed_form
rmf_axes_remain_orthonormal
rmf_has_no_flip_across_low_curvature
petal_whorl_has_stable_count_and_order
petal_width_is_zero_at_root_and_tip
flower_group_bounds_contain_stem_and_head
trace_selection_never_splits_flower_group
```

Numerically verify arc length by summing a high-resolution polyline and comparing with `L` within tolerance.

### 33.7 Prototype and instance tests

```text
prototype_binding_is_idempotent
unequal_binding_under_same_key_is_error
superellipsoid_is_finite_at_poles
prototype_bounds_contain_all_vertices
prototype_fingerprint_moves_with_each_parameter
instance_transform_round_trips_binary
burial_keeps_declared_fraction_below_surface
conservative_ellipse_contains_projected_base
linked_instances_share_prototype_identity
```

### 33.8 Hybrid Cycles tests

```text
v2_with_empty_secondary_matches_v1_tuned_geometry
secondary_counts_round_trip_header_and_buffers
unsupported_stamp_is_reported
camera_group_and_halo_group_are_distinct
halo_group_is_camera_invisible_and_shadow_visible
whole_secondary_selection_equals_union_of_slices
secondary_scene_is_not_regenerated_per_slice
prototype_table_is_written_once
```

The first test should compare geometry fingerprints and, where practical, a low-sample image baseline. Format metadata may differ; tuned geometry must not.

### 33.9 Regression test against generic-grass duplication

Compile a document containing all current grass populations and secondary content. Assert:

- no generic grass/fine/thatch mark reaches secondary lowering;
- tuned stroke counts remain within committed baseline tolerance;
- secondary primitive count contains flowers/stones/undergrowth only;
- the render-class report states exactly which path owns each population.

## 34. Cross-window determinism harness

Build a reusable test helper:

```rust
fn assert_composable<T: StableId + Eq + Ord>(
    whole_bounds: WorldRect,
    partitions: &[WorldRect],
    compile: impl Fn(WorldRect) -> Vec<T>,
) {
    let mut whole = compile(whole_bounds);
    whole.sort();

    let mut parts = partitions.iter()
        .flat_map(|b| compile(*b))
        .collect::<Vec<_>>();
    parts.sort();
    parts.dedup();

    assert_eq!(whole, parts);
}
```

Run it for:

- domain candidates after spacing;
- accepted candidate IDs;
- flower anchor IDs;
- stone instance IDs and transforms;
- undergrowth anchor IDs;
- interaction primitive IDs and parameters;
- selected secondary IDs across trace slices.

Compare semantic records, not only IDs. An ID that stays while its scale changes across windows is still a seam bug.

## 35. Locality harness for stone interactions

Compile with and without stones using the same seed. For every tuned latent root:

- outside every stone response reach, the complete stroke record must be bit-identical;
- inside a response reach, candidate address and all pre-interaction latent parameters must be equal;
- only the documented response fields may differ;
- roots inside hard footprints are absent from final strokes but remain present in an optional debug latent-candidate record.

This test proves local causality rather than merely visual plausibility.

## 36. Visual baselines

For each committed scenario and seed, store:

- beauty image;
- albedo AOV;
- normal AOV;
- depth AOV;
- material/appearance ID AOV;
- population ID AOV;
- interaction influence debug AOV;
- anchor/group ID debug image where feasible;
- manifest with counts and fingerprints.

Do not update all baselines because “the new ones look okay.” Every accepted baseline move must name which algorithm/version caused it.

---

# Part IX — Measurement and calibration

## 37. Quantitative metrics

### 37.1 Composability

```text
candidate_id_symmetric_difference      must be 0
secondary_record_symmetric_difference must be 0
max_join_pixel_error                   target 0 before denoise/crop tolerance
mean_join_pixel_error                  target 0
shadow_join_error                      target 0
```

If Cycles stochastic sampling produces pixel noise across independently rendered overlaps, compare deterministic scene/AOV data first and use matched sampling seeds for image joins.

### 37.2 Interaction correctness

```text
roots_inside_hard_footprints           0
outside_reach_changed_strokes          0
mean outward alignment for influenced tuft directions > 0
response discontinuity at outer band   approximately 0
```

For influenced stroke horizontal direction `v` and nearest-stone outward normal `n`, report:

\[
\operatorname{alignment}=v\cdot n.
\]

This is a diagnostic, not a target to maximise to one; forcing all grass radially outward would create a visible ring.

### 37.3 Grounding

For each instance, compute lowest visible prototype point after transform relative to final surface samples under its footprint. Report:

```text
floating_instance_count
fully_buried_instance_count
median visible height fraction
median burial fraction
```

Acceptance begins with zero obvious floaters and no systematic full burial. Exact geometry thresholds are calibrated by prototype.

### 37.4 Distribution

Per population:

```text
offered density
post-spacing density
accepted density
owned density
mean nearest-neighbour distance
5th percentile nearest-neighbour distance
radius distribution
prototype histogram
scale histogram
```

This catches a “working” variable-radius algorithm that accidentally suppresses all large stones or concentrates one prototype.

### 37.5 Visual quality counters

Retain existing metrics and add:

```text
visible ground fraction
secondary silhouette pixel fraction
flower-head connected-component count
stone contact-shadow fraction
population palette drift
secondary detail energy by spatial band
weakest-seed score
```

No single metric decides acceptance. They make visual changes legible.

## 38. Ground/soil benchmark programme

This programme is a release gate for the entire soil/ground stack. It answers six different questions that must not be collapsed into one beauty score:

1. **Did the mathematical surface match the authored profile?**
2. **Did representation tiering preserve the same surface signal?**
3. **Did material state behave physically and monotonically?**
4. **Did independent windows and trace slices agree?**
5. **Did the Cycles image preserve the causal AOVs?**
6. **Did a speed-up preserve all of the above?**

A visual review remains necessary, but it happens after the structural and quantitative failures have been removed.

### 38.1 Benchmark harness architecture

The benchmark is split into four layers:

```text
GroundEvaluator / analytic field
    ↓ exact sampled height and state
Topography analysis
    ↓ morphology, PSD, semivariogram, scale metrics
Cycles laboratory renders + AOVs
    ↓ optics and perceptual comparison
Performance recorder
    ↓ stage time, memory, counts, checksums
```

The topography half runs without Blender and must be fast enough for CI on the compact scenario set. The render half runs in the full visual gate and on baseline-acceptance jobs.

The supplied snapshot references `terrain_bench` without including it. The first benchmark task is therefore:

```text
recover existing crates/terrain_bench from source control
OR
create crates/terrain_bench with fixtures, metrics, report, baseline, and CLI modules
```

The CLI contract:

```text
terrain benchmark ground <document>
    --scenario <name|all>
    --seeds committed|<hex,...>
    --render-profile <profile>
    --resolution-ladder
    --state-sweeps
    --out target/ground-bench/<run-id>
    --compare benchmarks/ground/<baseline-id>
    --json
```

### 38.2 Pinned laboratory scenarios

Append, never repurpose, these isolated scenarios:

```text
ground_flat_card
ground_band_coarse_only
ground_band_crumb_only
ground_band_grain_only
ground_band_full_ladder
ground_cluster_sweep
ground_compaction_sweep
ground_moisture_sweep
ground_wet_film_hollow_crown
ground_crack_primary
ground_crack_hierarchy
ground_ripple_isotropic_control
ground_ripple_directional
ground_material_blend_cross_section
ground_resolution_ladder
ground_profile_ablation
ground_meadow_path_context
```

Each laboratory uses a flat authored elevation unless the tested feature requires otherwise. This keeps macro slope and the tuned grass mound field from contaminating soil measurements.

Every scenario records:

- document/profile/render digests;
- seed;
- world bounds and lattice origin;
- mesh, bump, and pixel spacing;
- complete `GroundReliefPlan`;
- raw height/state/material planes;
- beauty and required AOVs when rendered.

### 38.3 Detrending and valid measurement support

Do not compute roughness on a tilted or curved plate. For sampled points `(ui,vi,zi)`, fit the least-squares plane

\[
(a,b,c)=\arg\min_{a,b,c}\sum_i
[z_i-(a u_i+b v_i+c)]^2
\]

and analyse residuals

\[
z'_i=z_i-(a u_i+b v_i+c).
\]

For a scenario with intentional directional macroform, remove the declared macro component rather than fitting it away blindly.

Exclude a margin at least as large as:

```text
max(coarsest analysed wavelength,
    crack/ripple support,
    filter kernel reach,
    one bump texel)
```

from scalar statistics. Alternatively use a committed periodic laboratory. Never let clamped field borders enter a roughness measurement.

### 38.4 Scalar height and slope metrics

For every analysed height field report:

\[
S_a=\frac1N\sum_i|z'_i|,
\qquad
S_q=\sqrt{\frac1N\sum_i z_i'^2},
\]

\[
S_{sk}=\frac{E[z'^3]}{S_q^3+\epsilon},
\qquad
S_{ku}=\frac{E[z'^4]}{S_q^4+\epsilon}.
\]

Also report:

- minimum, maximum, and robust 1/5/50/95/99 percentiles;
- peak-to-trough range;
- RMS gradient `Sdq` from centred finite differences;
- positive/negative area fraction;
- cavity/height Pearson and Spearman correlation;
- profile-declared amplitude versus measured amplitude.

`Sq` is the renderer's equivalent of detrended random roughness. It is necessary and insufficient: two surfaces can share `Sq` and have completely different feature scales, which is why the spectral and spatial metrics below are mandatory.

### 38.5 Scale-dependent roughness

For displacement vector `ℓ`, compute the height-difference function

\[
H(\ell)=\sqrt{\frac12 E[(z'(x+\ell)-z'(x))^2]},
\]

the scale-dependent RMS slope

\[
S(\ell)=\frac{\sqrt{E[(z'(x+\ell)-z'(x))^2]}}{\lVert\ell\rVert},
\]

and second-difference curvature

\[
C(\ell)=\frac{
\sqrt{E[(z'(x+\ell)-2z'(x)+z'(x-\ell))^2]}
}{\lVert\ell\rVert^2}.
\]

Evaluate on logarithmically spaced scales and in at least the `u`, `v`, and two diagonal directions. These curves reveal whether a supposedly five-centimetre clod band secretly contains two-and-a-half-centimetre detail, whether a lattice grid is leaking through, and whether state flattening affects the intended scales.

### 38.6 Semivariogram and autocorrelation

For lag vector `h`, the experimental semivariogram is

\[
\gamma(h)=\frac{1}{2|P_h|}
\sum_{(i,j)\in P_h}(z'_j-z'_i)^2.
\]

Report omnidirectional and directional semivariograms, with fitted or directly measured:

- nugget;
- sill;
- practical range;
- first zero/`1/e` autocorrelation length;
- anisotropy ratio and direction.

The range should track the declared feature wavelength order of magnitude. A large nugget in a deterministic analytic field usually indicates aliasing, discontinuity, or a bad tier handoff rather than real stochastic microscale variation.

### 38.7 Two-dimensional power spectral density

Use the detrended field multiplied by a separable Hann window. For an unnormalised `Nx×Ny` discrete Fourier transform `F`, texel spacing `Δu,Δv`, and window-power normaliser

\[
U=\frac{1}{N_xN_y}\sum_{n,m}w_{n,m}^2,
\]

define

\[
PSD(k_u,k_v)=
\frac{\Delta u\Delta v}{N_xN_y\,U}|F(k_u,k_v)|^2.
\]

With frequency-bin areas `Δfu=1/(NxΔu)` and `Δfv=1/(NyΔv)`, the discrete integral of this PSD approximates the detrended height variance. The implementation test must verify Parseval consistency:

\[
\left|\sum PSD\,\Delta f_u\Delta f_v-S_q^2\right|
\le \epsilon_{fft}.
\]

Produce:

- 2-D log-PSD image;
- radial log-binned spectrum;
- per-authored-band integrated energy;
- dominant frequency and wavelength;
- out-of-band leakage;
- axis-aligned grid energy;
- high-frequency alias energy above the representable cutoff.

For authored band `b` with expected neighbourhood `Kb`,

\[
E_b=\sum_{k\in K_b}PSD(k)\Delta f_u\Delta f_v.
\]

Compare `Eb` across geometry, bump, high-resolution analytic reference, and state sweeps. It should move between representations, not appear twice or disappear.

### 38.8 Direction and anisotropy metrics

From the 2-D PSD, form the normalised spectral orientation tensor

\[
M=\frac1E\sum_{k\ne0}PSD(k)
\frac{1}{\lVert k\rVert^2}
\begin{bmatrix}
k_u^2 & k_uk_v\\
k_uk_v & k_v^2
\end{bmatrix}\Delta k.
\]

Let eigenvalues be `λ1≥λ2`. Report

\[
A=\frac{\lambda_1-\lambda_2}{\lambda_1+\lambda_2+\epsilon}
\]

and the principal wave-vector direction. For ripples, the crest direction is perpendicular to that wave vector. The directional ripple laboratory additionally measures:

- dominant wavelength error;
- direction error in degrees modulo `π`;
- crest/lee slope asymmetry;
- phase-meander RMS and correlation length;
- wetness suppression of directional energy.

The isotropic control should have low `A`; the ripple case should have high `A` at its declared frequency and not at unrelated bands.

### 38.9 Crack-network metrics

Measure cracks from the analytic crack signed-distance/depth field, not from thresholded beauty pixels.

Create a binary crack mask at a declared depth fraction, then report:

- crack area fraction;
- Euclidean-distance-transform width median and 5/95 percentiles;
- skeleton length per square metre;
- endpoint and junction density;
- connected-component count;
- enclosed polygon equivalent-diameter distribution;
- primary/secondary skeleton-length ratio;
- depth distribution;
- curl-shoulder positive-height ring and width.

The primary polygon diameter should track `polygon_m`, the width distribution should track `width_m`, and secondary branches must terminate at or merge into primary cracks rather than crossing them as an independent Voronoi overlay.

### 38.10 State-response sweeps

For each relief band, sweep compaction and moisture over a committed grid, initially:

```text
compaction = 0.0, 0.25, 0.5, 0.75, 1.0
moisture   = 0.0, 0.25, 0.5, 0.75, 1.0
```

The predicted amplitude multiplier is

\[
q_b(c,m)=(1-r_b c)(1-f_w m)
\]

before clustering. Therefore, relative to the dry loose control:

\[
\frac{S_q(c,m)}{S_q(0,0)}\approx q_b(c,m),
\qquad
\frac{E_b(c,m)}{E_b(0,0)}\approx q_b(c,m)^2.
\]

Test the measured ratios within a tolerance established by the finite window and material blending. Coarse clods must flatten more under compaction than grain when their profile responses say so. No state sweep may increase a band whose response is purely flattening.

For microfacet bands, perform the same check against `micro_slope_rms` and the fitted roughness transfer.

### 38.11 Material colour and wet response

Use a colour/albedo AOV under a neutral colour-management transform, not beauty pixels under variable lighting. For each profile/state report:

- linear-RGB mean, median, and percentiles;
- linear luminance;
- `G/R` and `B/R` ratios;
- CIE Lab median after a declared display/XYZ transform;
- CIEDE2000 distance between variants;
- saturation and hue shifts;
- cavity-to-luminance correlation.

The moisture sweep tests:

- every channel remains finite and non-negative;
- the declared dry mid and wet mid are hit at their endpoints;
- reflectance darkening is monotone through the calibrated range unless a later physically justified saturation model explicitly changes that contract;
- red survives relative to blue according to the profile rather than all channels receiving one grey multiplier;
- colour change comes from moisture, not `wet_film`.

Soil reflectance measurements show a nonlinear moisture relationship and saturation behaviour; the purpose here is not to fit remote-sensing spectra, but to reject a linear grey dimmer masquerading as wet soil.

### 38.12 Wet-film and BRDF laboratory

Render a flat card and a controlled shallow hollow with:

```text
fixed base profile and moisture;
wet_film = 0.0, 0.25, 0.5, 0.75, 1.0;
view/light angles spanning normal and grazing incidence;
coat AOV or glossy-direct/indirect passes where available.
```

Required checks:

- Coat Weight follows `wet_film` monotonically and is zero at zero film;
- base albedo is unchanged when only `wet_film` changes;
- coat IOR and roughness come from the profile;
- specular lobe energy grows with film while the base layer remains energy-conserving;
- the hollow carries more derived film than an otherwise equal crown;
- film does not move geometry or bump.

The beauty comparison includes highlight width, peak luminance, and integrated glossy energy over a region of interest.

### 38.13 Cross-tier identity tests

For every canonical band:

1. evaluate the exact Rust function on a high-resolution reference lattice;
2. force it to geometry and sample the resulting mesh at shared lattice points;
3. force it to bump and read back/export the bump plane;
4. compare values, gradients, PSD, and phase correlation.

At shared analytic lattice nodes, the forced geometry and forced bump source values must be exactly equal on the supported platform before mesh/image interpolation. After interpolation, report:

- maximum and RMS height error;
- gradient angular/error distribution;
- normalised cross-correlation;
- dominant-wavelength error;
- band-energy ratio.

The specific regression that must fail before the coherence fix and pass after it is:

```text
RoundedRidged/Angular band:
Rust monotonic transform == exported bump transform
```

No Blender Noise node or Python ridge function is allowed to participate in this test.

### 38.14 Resolution and representation ladder

Render and analyse a pinned patch at a ladder such as:

```text
trace: 36, 72, 144, 288 px/m
mesh budget: coarse, default, fine
```

For each step record which tier owns every contribution. Compare every lower-resolution result to a properly filtered high-resolution reference at the same final display resolution.

Report:

- tier moves;
- morphology metrics;
- PSD energy by band;
- normal AOV error;
- albedo AOV error;
- beauty FLIP;
- SSIM as a secondary diagnostic;
- temporal/phase stability under sub-texel camera translations if the output will be animated.

A tier transition is accepted only when the representation changes but the terrain fingerprint and causal field remain the same. The render-profile digest moves because representation settings changed; the document and scene placement digests do not.

### 38.15 Material-boundary and blend benchmarks

Use a one-dimensional cross-section and a 2-D ragged boundary with two deliberately different profiles. Measure:

- weight sum error;
- boundary position and width;
- displacement continuity;
- normal discontinuity;
- roughness continuity;
- colour transition;
- relief energy on each side and through the blend;
- cavity haloing;
- whether one profile's band appears on ground where its realised weight is zero.

The one-dimensional diagnostic exports every term as CSV/JSON at fixed world coordinates. The two-dimensional case checks that changing trace bounds does not move the realised boundary or any relief phase.

### 38.16 Seam, crop, and trace-slice invariance

For whole, left/right, top/bottom, and overlapping windows compare:

```text
material weights
elevation and authored microrelief
geometry displacement
bump height
micro_slope_rms
moisture / compaction / wet_film / cavity
crack and ripple identity
final ground mesh samples
```

At shared integer-addressed samples, equality is bit-exact on the supported platform. Rendered overlaps are compared after cropping with:

- raw colour/AOV max and RMS error;
- denoised colour separately;
- a seam-strip FLIP score;
- shadow continuity.

Never use denoised beauty equality as the only seam test; denoisers can hide a causal mismatch or introduce their own crop dependence.

### 38.17 Render-space evaluation

The required ground render passes are:

```text
beauty, raw and denoised
albedo/base colour
normal
position or depth
substrate weights / dominant profile
geometry displacement
bump height
micro_slope_rms
moisture
compaction
wet_film
cavity
roughness
coat weight
crack mask / ripple phase where present
```

For aligned render-to-render comparisons:

- use FLIP as the primary perceptual beauty-difference metric because it is designed for rendered-image differences;
- use SSIM only as a secondary structural summary;
- use CIEDE2000 for controlled colour-patch/AOV comparisons;
- always retain the error map and percentile distribution, not only one pooled number.

A pixelwise metric is invalid against an unregistered reference photograph. Photo references are compared through calibrated crops, colour/roughness distributions, feature scale, PSD, silhouette/coverage statistics, and human review unless camera, geometry, lighting, and colour transform have been matched.

### 38.18 Performance decomposition

Record wall time and peak memory for:

```text
parse / migrate / validate / prepare
field-stack sampling
derived fields
GroundEvaluator construction
ground geometry sampling
bump-field sampling
micro-slope field construction
crack/ripple solving
mesh construction
scene/package serialisation
Blender package parsing
float-image upload
prototype construction
BVH/synchronisation
Cycles trace
AOV and file write
```

Run at least five measured repetitions after one warm-up for CPU stages. Report median, median absolute deviation, and worst or p95 as appropriate. Record cold-cache loading separately where it matters.

The benchmark manifest pins:

- machine and OS;
- CPU/GPU and driver;
- Rust compiler/profile;
- Blender version and build hash;
- Cycles device/backend;
- thread counts;
- sample count, bounce limits, denoiser, and tile settings;
- other competing workloads if any.

No speed claim is valid unless the compared runs have equal document/scene identity, equal accepted-content counts, and ground morphology/optics metrics inside the quality gate. A speed-up caused by reclassifying or losing a relief band is a quality-tier change.

### 38.19 Statistical aggregation and weakest-case policy

Use a committed seed set large enough to expose phase pathologies; begin with at least ten seeds for fast structural metrics and five representative seeds for full Cycles renders. Report:

- mean and median;
- standard deviation or median absolute deviation;
- 5th/95th percentiles where meaningful;
- weakest seed by each critical metric;
- exact seed and crop for every outlier.

Do not average away a worm pattern, grid, seam, or floating-ground failure. Structural invariants are pass/fail per seed. Visual and distributional scores include the weakest seed in the release table.

### 38.20 Initial numerical gates

These are bootstrap thresholds, to be tightened or deliberately revised with baseline evidence:

| Test | Initial gate |
| --- | --- |
| PSD Parseval consistency | relative error ≤ `1e-5` in `f64` analysis |
| Band dominant wavelength | within 5% of high-resolution analytic reference |
| Band integrated energy at tier handoff | ratio `0.95..1.05` |
| Forced geometry/bump source correlation | ≥ `0.995` |
| State RMS-height response | within 5% of predicted `q_b` after finite-window correction |
| State band-energy response | within 10% of predicted `q_b²` |
| Shared addressed ground samples | bit-identical, supported platform |
| Material-weight sum | absolute error ≤ `1e-6` |
| Wet-film coat weight | monotone; zero endpoint exact |
| Out-of-reach interaction/soil change | exact identity |
| Render no-regression | baseline-relative FLIP/AOV gates, scenario-specific |

A threshold may move only with a committed report explaining whether the old threshold was statistically unstable, physically wrong, or intentionally superseded.

### 38.21 Report and artifact schema

Every run writes:

```text
report.json
manifest.json
height_geometry.f32
height_bump.f32
micro_slope_rms.f32
state/*.f32
materials/*.f32
metrics/topography.json
metrics/semivariogram.csv
metrics/psd_radial.csv
metrics/psd_2d.exr or .npy
renders/*.exr
renders/error/*.png
logs/timings.json
```

Core report shape:

```rust
pub struct GroundBenchmarkReport {
    pub schema_version: u32,
    pub run: RunIdentity,
    pub source: SourceIdentity,
    pub scenario: ScenarioIdentity,
    pub relief_plan: ReliefPlanRecord,
    pub counts: GroundCounts,
    pub topography: TopographyMetrics,
    pub spectrum: SpectralMetrics,
    pub cracks: Option<CrackMetrics>,
    pub ripples: Option<RippleMetrics>,
    pub optics: OpticsMetrics,
    pub composability: ComposabilityMetrics,
    pub render: Option<RenderMetrics>,
    pub performance: PerformanceMetrics,
    pub artifacts: Vec<ArtifactRecord>,
    pub verdict: BenchmarkVerdict,
}
```

The companion `ground-benchmark-report.schema.json` supplied with this specification defines the durable interchange shape. Rust types remain the source of truth, and schema/type drift is a test failure.

### 38.22 Baseline and review policy

Ground baselines live under a versioned committed directory, for example:

```text
benchmarks/ground/v1/
    baseline.json
    scenarios/<scenario>/<seed>/report.json
    selected-renders/
```

A substantial ground change includes a generated comparison table with:

```text
metric             old       new       delta      gate       verdict
```

and links every metric regression to an intentional visual/physical decision. Never update the baseline merely because the test says it moved. The benchmark exists to force the question: *why did the ground move?*

---

## 39. Calibration sequence

Calibrate in this order:

1. **Geometry correctness** with flat colours and no material noise.
2. **Scale** against metre references and target camera.
3. **Density** using population ID AOVs.
4. **Clustering** using abundance fields.
5. **Grounding/burial** with side-lit close crops.
6. **Grass response** with interaction AOV.
7. **Materials** in linear light.
8. **Full composition** with tuned grass and path.
9. **Weakest seeds**, not only the prettiest seed.

Changing materials while placement is still wrong hides causal mistakes and wastes renders.

## 40. Initial parameter table

These values are starting ranges for laboratory documents, not universal biological measurements.

### Flowers

| Parameter | Start |
| --- | ---: |
| Density | 4–8 plants/m² in rich patches |
| Stem length | 0.18–0.30 m |
| Stem radius | 0.0015–0.0028 m |
| Total bend | 0.05–0.45 rad |
| Petals | 5–8 |
| Head radius | 0.008–0.016 m |
| Petal length | 0.010–0.025 m |
| Petal width | 0.005–0.012 m |
| Exclusion centre distance | 0.07–0.11 m |

### Stones

| Parameter | Start |
| --- | ---: |
| Offered density | 0.4–1.2/m² depending on abundance mask |
| Major radius | 0.025–0.085 m |
| Axis ratio | 0.55–0.95 |
| Height/major radius | 0.45–1.10 |
| Clearance | 0.005–0.015 m |
| Grass response reach | 0.07–0.16 m |
| Burial | 0.16–0.48 by family |

### Undergrowth

| Parameter | Start |
| --- | ---: |
| Density | 0.5–2.5 clusters/m² |
| Leaves | 3–8 |
| Leaf length | 0.06–0.18 m |
| Leaf width | 0.018–0.055 m |
| Crown radius | 0.01–0.035 m |
| Rise | 0.01–0.04 m |
| Tip droop | 0.005–0.03 m |

Render-scale measurements must decide final defaults.

---

# Part X — Atomic implementation sequence

## 41. Dependency graph

```text
A. baselines and diagnostics
        ↓
B. render-class split and tuned-control model
        ↓
C. evaluator ownership moved into compiler
        ↓
D. anchors + prototypes + interaction scene vocabulary
        ↓
E. Cycles v2 empty-secondary bridge
        ↓
F. flower geometry and lowering
        ↓
G. variable-radius priority exclusion
        ↓
H. stone prototypes and instances
        ↓
I. interaction field + tuned grass response
        ↓
J. semantic bare fix + complete per-pass controls
        ↓
K. undergrowth
        ↓
L. soil experiment, final composition, corpus gate
```

Some branches can be developed in parallel after D, but acceptance should follow this order so each visual move has one cause.

## 42. Pull request / agent task plan

### PR 0 — Pin the baseline

**Goal:** make later changes attributable.

Deliver:

- current `refactor_fingerprints` results;
- current compile report for committed seeds;
- current Cycles beauty/AOV baselines;
- exact test count and command output;
- current tuned stroke counts by pass;
- current secondary scene mark counts, proving they are discarded.

No visual change.

### PR 1 — Render classes and duplicate prevention

Deliver:

- `RecipeRenderClass` and `TunedPass`;
- classifications for existing families;
- one-population-per-tuned-pass validation;
- deferred dirt-clod report;
- compiler skips generic production emission for tuned classes;
- regression test that secondary output contains no generic grass/fine/thatch.

Expected image change: none.

### PR 2 — Authoritative evaluator in compiler

Deliver:

- `SceneCompilation.ground`;
- evaluator constructed before recipe emission;
- final surface roots;
- CLI reuses evaluator;
- root/ground registration tests.

Expected image change: none until secondary geometry renders. Scene fingerprints may move because generic roots become physically correct; document why.

### PR 3 — Scene anchors, prototypes, and interactions

Deliver:

- anchor table;
- prototype table/binding;
- instance anchoring;
- interaction types;
- scene validation and fingerprints;
- no active rendering yet.

Expected image change: none.

### PR 4 — Cycles scene format v2 with empty secondary sections

Deliver:

- versioned header;
- empty secondary buffers/table;
- Blender v2 reader;
- tuned geometry/image equivalence test;
- package validation.

Expected image change: none. This is the key no-regression checkpoint.

### PR G0 — Restore and pin the ground benchmark harness

Deliver:

- recovered or recreated `terrain_bench` crate;
- topography, semivariogram, PSD, state, optics, seam, and timing modules;
- committed laboratory documents and seed sets;
- report JSON and companion schema validation;
- current ground baselines, including the known geometry/bump mismatch and unused-state evidence;
- CLI integration.

Expected image change: none. This PR records current failures instead of fixing them, so the fix has an attributable before/after.

### PR G1 — Unify ground relief tiers and state propagation

Deliver:

- `GroundReliefPlan`;
- reusable Rust band/ripple/crack basis;
- Rust-authored bump and micro-slope fields;
- removal of active Blender procedural relief;
- compaction applied to every tier;
- `wet_film` Principled coat;
- BRDF roughness transfer laboratory and versioned LUT;
- cross-tier, resolution-ladder, and state-response reports.

Expected image change: ground relief and wet highlights only, explicitly reviewed. This PR must land before final meadow colour and density calibration, so content is not tuned against a ground representation that will then move.

### PR 5 — Flowers end to end

Deliver:

- exact stem curve;
- RMF sweep or tested equivalent;
- petal/head prototypes;
- flower grouped emission;
- secondary curve/instance lowering;
- flower materials;
- slice-group tests;
- flower laboratory render.

Expected image change: flowers only.

### PR 6 — Variable-radius priority exclusion

Deliver:

- migration-safe spacing policies;
- addressed footprint radius;
- total priority key;
- bucketed variable conflict search;
- brute-force oracle and composability tests;
- report counts;
- version bump.

Expected image change: only populations migrated to the new policy. Initially stones may be the only one.

### PR 7 — Stones end to end

Deliver:

- prototype bindings and procedural mesh definitions;
- explicit instances;
- burial and transforms;
- stone materials;
- conservative bounds and footprints;
- prototype/instance binary lowering;
- stone laboratory render.

Expected image change: stones only; tuned grass still passes through them until PR 8, which is acceptable only in the isolated branch and must not be declared meadow-tier complete.

### PR 8 — Interaction field and grass response

Deliver:

- deterministic bucket field;
- ellipse clearance/normal;
- tuned placement queries;
- hard root exclusion;
- shortening and outward bend;
- locality tests;
- interaction AOV/debug view.

Expected image change: grass only within declared stone influence reaches.

### PR 9 — Tuned population controls and semantic bare fix

Deliver:

- `TunedPopulationSet`;
- pass factors for tuft/fine/thatch/broadleaf;
- new semantic broadleaf recipe;
- reference-density calibration;
- `semantic_bare` single source of truth;
- before/after visual table and version bump.

Expected image change: controlled density/bareness and pass authoring.

This PR may be split into controls and bareness if review quality improves. Do not hide the bareness look change inside plumbing.

### PR 10 — Undergrowth

Deliver:

- recipe/domain;
- broad leaf ribbon geometry;
- flow/rosette morphology;
- stone response;
- material;
- laboratory and full-meadow renders.

Expected image change: undergrowth only.

### PR 11 — Soil decision

Deliver:

- variants A/B/C;
- complete §38 ground benchmark table for each variant;
- controlled albedo/roughness/coat AOVs and state sweeps;
- chosen profile architecture;
- migrated document/profile assets;
- before/after baselines;
- explanation of rejected variants.

Expected image change: soil and interstitial ground only, explicitly reviewed. The beauty preference alone does not decide this PR; the chosen model must also preserve morphology, state response, and cross-tier coherence.

### PR 12 — Meadow-tier release gate

Deliver:

- full acceptance matrix;
- all committed seeds;
- overlap/slice proof results;
- performance/quality report;
- updated current-state documentation;
- no aspirational claims;
- conditioning-contract work remains explicitly next.

## 43. Implementation-agent completion rule

The implementing agent should not mark a task done because code compiles or an isolated render contains the new object. Every PR must include:

```text
1. code
2. unit/property tests
3. deterministic fingerprints
4. count/performance report
5. visual evidence where pixels can change
6. updated current-state documentation
```

If a visual output changes unexpectedly outside the PR's declared effect radius or population, stop and explain the causal path before continuing.

---

# Part XI — Failure modes and rejected alternatives

## 44. Rejected: render the generic `TerrainScene` wholesale

Why it fails:

- duplicates tuned grass, fine, and thatch;
- introduces a lower-quality geometry vocabulary into the final image;
- obscures which path owns density and style;
- makes a visual regression look like “more detail.”

Required alternative: explicit render classes.

## 45. Rejected: replace tuned grass with generic families

Why it fails:

- loses colonies, flow, tillers, statement fields, broad masses, and tuned morphology;
- turns a content integration task into a full visual rewrite;
- invalidates all grass baselines and makes the new meadow impossible to judge.

Required alternative: semantic modulation through the tuned path.

## 46. Rejected: scatter in Blender

Why it fails:

- breaks addressed world determinism;
- makes trace slices and Rust scene metadata disagree;
- prevents the neural conditioning data from naming the actual objects rendered;
- hides randomness in another language.

Required alternative: explicit Rust instances.

## 47. Rejected: sequential variable-radius dart throwing

Why it fails:

- acceptance depends on traversal and window;
- adding or removing an early candidate moves later results;
- page/trace composability is not provable with a finite local contract.

Required alternative: non-recursive priority thinning over addressed proposals.

## 48. Rejected: recursive survivor-only priority thinning

It can produce denser/maximal sets, but a candidate rejected by another rejected candidate creates dependency chains. Determining status may require following an unbounded graph toward the window boundary.

Required alternative for this phase: raw-proposal Matérn-II-style suppression with a finite halo.

## 49. Rejected: cut circular holes around stones

Why it fails:

- stone appears dropped into a pre-cut lawn;
- surviving grass keeps its original orientation and height;
- interaction has a hard, obvious ring.

Required alternative: hard footprint plus smooth local morphology response.

## 50. Rejected: exact iterative ellipse distance in the hot loop

Why it fails the cost/benefit test:

- tuned placement queries millions of roots;
- the response band is artistic and centimetre-scale;
- the approximate normalised ellipse clearance is stable, bounded, and visually sufficient.

Exact distance may be added behind a benchmark if a specific visible failure is demonstrated.

## 51. Rejected: one unique stone mesh per instance

Why it fails:

- scene-package and Blender construction cost scale with instances;
- high-frequency unique noise is mostly invisible at target scale;
- it removes the primary benefit of prototypes.

Required alternative: a small prototype library plus explicit transform/attribute variation.

## 52. Rejected: use the darker meadow profile as baked canopy shadow

Why it fails conceptually:

- material composition and lighting become conflated;
- Cycles computes canopy occlusion again;
- the same soil cannot transition consistently from covered to exposed.

Required alternative: test one physical profile plus state and real occlusion.

## 53. Rejected: begin corpus generation after flowers but before stones/undergrowth

Why it fails:

- conditioning planes and population vocabulary are still moving;
- instance occupancy and interaction influence are absent;
- every generated target becomes obsolete.

Required alternative: complete meadow tier, then freeze.

---

# Part XII — Neural-renderer gate after the meadow tier

## 54. Conditions that must hold first

Only after this specification's acceptance gates pass should the project define `TerrainConditioningContractV1`.

At that point decide and version:

### Raw authored/compiled planes

```text
substrate weights
canonical modifier channels
elevation
microrelief
covers when implemented
```

### Derived structural planes

```text
normal
slope/aspect
curvature
flow accumulation/direction
exposure
blend
boundary tangent
feature context when implemented
```

### Derived causal render planes

```text
realised substrate weights
geometry displacement
cavity
wet film
crack mask
prototype/instance occupancy
interaction influence and direction
population abundance controls
```

The causal planes are derived, never authored. They explain why the target contains a stone, a bare fringe, bent grass, or a wet highlight.

## 55. Corpus rebuild

The new corpus job pairs:

```text
TerrainConditioningContractV1 tensor
        ↔
Cycles target + AOVs
```

It does not pair a cheap raster image with a path-traced image. `RenderPair` must hold one compilation and one scene identity; the conditioning tensor and target are two observations of that same world.

## 56. Required target passes

At minimum:

```text
beauty
albedo
world/view normal
depth
substrate/material weights
population/appearance IDs
instance occupancy or IDs
```

These passes are not substitutes for beauty supervision. They make errors attributable and support evaluation.

---

# Part XIII — Final acceptance checklist

## 57. Architecture

- [ ] Tuned grass generator remains the production quality bar.
- [ ] Generic grass/fine/thatch do not enter active secondary rendering.
- [ ] One compilation owns fields, ground evaluator, secondary scene, tuned controls, and interactions.
- [ ] Secondary content compiles once per logical nine-tile render, never once per trace slice.
- [ ] Blender performs no placement randomness.
- [ ] Active Cycles format is explicitly versioned.

## 58. Sampling

- [ ] Candidate-specific footprint radius is addressed.
- [ ] Conflict rule is symmetric and documented.
- [ ] Priority key is a strict total order.
- [ ] Bucketed thinning matches brute-force thinning.
- [ ] Whole-region and partitioned-region outputs are identical.
- [ ] Density lowering preserves survivors.

## 59. Ground and interaction

- [ ] Secondary roots use final geometry surface height.
- [ ] Stone footprints are conservative.
- [ ] No tuned roots lie inside hard stone footprints.
- [ ] Grass response is smooth and local.
- [ ] Outside interaction reach, tuned strokes are bit-identical.
- [ ] Semantic bare responds to authored abundance.
- [ ] Each tuned pass has independent authored control.
- [ ] Every relief contribution appears in exactly one recorded tier.
- [ ] Geometry, bump, and microfacet tiers share one Rust-owned causal basis.
- [ ] Compaction and moisture responses reach every relevant tier.
- [ ] `wet_film` drives a distinct Principled coat.
- [ ] Ground bump, state, and material fields agree across windows and slices.
- [ ] Ground benchmark structural, spectral, optical, and performance gates pass.

## 60. Content

- [ ] Flowers have coherent stems, heads, and petals.
- [ ] Flower groups cannot split across trace selection.
- [ ] Stones use reusable prototypes and explicit instances.
- [ ] Stones are visibly buried.
- [ ] Undergrowth is low, broad-leaved, patchy, and below the intended canopy role.
- [ ] Broadleaf and thatch remain tuned passes.
- [ ] Dirt clods remain deferred until relief ownership is decided.

## 61. Rendering

- [ ] Secondary ribbons render.
- [ ] Secondary curves render.
- [ ] Prototype instances render.
- [ ] Halo-only secondary objects cast shadows but are camera-invisible.
- [ ] Empty-secondary v2 matches the old tuned render.
- [ ] All binary lengths and indices are validated.
- [ ] Materials use linear-light inputs and deterministic attributes.

## 62. Evidence

- [ ] Workspace tests, clippy, and format checks pass.
- [ ] Refactor fingerprints either pass or every intentional baseline move is explained.
- [ ] Cross-window semantic equality passes.
- [ ] Cross-slice selection equality passes.
- [ ] Before/after beauty and AOVs exist.
- [ ] Count/performance/quality table exists.
- [ ] Ground benchmark report and schema validation exist.
- [ ] PSD/semivariogram/state-sweep artifacts exist for every intentional soil change.
- [ ] Weakest committed seed is acceptable.
- [ ] Current-state documentation says only what now exists.

---

# Appendix A — Algorithm reference summaries

## A.1 Matérn type-II hard-core process

A parent point process receives independent priority/time marks. A point is retained when no point with a smaller time—or, under the reversed convention used here, greater priority—lies within its inhibition neighbourhood. This supplies the conceptual basis for Groundwork's non-recursive, traversal-independent priority thinning.

Groundwork differs from the classical stationary stochastic model because its proposals come from an addressed jittered lattice, priorities are deterministic functions of candidate addresses, and the domain is evaluated through finite windows with explicit halos.

## A.2 Variable-radius Poisson-disk criteria

Variable-radius sampling literature distinguishes asymmetric prior/current-radius conflict tests and symmetric maximum/minimum-radius tests; symmetric functions are order-independent as pairwise constraints. Physical non-overlap uses a sum-of-radii rule. Groundwork chooses sum-of-footprint-radii plus clearance because candidate radii represent occupied object disks, not merely desired local sample spacing.

## A.3 Rotation-minimising frames

A rotation-minimising frame transports a normal around a curve while avoiding unnecessary twist about the tangent. The double-reflection method provides a stable, high-order discrete approximation and avoids the Frenet frame's instability when curvature vanishes or changes sign. It is therefore appropriate for stems and leaves swept into tubes or ribbons.

## A.4 Superquadrics

Superquadrics generalise quadrics through signed powers. Superellipsoids compactly span rounded, blocky, flattened, and pinched silhouettes with a few parameters, making them suitable bases for a small deterministic stone prototype library. Low-order deformation and clipping supply natural irregularity without per-instance unique meshes.

## A.5 Blender data sharing and ray visibility

Blender linked duplicates share object data while retaining independent transforms, which is appropriate for explicit prototype instances. Cycles ray-visibility controls allow halo objects to be invisible to the camera while remaining visible to shadow and lighting rays. Bulk mesh property transfer through Blender's array/`foreach_set` APIs is the established path for avoiding slow per-vertex Python operations.

## A.6 Soil topography metrics

A single random-roughness value cannot identify feature scale or directional structure. Soil-surface studies therefore combine detrended height dispersion with spatial statistics such as semivariograms; modern rough-surface analysis further treats slope and curvature as explicitly scale-dependent and relates those measures to PSD and autocorrelation. Groundwork adopts that combination because a procedural band list is itself a scale model and must be tested at the scales it claims.

## A.7 Wet-soil reflectance and film response

Measured soil reflectance changes nonlinearly with moisture and can saturate in visible/near-infrared ranges. Groundwork does not attempt a spectral remote-sensing model, but it uses the result to reject a single linear greyscale multiplier. Moisture changes the substrate's absorption/scattering response; a continuous surface film is represented separately as a dielectric coat.

## A.8 Render-difference metrics

FLIP was designed specifically to evaluate perceptual differences between rendered images and aligned references. It is therefore the primary pooled beauty-difference metric in the render regression suite. It does not replace exact AOV comparisons, morphology metrics, or visual review, and it is not used pixelwise against an unregistered photograph.

---

# Appendix B — Primary research and official references

1. Scott A. Mitchell, Alexander Rand, Mohamed S. Ebeida, and Chandrajit Bajaj. **Variable Radii Poisson-Disk Sampling.** 24th Canadian Conference on Computational Geometry, 2012.
2. Markus Kiderlen and Mario Hörig. **Matérn’s Hard Core Models of Types I and II with Arbitrary Compact Grains.** Centre for Stochastic Geometry and Advanced Bioimaging Research Report 2013-05, 2013.
3. Jesper Møller, Mark L. Huber, and Robert L. Wolpert. **Perfect Simulation and Moment Properties for the Matérn Type III Process.** 2009. Its introduction states the type-I/type-II/type-III distinction used here; Groundwork deliberately uses the non-recursive type-II-style rule rather than survivor-recursive type III.
4. Robert Bridson. **Fast Poisson Disk Sampling in Arbitrary Dimensions.** SIGGRAPH Sketches, 2007. Used here as a contrast for active-list, history-dependent generation rather than as the implementation algorithm.
5. Wenping Wang, Bert Jüttler, Dayue Zheng, and Yang Liu. **Computation of Rotation Minimizing Frames.** ACM Transactions on Graphics 27(1), 2008.
6. Alan H. Barr. **Superquadrics and Angle-Preserving Transformations.** IEEE Computer Graphics and Applications 1(1), 1981.
7. Blender Python API documentation for mesh bulk access and `foreach_set`.
8. Blender manual/API documentation for linked duplicates and shared object data.
9. Blender Cycles documentation for per-ray object visibility.
10. H. Croft et al. **Modeling Fine-Scale Soil Surface Structure Using Geostatistics.** Water Resources Research, 2013. DOI: `10.1002/wrcr.20172`.
11. L. M. Thomsen et al. **Soil Surface Roughness: Comparing Old and New Measuring Methods and Application in a Soil Erosion Model.** SOIL 1, 2015, 399–410. DOI: `10.5194/soil-1-399-2015`.
12. Antoine Sanner, Wolfram G. Nöhring, Luke A. Thimons, Tevis D. B. Jacobs, and Lars Pastewka. **Scale-Dependent Roughness Parameters for Topography Analysis.** Applied Surface Science Advances 7, 2022, 100190. DOI: `10.1016/j.apsadv.2021.100190`.
13. David B. Lobell and Gregory P. Asner. **Moisture Effects on Soil Reflectance.** Soil Science Society of America Journal 66(3), 2002, 722–727. DOI: `10.2136/sssaj2002.7220`.
14. Pontus Andersson et al. **FLIP: A Difference Evaluator for Alternating Images.** Proceedings of the ACM on Computer Graphics and Interactive Techniques 3(2), 2020. DOI: `10.1145/3406183`.
15. Zhou Wang, Alan C. Bovik, Hamid R. Sheikh, and Eero P. Simoncelli. **Image Quality Assessment: From Error Visibility to Structural Similarity.** IEEE Transactions on Image Processing 13(4), 2004. DOI: `10.1109/TIP.2003.819861`.
16. Gaurav Sharma, Wencheng Wu, and Edul N. Dalal. **The CIEDE2000 Color-Difference Formula: Implementation Notes, Supplementary Test Data, and Mathematical Observations.** Color Research & Application 30(1), 2005. DOI: `10.1002/col.20070`.
17. Blender 5.2 LTS Manual, **Principled BSDF**, including Coat Weight, Coat Roughness, and Coat IOR.
18. Blender Python API, `bpy_prop_collection.foreach_set` and geometry attribute bulk access.

---

# Appendix C — Source baseline map

The key current-state claims in this specification were checked against these supplied paths:

```text
AGENTS.md
CLAUDE.md
assets/terrain/documents/meadow_path.terrain.ron
assets/terrain/materials/meadow_floor.ground.ron
assets/terrain/materials/compacted_loam.ground.ron
crates/terrain_core/src/{coords,document,ground_material,prepare,sample,seed,sources}.rs
crates/terrain_scene/src/{field,derive,mark,instance,scene}.rs
crates/terrain_generators/src/{compiler,domain,ownership,field,ground,placement,style,families,recipe}.rs
crates/terrain_cycles/src/{cycles,plate,export,package}.rs
tools/blender_cycles/render.py
tools/terrain_cli/src/main.rs
crates/terrain_dataset/src/{lib,shard}.rs
Cargo.toml and CLI references to the absent crates/terrain_bench snapshot path
```

The governing product direction comes from `groundwork-meadow-tier-spec.md`: complete the meadow content vocabulary, preserve the tuned grass generator, make compiled marks reach Cycles, wire grass around stones, fix bareness, and freeze the neural contract only afterward.

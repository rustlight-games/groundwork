# Low fidelity to high fidelity

How a semantic terrain description becomes one path-traced isometric plate, and
why each stage is where it is.

This describes the system as built. What it does not have yet is in
[todo/](todo/), one file per gap, so that nothing here has to be read in the
future tense.

## The unit, and what each half of the name means

The product is **one isometric subject tile**. The generation unit is not that
tile: it is the tile, its eight context tiles, and a halo around all nine large
enough for every neighbourhood operation anything performs.

```text
        generated continuously
    ┌────────┬────────┬────────┐
    │context │context │context │
    ├────────┼────────┼────────┤
    │context │SUBJECT │context │   + a derived halo outside this diagram
    ├────────┼────────┼────────┤
    │context │context │context │
    └────────┴────────┴────────┘

    output      the subject diamond
    generation  all nine tiles, and the halo
```

**Low fidelity** is the `TerrainFieldStack`: typed planes with declared units,
sampled onto an edge-anchored lattice. It is not a small picture. An RGB image
cannot tell the next stage whether a dark patch means lower ground, wet dirt,
shadow, dense grass, a different substrate or a painted colour variation, and
geometry, ownership and lighting each need a different one of those answers. The
cheap picture is a *derivative* of the stack, never its source.

**High fidelity** is the same field stack, the same accepted candidate
identities, the same ownership and the same scene primitives, rendered more
accurately: real tessellation, real materials, real light transport. It is never
a second, more detailed terrain.

## The pipeline

```text
Authored terrain document              assets/terrain/documents/*.terrain.ron
    │  parse · migrate · validate                          terrain_format
    ▼
TerrainDocument
    │  prepare — keys become indices, sources become fields,
    │            anything unsupported is refused here          terrain_core
    ▼
PreparedTerrain                immutable · Send + Sync · sampling cannot fail
    │
    │           terrain_generators::compiler, in this order:
    │
    │  resolve populations, domains, and the halo they imply
    │  sample one field stack over the generated bounds     ── THE MATRIX
    │  derive slope, curvature, flow, exposure, boundary frame
    │  generate each shared candidate domain once
    │    ├─ realise the substrate at the candidate     (transition solver)
    │    ├─ blend one target density from every claimant
    │    ├─ accept or reject, once                     (fixes the count)
    │    ├─ score every claimant and draw one owner    (fixes the identity)
    │    └─ hand the candidate to that owner's recipe
    │  lower emissions into the scene, with candidate-derived ids
    │  sort once, fingerprint
    ▼
TerrainScene                   one immutable scene, built once, rendered twice
    ├────────► Cycles                                        terrain_cycles
    ├────────► the cheap tier                                  terrain_bake
    └────────► the paired corpus                             terrain_dataset
```

`terrain compile` runs all of it. One qualification, and it matters: the scene
it path-traces is **not** the `TerrainScene` it just compiled. It compiles the
scene, reports its counters, and then hands Cycles the tuned generator driven by
a `SemanticOverlay` over the compiled field stack — because the tuned generator
is the quality bar and the compiler's own content families are not.

So everything down to and including the field stack, the derived fields, the
realised boundary and the candidate report is the production path today. The
last arrow — compiled marks reaching Cycles — is
[todo/one-grass-generator.md](todo/one-grass-generator.md) and
[todo/render-paths.md](todo/render-paths.md).

The overlay is a narrow bridge on purpose. It modulates how much grows and
whether earth shows, and every style field stays exactly as tuned: the colonies
still comb together, the statement field still lets passages collapse into
paint, the mounds still swell. Replacing the whole field with document values
would produce semantically correct ground that looks like a carpet.

## The invariants this rests on

These are acceptance gates, not aspirations. AGENTS.md is the governing
statement; this is what each one means at this level.

**Semantic determinism.** For a fixed document, seed, recipe versions and world
position, the answer does not depend on process, thread count, page layout,
render tile layout, crop size, or whether the neighbouring region was generated
at the same time.

**Addressed randomness only.** No decision depends on "the next number". Every
value is addressed by `root seed · candidate domain · world cell · candidate
rank · child rank · named stream · recipe version`. A new decision adds a stream
name; it never inserts a positional draw.

**One semantic scene.** Both renderers consume one `Arc<TerrainScene>`.
`RenderPair` has no constructor that accepts two scenes.

**Tiles never decide generation.** World tiles decide framing, subject
weighting, masks and output naming. Nothing in the compiler knows which tile is
the subject.

**Materials blend before rendering.** Blending operates on semantic weights and
candidate ownership. Two finished images are never composited to make a boundary.

**Layers and populations stay separate.** Layers answer *what is here*;
populations answer *which countable things exist here*. A population never
writes a material weight, because that would make composition circular.

**Quality tiers change measurement, not meaning.** A tier may spend fewer ribs,
fewer shadow samples, coarser filtering, less supersampling. It may not change
candidate identity, acceptance, ownership, root position or scene topology.

**Every channel declares itself.** Unit, legal range, filter, border rule. A
categorical plane must not be bilinearly interpolated — the average of material
2 and material 4 is not material 3 — and a direction plane must be renormalised
after averaging. `FieldDescriptor` carries all four so the rule travels with the
data rather than living in each consumer's head.

## The matrix

`terrain_scene::field::TerrainFieldStack`. Four groups, kept apart because their
arithmetic is genuinely different:

| Group | What it is | Composition |
| --- | --- | --- |
| **Structural** | `elevation_m`, `microrelief_m` | metres, added |
| **Substrate** | what the continuous ground is made of | mutually exclusive, normalised to one |
| **Cover** | what lies *over* the substrate | depth and coverage, **not** normalised against the substrate |
| **Modifier** | authored control fields | independent, each with its own rule from the document |
| **Derived** | what the compiler computes from the above | see below |

A population is none of these. A plane says *how much* grows, never *which
blades*.

The cover group is the modelling decision that was expensive to retrofit and
cheap to reserve: snow over grass leaves the grass semantically present
underneath, and a third normalised weight would erase it. `CoverPlane` carries
depth, coverage, compaction and wetness. Nothing fills one yet —
[todo/covers-and-snow.md](todo/covers-and-snow.md).

### Edge-anchored, and snapped to a global lattice

Two properties, both seam insurance.

1. The lattice samples its own **corners**: a grid over `columns × rows` cells
   holds `(columns + 1) × (rows + 1)` samples, so the last column of one grid
   *is* the first column of its neighbour.
2. The origin is **snapped down to a multiple of the spacing**. This is the one
   that is easy to miss. Edge anchoring alone only makes two grids agree if they
   happen to share a lattice, and two requests with different bounds do not —
   their samples interleave and the surface between them is nobody's. Snapping
   makes every grid at a given spacing a window onto *one* world lattice, so two
   regions compiled in different processes agree exactly wherever they overlap.

`FieldGridSpec::covering` is the only constructor a compiler should use,
precisely so that property cannot be opted out of by accident.

### Spacing

Derived from the request, not chosen: a quarter of a field sample per output
pixel, clamped to 5 mm–10 cm. A quarter because the matrix carries the *macro*
fields and everything finer comes from the transition solver and the marks, both
evaluated analytically rather than read off the grid. Sampling the matrix at
pixel rate would quadruple its cost to carry frequencies nothing reads from it.

### Sampled once, at a footprint

`terrain_scene::derive::sample_fields` is the only place in the framework that
walks `PreparedTerrain` on a lattice. Everything downstream interpolates *this*
rather than asking the terrain again. Two consumers sampling the same path edge
at their own rates disagree about where it is by a fraction of a texel — which
is invisible in every test and is a seam in the picture.

Each sample carries a footprint of half the grid spacing, so a mask read at a
lattice point antialiases against the ground it actually covers instead of
aliasing at a mathematical point. Without it a path edge staircases at the
sampling rate, and the edge is the one thing on the plate the eye is guaranteed
to look at.

### Derived once, for the same reason

`DerivedFieldSet` carries the ground normal, slope as a tangent, aspect, mean
curvature, sky exposure, flow accumulation and direction, the dominant and
secondary substrate, how mixed the ground is, and the boundary tangent.

The characteristic failure is not that one of them is wrong; it is that two of
them are *slightly different*, so a population that thinned on a slope and a
renderer that shaded it disagree about where the slope was.

Every derivation is a finite difference over the **combined** structural
surface — elevation plus microrelief — rather than a mixture of analytic and
differenced gradients. The analytic microrelief gradient is more accurate in
isolation and is the wrong choice: the renderer differences the grid, so a
placement derived analytically would sit on a surface with a slightly different
normal from the one it is drawn against.

Rows sample in parallel and stitch in index order. The flow solver orders cells
by height with the sample index as the tie-break, because two cells at exactly
equal height are common on ground that is mostly flat.

The world being flat makes three of these near-constant today —
[todo/elevation.md](todo/elevation.md).

## Substrate, vegetation, cover

Three questions, three answers, and conflating any two of them produces a
characteristic failure.

**Substrate** — what continuous ground is beneath everything. Normalised to one.
Drives ground material, displacement character, moisture and compaction
response, and population affinity.

**Vegetation** — what discrete canopy grows out of the substrate. Principally a
population controlled by density and morphology fields, which is what lets the
same grass grow thickly on meadow soil and sparsely on a compacted track without
inventing a `dirt-with-a-bit-of-grass-on-it` material for every combination.
`meadow_path` names its base material `meadow_soil` for exactly this reason: the
grass is not the ground.

**Cover** — what continuous material lies over the substrate and around the
populations. Depth and coverage, independent of substrate normalisation.

The visible surface then resolves in one order: structural height, substrate
mixture, cover depth, discrete roots and blades and stones, cover–geometry
interaction, lighting. That order is what makes "snow on grass on soil" a
meaningful state rather than a colour blend of three unrelated pictures.

## The boundary between two substrates

Two scales, separately authored, and the distinction is the whole design.

The **band** is where the ground is changing, and the document says so through a
mask. The **raggedness** is how the change is realised inside that band, and it
is a property of the pair of materials meeting there.

An authored `SmoothBand` over a spline distance produces a clean monotone ramp.
Rendered directly it reads as an airbrushed decal — the boundary is a smooth
curve at the band's own scale, and nothing in nature has one. The reference
plates show boundaries broken into islands and peninsulas a few centimetres
across sitting inside a band tens of centimetres wide; see
[references/](references/).

`terrain_generators::transition` perturbs each material's score by its own noise
field before normalisation:

```text
w         = normalise(score)
contest_k = 4 · w_k · (1 − w_k)        // 1 where evenly split, 0 at either end
w_k'      = max(0, w_k + amplitude · (noise_k(p) − ½) · contest_k)
weights   = normalise(w')
```

Per material rather than one shared field, so the lobes of one interpenetrate
the lobes of another instead of the whole boundary wobbling as a unit.

The **contest** term is what makes the extremes safe. Perturbing a lone material
at weight one can drive it to zero, leaving ground made of nothing; perturbing a
material at weight zero can conjure mud into the middle of a clean meadow.
Scaling by `4w(1−w)` removes both.

What falls out of this formulation, and why it was chosen over displacing the
boundary curve directly: the contour where two weights cross moves by roughly
`amplitude / |∇score|` **metres**. A wide gentle band gets big islands and a
tight band gets a crisp edge, from the *same* raggedness setting. An author gets
both references by moving the band rather than by retuning the noise.

It is deliberately **not** baked into the field stack. The lobes are finer than
a sensible grid spacing, and — the reason that matters — ownership and ground
shading must consult the same answer. A candidate asks "is this point grass?"
and a texel asks the same question a millimetre away; if one read a baked plane
and the other evaluated the function, the mud would be painted in a slightly
different place from where the grass thinned, and the transition would read as
two unrelated effects that nearly line up.

## Shared candidate domains

The mechanism that stops a transition doubling its density.

### The failure it exists to prevent

Ask two populations to fill the same ground and a transition gets both. Grass at
70% emits a full grass population scaled to 0.7, dirt detail at 30% emits a full
one scaled to 0.3, and where they meet there are *more* marks than on either
side — a busy stripe down the boundary, which is the most recognisable failure a
terrain blend has. Scaling by weight does not fix it: two independent scatters
at 0.7 and 0.3 still put down 1.0 of *positions*, and the clumps of one
interleave with the clumps of the other.

So the candidate field is shared. One lattice, one acceptance decision, then a
separate draw deciding which recipe gets each accepted candidate. A transition
emits one mark per accepted candidate, exactly as the pure ground on either side
does, and the only thing that changes across the boundary is which recipe drew
it.

**Acceptance happens before ownership.** That is the whole reason it works: the
number of things is settled while the materials are still an undecided question.

### Capacity is fixed; density is a threshold

The tempting design is to size the lattice from the density the author asked
for. It is wrong, and the symptom is confusing: changing density changes the
cell size, which changes every candidate's cell and rank, which changes every
address, and the whole meadow is redrawn. An author nudging density from 400 to
420 sees every blade move.

Instead a domain declares a fixed capacity — a cell side and a count per cell —
and density becomes an acceptance probability:

```text
p_accept = target_density / max_density
accept if unit(candidate, "accept") < p_accept
```

The draw belongs to the candidate, so raising density can only *add* candidates
and lowering it can only remove them. Survivors never move. That is what makes a
density field paintable and a boundary moveable without the terrain popping.

More than one candidate per cell, always. A lattice offering exactly one shows
its own grid however hard the position is jittered, because the *count* is
uniform even when the placement is not — and the eye finds a uniform count
faster than a uniform position.

The domains in `terrain_generators::families`:

| Domain | Cell | Per cell | Spacing |
| --- | ---: | ---: | --- |
| `vegetation.tuft_anchor` | 12 cm | 8 | exclusion |
| `vegetation.fine` | 6 cm | 8 | jittered |
| `vegetation.emergent` | 25 cm | 6 | exclusion, 8 cm |
| `surface.grit` | 8 cm | 8 | jittered |
| `rock.large` | 50 cm | 4 | exclusion, 22 cm |

Capacity is declared by the recipe rather than by the document, and the first
recipe naming a domain supplies it — capacity is a property of the lattice
rather than of any one occupant, and two recipes sharing a domain have to agree
about it.

### Conflict thinning is by stated priority

Pure jitter clumps. For tuft anchors, stones and flowers that clumping is the
wrong kind — real ones exclude each other — so a candidate is dropped when a
higher-priority neighbour lies inside its exclusion radius.

The test is deliberately **non-recursive**: a candidate is compared against every
neighbour, not against the neighbours that *survived*. A recursive test packs
slightly better and is order-dependent to compute, so the result would depend on
which region was walked first, and two neighbouring plates would disagree along
their join. Thinning slightly harder is the cheaper mistake by a wide margin.

For that to hold across regions, `generate` expands its working area by the
domain's maximum exclusion radius before thinning, so a candidate near the edge
is judged against the same neighbours whichever window asked for it.

### The ownership draw

```text
owner_score_k = substrate_affinity_k · abundance_k · profile_weight_k · boundary_k
```

A product rather than a sum, because every term is a veto. A recipe with no
affinity for the ground under it should get the candidate *never*, not rarely —
and a sum lets a large abundance drown a zero affinity, which is how grass ends
up growing on bare rock at low density instead of not at all.

One value in `0..1`, addressed to the candidate on the `owner` stream, walked
against the normalised scores in owner order. Ownership is therefore stable
under a rebuild, independent of registration order, and independent of which
region was generated.

Ownership *does* move when a neighbouring owner's score changes, and that is
inherent to a categorical draw rather than a defect. What is preserved — and
what matters for popping — is that the candidate's **position and latent
attributes do not move** when ownership changes hands. A boundary nudged by a
centimetre reassigns a handful of marks and leaves every other one exactly where
it was.

An accepted candidate no recipe wants is left unowned: bare ground, counted in
the compile report.

### Ids come from identity, never enumeration

`MarkId` derives from the domain, cell, rank, owning recipe, child index and
recipe version. Enumeration order changes when a recipe is added or emission is
parallelised, and an id that moves for that reason takes every cache and every
comparison with it.

## The halo is derived, never guessed

A mark rooted outside the visible rectangle still leans into it, shades into it
and occludes it, and a neighbourhood term reads further still. The generated
bounds are the visible bounds grown by the largest reach anything needs:

```text
halo = max(recipe reach, conflict radius, flow reach, source reach)
```

Every term is an upper bound rather than a typical value, because getting it
wrong in the small direction is a bright seam at the frame edge and getting it
wrong in the large direction is only wasted work.

## One scene, rendered twice

`TerrainScene` carries ribbons, curves, analytic marks and stamps, plus
prototype instances, plus the ground. It has no `render_grass` and no
`render_wildflowers`, because a method per content type is a renderer that grows
one per ecological category, each duplicating most of the last.

A mark carries how *old* it is and how *wet* its ground is — intrinsic
properties — and nothing about the current light. That is what lets a scene
survive a lighting change without being regenerated.

Painter order is semantic and total: stratum, then quantised depth, then
sublayer, then a stable id as the tie-break. Never derived from generation
order. The tie-break is easy to omit and expensive to add — a fork's two
children and a tuft's blades tie exactly.

### What Cycles owns, and what it does not

Cycles owns material shader graphs, tessellation budget, light transport,
camera, AOVs, sampling and denoising. It does **not** own scattering, candidate
acceptance, material ownership, grass profile selection or framing. Blender's
own scattering would break determinism quietly — the seam would appear, nothing
would report it, and the cause would be in a different language from the
symptom.

See [CYCLES_BACKEND.md](CYCLES_BACKEND.md) for the package format, the mirrored
world, the trace tiling and the passes.

### The cheap tier

`terrain_bake::generic::render_scene` takes a `&TerrainScene` and its field
stack. It never constructs a `WorldField` or a `GrassScene`, never scatters, and
never asks the terrain a question — everything it draws was decided by the
compiler.

Its ground pass calls `terrain_generators::transition::realise` rather than
reading the substrate planes directly, which is not an optimisation: it is why
the boundary works. Ground shading and ownership ask the same function the same
question.

Which routes are actually wired to which renderer, and what survives the
deletion of the rasterisers, is [todo/render-paths.md](todo/render-paths.md).

## What the network is handed

The eventual consumer is a neural renderer trained to produce the path-traced
picture from the cheap one. What it receives is a structured set of channels,
not only RGB, because a network asked to infer geometry from colour will learn
to hallucinate it.

Exported today, per pair: the cheap beauty with separate alpha, the layout
coverage and subject mask, and the Cycles target with its implemented passes —
`terrain_cycles::aov` names twelve and this build produces at least eight.
Asking for an unimplemented pass is a reported error rather than a silent
omission, because a corpus with nine channels where ten were asked for is a
corpus with a hole nothing reports.

Available in the field stack and not yet exported per pixel: the substrate
weights, the dominant material and blend amount, every modifier channel the
document declares, and every derived field. Those are the channels worth
ablating — a channel never exported cannot later be tested — and the shard is
still page-shaped rather than tile-shaped, which is
[todo/dataset-tile-shape.md](todo/dataset-tile-shape.md).

**Semantic colour is not premultiplied by alpha.** Colour and coverage stay
separate, or the network learns that partially covered grass is intrinsically
darker. The slice filter weights colour by coverage for the mirror-image reason:
with a transparent film, background samples come back black at zero alpha, and
averaging them in unweighted puts a black fringe on every silhouette.

## Versions, kept in separate domains

A renderer change must not relocate grass, and a candidate change must not
masquerade as a shader change. So each of these moves on its own:

| Constant | Where | Moving it means |
| --- | --- | --- |
| `SEED_ALGORITHM_VERSION` | `terrain_core::seed` | every plant in every world relocates |
| `DIGEST_ALGORITHM_VERSION` | `terrain_core::digest` | cached comparisons invalidated, nothing moves |
| `CURRENT_FORMAT_VERSION` | `terrain_format::envelope` | a migration step must exist for the previous one |
| `BUILTIN_SOURCE_VERSION` | `terrain_core::registry` | a built-in source answers differently |
| `FIELD_STACK_VERSION` | `terrain_scene::field` | the matrix's own layout changed |
| `DOMAIN_ALGORITHM_VERSION` | `terrain_generators::domain` | every candidate in every domain moves |
| `TRANSITION_VERSION` | `terrain_generators::transition` | every realised boundary moves |
| `COMPILER_VERSION` | `terrain_generators::compiler` | how a candidate becomes a mark changed |
| `GENERATOR_VERSION` | `terrain_bench::fingerprint` | the meadow is meant to be different |
| `GENERIC_RASTER_VERSION` | `terrain_bake::generic` | the cheap picture changed |
| `PACKAGE_VERSION` | `terrain_cycles::package` | the Blender side must be updated with it |

Each changes deliberately, in the same commit as the change that caused it, with
the reason in the message.

## The decisions that are locked

1. The canonical low-fidelity representation is a typed field stack, not RGB.
2. The output is one subject tile; generation is context plus halo.
3. `TerrainScene` is built directly from `PreparedTerrain`.
4. Substrate, vegetation and cover are distinct semantic groups.
5. A cover is continuous, with depth and coverage — never a normalised base
   material.
6. Grass detail variants are population and profile fields, not separately
   rendered images.
7. Candidates come from shared fixed-capacity domains.
8. Density is threshold acceptance; it does not resize the lattice.
9. Spacing conflicts resolve by stable candidate priority.
10. Each candidate has at most one owner per domain.
11. Mark ids derive from candidate and child identity, never enumeration.
12. Derived terrain fields are computed once and carried.
13. Both renderers receive one immutable scene.
14. Every substantial change carries semantic, visual, seam and performance
    evidence.

## Where the reasoning came from

The design choices above are supported by production practice and published
work rather than invented:

- Production height-field tools treat terrain as named layers that combine,
  convert to masks, drive parameters and separate materials — the field-stack
  model rather than a monolithic image.
- Feature masks from slope, height, curvature, facing and occlusion are standard
  and explicitly useful for snow placement and vegetation growth — first-class
  derived fields.
- Mask-controlled scattering with density per square metre is a mature pattern —
  world-unit density fields and typed population controls.
- Base surfaces and top covers want different composition semantics: normalised
  weight blending for the base, independent height-aware coverage for what lies
  over it — substrates apart from covers.
- Priority-based Poisson-disk methods assign stable unique priorities and
  resolve conflicts through them while supporting spatially varying density —
  which maps directly onto addressed randomness and deterministic parallel
  thinning.
- Fallen-snow modelling benefits from separating accumulation from stability,
  conserving mass and locally redistributing what is unstable — a deterministic
  deposition, stability and wind solver rather than white masking.
- Neural reconstruction benefits from auxiliary per-pixel channels such as depth
  and normals — semantic and structural AOVs beside the cheap RGB.

Sources: SideFX Houdini documentation (HeightField Layers; Mask by Feature;
Scatter; Flow Fields and Slump); Epic Games Unreal Engine documentation
(Landscape Materials; Landscape Layer Blend); Ying, Xin, Sun and He, *An
Intrinsic Algorithm for Parallel Poisson Disk Sampling on Arbitrary Surfaces*,
IEEE TVCG 2013; Fearing, *Computer Modelling of Fallen Snow*, SIGGRAPH 2000;
Chaitanya et al., *Interactive Reconstruction of Monte Carlo Image Sequences
using a Recurrent Denoising Autoencoder*, TOG 2017.

## The risks this design is built against

Each is a mistake somebody makes when they do not know why a piece is shaped the
way it is.

| Risk | What it produces | What prevents it |
| --- | --- | --- |
| Low fidelity treated as RGB | ambiguous edits; the network must infer semantics | typed planes are canonical, RGB is a derivative |
| Candidate lattice sized by density | every density edit pops the terrain | fixed capacity, threshold acceptance |
| Snow as another normalised material | thin cover becomes muddy three-way blending | covers carry depth and coverage |
| Tile-isolated generation | straight grass cuts, missing shadows, tile-border artefacts | subject output, context generation, derived halo |
| Derived fields recomputed per consumer | placement and shading disagree about the slope | derived once into the stack |
| Order-dependent cover solver | thread count or crop layout changes the snow | Jacobi/red-black updates, stable reductions |
| One universal candidate domain | grass spacing applied to boulders | a small set of semantic scale domains |
| Quality tier changing semantics | the pair stops being paired data | the scene is compiled before quality is chosen |
| New content built on the legacy path | two sources of truth to migrate later | no new semantic features in `WorldField`/`GrassScene` |

# Groundwork: what this system is, where it actually is, and what comes next

A specification and an honest inventory, written for somebody who has the source
but has not been living in it. Every claim about the current state below was
checked against the code rather than recalled.

---

## 1. What this system is

**A semantic terrain compiler that produces geometry for a path tracer.**

An author writes a document describing what the ground *means* — this material
here, this much wetter there, this much less growing along that. The compiler
turns that into geometry. Blender Cycles renders the geometry. There is no
second renderer and no cheap tier.

Three invariants hold everything else up.

### The unit is nine tiles, always

Three by three, subject in the middle, generated as **one continuous scene**
with a halo — never nine scenes composited. Grass crosses the internal joins,
shadows fall across them, the colour field does not stop at a tile edge. This is
not a rendering convenience; it is what makes a tile composable with its
neighbours without a visible seam.

### Low fidelity means a matrix, not a small picture

The low-fidelity representation of a terrain is the `TerrainFieldStack`: typed
planes with declared units, filters and border rules, sampled on an
edge-anchored lattice addressed by **integer index**. It is not a downscaled
render.

That distinction is the whole design. A neural renderer trained to go from a
cheap picture to an expensive one learns to undo a rasteriser's mistakes. A
neural renderer trained to go from a *semantic matrix* to a render learns
terrain.

### Randomness is addressed, never drawn

Every stochastic decision is keyed on a population hash, a world cell, a
candidate rank and a **named stream**. Nothing is a sequential draw. This is what
lets two overlapping sampling windows agree bit-for-bit, which is what makes the
nine-tile invariant true rather than approximately true.

### What this system is not

- Not a game engine's terrain system. Nothing here runs per frame.
- Not a general 3D scattering tool. It knows about ground and what grows on it.
- Not a renderer. It builds geometry and hands it over.

---

## 2. Where it actually is

Verified against the tree, not remembered. Nine crates, 608 tests.

### Done and load-bearing

**The matrix.** `TerrainFieldStack` with derived slope, aspect, curvature, flow
accumulation and exposure. Integer-lattice addressing, so two windows over the
same ground sample the same points. Bounded flow solver, so upstream support
cannot depend on how big a window happened to be.

**The transition solver.** Perturbs material weights by per-material noise scaled
by `contest = 4·w·(1−w)`, so pure ground stays pure and noise cannot conjure a
material the document excluded. A contour moves by roughly amplitude over
gradient, so band width controls raggedness scale for free.

**Shared candidate domains.** One lattice, one acceptance decision, then a
separate ownership draw. Fixed capacity with threshold acceptance, so lowering a
density removes candidates without moving the survivors. This is what stops a
material boundary doubling its density.

**Ground material profiles.** `assets/terrain/materials/*.ground.ron` — versioned
independently of the documents that name them. Measured three-stop palette, wet
response fitted through a declared soaked colour, a list of relief bands, cracks,
ripples, scatter budget, vegetation affinity.

**One `GroundEvaluator`.** Every consumer — the mesh, the shader, the grass
overlay — asks it. It realises the substrate once. Two callers realising the same
ragged boundary separately is how a track's colour ends up a centimetre from
where its grass thinned, and this is the structural answer to that.

**A three-tier relief ladder.** A band is geometry if the lattice resolves it,
bump if a traced pixel does, and roughness otherwise. Each band drawn exactly
once by whichever tier can draw it. The lattice spacing is chosen from the
coarsest band of the soils in play, not from a constant.

**The cavity channel.** How deep in its own relief a point sits, out of the same
band sum that makes the displacement. The dark in soil is occlusion, not pigment
— which is why the darkest fifth of a photograph of bare earth is nearly neutral
in hue while its median is a warm brown. Before this the shader took its tone
from noise uncorrelated with the height, and the surface read as painted paper
however much relief the mesh carried.

**Cycles as the only renderer.** The painterly and generic rasterisers are
deleted. `terrain_bake` is now a request contract with no renderer in it.

### The content that actually renders

This is the part most likely to be misremembered, so it is stated as a table.

| Content | In the tuned generator? | In `families`? | **Renders?** |
| --- | --- | --- | --- |
| Grass tufts | yes | yes | **yes** |
| Fine grass | yes (`Stream::Fine`) | yes | **yes** |
| Thatch / ground mat | yes (`Stream::Thatch`) | yes | **yes** |
| Broadleaf clusters | yes (`Stream::Leaf`, `leaf_cluster`) | no | **yes** |
| Flowers | **no** | yes | **no** |
| Stones | **no** | yes | **no** |
| Dirt clods | **no** | yes | **no** |
| Undergrowth | **no** | no | **no** |

`grep -ci 'flower\|petal\|bloom' placement.rs` returns zero. The tuned generator
has never grown a flower.

### The one fact that blocks the next phase

**Compiled marks are discarded.** `compile_scene` places every population a
document declares — `meadow_path` asks for six, including flowers at 8/m² and
field stones — and the CLI references the result in exactly two places, both
inside the progress report:

```rust
compiled.scene.fingerprint().short(),
compiled.scene.mark_density()
```

The picture comes entirely from the tuned `WorldField`. Everything the scene
compiler decides is computed, counted, printed and thrown away.

### Smaller gaps, precisely

- **`bare` does not know about authored density.** It is set from material
  affinity only: `ground.bare = ground.bare.max(1.0 - vegetated)`. Turn a
  document's `vegetation_density` down over meadow soil and the grass thins, but
  `bare` stays at zero — so the fourteen places in `placement.rs` that respond to
  exposed ground never engage. You get lush grass with holes rather than sparse
  grass.
- **Macro shape comes from the grass generator's mound field**, not from the
  document's elevation layers. A soil plate at close framing has a swell across
  it that nothing authored.
- **Two soils that are one soil.** `meadow_floor` and `compacted_loam` differ by
  4.4× in brightness and are physically the same earth in two lighting
  conditions. The canopy darkening is *also* being produced by Cycles. This is
  the same error the profile system exists to prevent, committed inside the
  profile system.
- **The corpus builder is gone.** `terrain_dataset` kept `RenderPair`,
  `ShardManifest`, `ShardLayout` and `checksum`. The job that filled a shard went
  with the rasteriser, because its input side rasterised.
- **`write_package` is off the active path.** The generic scene package —
  `GeometryManifest`, `PrototypeManifest`, `MaterialBindingManifest` — exists and
  is tested and nothing production calls it.

### What the current state actually is

**A mud experiment that worked.** It proved the pipeline end to end: an authored
document reaches a path tracer as geometry, with a ragged material boundary that
the grass and the ground agree about, and a soil whose look is an asset rather
than a shader literal. That was the thing to prove. It is proved.

It is not yet a meadow generator, because a meadow has flowers in it.

---

## 3. The next version: the meadow tier

One phase, one goal: **everything that grows in a meadow, rendered, with the
things on the ground and the things growing around them agreeing.**

Explicitly *before* any neural work. The reason is stated in §4.

### 3.1 Marks must reach Cycles

The blocking item. `terrain_scene` already has the vocabulary — `RibbonMark`,
`CurveMark`, `AnalyticMark`, `StampMark` — and `terrain_cycles::package` already
has the manifest shapes. What does not exist is the path from a compiled
`TerrainScene` to Cycles geometry alongside the tuned blade curves.

Two options, and the choice matters:

**(a) Extend the active `CyclesScene`.** Add instance buffers beside the blade
buffers. Lower risk, keeps the tuned grass untouched, and leaves two export paths
in the tree.

**(b) Move production onto `write_package`.** Correct destination, and it cannot
happen until the package can carry the exact tuned blade vocabulary without
losing quality. Attempting it as part of the content work would put the grass at
risk, and the grass is the thing that already looks right.

**Recommendation: (a) now, (b) as its own measured migration later.** The
migration is a refactor with a fingerprint test; the content work is not.

### 3.2 Prototypes and instances

Rust decides identity, acceptance, ownership, prototype choice, rotation, tilt,
scale, burial depth and tint. Blender builds each prototype once and instances
the transforms. **Blender creates no terrain randomness** — no Geometry Nodes
scattering, no Poisson distribution, no random material ownership.

A small prototype library, per the reference photographs:

```
rounded clod        fractured clod
flat pebble         elongated pebble
shell fragment      dark organic fragment
```

Instances are sunk partially into the ground so they read as embedded rather than
sprinkled. Density is driven by low-frequency masks, not by a uniform rate — a
field uniformly covered in stones reads as gravel.

### 3.3 Grass growing around stones

This is the interesting one and the machinery is already there.

`domain.rs` has `SpacingPolicy::Exclusion { max_radius_m }` with priority-based,
order-independent thinning, and the working area is grown by the maximum
exclusion radius so the answer agrees across joins. What is missing is that the
**tuned generator does not consult it**: `placement.rs` scatters blades from its
own fields and knows nothing about a stone.

The shape of the fix:

1. Stones are accepted first, in a shared candidate domain, with an exclusion
   radius proportional to their own radius.
2. The tuned generator's blade acceptance takes an occupancy field derived from
   the accepted stones.
3. Blades near a stone are not merely deleted — they lean away and shorten,
   which is what real grass at the foot of a rock does and what makes the stone
   look like it has been there a while.

The third point is what separates this from a hole cut in a lawn. It is also why
this belongs to the tuned generator rather than to a post-process.

### 3.4 The content list

| Content | What it is | Where it comes from |
| --- | --- | --- |
| Flowers | stem, head, a few petals; sparse; clustered by a low-frequency mask | new geometry recipe |
| Stones | prototype instances, buried, with grass responding | new, plus the exclusion wiring |
| Undergrowth | broader, lower, denser-leaved plants below the canopy | new geometry recipe |
| Broadleaf | already grows — needs the *document* to be able to ask for more or less | expose through the overlay |
| Thatch | already grows — same | expose through the overlay |

The last two are the cheap wins and should be done first, because they are
plumbing rather than art and they prove the overlay can carry a per-population
abundance rather than a single vegetation density.

### 3.5 The `bare` fix

Small, and directly in the direction of "patchy grass you can see through".
`bare` should rise as authored abundance falls, not only as the material stops
supporting plants. Without it, thinning a meadow by document produces lush grass
with gaps in it instead of sparse grass.

### 3.6 One soil, or two, decided

Either commit to `meadow_floor` and `compacted_loam` being genuinely different
soils — different organic content, and say so in the profiles — or collapse them
to one and let the canopy occlusion produce the darkening. The current position
is neither, and it is double-counting.

This carries real visual risk: the floor value was tuned by eye against renders
that already had the shadowing in them. It needs a before-and-after, not a
refactor.

### Acceptance for the meadow tier

- A document declaring flowers, stones, undergrowth, thatch and broadleaf
  produces all five in a Cycles render.
- Turning any one of them to zero removes it and moves nothing else.
- Grass leans away from stones; stones look embedded, not placed on top.
- Lowering a population's density removes candidates without moving survivors —
  the existing monotonicity property, extended to the new content.
- Two overlapping nine-tile plates agree exactly across the join, for every
  content type.
- `refactor_fingerprints` still passes: the meadow did not move.

---

## 4. What must be true before the neural renderer

Stated as gates, because starting the corpus before these hold means training on
a contract that then changes.

### 4.1 The conditioning contract must be frozen

The neural renderer's input is the low-fidelity matrix. That means somebody has
to decide, and write down, **exactly which planes constitute the input** and what
each one's units and ranges are. Today `TerrainFieldStack` is what the compiler
happens to produce. It needs to be a declared contract with a version number, the
same way a ground profile is.

Candidate input planes, from what already exists:

```
raw substrate weights          elevation and microrelief
canonical state modifiers      slope, curvature, flow, exposure
boundary frames                feature frames (not yet filled in)
```

Plus a derived conditioning stack — realised weights, meso displacement, crack
mask, wet film, cavity, instance occupancy — which is *derived*, never authored,
and is the causal bridge between the matrix and the picture.

### 4.2 The corpus builder must be rebuilt against the matrix

`terrain_dataset` keeps the contracts and lost the job that filled them. The
replacement must pair **the matrix** against a Cycles target, not a cheap picture
against an expensive one. That is a rewrite, not a repair, and it is the single
largest piece of work between here and training.

### 4.3 Determinism has to be provable, not asserted

Same document, seed and world point, identical output. Overlapping lattices
bit-identical. Trace slicing must not move a clod, a crack, a stone or a material
edge. Some of this is tested; the new content will need the same treatment, and
the tests should exist *before* a corpus is generated from it.

Known limit worth writing into the contract: transcendental determinism is
same-platform only. `atan2`, `powf` and the noise are not guaranteed
bit-identical across architectures. Either the corpus is generated on one
architecture or that has to be fixed first.

### 4.4 AOVs

The target side needs more than a beauty pass — albedo, normal, depth, material
weights — or the model has nothing to attribute error to. `RenderProfile` has a
`passes` flag and it is not yet the set a trainer needs.

### 4.5 What the corpus is *for*

Worth stating so the design does not drift: the goal is a renderer that takes the
semantic matrix and produces the picture Cycles would have produced, so that a
map can be authored and seen without a path tracer in the loop. It is **not** an
upscaler and **not** a style transfer. If the input is ever allowed to become a
picture, that is what it will become.

---

## 5. Sequencing

```
now ─── meadow tier ─────────────── contract ──── corpus ──── training
        flowers, stones,            freeze the    rebuild
        undergrowth, exposed        input         against
        thatch/broadleaf,           planes;       the matrix
        grass around stones,        version it
        the `bare` fix
                                    ↑
                        do not start the corpus before here
```

The order matters for one reason: **every content type added after the corpus is
generated invalidates the corpus.** A model trained on a meadow without flowers
does not learn flowers, and the matrix it was conditioned on had no plane for
them. Get the content vocabulary settled first, then freeze, then generate.

---

## 6. Non-goals, and things that must not happen

- **Do not write a second renderer.** This has been done once and cost a day, and
  the removal cost more. If a change needs a picture, it needs Cycles geometry or
  a Cycles material.
- **Do not replace the tuned grass generator.** `placement.rs` is the quality bar.
  A from-scratch generic recipe is a massive visual regression however clean its
  architecture. The way to add semantics is `SemanticOverlay`, which modulates
  only how much grows and whether earth shows, leaving every style field as
  tuned.
- **Do not let Blender decide where anything goes.** Rust owns placement,
  ownership and every transform. Blender instances what it is given.
- **Do not give the ground a mark vocabulary.** Dirt is a continuous field with a
  sparse population on top. Building it a second mark system would mean the
  transition between ground and grass had two systems to reconcile instead of one
  shared candidate field.
- **Do not fold state into material.** Mud is wet loam. The moment there is a
  `mud` profile beside a `loam` profile, nothing can express the ground halfway
  between them.

---

## 7. The shortest honest summary

The pipeline works, the ground is an authored asset with measured numbers, and
the grass is the tuned generator with a document driving how much of it there is.
What is missing is *the rest of the meadow* — and the reason it is missing is a
single unwired connection: the scene compiler places everything and the renderer
never sees it.

Close that, add the three content types that do not exist yet, wire the exclusion
machinery that already exists so grass grows around stones, and the meadow tier
is done. Then freeze the input contract and rebuild the corpus.

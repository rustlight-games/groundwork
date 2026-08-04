# Architecture

## The pipeline

```text
Authored terrain document
    │  parse · migrate · validate                  terrain_format
    ▼
TerrainDocument            semantic, validated, digestible
    │  prepare                                     terrain_core
    ▼
PreparedTerrain            immutable · Send + Sync · sampling cannot fail
    │  sample                                      terrain_core
    ▼
TerrainScene               built once, reused by every renderer
    │                                              terrain_scene
    ├────────► Cycles                              terrain_cycles
    ├────────► cheap rasteriser                    terrain_bake
    ├────────► debug plates                        terrain_bake
    └────────► paired corpus                       terrain_dataset
```

Four decisions are non-negotiable, and the rest of the design is consequences.

## 1. Terrain is a continuous function of world position

Not a grid, not tiles, not pages. Those are ways of *addressing* or *presenting*
terrain; making any of them the identity gives the framework a preferred
resolution and a preferred origin, and then two pages that have never met stop
agreeing along their shared edge.

Three consequences, each of which costs something that is easy to mistake for
waste:

- **World positions are `f64`.** An `f32` at ten kilometres out has about a
  millimetre of spacing — coarser than a close-up render resolves. `f32` only
  after subtracting a stable local origin.
- **Rectangles and cells are half-open**, so tiling is a partition and no
  quantity is computed twice along a seam.
- **Division floors.** Truncation puts −0.5 and +0.5 in the same cell, making
  cell zero twice as wide as every other and drawing a stripe through the world
  origin in every population keyed on it.

## 2. Rust owns semantics and placement; Cycles owns light

Blender receives explicit geometry and never scatters. See
[CYCLES_BACKEND.md](CYCLES_BACKEND.md).

## 3. One scene is reused for every renderer

A training pair must never be produced by generating the terrain twice. The API
makes the wrong thing hard to write: `RenderPair` takes an `Arc<TerrainScene>`
and two closures. See [DATASETS.md](DATASETS.md).

## 4. Composition happens before rendering

Material blending, path depression, vegetation suppression and rock abundance
all affect *procedural decisions* before any RGB exists. See
[MATERIAL_BLENDING.md](MATERIAL_BLENDING.md).

## Why the document is compiled rather than sampled

A `TerrainDocument` is a good thing to author, edit, diff and validate, and a bad
thing to evaluate ten million times: every layer names its source by string, and
every sample would carry the possibility of a reference that does not resolve.

So `prepare` compiles it once. Keys become dense indices, sources become fields,
layers become an evaluation order, and **anything unsupported is rejected before
any sampling happens**.

That last part is the design, not an optimisation. A sampler that can fail is a
sampler whose caller has to decide what to do about a failure ten million times,
inside a scatter, with no sensible answer available. Moving every failure to one
fallible step at the front makes `sample` total.

`PreparedTerrain` is immutable and `Send + Sync`, asserted rather than assumed,
because baking is embarrassingly parallel only for as long as the thing every
thread reads is genuinely read-only.

## Why the scene is a separate thing from the generator

Three consumers read what a generator produces — the cheap rasteriser, the
Cycles exporter, the shadow pass — and none is more canonical than the others. A
description living inside one of its consumers quietly becomes that consumer's
private format.

The scene carries four primitives: ribbons, curves, analytic shapes and stamps,
plus prototype instances. It has no `render_grass` and no `render_wildflowers`,
because a method per content type is a renderer that grows one per ecological
category, each duplicating most of the last.

A mark carries how *old* it is and how *wet* its ground is — intrinsic
properties — and nothing about the current light. That is what lets a scene
survive a lighting change without being regenerated.

## Painter order is semantic and total

```text
 bits 62..64   stratum          ground · canopy · emergent
 bits 22..62   quantised depth  nearer is larger
 bits 16..22   sublayer
 bits  0..16   stable id        the tie-break
```

Never derived from generation order. The tie-break is easy to omit and expensive
to add: a fork's two children and a tuft's blades tie exactly, and without a
deterministic break they resolve by whatever order a threaded sort left them in.

## Two hashes, kept apart

`terrain_core::seed` decides **where things go**. `terrain_core::digest` decides
**whether two things are equal**. They look alike and do opposite jobs: improving
the second is maintenance with no visible consequence; changing the first
relocates every plant in every world.

Merged, the first kind of change silently becomes the second. Separate modules,
separate version constants.

## Randomness is addressed

```text
seed algorithm version · root seed · recipe version · population key
    · integer world cell · candidate rank · named stream · child path
```

Every one knowable without having generated anything else. Streams are *named*
rather than positional, which costs a hash per draw and buys the property that
inserting a `sway` between `bend` and `twist` does not move every subsequent
parameter of every mark in the world.

## Candidates, not counts

A recipe does not decide how many things to make. It walks the candidates its
cells offer — each with an identity that exists whether or not anything grows
there — and accepts or rejects each. So an abundance change moves the acceptance
rate and none of the survivors.

That is also the mechanism that will let two materials share one candidate field
instead of each generating a full set and doubling the marks through a
transition.

## Crate boundaries

```text
terrain_core        ← nothing but serde
terrain_format      ← terrain_core
terrain_scene       ← terrain_core
terrain_generators  ← terrain_core, terrain_scene
terrain_bake        ← terrain_core, terrain_generators
terrain_cycles      ← terrain_core, terrain_generators, terrain_scene
terrain_dataset     ← terrain_bake, terrain_cycles
terrain_bench       ← terrain_core, terrain_generators, terrain_scene, terrain_bake
terrain_bevy        ← terrain_core, terrain_generators, terrain_bake, bevy
```

**Only `terrain_bevy` takes Bevy**, and the compiler enforces it rather than a
convention. Everything upstream has to be usable from a command line, a test, a
benchmark and a dataset job.

The boundary that made this possible was splitting the parameter block: the
generator takes a `GrassParams` — which world, how hard to work, where the sun
is, what the grass is made of — and the rasteriser's twenty-three shading terms
stayed with the rasteriser. While placement took the whole block, every module
that decided where a blade goes depended on the module that drew one.

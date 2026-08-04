# Groundwork

A headless terrain compiler and rendering laboratory, in Rust.

You write a terrain document. Groundwork compiles it into an immutable
world-space function, builds one deterministic scene from that function, and
hands the *same* scene to every renderer it has: a path tracer for the picture
you want, a cheap rasteriser for the picture you can afford, and a dataset
exporter that pairs the two.

```text
Authored terrain document
        │  parse · migrate · validate · prepare
        ▼
Immutable PreparedTerrain            a continuous function of world position
        │  material weights · elevation · microrelief · named modifiers
        ▼
Deterministic TerrainScene           built once, reused by every renderer
        │
        ├──────────► Blender Cycles          the master render
        ├──────────► cheap raster preview    the runtime tier
        ├──────────► debug and validation plates
        └──────────► paired neural-training corpus
```

## Why it is shaped like this

The eventual destination is a Bevy game whose ground is rendered by a neural
network — trained to produce the path-traced picture from the cheap one, so a
frame costs milliseconds instead of seconds. Everything here follows from what
that requires.

**A training pair must be one meadow.** If the cheap render and the expensive
render come from two generation passes, the network is learning to hallucinate
rather than to reconstruct, and the failure is silent — the loss simply stops
falling and no image in the corpus looks wrong. So the scene is built once, held,
and rendered twice.

**Rust decides where every blade goes; Cycles only decides what light does.**
Blender receives explicit geometry and never scatters anything itself. That is
what makes the expensive render and the cheap render the same ground, and it is
what makes a training corpus reproducible from a seed rather than from a backup.

**Terrain is a continuous function of world position, in metres.** Not a grid,
not tiles, not pages. Those are ways of addressing or presenting terrain; making
any of them the identity gives the framework a preferred resolution and a
preferred origin, and then two pages that have never met stop agreeing along
their shared edge.

**Randomness is addressed, not drawn.** You never ask for the next number — you
ask for the value at an address built from the population, the world cell, the
candidate's rank and a named stream. A sequential generator makes every value
depend on how many values came before it, which means a page's contents depend
on where its edges were.

## Status

**The pipeline above exists end to end. Most of the content does not.**

This repository began as a game. The game is gone, and the framework around the
grass is built: a document loads, validates, compiles into a sampler, samples
continuously in metres, and produces a scene that Cycles and the rasteriser both
consume.

Working today:

- The authored document: versioned, migratable, validated in one pass with
  paths and did-you-mean suggestions, digested stably.
- `PreparedTerrain`: continuous world-space sampling with normalised material
  weights, elevation, microrelief and declared modifier channels.
- Sources: constant, world-space noise, and spline distance.
- Layers: material, elevation, microrelief and modifier, with smooth-band,
  ramp and threshold profiles.
- A generic scene IR — ribbons, curves, analytic shapes, stamps, instances —
  with a total painter order and an exact fingerprint.
- The Cycles backend: a generic scene package, appearance-key material
  dispatch, tiling, guard bands and the derived trace resolution a wide view
  needs.
- The cheap rasteriser, as the preview tier and the neural network's input.
- Paired dataset export with AOVs, and a shard manifest pinning every version.
- Four population recipes. One is finished.

Not built yet:

- **Raster and shape-distance sources.** They need an image decoder and a
  polygon index; `prepare` refuses them with a message rather than silently
  sampling zero.
- **Finished dirt, wildflowers and rocks.** They validate, emit, and are
  honestly minimal — a wildflower is a curve and a disc.
- **Terrain blending.** Reserved, and deliberately unimplemented: the weights
  compose, and the shared candidate field that stops a transition doubling its
  marks does not exist yet.
- **The neural renderer.** It becomes a consumer of this corpus once the
  input/target contract has stabilised, and it lives in its own repository.

## Running it

```sh
cargo run -p terrain_cli -- --help

# Read a document, and report everything wrong with it in one pass.
cargo run -p terrain_cli -- validate assets/terrain/documents/blend_lab.terrain.ron

# What is the ground here, and why?
cargo run -p terrain_cli -- inspect assets/terrain/documents/blend_lab.terrain.ron --at 0,5

# A plate through the cheap rasteriser: no window, no GPU.
cargo run --release -p terrain_cli -- preview-export --size 1024 --out target/preview.png

# The same ground, path-traced.
cargo run --release -p terrain_cli -- render --size 768 --samples 512 --out target/render.png

# A paired corpus: one scene, two renderers, structural channels beside it.
cargo run --release -p terrain_cli -- dataset --shards 8 --aovs --out target/corpus

# One whole scene at 1920x1080, traced and opened.
./render

# The terrain live, in a window. Pan with WASD, zoom with the wheel,
# 1/2/3 for the close, standard and wide framings.
cargo run --release -p terrain_preview
```

Cycles renders need Blender on the path; `TERRAIN_BLENDER` overrides where it is
found. Pinned to 5.2 LTS.

## Layout

```text
crates/
  terrain_core        coordinates, keys, seeds, digests, the document,
                      validation, PreparedTerrain, sampling, built-in sources
  terrain_format      the versioned file: envelope, migration, RON
  terrain_scene       projection, ground, marks, instances, painter order
  terrain_generators  what grows: fields, candidates, blades, recipes
  terrain_bake        the cheap raster tier, bake requests and manifests
  terrain_cycles      the scene package, the tiled plate driver, Blender
  terrain_dataset     paired renders and shards
  terrain_bevy        page cache, material, plugin — the only Bevy crate
  terrain_bench       seeds, scenarios, metrics, seams, comparison

tools/
  terrain_cli       the `terrain` binary
  terrain_preview   the terrain live, in a window
  blender_cycles    the Blender half of the path tracer (Python, not a crate)
```

Dependencies point downward, and `terrain_core` depends on nothing but serde.
Only `terrain_bevy` links Bevy — the compiler enforces that rather than a
convention, because everything upstream has to be usable from a command line, a
test, a benchmark and a dataset job.

## Measuring it

Substantial work here ends with a before/after table, not a description of the
improvement. A generated world degrades silently: the geometry stays valid and
the output just looks worse, which no correctness test notices.

Three instruments, answering different questions:

- **`cargo test -p terrain_bench --test refactor_fingerprints`** — *is it the same
  meadow?* Hashes the generated scene itself: every mark, its shape, its
  material, and the ground beneath. No renderer in the loop, so it survives a
  refactor that moves the renderer. Runs in a tenth of a second.
- **`cargo bench -p terrain_bake`** — *what did it cost?* Deliberately granular: the
  bake is five stages and each is timed separately, because a single number for
  "a page costs 100 ms" tells an optimiser nothing about which fifth to attack.
- **`grass_snapshot`** — *did the picture move?* Photographs three places at four
  camera heights and compares pixel for pixel against the last accepted set.

A speed improvement bought by generating fewer marks or shorter grass is a
quality-tier change, not an optimisation, so every speed claim carries its
quality counter-metrics: mark count, coverage, detail energy, palette drift,
seam error, and the weakest seed.

## Licence

Not yet chosen.

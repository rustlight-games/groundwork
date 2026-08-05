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

**A render is nine world tiles, and they are a composition, not a boundary.**
The destination is an isometric game, so a render is three by three two-metre
tiles with the middle one as the subject and the eight around it as set
dressing — one continuous scene over all of them plus a halo, never nine scenes
composited. Grass crosses the internal joins, shadows fall across them, and the
measured step across a join is indistinguishable from the step the grass makes
on its own. See [docs/ISOMETRIC_TILES.md](docs/ISOMETRIC_TILES.md).

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
- The isometric nine-tile layout: one resolver both renderers frame from, an
  RGBA silhouette that ends where the tiles do, and a manifest that makes a
  random render reproducible from a seed.
- The Cycles backend: a generic scene package, appearance-key material
  dispatch, tiling, guard bands and the derived trace resolution a wide view
  needs.
- The cheap rasteriser, as the preview tier and the neural network's input.
- Paired dataset export with AOVs, and a shard manifest pinning every version.
- Four population recipes. One is finished.

Not built yet:

- **Elevation, in the layout.** The nine tiles are coplanar: no steps, no cliffs,
  no camera pitch. Deliberately, so a bad result after the isometric change has
  one cause rather than three.

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
# Nine isometric tiles, somewhere new, path-traced. Minutes.
./render

# That world again, exactly. The script prints the command that repeats it.
TERRAIN_SEED=5a17e33b0c9d2f14 ./render

cargo run -p terrain_cli -- --help

# Read a document, and report everything wrong with it in one pass.
cargo run -p terrain_cli -- validate assets/terrain/documents/blend_lab.terrain.ron

# What is the ground here, and why?
cargo run -p terrain_cli -- inspect assets/terrain/documents/blend_lab.terrain.ron --at 0,5

# Compile a document and path-trace it: the production path.
cargo run --release -p terrain_cli -- compile assets/terrain/documents/meadow_path.terrain.ron

# A hand-framed laboratory plate, for a diagnostic that has to be the same
# twice. The layout options and the manual ones refuse each other.
cargo run --release -p terrain_cli -- render --manual --size 1024 --px-per-metre 192

# The terrain live, in a window. Pan with WASD, zoom with the wheel,
# 1/2/3 for the close, standard and wide framings.
cargo run --release -p terrain_preview
```

Every layout render writes four files: the picture as RGBA with everything
outside the diamond transparent, a debug plate with the nine tiles outlined and
labelled, a subject mask, and a manifest naming the seed, the centre tile, the
bounds and the scale.

Cycles renders need Blender on the path; `TERRAIN_BLENDER` overrides where it is
found. Pinned to 5.2 LTS.

## Layout

```text
crates/
  terrain_core        coordinates, keys, seeds, digests, the document,
                      validation, PreparedTerrain, sampling, built-in sources
  terrain_format      the versioned file: envelope, migration, RON
  terrain_scene       projection, tile layout, frame resolver, ground, marks,
                      instances, painter order
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
- **`terrain compile`** — *did the picture move?* Cycles is the only renderer,
  so this is the only thing that produces a picture to compare. It needs
  Blender and it takes minutes, which is why the fingerprint test above exists.
- **`terrain_bench::iso`** — *do the subject and join metrics still measure what
  they claim?* A nine-tile plate is eight ninths set dressing, so every number
  is taken twice: once over the layout and once weighted by the subject mask.
  Checked against synthetic plates — there is no renderer in that crate.

A speed improvement bought by generating fewer marks or shorter grass is a
quality-tier change, not an optimisation, so every speed claim carries its
quality counter-metrics: mark count, coverage, detail energy, palette drift,
seam error, and the weakest seed.

## Licence

Not yet chosen.

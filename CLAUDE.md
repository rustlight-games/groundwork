# CLAUDE.md

The map. Read [AGENTS.md](AGENTS.md) first — it is the governing policy, and
this is where things are.

## The pipeline

```text
Authored terrain document        assets/terrain/documents/*.terrain.ron
    │  parse · migrate · validate            terrain_format
    ▼
TerrainDocument                             terrain_core::document
    │  prepare                               terrain_core::prepare
    ▼
PreparedTerrain          immutable, Send + Sync, sampling cannot fail
    │  sample                                terrain_core::sample
    ▼
TerrainScene             built once, reused by every renderer
    │                                        terrain_scene
    ├────────► Cycles                        terrain_cycles
    ├────────► cheap rasteriser              terrain_bake
    ├────────► debug plates                  terrain_bake
    └────────► paired corpus                 terrain_dataset
```

## Crates

```text
crates/
  terrain_core        coordinates, keys, seeds, digests, the document,
                      validation, PreparedTerrain, sampling, built-in sources
  terrain_format      the versioned file: envelope, raw types, migration, RON
  terrain_scene       projection, tile layout, frame resolver, ground, marks,
                      instances, painter order
  terrain_generators  what grows: fields, candidates, blades, recipes
  terrain_bake        the cheap raster tier, and bake requests and manifests
  terrain_cycles      the scene package, the tiled plate driver, Blender
  terrain_dataset     paired renders and shards
  terrain_bevy        page cache, material, plugin  — the only Bevy crate
  terrain_bench       seeds, scenarios, metrics, seams, comparison, the lab

tools/
  terrain_cli         the `terrain` binary
  terrain_preview     the terrain live, in a window
  blender_cycles      the Blender half (Python, not a crate)
```

Dependencies point downward. `terrain_core` depends on nothing but `serde`.

## Where to look for a thing

| Question | File |
| --- | --- |
| Why is this a half-open rectangle? | `terrain_core/src/coords.rs` |
| Why is randomness addressed? | `terrain_core/src/seed.rs` |
| Why two hashes? | `terrain_core/src/digest.rs` |
| What can an author write? | `terrain_core/src/document.rs` |
| Why is that an error and this a warning? | `terrain_core/src/validate.rs` |
| Why can sampling not fail? | `terrain_core/src/prepare.rs` |
| Why is noise per metre? | `terrain_core/src/sources.rs` |
| Why is the projection a mirror? | `terrain_scene/src/projection.rs` |
| Why nine tiles and not a rectangle? | `terrain_scene/src/layout.rs` |
| Where does 144 px/m come from? | `terrain_scene/src/frame.rs` |
| Why is the ground grid edge-anchored? | `terrain_scene/src/ground.rs` |
| What decides what draws over what? | `terrain_scene/src/mark.rs` |
| Why candidates and not counts? | `terrain_generators/src/population.rs` |
| Why does the exporter not know about grass? | `terrain_cycles/src/export.rs` |
| Why does a shard record all that? | `terrain_dataset/src/shard.rs` |
| Why is a seam measured that way? | `terrain_bench/src/seams.rs` |

Crate-level `//!` docs carry the reasoning. They are written to be read.

## Running things

```sh
cargo run -p terrain_cli -- --help

terrain validate <document>          # every problem, in one pass
terrain inspect  <document> --at U,V # what the ground is, and why

./run                                # nine tiles, cheap tier, seconds
./render                             # the same nine tiles, path-traced
TERRAIN_SEED=5a17e33b ./run          # that world again, exactly

terrain preview-export --seed 7      # the cheap tier, headless
terrain render --samples 512         # nine tiles through Cycles
terrain dataset --shards 8 --aovs
cargo run --release -p terrain_preview
```

A render is **nine world tiles**, three by three, subject in the middle, on a
transparent background. Random by default and reproducible from the manifest
written beside it — see [docs/ISOMETRIC_TILES.md](docs/ISOMETRIC_TILES.md).
`--manual` is the old hand-framed laboratory plate, and the two modes refuse
each other's options rather than picking silently.

`TERRAIN_BLENDER` overrides where Blender is found. Pinned to 5.2 LTS.

## Measurement

Three instruments, answering different questions. Use the right one.

- **`cargo test -p terrain_bench --test refactor_fingerprints`** — *is it the
  same meadow?* Hashes the scene: every mark, its shape, its material, and the
  ground beneath. No renderer in the loop, so it survives a refactor of the
  renderer. A tenth of a second.
- **`cargo bench -p terrain_bake --bench bake`** — *what did it cost?*
  Deliberately granular: the bake is five stages and each is timed separately,
  because one number for "a page costs 100 ms" tells an optimiser nothing about
  which fifth to attack.
- **`grass_snapshot`** — *did the picture move?* Pixel for pixel against the
  last accepted set.
- **`terrain_bench::iso`** — *is the subject any good, and are the joins
  invisible?* Every number twice, once over the layout and once weighted by the
  subject mask, because a nine-tile plate is eight ninths set dressing.

`terrain_bench::SCENARIOS` is the pinned ground. Append only.

## Known gaps

Real, currently true, and worth knowing before tripping over them.

- **`blade_bend` reaches nothing.** Read only by `Mark::shape`, which is never
  called. `blade_bend_reaches_nothing` asserts the gap.
- **`luminance_spread` is a dead column** in the rock metrics: the palette
  applies one hue drift to all three tones, so the spread reads identically for
  every seed.
- **Raster and shape-distance sources are refused by `prepare`**, with a
  message. They need an image decoder and a polygon index. Constants, noise and
  spline distance compile.
- **Three of four population recipes are minimal.** Wildflowers are a curve and
  a disc; rocks and grit are analytic. They validate, emit, and are honestly not
  finished.
- **`path.t_junction` and `path.x_junction` are named and not pinned.** They
  wait on a spline that branches.
- **The grass generator has not yet been rewritten as a `PopulationRecipe`.**
  `terrain_generators::placement` is the original code and `recipes::GrassRecipe`
  is the new interface; the meadow the fingerprints pin comes from the former.
- **No wind, no trampling, no animated crown layer.** The baked surface is
  static.
- **The nine-tile world is flat.** All tile bases are coplanar; no steps, no
  cliffs, no camera pitch. Deliberate, so a bad result after the layout change
  has one possible cause rather than three. Only the seed and the centre tile
  are randomised — sun, camera, framing and style are fixed.
- **`TileLayoutPreset` has one variant.** Twenty-seven tiles is not a number, it
  is a shape nobody has chosen; the layout is a coordinate list precisely so
  choosing one later changes `layout.rs` and nothing downstream.
- **`terrain dataset` still frames by page, not by layout.** It crops square
  patches at a chosen scale, which was right when a render was a rectangle. Once
  the neural renderer's unit is a tile, the corpus should be tile-shaped and
  carry the subject mask — that is a contract change, so it waits on the
  input/target contract settling rather than being done in passing.
- **Terrain blending is reserved, not implemented.** The weights compose; the
  shared candidate field that stops a transition doubling its marks does not
  exist yet. See [docs/MATERIAL_BLENDING.md](docs/MATERIAL_BLENDING.md).

## Content

Everything under `assets/terrain/` is authored. Adding a material or a
population should be a file and a recipe, not a code change.

```text
assets/terrain/
  documents/    constant_grass, blend_lab
  features/     main_path.spline.ron
```

A spline is one `u v` pair per line — not RON, despite the extension. See
`terrain_core::sources::Spline::parse` for why.

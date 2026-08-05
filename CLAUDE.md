# CLAUDE.md

The map. Read [AGENTS.md](AGENTS.md) first — it is the governing policy, and
this is where things are.

## The goal

**One isometric 3×3 tile plate, generated from a top-down semantic matrix and
path-traced by Blender Cycles, with smooth transitions between terrain types and
every level of detail present.**

## Cycles is the only renderer

This framework **builds geometry**. Blender Cycles **renders it**. There is no
second renderer, no cheap tier to fall back to, and no picture produced by any
code in this repository.

That is not a preference, it is the point of the project. The whole reason for a
semantic terrain compiler is to produce geometry good enough for a path tracer;
anything that draws pixels here is a distraction that has to be maintained,
tuned and kept in agreement with the real renderer — and it never is.

The rasterisers that still exist are **being deleted** — see
[issue #1](https://github.com/rustlight-games/groundwork/issues/1). Do not add
features to them, do not tune them, and do not write a new one. If a change
needs a picture, it needs a Cycles material or Cycles geometry.

Low fidelity does **not** mean a small picture. It means the
`TerrainFieldStack` — typed planes with declared units. The neural renderer's
input is that matrix; its target is the Cycles render.

The unit is always nine world tiles — three by three, subject in the middle,
generated as *one continuous scene*. That never changes. See
[docs/LOW_TO_HIGH_FIDELITY_SPEC.md](docs/LOW_TO_HIGH_FIDELITY_SPEC.md) for the
full specification and [docs/references/](docs/references/) for what the output
is aiming at.

High fidelity means *the same* terrain rendered more accurately — never a
second, more detailed terrain.

## Do not reinvent this

Read this before writing a grass generator. The mistake has been made once
already and it cost a day.

**`terrain_generators::{field, placement, scene, stroke, style}`** is the tuned
grass generator: colonies, flow, tillers, statement fields, tuft groups. It is
the quality bar and it feeds Cycles the blade geometry. A from-scratch "generic
tuft recipe" is a massive visual regression, however clean its architecture.

The way to add semantics to it is **not** to replace it. It is
`terrain_generators::field::SemanticOverlay`, which lets an authored document
modulate `Ground::density` and `Ground::bare` — *how much grows* and *whether
earth shows* — while every style field stays exactly as tuned. That boundary is
the whole design: the document owns meaning, the generator owns look.

## The pipeline

```text
Authored terrain document        assets/terrain/documents/*.terrain.ron
    │  parse · migrate · validate            terrain_format
    ▼
TerrainDocument                             terrain_core::document
    │  prepare                               terrain_core::prepare
    ▼
PreparedTerrain          immutable, Send + Sync, sampling cannot fail
    │  sample onto an edge-anchored lattice  terrain_scene::derive
    ▼
TerrainFieldStack        THE low-fidelity matrix   terrain_scene::field
    │  derive slope, curvature, flow, exposure, boundary frames
    │  realise the ragged boundary           terrain_generators::transition
    │  shared candidates → ownership          terrain_generators::{domain, ownership}
    ▼
SemanticOverlay ──► the tuned generator ──► geometry ──► Cycles   ← the render
```

Geometry means blade curves, a displaced ground mesh carrying clod relief, and a
per-vertex `earth` attribute so one ground material can shade a meadow floor and
a bare track without splitting the mesh at the boundary.

## Crates

```text
crates/
  terrain_core        coordinates, keys, seeds, digests, the document,
                      validation, PreparedTerrain, sampling, built-in sources
  terrain_format      the versioned file: envelope, raw types, migration, RON
  terrain_scene       projection, tile layout, frame resolver, marks, instances,
                      painter order, AND the field stack + derived fields
  terrain_generators  what grows: the tuned grass generator, the semantic
                      overlay, candidate domains, the transition solver,
                      ownership, the scene compiler, content families
  terrain_bake        the tuned painterly raster tier, and bake requests
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
| Why can sampling not fail? | `terrain_core/src/prepare.rs` |
| **What is the low-fidelity matrix?** | `terrain_scene/src/field.rs` |
| **Why is the lattice addressed by integer?** | `terrain_scene/src/field.rs` |
| **Where do slope, flow and curvature come from?** | `terrain_scene/src/derive.rs` |
| **Why does a boundary look ragged?** | `terrain_generators/src/transition.rs` |
| **Why does a transition not double its density?** | `terrain_generators/src/domain.rs` |
| **Who decides what a candidate becomes?** | `terrain_generators/src/ownership.rs` |
| **How does a document reach the tuned generator?** | `terrain_generators/src/field.rs` — `SemanticOverlay` |
| Document → one scene | `terrain_generators/src/compiler.rs` |
| Why is the projection a mirror? | `terrain_scene/src/projection.rs` |
| Why nine tiles and not a rectangle? | `terrain_scene/src/layout.rs` |
| Where does 144 px/m come from? | `terrain_scene/src/frame.rs` |
| What decides what draws over what? | `terrain_scene/src/mark.rs` |
| **What colour is earth, and how was that measured?** | `terrain_bake/src/palette.rs` — `EARTH` |
| Why does the exporter not know about grass? | `terrain_cycles/src/export.rs` |
| Why does a shard record all that? | `terrain_dataset/src/shard.rs` |

Crate-level `//!` docs carry the reasoning. They are written to be read.

## Running things

```sh
cargo run -p terrain_cli -- --help

terrain validate <document>          # every problem, in one pass
terrain inspect  <document> --at U,V # what the ground is, and why

# The production path: document → matrix → candidates → tuned render.
terrain compile assets/terrain/documents/meadow_path.terrain.ron \
  --seed 5a17e33b0c9d2f14 --centre-tile=0,0 --samples 128 --out target/plate.png

./render                             # nine tiles of the laboratory meadow
```

`terrain compile` reads a document and path-traces it. That is the pipeline.
`preview-export` and `./run` render the laboratory meadow through the old
rasteriser and are on their way out with issue #1.

`TERRAIN_BLENDER` overrides where Blender is found. Pinned to 5.2 LTS.

## Content

Everything under `assets/terrain/` is authored.

```text
assets/terrain/
  documents/    constant_grass, blend_lab, meadow_path
  features/     main_path.spline.ron
```

- **`constant_grass`** — the base case: one material, everywhere.
- **`blend_lab`** — four layers reading one spline. Note that it tops out at an
  *even split*: its base grass claims the ground with `Replace` and the path adds
  a dirt score of one, so its path centre normalises to 0.5/0.5 and it cannot
  express bare ground. That is a property of the document, not of the sampler.
- **`meadow_path`** — the one that does. Its path uses `Replace`, so one band
  sweeps pure meadow to bare earth, and six layers read the spline at six
  different widths.

## Measurement

- **`cargo test -p terrain_bench --test refactor_fingerprints`** — *is it the
  same meadow?* No renderer in the loop. A tenth of a second.
- **`cargo bench -p terrain_bake --bench bake`** — *what did it cost?*
- **`grass_snapshot`** — *did the picture move?*
- **`terrain_bench::iso`** — *is the subject any good, and are the joins
  invisible?*

`terrain_bench::SCENARIOS` is the pinned ground. Append only.

## Known gaps

Real, currently true, and worth knowing before tripping over them.

- **The scene compiler's own content families are not the tuned grass.**
  `terrain_generators::families` emits tufts, undergrowth, thatch, flowers,
  stones and clods into a `TerrainScene`, and `terrain compile` does *not* draw
  them — it renders through the tuned generator with a `SemanticOverlay`. The
  families exist for the Cycles path and the corpus, and their look is nowhere
  near `placement.rs`. Do not wire them into the cheap picture.
- **Bare earth has clods but no cracks or pebbles.** `WorldField::earth_relief`
  displaces the ground mesh at the measured scales — clods 2–8 cm, crumb 2–15 mm
  — scaled down by compaction, so a packed track is smooth and its shoulders are
  cloddy. Not there yet: desiccation polygons (5–25 cm), embedded pebbles, and
  wheel or foot ruts.
- **`blade_bend` reaches nothing.** Read only by `Mark::shape`, never called.
- **`luminance_spread` is a dead column** in the rock metrics.
- **Raster sources are refused by `prepare`**, with a message. Spline distance,
  constants and noise compile.
- **Cycles still renders through `GrassScene`**, not the generic package.
  `write_package` exists and is not on the active path.
- **`terrain dataset` still frames by page, not by layout.**
- **The world is flat.** All tile bases are coplanar; no steps, no cliffs, no
  camera pitch.
- **Transcendental determinism is same-platform only.** `atan2`, `powf` and the
  noise are not guaranteed bit-identical across architectures.
- **Snow, and covers generally, are types without a solver.** `CoverPlane`
  exists; nothing fills one.

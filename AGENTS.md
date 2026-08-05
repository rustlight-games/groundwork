@/Users/gpriday/.codex/RTK.md

# Groundwork — agent guide

The governing policy. [CLAUDE.md](CLAUDE.md) is the map of where things are;
this is what must stay true while you move them.

## What this is

A headless terrain compiler and rendering laboratory. An authored document is
parsed, migrated, validated and compiled into an immutable world-space function;
one deterministic scene is built from that function; and that *same* scene is
handed to a path tracer, a cheap rasteriser, and a dataset exporter.

The eventual consumer is a neural renderer inside a Bevy game — trained to
produce the path-traced picture from the cheap one. Everything below follows
from what that requires.

## The ten invariants

**1. Semantic terrain and candidate placement are deterministic.**
The same document, seed and world position produce the same answer on any
machine, in any process, at any tiling. This is not a nicety: it is what lets
two pages that have never met agree along a shared edge, and what lets a
training crop be the same ground as the render it came from.

**2. Randomness is addressed, never drawn.**
You do not ask for the next number; you ask for the value at an address built
from the population, the world cell, the candidate's rank and a *named* stream.
A sequential generator makes every value depend on how many came before it, so
skipping a candidate shifts everything after it and baking the same ground
inside a different rectangle gives a different meadow. A new decision means a
new stream name, never a new positional draw.

**3. The terrain core has no Bevy dependency.**
Only `terrain_bevy` and `terrain_preview` link it. Everything upstream must be
usable from a command line, a test, a benchmark and a dataset job, none of which
want a window.

**4. Blender never decides terrain placement.**
Cycles receives explicit geometry and owns light transport, materials, sampling
and output. It never scatters. Blender's own scattering would break invariant 1
quietly — the seam would appear, nothing would report it, and the cause would be
in a different language from the symptom.

**5. A training pair always uses one `TerrainScene`.**
Built once, held, rendered twice. `RenderPair` enforces this with the type:
there is no constructor that takes two scenes. Generating twice would agree
today; the point is that nothing can *later* make it disagree, and the failure
is silent — the loss stops falling and no image in the corpus looks wrong.

**6. Render pages are disposable derivatives, not terrain state.**
A plate is one logical output; a page is a storage unit within one; a trace tile
is a slice of a plate small enough for Blender to hold. None of the three is a
**world tile** — see invariant 10. All of them can be deleted and rebuilt from a
document and a seed. Nothing about the terrain lives only in a cache.

**7. Material blending affects procedural ownership before rendering.**
Never alpha-blend two finished images. Compose *material weights*, then let one
shared candidate field decide ownership. Two renders blended after the fact give
transparent grass ghosts, double mark density and muddy colour — see
[docs/MATERIAL_BLENDING.md](docs/MATERIAL_BLENDING.md).

**8. Substantial changes require benchmark and visual evidence.**
A before/after table, not a description. A generated world degrades silently:
the geometry stays valid and the output just looks worse, which no correctness
test notices. Every speed claim carries its quality counter-metrics — mark
count, coverage, detail energy, palette drift, seam error, weakest seed. A
speed-up bought by generating fewer marks is a quality-tier change.

**9. Documentation describes the current architecture only.**
No "this used to be", no aspirational present tense. A doc that describes what
is planned as though it exists costs more than no doc.

**10. World tiles are a composition, never a generation boundary.**
A render is nine tiles of *one continuous scene*, framed by one camera, produced
by one pass. Never nine scenes, never nine images composited, never a lower
quality or density outside the subject. Generating per tile would put a join at
every internal edge, stop every shadow at a tile boundary, and — worse for what
this exists for — make the context systematically different from the subject one
tile from the middle of every frame, which is precisely the artefact a neural
renderer learns in preference to learning grass. The tiles decide what the render
is *about*; they never decide what gets generated. See
[docs/ISOMETRIC_TILES.md](docs/ISOMETRIC_TILES.md).

## Rules that follow

- **Metres are the unit.** World positions are `f64`; `f32` only after
  subtracting a stable local origin. Rectangles and cells are half-open.
  Division floors. See `terrain_core::coords`.
- **Identity is a string the author chose**, never a number derived from file
  order. Dense indices exist only inside `PreparedTerrain`.
- **Two hashes, kept apart.** `seed` decides where things go; `digest` decides
  whether two things are equal. Merging them would make a maintenance change to
  the second relocate every plant in the world.
- **Validation collects.** One rename breaks several references; reporting them
  one rebuild at a time makes a rename cost as much as the damage it did.
- **Unknown fields in a document are errors.** A misspelled parameter that
  silently does nothing is the worst failure authored content has.
- **Recipes emit primitives**, never content types. There is no
  `render_wildflowers` anywhere.
- **Registration is explicit.** Link-order-dependent registration means the same
  document produces different terrain depending on how the binary was built.
- `SEED_ALGORITHM_VERSION`, `DIGEST_ALGORITHM_VERSION`, `GENERATOR_VERSION`,
  `PACKAGE_VERSION`, `SEEDS`, `SCENARIOS` and committed baselines change only
  deliberately, in the same commit as the change that caused them, with the
  reason in the message.
- **Never add AI attribution**, generated-by comments, or `Co-Authored-By`
  trailers.

## Verifying a change

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all -- --check

# Is it the same meadow? A tenth of a second, no renderer in the loop.
cargo test -p terrain_bench --test refactor_fingerprints

# Did the picture move? Cycles is the only renderer, so this is the only
# thing that answers it. Minutes, and it needs Blender.
cargo run --release -p terrain_cli -- compile assets/terrain/documents/meadow_path.terrain.ron

# Does the document still mean what it says?
cargo run -p terrain_cli -- validate assets/terrain/documents/blend_lab.terrain.ron
cargo run -p terrain_cli -- inspect assets/terrain/documents/blend_lab.terrain.ron --at 0,5
```

Always benchmark in `--release`, and never while another build competes for
cores.

## When you find something wrong

Two dead parameters surfaced during the migration: `blade_bend`, which reaches
nothing, and the rock palette's `luminance_spread`, which reads the same value
for all ten seeds. Both are documented by a test that **asserts the gap rather
than the fix**, so the next person is told it is known and whoever fixes it is
told to delete a test.

Do that rather than quietly repairing it, when repairing it would change the
output. A change whose claim is "this moved nothing" must not smuggle a look
change into itself.

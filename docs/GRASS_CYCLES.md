# The grass renderer

**Rust builds the scene. Cycles renders it.** That line is the whole design, and
everything below is a consequence of where it falls.

```text
world fields ─→ colonies ─→ tufts ─→ blades        │  curves in world metres
   field.rs     placement.rs        stroke.rs      │        ↓
                                                   │  headless Blender
   ─────────────── Rust ───────────────────────────┤        ↓
                                                   │  path-traced beauty + AOVs
                                                   │     cycles.rs · render.py
```

## Why the line falls there

| Rust keeps | Cycles gets |
| --- | --- |
| Where every blade is, and why | How light reaches it |
| Guard bands, seams, page independence | Shadows, occlusion, scattering |
| Stable per-call-site random streams | Denoising, sampling, devices |
| The world being a pure function of a coordinate | — |

**Placement never crosses.** Two pages that have never met agree along a shared
edge only because every placement decision is a pure function of a world
coordinate. Let Blender's own scattering decide where grass goes and the world
becomes a finite set of tiles with blend masks, which is a different game and
not one this project is playing. Cycles is handed an explicit list of curves and
has no opinion about where they came from.

**Light transport never stays.** The renderer this replaced had five separate
terms describing darkness — horizon occlusion, optical occlusion, an interior
density, a micro-occlusion and a shade depth — because a rasteriser cannot
integrate a hemisphere and each was an approximation of some part of doing so.
They interacted, so tuning one moved the others, and a whole phase of work went
on subtracting them from each other. A path tracer computes the quantity they
were approximating.

The second reason matters more as the project grows: light transport written by
hand costs *O(surfaces)*. Rock, sand, snow, bark and moss each want their own
geometry vocabulary **and** their own approximations. Cycles is *O(1)* engine
plus content per surface, so a new material is a shader rather than a new
integrator.

## Running it

```sh
./render                      # one whole scene, path-traced, 1920x1080
BW_SAMPLES=512 ./render       # cleaner, slower
BW_DETAIL=96 ./render         # pixels per metre; lower shows more ground
BW_SEED=12 ./render           # somewhere else entirely

./run                         # the game. Rasterised, interactive
BW_GRASS_TRACED=1 ./run       # read traced pages where they have been baked

cargo run --release -p bw_grass --example grass_prebake     # fill that cache
cargo run --release -p bw_grass --example grass_dataset     # training corpus
cargo run --release -p bw_grass --example grass_critique -- plate.png \
    --target docs/art/grass-target.png                      # the look gate
```

`BW_BLENDER` overrides where Blender is found. Pinned to 5.2 LTS; the compositor
and `ShaderNodeMix` socket layouts both changed in the 4.x → 5.x window, so a
different build is not assumed to work.

## Three things about the camera that are not obvious

**The projection is orthogonal but anisotropic.** `iso::project` is
`screen.x = X − Y` and `screen.y = −(X + Y)/2 + Z`; as dot products those are
`r = (1, −1, 0)` and `u = (−½, −½, 1)`. They are perpendicular, so it really is
an orthographic view down the isometric axis. But `|r| = √2` against `|u| = √3/2`,
and that `2/√3` is the entire difference between the game's 2:1 dimetric diamond
and true isometric. No camera transform expresses it — Blender carries it as a
non-square pixel, and the factor falls out independent of resolution, which is
the check that it belongs to the projection rather than to the render size.

**The projection is also a mirror.** Taken at face value the basis gives
`r × u = −(1,1,1)/√3`, which puts the camera *under the ground looking up*. A
real overhead camera sends `+X` left; `screen.x = X − Y` sends it right, and no
rotation joins them. The game's projection is left-handed, which is normal for a
tile-based isometric game and self-consistent because everything in the game
lives inside it. So the *world* is reflected across `x = y` on the way out
instead, which turns the mirrored view into a physical one exactly. Everything
crossing the boundary goes through the same swap, **including the sun's
bearing** — a field reflected while its sun was not would be lit from the wrong
side and would look entirely plausible.

**Blades must be traced above the resolution they are stored at.** A grass blade
is a few millimetres wide, so at the authoring scale of 96 px/m it is under half
a pixel. The rasteriser copes because a mark has a minimum width by
construction; a triangle does not, and geometry thinner than a pixel does not
become a fine blade — it becomes a partially covered pixel, which at canopy
density averages the whole field into a flat wash. `grass_prebake` traces at 3×
and box-filters down.

## Two things about the geometry that cost a render each

**Cycles curve primitives cannot light a blade.** A `RIBBONS` curve is a
camera-facing quad whose shading normal is derived to face the viewer, so every
blade in the field presents the same normal to the sun and the canopy shades
uniformly flat. It is a property of the primitive and cannot be overridden.
Blades are real mesh ribbons: three vertices per rib, the middle standing proud
by `geometry::RIDGE` of the half-width. The fold is what puts a lit side and a
shaded side inside one blade.

**`ShaderNodeMix` has duplicate socket names.** It carries one A/B/Result triple
per data type and they all share their names, so `inputs["A"]` returns the
`VALUE` socket at index two — which is *disabled* whenever the node is set to
RGBA. Linking to it is not an error and draws no warning; the link simply does
nothing. Two material features were wired to dead sockets and had been
evaluating nothing at all. `render.py`'s `live()` picks the enabled socket by
name, and nothing in that file should use `inputs[...]` on a Mix node again.

## Measuring it

Two instruments, and they answer opposite questions.

`compare` asks **did the picture move**, against our own last output. Right gate
for an optimisation, useless for a deliberate look change — the answer is always
"completely".

`critique` asks **what is the picture**, in numbers computable for reference art
and our own bake alike, needing no pixel correspondence. Six gated bands,
centred on real measurements of `docs/art/grass-target.png`:

| | Current | Target | Band |
| --- | ---: | ---: | --- |
| median luminance | 0.068 | 0.060 | 0.042–0.080 |
| deep shadow L\*<20 | 27.2% | 22.1% | 15–32% |
| highlight L\*>55 | 9.6% | 7.2% | 4–10.5% |
| coherence @32px | 0.502 | 0.504 | 0.36–0.62 |
| Lab chroma | 32.7 | 38.9 | 32–46 |
| highlight chroma | 48.3 | 58.7 | 30–58 |

The pair doing the most work is median luminance against deep-shadow share.
Grading a plate down satisfies the second and immediately breaks the first, so
the only way to hold both is to put the darkness where darkness belongs — in
occlusion and cast shadow — rather than in the exposure.

Reported but **not** gated: gradient energy, detail energy, hue spread, hue mean,
clipping. A band on gradient energy would be a band on how many blades to draw.
Hue spread currently sits at 4° against the reference's 7°, and is left there
deliberately: the remaining variation would have to be bimodal, which reads as
two species rather than one species in several moods.

### A trap worth not falling into twice

**A small plate is not a valid sample.** A 320-pixel page at 192 px/m covers
1.67 metres of ground and passed bands that the 768-pixel plate failed. The gate
is only meaningful on a plate big enough to hold several colonies.

## What is still open

- **Hue spread**, above. Reported, not gated, and a deliberate stop.
- **A persistent Blender worker.** Startup is several seconds and a page traces
  in about one, so `grass_prebake` spends most of its life starting processes.
  `render.py` already accepts a manifest of many pages in one invocation; the
  pre-baker does not use it yet.
- **Cycles AOVs.** `dataset.rs` still exports the *rasteriser's* `Passes`
  alongside the traced target. Cycles' own render passes and cryptomatte would
  give per-blade IDs and physically consistent channels by configuration rather
  than by hand-plumbing ten of them.
- **The rasteriser's dead light transport.** `lighting.rs`, `shadow.rs` and the
  five darkness terms in `bake.rs` now serve only the cheap input tier. They are
  not wrong, but they are no longer the way the grass is meant to look, and the
  crate docs in several places still read as though they are.

# The grass reference renderer

The grass was built against a real-time budget: bake a page in tens of
milliseconds on a background thread, stream it, draw it once. That constraint is
gone. The surface is now the **training target for a neural renderer**, so the
expensive path no longer has to run at frame rate — it has to be *right*,
deterministic, and reproducible.

**Status: built.** All eight phases below are in the binary. This document
describes what exists and why; the phase headings are kept because each one
names a decision that is expensive to revisit, not because any of them is
outstanding.

```sh
cargo run --release -p bw_grass --example grass_lab      # the laboratory plate
cargo run --release -p bw_grass --example grass_lab -- --sweep
cargo run --release -p bw_grass --example grass_bake -- --quality reference
cargo run --release -p bw_grass --example grass_dataset -- --shards 64 --aovs
```

## What it costs

One 256-pixel page at the authoring scale, single-threaded, on a quiet machine:

| Tier | Per page | Supersample | Shadows | Sun samples | Occlusion |
| --- | ---: | ---: | ---: | ---: | ---: |
| `Preview` | 250 ms | 3× | none | — | blur difference |
| `Dataset` | 1.00 s | 4× | 3× density | 4 | 8 directions |
| `Reference` | 4.40 s | 4× | 4× density | 12 | 16 directions |

Against 65 ms before this work, at a quality the tiers did not previously
distinguish. A four-shard paired corpus with every auxiliary pass takes about a
second a shard on sixteen cores, so a four-thousand-shard corpus is minutes.

## What was actually missing

Three things separated the plate from the target art, in order of weight. All
three are addressed; the diagnosis is kept because it is the reasoning the
design rests on.

**There are no normals anywhere in the system.** Every lighting term in the
baker is derived from a height field: `Ground::lit` is a scalar dome-facing
term, `canopy_relief` is a blurred height difference, `directional_shadow` is a
march over canopy heights. Nothing in the crate knows which way a surface faces.
That is why the plate has tone variation but no *form* — a blade has no lit edge
and shaded edge, only `side_light: 0.118` applied through a pseudo-cylindrical
approximation across a one-dimensional rib (`stroke.rs`). Everything about
"one side receives more light" is upstream of cast shadows and has to land
first.

**The palette cannot render a shadow.** `GRASS[0]` is 0.22 luminance and
`THATCH[0]` is 0.16. The ramp is a percentile map of a painting that contains no
shadows, `palette.rs` says so in as many words, and `BROAD_DARK = 0.58` halves
every broad dark term on top of that. A physically-derived shadow looked up in
that ramp comes back as a slightly duller green. **The palette work is therefore
not cosmetic and cannot be deferred to the end** — build shadows against an
unchanged ramp and they will be evaluated through something incapable of showing
them.

**A mark is a stroke, not a leaf.** `Stroke` is a centreline plus a scalar width
profile, rasterised as a one-dimensional perpendicular rib. There is no
cross-section, no twist, and no branch, so split tips are not expressible
without new geometry.

Everything else is closer than it looks. The field already carries four spatial
scales, placement is already a pure function of world coordinates, and
compositing is already a depth test rather than alpha-over.

### The layer that is genuinely absent

Of the four layers the target needs — ground, dirt, fine grass, tufts — three
exist. The **fine grass layer does not**. Today's mat (`thatch: 395/m²`) is drawn
to be buried and shaded through `Tone::Thatch`, so it reads as floor rather than
as grass. The reference art's dominant texture is thousands of short, strongly
combed blades forming a *closed visible canopy* underneath the statement tufts.

That layer goes in early, because it sets the density everything else is tuned
against. It is also the one place blade count should rise: fine blades up two to
three times, statement blades roughly flat. Raising both is how a field starts
reading as fur.

## Decisions taken

| Decision | Value | Consequence |
| --- | --- | --- |
| Lowest supported sun elevation | **35°** | Shadow reach 1.43 × canopy height; scene footprint grows about half again |
| The streaming runtime (`plugin.rs`) | **Preview quality only** | No `Page::for_view` wiring, no atlas, no mip chain — the reference path is free of every real-time constraint |

The second deliberately deprioritises what used to be called the single largest
remaining win in this crate. It stops being a win when the runtime it optimises
is no longer the product.

### Elevation is a world angle, and image `+Z` is not up

Worth its own heading because it cost a day and would cost anyone else the same.
The renderer authors its key light in *image* coordinates where `+Z` points at
the **viewer**, and this camera looks down at 35° — so a light built as
`(plane · cos θ, sin θ)` sits nowhere near `θ` above the horizon. A "35° sun"
constructed that way measured 55°, and the shadow guard band, which is sized
from one over the tangent of the elevation, came out a third short of what the
field actually casts.

`iso::image_to_world` is the bridge and carries the warning; `lab::Key` places
the sun in the world and solves for the image vector that keeps the screen
bearing. Both have tests that pin the trap rather than the fix.

## Architecture

The change that makes everything else possible is inserting an explicit scene
between placement and rasterisation.

```text
WorldField                    + analytic ground normal, + morphology fields
      │
      ▼
GrassScene                    patches → tuft groups → blade ribbons
      │                       (a VALUE — built once, rendered many times)
      ▼
BakeRegion                    N×N pages + guards, cropped after
      ├── light-space depth       one per sun sample, reused
      ├── camera G-buffer         depth, normal, material, s, thickness,
      │                           side, tuft id, coverage, optical depth
      └── canopy height / horizon AO scans
      │
      ▼
lighting                      three normals: ground → tuft envelope → blade
      │
      ▼
resolve                       multi-axis LUT, glaze, edge-aware soften,
                              high-quality downsample
      │
      ▼
RGB + AOVs
```

Module split:

```text
field.rs        continuous world fields          (exists, extended)
placement.rs    patch, group and candidate placement
geometry.rs     blade ribbons, cross-section, twist, tips
scene.rs        GrassScene, BakeRegion
raster.rs       the deterministic CPU rasteriser  (Painter, reduced in scope)
shadow.rs       light-space projection and visibility
lighting.rs     local and macro illumination
resolve.rs      palette, glaze, downsample        (split out of bake.rs)
surface.rs      G-buffer and accumulation buffers (exists, widened)
```

`Painter::draw` currently combines shape generation, projection, shading and
rasterisation. Splitting them is the load-bearing refactor: it is what lets the
shadow pass see the *same* geometry the camera pass sees, and it is the same
property that makes paired low-resolution input and high-resolution target come
from one scene — which the neural training depends on absolutely. Generating the
scene twice, even deterministically, is both twice the expensive work and a
standing invitation for the two to drift apart after a later edit.

The existing `Mark` vocabulary survives unchanged as **centreline morphology
presets**. `Dash`, `Kink`, `Sway`, `Hook`, `Broad`, `Tangle`, `Fleck` and
`Buried` are the right set and were expensive to arrive at.

## Quality tiers

```rust
pub enum GrassRenderQuality { Preview, Dataset, Reference }
```

| | Preview | Dataset | Reference |
| --- | ---: | ---: | ---: |
| Supersample | 3 (current) | 4 | 4 |
| Shadow map density | none | 3× final | 4× final |
| Sun samples | 0 | 4 | 8–16 |
| Horizon AO directions | approximation | 8 | 16–24 |
| Parent blade segments | adaptive (current) | 8–12 | 12–20 |
| Fork child segments | collapsed | 3–5 | 5–8 |
| Output | RGB | RGB + primary AOVs | full AOV set |

Preview is what `plugin.rs` runs and what keeps the game launchable. It is not
held to the reference look.

## The shadow guard band

This is the highest-risk item in the plan, because an insufficient shadow guard
is invisible in every still that does not contain a page join, and shows up
later as a straight line down the world.

At 35° elevation, holding the existing plane azimuth of `LIGHT_PLANE`:

```text
light      = (-0.5932, -0.5649, 0.5736)      |plane| = 0.8192, z = 0.5736
reach/height = |L.xy| / L.z                  = 1.4281
```

Shadows fall *away* from the light. The light plane points up and to the left in
image space, so a caster that can shade the page lies **above and to the left of
it**, and the guard grows on those two edges only:

```text
guard_left  = SIDE  + shadow_reach × 0.7241
guard_above = ABOVE + shadow_reach × 0.6897
guard_right = SIDE                              (unchanged)
guard_below = BELOW                             (unchanged)

shadow_reach = max_canopy_height × 1.4281
             + penumbra radius
             + PCF radius
             + downsample support
```

With a measured max canopy near 90 reference pixels that is roughly 150 pixels
of shadow guard, taking `footprint()` from 500 × 458 to about 609 × 562 — a
**1.49× scene-construction cost**, paid in field sampling and geometry building
rather than in camera rasterisation, since `paint()`'s per-stroke reach test
rejects those marks from the camera pass cheaply.

Three rules, all of which the existing `MARGIN` discipline already establishes
and which must be carried over rather than reinvented:

1. **Derive the guard from the light, never write it as a constant.** It is a
   function of elevation, and elevation is now a parameter.
2. **Measure the maximum canopy height, do not assume it.** `CANOPY_CEILING = 48`
   is a shading normaliser, not a bound — the tallest mark the vocabulary can
   grow reaches roughly twice that. The same mistake once had a guard-band test
   certifying a band against a mark seven percent shorter than the baker grows.
3. **Sweep the test, do not reason about it.** Extend
   `the_guard_band_covers_the_longest_mark_the_field_can_grow` across mark
   families × vigour ceiling × fork reach × minimum sun elevation × page scales,
   and add a companion that checks two adjacent pages have no step in shadow
   visibility, not just in colour.

## Determinism

Every new decision needs its own `Stream` variant. The rule in `rng.rs` is
absolute and the reason is stated there: reusing a stream correlates two fields
and the eye finds the rule immediately.

New variants: `TuftGroup`, `Maturity`, `Moisture`, `Exposure`, `Twist`,
`Fork`, `ForkGeometry`, `Ridge`, `Underside`, `Glint`, `Fine`.

There is a second, subtler hazard. Adding a `draw.range()` call in the *middle*
of an existing sequence — inside `Mark::shape`, say — reshuffles every draw
after it and therefore the whole world. During a deliberate look change that is
acceptable exactly once. So:

> **Add every new stream in one commit at the start of Phase 2, accept the
> single reshuffle, and afterwards derive new decisions from `(world cell, tuft
> id, blade id, named stream)` rather than by appending to a sequence.**

The soft-sun sample directions must come from a globally fixed low-discrepancy
sequence anchored in world space, never from a page-local draw. A shadow pass
whose sample pattern depends on which page it is in produces a visible page
grid under a soft sun.

## Phases

### Phase 0 — Freeze and fixture

Save the current output under `legacy-b292254`. Add `GrassRenderQuality`. Build
the **one square metre laboratory plate**: one fine tuft, one mature broad tuft,
several twisted blades, one clearly forked blade, two overlapping blades, a
small dirt opening, a low canopy mat, one blade rooted outside the crop, and a
rotatable directional key. Add the side-by-side harness that puts the lab plate
next to matched-scale crops of the reference art. Add per-stage timing rows for
the passes that do not exist yet, so they read as zero rather than as missing.

*Done when: the lab plate renders headless, the key rotates, and the comparison
against a reference crop runs.*

### Phase 1 — `GrassScene` and the world-aligned lattice

Extract shape generation out of `Painter::draw` into `geometry.rs`; `Painter`
becomes a rasteriser over already-built geometry. Assign stable tuft and blade
IDs. **World-align the macro lattice** — today `Macro::build` lays a stride-6
lattice from each page's own origin and 256 is not a multiple of 6, so
neighbouring pages read the composition fields from points up to four pixels
apart and a whole-region bake differs from a tiled one across about a fifth of
its pixels. That is a curiosity today and a correctness bug the moment a shadow
pass is shared across a region. Introduce `BakeRegion`.

*Done when: the legacy path reproduces the current plate at `Verdict::Imperceptible`,
and a region bake equals the tiled bake of the same region pixel for pixel.*

### Phase 2 — Ribbons, cross-section, twist and forked tips

A three-strip cross-section with a raised centre ridge, giving real per-fragment
normals: a consistently lit side, a consistently shaded side, a central
highlight at some orientations, a darker underside, and actual geometry to cast
from. Five strips only for broad statement blades in Reference.

Replace the pure taper with a leaf profile — narrow at the attachment, widening
over the first 20–35%, broadest at 30–50%, then a rapid final taper. Add twist
(root orientation, total twist, non-linear curve, tuft-correlated), which is the
cheapest way to stop a tuft reading as a comb: without it every blade presents
the same face to the light.

`TipProfile::{Pointed, Notched, Forked}`. Fork **only** broad mature blades —
15–35% of them, 5–12% of medium, almost none of the fine. Split at 72–88% of
parent length, 5–20° opening, 10–28% child length, strongly asymmetric, with
positional and tangent continuity at the split. Collapse an unresolved fork to a
notch at distance rather than letting two subpixel children scintillate.

*Done when: the lab fixture shows a forked blade with continuous tangent at the
split, and the extended reach tests pass.*

### Phase 3 — The fine grass layer and the tuft hierarchy

Split today's thatch pass in two: the dark buried structural mat stays as it is,
and a new **fine grass layer** goes in above it — short, densely combed along
`flow`, visible, forming a closed canopy. Add parent **tuft groups** carrying
shared flow, dominant lean, crown radius and height, maturity, density falloff,
an asymmetric lit-facing rim and a trailing skirt. Generalise the existing
down-screen skirt from a fixed trick into a perimeter-blade system, keeping a
modest down-screen bias because it remains the cheapest isometric depth cue
there is.

Add correlated morphology fields — maturity, moisture, exposure, tuft scale,
crown height — as *correlated* fields, not independent noise layers: moist areas
denser and darker at the root, exposed crowns yellower at the tip, mature areas
broader and more frequently split. Deterministic priority blue-noise placement
in Dataset and Reference; keep the jittered grid for Preview.

*Done when: an unlit albedo-and-normal preview already reads as the reference's
crowns and valleys, before any lighting work.*

### Phase 4 — G-buffer, normals, material lighting, and opening the palette

Widen `Cell` into `GrassSample`: depth, octahedral normal, material, root-to-tip
position, thickness, front-or-underside, tuft id, coverage, and **optical depth
accumulated on every fragment, winner or loser**. That last one is the cheap way
to get dense-interior occlusion, and it is the channel `surface.rs` deliberately
removed when it was only a buried *count* with no consumer.

Three normals, blended rather than chosen — ground from the analytic dome
gradient (cheap, because the mounds are placed ellipsoids and not noise), tuft
envelope from the group crown, blade from the ribbon. Weight toward the tuft
normal at battle distance and the blade normal up close. Wrapped front diffuse,
underside fill, back transmission, and a restrained tangent-aligned glint that
subsumes the current `s^8` tip catch rather than sitting beside it.

**Open the palette's dark end.** Keep the ramp philosophy — a lookup, never a
multiply — but add stops below the current bottom of `GRASS` and `THATCH`, and
separate the shadow-hue axis from the light-index axis. Retire `BROAD_DARK`'s
asymmetry **for the reference path only**: that rule exists because nothing in a
flat meadow casts a shadow metres across, which stops being true the moment
shadows have casters.

*Done when: rotating the key swaps which side of every blade is lit, with zero
change to geometry.*

### Phase 5 — Geometry-derived cast shadows

Light-space orthographic depth rendered from the same `GrassScene`, with the
guard band and tests described above. Ground mounds render into the shadow map
too. Slope-scaled and normal-offset bias — never a large global depth bias,
which detaches shadows from their blades. Conservative widening of very thin
ribbons by a quarter to half a shadow texel, with geometric length left exact.

Start with one sharp map plus 3×3 or 5×5 PCF and strong ambient sky fill. Only
once direction, contact and seams are correct, add 4 (Dataset) or 8–16
(Reference) deterministic sun samples over a small angular disk, rendering one
map at a time and accumulating visibility rather than holding several.

*Done when: the lab fixture's forked blade casts a forked shadow onto the dirt,
and a page cropped at a boundary has identical shadows to the same ground baked
whole.*

### Phase 6 — Ambient occlusion, and retiring the fakes

Mostly subtraction. Overlap occlusion from optical depth, `1 - exp(-k·τ)`.
Canopy horizon AO over 8–16 directions at three radii — blade-to-blade contact,
within-tuft, and crown against adjacent valley. A root-contact term that darkens
the floor immediately beneath a tuft and fades within a few centimetres, so dirt
openings read as enclosed by grass rather than cut out of the green.

Then rebalance, because there will otherwise be five terms all describing
darkness. The under-stroke drops to 20–40% of its current strength and keeps
only its edge-separation job; `directional_shadow` demotes to a broad tuft-form
term; `micro_occlusion` folds into the AO calculation instead of being an
independent darkening. Sky fill and green ground bounce keep cavities green
rather than black — the fix for over-dark tufts is fill, never a weaker sun.

### Phase 7 — Painterly resolve and the training exports

The multi-axis LUT: material, sun exposure, occlusion, transmission, maturity
and root-to-tip in, colour out. Warm luminous yellow-green on exposed lit
surfaces, cooler emerald in occluded interiors, deep green-brown at the roots
and never neutral black. Height-aware glaze, edge-aware soften, Mitchell
downsample. AOV export with renderer version, style hash, seed, light direction
and sample sequence in the metadata. Paired low and high renders from one scene,
and centre-crop dataset generation with genuine neighbourhood context so the
network never learns a page border.

Establish `grass-ultra-v1` as the accepted visual baseline.

## Validation

The snapshot suite measures the plate against **our own previous output**. That
is exactly right for optimisation and exactly wrong for a deliberate look
change — it will report every phase here as a total regression. So it gets
re-pointed rather than obeyed:

- Keep the current output under `legacy-b292254` and stop gating on it.
- Gate hard on the **structural** invariants, which do not change: world
  coordinate purity, split-page geometry equality, seam tests, reach bounds,
  stable random streams, no NaNs in degenerate orientations, and — new — unit
  ribbon normals, continuous normal orientation along a blade, fork children
  starting exactly at the parent split, and no shadow-visibility step across a
  page join.
- Add visual **guardrails**, which are not the art judge: luminance percentiles,
  shadow-area share, tip-highlight share, multi-scale frequency energy,
  orientation histogram, clump-size distribution, hue by luminance band, and
  close-versus-far detail retention.

Provisional bands from the concept plate: very few near-black pixels, a brighter
p99 than the current field, similar fine-scale energy, substantially stronger
mid- and broad-scale variation, and more yellow-green highlights **without** the
whole field shifting yellow.

## Cost, and why this stays on the CPU

Rough estimate against the committed baseline of 135 ms for a full-detail
256-pixel page: supersample 3→4 is ×1.8 on all raster work, the fine layer adds
about 30%, the cross-section about 20%, forks about 8%, the shadow pass runs
roughly 1.5× the stroke pass per sun sample, and the AO scans are a few
milliseconds. Call it **10–20×, so 1.5–3 s per page single-threaded**, and
about 0.1–0.2 s per page of throughput on sixteen cores. A four-thousand-page
corpus is minutes.

Memory per page in flight at Reference: a 33.5 MB G-buffer (1024² samples at 32
bytes), one 12 MB shadow map at a time, a 4 MB visibility accumulator, and a few
megabytes of scratch — about **55 MB**, so a sixteen-way bake wants under a
gigabyte. Note that the guard band governs *placement*, not surface extent:
`Painter::rib` already rejects off-surface ribs, so only the shadow map has to
cover the guard.

**Stay on the CPU.** There is no direct `wgpu` dependency in the workspace, the
baker is already `rayon`-parallel across pages, determinism stays trivially
reproducible across machines, and a GPU port is a large infrastructure cost for
a speedup the corpus size does not yet demand. Revisit past roughly 10⁵ tiles.

## Risks, named

| Risk | Why it bites | Mitigation |
| --- | --- | --- |
| Shadow guard too narrow | Invisible in any still without a join; a straight line down the world once tiled | Derive from light, measure max canopy, sweep the test |
| Palette not opened first | Shadows evaluated through a ramp that cannot show them; the work reads as a failure | Palette range lands in Phase 4, with the lighting |
| Five terms describing darkness | Additive stacking crushes the field; each term is individually defensible | Phase 6 is explicitly a subtraction phase |
| Lattice not world-aligned | Region and tiled bakes diverge; training crops carry page structure | Prerequisite in Phase 1 |
| Stream churn | Appending a draw mid-sequence reshuffles the world every time | One reshuffle, in one commit, at the start of Phase 2 |
| Tuning in the meadow | Blade shape, tufts, palette, AO and shadow bias all interact at once | The laboratory plate is the first milestone and the iteration surface |

## The first milestone

Phases 0 to 2 only, and no meadow. Ribbon blades with cross-section, twist and
forked tips, real normals, a rotatable key, rendered onto the one square metre
laboratory plate and compared side by side against matched crops of the
reference art. No shadows, no ambient occlusion, no field tuning.

It is finished when rotating the key clearly swaps light and dark blade sides,
the fork reads as one blade separating rather than two glued together, the tuft
has a coherent bright crown and a dark interior, the result survives being
downsampled to battle scale, and cropping the plate at a page boundary does not
alter the geometry.

The mark gets decided there, and every later phase is cheaper once it is.

---

*This document replaces the screen-space atlas plan that used to live in
`docs/GRASS_PROCEDURAL.md`, which described a `GroundMaterial` shader loop and
`scene.rs` clump instancing that no longer exist.*

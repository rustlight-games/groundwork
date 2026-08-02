# Benchmarks

Benchmarking is a first-class activity here, not something bolted on before a
release. This document is the standard: what to measure, when a benchmark is
required, and how to keep results comparable.

It covers two kinds of measurement that are usually kept apart.

**Performance** is the familiar half. Simulation throughput, flow-field rebuild
cost, grass frame time, inference latency.

**Aesthetics** is the unusual half, and it exists because most of this game is
generated. A rock generator can regress in a way no unit test notices: the rocks
still have valid geometry, they just look worse — spikier, or all alike, or
clumped when scattered. Aesthetic metrics turn those judgements into numbers.

Aesthetic metrics do not replace looking at the output. They catch the drift
between the times you look.

## When a benchmark is required

Required:

- **Every generator.** Terrain, rocks, scatter. Both a timing benchmark and
  aesthetic metrics. Generators are the part of the codebase most likely to
  regress invisibly.
- **Anything on the per-tick path.** Movement, targeting, effect resolution,
  pathfinding. Throughput is a gameplay constraint, not a nicety: the trainer's
  wall-clock cost is directly this number.
- **Anything on the per-frame path.** Grass in particular.
- **Inference.** Batched forward-pass latency at each scenario's unit count.

Not required:

- Pure data structures with obvious cost (`Interner`, `Params`).
- One-off tooling.
- Anything whose cost is dominated by something already benchmarked.

**A new plugin needs benchmarks before it merges**, not after. A plugin without
one has no baseline, and without a baseline there is no way to notice it getting
slower or uglier — which is exactly what happens to generators over time.

## Keeping results comparable

**Always use `bw_bench::SEEDS`.** A measurement taken against a random input is
not a measurement. Ten fixed seeds, because a single seed can flatter or punish
a generator by luck. Append to the list; never reorder it or change a value —
benchmark history only means something if seed *n* means the same thing it did
last month.

**Always name a `Scenario`.** `Small`, `Medium`, `Large`. Performance changes
shape with scale, and a regression that only appears on a large map is the one
most worth catching. `Medium` is the default for tracking.

**Name measurements by dotted path**, matching the crate structure:
`rocks.boulder.compactness`, `sim.tick_throughput`, `nav.flow_field_rebuild`,
`grass.chunk_build`.

**Record the direction of improvement.** `Measurement::higher_is_better`. The
suite mixes both — throughput should rise, frame time should fall — and a
comparison that guesses reports the wrong half as regressions.

## Layout

Performance benchmarks use criterion, in each crate's `benches/`:

```
crates/bw_sim/benches/tick_throughput.rs
crates/bw_nav/benches/flow_field.rs
plugins/bw_fx_rocks/benches/generation.rs
```

Aesthetic metrics go through `bw_bench::metrics` and are emitted as a
`bw_bench::Report`. `cargo run -p bw_forge -- score-rocks` is the worked
example; copy its shape for other generators.

## The aesthetic metrics

Each returns a documented range with a documented direction.

| Metric | Range | Measures | Healthy band for rocks |
|---|---|---|---|
| `compactness` | 0..1 | Isoperimetric quotient; 1.0 is a circle | 0.6 – 0.9 |
| `convexity` | 0..1 | Area over convex hull area | 0.85 – 1.0 |
| `blue_noise_score` | 0..1 | How evenly a scatter is spread | higher is better |
| `luminance_spread` | 0..1 | Lightest to darkest tone | 0.3 – 0.6 |
| `silhouette_variety` | 0..1 | Variation across seeds | > 0.1 |

`silhouette_variety` deserves attention: near zero means the generator is
producing the same shape for every seed. That is a real and easy failure to
introduce, and it is invisible to every correctness test — the shapes are all
perfectly valid, just identical.

## Baselines and tolerances

Commit baselines under `benchmarks/baseline/`. Compare with
`Report::regressions_against`.

Suggested tolerances:

- Performance: **5%**. Tighter than that and machine noise dominates.
- Aesthetic: **10%**. Noisier, being averages over ten seeds.
- Determinism-adjacent numbers: **0%**. They should not move at all.

A measurement absent from the baseline is not a regression — a new benchmark
should not fail the build on its first run.

## Interpreting a regression

Ask in this order:

1. Is the benchmark still measuring the thing it claims to? A refactor can quietly
   make a benchmark optimise away.
2. Was it intentional? If a change trades rock compactness for more interesting
   silhouettes, update the baseline in the same commit and say so in the message.
3. Is it real? Re-run. Performance benchmarks on a laptop under thermal load
   produce nonsense.

## The grass suite

`cargo bench -p bw_grass` is the worked example for a per-frame system, the way
`score-rocks` is for a generator. It writes `benchmarks/grass.ron` and compares
against `benchmarks/baseline/grass.ron`.

It is one binary in several modules, and `autobenches = false` is load-bearing:
without it Cargo compiles every module file under `benches/` a second time as a
bench target of its own.

| Module | Section | Answers |
|---|---|---|
| `perf.rs` | Performance | What does it cost — per phase, under load, at the margin |
| `stability.rs` | Stability | Does it move, or does it merely change |
| `motion.rs` | Motion | Does wind read as wind, contact as contact, a blast as a blast |
| `atlas.rs` | Style | Does the sprite sheet read as drawn artwork |
| `card.rs` | Card | Does the bend bend, what does a fragment cost, is the tone wide enough |
| `texture_match.rs` | Resemblance | How close is a frame to the art target |
| `mirror.rs` | — | A CPU model of the clump shader's vertex stage |
| `harness.rs` | — | Sampled timing, standard scenes, signal analysis |

### The design goal the numbers serve

**Grass is background.** The model is StarCraft's creep: a surface that reacts
and reads as alive on a budget small enough that the rest of the game never
notices it. That decides which numbers are headlines:

- `grass.step.*.frame_share` and `grass.pressure.battle.frame_share` are what a
  change ships or does not ship on.
- `grass.step.trampled_multiplier` says whether a *battle* is affordable rather
  than an empty meadow. A quiet field takes the cheap path through every branch;
  the ratio between quiet and saturated is the honest worst case.
- `grass.pressure.marginal_unit` is the cost of one more unit in the grass, and
  `units_within_tenth_frame` turns it into a headroom.
- Every timing reports a median, a p95 and a **jitter** (p95 over median).
  Background systems fail by hitching, and a mean hides exactly that.

### Performance is measured per phase

A step is six phases and they are wildly unequal — the solver is six Jacobi
sweeps over the whole grid and everything else is one pass. Timing only the
total says the grass got slower and nothing else, so `grass.phase.*` prices each
one, and `grass.phase.*_share` is usually the more useful form when reading a
regression.

### Every stability metric is paired with a motion metric

The most important structural rule in the suite. A field that does not move has
no flicker, no jerk, no chatter and no churn, and would sweep the stability
section outright. `grass.stability.motion_share`,
`grass.stability.tip_travel_pixels` and `grass.wind.dynamic_area` are what stop
that reading as a win.

Flicker is measured as the share of temporal power above **8 Hz**. Grass motion
the eye reads as motion lives below about 3 Hz; above 8 Hz is at most seven
frames per cycle, and nothing in a stylised field should oscillate that fast.
It is measured at four stages, because a flicker can be born in any of them and
each looks innocent from the others:

| Born in | Looks like | Metric |
|---|---|---|
| The wind | Everything shimmering at once | `stability.wind_hf_ratio` |
| The solver | Cells vibrating against their neighbours | `stability.field_hf_ratio`, `jerk_p95` |
| The pixel grid | Edges crawling; motion that stutters | `stability.pixel_chatter`, `silhouette_churn` |
| The sampler | Sparkle inside the sprite | `stability.atlas_minification`, `subpixel_*` |

`grass.stability.rest_drift` is the sharpest of them: with no wind and nothing
touching it, a settled field must stop.

### The card section, and why a dead parameter needed a benchmark

`card.rs` exists because of a specific failure: `ClumpSettings::root_stiffness`
was documented for months as the exponent that keeps a plant's base planted
while its tip curls over, and it did nothing at all. A clump was four vertices,
`up` took the values zero and one, and `pow` fixes both of those for every
exponent — so the parameter was applied to precisely the two inputs on which it
is the identity, and the rasteriser drew a straight line between them.

Nothing in the project could see it. Every correctness test passed, the field
was bit-identical, and the shader's own comment asserted the opposite.
`grass.card.stiffness_effect` is the guard: it places a clump at full bend and
compares it against the same clump with the exponent forced flat. It read
exactly `0.0000`, and could not have read anything else.

The section carries three families, and each one answers a question that lives
in the gap between what the code says and what the picture does:

- **Geometry** — `stiffness_effect`, `base_lean_share`, `length_error`. Each is
  paired with what a shear would have given (`shear_lean_share`,
  `shear_length_error`), so the table carries the old behaviour without needing
  a baseline run to remember it.
- **Overdraw** — the fragment cost, which nothing priced before. `layers_per_pixel`
  is depth complexity from geometry; `early_z_rejected` runs an actual depth test
  over a real chunk in the order the index buffer presents it. It is reported
  beside `early_z_other_order` because a rejection rate only means something
  next to what the other draw order would have given.
- **Tone** — `clump_spread` against `target_spread`. The second is a property of
  the reference plate and never moves; the first is what the renderer produces.
  Measured *between* clumps as well as per pixel, because a clump is thirty
  pixels at the battle camera and nothing inside one survives to the eye.

`grass.tone.clump_spread` read **0.000** when it was first written — every clump
in the field landing in one of the art target's ten tone buckets. That is the
kind of thing a suite exists to find.

### Physics

Properties that no correctness test notices going wrong, because the grass still
moves:

| Metric | Range | Catches |
|---|---|---|
| `timestep_invariance` | 0..1, higher | An integrator whose answer depends on frame rate |
| `energy_monotonicity` | 0..1, higher | A solver quietly manufacturing energy — grass that eventually vibrates on its own |
| `direction_isotropy` | 0..1, higher | Anything simulated in screen space, where a shove from the north behaves differently from one from the east |
| `blast_isotropy` | 0..1, higher | Explosions coming out egg-shaped |
| `coupling_locality` | 0..1, higher | Neighbour coupling creeping up until the field moves like a rubber sheet |
| `axis_reinforcement` | 0..1, higher | The unsigned flattening axis failing to survive a path walked both ways |
| `polar_cancellation` | 0..1, higher | Signed direction memory *not* cancelling when it should |
| `wind.divergence` | 0, lower | Turbulence with sources and sinks, which sucks grass toward fixed points |
| `wind.carpetness` | 0..1, lower | Local and global coherence both high — one rigid sheet, which is how grass ends up reading as water |

`direction_isotropy` and `energy_monotonicity` should both read exactly 1.0.
They are structural properties, not tuning, so treat any movement as a bug
rather than as drift.

### Aesthetic metrics are bands, not maxima

Almost nothing in the motion and style sections wants to be maximised, and this
is the main way aesthetic metrics differ from performance ones. Wind coherence
at 1.0 is a rigid sheet and at 0.0 is static. Contact spill near zero means a
person walking through grass disturbs their own footprint and nothing around it.
Each measurement records the direction of its *nearest* failure and says in a
comment what the other end looks like.

### Resemblance to the art target

`benchmarks/reference/pixel_grass_target.png` is the art target, and nothing in
`texture_match.rs` compares pixels by position — the shader generates unbounded
non-tiling grass, so every metric is a *descriptor* computed identically on both
images. `grass.match.feature_scale` is the one with no substitute: grass drawn
at twice the right size satisfies every value and frequency statistic in the
file and looks obviously wrong beside the plate.

This section needs a screenshot and skips itself with instructions when there is
not one:

```sh
BW_CAPTURE=$PWD/benchmarks/capture/grass.png BW_CAPTURE_AFTER=3 \
  cargo run --release -p bw_grass --example grass_sandbox
```

The screenshot is of the **canvas**, not the window, and that distinction is
load-bearing rather than cosmetic. Every metric in this section measures features
in pixels; photograph the window and each one is `PixelCanvas::scale` times too
big, so the same field scores differently on a retina display than on a plain
one. The canvas is 960×540 whatever the display is, which is the resolution the
art is authored at and the only one comparable to the plate.

Two metrics in this section are known to punish large-scale tonal structure, and
a run that improves the tone field will show both falling. `match.feature_scale`
and `match.repetition` are both computed against a small tiling swatch of
near-uniform tone; a field with 14 m patches has no way to score well on either,
because the reference has no room to contain the thing being measured. Neither is
wrong — read them beside `tone.spread_ratio` and `match.overall` rather than on
their own, and see `clump.rs` for the controlled comparison that pins the cause.

Everything else runs without a GPU. That is deliberate: a suite that needs a
window does not run in CI, on a headless box, or twice in the same minute.

### Tolerances

The suite splits its baseline comparison three ways, because the families are
not noisy in the same way:

- **Performance** — 15%. A timing on a laptop under thermal load moves ten
  percent between runs of identical code.
- **Aesthetic** — 10%.
- **Structural** — 0%. `direction_isotropy`, `energy_monotonicity`,
  `root_pinning`, `palette.monotonicity`, `wind.divergence` and
  `atlas.off_palette_share` do not drift. When they move, it is a bug.

The run also prints how many measurements were *new* and therefore not compared,
because a run where most of the suite is new has a "no regressions" line that
means much less than it looks like.

## Current state

`bw_grass` has a full suite; the harness, fixtures, metrics, and reporting are
tested. The criterion `benches/` directories for the simulation and navigation
crates are not written yet, and this document is the standard they should
follow.

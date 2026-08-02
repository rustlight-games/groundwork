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

It measures three things at once, because they trade against each other and any
one of them alone is misleading.

**Performance.** Step cost at three field resolutions, expressed both in
absolute time and as `frame_share_at_60hz` — the fraction of a frame the field
consumes. That last one is the number that decides whether a change ships.

**Physics.** Properties that no correctness test notices going wrong, because
the grass still moves:

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
| `wind.gust_contrast` | 0..1, higher | A single global wind vector, where the whole meadow leans as one sheet |

`direction_isotropy` and `energy_monotonicity` should both read exactly 1.0.
They are structural properties, not tuning, so treat any movement as a bug
rather than as drift.

**Aesthetics.** `placement_spread` catches blades clumping instead of spreading,
`length_variety` catches a canopy with a mown flat top, and `luminance_spread`
catches a palette flat enough that the grass reads as one material.

## Current state

`bw_grass` has a full suite; the harness, fixtures, metrics, and reporting are
tested. The criterion `benches/` directories for the simulation and navigation
crates are not written yet, and this document is the standard they should
follow.

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
crates/bw_grass/benches/bake.rs          <- the one that exists
crates/bw_sim/benches/tick_throughput.rs
crates/bw_nav/benches/flow_field.rs
plugins/bw_fx_rocks/benches/generation.rs
```

Aesthetic metrics go through `bw_bench::metrics` and are emitted as a
`bw_bench::Report`. `cargo run -p bw_forge -- score-rocks` is the worked
example; copy its shape for a generator whose look is still being decided.

Once a look is settled, aesthetic metrics stop being the right instrument and
should be replaced by a snapshot comparison against the system's own previous
output. `crates/bw_grass` has made that transition and the reasoning is in **The
grass suite** below — it is the more interesting half of this document.

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

The grass is the one subsystem here whose look is **finished**, and that changes
what measuring it means. This section is worth reading even if you never touch
`bw_grass`, because every generator eventually reaches the same crossover.

### Why the aesthetic metrics were deleted

The grass used to be scored against a piece of reference art: a bag of
descriptors — tone, saturation, a six-rung detail ladder, a six-rung structure
ladder — computed identically on the candidate and the target and compared. That
was the right instrument for the question being asked, which was *"does this look
like the art"*. A generated plate and a painted one share no placement, so they
cannot be compared pixel for pixel, and descriptors are the only thing left.

They were removed the day that question was answered, for one reason:

> **Descriptors tell you whether something looks right. They cannot tell you what
> an optimisation cost.**

They are lossy by construction. A plate can lose a fifth of its stroke texture,
or have its supersampling halved, or lose a shading term entirely, and hold every
descriptor inside its documented band — because a standard deviation over a
million pixels does not care *which* pixels. Used as a gate they pass almost
everything, and a gate that passes almost everything reads as a green light.

### What replaced them

Comparing a bake against **its own previous output**. Same seed, same place, same
scale, so every pixel has a counterpart and "unchanged" has an exact meaning:
zero. `bw_grass::compare` does it, and it is far stricter than any aesthetic
metric could be — an optimisation that merely reassociates arithmetic moves a
scattering of pixels by one 8-bit step, and anything that changes what is drawn
does not.

| Number | Catches |
|---|---|
| `ssim` | Structural change — blurring, smearing, lost edges |
| `psnr` / `rmse` | Overall magnitude |
| `p99Δ` | The typical worst pixel, in 8-bit steps, without one outlier setting it |
| `changed` | How *much* of the plate moved, rather than how far |
| `detail` | Fine texture relative to the baseline; below 1.0 is smoother |
| `lumaΔ` | Signed — the whole field went pale or muddy |

`detail` sits beside `ssim` rather than under it because SSIM punishes blur as a
*fraction of local contrast*, so a plate that lost a fifth of its texture
uniformly can still score well. Below 1.0 is the shape almost every grass
optimisation takes when it goes wrong.

The verdict bands are the output that matters:

| Verdict | Means |
|---|---|
| `identical` | Byte for byte. The change was free |
| `imperceptible` | ssim ≥ 0.9995 and p99Δ ≤ 1 step. Rounding, nothing else |
| `close` | ssim ≥ 0.990 and p99Δ ≤ 6 steps. Worth a glance |
| `drifted` | ssim ≥ 0.960. Look at it before accepting the speed |
| `changed` | A different picture |

### Zoom levels are the point, not a detail

A page is *authored* at 96 cache pixels to the world metre and displayed at many
scales. At the snapshot ladder's middle rung the ground shows at about **43
percent**; at the widest, under a quarter; at the height the game actually opens
at, a fifth. A page can now also be **baked** at a chosen scale
(`Page::at_detail`), but the snapshot suite deliberately photographs the
authoring scale, because it is the fidelity reference the cheaper levels are
compared against. So an optimisation that throws away
fine texture is nearly invisible at 48 metres and obvious at 13, and one that
coarsens the mound field is exactly the other way round.

A suite that photographs a single height will therefore certify half of the
changes that damage the look. `bw_grass::fixtures::ZOOMS` is the ladder — 13, 26,
35 and 48 metres, with the shipping height in the middle — crossed with three
widely separated places, and the report carries the **worst row** as well as the
mean. The mean is the summary; the worst row is the finding.

### Granularity is the whole design of the performance half

`bake()` is five public stages, and they are public specifically so they can be
timed apart:

| Stage | What it does |
|---|---|
| `fields` | Builds a `WorldField` — once per page, not once per view |
| `lattice` | Samples the composition fields on a six-pixel lattice |
| `allocate` | Six channels over a 3× supersampled page |
| `floor` | The soil and thatch under everything |
| `strokes` | Every blade, leaf and mat mark |
| `shade` | Ramp lookup, shading terms, blurs, downsample |

A single number for "a page costs 100 ms" tells an optimiser nothing about which
fifth to attack, and the answer is not guessable from reading the source. Two of
these look far cheaper than they are: `fields` reads as setup but is paid per
page, and `shade` reads as a resolve step but runs over nine times the final
pixel count. `stroke/blade` measures one mark on its own, so `strokes` divided by
it separates "each mark is expensive" from "there are a great many of them" —
findings with nothing in common as repairs.

`page_size` earns its row by answering a question that would otherwise be
guessed. A page pays for a guard band around its edge, so its cost has an area
term and a perimeter term. If the perimeter term is large, *fewer and larger*
pages is a real optimisation and the draw-call problem and the bake-cost problem
have the same fix. If it is small, page size is free to be chosen on streaming
grounds alone.

### Latency, not throughput

Pages are baked on a background thread as the camera approaches them, one page
per task. The question the renderer asks is never "how many pages a second can
this machine bake" — it is **"will this one page be finished before the camera
gets there"**.

So every criterion measurement is single-threaded and taken at the page size that
actually ships. Dividing a parallel sweep's wall clock by the pages it baked
measures throughput on a fully loaded machine and prints it where latency
belongs; the two differ by the core count, and only one of them decides whether
the grass pops in. `grass.view_fill` in the snapshot report *is* a throughput
number, and it is labelled as one.

### The workflow

```sh
cargo run --release -p bw_grass --example grass_snapshot -- --accept
cargo bench -p bw_grass -- --save-baseline before
#   ... optimise ...
cargo bench -p bw_grass -- --baseline before
cargo run --release -p bw_grass --example grass_snapshot
```

Snapshots live under `target/grass-snapshots/` and are **never committed**. They
are working state for one round of optimisation, they are three megabytes each,
and a promoted baseline means nothing to any machine but the one that took it.

Timings go to `benchmarks/grass.ron` and are compared against the committed
`benchmarks/baseline/grass.ron`. `--accept-perf` writes that baseline, and it is
a separate flag from `--accept` on purpose: promoting a picture is a local
decision, and moving a committed baseline is a claim about the project.

### Tolerances

| Family | Tolerance | Why |
|---|---|---|
| Timings | 15% | A laptop under thermal load moves ten percent between runs of identical code |
| `view_pages`, `view_pixels` | 0% | Arithmetic, not measurement. When one moves, something changed on purpose or something is broken |
| Similarity | see the verdict bands | Not a percentage question |

The run also prints how many measurements were **new** and therefore not
compared, because a run where most of the suite is new has a "no regressions"
line that means much less than it looks like.

### What the grass suite still does not measure

- **Frame cost.** The runtime side is one opaque texture sample per pixel and a
  draw per page. `grass.view_pages` counts the draws — a couple of hundred at
  1080p — but nothing here times a frame, because that needs a GPU and a window
  and the number would differ more between two machines than most optimisations
  do. When it is built it belongs in the sandbox, driven by a scripted pan.
- **Anything moving.** There is no wind and no interaction yet, so there is
  nothing to measure. When the animated crown layer lands it needs stability and
  motion metrics, and those *will* be descriptors again — for the same reason the
  old ones existed, and with the same caveat about what they cannot decide.

## Current state

`bw_grass` has the only criterion suite in the workspace and the only committed
baseline. `bw_sim`, `bw_nav` and `bw_ai` have neither; this document is the
standard they should follow.

`bw_fx_rocks` is still scored aesthetically through
`cargo run -p bw_forge -- score-rocks`, and correctly so — the rocks are not
finished, so "does this look right" is still the live question there. The
crossover described at the top of this section has not happened for them yet.

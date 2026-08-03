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

`cargo run --release -p bw_grass --example grass_score` is the worked example for
a cached generator, the way `score-rocks` is for a procedural one. It bakes
**three plates per seed** in `bw_bench::SEEDS`, from three widely separated
places in each world, describes each against the reference art, and writes
`benchmarks/grass.ron` to compare against `benchmarks/baseline/grass.ron`.

### Three places, not one

One plate per seed measures the generator and the *place* together and cannot
tell them apart, which is a trap this suite fell into and had to be dug out of.
Ten seeds scored one plate each looked like a seed-dependent generator: mean
luminance swung six percent between worlds. Baking four plates from four
separated regions of a *single* world produced the same swing. So most of it was
regional drift doing exactly its job.

The distinction matters because the two findings call for opposite repairs.
Regional spread should be left alone — a map whose ground is the same brightness
everywhere is the defect, not the feature. A genuinely seed-dependent generator
should be fixed. With one plate per seed you cannot tell which you are looking
at, and tuning the generator until that one plate matches is how a suite ends up
certifying a field that only looks right in one place.

`cargo run --release -p bw_grass --example grass_bake` is the iteration loop
rather than the record: one plate, a PNG, and the full descriptor table beside
the target's. Use it while changing the look; use `grass_score` to decide whether
the change was an improvement.

### The design goal the numbers serve

The grass is a **cached ground surface**, not a field of objects. Everything a
pixel needs is decided once, when its page is baked, and the runtime cost is one
texture read. So the suite splits cleanly in two: how long a page takes to bake,
and whether the baked page looks like the art it is meant to look like.

`benchmarks/reference/grass_target.png` is that art. Nothing in the suite
compares pixels against it — the candidate is generated and shares no placement
with the plate — so every metric is a *descriptor* computed identically on both
images.

### The two ladders are the whole diagnosis

`grass.detail.r{2,4,8,16,32,64}` is the standard deviation of luminance minus its
own blur at six radii. `grass.structure.r{...}` is the standard deviation of that
blur. Together they say something no single number can:

| Reading | What it means |
|---|---|
| detail low, structure right | The composition is there and the brush is wrong. More blades will not fix it |
| detail right, structure low | The marks are right and the field has no organisation. It reads as carpet |
| both right, distance high | Tone or colour has drifted; check the percentile rows |

Each rung diagnoses a different subsystem: 2–4 pixels is the stroke language,
12–20 is clumps and cavities, 50 and up is mound distribution and regional
colour. The mistake this ladder exists to prevent is answering a mismatch at 64
pixels by drawing more grass.

#### Read the ladder as energy per octave, not rung by rung

The rungs are cumulative — each is the variance surviving a blur — so a single
rung reading low says almost nothing about *where* the shortfall is. Differencing
the squares does: `structure.r32² − structure.r64²` is the energy between those
two radii, and the row of differences localises a problem to one octave.

This is worth doing before touching anything, because the two readings suggest
opposite repairs. A ladder that is uniformly ten percent low means the plate is
flat and wants more contrast everywhere. A ladder that is low at every rung
because one octave is empty means the plate has the right amount of contrast at
the wrong *scale*, and adding more will make it worse. The second has happened
here twice. The most recent time, the plate carried half again the reference's
energy between four and sixteen pixels and less than half of it between
thirty-two and sixty-four — a field organised at the stroke scale and not at the
scale the eye groups by — and the repair was to move the radius the canopy-relief
term reads at, not to change any amount of anything.

Both times the naive reading pointed at "add detail", and both times detail was
already in surplus.

### The light index is a percentile, so tone matching is arithmetic

`bw_grass::palette::GRASS` is measured from the reference in equal-population
buckets, so stop *i* is the colour of the reference at its `i/31` percentile. Feed
it an index uniform on `[0, 1]` and the histogram that comes back is the
reference's histogram.

That changes what a failing row means. A low `luma.p95` is not "too dark"; it is
"the light index does not reach far enough at the top", which is a different
repair — and usually the highlight terms rather than the base.

### Distribution shape, not just spread

Two plates can have identical detail figures and look nothing alike: one built
from deep shadows and hard glints, the other from a full mid-range. The
separating measurement is the **kurtosis** of the detail residual. The reference
sits at 5.55; a candidate above about 6.5 has too much extreme contrast and too
little middle, however well its standard deviation matches.

This is not in the report because it needs the reference to be meaningful, but it
is the measurement to reach for when the ladders match and the plate still looks
wrong.

### Variety across seeds

`grass.variety.across_seeds` is the one metric that cannot be computed against
the reference at all, and it catches the failure that every other number in the
file is blind to: a generator whose ten seeds produce the same field. It scores
near zero when the seed has stopped mattering, and perfectly everywhere else.

It is also in direct tension with `grass.match.distance`, and knowing that stops
a whole class of false progress. The reference is one plate from one place. Any
field that varies regionally will therefore have plates brighter and duller than
it, and every one of those pays `luma_mean` — the heaviest term in the distance —
for varying at all. So distance can always be improved by making the ten worlds
more alike, and a change that moves distance and variety in opposite directions
by similar fractions has bought nothing. Narrowing the regional drift is the
usual way this happens by accident; raising its *frequency* is the same trick in
disguise, because a plate that spans more cycles of a field averages more of it
away. Check both numbers before believing either.

The corollary is that `structure.r64` on a single plate is partly a measurement
of how much the regional field varies *within* one plate — so it can be satisfied
by turning the regional drift up, at variety's expense. When both move together,
the structure was real.

### Aesthetic metrics are bands, not maxima

| Metric | Healthy band | What leaving it means |
|---|---|---|
| `grass.match.distance` | below 0.10 | The plate has drifted from the art it is meant to match |
| `grass.tone.luma_mean` | 0.37–0.41 | The field is going pale or muddy |
| `grass.tone.saturation` | 0.86–0.90 | Shading has started multiplying rather than looking up a ramp |
| `grass.canopy.bright_share` | 0.03–0.06 | Tip glints have become sparkle, or vanished |
| `grass.ground.soil_share` | 0.005–0.03 | Bare earth has taken over, or closed up entirely |
| `grass.variety.across_seeds` | above 0.03 | The seed has stopped changing the field |

Higher is not better for most of these, which is why they are bands. A candidate
that improved `bright_share` to 0.12 has not improved anything.

`bright_share` deserves one warning. Its threshold sits almost exactly at the
reference's own 95th percentile, so it is a knife edge there: moving `p95` by
four percent moves it by a third. That makes it a good alarm for highlights that
have genuinely run away, and a bad thing to aim at — chased directly it will walk
the whole tonal range around to close a gap that is within a few percent on every
percentile. Read it against `luma.p95`, not on its own.

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

`bw_grass` has a full suite through `grass_score`, and a committed baseline. The
harness, fixtures, metrics, and reporting are tested. The criterion `benches/`
directories for the simulation and navigation crates are not written yet, and
this document is the standard they should follow.

Two things the grass suite does not yet measure, and should:

- **Frame cost.** The runtime side is one opaque texture sample per pixel and a
  draw per page, which is cheap enough that nothing has needed measuring — but
  "cheap enough" is a claim without a number, and the page count is currently
  high enough to be worth one.
- **Anything moving.** There is no wind and no interaction yet, so there is
  nothing to measure. When the animated crown layer lands it needs the stability
  and motion metrics this document used to describe.

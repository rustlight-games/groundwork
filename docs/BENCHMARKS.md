# Benchmarks

Substantial work ends with a before/after table, not a description of the
improvement. A generated world degrades silently: the geometry stays valid and
the output just looks worse, which no correctness test notices.

## Four instruments, four questions

| Instrument | Question | Cost |
| --- | --- | --- |
| `refactor_fingerprints` | Is it the same meadow? | 0.1 s |
| `cargo bench -p terrain_bake` | What did it cost? | minutes |
| `grass_snapshot` | Did the picture move? | minutes |
| `terrain_bench::iso` | Is the subject good, and are the joins invisible? | minutes |

They are not substitutes. The fingerprint has no renderer in the loop, so it
survives a refactor of the renderer and answers the only question worth asking
during one. The snapshot compares finished pixels, so a deliberate look change
moves it entirely and its answer stops meaning anything — what gates a look
change instead is the structural invariants plus somebody looking. `iso` takes
every number twice, once over the whole layout and once weighted by the subject
mask, because a nine-tile plate is eight ninths context and a change that
improved only the middle tile moves a whole-frame metric by a ninth of its real
size. See [ISOMETRIC_TILES.md](ISOMETRIC_TILES.md).

## Fixed inputs

`terrain_bench::SEEDS` — ten of them. `terrain_bench::SCENARIOS` — twelve.
**Append only, never reorder, never edit.** A benchmark history means something
only if scenario three is the same ground it was last month, and editing one
silently makes every measurement before it incomparable with every measurement
after.

Several scenarios exist because they are where a class of bug lives rather than
because they are typical:

- `page.one_texel_mask` — a feature narrower than the sampling rate, which a
  filter either handles or aliases, and which is invisible in anything larger.
- `page.edge_transition` — a material boundary *on* a page edge, so a seam and a
  transition coincide.
- `page.external_root_mark` — content rooted outside the region that reaches
  into it: the halo's whole reason for existing.
- `grid.four_page_junction` — where four independently baked pages meet.
- `grid.worst_grass_density` — the expensive case. A suite run only on typical
  ground reports a mean and misses the cliff.
- `view.reference_close` and `view.reference_rts` — far enough apart that an
  optimisation can be nearly free at one and obvious at the other.
- `blend.grass_dirt` — the only one that puts two substrates against each other,
  and so the only one where a change to acceptance, ownership or the transition
  solver has anywhere to show up.

## Measurement names

Dotted, and stable — a history keys on them. What exists today:

```text
grass.page_bake                     grass.similarity
grass.page_bake.per_pixel           grass.similarity.ssim
grass.view_fill                     grass.similarity.detail_ratio
grass.view_pages                    grass.similarity.worst_view
grass.view_pixels
```

Every one of them begins with `grass.`, and that is an accurate description of
what has counters: the suite was built around the tuned generator and the
painterly rasteriser. The compiler, the field stack and the candidate domains
have none — `SceneCompileReport` counts candidates generated, accepted and
unowned per compile and nothing collects them into a baseline. See
[todo/measurement.md](todo/measurement.md).

Each `Measurement` records its own `higher_is_better`, because the suite mixes
directions and a comparison that guesses reports the wrong half as regressions.

## Tolerances

- **Determinism-adjacent: 0%.** Split equivalence, seam errors on material
  weights, candidate identities. These come from pure functions; anything but
  equality is a bug.
- **Performance: 5%.**
- **Aesthetic: 10%.**

## Every speed claim carries counter-metrics

A speed improvement obtained by silently generating fewer flowers or shorter
grass is a **quality-tier change**, not an optimisation. So a table reports:

- mark count
- candidates generated, accepted and unowned
- material coverage
- detail energy
- palette drift
- seam error
- memory
- **the weakest seed and the weakest scenario**

The candidate counts are the ones a compiled scene turns on. An optimisation
that got faster by accepting fewer candidates is a quality-tier change, and
`SceneCompileReport` is the only thing that would say so.

That last row is the one most often left out and the one most often load-bearing.
A mean over ten seeds hides the one where the change was catastrophic.

## The granularity of the bake bench

`benches/bake.rs` times six stages separately — `fields`, `lattice`, `allocate`,
`floor`, `strokes`, `shade` — plus one mark drawn alone, and separate groups for
page size, detail tier, view, seed spread, field sampling, blur and resample. A
single number for "a page costs 100 ms" tells an optimiser nothing about which
sixth to attack.

## Running it

```sh
cargo bench -p terrain_bake --bench bake -- --save-baseline before
# ... make the change ...
cargo bench -p terrain_bake --bench bake -- --baseline before

cargo test -p terrain_bench --test refactor_fingerprints
cargo run --release -p terrain_bench --example grass_snapshot
cargo run --release -p terrain_bench --example grass_snapshot -- --accept
```

Always in `--release`, and never while another build competes for cores. A
timing taken against a busy machine is not a timing.

## Aesthetic metrics

`terrain_bench::metrics` scores generated output on properties that correlate
with looking right. They are proxies, not judges — a rock that scores well can
still be ugly. What they reliably catch is **drift**: a generator that slowly
starts producing spikier rocks, or scatter that starts clumping, over the weeks
between the times anybody looks closely.

Healthy bands for the rock generator: compactness 0.6–0.9, convexity 0.85–1.0,
luminance spread 0.3–0.6, silhouette variety above 0.1.

`silhouette_variety` deserves the most attention — near zero means the generator
produces the same shape for every seed, which is real, easy to introduce, and
invisible to every correctness test.

`luminance_spread` is currently a **dead column**: the palette applies one hue
drift to all three tones, so it reads identically for all ten seeds and can
never catch a regression as written.

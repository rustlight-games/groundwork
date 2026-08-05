# The suite measures the old renderer

AGENTS.md invariant 8 says a substantial change ends with a before/after table
carrying quality counter-metrics. The instruments that produce that table were
built around the tuned generator and the painterly rasteriser. The compiler, the
field stack and the candidate domains are measured by almost nothing.

## What is measured today

| Instrument | Question | Reports |
| --- | --- | --- |
| `refactor_fingerprints` | Is it the same meadow? | fingerprints, no renderer in the loop |
| `cargo bench -p terrain_bake --bench bake` | What did it cost? | six page stages, page size, detail, views, seeds |
| `grass_snapshot` | Did the picture move? | `grass.similarity.*`, `grass.page_bake*`, `grass.view_*` |
| `terrain_bench::iso` | Is the subject good, are the joins invisible? | subject-masked metrics, `join_visibility` |
| `terrain_bench::seams` | Does splitting a bake move it? | material weight, elevation, colour error |

Every `Measurement` the suite pushes is named `grass.something`, and every `iso`
scenario is named `iso_nine.something`. That is not a naming problem; it is an
accurate description of what has counters.

## The compiler counts, and nothing collects

`SceneCompileReport` already carries the semantic half, counted rather than
estimated and for exactly this reason:

```text
field_samples · field_spacing_m · halo_m
candidates_generated · candidates_accepted · candidates_unowned
marks_emitted · marks_by_population
```

`candidates_unowned` is the best of them. A candidate that is accepted and then
owned by nobody is a hole in the ground that no test notices and no image
obviously shows — it looks like sparse grass — and it is exactly what a mistake
in an affinity table produces.

None of it reaches a baseline. The report is printed by `terrain compile` and
then discarded; there is no scenario that compiles a document, no `Measurement`
carrying any of these names, and so no way for a change to be caught making one
of them worse.

Timing has no counters at all:

- time to fill the field stack, and time per derived field, so the eight-ray
  exposure scan can be seen to be the expensive one;
- candidate generation, thinning and ownership, separately;
- scene memory and the time to fingerprint it;
- Cycles package size and vertex count.

## What a speed claim has to carry

Unchanged from BENCHMARKS.md and worth restating in these terms: a speed-up that
changes **accepted candidate count, cover mass, or field resolution** is a
quality-tier change and must be labelled as one. The first and third are counted
per compile and compared against nothing; the second does not exist yet.

## Performance budgets

The spec proposed targets for one 2 m subject tile with 3×3 context. They were
never measured against, and they are worth keeping as targets rather than
deleting, because the shape of the budget is the useful part — the field stack
and the derived fields should be tens of milliseconds, the cover solver
hundreds, and a cheap preview under a second.

| Stage | Interactive | Dataset |
| --- | ---: | ---: |
| Prepare, warm cache | < 20 ms | < 20 ms |
| Field stack | < 50 ms | < 150 ms |
| Derived fields | < 50 ms | < 150 ms |
| Cover solve | < 75 ms | < 300 ms |
| Candidates and scene build | < 150 ms | < 500 ms |
| Cheap preview, end to end | < 750 ms | < 2 s |
| Cycles | asynchronous | quality-dependent |

Nothing has been compared against these. Do not quote them as if something had.

## Two dead parameters

Both documented by a test that **asserts the gap rather than the fix**, so the
next person is told it is known and whoever fixes it is told to delete a test.

- **`blade_bend` reaches nothing.** Read only by `Mark::shape`, never called.
- **`luminance_spread` is a dead column** in the rock metrics: the palette
  applies one hue drift to all three tones, so it reads identically for all ten
  seeds and can never catch a regression as written.

Repairing either changes the output, so neither may be smuggled into a change
whose claim is "this moved nothing".

## Done looks like

- `SceneCompileReport`'s counters are pushed as `Measurement`s against a
  baseline, so a change that quietly accepts fewer candidates is caught by the
  suite rather than by somebody looking.
- Scenario names for the compiled path exist and are appended, never reordered
  and never edited — `terrain_bench::SCENARIOS` is twelve today and `blend.grass_dirt`
  is the only one that exercises a transition.
- The weakest seed and the weakest scenario are still the rows nobody omits.

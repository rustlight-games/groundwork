@/Users/gpriday/.codex/RTK.md

# Backseat Warlord — agent guide

> **This documentation is stale and is being rewritten.**
>
> The game — simulation, DQN, navigation, battle UI, all of its content — has
> been deleted. What remains is becoming a headless terrain compiler and
> rendering laboratory: an authored terrain document is parsed, validated and
> compiled into an immutable sampler; a deterministic scene is built once from
> it; and that one scene is handed to Cycles, to the cheap rasteriser, and to
> the dataset exporter.
>
> Everything below that mentions battles, units, abilities, ticks, the trainer
> or content IDs describes code that is gone. It is left in place only until the
> documentation rewrite lands, so that the migration's commits stay reviewable
> against what they replaced.
>
> What is true today: `cargo run -p terrain_cli -- --help`, and four crates —
> `bw_grass`, `bw_bench`, `terrain_cli`, `terrain_preview`.

Global rules for the entire repository. A nested `AGENTS.md` may add stricter
local rules; it may never relax the rules here.

## What this project is

A 2D auto-battler where units learn to fight via a Deep Q-Network, set in a
heavily procedural world. Rust, Bevy 0.19, Burn 0.21, edition 2024, MSRV 1.95.

**Status: skeleton.** The structure, boundaries, and the properties that are
expensive to retrofit are in place and tested. Gameplay is not. Keep target
design and implemented behaviour separate in plain status language — a design
note is not evidence that the mechanic exists in the binary.

Two properties drive nearly every structural decision, and both are expensive
to retrofit:

1. **The simulation is bit-deterministic and runs headless.** Training plays
   millions of ticks with no window and no GPU, and the policy learned there
   must behave identically in the game.
2. **Content volume must not equal code volume.** Abilities are composed from
   data, not written one Rust type at a time.

Almost everything else in this file follows from those two.

## Benchmark-driven development

This is a numerical optimisation project. Most of the world is generated and
most of the behaviour is learned, and neither kind of thing fails by throwing —
it fails by getting slightly worse every week while every test still passes. So
the governing rule is:

> **A substantial change is not finished when its tests pass. It is finished
> when a before/after table shows what it did to the numbers.**

Prose claims about improvement are not accepted. "Faster", "smoother", "the
rocks look better", "training converges sooner" are all hypotheses until there
is a measurement with a baseline beside it.

### Every substantial job ends with a table

Not a paragraph mentioning numbers. An actual table, in the handoff, in this
shape:

| Measurement | Scenario | Direction | Baseline | Candidate | Change | Favourable % |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| `sim.tick_throughput` | medium | higher | — | — | — | — |
| `nav.flow_field_rebuild` | large | lower | — | — | — | — |
| `rocks.boulder.silhouette_variety` | seeds | higher | — | — | — | — |

Rules for the table:

- **Name every measurement by its dotted path**, matching crate structure:
  `sim.tick_throughput`, `nav.flow_field_rebuild`, `grass.chunk_build`,
  `rocks.boulder.compactness`. The same name must mean the same thing across
  runs or the history is worthless.
- **State the favourable direction explicitly** in its own column. The suite
  mixes both — throughput rises, frame time falls — and a reader who has to
  guess will read half the table backwards.
- **Include the counter-metric you might have broken.** A table showing only
  the number that improved is an advertisement, not evidence. If you made
  generation faster, show the aesthetic scores. If you made rocks prettier,
  show generation time.
- **Include the weakest case, not just the average.** The worst scenario and
  the worst seed are what a player actually hits.
- Percentages, for a higher-is-better measure:
  `(candidate - baseline) / abs(baseline) * 100`; for lower-is-better:
  `(baseline - candidate) / abs(baseline) * 100`. When a zero baseline makes
  the percentage undefined, report the absolute change and say so.
- Determinism numbers are not a column that may move. A changed state hash is a
  separate, deliberate, explained line in the handoff.

A change with no measurable effect is a legitimate outcome — report the flat
table and say the change was for clarity or correctness. What is not acceptable
is having no table because none was taken.

### Capture the baseline before you touch production code

Run the measurement first, on unmodified code, and keep it. A baseline
reconstructed after the fact from memory or from a different machine is not a
baseline. Baseline and candidate must share seeds, scenario, build profile,
machine, and thermal state.

### When a benchmark is required

`docs/BENCHMARKS.md` is the standard and owns the detail. In summary — required
for every generator (timing *and* aesthetic metrics), anything on the per-tick
path, anything on the per-frame path, and inference latency. Not required for
pure data structures with obvious cost, one-off tooling, or anything whose cost
is dominated by something already benchmarked.

**A new plugin needs its benchmarks before it merges**, not after. A plugin
without a baseline cannot be observed getting slower or uglier, which is
precisely what happens to generators over time.

### Keeping results comparable

- **Always draw seeds from `bw_bench::SEEDS`.** A measurement against a random
  input is not a measurement. The list is **append-only** — never reorder it,
  never change a value. Seed *n* must mean what it meant last month.
- **Always name a `bw_bench::Scenario`** (`small` / `medium` / `large`).
  `medium` is the default for tracking; `large` is where the cliffs are.
- Record measurements as `bw_bench::Measurement` into a `bw_bench::Report`,
  which serialises to RON. Compare with `Report::regressions_against`.
- Commit accepted baselines under `benchmarks/baseline/`. Transient runs go to
  gitignored `benchmark-results/` and are never cited as durable evidence.
- Tolerances: performance **5%** (tighter and machine noise dominates),
  aesthetic **10%** (noisier, being averages over ten seeds), determinism-
  adjacent numbers **0%** (they must not move at all).
- A measurement absent from the baseline is not a regression. A new benchmark
  does not fail the build on its first run.

### Reading a regression

In this order: is the benchmark still measuring what it claims (a refactor can
quietly let it optimise away)? Was it intentional (if so, update the baseline in
the same commit and say why in the message)? Is it real (re-run; a laptop under
thermal load produces fiction)?

Never move a threshold to make a candidate pass. Never keep a candidate that
improved the aggregate while a required property regressed. Aesthetic metrics
are proxies and do not replace looking at the output — they catch the drift
between the times you look.

## Execution rules

- Prefix shell commands with `rtk` (`rtk proxy ...` when no adapter exists).
  The repository's own `./run` may be invoked directly.
- Prefer headless, deterministic paths: `cargo test`, `bw_forge`, `bw_train`,
  the `bw_grass` example. A bounded `./run` launch is allowed when the user asks
  for a runtime smoke test or a startup-only failure is being diagnosed; it is
  supplemental evidence, never the authoritative result.
- Claim manual playtesting only when it actually happened, and label it as
  supplemental.
- Benchmark on a quiet machine. Do not report timings taken while a build,
  another benchmark, or a training run was competing for cores.

## Core invariants

### Determinism

`docs/DETERMINISM.md` is the full reasoning. The rules:

- **No floats in simulation state.** Use `bw_core::Real`. The two sanctioned
  exceptions are content authoring (`f64` in RON, converted once at load) and
  observations (`f32`, one-way out to the network — the network returns an
  action *index*, and no float re-enters simulation state).
- **No `HashMap`/`HashSet`, no `Instant`/`SystemTime`.** `clippy.toml` denies
  all four workspace-wide. Render-side crates that genuinely need a hash map use
  `bevy_platform::collections::HashMap`, which is seeded deterministically.
- **Iterate by `UnitId` via `UnitIndex::sorted_ids`, never by `Query` order.**
  Query order follows archetype layout, so a unit gaining a status would quietly
  reorder the whole battle.
- **Break every tie explicitly.** Equal distance resolves to the lowest
  `UnitId`; equal cost resolves in a fixed neighbour order.
- **Never draw from a shared generator.** `SimRng` derives an independent stream
  from `(root, stream, tick, salt)`; pass the acting unit's id as salt. A new
  random draw needs a **new `RngStream` variant**, never a reused one —
  otherwise adding a crit roll silently changes every targeting decision in the
  game and invalidates every trained policy.
- **Queue effects, never apply inline.** Producers push to `EffectQueue`; one
  exclusive system sorts and drains it.
- Tick rate is **64**, not 60, because `1/64` is exactly `2⁻⁶` and `dt` does not
  drift. Prefer power-of-two denominators everywhere you have the choice, and
  `real_ratio` over a float literal where you do not.
- Use `bw_core::floor_div_to_int` / `ceil_div_to_int` rather than rolling your
  own. `fixed`'s `to_num` rounds toward negative infinity while Rust's `as`
  truncates toward zero; code that "corrects for truncation" double-corrects on
  negative input. This was a real bug in two crates at once.

### Crate boundaries

- **`bw_sim` depends on `bevy_ecs`, never on `bevy`.** That one line is what
  makes the headless trainer possible, and it is also what makes it impossible
  for a renderer to influence a battle outcome. The same rule applies to every
  crate under `plugins/`. Adding `bevy` to any of them silently destroys both
  properties, and nothing will fail loudly.
- Dependencies point downward. `bw_app` is the only crate that knows about all
  the others.
- The trainer and the game must register **the same** effect handlers and
  generators. If those lists diverge, a policy is trained against rules the
  player never sees. See `crates/bw_app/src/registries.rs`.

### Pinned values that must change deliberately

Each of these is a tripwire. When one fires and the change was intended, update
it **in the same commit as the change that caused it**, and say so in the
message. Never update one to make a test green.

| Pinned value | Where | Fires when |
| --- | --- | --- |
| `GOLDEN` state hash | `crates/bw_sim/tests/determinism.rs` | Battle rules changed |
| `PINNED_HASH` | `crates/bw_core/src/hash.rs` | The hashing scheme changed — invalidates every golden value in the repo |
| `OBS_VERSION` | `crates/bw_ai/src/obs.rs` | The observation encoding changed in any way |
| `ActionSpace::SIZE` | `crates/bw_ai/src/action.rs` | The action space changed |
| `bw_bench::SEEDS` | `crates/bw_bench/src/fixtures.rs` | Append only. Never reorder, never edit a value |
| Committed baselines | `benchmarks/baseline/` | Only alongside a deliberate, explained change |

`OBS_VERSION` and the action space form a contract between trainer and game,
recorded in a `ModelManifest` beside the weights. A model trained against one
encoding and run against another does not crash — it produces confident
nonsense, which is worse. The manifest refuses to load on a mismatch, and it
only works if you bump the version.

Content files load in **sorted filename order**, and that order assigns
`ContentId`s, which end up inside observation vectors. Renaming a content file
can shift ids and invalidate a trained policy. Prefer adding files to renaming
them.

### Attribution

Never add AI attribution, generated-by comments, or `Co-Authored-By` trailers.

## Task routing

| Work | Start here |
| --- | --- |
| Determinism, fixed point, RNG, hashing, ticks, grid | `crates/bw_core`, `docs/DETERMINISM.md` |
| Battle rules, systems, tick phases | `crates/bw_sim`, then re-pin `GOLDEN` |
| Pathfinding, avoidance, cost fields | `crates/bw_nav` |
| Observations, action space, network, policies | `crates/bw_ai`, then bump `OBS_VERSION` |
| Characters, abilities, statuses, terrain, rocks, props | `assets/content/`, `docs/CONTENT.md`, then `bw_forge validate` |
| Generators | the owning `plugins/` crate, plus its timing and aesthetic metrics |
| Rendering, camera, interpolation, grass, UI | `crates/bw_render`, `bw_grass`, `bw_ui` |
| Measurement harness, metrics, reports | `crates/bw_bench`, `docs/BENCHMARKS.md` |
| Training loop | `tools/bw_train` |

## Required validation

Scale to risk; prefer the smallest relevant check while iterating, then broaden.

```sh
rtk cargo check --workspace --all-targets
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets
rtk cargo fmt --all -- --check
rtk cargo run -p bw_forge -- validate          # after any content change
```

`cargo test --workspace` is fast on a warm build. There is no excuse for
skipping it. `crates/bw_sim/tests/determinism.rs` is the most valuable test in
the repository — it compares state hashes at *every* tick, so a divergence that
later re-converges is still caught.

Tests do not replace benchmark evidence, and benchmark evidence does not replace
tests.

## Handoff

State, every time:

- what production behaviour changed and why;
- **the before/after table**, with directions and percentages;
- the exact commands that produced baseline and candidate, and where their
  reports live;
- which pinned values moved and why;
- the largest remaining regression, uncertainty, or thing you did not measure;
- which of check / test / clippy / fmt / validate were run, and anything skipped.

When direction or structure changes, rewrite the affected documents in place and
delete claims that are no longer true. Current docs should read as one coherent
project today; Git history is the archive. Do not create superseded copies,
version-labelled variants, or migration narratives.

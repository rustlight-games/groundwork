# CLAUDE.md

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

A compact implementation orientation for Claude Code and other coding agents.

Read [AGENTS.md](AGENTS.md) first. It is the governing policy for execution,
determinism, crate boundaries, benchmark-driven development, validation, and
handoff. This file is the map of where things are and how to move around them.

The single rule worth stating twice: **substantial work ends with a before/after
table of measurements, not a description of the improvement.** This is a
numerical optimisation project — a generated world and a learned policy both
degrade silently, and the only defence is a number with a baseline beside it.

## Project snapshot

Backseat Warlord is a 2D auto-battler where units learn to fight via a Deep
Q-Network, set in a heavily procedural world. Rust on Bevy 0.19 and Burn 0.21,
edition 2024, MSRV 1.95.

**Status: skeleton.** The structure, boundaries, and the properties that are
expensive to retrofit are in place and tested. Gameplay is not. Do not read a
design note as evidence that the mechanic exists in the binary.

Two properties drive nearly every structural decision: the simulation is
bit-deterministic and runs headless, and content volume must not equal code
volume. [docs/DETERMINISM.md](docs/DETERMINISM.md) explains most of the
decisions everywhere else — start there.

## Repository layers

```
crates/
  bw_core      fixed-point maths, deterministic RNG, ids, ticks, grid, hashing
  bw_content   RON schemas, ContentDb, validation, generator registries
  bw_nav       flow-field pathfinding, local avoidance
  bw_sim       the battle simulation            (bevy_ecs only — no renderer)
  bw_ai        observation encoding, DQN, policies
  bw_bench     benchmark fixtures, metrics, reporting
  bw_render    presentation: interpolation, camera, debug overlays
  bw_grass     grass: procedural placement, and a Cycles path-traced renderer
  bw_ui        screens and HUD, plus GameState
  bw_app       composition root

plugins/
  bw_fx_abilities   spell and ability primitives        (no bevy)
  bw_fx_terrain     terrain generators, effects, scatter (no bevy)
  bw_fx_rocks       procedural 2D rock artwork           (no bevy)

tools/
  bw_train     headless DQN trainer
  bw_forge     content validation and generator scoring
  bw_cycles    the Blender half of the grass renderer (Python, not a crate)

assets/content/   RON: characters, abilities, status, terrain, rocks, props,
                  encounters — loaded in sorted filename order
assets/models/    weights plus the ModelManifest that guards them
docs/             architecture, determinism, content, benchmarks
```

Dependencies point downward; `bw_app` is the only crate that knows all the
others. `bw_sim` and everything under `plugins/` take `bevy_ecs`, never the
`bevy` facade — see AGENTS.md for why that line is load-bearing.

## The measurement system

This is the part of the project that most needs an agent to behave differently
from its defaults, so it is worth understanding before touching anything.

`crates/bw_bench` is the shared harness, and it covers two kinds of measurement
that are usually kept apart:

- **Performance** — the familiar half. Simulation throughput, flow-field rebuild
  cost, grass frame time, inference latency. criterion, in each crate's
  `benches/`.
- **Aesthetics** — the unusual half, and it exists because most of this game is
  generated. A rock generator can regress in a way no unit test notices: the
  geometry is still valid, the rocks just look worse — spikier, all alike, or
  clumped when scattered. `bw_bench::metrics` turns those judgements into
  numbers. They are proxies, not judges; they catch the drift between the times
  a human looks at the output.

Three things make any of it comparable, and all three are contracts rather than
suggestions:

| Contract | Where | Rule |
| --- | --- | --- |
| Fixed seeds | `bw_bench::SEEDS` | Ten of them. Append-only, never reorder or edit |
| Named scenarios | `bw_bench::Scenario` | `small` 32×32/8 units, `medium` 128×128/40, `large` 512×512/200 |
| Dotted measurement names | `bw_bench::Measurement` | `sim.tick_throughput`, `rocks.boulder.compactness` — matches crate structure |

Each `Measurement` records its own `higher_is_better`, because the suite mixes
directions and a comparison that guesses reports the wrong half as regressions.
`Report::regressions_against` does the comparison; reports serialise to RON.

`docs/BENCHMARKS.md` is the standard: when a benchmark is required, the
aesthetic metrics and their healthy bands, tolerances (5% performance, 10%
aesthetic, 0% determinism-adjacent), and how to read a regression.

### The worked example

`cargo run -p bw_forge -- score-rocks` is the shape to copy for any generator:
run across all ten seeds, print the per-seed numbers, then the across-seed
variety. It produces a table like this, which is roughly what a baseline capture
looks like in practice:

```
              seed  compactness  convexity   contrast
0x0000000000000001        0.827      0.933      0.360
0x000000005eed1234        0.824      0.951      0.360
...
variety across seeds: 0.112
```

Read against the healthy bands in `docs/BENCHMARKS.md`: compactness 0.6–0.9,
convexity 0.85–1.0, luminance spread 0.3–0.6, silhouette variety above 0.1.
`silhouette_variety` deserves the most attention — near zero means the generator
is producing the same shape for every seed, which is a real and easy failure to
introduce and is invisible to every correctness test.

### The grass is measured differently, and on purpose

The grass used to be scored against reference art. It no longer is, and the
reason generalises: **descriptors are for deciding whether something looks
right; they are useless for deciding what an optimisation cost.** A plate can
lose a fifth of its stroke texture and hold every descriptor inside its band.

The suite therefore measures speed and self-similarity. Note that
self-similarity is the right gate for an *optimisation* and the wrong one for a
deliberate look change — the reference-renderer work moved every pixel on
purpose, and the snapshot baseline was retired rather than obeyed. What gates a
look change instead is the structural invariants: seams, reach bounds,
world-coordinate purity, stable streams, and the laboratory plate.

- `cargo bench -p bw_grass` — criterion, and deliberately **granular**. `bake()`
  is five public stages (`fields`, `lattice`, `floor`, `strokes`, `shade`), each
  timed separately, plus one mark drawn on its own. A single number for "a page
  costs 100 ms" tells an optimiser nothing about which fifth to attack.
- `cargo run --release -p bw_grass --example grass_snapshot` — photographs three
  places at four camera heights, compares each against the last accepted set
  pixel for pixel, and prints a verdict from `identical` to `changed`. See
  `bw_grass::compare`.
- `cargo run --release -p bw_grass --example grass_critique` — the **look gate**,
  and the counterpart to the snapshot rather than a replacement. The snapshot
  answers "did the picture move"; a deliberate look change moves it entirely and
  the answer stops meaning anything. This answers "is the picture the one we are
  aiming at", against `docs/art/grass-target.png`, in six bands that need no
  pixel correspondence at all. See `bw_grass::critique`.

Snapshots are working state and live under `target/`. The timings go to
`benchmarks/grass.ron` against the committed `benchmarks/baseline/grass.ron`.

### Current state of the harness

Honest status, so nobody reports against a rig that does not exist:

- The fixtures, metrics, reporting, and comparison logic exist and are tested.
- `crates/bw_grass/benches/bake.rs` is the only criterion suite written. The
  simulation and navigation crates still have none; `docs/BENCHMARKS.md` is the
  standard they should follow.
- End-to-end measurements wired up: `cargo run -p bw_forge -- score-rocks`
  (still aesthetic — rocks are not settled), and the grass pair above.
- `benchmarks/baseline/grass.ron` is the only committed baseline.

So a performance claim about the tick path usually still means building the
benchmark as the first milestone of the work. That is expected — "there was no
rig" is a reason to build one, not a reason to skip the table.

## Finding documentation

Task-oriented rather than an index of everything:

- [README.md](README.md) — status, how to run, layout.
- [docs/DETERMINISM.md](docs/DETERMINISM.md) — the rules the simulation lives
  under, and the reasoning behind each. Read before touching `bw_sim`,
  `bw_core`, or `bw_nav`.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — why the workspace is split this
  way, the tick phase order, why plugins are compile-time crates.
- [docs/CONTENT.md](docs/CONTENT.md) — authoring characters, abilities, terrain,
  rocks, props; how effect trees compose.
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — the measurement standard.
- [docs/GRASS_CYCLES.md](docs/GRASS_CYCLES.md) — how the grass is rendered: what
  stays in Rust, what goes to Cycles, and the several things about the camera and
  the geometry that each cost a wasted render to discover.
- Crate-level `//!` docs carry the reasoning for each crate. They are written to
  be read, not skimmed — `bw_core`, `bw_sim`, `bw_ai`, and `bw_nav` in
  particular explain decisions that are not obvious from the code.

Useful discovery:

```sh
rtk rg --files crates plugins tools assets docs
rtk rg -n "<concept>" crates plugins tools docs assets
rtk rg -n "OBS_VERSION|GOLDEN|PINNED_HASH|SEEDS|higher_is_better" crates
rtk cargo doc --workspace --no-deps --open
```

## Execution preference

Prefer deterministic headless paths. The full game launch is for runtime smoke
tests and startup failures, not for evaluating behaviour.

```sh
rtk cargo check --workspace --all-targets
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets
rtk cargo fmt --all -- --check

rtk cargo run -p bw_forge -- validate            # every content change
rtk cargo run -p bw_forge -- score-rocks         # rock generator metrics
rtk cargo run -p bw_train --release -- --episodes 10
rtk cargo run --release -p bw_grass --example grass_bake     # a plate, headless
rtk cargo run --release -p bw_grass --example grass_bake -- --quality reference
rtk cargo run --release -p bw_grass --example grass_lab      # the laboratory plate
rtk cargo run --release -p bw_grass --example grass_lab -- --sweep   # turn the sun
rtk cargo run --release -p bw_grass --example grass_dataset -- --aovs
rtk cargo run --release -p bw_grass --example grass_snapshot # photograph, compare
rtk cargo run --release -p bw_grass --example grass_sandbox  # the live renderer
rtk cargo bench -p bw_grass                                  # where the time goes

./render               # one whole scene, path-traced by Cycles. 1920x1080
BW_SAMPLES=512 ./render
BW_DETAIL=96 ./render  # pixels per metre; lower shows more ground

./run                  # the game, debug. Rasterised — Cycles cannot run in a frame
BW_DEV=1 ./run         # dynamic Bevy linking, much faster incremental builds
BW_RELEASE=1 ./run     # optimised
BW_GRASS_TRACED=1 ./run  # read pre-traced pages where they exist
```

Always benchmark in `--release`, and never while another build or training run
is competing for cores.

## Architecture

The authoritative path through a tick:

```text
Intent (policy or script)
    -> BattleSim::step
    -> Begin → Perception → Decision → Movement → Combat → Effects
       → Status → Death → Cleanup
    -> state hash / observation / outcome
    -> Bevy presentation (read-only) or trainer
```

`BattleSim` owns a `World` and a `Schedule` and exposes exactly four operations:
step, observe, check for an outcome, and hash. There is deliberately no `App`
and no main loop — the trainer runs battles as fast as the CPU allows, the game
runs one tick per frame, and neither should have to accommodate the other.

The phase order is spelled out explicitly rather than inferred from data access,
because Bevy's automatic ordering shifts when a system's parameters change and a
battle's outcome would shift with it. The order encodes real rules: `Effects`
follows `Combat` so a hit and its status land together; `Death` follows both so
two units that kill each other on the same tick both connect.

Simulation runs at 64 Hz; decisions at 8 Hz. Presentation reads simulation state
and never writes back — `bw_render` is the only place fixed point becomes `f32`,
and the conversion is one-way.

## Core invariants

Full statements and reasoning are in [AGENTS.md](AGENTS.md). In brief:

- No floats in simulation state; no `HashMap`/`HashSet`; no wall clock.
  `clippy.toml` enforces the last two.
- Iterate by `UnitId`, break every tie explicitly, queue effects rather than
  applying them inline.
- Derive randomness per call site through `SimRng`; a new draw needs a new
  `RngStream` variant, never a reused one.
- `bw_sim` and `plugins/*` never depend on the `bevy` facade.
- The trainer and the game register the same handlers and generators.
- `GOLDEN`, `PINNED_HASH`, `OBS_VERSION`, `ActionSpace::SIZE`, `SEEDS`, and
  committed baselines change only deliberately, in the same commit as the change
  that caused them, with the reason in the message.
- Never add AI attribution, generated-by comments, or `Co-Authored-By` trailers.

## Content authoring

Everything under `assets/content/` is RON, loaded by `bw_content` and validated
by `cargo run -p bw_forge -- validate`. Adding a character or a spell should be a
file, not a code change.

An ability is a tree of registered primitives — `damage`, `heal`,
`apply_status`, `sequence`, `terrain_mud` today — with targeting separated from
payload, so one `damage` primitive serves a cone, a chain, and a single-target
nuke. Prefer composing existing primitives; a new one is justified when several
abilities want it, not when one does.

Durations are always in ticks (64 per second), never seconds. Numbers are
authored as decimals and converted to fixed point once, at load.

Files load in sorted filename order and that order assigns `ContentId`s, which
end up in observation vectors. **Prefer adding files over renaming them** — a
rename can shift ids and invalidate a trained policy.

## Known gaps

Real, currently true, and worth knowing before you trip over them:

- `crates/bw_grass/benches/` is the only criterion suite; `bw_sim`, `bw_nav` and
  `bw_ai` still have none. See the harness status above.
- `tools/bw_train` duplicates the registry lists from `bw_app::registries`
  rather than importing them (importing would pull the renderer into the
  trainer). Its own comment claims a test keeps the two lists honest — there is
  no such test, and its generator registry currently omits `bw_fx_rocks`. The
  trainer/game parity invariant is therefore unenforced.
- The grass has no wind, no trampling and no animated crown layer. The baked
  surface is static, and the rear/front crown split — the thing that lets a unit
  stand *in* the grass rather than on it — is not built.
- Each grass page is its own texture and its own draw call, so a 1080p view is a
  couple of hundred draws rather than the handful an atlas-packed cache would
  need. `Page::for_view` would cut this by the square of the camera's display
  scale and `bw_grass::plugin` still does not call it. **This is deliberately
  parked**: the rasteriser is now the cheap input tier only — see
  [docs/GRASS_CYCLES.md](docs/GRASS_CYCLES.md) — and optimising a runtime whose
  output is not the shipping picture is work with no destination.
- `lighting.rs`, `shadow.rs` and the five darkness terms in `bake.rs` now serve
  only that cheap tier. They are not wrong, but they are no longer how the grass
  is meant to look, and several crate-level doc comments still read as though
  they are.
- `dataset.rs` exports the *rasteriser's* `Passes` alongside a Cycles target.
  Cycles' own render passes and cryptomatte would give per-blade IDs and
  physically consistent channels by configuration rather than by hand-plumbing
  ten of them.
- `grass_prebake` starts a Blender process per page, and startup is several
  seconds against about one second of tracing. `tools/bw_cycles/render.py`
  already accepts a manifest of many pages in one invocation; the pre-baker does
  not use it yet.
- A page's bake scale is now a parameter (`Page::at_detail`), and the art
  constants scale with it — see `Page::detail` for what does and does not. Pages
  still have no mip chain, so a page is only sampled cleanly near the scale it
  was baked at; the difference is that the scale is now chosen rather than fixed
  at 96 cache pixels per world metre.
- A page baked with `bake` rather than `bake_padded` has its neighbourhood-
  reading shading terms computed against whatever part of the neighbourhood fell
  inside it, and the directional relief offset collapses within seventeen pixels
  of the page's left edge. `bake_padded` — which `BakeRegion` and the dataset
  exporter both use — rasterises the surrounding ground and crops, and its pad is
  derived from the chain of reaches rather than picked. Plain `bake` keeps the
  artefact and is the streaming tier's path, where a page popping in with a
  slightly different relief at one edge is not what anybody notices.
- `bw_fx_rocks` varies its palette by applying one hue drift to all three tones
  equally, which leaves the lightest-to-darkest spread unchanged. So
  `luminance_spread` reads exactly 0.360 for all ten seeds — a dead column that
  can never catch a regression as written.
- `apply_status` takes its status as an integer `ContentId` rather than a key
  string, because parameter values are not resolved against the interner at load
  time. Awkward to author; the fix is a key-to-id resolution pass in `Params`.
- `crates/bw_sim/tests/determinism.rs` contains
  `a_battle_with_no_random_element_is_seed_independent`, which documents that
  nothing in movement, targeting, or basic attacks currently draws. It is meant
  to start failing when crits or damage variance land, and then be deleted.

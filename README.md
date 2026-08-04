# Backseat Warlord

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

A 2D auto-battler where units learn to fight via a Deep Q-Network, set in a
heavily procedural world.

**Status: skeleton, plus ground.** The structure, boundaries, and the properties
that are expensive to retrofit are in place and tested. Gameplay is not. The
ground is: grass is a procedurally baked isometric surface cache, generated from
world coordinates — see [`crates/bw_grass`](crates/bw_grass/src/bake.rs). It is
static; wind and trampling are not built yet.

The look is settled and the speed is not. Matching the grass to reference art was
the previous phase and it is finished; a page currently costs around a tenth of a
second to bake, which is the price that bought it. So the grass suite now
measures two things and no longer scores the art: **how long each stage takes**,
and **how far the picture has moved from the last accepted snapshot**.

## Running

```sh
cargo run                     # the game
cargo run --features dev      # dynamic linking, much faster incremental builds
cargo test --workspace        # everything

cargo run -p bw_forge -- validate      # check all content
cargo run -p bw_forge -- score-rocks   # aesthetic metrics for the rock generator
cargo run -p bw_train -- --episodes 10 # headless training loop

cargo run --release -p bw_grass --example grass_sandbox   # the grass, on its own
cargo run --release -p bw_grass --example grass_bake      # bake a plate to a PNG
cargo run --release -p bw_grass --example grass_snapshot  # photograph and compare
cargo bench -p bw_grass                                   # where the time goes
```

The game boots straight onto the battlefield. The sandbox pans with the arrow
keys or WASD, zooms with the mouse wheel, and takes a screenshot with F12 —
panning is the thing worth doing there, because pages are baked independently and
a long diagonal drive is what proves they agree along their edges.

### Optimising the grass

Three commands, in this order:

```sh
cargo run --release -p bw_grass --example grass_snapshot -- --accept
cargo bench -p bw_grass -- --save-baseline before
#   ... change something ...
cargo bench -p bw_grass -- --baseline before
cargo run --release -p bw_grass --example grass_snapshot
```

`cargo bench` says what got faster, per stage. `grass_snapshot` says what the
picture paid: it re-photographs three places at four camera heights and compares
each against the accepted set pixel for pixel, printing a verdict per view from
`identical` through to `changed`. Snapshots live under `target/` and are never
committed; the timings go to `benchmarks/grass.ron` and are compared against the
committed `benchmarks/baseline/grass.ron`.

Requires Rust 1.95 or newer (Bevy 0.19's MSRV).

## Layout

```
crates/    engine crates — see docs/ARCHITECTURE.md
plugins/   content plugins: abilities, terrain, rocks
tools/     bw_train (headless trainer), bw_forge (content pipeline)
assets/    RON content, shaders, sprites, model weights
docs/      architecture, determinism, content, benchmarks
```

## Docs

- [Architecture](docs/ARCHITECTURE.md) — why the workspace is split this way
- [Determinism](docs/DETERMINISM.md) — the rules the simulation lives under
- [Content](docs/CONTENT.md) — authoring characters, abilities, terrain, rocks
- [Benchmarks](docs/BENCHMARKS.md) — including aesthetic metrics for generators

Start with Determinism. It explains most of the decisions everywhere else.

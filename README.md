# Backseat Warlord

A 2D auto-battler where units learn to fight via a Deep Q-Network, set in a
heavily procedural world.

**Status: skeleton, plus grass.** The structure, boundaries, and the properties
that are expensive to retrofit are in place and tested. Gameplay is not. The
terrain is: grass is a real bend-field simulation with wind, trampling and
blasts — see [`crates/bw_grass`](crates/bw_grass/src/field.rs).

## Running

```sh
cargo run                     # the game
cargo run --features dev      # dynamic linking, much faster incremental builds
cargo test --workspace        # everything

cargo run -p bw_forge -- validate      # check all content
cargo run -p bw_forge -- score-rocks   # aesthetic metrics for the rock generator
cargo run -p bw_train -- --episodes 10 # headless training loop

cargo run -p bw_grass --example grass_sandbox   # the grass, on its own
cargo bench -p bw_grass                         # grass performance + physics metrics
```

The game boots straight onto the battlefield. In it and in the sandbox:
**left click** sets off a blast, **right drag** walks something heavy through
the grass. The sandbox adds mouse-wheel zoom, arrow keys to turn the wind,
`-`/`=` for its strength, space to stand everything back up, and F12 for a
screenshot.

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

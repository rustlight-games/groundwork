# Backseat Warlord

A 2D auto-battler where units learn to fight via a Deep Q-Network, set in a
heavily procedural world.

**Status: skeleton.** The structure, boundaries, and the properties that are
expensive to retrofit are in place and tested. Gameplay is not.

## Running

```sh
cargo run                     # the game
cargo run --features dev      # dynamic linking, much faster incremental builds
cargo test --workspace        # everything

cargo run -p bw_forge -- validate      # check all content
cargo run -p bw_forge -- score-rocks   # aesthetic metrics for the rock generator
cargo run -p bw_train -- --episodes 10 # headless training loop
cargo run -p bw_grass --example grass_sandbox
```

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

# Backseat Warlord

A 2D auto-battler where units learn to fight via a Deep Q-Network, set in a
heavily procedural world.

**Status: skeleton, plus ground.** The structure, boundaries, and the properties
that are expensive to retrofit are in place and tested. Gameplay is not. The
ground is: grass is a procedurally baked isometric surface cache, generated from
world coordinates and matched against reference art — see
[`crates/bw_grass`](crates/bw_grass/src/bake.rs). It is static; wind and
trampling are not built yet.

## Running

```sh
cargo run                     # the game
cargo run --features dev      # dynamic linking, much faster incremental builds
cargo test --workspace        # everything

cargo run -p bw_forge -- validate      # check all content
cargo run -p bw_forge -- score-rocks   # aesthetic metrics for the rock generator
cargo run -p bw_train -- --episodes 10 # headless training loop

cargo run --release -p bw_grass --example grass_sandbox  # the grass, on its own
cargo run --release -p bw_grass --example grass_bake     # bake a plate to a PNG
cargo run --release -p bw_grass --example grass_score    # score it against the art
```

The game boots straight onto the battlefield. The sandbox pans with the arrow
keys or WASD, zooms with the mouse wheel, and takes a screenshot with F12 —
panning is the thing worth doing there, because pages are baked independently and
a long diagonal drive is what proves they agree along their edges.

`grass_bake` writes a plate and, given `--reference`, prints a descriptor table
beside the reference art's. `grass_score` does the same across all ten fixed
seeds and writes a report to compare against `benchmarks/baseline/grass.ron`.

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

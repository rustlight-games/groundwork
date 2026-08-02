# Architecture

Backseat Warlord is a 2D auto-battler whose units learn to fight via a Deep
Q-Network, set in a heavily procedural world.

This document explains why the workspace is split the way it is. The short
version: two properties drive nearly every structural decision, and both are
expensive to retrofit.

1. **The simulation must be bit-deterministic and runnable headless.** Training
   plays millions of ticks with no window and no GPU, and the policy learned
   there has to behave identically in the game.
2. **Content volume must not equal code volume.** Hundreds of units and
   abilities only work if abilities are composed from data rather than written
   one Rust type at a time.

## Crate map

```
crates/
  bw_core      fixed-point maths, deterministic RNG, ids, ticks, grid, hashing
  bw_content   RON schemas, ContentDb, validation, generator registries
  bw_nav       flow-field pathfinding, local avoidance
  bw_sim       the battle simulation            (bevy_ecs only — no renderer)
  bw_ai        observation encoding, DQN, policies
  bw_bench     benchmark fixtures, metrics, reporting
  bw_render    presentation: interpolation, camera, debug overlays
  bw_grass     grass: a bend-field simulation and its blade renderer
  bw_ui        screens and HUD, plus GameState
  bw_app       composition root

plugins/
  bw_fx_abilities   spell and ability primitives
  bw_fx_terrain     terrain generators, terrain effects, prop scatter
  bw_fx_rocks       procedural 2D rock artwork

tools/
  bw_train     headless DQN trainer
  bw_forge     content validation and generator scoring
```

Dependencies point downward. `bw_app` is the only crate that knows about all the
others.

## The split that matters most

`bw_sim` depends on `bevy_ecs` and `bevy_app` — never on `bevy`. That single
line in its `Cargo.toml` is what makes the trainer possible: pulling in the
`bevy` facade would drag in the renderer, a window, and a GPU requirement, and
every headless battle would need all three.

It also enforces the second half of the rule. A renderer that cannot be reached
from simulation code cannot accidentally influence a battle's outcome.

The plugin crates follow the same rule for the same reason. `bw_fx_abilities`
has no Bevy dependency at all, so the trainer runs against exactly the effect
handlers the game does. If those two lists ever diverged, a policy would be
trained against rules the player never sees.

## How a battle runs

`BattleSim` owns a `World` and a `Schedule`, and exposes four operations: step,
observe, check for an outcome, and hash. There is deliberately no `App` and no
main loop — the trainer runs battles as fast as the CPU allows, the game runs
one tick per frame, and neither should have to accommodate the other.

A tick executes these phases in a fixed, explicit chain:

```
Begin → Perception → Decision → Movement → Combat → Effects → Status → Death → Cleanup
```

Spelled out rather than inferred from data access. Bevy can order systems
automatically from their queries, but that ordering shifts when a system's
parameters change — and a battle's outcome would shift with it.

The order encodes real rules. `Effects` follows `Combat` so a hit and the status
it inflicts land together. `Death` follows both, so two units that kill each
other on the same tick both connect; resolving death immediately would give the
earlier-iterated unit an advantage that depends on entity layout.

Simulation runs at 64 Hz. Decisions run at 8 Hz — a Q-network forward pass per
unit per tick is neither affordable nor useful.

## Abilities are data

An ability is a tree of registered primitives, authored in RON:

```ron
(kind: "damage",
 targeting: Some((shape: Cone(radius: 2.5, arc_degrees: 110.0), filter: Enemies, ...)),
 params: { "amount": Num(18.0) })
```

Targeting is separated from payload, so one `damage` primitive serves a cone, a
chain, and a single-target nuke. Adding a spell is a file, not a code change.

See [CONTENT.md](CONTENT.md).

## Plugins are compile-time crates

Rust has no stable ABI, so runtime-loaded plugins would require every plugin to
be built with a byte-identical compiler and dependency graph. Compile-time
crates registering into string-keyed registries give the same modularity with
none of that fragility — and content *data* stays hot-reloadable, which is where
iteration speed actually matters.

## Grass simulates a field, not blades

`bw_grass` keeps a world-aligned grid holding the posture of the canopy — which
way it leans, how fast it is moving, where it has been trodden — and the
renderer reconstructs however many blades it needs by sampling that grid in a
vertex shader. Cost then scales with the *area* being disturbed rather than with
how much grass is drawn on it, which is the only reason a battlefield's worth of
grass can react to a battlefield's worth of units.

Two rules there are expensive to retrofit.

**Simulate in world space; project only when drawing.** A blade shoved west must
behave exactly like a blade shoved north. Simulating in screen space makes the
response depend on the camera, and `grass.physics.direction_isotropy` exists to
keep it that way.

**Blades bend in a virtual third dimension.** A blade is a curve through
`(X, Y, Z)` that preserves its arc length, projected at the last moment. That is
what makes leaning shorten the silhouette and the tip travel along an arc.
Shearing a flat sprite instead is what makes grass look like rubber.

The unusual piece is that a cell records both a signed lean *and* an unsigned
flattening axis. A path walked in both directions cancels to zero as a
direction while remaining visibly flattened, and only the axis can tell that
apart from undisturbed grass. See the `bw_grass::field` module docs.

## Learning

`bw_ai` is generic over the Burn backend. The game runs CPU inference; training
happens out of process in `tools/bw_train`.

Observations are `f32`, which looks like a violation of the no-floats rule but
is not: the flow is one-way. The simulation produces an observation, the network
returns a discrete action *index*, and the simulation acts on that integer. No
float re-enters simulation state.

`OBS_VERSION` and the action space form a contract between trainer and game,
recorded in a `ModelManifest` beside the weights. A model trained against one
encoding and run against another does not crash — it produces confident
nonsense, which is worse. The manifest refuses to load on a mismatch.

Bevy and Burn coexist in one build. Both want `wgpu 29`, verified by resolving
and compiling them together, so an in-process GPU training mode stays available
even though the game does not use one.

## Reading order

New to the codebase: [DETERMINISM.md](DETERMINISM.md), then `bw_core`, then
`bw_sim`'s crate docs, then `crates/bw_sim/tests/determinism.rs` — which is the
most valuable test in the repository.

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
  bw_grass     grass: a baked ground-surface cache and its renderer
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

## Grass is a cached surface, not a field of objects

`bw_grass` does not draw grass. It bakes it: a page of already-composited ground
is generated from world coordinates, cached, and then drawn as one opaque
texture. Roughly nine tenths of the grass pixels on screen therefore cost what a
background costs, and the detail in them is paid for once rather than every
frame.

Four rules there are expensive to retrofit.

**Place in world space; project only when baking.** A clump placed by screen
position slides when the camera moves, and a mound shaped in screen space changes
shape as the view scrolls. Everything in `bw_grass::field` is a pure function of
a world coordinate — which is also the property that lets two pages that have
never met agree along a shared edge, with no neighbour lookups and no seams to
hide.

**Shade through a ramp, never by multiplying.** Multiplying an albedo by a
lambert term produces grey-green shadows. The reference art's darkest pixels are
still saturated green and its brightest are yellow-green paint. `bw_grass::palette`
encodes that directly, measured from the art rather than derived.

**Composite by isometric depth, not by draw order.** Alpha-over produces a
collage of decals stacked on one another. A depth test — using
`bw_grass::iso::depth`, the same ordering the renderer uses against units — is
what gives the cache an inside, because a stroke that loses its pixel still
counts as occlusion.

**Detail belongs to the mound, not to the pixel.** The art is not uniformly
detailed: bright crowns, dark backs and dark interiors are organised by a mound
field, and grass that is uniformly busy everywhere reads as carpet however good
the individual marks are.

**Relief is a rhythm, and a rhythm is not round.** Two failures sit either side
of this. Shade the mound field hard and every swell becomes a cushion — crowned,
ringed, and roughly the size of its neighbours — and the surface reads as
upholstery whatever the marks on it are. Shade it softly and take nothing else
in exchange and the plate goes flat at every radius, not just the one that was
shouting. So the mounds are drawn as elongated ridges oriented along a shared
local flow rather than as discs, the directional term on them is restrained, and
the structure they used to carry is supplied instead by fields that have nothing
to do with height: regional colour, three scales of clump density, and how far a
bunch of grass stands above the ground beside it. The last of those is measured
against a blur a third of a metre wide and applied *signed* — a term that only
ever darkens the shortfall draws a ring at the foot of every bright mass, which
is the cushion reading arriving by a different route.

**Light may be broad; dark may not.** The single least symmetric rule here, and
the one that most changes how a generated field reads. A broad bright area is a
place the sun is reaching and the eye accepts one at any size. A broad *dark*
area is not a shadow — shadows have a caster, and nothing in a flat meadow casts
one metres across — so it reads as a patch of grass that has simply been dimmed,
a stain on the texture rather than an event in the world. It gives itself away
worst where the canopy is open, since there is not even any thickness to explain
it. So every slowly varying lighting term gets its negative half compressed hard
(`bake::BROAD_DARK`) and the fast ones — micro-occlusion at three pixels, the
under-stroke on each mark, the mat below the canopy — keep theirs in full. Dark
then only ever appears as a narrow thing between two lit things, which is the
only place it is legible as depth. There is a corollary that took a round of
tuning to find: a broad dark area is only wrong while it is *featureless*. A
handful of lit tufts scattered through one turn the same darkness back into
depth, because there is finally something in front for it to be behind — so the
baker seeds bright accents at a rate that deliberately rises where the ground is
dim and loosely described, against the grain of every other term.

**A hue shift is not an exposure shift.** Three terms push a resolved colour
toward a different green — canopy-depth cooling, chroma calming, regional drift
— and each answers "*which* green is this", never "how much light is on it".
Written by hand they fail that: dropping red by a fifth takes real luminance with
it. `bake::hue_only` renormalises each shift back to the luminance it started
from. It matters most for the regional drift, because that one keys on position:
a hue shift that also darkens turns "this part of the meadow is a different
green" into "this part of the meadow is dimmer", whole regions lose light, and
the ten seeded worlds spread apart in mean luminance. A compensating lift
elsewhere restores the mean and leaves the spread exactly where it was, which is
why the fix belongs at the shift rather than after it.

**Isotropy is a look, and it is the wrong one.** Grass with a uniformly random
heading looks the same in every direction, so nothing at the middle scale
survives except the outline of each clump. One low-frequency flow field orients
the ridges, the tuft headings, the mat and the worn openings, loosely and with a
minority ignoring it — enough to give the eye somewhere to travel, not enough to
comb.

**Shade the shapes you know, do not differentiate the shape you built.** The
mounds are domes, and a dome's normal is known in closed form, so each one shades
itself analytically and the results are averaged where they overlap. The obvious
alternative — finite-difference the composited height field and treat the
gradient as a normal — works and is quietly wrong: the composite is read back off
a lattice, so its *slope* is piecewise constant and jumps at every lattice line,
which shows up as faint creases in the one thing that must not have any. The
canopy is also translucent rather than opaque, so its shaded side falls away at
about half the rate its lit side climbs and picks up a transmitted term where the
grass is thinnest. That is what makes a mound read as lit rather than cut out.

There is no simulation here yet. Wind, trampling and the animated crown layer
that would carry them are the next piece of work; today the surface is static.

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

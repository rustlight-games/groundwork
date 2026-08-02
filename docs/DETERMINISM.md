# Determinism

A battle must be a pure function of its seed and its inputs. The same seed must
produce the same battle on any machine, in the headless trainer and in the game,
today and after a refactor.

This is not perfectionism. Three things depend on it:

- **Training is trustworthy.** A policy learned against the trainer's rules only
  transfers if the game's rules are identical.
- **Bugs are reproducible.** "Seed 0xBEEF diverges at tick 412" is a bug report
  you can act on. "It looked wrong in that fight" is not.
- **Replays are cheap.** A replay is a seed plus a list of intents, which is
  kilobytes rather than the megabytes a state recording would cost.

## The rules

**No floats in simulation state.** Use `bw_core::Real` (fixed point). Floats are
not reproducible across platforms once fused multiply-add, x87 excess precision,
or a vectoriser's choices get involved. Fixed point is exact integer arithmetic.

The one sanctioned exception is content authoring: RON files use `f64` because
writing `0.35` is nicer than writing a bit pattern. Those values are converted
once, at load, and the simulation only ever sees the converted result. Parsing a
decimal literal and converting it to fixed point are both correctly rounded and
identical everywhere; *arithmetic* on floats during simulation is not, and does
not happen.

The other is observations, which are `f32` — see ARCHITECTURE.md for why that is
one-way and therefore safe.

**No `HashMap` or `HashSet`.** Iteration order is not deterministic. Use
`IndexMap`, `BTreeMap`, or a sorted `Vec`. `clippy.toml` denies the std types
workspace-wide.

**No wall-clock time.** No `Instant`, no `SystemTime`, no variable delta. The
simulation advances in whole ticks and every duration in content is in ticks.
Also denied in `clippy.toml`.

**Iterate by `UnitId`, never by `Query` order.** Query iteration follows
archetype and table layout, which shifts the moment a component is added or
removed — a unit gaining a status would quietly reorder the whole battle. Use
`UnitIndex::sorted_ids` and look entities up.

**Break every tie explicitly.** Equal-distance targets resolve to the lowest
`UnitId`. Equal-cost flow-field neighbours resolve to the first in a fixed
order. Coincident units separate by id order. Any comparison that can tie needs
a rule, or the outcome depends on iteration order.

**Never draw from a shared generator.** `SimRng` holds only a root seed; each
call site derives an independent stream from `(root, stream, tick, salt)`. Two
systems drawing in either order get the same answers, because neither draw
depends on the other having happened. Pass the acting unit's id as the salt.

Adding a new random draw needs a new `RngStream` variant, not a reuse of an
existing one — otherwise adding a crit roll silently changes every targeting
decision in the game and invalidates every trained policy.

**Queue effects, do not apply them inline.** Producers push to `EffectQueue`;
one exclusive system sorts and drains it. That is what makes effect resolution
independent of which system queued what first.

## Two properties that are easy to confuse

Fixed point buys **reproducibility** — the same answer everywhere. It does not
by itself buy **exactness**.

`1/60` is not representable in binary fixed point. It rounds, `dt * 60` comes
out at 0.9999999963, and integrating it over a long battle accumulates real
error — deterministically, but really.

So the tick rate is 64, not 60. `1/64` is exactly `2⁻⁶`: `dt` is exact,
sixty-four of them sum to exactly one second, and integration drifts by nothing
at all. The four extra ticks per second cost nothing. Choosing a power-of-two
rate buys correctness for free, on top of the reproducibility fixed point
already provided.

The same reasoning applies elsewhere: prefer power-of-two denominators when you
have the choice, and use `real_ratio` rather than a float literal when you do
not.

## A trap worth knowing

`fixed`'s `to_num` rounds toward negative infinity. Rust's `as` casts on floats
truncate toward zero. Code that "corrects for truncation" after calling `to_num`
double-corrects and lands one cell too low for every negative input — a bug that
only shows up on the left and bottom of a map.

Use `bw_core::floor_div_to_int` and `ceil_div_to_int` rather than rolling your
own. This was a real bug during initial development, in two crates at once.

## How it is enforced

`crates/bw_sim/tests/determinism.rs` is the instrument. It runs a battle twice
and compares the state hash *at every tick* rather than only at the end, so a
divergence that later re-converges is still caught. It also checks that:

- two simulations advancing in lockstep do not influence each other, which
  catches accidental global state — the thing that breaks a parallel trainer;
- the battle actually progresses, so the other tests are not trivially passing
  against a simulation where nothing happens;
- a pinned golden hash still matches, so a rules change is a deliberate edit
  visible in the diff rather than a silent invalidation.

When the golden hash test fails and the change was intentional, update the
constant in the same commit as the rules change.

`bw_core`'s hashing scheme is pinned by its own test for the same reason:
changing it invalidates every golden value in the repository.

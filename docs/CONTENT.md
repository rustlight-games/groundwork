# Content

Everything under `assets/content/` is RON, loaded by `bw_content` and validated
by `cargo run -p bw_forge -- validate`.

The goal is that adding a character or a spell is a file, not a code change.
With hundreds of units planned, one Rust type per spell would mean the codebase
grows linearly with the content, every addition needs a recompile, and nobody
without a compiler can contribute.

## Layout

```
assets/content/
  characters/   fieldable units
  abilities/    activated abilities
  status/       timed modifiers
  terrain/      tile types
  rocks/        procedural rock definitions
  props/        sprite-based scatter (trees, bushes, debris)
  encounters/   battle setups
```

Files load in sorted filename order, and that order assigns `ContentId`s. Ids
end up inside the observation vectors fed to the network, so the ordering is
part of the training contract — renaming a file can shift ids and invalidate a
trained policy. Prefer adding files over renaming them.

## Abilities are trees

An ability composes registered primitives. Each node names a handler, optionally
selects targets, carries parameters, and may have children.

```ron
(
    key: "cleave",
    name: "Cleave",
    cooldown_ticks: 96,
    effect: (
        kind: "damage",
        targeting: Some((
            shape: Cone(radius: 2.5, arc_degrees: 110.0),
            filter: Enemies,
            sort: Nearest,
            max_targets: 4,
        )),
        params: { "amount": Num(18.0), "variance": Num(0.2) },
    ),
)
```

Targeting is separate from payload on purpose: the same `damage` primitive backs
a cone, a chain, and a single-target nuke.

Registered primitives today: `damage`, `heal`, `apply_status`, `sequence`,
`terrain_mud`. `knockback`, `projectile`, `chain`, `aura` and `summon` follow
the same pattern and are the obvious next additions.

Prefer composing existing primitives. A new primitive is justified when several
abilities want it, not when one does.

## Durations are in ticks

64 ticks per second. Content never specifies seconds, so it cannot introduce
rounding. `cooldown_ticks: 96` is a second and a half.

## Numbers

Stats and parameters are authored as decimals and converted to fixed point once,
at load. Write `18.0` or `18` — both work where a number is expected. See
DETERMINISM.md for why this is safe and why arithmetic on those values during a
battle would not be.

## Rocks and props are different things

**Rocks are generated.** The game needs a great many of them, at many sizes and
silhouettes, and hand-drawn variants would run out or start visibly repeating. A
`RockDef` names a generator and its parameters; the output is geometry, which
lets the renderer rasterise it, the simulation use the outline as a collider,
and `bw_bench` score the silhouette.

**Props are drawn.** Trees, bushes and debris are authored sprites placed by a
`ScatterRule`. Trees are more recognisable than rocks, fewer are needed, and
procedural foliage is a much harder problem than procedural stone.

Terrain is the other generated thing, and it feeds three consumers at once:
movement costs for navigation, density for the grass renderer, and elevation for
prop placement.

## Validation

`bw_forge validate` checks every cross-reference: characters point at abilities
that exist, effect trees name registered primitives, rocks and encounters name
registered generators, and generator parameters are in range.

Run it before committing content. It is fast, and it catches the whole class of
mistakes that would otherwise surface as a mid-battle no-op.

## Known rough edge

`apply_status` takes its status as an integer `ContentId` rather than a key
string, because parameter values are not resolved against the interner at load
time. That is awkward to author and should be fixed by teaching `Params` a
key-to-id resolution pass. The seed content avoids it for now.

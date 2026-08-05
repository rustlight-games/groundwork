# Nothing can be looked at

The field stack is the canonical low-fidelity representation and there is no way
to see a single plane of it. The candidate domains decide where everything is
and there is no way to list one. The ownership draw decides what everything
becomes and there is no way to ask why.

This is the cheapest outstanding item and the one that will save the most time,
because the failures it addresses are the ones where **a boundary looks wrong
and no correctness test fails**.

## What exists

```sh
terrain validate <document>            # every problem, in one pass
terrain inspect  <document> --at U,V   # the composed sample, and every source
```

`inspect` is genuinely good at its one job: it prints the composed sample *and*
every source's own value, because a material weight that is wrong is nearly
always a mask that is wrong. It stops there. It does not know about the field
stack, the derived fields, the transition solver, candidates or owners — all of
which are downstream of it and all of which can be the thing that is wrong.

## What is missing

**Field plates.** One command, one channel, one image.

```sh
terrain fields <document> --at U,V --list
terrain fields <document> --at U,V --channel slope
terrain fields <document> --at U,V --channel tuft_density
```

`DerivedFieldSet::scalar_planes` and `vector_planes` already hand back every
plane with its key, precisely so a debug exporter can walk them without knowing
what any of them mean. The exporter is the missing half.

**Candidate and ownership views.**

```sh
terrain candidates <document> --at U,V --domain vegetation.tuft_anchor
terrain ownership  <document> --at U,V --domain vegetation.tuft_anchor
```

Candidate positions, which were accepted, which were thinned by a
higher-priority neighbour, and which owner each accepted one went to. The
transition boundary is where all three of those interact and it is currently
inspected by rendering it and squinting.

**One candidate, explained.**

```sh
terrain explain-candidate <domain> <cell_u> <cell_v> <rank>
```

Position, every named random value, the local field sample, the target density,
the acceptance threshold, the priority and which neighbours conflicted with it,
every owner's score, the winner, and the ids of the children it emitted.

Addressed randomness makes this possible in a way a sequential generator never
could: every one of those values is knowable from the candidate's address alone,
without having generated anything else. The mechanism is built; nothing prints
it.

**Two scenes, compared.** `terrain compare-scene before.scene after.scene` —
what moved, what changed hands, what appeared. `terrain_bench::compare` already
does the numeric half for measurements.

**Semantic preview.** A false-colour view of material, density and derived
fields, so the beauty render stops being the only debugging surface. This one
is entangled with [render-paths.md](render-paths.md): if the rasterisers go, the
semantic view is the thing worth keeping from them.

## `inspect`, extended

The full chain, at one point:

```text
world position
source values
layer contributions
normalised substrate weights          ← stops here today
realised substrate                    (after the transition solver)
modifier values
derived terrain features
candidate domains covering the point
population affinity, target density, owner scores
```

The line worth adding first is the realised substrate, because it is the one
place the composed weights and the picture legitimately differ — the transition
solver perturbs them — and today the tool reports the smooth answer while the
render shows the ragged one.

## Done looks like

Somebody investigating a wrong-looking boundary can answer "is the mask wrong,
is the realisation wrong, is acceptance wrong, or is ownership wrong?" without
building a one-off binary. All four are separately printable.

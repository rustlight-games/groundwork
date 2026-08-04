# Authoring terrain

A terrain document says what the ground *is*. Everything downstream — the
sampler, the scene, both renderers — is derived from it.

## Three vocabularies

Keeping them apart is most of why the model has the shape it does.

- **Sources** are fields: a constant, some noise, a painted mask, the distance
  to a spline. They say nothing about terrain on their own.
- **Layers** say what the terrain **is**, continuously. A layer takes a source,
  shapes it into a mask, and applies one operation.
- **Populations** say what **grows or sits on** it: discrete, countable things
  with their own identities.

The line between the last two matters. Layers produce continuous fields that
every population reads; populations produce marks that no layer reads. Letting a
population contribute to material weight would make composition circular, and
there would be no answer to "what is the material here" that did not depend on
which population ran first.

## The smallest document that says anything

```ron
(
    format: "terrain-document",
    format_version: 1,
    document: (
        root_seed: "8df782f95ce1a4d4",
        materials: [(key: "grass_lush", appearance: "surface.grass_lush")],
        sources: [(key: "everywhere", source: Constant((value: 1.0)))],
        layers: [(
            key: "base_grass",
            mask: Source("everywhere"),
            operation: Material((material: "grass_lush", mode: "Replace", amount: 1.0)),
        )],
    ),
)
```

## One spline, four layers

The thing the model exists for. `blend_lab.terrain.ron` writes a path once and
reads it four times — for material, for a depression, for vegetation
suppression, and for grit. Moving the spline moves all four, because they are
all reading one source.

```ron
(
    key: "path_material",
    mask: Profile((source: "main_path", shape: SmoothBand((inner_m: 1.5, outer_m: 2.6)))),
    operation: Material((material: "dirt_compacted", mode: "AddScore", amount: 1.0)),
),
(
    key: "path_depression",
    mask: Profile((source: "main_path", shape: SmoothBand((inner_m: 1.4, outer_m: 2.3)))),
    operation: Microrelief((mode: "Add", metres: -0.06)),
),
(
    key: "path_vegetation_suppression",
    mask: Profile((source: "main_path", shape: SmoothBand((inner_m: 1.4, outer_m: 2.8)))),
    operation: Modifier((channel: "vegetation_density", mode: "Multiply", value: 0.15)),
),
```

The suppression band is *wider* than the material band on purpose. Grass thins
slightly beyond where the ground stops being grass, and a path whose vegetation
stops exactly where its dirt starts reads as a decal.

## Modifier channels are declared

A layer that writes `vegetaion_density` and a population that reads
`vegetation_density` are, without declarations, two channels that never meet and
never complain. So every channel is declared once, with a range, a default, a
unit and a composition rule:

```ron
(
    key: "vegetation_density",
    range: (0.0, 1.5),
    default_value: 1.0,
    composition: "Multiply",
    unit: "Unitless",
),
```

**The composition rule belongs to the channel, not the writer.** A channel whose
rule varied by writer would have no well-defined value — the result would depend
on layer order in a way that is invisible in the document. A layer may still say
which rule it is using, and validation checks the two agree.

`Multiply` for anything two writers should compound, like a suppression.
`Max` for anything a region *grants* rather than modulates: two overlapping rock
zones should not double the rocks.

## Masks fade, they do not switch

A modifier at a mask of 0.5 is half applied. A partial `Replace` leaves
proportionally what was there. Without that, every layer has a hard edge
wherever its mask crosses zero, however smooth the mask is.

Channels are clamped once at the end, not per layer — clamping per layer makes
the result depend on the order of layers that each individually overshoot.

## Material scores, not weights

`AddScore` accumulates an unbounded score; normalisation happens once, at the
end. So an author writing several overlapping claims does not have to keep them
summing to one, and a claim can be turned up without every other one being
adjusted.

## Populations

```ron
(
    key: "meadow_flowers",
    recipe: "population.wildflowers_meadow",
    seed_stream: "flowers",
    material_affinity: [("grass_lush", 1.0)],
    abundance_channel: Some("flower_abundance"),
    parameters: [("density", Number(6.0))],
),
```

`material_affinity` is how readily it grows on each material; `abundance_channel`
is the declared channel that scales it. The product is the acceptance rate. An
empty affinity means "anywhere" — a rock does not care what grows around it.

Two populations sharing a `seed_stream` land in the same places. Legal, and
warned about, because it is much more often a copy-paste than an intention.

## What is refused

Unknown fields are errors. A misspelled `transition_width_m` that silently does
nothing is the worst failure authored content has: the document loads, the
terrain is wrong, nothing says why, and the author's next move is to change the
value — which also does nothing.

Validation reports everything it finds in one pass, with a path and often a
suggestion:

```
error[unknown_source] layers[3].mask.source: no source named `everywere`
  help: did you mean `everywhere`?
```

Errors mean the document cannot be prepared. Warnings mean it can and probably
should not — an unread source, two populations sharing a stream. Warnings that
cannot be silenced become noise, and noise trains people to ignore the errors
printed beside them, so a warning here names something somebody would actually
want to fix.

## Assets

Relative to the document, always. An absolute path is a document that only works
on the machine it was written on.

A spline is one `u v` pair per line, `#` for comments, `closed` on its own line:

```
# The main path.
-40   -18
-28   -12
-16    -4
```

Not RON, despite the extension. A path is hundreds of points from a tool, and
one point per line means a diff shows *which part of the path moved* where a RON
array reflows and shows all of it.

## Checking it

```sh
terrain validate assets/terrain/documents/blend_lab.terrain.ron
terrain inspect  assets/terrain/documents/blend_lab.terrain.ron --at 0,5
terrain inspect  assets/terrain/documents/blend_lab.terrain.ron --at 0,5 --source main_path
```

`inspect` prints the composed sample *and* every source's own value, because a
material weight that is wrong is nearly always a mask that is wrong, and seeing
both at once is the difference between a guess and an answer.

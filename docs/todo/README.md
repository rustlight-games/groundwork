# To do

What [LOW_TO_HIGH_FIDELITY_SPEC.md](../LOW_TO_HIGH_FIDELITY_SPEC.md) asked for
and this repository does not have yet.

The spec is a description of the system as built. Everything it once described
in the future tense lives here instead, one file per gap, so that neither
document has to hedge — the spec says what is, and these say what is not.

Each file states the gap, why it matters, what already exists to build on, and
what would make it done. They are notes for whoever picks the work up, not
tickets: no estimates, no ordering beyond what genuinely blocks what.

## The gaps

| File | Gap |
| --- | --- |
| [covers-and-snow.md](covers-and-snow.md) | `CoverPlane` is a type nothing fills. No cover solver, no snow. |
| [authoring-model.md](authoring-model.md) | Rasters parse but do not compile. No derived sources, no cover declarations, no profile assets. |
| [feature-context.md](feature-context.md) | `FeatureContext` is declared, never populated. No tangent, no along-feature distance, no junctions. |
| [one-grass-generator.md](one-grass-generator.md) | Two grass implementations: the tuned one and the compiler's families. Only one of them looks right. |
| [render-paths.md](render-paths.md) | Cycles still traces through `GrassScene`. The generic package is written and unused. |
| [dataset-tile-shape.md](dataset-tile-shape.md) | The corpus is still page-shaped, and the renders it pairs are tile-shaped. |
| [diagnostics.md](diagnostics.md) | The field stack, the candidates and the ownership draw cannot be looked at. |
| [dirt-finish.md](dirt-finish.md) | Cracks, pebbles, ruts, sorted fines, authored colour. |
| [elevation.md](elevation.md) | The world is flat, and most of the derived fields are waiting for it not to be. |
| [measurement.md](measurement.md) | The suite measures the old renderer. The compiler's counters reach no baseline. |

## The order that is real

Three of these block others and the rest do not.

- **[one-grass-generator.md](one-grass-generator.md) blocks
  [render-paths.md](render-paths.md).** There is no point making the generic
  package the only Cycles route while the only good-looking grass is on the
  other one.
- **[render-paths.md](render-paths.md) blocks
  [dataset-tile-shape.md](dataset-tile-shape.md).** A tile-shaped corpus pairing
  two different generators is worse than a page-shaped one pairing the same.
- **[elevation.md](elevation.md) blocks nothing and unblocks a lot.** Slope,
  curvature, flow and exposure are all derived and carried today, and on a flat
  world three of the four are constant, so nothing that reads them can be
  evaluated.

Everything else is independent. [covers-and-snow.md](covers-and-snow.md) in
particular needs no other file finished first — the field stack already has the
group reserved and the solver has no dependency on which generator draws the
grass.

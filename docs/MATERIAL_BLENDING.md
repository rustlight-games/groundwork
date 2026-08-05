# Material blending

Composing *material weights* and letting one shared candidate field decide
ownership, rather than blending two finished pictures.

Built, on the production path, and load-bearing: `meadow_path` sweeps one band
from pure meadow to bare earth and the transition emits one mark per accepted
candidate.

## The obvious wrong thing

```text
render a complete grass image
render a complete dirt image
alpha-blend the two
```

Every failure it produces is characteristic, and none is subtle once seen:

- **Transparent grass ghosts.** A blade at 30% opacity is not sparse grass; it
  is a blade you can see through.
- **Double mark density.** Both materials generated a full population, so the
  transition has twice the marks of either side and reads as a busy stripe.
- **Muddy colour.** Averaging two finished pictures averages their shading, so
  the transition is flatter than either side and its highlights are halved.
- **Unstable clump placement.** Nothing ties one image's clumps to the other's,
  so they interleave differently at every blend value.
- **Inconsistent depressions and shadows.** Two renders each computed their own,
  and neither is the depression the ground actually has.

The reference plates confirm the first two visually. On
`references/grass_to_mud_transition.jpg` the isolated clumps standing on bare
mud are full height and full density — grass thins by losing whole tufts, and
nothing anywhere is a half-opacity blade.

## The interface is normalised weights

```text
meadow_soil:     0.72
dirt_compacted:  0.28
```

That is the whole global semantic model. `MaterialWeightSet` enforces finite,
non-negative, normalised, zero-pruned and ordered; the document composes
unbounded scores and normalisation happens once at the end, so an author writing
several overlapping claims does not have to keep them summing to one.

`Replace` is what lets a band reach the ends of the range. `blend_lab` adds a
dirt score of one on top of a grass score of one, so its path centre normalises
to an even split and it cannot express bare ground at all — fine for a blending
laboratory, wrong for a track, because the middle of a worn path is worn rather
than half-worn.

## The evaluation order

Blending affects **procedural decisions before any RGB exists**.

```text
sample terrain and environmental fields
query splines, shapes and painted edits
compose material scores
normalise scores to material weights
derive the dominant pair, the blend amount and the boundary tangent
realise the boundary                            terrain_generators::transition
evaluate one shared candidate domain            terrain_generators::domain
accept or reject — this fixes the count
assign each accepted candidate one owner        terrain_generators::ownership
sample material-owned attributes
emit the scene
```

Acceptance before ownership is the entire mechanism. The number of things is
settled while the materials are still an undecided question, so a 70/30 boundary
emits exactly what the pure ground on either side does.

## One candidate field, not two

1. One **stable candidate field**, independent of material.
2. A **blended target density**, from the weights.
3. One **stable acceptance draw** per candidate.
4. **Material weights decide ownership**: a separate draw picks which recipe
   gets it.

Candidate positions and latent attributes stay stable as the blend changes; only
acceptance and ownership move. That is what prevents popping. Nudge a path's
edge by a centimetre and the marks that stay do not move — a handful change
hands or disappear. Under the two-population approach every mark on both sides
is regenerated and the whole band shimmers.

The ownership score is a product, because every term is a veto:

```text
owner_score_k = substrate_affinity_k · abundance_k · profile_weight_k · boundary_k
```

A sum lets a large abundance drown a zero affinity, which is how grass ends up
growing on bare rock at low density instead of not at all.

An accepted candidate that no recipe wants is left unowned. That is bare ground,
and it is counted — `SceneCompileReport::candidates_unowned` — because a hole
that nobody claims looks exactly like sparse grass and no test would otherwise
notice it.

## The weights are not the boundary

The document's mask says where the ground is *changing*. It does not say what
the change looks like inside that band, and a monotone ramp rendered directly
reads as an airbrushed decal.

`terrain_generators::transition` perturbs each material's score by its own noise
before normalising, scaled by a contest term that is one where two materials are
evenly matched and zero where one already owns the ground. The realised contour
then moves by roughly `amplitude / |∇score|` metres — so a wide gentle band gets
big islands and a tight band gets a crisp edge from the same raggedness setting.

It is evaluated, never baked. Ownership and ground shading call the same
function, or the mud is painted in a slightly different place from where the
grass thinned and the transition reads as two effects that nearly line up. The
cheap tier's ground pass calls it for exactly this reason, and so does the
`SemanticOverlay` that carries an authored document into the tuned generator.

Full derivation in
[LOW_TO_HIGH_FIDELITY_SPEC.md](LOW_TO_HIGH_FIDELITY_SPEC.md); the reasoning
about what the references show is in [references/](references/).

## Pair-specific transitions are content, not topology

Normalised weights are the global model. A pair recipe — sparse edge grass,
clipped blades, exposed roots, loose soil — is *optional decoration* on a
boundary the weights already decided.

The rule that keeps this from rotting: **a pair recipe must not own terrain
topology.** The moment where-the-boundary-is comes from a pair recipe rather
than from the weights, adding a third material means writing three pair
recipes, and the model has become a matrix.

None exist yet. What produces the boundary's character today is the raggedness
plus the bands being deliberately non-concentric — in `meadow_path` the
vegetation band is 65 cm wider than the material band, so grass thins beyond
where the ground stops being grass. A path whose vegetation stops exactly where
its dirt starts reads as a decal laid on a lawn.

## What is still reserved

`FeatureContext` is carried on every sample and never populated:

```rust
pub struct FeatureContext {
    pub feature_id: FeatureId,
    pub signed_distance_m: f32,
    pub tangent: [f32; 2],
    pub normal: [f32; 2],
    pub along_feature_m: f32,
    pub junction: JunctionClass,
}
```

It is what a rut aligned to a track, a width that narrows along a path, or a
junction that pools differently from a bend would read. `boundary_tangent` in
the derived fields covers the most common use of `tangent` and is not a
substitute — see [todo/feature-context.md](todo/feature-context.md).

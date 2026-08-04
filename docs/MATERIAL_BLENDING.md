# Material blending

**Reserved, and deliberately unimplemented.** The weights compose today; the
mechanism that stops a transition doubling its marks does not exist yet. This
records the design so that when it lands it is content and a recipe rather than
another architectural change — and so nobody implements the obvious wrong thing
in the meantime.

## The obvious wrong thing

```text
render a complete grass image
render a complete dirt image
alpha-blend the two
```

Every failure it produces is characteristic and none of them is subtle once
seen:

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

## The interface is normalised weights

```text
grass_lush:      0.72
dirt_compacted:  0.28
```

That is the whole global semantic model, and it already works —
`MaterialWeightSet` enforces finite, non-negative, normalised, zero-pruned and
ordered, and a `SmoothBand` over a spline distance produces it.

## The evaluation order

Blending has to affect **procedural decisions before any final RGB exists**.

```text
sample terrain and environmental fields
query splines, shapes and painted edits
compose material scores
normalise scores to material weights
derive substrate mixture
derive dominant pair and boundary frame
evaluate a shared candidate population        ← the part that is missing
assign each accepted candidate to one material
apply boundary-specific replacement rules
sample material-owned attributes
emit the scene
```

Steps one to six work today. Everything from the shared candidate population
down is the reserved half.

## One candidate field, not two

The mechanism, and the reason it has to be built rather than approximated:

1. One **stable candidate field**, independent of material.
2. A **blended target density**, from the weights.
3. One **stable ownership draw** per candidate.
4. **Material weights decide ownership**: the draw picks which material gets it.

So a transition emits *one mark per accepted candidate* rather than one per
material. Candidate positions and latent attributes stay stable as the blend
changes; only acceptance and ownership move.

That last property is what prevents popping. Nudge a path's edge by a
centimetre and the marks that stay do not move — a handful change hands or
disappear. Under the two-population approach, every mark on both sides is
regenerated and the whole band shimmers.

`terrain_generators::population` already has the shape: candidates carry an
identity that exists whether or not anything grows there, and a test asserts
that lowering an abundance removes candidates without moving the survivors. What
is missing is a candidate field shared *between* populations rather than one per
population.

## What `TerrainSample` already reserves

`FeatureContext` is carried and mostly unread, because the things it enables all
need the same information and none can compute it afterwards:

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

- **`tangent`** — ruts aligned to a path, grass leaning away from a track,
  stones following a boundary.
- **`along_feature_m`** — anything that varies *along* a path rather than only
  across it: a width that narrows, a surface that gets rougher.
- **`junction`** — a T behaves differently from a crossing, and both differ from
  a bend.

## Pair-specific transitions are content, not topology

Normalised weights are the global model. A pair recipe — sparse edge grass,
clipped blades, exposed roots, loose soil, an irregular boundary width — is
*optional decoration* on a boundary the weights already decided.

The rule that keeps this from rotting: **a pair recipe must not own terrain
topology.** The moment where-the-boundary-is comes from a pair recipe rather
than from the weights, adding a third material means writing three pair recipes,
and the model has become a matrix.

## Why this is not built yet

Because there is one material. A blending system designed against one example is
a system designed against a guess, and the parts that would be wrong are exactly
the parts that are expensive to change: what a shared candidate field is keyed
on, and what a boundary frame carries.

What exists now is the part that is expensive to *retrofit* — normalised weights
in the sample, a candidate model with stable identity, and feature context
reserved in the type. Those are load-bearing. The rest waits for dirt to be a
material somebody has actually looked at.

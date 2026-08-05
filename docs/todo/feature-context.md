# Feature context

**Declared, never populated, never read.** `terrain_core::sample::FeatureContext`
sits on every `TerrainSample` as an `Option` that is always `None`.

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

## Why it is reserved rather than deleted

Every field on it has to be computed while the spline is still being evaluated.
A recipe handed only a distance cannot recover which way the path was running,
how far along it the point was, or whether it was standing in a junction — those
are properties of the query, not of the answer, and reconstructing them
afterwards means re-walking the spline at a different rate and disagreeing with
the sampler about where the path is.

So the cost of *not* having it is not that these effects are hard. It is that
they are impossible without changing the sampler, which is why the type went in
before the producer.

## What it would buy

- **`tangent`** — ruts running along a track rather than across it, elongated
  grit oriented to traffic, grass leaning away from an edge, stones settling
  along a boundary. Today a rut can only be a symmetric depression because
  nothing knows which way the path points.
- **`along_feature_m`** — anything varying *along* a path rather than only
  across it. A track that narrows, ground that gets rougher where it climbs,
  damage that concentrates in one stretch. Today every metre of a path is
  statistically the same metre.
- **`junction`** — a T behaves differently from a crossing and both differ from
  a bend. Water pools at junctions, wear concentrates there, and the vegetation
  band is wider on the inside of a curve.

## What partly covers for it

`DerivedFieldSet::boundary_tangent` gives the direction the *substrate boundary*
runs, which is the tangent's most common use and is available today on the
lattice. That is why the absence has not bitten yet.

It is not a substitute. A boundary tangent exists only where two substrates
meet, so it says nothing in the middle of a wide path, and it is the boundary's
direction rather than the feature's — on a path with a ragged edge those differ
by whatever the transition solver did locally, which is exactly the wrong
amount.

## Done looks like

- `prepare` compiles spline sources into something that answers all six fields,
  not just distance.
- A point sample inside a path band comes back with a tangent that matches the
  spline's own direction there, checked against a fixture rather than eyeballed.
- `terrain inspect` prints it — see [diagnostics.md](diagnostics.md).
- One recipe reads it. A rut aligned to `tangent` is the smallest honest
  demonstration and it is also on the dirt list; see
  [dirt-finish.md](dirt-finish.md).

## The trap

Rasterising it onto field planes as the *primary* representation. A junction
class bilinearly interpolated is meaningless, and a tangent averaged across a
fork points at neither branch. The structured point sample is the real value;
optional planes are a convenience for the renderer and have to declare
themselves categorical or renormalised, which `FieldDescriptor` already supports.

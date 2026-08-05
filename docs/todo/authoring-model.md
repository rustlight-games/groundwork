# The authoring model

Four things an author cannot say. Each is a hole in the document, not in the
pipeline underneath it — in every case the thing the document would drive
already exists.

## Rasters parse and do not compile

`terrain_format::raw` reads three of them — `ScalarRaster`, `CategoricalRaster`
and `WeightRaster` — with full placement, filtering and wrap declarations, and
`terrain_core::document` carries them. `terrain_core::prepare` then refuses
them with a message, because compiling one needs an image decoder and
`terrain_core` depends on nothing but `serde`.

That dependency rule is the actual obstacle and it is worth keeping. The way
through is to decode outside the core and hand `prepare` a resolved buffer,
rather than to let `terrain_core` grow an image crate.

It matters more than it looks. A painted mask is the direct route from a
low-fidelity matrix somebody drew into the same semantic pipeline as everything
else, and until it works the only ways to say where something is are a spline, a
constant and noise. `blend_lab` and `meadow_path` both read splines because
splines are what there is.

Nearest for categorical planes and bilinear for continuous ones is already
declared per source and already validated. What is missing is the decode and the
sampler behind it.

## A document cannot read a field it produced

There are no derived sources. An author can write a slope-dependent layer only
by re-deriving slope from a noise field, which is not the same slope the
compiler carries and will not agree with it.

The list worth having is small and all of it is already computed once per render
in `DerivedFieldSet`:

```text
Slope · Aspect · Curvature · Exposure
FlowAccumulation · FlowDirectionX · FlowDirectionY
MaterialWeight(material) · ModifierValue(channel)
FeatureSignedDistance(feature)
```

The constraint is ordering, not arithmetic. A derived source reads fields that
layer composition produces, so it has to run in a declared post-sampling phase
rather than recursively re-entering composition — otherwise a document can
describe a cycle and validation has to find it.

Note that this is largely waiting on [elevation.md](elevation.md). On coplanar
tile bases slope is microrelief-scale everywhere and curvature is close to
noise, so a slope-driven layer has almost nothing to key on.

## A document cannot declare a cover

No `covers:` list, no `Cover` operation, no `cover_solvers:` section. See
[covers-and-snow.md](covers-and-snow.md) — the whole feature is missing, and
this is the author-facing half of it.

The shape it should take, when it lands:

```ron
covers: [
    (key: "snow_fresh", appearance: "cover.snow_fresh", depth_range_m: (0.0, 0.6)),
],
```

with layers writing depth through an operation, and a solver reading a declared
input channel rather than a layered depth directly — so that snowfall can be
painted and *accumulation* still computed.

## Morphology has nowhere to live but the generator

There are no vegetation profile assets. A grass profile — blades per tuft,
length and width ranges, bend, fork probability, dryness response — is a
semantic bundle that several documents should share, and today the only place
those numbers exist is inside the tuned generator's own tuning, where a document
cannot reach them.

The consequence is [one-grass-generator.md](one-grass-generator.md): the
compiler's `families` recipes had to invent their own morphology because there
was no asset to reference, and now there are two sets of numbers describing the
same plant.

Profiles as authored assets, referenced by populations, is the fix. It also
keeps forty morphology parameters from being copy-pasted into every terrain
document, which is the failure mode that makes an authoring model unusable
rather than merely incomplete.

## What is *not* missing

Worth stating, because the spec listed them together and the gap is uneven.

- **Modifier channels are fully declared** — range, default, composition rule
  and unit, checked against every writer.
- **Material scores, normalised once at the end**, with `Replace` able to sweep a
  band from pure to pure. `meadow_path` depends on it.
- **Unknown fields are errors** and validation collects every problem in one
  pass with a suggestion.
- **Populations declare affinity and an abundance channel**, and both reach the
  ownership draw.

A population does *not* declare its candidate domain — the recipe does, through
`PopulationRecipe::domain`, and the first recipe naming a domain supplies its
capacity. That was a deliberate departure from the spec's sketch: capacity is a
property of the lattice rather than of any one occupant, and two populations
sharing a domain have to agree about it. Making it author-facing would let a
document declare two capacities for one lattice. If it ever becomes visible in
canonical form it should be as a *check*, not as an input.

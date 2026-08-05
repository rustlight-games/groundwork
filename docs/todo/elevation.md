# The world is flat

All nine tile bases are coplanar. No steps, no cliffs, no raised platforms, no
camera pitch. The grass mound field stays, because it is surface-scale variation
rather than gameplay elevation.

This is deliberate and it was the right call — elevation arrives once the layout
is settled, so that a bad result has one possible cause rather than three. The
layout is settled now.

## What is idle because of it

More than it looks. Four derived fields are computed, carried, digested and
read, and on coplanar ground three of them are near-constant:

| Field | On flat ground | What it is for |
| --- | --- | --- |
| `slope` | microrelief-scale everywhere | snow retention, growth limits, rock settling |
| `aspect` | near-arbitrary where slope is ~0 | sun response, snow facing, dryness |
| `curvature` | clod-scale noise | snow collection, moisture pooling, grain sorting |
| `exposure` | ~1 everywhere, eight rays for nothing | shelter, deposition, moisture persistence |
| `flow_accumulation` | works, on microrelief | wetness, mud, erosion, debris sorting |

`flow_accumulation` is the one that earns its keep today — the
`SemanticOverlay`'s wetness term reads it, so a hollow on a path declared dry is
still the wettest part of it. The rest are correct code with nothing to be
correct about.

Three of the To Do items are waiting on this rather than on each other:

- **[covers-and-snow.md](covers-and-snow.md)** — deposition is a product of
  slope retention, facing and shelter. On a plane it is a constant, and the
  interesting behaviour of a snow solver is entirely the behaviour on terrain
  that is not a plane. The slump solver in particular has nothing to slump.
- **[authoring-model.md](authoring-model.md)** — derived sources let a document
  key a layer on slope or curvature. On flat ground there is nothing to key on.
- **[dirt-finish.md](dirt-finish.md)** — loose-material sorting reads curvature
  and flow. Microrelief gives it something, but the macro sorting a real path
  has comes from the path having a crown and a camber.

## What it needs

**Elevation in the document.** The layer operation exists — `Elevation` with a
height mode and metres — and `PreparedTerrain` samples it. What has never been
exercised is a document that uses it for anything larger than a path depression,
and the parts that would show the strain are the ones nothing has stressed: the
ground mesh's tessellation budget over real relief, and the projection.

**A camera that can look at it.** The projection is orthogonal down
`(1,1,1)/√3`, 35.26° above the ground, and it is fixed. Elevation is visible in
it — `screen.y` carries `+z` — so a step reads as a step without any camera
change. What does not exist is any way to pitch, and whether that is wanted is a
gameplay question rather than a rendering one.

**A decision about steps versus slopes.** A continuous height field and a tiled
world with discrete levels are different products, and the layout does not
imply either. Diablo II's tiles carry a height; Dota's terrain is a grid with
cliffs between levels. This repository has neither and should choose before it
builds one, because the answer changes what a world tile *is* — and world tiles
are a composition and never a generation boundary, which a cliff at a tile edge
would quietly violate.

## Done looks like

- A document can raise ground, and the nine-tile plate shows it without a seam
  at any internal join.
- Slope and curvature carry real signal, and something reads them: grass
  thinning on a steep face is the smallest honest demonstration.
- The vertex budget in `terrain_cycles::plate` still computes a workable trace
  tile count over relief, rather than Blender taking a segmentation fault
  several minutes in.
- Shadows fall across the internal joins of a stepped layout as readily as they
  do across a flat one.

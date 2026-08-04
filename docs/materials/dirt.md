# Dirt

**Minimal.** A material a document can name, a population that scatters grit on
it, and no procedural artwork.

## What exists

- `dirt_compacted` as a material key, bound to `surface.dirt_compacted`.
- A ground shader in `render.py`, currently the same warm olive-brown the
  substrate between grass clumps uses.
- `population.dirt_scatter` — flat analytic discs, keyed on a `grit_abundance`
  channel.

That is enough for `blend_lab.terrain.ron` to describe a path: the ground there
is dirt, six centimetres lower, with vegetation suppressed and grit scattered.

## Why the substrate colour matters more than its area suggests

It is the only warm colour in the picture. It separates one tuft from the next,
makes a density change legible, and gives the eye somewhere to rest. A canopy
with nothing at all between it reads as fur rather than as plants standing in
ground — which is why the grass density was reduced from eight times the
rasteriser's counts to seven: at eight the canopy sealed completely.

## What a finished dirt needs

- Procedural grain at two or three scales, so it does not read as a flat wash.
- Compaction varying along a path — a track is harder in the middle.
- Moisture darkening in the depression, since water collects where the ground is
  lower.
- Loose material that is *sorted*: fines in the hollows, coarse on the crown.

## What it must not become

A second full renderer. Dirt is a substrate — mostly continuous field with a
sparse population on top — and the temptation is to give it the same mark
vocabulary grass has. It does not need one, and building one would mean the
transition between them had two mark systems to reconcile rather than one shared
candidate field. See [../MATERIAL_BLENDING.md](../MATERIAL_BLENDING.md).

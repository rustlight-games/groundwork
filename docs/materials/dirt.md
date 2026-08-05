# Dirt

A substrate with measured colour, real relief, and a wet response. Still short of
what a finished earth needs, and no longer a placeholder.

## What exists

- `meadow_soil` and `dirt_compacted` as material keys, bound to
  `surface.meadow_soil` and `surface.dirt_compacted`.
- **A measured palette.** The earth colour is not chosen by eye — it is sampled
  off `docs/references/grass_to_mud_bumpy.jpg` over a 700 px square:

  ```text
  25%           linear [0.061, 0.031, 0.013]
  median        linear [0.084, 0.053, 0.030]
  brightest 5%  linear [0.195, 0.127, 0.072]
  ```

  The ratios matter more than the levels and they hold across the whole range:
  **G/R about 0.63, B/R about 0.36.** The old substrate ramp sat at G/R of nearly
  *one*, which is why a whole track of it read as sand — the error was the green,
  never the brightness.
- **Relief that is geometry, not a bump map.** `WorldField::earth_relief`
  displaces the ground mesh where a document exposes earth, at the scales bare
  ground actually has: clods of 2–8 cm standing about a third of their width, and
  crumb at 2–15 mm. Grain below that is a shader bump, because it is finer than
  any sane mesh resolves.

  The clods have to be mesh. A normal map cannot make a lump occlude its own
  shadow, and the shadows the clods throw are most of what makes bare ground read
  as ground.
- **Compaction flattens it.** `soil_compaction` scales the relief down, so a
  packed track is smooth and its shoulders are cloddy. That difference is most of
  what makes a path read as walked on rather than as a strip of different-coloured
  ground.
- **A wet response.** `soil_moisture`, and where water collects, drive it. Water
  fills the pores, so internal scattering falls and absorption rises: albedo
  darkens toward its own square, the hue warms because the film absorbs blue
  harder than red, and roughness collapses from about 0.78 to 0.20.

  The roughness is the important one. The sheen arrives before the darkening is
  noticeable, and darkening alone is exactly what makes wet ground read as ground
  in shadow instead of as wet ground.
- `population.dirt_clods` — analytic lumps keyed on `grit_abundance`, in the
  shared `surface.grit` candidate domain.

`meadow_path.terrain.ron` and `narrow_track.terrain.ron` both describe tracks
with this: bare in the middle, blending to meadow, vegetation thinning wider than
the material band.

## No subsurface scattering

Worth stating because it is the obvious thing to reach for and it is wrong.
Production mud shaders — Megascans, Substance, Unreal — do not use SSS. Mud is a
rough dark dielectric under a glossy coat, and scattering inside a surface that
absorbs this hard buys nothing for the cost. If a clay-heavy mud ever needs it,
the radius is half a millimetre to two, not centimetres.

## Why the substrate colour matters more than its area suggests

It is the only warm colour in the picture. It separates one tuft from the next,
makes a density change legible, and gives the eye somewhere to rest. A canopy
with nothing at all between it reads as fur rather than as plants standing in
ground.

Note that this cuts both ways, and the two cases need different colours. Earth
glimpsed *between blades* has to stay dark or it stops reading as ground seen
through a canopy and starts reading as a rock lying on top of one. Earth on an
*exposed track* is the subject and can be four times brighter. The ground
material carries both and mixes them by the `earth` attribute the compiler
writes.

## What a finished dirt still needs

- Desiccation polygons — 5–25 cm across, cracks 0.2–2 cm wide and up to 5 cm
  deep — driven by dryness.
- Embedded pebbles and grit as real instanced geometry rather than analytic
  discs.
- Ruts: foot and wheel deformation, 2–15 cm deep in churned mud and under half a
  centimetre on a packed path.
- Loose material that is *sorted*: fines in the hollows, coarse on the crown.
  The curvature and flow fields to do it with already exist.
- Authored colour. The palette above is in the Blender ground material rather
  than in the document, so an author cannot yet say whether their soil is a rich
  dark loam or a pale sand.

## What it must not become

A second full renderer. Dirt is a substrate — mostly continuous field with a
sparse population on top — and the temptation is to give it the same mark
vocabulary grass has. It does not need one, and building one would mean the
transition between them had two mark systems to reconcile rather than one shared
candidate field. See [../MATERIAL_BLENDING.md](../MATERIAL_BLENDING.md).

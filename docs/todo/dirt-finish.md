# Finishing the dirt

The measured colour, the clod relief, the compaction response and the wet
response all exist — see [../materials/dirt.md](../materials/dirt.md) for what
was measured and why. Five things are still missing, and they are the ones that
make bare ground read as ground somebody walked on rather than as a smooth
brown surface with lumps.

## Desiccation polygons

5–25 cm across, cracks 0.2–2 cm wide and up to 5 cm deep, driven by dryness.

Displacement on the ground mesh rather than a shader effect, for the same reason
the clods are geometry: a crack that cannot occlude its own interior does not
read as a crack. The dryness to drive it is a modifier channel a document can
already declare; nothing reads one.

## Pebbles and grit as geometry

`population.dirt_clods` emits analytic lumps into the `surface.grit` domain
today. Analytic is right for a clod, which is a lump of the same soil, and wrong
for a pebble, which has a distinct silhouette and a different material.

Prototype instances, per the prototype policy: described marks when variation is
cheap and continuous, prototypes when the silhouette is expensive and
distinctive.

## Ruts

2–15 cm deep in churned mud, under half a centimetre on a packed path.

This one is blocked, and honestly so. A rut runs *along* a track, and nothing in
the pipeline knows which way a track points — see
[feature-context.md](feature-context.md). A symmetric depression is what a
distance field can express and it is not a rut; it is a valley. `tangent` is the
missing input, and the depression band in `meadow_path` is where the rut would
go once it exists.

## Sorted loose material

Fines in the hollows, coarse on the crown. Moisture darkening the depressions.
Traffic smoothing the centre and pushing loose material outward.

The fields to do it with are all derived and carried: `curvature`,
`flow_accumulation`, `flow_direction`, and the compaction channel the document
declares. What is missing is a recipe that reads them when it places grit,
instead of scattering it uniformly across the band.

This is the cheapest of the five and the most visible per unit of work, because
sorting is what the eye reads as *history* — a surface where the coarse and fine
material are separated has been rained on and walked over, and one where they
are mixed has not.

The reference confirms it: on `grass_to_mud_transition.jpg` the last grass to
survive sits in the hollows, following the darker wetter channels. The same
field that should thin the grass should sort the grit.

## Authored colour

The measured palette lives in the Blender ground material, not in the document.
So an author cannot say whether their soil is a rich dark loam or a pale sand —
every track in every document is the same brown.

The ratios are the part to preserve when this moves: **G/R about 0.63, B/R about
0.36**, holding across the whole range. The old ramp sat at G/R near one, which
is why a track of it read as sand. A colour control that lets an author break
those ratios by eye will reproduce that failure, so the control should be
lightness and warmth over a measured base rather than three free channels.

## What it must not become

A second full mark system. Dirt is a substrate — mostly continuous field with a
sparse population on top — and giving it the same vocabulary grass has would
mean the transition between them had two mark systems to reconcile instead of
one shared candidate field. Everything above is either displacement on the
ground mesh or content in an existing domain. Nothing above needs a new one.

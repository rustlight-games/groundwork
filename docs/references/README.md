# Reference plates

What the renderer is aiming at, and what each reference is evidence *for*. These
are targets, not screenshots of this repository.

Stored through Git LFS — see `.gitattributes`.

**Look references are JPEG; measurement references are PNG**, and the difference
is not tidiness. A plate that only has to show what something should look like
survives compression fine, and a 10 MB PNG of one costs more than the detail it
carries at the size anybody views it. A plate whose *pixel values are quoted in
code* cannot be compressed at all — re-measuring a JPEG gives different numbers
from the ones in the source, and the discrepancy would look like a regression in
the renderer rather than a change in the reference.

## `grass_to_mud_transition.jpg`

A wide, ragged grass→mud boundary. The mud is in the upper right and the
transition runs diagonally.

What it establishes:

- **The realised boundary is broken up at tuft scale**, roughly 2–5 cm, while
  the *band* it happens over is 20–40 cm. Grass breaks into islands and
  peninsulas; mud reaches back in channels. A boundary driven only by the
  authored weight ramp is smooth at the band's scale and reads as an airbrushed
  decal.
- **Grass thins by losing whole tufts.** The isolated clumps standing on bare
  mud are full height and full density. Nothing is a half-opacity blade. This is
  the shared-candidate-ownership argument confirmed visually: acceptance falls,
  survivors keep their identity.
- **The last grass to survive sits in the hollows.** The green streaks follow
  the darker, lower, wetter channels — so curvature and flow accumulation belong
  in the abundance term rather than only in shading.
- **A dull thatch band sits between green grass and clean mud**, neither one nor
  the other.
- **Mud carries three scales**: broad wet/dry tonal sweep, 2–5 cm clods, and a
  fine grain, plus scattered grit. It darkens where it is lower.

## `grass_to_mud_bumpy.jpg`

A sharp turf edge, mud on the right, and much more pronounced mud relief.

What it establishes, mostly by *contrast* with the plate above:

- **Transition width is a parameter, not a constant.** Here the interpenetration
  zone is a few centimetres and the edge is nearly definite, where the other
  plate runs to tens of centimetres of islands. One system has to produce both,
  so band width, raggedness amplitude and raggedness frequency are three
  separate authored controls rather than one blend factor.
- **The grass mat has thickness.** The canopy stops and steps down to the dirt,
  and blades rooted at the edge lean out over it. The overhang is a consequence
  of edge tufts being full length, not a special case.
- **Mud relief is geometry, not texture.** The clods are 3–8 cm and cast their
  own shadows under a low sun. A normal map cannot produce that silhouette; the
  ground mesh has to carry it, which is what makes mud bumpiness an authored
  displacement channel.
- **A dark contact band sits at the foot of the turf** — occlusion plus retained
  moisture where the canopy meets bare ground.
- **Substrate hue is not fixed.** This mud is warm brown where the other is a
  cooler grey-brown, so colour variation is a channel rather than a constant per
  material.

## `grass-target.png`

The painted meadow the grass generator is measured against. Lossless, and it has
to stay that way: `terrain_bench::critique` quotes real measurements of a 1024²
crop of this file and builds its acceptance bands around them.

```text
median luminance   0.057
deep shadow        23.9%
highlight           6.7%
```

Those numbers are the reason the bands are where they are. Anyone re-deriving
them must measure *this* file, uncompressed — see `critique.rs` for what each
band means and, more usefully, for why passing one is not the same as matching
the painting.

It was under `docs/art/` and is here now because it is the same kind of thing as
the plates above: a statement of what the output should look like. Two folders
for one idea meant new references landed in whichever one the last person
happened to remember.

## What the mud plates imply for the architecture

Three separable things, and conflating any two of them produces one of the
failures above:

1. **Substrate weights** say what the ground is. Normalised, authored, smooth.
2. **A transition solver** turns that smooth ramp into the realised, ragged
   boundary. Evaluated analytically rather than read off the field grid, so the
   raggedness detail is not capped by the grid spacing — and consulted by *both*
   ownership and ground shading, or the tufts sit in the wrong place relative to
   the painted mud.
3. **Per-substrate displacement** at macro and meso scale, blended by weight, so
   bumpy mud can sit beside flat turf without either being a special case.

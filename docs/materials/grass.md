# Grass

The finished material, and the shape every other recipe is written against.

## What it is

A canopy of tapered ribbons, placed in tufts rather than independently. That
distinction is most of what the eye reads as vegetation: a tuft shares a lean, a
length and a brightness with its neighbours, and scattering the same number of
blades uniformly turns the field into a doormat.

## The rules that are expensive to break

**Place in world space, project at the very end.** A clump placed by screen
position slides when the camera moves. Everything in `terrain_generators::field`
is a pure function of a world coordinate, which is also what lets two pages that
have never met agree along a shared edge.

**Shade through a ramp, never by multiplying.** Multiplying albedo by a lambert
term gives grey-green shadows. The reference's darkest pixels are still
saturated green and its brightest are yellow-green paint; only a lookup
reproduces that. Light moves a surface *along* the ramp rather than scaling it.

**Composite by depth, not by draw order.** Alpha-over produces a collage of
decals. The depth test is what gives a page an inside.

**Detail belongs to the mound, not to the pixel.** Grass that is uniformly busy
everywhere reads as carpet however good the individual marks are. Bright crowns,
dark backs and dark interiors are organised by the mound field.

## Two width vocabularies, and why they differ

The rasteriser's widths are **stroke** widths — how much paint a mark lays down
— tuned so a 2D mark vocabulary fills the frame. Its broadest marks are six
centimetres of paint standing for a clump.

A botanically correct four-millimetre blade is under half a pixel at the
authoring scale and cannot be rasterised at all. That is why the Cycles export
carries a `blade_width` multiplier: both numbers are honest about different
things, and only the traced one is a plant.

## Blade width is a mip parameter

Not obvious, and the last thing to go wrong. A blade drawn at life size is a
fifth of a pixel at the game camera, and a fifth of a pixel does not minify into
a thin blade — it minifies into nothing, taking its highlight and silhouette
with it.

Measured at the overview: life-size blades gave a detail energy of 15 against
reference art's 22, and a highlight share of 0.4% against 3.3%. Drawing them
wider fixes that and ruins a close-up, so the width scales inversely with the
shown resolution, exactly like the rib count and the supersample.

## The parameters that decide the meadow

Fourteen, in `GrassStyle`. Population counts (`tufts`, `fine`, `thatch`,
`leaves`), morphology (`blade_length`, `blade_width`, `blade_bend`), and the
intrinsic colour family (`base_light`, `tip_light`, `glint`, `side_light`,
`under`, `scatter`).

The other twenty-three parameters of the old block decide only the *picture* and
live in `PreviewRasterStyle`. A scene survives any change to those.

**`blade_bend` reaches nothing.** It is read in one place — `Mark::shape`, which
builds a tiller's base stroke — and `Mark::shape` is never called. Set the range
from (0.35, 1.40) to (5.0, 9.0) and not one of nine thousand marks moves.
`blade_bend_reaches_nothing` asserts the gap rather than the fix, because wiring
it up changes the meadow and that is a look change rather than a repair.

## Status

The generator is the original code in `terrain_generators::placement`, and
`recipes::GrassRecipe` is the new `PopulationRecipe` interface. The meadow the
fingerprints pin comes from the former. Rewriting placement against the recipe
interface is its own measured change.

No wind, no trampling, no animated crown layer. The baked surface is static, and
the rear/front crown split — the thing that would let something stand *in* the
grass rather than on it — is not built.

# The Cycles backend

Rust decides where everything goes. Cycles decides what light does. That line is
the most important decision in the rendering half of the framework, and
everything here follows from it.

## Why the line is there

Two things depend on Blender never scattering anything itself.

**The cheap render and the expensive one must be the same ground.** They are
handed one `TerrainScene`. If Blender scattered its own grass, the path-traced
target would be a different meadow from the rasterised input, and a network
trained on that pair learns to hallucinate rather than to reconstruct.

**A corpus must be reproducible from a seed rather than from a backup.** Nothing
about the picture may live inside a `.blend` file, or regenerating last month's
shard means finding last month's file.

## A subprocess, not a linked library

Cycles' standalone interface is explicitly not a stable API, and linking its
internals means owning Embree, OpenImageIO, OpenColorIO, OIDN and a device
abstraction across four GPU backends. A process boundary costs a few seconds of
startup and buys immunity from all of it.

Startup is real. `render.py --manifest` takes a file of scene/output pairs so a
whole pre-bake pays it once.

## The package

```text
scene/
  manifest.json          what is here, how much, and in what layout
  ground/
    elevation.bin
    microrelief.bin
    material_weights.bin one plane per material
    modifiers.bin        one plane per channel
  geometry/
    ribbons-000.bin      one buffer per material
    ribbons-000-attributes.bin
  materials/bindings.json
```

A `.bin` is little-endian `f32` and nothing else — no header, no length, no type
tag. The manifest says how many elements and in what grouping, and the reader
checks the file's length before reading a byte.

That check is not defensive. A buffer one element short is not a crash: it is a
renderer reading the tail of one mark as the head of the next, for every mark
after the short one, producing geometry that is subtly and inexplicably wrong.

## Materials dispatch on appearance keys

A scene names its materials by key — `plant.grass_blade`,
`surface.dirt_compacted`, `rock.granite` — and `material_for` in `render.py`
turns a key into a shader graph.

That indirection is what lets the Rust side stay generic. Adding wildflowers is
a builder here and a binding there, not teaching the exporter that Blender has a
flower shader.

An appearance key is a **renderer-side implementation id**, not a material-weight
identity. A blade of grass growing on ground that is 70% grass and 30% dirt is
still made of grass, and the ground under it is separately 70/30. Conflating the
two is what produces transparent grass ghosts at a boundary.

An unknown key is a *reported* fallback. A scene naming `plant.wildflowe_head`
and getting grass would render perfectly, be wrong, and say nothing.

## Three things about the camera that each cost a wasted render

**The projection is orthogonal.** `screen.x = (u − v)` and
`screen.y = −(u + v)/2 + z` are perpendicular basis vectors, so this is a real
orthographic view down `(1,1,1)/√3` — 35.26° above the ground.

**It is anisotropic.** Those basis vectors have different lengths — `√2` and
`√3/2` — so the projection stretches horizontally by `2/√3 ≈ 1.1547`. That factor
is the entire difference between the 2:1 dimetric diamond this draws and true
isometric. **No camera transform can express it**, because a transform that
scales one screen axis is not a rotation. Blender carries it as a non-square
pixel: `pixel_aspect_y`.

**It is left-handed.** Take the basis at face value and the camera is *below the
ground looking up*. That is a property of the convention, not a sign slip: a
physical camera above `+u+v+z` sees `+u` go left, and this sends `+u` right.
Tile projections are picked to suit the tile grid.

A path tracer cannot be handed a mirrored camera, so the **world** is reflected
instead — a swap of the two ground axes. The rule this creates is worth stating
as loudly as possible:

> **Nothing may cross the boundary without the swap.**

A blade reflected while its sun is not would be lit from the wrong side, and it
would look entirely plausible. The swap happens in `terrain_cycles::export` and
nowhere else — positions, the ground origin, the bounds and the sun's bearing —
and a test pins it against a physical right-handed basis rather than trusting
the arithmetic.

## Tiling, and why a wide view is bought with time

A grass blade is about three millimetres across, so it is one pixel wide at
roughly 330 pixels per metre. Below that it is a *partially covered* pixel, and
a canopy of partially covered pixels averages to a flat wash — no highlights, no
silhouettes, no tufts, however many blades are in it and however many samples it
gets.

So three numbers scale together, and `terrain_cycles::plate` derives all three:

- **Trace resolution is fixed** at 330 px/m and the supersample is derived from
  it. A wide view is the same render over more ground, filtered down further,
  not a coarser one.
- **Blade width is a mip parameter.** At the game's framing a life-size blade is
  a fifth of a pixel and minifies into nothing, taking its highlight with it.
  Measured, life-size blades at the overview gave a detail energy of 15 against
  reference art's 22 and a highlight share of 0.4% against 3.3%.
- **Tiles come from the vertex budget**, computed rather than guessed. Guessing
  wrong is not a slow render — it is Blender taking a segmentation fault inside
  `Session::wait()` several minutes in, with a crash log instead of a picture.

Thinning instead of tiling was tried and is worse than it sounds. Holding
*coverage* is not holding *structure*: fewer, fatter blades cover the same
ground and stop forming legible tufts. Measured, coherence fell from 0.46 to
0.22.

Tiles are seamless for free, because placement is a pure function of world
position. Each is grown with a half-metre guard band so blades just outside it
still shadow inward, then the guard is cropped.

## Passes

`terrain_cycles::aov` names twelve and says which eight this build produces.
Naming them all now is the cheap half: a dataset manifest recording
`"direct_diffuse"` has to mean the same thing when the channel lands, and
renaming a channel a model has learned the statistics of is not a rename.

Asking for an unimplemented pass is visible. A dataset job that quietly produced
nine channels where ten were asked for is a corpus with a hole nothing reports.

Blender renames pass flags between releases, and assigning to a retired one
raises — which once turned a cosmetic API change into `--aovs` producing *no
image at all*. Passes are now set through a guard that skips and names what this
Blender does not have.

## Running it

```sh
./render                                         # nine tiles, 1920x1080, opened
terrain render --samples 512                     # sliced automatically
terrain render --seed 5a17e33b0c9d2f14           # that world again, exactly
terrain render --dry-run                         # the plan, no tracing
terrain render --manual --size 768 --px-per-metre 192   # a laboratory plate
```

`--trace-tiles-across` overrides the memory split above. It is **not** the world
tile layout — see [ISOMETRIC_TILES.md](ISOMETRIC_TILES.md) for the four things
this repository calls a tile.

With a tile layout the film is transparent, the ground mesh ends exactly at the
layout's outer boundary, and blades rooted outside it are a second object with
`visible_camera = False`: they shadow inward without appearing. Dropping them
instead leaves a bright rim at the edge of the picture. The slice filter weights
colour by coverage for the same reason — with a transparent film the background
samples come back black at zero alpha, and averaging them in unweighted puts a
black fringe on every silhouette in the frame.

`TERRAIN_BLENDER` overrides where Blender is found. Pinned to 5.2 LTS.

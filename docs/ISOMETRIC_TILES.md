# Isometric tiles

A render is nine square world tiles, three by three, with the middle one as the
subject and the eight around it as set dressing.

```text
      ┌───┬───┬───┐        one continuous scene over all nine, plus a halo
      │   │   │   │        ───────────────────────────────────────────────
      ├───┼───┼───┤        grass crosses the internal joins
      │   │ ■ │   │        shadows fall across them
      ├───┼───┼───┤        the colour field does not stop at a tile edge
      │   │   │   │
      └───┴───┴───┘        ■ = subject
```

Projected, that is one large diamond of ground:

```text
                    /\
                  /    \
                /  /\    \
              /  /    \    \
            <  <   ■    >   >        6S wide, 3S tall on screen
              \  \    /    /
                \  \/    /
                  \    /
                    \/
```

## The four words that mean four different things

Three of them were already taken, so the vocabulary is worth fixing before
anything else:

| Word | What it is | Where |
| --- | --- | --- |
| **world tile** | one semantic square of terrain | `terrain_scene::layout` |
| **plate** | one finished image | `terrain_cycles::plate` |
| **page** | a rectangular unit of runtime cache | `terrain_generators::page` |
| **trace tile** | a slice of a plate small enough for Blender to hold | `terrain_cycles::plate` |

A nine-tile layout is **not** nine pages, and it is **not** a trace-tile split.
`--trace-tiles-across 4` splits one plate into sixteen pieces for memory reasons
and changes nothing about the layout. It is named at that length precisely so it
cannot be confused with the thing that matters.

## Why coordinates and not a count

`tile_count: usize` would be the obvious model and it says nothing. Nine is three
by three; twenty-seven could be three by nine, three layers of nine, or a ring. A
layout is therefore an explicit list of `WorldTileCoord`, and a preset is a
function that produces one — so a new arrangement changes `layout.rs` and nothing
downstream. The resolver, the camera and both renderers read the coordinate list.

## A tile is two metres

Three things agree on it.

**Precedent.** Diablo II's floor tile is 160×80 pixels at 2:1 dimetric, cut into
5×5 collision subtiles. Work back from a character at roughly 80 pixels for 1.8
metres — about 44 pixels to the metre — and a tile is a shade under two metres,
with 0.4-metre pathing cells. Dota 2 is coarser: Source units are inches and its
terrain grid is 128 of them, so 3.25 metres, with 32-unit pathing cells and a
0.6-metre hero collision radius. Its heroes are heroically oversized and its
camera sits much further out.

**Gameplay.** A tower occupies a tile, a keep two by two, and a hero at a
0.6-metre collision radius moves across tiles rather than snapping between them.

**The renderer**, and this is the one that settles it. The subject diamond is
576×288 pixels at the default framing *whatever* the tile side, because the
layout always fills the same fraction of the frame. What the tile side changes is
how many metres those pixels cover:

| Tile side | Shown at | Traced at | Blade width in the trace |
| --- | --- | --- | --- |
| 2 m | 144 px/m | 432 px/m | over a pixel |
| 4 m | 72 px/m | 216 px/m | two thirds of a pixel |

A grass blade is about three millimetres across, so it is one pixel wide at
roughly 330 pixels to the metre and a *partially covered* pixel below that — and
a canopy of partially covered pixels averages to a flat wash with no highlights
and no tufts, however many samples it gets. The path tracer supersamples by at
most three (`terrain_cycles::plate::MAX_SUPERSAMPLE`), so two metres is the
largest tile at which the expensive renderer can actually see the grass it is
rendering. It is also a quarter of the geometry, because the frame covers a
quarter of the ground.

## The framing is fitted, not chosen

`terrain_scene::frame` resolves a layout, a projection and a frame into a scale,
a window, a raster origin and nine polygons. One function, called by both
renderers, because the reliable way to make a cheap plate and a traced plate
register is not to write the arithmetic down twice.

The scale comes from the layout's *projected* corners rather than from a `6S × 3S`
formula, so a layout that is not a filled square — or a projection that is not
2:1 — still frames correctly without the resolver knowing about either.

At the defaults:

```text
output            1920 × 1080
tile side         2 m
fill              0.90
visible ground    6 m × 6 m
projected         12 m × 6 m of screen
scale             144 px/m
outer diamond     1728 × 864 px
subject diamond   576 × 288 px at (960, 540)
margins           96 px across, 108 px down
```

The fill is ninety percent rather than one because grass rooted in the outer
tiles leans past the diamond, and a layout that filled the frame exactly would
cut those blades against the border — which reads as a crop rather than an edge.

## The silhouette

The picture is a diamond on nothing. Both renderers write RGBA.

The silhouette is a **union** of two things, and each half is there for a
different reason:

- The **visible ground** gives the diamond its shape. It is sampled four times on
  each axis, because the edges run at two to one and a hard test on a two-to-one
  diagonal is a staircase — on the one edge in the frame that has to read as a
  clean isometric silhouette.
- The **canopy over it** lets a blade rooted in an outer tile lean past that shape
  and stay in frame. Measured on the pinned fixture the silhouette comes out 13%
  larger than the bare ground — a rim of blades, not a halo.

Grass rooted *outside* the layout is a different case, and it is drawn rather
than dropped:

| | drawn | occludes | shadows | in the silhouette |
| --- | --- | --- | --- | --- |
| rooted inside | yes | yes | yes | yes |
| rooted outside (halo) | yes | yes | yes | **no** |

Dropping the halo is the tempting shortcut and it produces a bright rim exactly
at the edge of the picture, where the eye goes. In the rasteriser a halo mark
carries one bit saying it is not what the picture is *of*; in Cycles it is a
second object with `visible_camera = False`.

The bit follows the *winning* fragment rather than being sticky. A halo blade in
front of an inside one therefore leaves a blade-wide gap in the silhouette, which
against a transparent background reads as a gap between blades — which is what it
is. The alternative shows grass that is not in the picture, which is worse.

### Why the halo cannot simply be clipped

The obvious implementation — skip halo marks entirely — fails for a reason worth
recording. `bake_padded` grows the page by the shading reach, which at the
default framing is a few metres, and every neighbourhood-reading term inside the
diamond samples that far outward. A canopy that stopped at the layout's edge
would leave a several-metre band inside the picture shaded as though the world
ended, on a layout that is only six metres across.

## Random, and reproducible

An ordinary invocation of `./run` or `./render` picks a fresh world and derives a
centre tile from it, so every run is somewhere new. That is only useful if the
result can be got back, so every render writes a manifest beside the picture and
prints the command that reproduces it:

```sh
./run
# …
# replay:
#   terrain preview-export --layout nine --tile-size-m 2 --seed d56df3558cff96ca --centre-tile=-1829,1410
```

The centre tile is *derived* from the seed through two named streams rather than
drawn beside it, so one number reproduces the whole frame. The derivation is
checked for what actually goes wrong: consecutive seeds must not walk the grid,
and the two axes must not share a stream and put every centre tile on the
diagonal.

Fresh seeds come from the operating system, in the command line, and deliberately
not from the clock — two renders started in the same second would be the same
meadow. Nothing below the binary may consult the world for a number.

## Sidecars

The beauty render has no visible tile boundaries. That is the point, and it
leaves nothing in the picture to check the framing against, so three files land
beside it:

```text
plate.png                 the picture, RGBA
plate-tiles.png           every tile outlined and labelled, the subject heavier
plate-subject-mask.png    white inside the centre diamond, black outside
plate.ron                 seed, centre tile, bounds, scale, replay command
```

The subject mask is what a centre-only metric crops with and what a weighted
training loss multiplies by. Nothing darkens or blurs the context tiles in the
beauty render: a context tile that differed systematically from the subject would
put an artefact one tile from the middle of every frame, which is precisely what
a neural renderer would learn in preference to learning grass.

## Measuring it

`terrain_bench::iso` answers the two questions the other instruments cannot.

**Is the subject any good?** A nine-tile plate is eight ninths context, so every
number is taken twice — once over the layout, once weighted by the subject mask.
A change that improved only the middle tile moves a whole-frame metric by a ninth
of its real size, which is inside the noise.

**Are the joins invisible?** `join_visibility` compares the step across a tile
join against the step across an arbitrary parallel line a few pixels away in the
same picture. Relative on purpose: grass is high-frequency, so the absolute
difference is large whether or not there is a seam, and an absolute threshold
would pass everything or fail everything depending on how lush the meadow was.

One is a join indistinguishable from nothing. Two is a line. Currently:

| Scenario | Join visibility |
| --- | --- |
| `iso_nine.origin` | 0.97 |
| `iso_nine.far_negative` | 0.93 |
| `iso_nine.coarse_tiles` | 0.88 |

Scenarios name their seed *and* their centre tile rather than deriving one from
the other, so a change to the derivation cannot silently move every measurement
to different ground. Append only, like `terrain_bench::SCENARIOS`.

## What is deliberately not done

- **The world is flat.** All nine tile bases are coplanar; no steps, no cliffs,
  no raised platforms, no camera pitch. The grass mound field stays, because it
  is surface-scale variation rather than gameplay elevation. Elevation arrives
  once the layout is settled, so that a bad result has one possible cause rather
  than three. The layout is settled now; see [todo/elevation.md](todo/elevation.md)
  for what is idle while it stays flat.
- **Only two things are randomised**: the world seed and the centre tile. Sun,
  camera, output size, fill, tile size and grass style are all fixed, so each
  render is visually new and directly comparable to the last. Independent streams
  for lighting, season and wetness arrive later; randomising everything at once
  would give every failed image five possible causes.
- **No centre-tile curation.** Pure random sampling will occasionally put a quiet
  patch in the middle. Rejection sampling would bias a training corpus toward
  interesting terrain, so if it arrives it will be an opt-in flag and never the
  default for `terrain dataset`.
- **Twenty-seven tiles.** Not a number — a shape nobody has chosen yet. When one
  is chosen it is a variant of `TileLayoutPreset` and the rest of the pipeline
  does not change.
- **`terrain dataset` still frames by page.** It crops square patches at a chosen
  scale, which was right when a render was a rectangle. Once the neural
  renderer's unit is a tile, the corpus should be tile-shaped and should carry
  the subject mask beside each pair. That is a change to the input/target
  contract, so it waits on the contract rather than being done in passing. See
  [todo/dataset-tile-shape.md](todo/dataset-tile-shape.md).

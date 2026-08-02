# Procedural grass: the screen-space plan

The current renderer instances a baked clump sprite per plant. It works and it
looks like grass, but it is the wrong architecture for this game:

- **It builds.** 256 chunk meshes upload to the GPU one at a time, which is the
  visible "drawing in row by row" at startup.
- **It costs per plant.** Every clump is geometry, submitted, sorted and blended
  every frame, whether or not anything about it changed.
- **It cannot get denser.** Blended sprites pay for every overlapped fragment,
  so density is capped by fill rate rather than by how the field should look.

The replacement evaluates grass **per pixel, from world position**, with no
geometry, no meshes and no build step. One quad covers the field. Every pixel
asks which clumps cover it and looks them up in the baked atlas.

## Why this has to be screen space

The obvious version — a ground-plane texture where each pixel samples the clump
grid under it — cannot work. Grass *stands up*. A clump rooted at `R` paints
screen pixels **above** `project(R)`, so a pixel is coloured by plants rooted
some distance behind it, not by whatever is directly beneath it.

So the search runs the other way: given a screen pixel, find the roots that can
reach it.

## The search bounds

Working from `iso.rs`, a ground point `R` projects to

```text
screen.x = (R.x - R.y) * HALF_TILE_W
screen.y = -(R.x + R.y) * HALF_TILE_H
```

A clump at `R`, `h` metres tall and `w` wide, covers screen point `S` when

```text
|S.x - project(R).x| <= w / 2
0 <= S.y - project(R).y <= h
```

Substituting `project(R).y` and writing `u = R.x + R.y` for the depth axis:

```text
u ∈ [ -S.y / HALF_TILE_H , -S.y / HALF_TILE_H + h / HALF_TILE_H ]
```

and `-S.y / HALF_TILE_H` is exactly `P.x + P.y` for `P = unproject_ground(S)`.
So:

> **A pixel is covered only by clumps whose roots lie between `P` and `P + 2h`
> along the depth axis, and within `± w` across it.**

That is a short, bounded search — not a scan of the field. With clump cells of
`c` metres it is `ceil(2h / c)` cells deep by `ceil(2w / c)` across. At `h = 1.4`,
`w = 1.4`, `c = 0.7` that is 4 × 4 = 16 candidate cells, and the cost is
**constant** regardless of how dense the grass is.

## The loop

```text
P = unproject_ground(pixel)
colour = base wash at P

for each candidate cell, far to near along (R.x + R.y):
    R      = cell centre + hash jitter          # the clump's root
    if hash says this cell is empty: skip       # Perlin density
    lean   = bend field at R                    # the simulation, unchanged
    local  = pixel - project(R)                 # into sprite space
    local -= lean * (local.y / h)               # rooted shear: zero at the base
    uv     = atlas cell for this variant + local / sprite size
    sample = textureSample(atlas, uv)
    colour = mix(colour, sample.rgb, sample.a)  # painter's, far to near
```

Far-to-near ordering falls out of the loop order rather than needing a sort,
which is what removes the isometric lattice for free — there is no draw order to
get wrong.

## What this buys

| | instanced | procedural |
|---|---|---|
| Startup | 256 mesh builds and uploads | none |
| Geometry | 4 verts per clump | 4 verts total |
| Cost per frame | scales with clump count | fixed samples per pixel |
| Density limit | fill rate | none |
| Draw-order artefacts | needs sorting | impossible by construction |
| Memory | ~40 MB of vertex buffers | the atlas alone |

## What it gives up

- **Clumps cannot overlap more than the search depth.** A plant taller than
  `2h / HALF_TILE_H` behind the pixel is missed. That bounds how tall grass can
  be before the search has to widen.
- **Per-clump state has to be derivable from position.** Anything a clump needs
  must come out of a hash of its cell, because there is nowhere to store it.
  That is a real constraint on future features — a clump that has been *burnt*,
  say, needs the burning to live in a field rather than on the clump.
- **The atlas is now load-bearing for performance**, not just for looks: every
  pixel does several samples of it, so its size and cache behaviour matter in a
  way they did not when it was read once per sprite.

## Order of work

1. Move the atlas and the bend texture onto `GroundMaterial`.
2. Replace the ground fragment shader's body with the loop above.
3. Delete clump instancing from `scene.rs` — the ground quad is the whole field.
4. Re-tune density and clump size, which are now free of fill-rate pressure.
5. Keep `clump::bake` and its `Style` exactly as they are. The art pipeline does
   not change; only how the sprites reach the screen does.

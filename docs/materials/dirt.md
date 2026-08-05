# Dirt, mud, sand and soil

Ground is described by an **asset**, not by a shader. A material in a terrain
document says *which* soil this is; a `.ground.ron` profile says what that soil
is made of and how it responds.

## The three-way split

The rule that settles most arguments about this system:

> **Material** says what particles are present. **State** says their current
> condition. **Disturbance** says what happened to them.

| Question | Where the answer lives |
| --- | --- |
| Is this loam, clay, beach sand or volcanic dust? | a ground profile |
| Is it wet, compacted, dry or loose? | a modifier channel with a `ModifierRole` |
| Was it churned by wheels, or worn smooth by feet? | a disturbance channel |
| Does water collect here? | derived flow, concentrating the declared supply |

**Mud is not a material.** Mud is loam whose moisture is high; churned mud is
loam whose moisture is high and whose compaction has been broken up. A materials
list containing `dirt`, `mud`, `wet_mud` and `churned_mud` cannot express the
ground halfway between any two of them.

## The library

`assets/terrain/materials/*.ground.ron`, two of them:

| Profile | What it is |
| --- | --- |
| `meadow_floor` | dark organic ground under a canopy |
| `compacted_loam` | a worn track — the measured reference soil |

**Two, deliberately.** Five more were written — loose farm soil, clay, beach
sand, desert sand, hardpan — and removed. The schema describes them fine; the
meadow is what this project is for, and a library of grounds that nothing
renders is a library nobody is checking. Adding one back is a file, and the
ripple and crack machinery they exercised is still here and still tested.

### Where `compacted_loam`'s colours come from

Sampled off `docs/references/grass_to_mud_bumpy.jpg` over a 700 px square, in
linear light:

```text
25%           0.061, 0.031, 0.013
median        0.084, 0.053, 0.030
brightest 5%  0.195, 0.127, 0.072
```

The ratios matter more than the levels and they hold across the whole range:
**G/R about 0.63, B/R about 0.36.** An earlier ramp sat at G/R of nearly one,
which is why a whole track of it read as sand — the error was the green, never
the brightness.

`meadow_floor` is the *same physical soil* seen under a canopy, and it is four
times darker. Earth glimpsed between blades has to stay dark or it stops reading
as ground seen through a canopy and starts reading as a rock lying on one.

## Relief is a list of bands, and the renderer decides where each is drawn

A profile declares its relief as bands — a wavelength, an amplitude, a shape —
and says nothing about whether a band should be mesh or bump. It cannot: that
depends on the sampling rate, which the profile does not know.

```text
wavelength >= 4 x lattice spacing    displaced geometry — casts its own shadow
wavelength >= 2 x traced pixel       shader bump — tilts the normal
below that                           roughness — a microsurface, not a shape
```

Each band is drawn exactly once, by whichever tier can actually draw it. The
lattice itself is chosen from the coarsest band of the soils in play, so a
document of hardpan and beach sand gets a finer mesh than one of turned farm
soil. `terrain compile` prints the split.

Two things this ladder exists to prevent, both of which happened:

- **A band the mesh cannot resolve, meshed anyway.** A five-centimetre band
  built from two noise octaves contains content at two and a half, which the
  lattice sized for the band aliases. The plate came back as black speckle. A
  band is one scale now; the multi-scale structure is the *list*.
- **A band finer than a pixel, bumped anyway.** A one-millimetre grain sampled
  at three millimetres per pixel is not fine texture, it is boiling noise.
  Sub-pixel relief is what roughness means.

## Wet ground is not dry ground turned down

Water fills the pores, so the air-soil boundary that scattered light diffusely
becomes a water-soil boundary. Three things follow, and doing only the first is
what makes wet ground read as ground in shadow:

```text
albedo      darkens toward its own square
hue         warms — the film absorbs blue and green harder than red
roughness   collapses, and does so before the darkening is noticeable
```

The author writes two colours — the dry mid stop and what it becomes when
soaked — and the square law is fitted between them. A per-channel gain would be
unusable: the number that keeps a dark meadow floor in range is four times the
one that keeps a mid loam in range, so it means nothing on its own.

**No subsurface scattering.** Production mud shaders do not use it; mud is a
rough dark dielectric under a glossy coat, and scattering inside a medium that
absorbs this hard buys nothing for the cost.

## Cracks are a capability times an occasion

The profile says whether a soil *can* crack — a question about clay content and
cohesion — and the state says whether it has. The terms multiply, so any one of
them closes the network completely:

```text
dryness x desiccation x (1 - disturbance) x cohesion
```

Declaring cracks on a material with cohesion below 0.25 is refused at load: loose
material slumps instead of cracking.

## Ripples are what make sand sand

Isotropic noise at ripple amplitude reads as a rough plane. The coherence over
distance is the entire signal — one crest implies the next, in a direction — and
that is the one thing a noise texture cannot supply. Hence a declared direction,
wavelength, meander and windward/lee asymmetry.

Saturation suppresses them, so a wet line on a beach is visible as a change in
*texture* as much as in colour.

## What a finished ground still needs

- **Ruts, footprints and hoof marks.** The full profile — a depression *and* a
  displaced rim — not a subtracted trench. `FeatureContext` reserves the tangent,
  normal and progress data and the sampler does not yet fill it in.
- **Puddles as separate water geometry.** Wet film exists; standing water needs
  its own approximately horizontal surface at IOR 1.333, with wet soil visible
  beneath it. A puddle is not glossy displaced dirt.
- **Pebbles and grit as real instanced geometry** rather than analytic discs,
  with burial depth and curvature sorting.
- **Sorted fines**: fines in the hollows, coarse on the crown. The curvature and
  flow fields to do it with already exist and are read for moisture only.
- **The macro shape still comes from the grass generator's mound field**, not
  from the document's elevation layers. It is why a soil plate rendered at close
  framing has a swell across it that nothing authored.

## What it must not become

A second full renderer. Dirt is a substrate — a continuous field with a sparse
population on top — and the temptation is to give it the mark vocabulary grass
has. It does not need one, and building one would mean the transition between
them had two mark systems to reconcile rather than one shared candidate field.
See [../MATERIAL_BLENDING.md](../MATERIAL_BLENDING.md).

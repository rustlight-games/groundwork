# Covers and snow

**A type with no producer.** `terrain_scene::field::CoverPlane` exists, carries
depth, coverage, compaction and wetness, is digested with the rest of the stack
and is checked by the stack's own validity test. `derive::sample_fields` sets
`covers: Vec::new()` and nothing else ever pushes one.

## Why it is shaped the way it is already

The modelling decision is made and it is the part that would have been expensive
to retrofit: **a cover is not a substrate**. Substrate weights normalise to one
because a square metre of ground is entirely something; a cover lies *over* that
ground and leaves it semantically present underneath. Snow as a third normalised
weight would erase the dirt it is lying on, and thin snow would become a muddy
three-way blend instead of a dusting with earth showing through it.

Depth *and* coverage, for the same reason. Depth alone cannot say whether two
centimetres is a sheet or a scattering of patches caught between blades, and
coverage alone cannot say how far the surface stands proud of the ground.
Dusting is the interesting half of the range and it needs both.

## What is missing

**The document cannot declare one.** There is no `covers:` list beside
`materials:`, no `Cover` layer operation, and no `cover_solvers:` section. The
authoring shape the spec sketched is still a sketch — see
[authoring-model.md](authoring-model.md).

**There is no solver.** The three stages are all absent:

- *Deposition* — how much incoming snow reaches each lattice point, as a product
  of sky visibility, slope retention, facing, shelter, surface stickiness and
  procedural variation. Every input except stickiness is a derived field that
  already exists and is already carried, which is most of why this is worth
  doing next rather than later.
- *Stability* — the slump. Where the surface angle exceeds the angle of repose,
  move a bounded amount of snow downhill, iterated to a residual. It must be a
  Jacobi or red-black update rather than an in-place scan, or the answer depends
  on which cell was visited first and two neighbouring plates disagree along
  their join. It must conserve mass except where outflow past the generated
  bounds is explicitly allowed, and the test for that is arithmetic rather than
  visual: deposited mass in, settled mass out.
- *Surface reconstruction* — an offset surface `ground + depth`, edge-aware
  smoothed so small features round over and tiny gaps bridge without smearing
  across a hard exclusion boundary. One mesh, not one object per lattice cell.

**Nothing renders one.** No cover pass in `terrain_bake::generic`, no
`cover.snow_fresh` appearance in `render.py`, no cover binding in the scene
package.

**Nothing interacts with one.** The rule that makes it worth having — snow
changes the *visibility and deformation* of existing vegetation and never
regenerates a different meadow — has nothing enforcing it, because there is no
snow for a blade to be buried in.

## What exists to build on

- The cover group in the field stack, digested and validated.
- `slope`, `aspect`, `curvature`, `exposure`, `flow_accumulation` and
  `flow_direction` in `DerivedFieldSet`, all computed over the combined
  structural surface and all carried on the same grid a solver would run on.
- The generated-bounds halo, derived in `terrain_generators::compiler` as a
  maximum over every declared reach. A solver stencil is one more term in that
  maximum, and the mechanism for adding it is already there.
- Edge-anchored, globally-snapped grids, so two regions solved in different
  processes share their boundary samples exactly. This is the property the seam
  test for snow depth would rest on and it is already true.

## Done looks like

- A continuous snowfall control moves a plate from bare ground through dusting
  to blanket, with the substrate and the grass still readable at the shallow
  end.
- Snow collects in hollows and thins on crests without anybody painting it
  there.
- Total settled mass equals deposited mass to numerical tolerance on a closed
  boundary, and integrated mass rises monotonically with snowfall input.
- Depth agrees exactly along the join between two independently compiled
  regions.
- Candidate identities do not move when the snow does.

## The trap

Solving covers **after** discrete vegetation is resolved. The order in
`terrain_generators::compiler` is deliberate — a cover is a continuous field and
belongs with the other continuous fields, before any candidate is generated,
because grass density responds to snow load and snow deposition responds to
canopy shelter, and only one of those two can be second. Making vegetation
second is the cheaper coupling: a blade can be clipped by a depth that is
already known, where a depth cannot be deposited onto a canopy that does not
exist yet.

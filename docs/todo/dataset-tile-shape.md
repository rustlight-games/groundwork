# The corpus is still page-shaped

`terrain dataset` crops square patches at a chosen scale from a larger bake and
trims the margin off both halves. That was right when a render was a rectangle.
A render is now nine world tiles framed as a diamond with a subject in the
middle, and the corpus does not know it.

## What is wrong with a page-shaped shard

**The unit does not match the product.** The neural renderer's job is to produce
one subject tile from its context. A square crop at an arbitrary scale gives it
neither — no subject mask, no statement of which part of the frame is the answer
and which part is the question.

**The framing metadata is a page, not a layout.** A shard records bounds, pixel
scale and a camera digest, which is enough to reproduce the *bake* and not
enough to say where the tile joins fell or which tile was the subject.

**The subject mask is written beside every render and not beside any shard.**
`plate-subject-mask.png` exists on the render side. It is what a centre-only
metric crops with and what a weighted training loss multiplies by, and the
corpus is the one consumer that needs it most.

## What is right and should not be disturbed

The parts that were expensive to get correct are correct, and this is a
reshaping rather than a rebuild:

- **One scene, rendered twice.** `RenderPair` holds an `Arc<TerrainScene>` and
  takes two closures. There is no constructor that accepts two scenes.
- **Crops come from the middle of a larger bake**, so no neighbourhood-reading
  term is evaluated near an edge. The tile layout's halo is the same idea with a
  better-defined size.
- **Every shard is its own world**, so a validation split is genuinely held out.
- **The manifest pins everything** — document digest, scene fingerprint, root
  seed, bounds, camera digest, pixel scale, both renderer versions, the Blender
  version, every recipe version, the crop margin, the pass list, a checksum per
  file and the mark count.

## Done looks like

- A shard is one subject tile plus its eight context tiles, framed by
  `terrain_scene::frame` — the same resolver both renderers already use, so the
  input and the target register without the arithmetic being written twice.
- The subject mask ships beside each pair.
- The manifest records the layout, the tile side, the centre tile and the halo,
  not just a rectangle.
- The model can be handed full context and asked for the subject only.

## What it is waiting on

[render-paths.md](render-paths.md), and not for a scheduling reason. The corpus
pairs a cheap input with an expensive target; while those two come from
different generators, making the shard tile-shaped improves the framing of a
pair that is already wrong in a way no framing fixes.

The neural input contract is the other half of the same question — whether the
cheap input is an RGB render at all, or the field stack plus structural AOVs.
See [LOW_TO_HIGH_FIDELITY_SPEC.md](../LOW_TO_HIGH_FIDELITY_SPEC.md) §
"What the network is handed", which describes what is exported today and marks
what is not.

## The trap

Rejection-sampling interesting centre tiles. Pure random sampling will sometimes
put a quiet patch in the middle, and curating that away biases the corpus toward
terrain the network then over-predicts. If it ever arrives it is an opt-in flag
and never the default.

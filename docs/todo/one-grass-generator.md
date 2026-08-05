# One grass generator

**There are two, and only one of them looks right.**

| | Where | Drawn by | Look |
| --- | --- | --- | --- |
| The tuned generator | `terrain_generators::{field, placement, scene, stroke, style}` | `terrain compile`, `./render`, `./run` | The quality bar |
| The content families | `terrain_generators::families` | nothing, today | Nowhere near it |

`terrain compile` path-traces the authored document by running the **tuned**
generator under a `SemanticOverlay` — the document modulates `Ground::density`
and `Ground::bare` and every style field stays exactly as tuned. Meanwhile
`terrain_generators::compiler` compiles the same document into a `TerrainScene`
through `families`, which emits tufts, undergrowth, thatch, flowers, stones and
clods as generic marks. Both are on the shipping path. They do not draw the same
meadow.

## Why it happened, and why it is not simply a mistake

The compiler needed something to emit, and the tuned generator does not emit
marks — it grows a `GrassScene` of its own with its own colonies, flow fields,
tillers and statement fields, and its output is strokes for a painterly
rasteriser rather than candidates carrying identity. Writing `families` was how
the shared-candidate mechanism got proved: acceptance before ownership, one mark
per accepted candidate, stable ids from candidate identity. That mechanism is
correct and it is load-bearing.

What `families` is not is a grass generator anybody would ship. It is a generic
tuft recipe, which is the exact mistake CLAUDE.md warns about — and the warning
exists because it was made once already.

The `SemanticOverlay` is the bridge that made a document reach the good grass
without replacing it, and it works. It is also narrow by design: it can say how
much grows and whether earth shows, and it cannot say *what* grows, because the
tuned generator has no notion of a profile.

## What finishing this means

Port the real placement into recipes rather than porting the recipes' look up to
match. Specifically:

- **Colonies, flow and tillers become candidate structure.** A tuft anchor is
  already the right unit — `vegetation.tuft_anchor` exists and children hang off
  a parent candidate. What is missing is that the parent's latent attributes
  carry what `placement.rs` decides: colony membership, comb direction, the
  statement-field weight that lets a passage collapse into paint.
- **Morphology moves into profiles.** Blades per tuft, length, width, bend, fork
  probability, dryness response. See
  [authoring-model.md](authoring-model.md) — there is nowhere to put them today.
- **Comparison is structural, not identical.** The two paths will never produce
  the same ids and should not be asked to. `terrain_bench::critique` and the
  detail-energy and coherence metrics are the gates: mark counts, canopy
  structure, highlight share, palette.

## Grass detail fields, once the above is true

The control vocabulary the spec asked for is mostly declarable today —
`meadow_path` already declares `tuft_density`, `fine_density`, `thatch_density`,
`flower_abundance`, `stone_abundance` and `grit_abundance` as modifier channels
— and mostly unread, because the tuned generator reads its own tuning instead.
The ones with no channel at all are the morphology side: height scale, width
scale, bend scale, dryness, maturity, and the shared orientation tendency.

The test that it worked: one plate contains visibly different grass passages
driven by fields, without a second rendered layer and without a second material.

## Done looks like

- One generator. `families` either grew into the tuned one or was deleted.
- `terrain compile` no longer needs a `SemanticOverlay` to reach good grass,
  because the compiler's own output *is* the good grass.
- A document can say the grass is short and dry here and lush there.
- The quality bands in `terrain_bench::critique` pass on the compiled path at
  every camera height, and the before/after is in the commit that did it.

## The trap

Deleting the tuned generator first, on the grounds that the architecture is
better on the other side. It is, and the picture is worse, and a day was already
spent learning that. The tuned generator is the specification for what the
output should look like; it goes last.

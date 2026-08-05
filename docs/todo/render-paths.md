# Render paths

Three routes to a picture exist. One is the production path, one is written and
unused, and one is being deleted.

| Route | Entry | Consumes | Status |
| --- | --- | --- | --- |
| Cycles through `GrassScene` | `terrain compile`, `terrain render`, `./render` | the tuned generator's own scene | the production path |
| The generic package | `terrain_cycles::write_package` | a `TerrainScene` | written, never on the active path |
| The cheap rasteriser | `terrain preview-export`, `./run`, `terrain dataset` | `GrassScene` / `WorldField` | being deleted, [issue #1] |

[issue #1]: https://github.com/rustlight-games/groundwork/issues/1

## Cycles does not read the generic scene

`terrain_cycles::export::write_package` writes the whole thing — a manifest, the
ground planes, per-material geometry buffers and material bindings — and it is
exercised only by its own tests. `plate::trace` builds its Blender scene from a
`GrassScene` instead.

The consequence is narrow but real: a `TerrainScene` compiled from a document
cannot be path-traced as itself. `terrain compile` gets a traced picture of the
authored document by running the tuned generator with a `SemanticOverlay` over
the compiled field stack, which is the right *look* and is not the compiled
scene.

Nothing about the package is wrong. It is blocked behind
[one-grass-generator.md](one-grass-generator.md), because switching Cycles onto
the generic route today would trade the tuned meadow for the families' one.

There is also a version to keep honest when it happens: `PACKAGE_VERSION` is 2
and the Blender side reads it. Moving the active route across is the kind of
change that must bump it in the same commit.

## The cheap tier is two renderers deep

`terrain_bake::generic::render_scene` already takes a `&TerrainScene` and its
field stack, never constructs a `WorldField` or a `GrassScene`, and shares the
transition solver with the compiler so its ground and its marks agree about
where the mud is. That is the contract the spec asked for and it is met.

It is also not what anything calls. `preview-export` and the dataset both drive
`GrassScene::build` and the painterly path in `bake.rs`.

The complication is that **CLAUDE.md says the rasterisers are going away
entirely**. Cycles is the only renderer; the cheap tier exists for the neural
corpus and for nothing else. So this is not "finish the generic rasteriser" —
it is a decision about what survives issue #1:

- If the corpus keeps a cheap input tier, `generic::render_scene` is what it
  should be, and `bake.rs` goes.
- If the corpus's input becomes the field stack itself plus structural AOVs —
  which is what [DATASETS.md](../DATASETS.md) and the neural input contract
  actually want — then both rasterisers go and `generic::render_scene` survives
  only as a debug view.

That decision is not made. Making it is the work; the code either way is
smaller than the argument.

## Done looks like

- One Cycles route, and it reads the generic package.
- `PACKAGE_VERSION` bumped in the commit that moves it.
- Nothing in `crates/` builds a `GrassScene` except the tuned generator itself,
  and nothing in `tools/` builds one at all.
- The rasteriser question is answered in writing before any code moves.

## The trap

Adding a flag that chooses between the two Cycles routes. `terrain compile` is
already named apart from `preview-export` for exactly this reason — a flag that
silently picks a pipeline is how a render comes out of a path nobody meant, and
the failure is a picture that looks fine and came from the wrong generator.

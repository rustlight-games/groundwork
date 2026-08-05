# The meadow tier, as it actually stands

What was built against `docs/spec/meadow-tier-implementation.md`, what holds,
and what does not. Written to be checkable rather than reassuring: every claim
below names the test that proves it, and every gap is a gap somebody wrote down
rather than one a reader has to discover.

## What changed

The compiled `TerrainScene` reaches Cycles. Before this it was computed,
fingerprinted, counted and discarded on every render — every accepted flower and
stone in the framework's history had been thrown away.

```text
document → PreparedTerrain → TerrainFieldStack → GroundEvaluator
              │                                       │
              ├── tuned controls  ─────────────┐      │
              ├── secondary scene ──┐          │      │
              └── interaction field ┤          │      │
                                    ▼          ▼      ▼
                          bridge ──► CyclesScene v2 ──► Blender
```

## Architecture

- [x] The tuned grass generator is untouched. `refactor_fingerprints` passes
      unchanged across every commit of this work.
- [x] Generic grass, fine grass and thatch never enter secondary rendering.
      Every recipe declares a `RecipeRenderClass` with no default, so a new
      family without one is a compile error rather than a second canopy.
      `meadow_baseline.rs::no_tuned_population_reaches_the_secondary_scene`.
- [x] One compilation owns the fields, the ground evaluator, the secondary
      scene, the tuned controls and the interactions.
      `final_surface.rs::the_compilation_hands_back_the_evaluator_it_used`
      asserts identity, not equality.
- [x] Secondary content compiles once per logical plate and is *selected* per
      trace slice, never regenerated.
- [x] Blender performs no placement randomness. Every position, rotation,
      prototype choice, burial and tint is decided in Rust.
- [x] The Cycles package is versioned. A reader that finds a number it does not
      know refuses rather than rendering a package with a section missing.

## Sampling

- [x] Candidate footprint radii are addressed, not drawn.
      `domain.rs::a_candidates_radius_is_the_same_whichever_window_computed_it`.
- [x] The conflict rule is symmetric: `‖xᵢ − xⱼ‖ < rᵢ + rⱼ + c`.
- [x] The priority key is a strict total order — priority bits, then the whole
      candidate address. Rank alone is not enough because different cells share
      ranks. `the_priority_key_is_a_strict_total_order`.
- [x] Bucketed thinning matches brute force.
      `the_bucketed_search_agrees_with_brute_force`.
- [x] Whole and partitioned regions agree.
      `footprint_thinning_agrees_across_a_join`.
- [x] Ownership indices rank by population key, so reordering declarations in a
      document does not reassign candidates.
      `compiler.rs::owner_ranks_come_from_the_keys_and_not_from_the_order`.

## Ground and interaction

- [x] Secondary roots sit on the *mesh* the renderer draws, not on the analytic
      surface. Between two vertices the mesh is the chord and the surface is the
      curve, and over a crest the gap is more than a stem is thick.
      `final_surface.rs::every_secondary_root_sits_on_the_mesh_the_renderer_draws`,
      with `the_mesh_surface_differs_from_the_analytic_one_by_enough_to_matter`
      guarding it against being a tautology.
- [x] No single-mark plant roots inside a stone.
      `stone_locality.rs::no_single_mark_plant_roots_inside_a_stone`.
- [x] Outside the interaction reach the two meadows are **bit-identical**.
      `outside_every_reach_the_two_meadows_are_bit_identical`.
- [x] Grass near a stone leans away and is shorter than *its own self* in the
      meadow without stones.
- [x] Semantic bareness responds to authored abundance. A sparse stand is short,
      dry and pale at the root rather than a lush one with holes in it.
- [x] Each tuned pass has an independent control, and asking for exactly what
      the style already does moves nothing.
- [x] Every relief contribution has exactly one recorded tier, and a budget that
      cannot carry a bump band moves it down and says so.
- [x] `wet_film` drives a distinct Principled coat with the profile's film IOR.
- [x] Ground fields agree across windows, checked with **independently
      constructed** evaluators — the first version built one and sampled it
      twice, which proves nothing.

## Content

- [x] Flowers have coherent stems, disks and petal whorls, with the colour
      authored by the document.
- [x] Flower groups cannot split across trace selection: the selector classifies
      placements, never primitives.
- [x] Stones use four reusable superellipsoid prototypes and explicit instances.
- [x] Stones are visibly buried, by a fraction of their own height, addressed
      per stone so no common horizon line appears.
- [x] Undergrowth is low, broad-leaved and patchy.
- [x] Dirt clods remain `Deferred`: the ground profile's aggregate band already
      carries clod-scale structure and drawing both would count one physical
      signal twice. Reported on every compile, never silently dropped.

## The ground benchmark

Seven pinned laboratories, six kinds of measurement, no Blender, about a second.
`terrain benchmark ground`.

- [x] Topography: detrended Sa/Sq/skew/kurtosis, RMS slope, and scale-dependent
      height difference, slope and curvature in four directions.
- [x] Spectrum: a radix-2 FFT with Parseval as a self-test, holding to 5×10⁻¹⁵.
- [x] Semivariogram, because a nugget in a deterministic field is aliasing or a
      bad tier handoff rather than microscale variation.
- [x] Optics swept off the profile, with a hue-ratio span that makes a grey
      dimmer masquerading as wet soil visible.
- [x] Composability over raw samples: two windows can be equally rough and still
      disagree at every point.
- [x] Gates are pass / fail / needs-review / not-applicable, never a score — a
      band that lost half its energy must not be paid for by a better colour
      match.
- [x] Every laboratory passes. Compaction cuts Sq by 71%; saturation leaves 55%
      of the relief, which is exactly the profile's declared 0.45 flattening.

### The two soils are two soils

Settled by measurement, not preference. Their green-to-red ratios differ by 0.22
— and occlusion multiplies every channel by one number, so it moves luminance
and leaves the ratios untouched. A ratio difference can only be composition. The
track's coarsest aggregate band is also larger, which no amount of state
produces from another band list. `soil_decision.rs`.

## What is not done

Stated because an unenumerated gap is worse than a known one.

- **The Rust-authored bump plane is not exported.** Blender's shader bands now
  match Rust term for term in morphology and state response — one octave, the
  monotone skew, compaction on every tier — but the *phase* still differs
  because Blender Noise and Rust value noise are different functions. Closing it
  needs the bump field uploaded as a float image. The relief plan records which
  bands are affected.
- **`terrain_dataset` has no corpus builder.** It kept `RenderPair`,
  `ShardManifest` and `ShardLayout`; the job that filled a shard went with the
  rasteriser. This is the largest single piece between here and training, and it
  is a rewrite against the matrix rather than a repair.
- **The performance recorder does not exist.** `run`, `performance` and
  `artifacts` in the report schema are unfilled, and `ground_benchmark.rs`
  enumerates exactly which keys and why — a downstream tool reading a missing
  key gets null and reports zero, which is indistinguishable from a measurement
  of zero.
- **No render-half benchmark.** FLIP, AOV comparison and the resolution ladder
  need Blender and belong on the visual gate.
- **A few flowers still stand on ground that reads as bare.** Not a broken
  control: `plants_on_dirt.rs` reports the vegetation channel at every plant root
  by quintile and no plant sits below a quarter. Grass is what covers ground, so
  thinning it to a quarter makes the ground look bare while leaving two flowers
  a square metre legible. It is a document question, and that test is the
  instrument for settling it rather than the eye.
- **`constant_grass` and `blend_lab` cannot compile.** They name recipes from
  the older population registry. Recorded in `documents::NOT_COMPILABLE` with a
  test that fails if one of them starts working without anyone saying so.
- **The world is still flat**, transcendental determinism is still
  same-platform, and `write_package` is still off the production path.

## Before the neural renderer

The corpus contract must not be frozen until the content vocabulary settles,
because every content type added after a corpus is generated invalidates it: a
model trained on a flowerless meadow cannot learn flowers, and the matrix it was
conditioned on has no plane for them. The vocabulary is now flowers, stones,
undergrowth, four tuned grass passes and a soil with a recorded relief ladder.
The next step is the conditioning contract, not more content.

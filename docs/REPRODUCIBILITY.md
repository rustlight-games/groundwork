# Reproducibility

Four different contracts, and treating them as one is how a suite ends up
either too strict to pass or too loose to catch anything.

| Kind | What it means | Where |
| --- | --- | --- |
| **Exact** | Bit-identical, always | document digests, seed derivation, candidate identities, painter order, scene fingerprints |
| **Quantised** | Identical after rounding | ground heights, encoded weights, packed normals |
| **Perceptual** | Within a calibrated band | Cycles beauty, GPU preview, denoised output |
| **Human** | Somebody looks | the laboratory plate, a look change |

## Exact

These come from pure functions of world position and integer identity. Anything
but equality means the function is not pure, which is a bug rather than a
tolerance question.

- Canonical document serialisation and its digest.
- Source and layer digests.
- Page and world-cell addresses.
- Seed-domain derivation and candidate IDs.
- Candidate acceptance and material ownership.
- Stable mark IDs and mark root ownership.
- Canonical painter order.
- Field-stack fingerprints, and scene fingerprints.

The field stack earns a place on that list because of how its grid is built: the
origin is snapped down to a multiple of the spacing, so every grid at a given
spacing is a window onto one world lattice. Two regions compiled in different
processes therefore agree *exactly* wherever they overlap, rather than
interleaving their samples and interpolating a surface that is nobody's.

`terrain_bench::seams::tolerance::MATERIAL_WEIGHT` is zero for this reason.

**Exact means same-platform.** `atan2`, `powf` and the noise are not guaranteed
bit-identical across architectures, so the contract is that any two runs on one
machine agree, not that a Mac and a Linux box do. That is enough for what these
guarantees are for — two pages agreeing along an edge, and a training crop being
the same ground as the render it came from — and claiming more would be a
promise the standard library does not make.

## Quantised

Values that fall out of a long chain of transcendental functions, where the last
bit is arithmetic noise rather than a decision anybody made. They are compared
after rounding, and the rounding is chosen so a real change cannot hide under
it:

- **Ground heights**: a tenth of a millimetre, against relief that runs to a
  quarter of a metre — four parts in ten thousand.
- **Material weights on disk**: one part in ten thousand.
- **Encoded normals**: one destination code value.

The categorical result must not change near a threshold. A weight that rounds
across `WEIGHT_EPSILON` is a material appearing or disappearing, which is an
exact difference wearing a quantised one's clothes.

## Perceptual

Cycles is a sampler. Its output legitimately depends on sample count, denoiser
version, device and driver, and demanding equality there would fail on a change
that improved the picture. These are judged in calibrated bands:

- Multi-scale structural similarity against an accepted plate.
- Palette drift, as a mean colour difference.
- Detail energy and highlight share.
- Repetition, as an autocorrelation peak.

Cross-device exact comparison of a path-traced image is not a contract anybody
can keep.

## Human

A deliberate look change moves every pixel, and the snapshot's answer stops
meaning anything the moment it does. What gates a look change instead is the
structural invariants — seams, reach bounds, world-coordinate purity, stable
streams — plus somebody looking at the laboratory plate.

## The seam test

The single most valuable reproducibility check, because it catches the whole
class at once.

Bake a region as one plate. Bake it as four and stitch them. Subtract. Anything
but zero is a term that depended on where the rectangle's edges happened to be.

The measurement is deliberately unforgiving: a seam of one code value is visible
on flat ground, because the eye finds a straight edge in noise far more readily
than it finds a shape, and a one-value step running for two thousand texels in a
straight line is exactly what it is best at.

## What may not reach a digest

Pointer addresses, allocation capacities, `Debug` strings, wall-clock time, and
the order a hash map happened to iterate in. A digest that moves for a reason
nobody can explain is a digest whose failures get accepted without looking —
which manufactures the habit of re-accepting baselines unread, and that habit
costs more than the digest was worth.

Threading is deliberately *not* in a bake's digest: pages are independent by
construction, so two plates differing only in how they were computed must share
a key or every cache misses for nothing.

## Version constants

Changing one of these is a decision, and the commit message is where its reason
lives.

| Constant | Where | Moving it means |
| --- | --- | --- |
| `SEED_ALGORITHM_VERSION` | `terrain_core::seed` | every plant in every world relocates |
| `DIGEST_ALGORITHM_VERSION` | `terrain_core::digest` | cached comparisons are invalidated, nothing moves |
| `CURRENT_FORMAT_VERSION` | `terrain_format::envelope` | a migration step must exist for the previous one |
| `BUILTIN_SOURCE_VERSION` | `terrain_core::registry` | a built-in source answers differently |
| `FIELD_STACK_VERSION` | `terrain_scene::field` | the matrix's own layout changed |
| `DOMAIN_ALGORITHM_VERSION` | `terrain_generators::domain` | every candidate in every domain moves |
| `TRANSITION_VERSION` | `terrain_generators::transition` | every realised boundary moves |
| `COMPILER_VERSION` | `terrain_generators::compiler` | how a candidate becomes a mark changed |
| `GENERATOR_VERSION` | `terrain_bench::fingerprint` | the meadow is meant to be different |
| `GENERIC_RASTER_VERSION` | `terrain_bake::generic` | the cheap picture changed |
| `PACKAGE_VERSION` | `terrain_cycles::package` | the Blender side must be updated with it |

The separation between the first two is the point of having both. Improving the
content digest is maintenance; improving the seed hash is a world rebuild.

The same argument splits the middle of the table. `DOMAIN_ALGORITHM_VERSION`
moves every candidate in every domain; a recipe's own version moves only what
that recipe drew. Conflating them would make a grass tweak invalidate the
stones.

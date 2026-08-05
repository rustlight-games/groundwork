//! The field stack against the real authored documents.
//!
//! The unit tests in `field` and `derive` prove the arithmetic against stated
//! surfaces. These prove the thing the arithmetic exists for: that a document an
//! author actually wrote compiles into an honest matrix, and that two regions of
//! it agree where they meet.
//!
//! A seam test that only ever sees a synthetic ramp is a seam test that passes
//! for the wrong reason. `blend_lab` has a spline running through it, which is
//! the only feature in the repository whose value at a point depends on a
//! spatial index — and an index consulted at two different rectangles is exactly
//! where a disagreement would come from.

use std::path::{Path, PathBuf};

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_scene::derive::{DerivedFieldRequest, derive_fields, sample_fields};
use terrain_scene::field::FieldGridSpec;

struct BesideDocument {
    root: PathBuf,
}

impl terrain_core::AssetResolver for BesideDocument {
    fn read(&self, path: &str) -> Result<Vec<u8>, terrain_core::AssetError> {
        std::fs::read(self.root.join(path)).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => terrain_core::AssetError::NotFound,
            _ => terrain_core::AssetError::Unreadable(error.to_string()),
        })
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }
}

fn assets_root() -> PathBuf {
    // From `crates/terrain_scene/` up to the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("assets/terrain")
}

fn prepared(document: &str) -> std::sync::Arc<terrain_core::PreparedTerrain> {
    let path = assets_root().join("documents").join(document);
    let loaded = terrain_format::load(&path).expect("the document loads");
    terrain_core::prepare(
        &loaded.document,
        &BesideDocument {
            root: assets_root(),
        },
        &terrain_core::SourceRegistry::new(),
        &terrain_core::PrepareOptions::default(),
    )
    .expect("the document prepares")
}

fn rect(min: (f64, f64), max: (f64, f64)) -> WorldRect {
    WorldRect::new(WorldPoint::new(min.0, min.1), WorldPoint::new(max.0, max.1))
}

#[test]
fn constant_grass_compiles_into_an_honest_stack() {
    let terrain = prepared("constant_grass.terrain.ron");
    let grid = FieldGridSpec::covering(rect((-3.0, -3.0), (3.0, 3.0)), 0.05);
    let stack = sample_fields(&terrain, grid);

    assert!(stack.is_well_formed(), "planes must match the grid");
    // One material, everywhere, at full strength — so exactly one substrate
    // plane survives the prune and it sums to one at every sample.
    assert_eq!(stack.substrates.len(), 1, "constant grass is one substrate");
    assert!(
        stack.worst_substrate_sum_error() < 1.0e-5,
        "substrate weights must normalise: worst error {}",
        stack.worst_substrate_sum_error()
    );
    // The declared channel is carried at its default rather than at zero. Zero
    // would read as "vegetation suppressed everywhere", which is a plausible
    // looking answer and the wrong one.
    assert_eq!(stack.modifiers.len(), 1);
    let density = &stack.modifiers[0].values;
    assert!(
        density
            .values
            .iter()
            .all(|value| (*value - 1.0).abs() < 1.0e-5),
        "vegetation density should be the document's default of one"
    );
}

#[test]
fn blend_lab_carries_a_path_that_moves_the_substrate_and_the_channels() {
    let terrain = prepared("blend_lab.terrain.ron");
    // The path runs through the middle of the authored spline, so a window on
    // the origin crosses it.
    let grid = FieldGridSpec::covering(rect((-4.0, -4.0), (4.0, 4.0)), 0.05);
    let stack = sample_fields(&terrain, grid);

    assert!(stack.is_well_formed());
    assert!(
        stack.worst_substrate_sum_error() < 1.0e-5,
        "substrate weights must normalise across a transition"
    );

    // Two substrates appear: the meadow and the path.
    assert_eq!(
        stack.substrates.len(),
        2,
        "a path through grass is two substrates"
    );

    // Somewhere the ground is mostly dirt and somewhere it is mostly grass.
    // Without both, the document compiled into a uniform field and the whole
    // transition is untested.
    let dirt = stack
        .substrates
        .iter()
        .find(|plane| plane.weights.descriptor.key.contains("dirt"))
        .expect("a dirt substrate plane");
    let (low, high) = dirt.weights.extent();
    assert!(low < 0.05, "somewhere should be free of dirt, got {low}");
    assert!(
        high > 0.4,
        "somewhere should be substantially dirt, got {high}"
    );

    // `blend_lab` tops out at an even split, and that is a property of how it
    // is authored rather than of the sampler. Its base grass layer claims the
    // ground with `Replace` at full strength and its path adds a dirt score of
    // one on top, so the centre of the path normalises to 0.5/0.5 and the
    // document cannot express bare ground at all.
    //
    // Asserted rather than fixed here, because fixing it means changing an
    // authored document that other measurements are pinned to. The document
    // that does express a bare core is `meadow_path`, which uses `Replace` for
    // the path core exactly so that the middle of a track is track.
    assert!(
        high < 0.6,
        "blend_lab is expected to top out near an even split; if this now \
         reaches bare dirt the document was changed and this note is stale"
    );

    // And the suppression channel actually suppresses: the vegetation density
    // plane must dip well below its default on the path.
    let density = stack
        .modifiers
        .iter()
        .find(|plane| plane.values.descriptor.key.contains("vegetation_density"))
        .expect("a vegetation density plane");
    let (lowest, highest) = density.values.extent();
    assert!(
        lowest < 0.5,
        "vegetation should be suppressed on the path, lowest was {lowest}"
    );
    assert!(
        highest > 0.9,
        "and unsuppressed off it, highest was {highest}"
    );
}

#[test]
fn two_regions_of_one_document_agree_exactly_where_they_overlap() {
    // The property the whole grid design exists for. Two requests with
    // unrelated rectangles must sample the same world points and get the same
    // numbers, or a nine-tile plate and a re-render of its middle disagree
    // along a line.
    let terrain = prepared("blend_lab.terrain.ron");
    let spacing = 0.05;
    let left = FieldGridSpec::covering(rect((-4.0, -2.0), (0.5, 2.0)), spacing);
    let right = FieldGridSpec::covering(rect((-0.37, -2.0), (4.0, 2.0)), spacing);

    let a = sample_fields(&terrain, left);
    let b = sample_fields(&terrain, right);

    // Walk the overlap on the shared lattice and compare every plane.
    let overlap = left
        .bounds()
        .intersection(right.bounds())
        .expect("the two windows overlap");
    let mut compared = 0usize;
    let mut v = overlap.min.v_m;
    while v <= overlap.max.v_m {
        let mut u = overlap.min.u_m;
        while u <= overlap.max.u_m {
            let at = WorldPoint::new(u, v);
            assert_eq!(
                a.surface_height(at),
                b.surface_height(at),
                "surface disagrees at {u}, {v}"
            );
            // Over the *union* of both windows' materials, not just A's. The
            // prune drops planes that are all zero in a region, so a material
            // present only in B would go unchecked if the walk were one-sided —
            // and "B has a substrate A never heard of" is exactly the
            // disagreement worth catching.
            for plane in a.substrates.iter().chain(b.substrates.iter()) {
                let mine = a.substrate_weight(plane.material, at);
                let theirs = b.substrate_weight(plane.material, at);
                assert!(
                    (mine - theirs).abs() < 1.0e-6,
                    "substrate {:?} disagrees at {u}, {v}: {mine} vs {theirs}",
                    plane.material
                );
            }
            compared += 1;
            u += spacing;
        }
        v += spacing;
    }
    assert!(compared > 1000, "the overlap should be substantial");
}

#[test]
fn sampling_is_the_same_however_many_threads_ran_it() {
    // Rows are sampled in parallel. The stitch is by index, so the result must
    // not depend on the pool — but "must not" is the kind of claim that stops
    // being true after somebody replaces a collect with a reduce.
    let terrain = prepared("blend_lab.terrain.ron");
    let grid = FieldGridSpec::covering(rect((-2.0, -2.0), (2.0, 2.0)), 0.05);

    let once = sample_fields(&terrain, grid);
    let again = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("a single-threaded pool")
        .install(|| sample_fields(&terrain, grid));

    assert_eq!(once.fingerprint(), again.fingerprint());
}

#[test]
fn derived_fields_survive_a_real_document() {
    let terrain = prepared("blend_lab.terrain.ron");
    let grid = FieldGridSpec::covering(rect((-3.0, -3.0), (3.0, 3.0)), 0.05);
    let mut stack = sample_fields(&terrain, grid);
    derive_fields(&mut stack, DerivedFieldRequest::ALL);

    // The path is six centimetres lower than the meadow, so there is a real
    // slope at its shoulder and a real hollow in its middle.
    let slope = stack.derived.slope.as_ref().expect("slope");
    let (_, steepest) = slope.extent();
    assert!(
        steepest > 0.01,
        "a depressed path should produce a measurable slope, got {steepest}"
    );

    let curvature = stack.derived.curvature.as_ref().expect("curvature");
    let (hollow, crest) = curvature.extent();
    assert!(
        hollow < 0.0 && crest > 0.0,
        "a rut has both a hollow and shoulders: {hollow} .. {crest}"
    );

    // And every derived plane is finite and the right length.
    assert!(stack.is_well_formed());
    for (key, plane) in stack.derived.scalar_planes() {
        assert_eq!(
            plane.values.len(),
            grid.sample_count(),
            "{key} is the wrong length"
        );
        assert!(
            plane.values.iter().all(|value| value.is_finite()),
            "{key} holds a non-finite value"
        );
    }
}

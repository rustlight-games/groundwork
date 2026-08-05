//! One surface, and everything standing on it.
//!
//! The defect these tests exist for is invisible from either side. Secondary
//! content used to be rooted at `TerrainFieldStack::surface_height` — authored
//! elevation plus authored microrelief — while the ground mesh Cycles renders
//! adds the ground profile's geometry displacement on top. Both are "the
//! surface" from their own point of view, and they differ by whatever relief the
//! soil happens to have: a couple of centimetres on cloddy loam, which is
//! exactly the scale at which a pebble stops touching the ground and a stem
//! starts growing out of thin air.
//!
//! Nothing reported it. A floating stone is a rendering artefact somebody
//! notices in a close crop weeks later, and by then it looks like a burial
//! parameter that needs tuning rather than a registration bug.

use glam::Vec2;
use terrain_bench::documents::{self, COMPILABLE};
use terrain_bench::meadow;
use terrain_core::coords::WorldPoint;
use terrain_generators::compiler::{SceneCompileOptions, compile_scene};
use terrain_generators::ground::GroundEvaluator;

/// Where the compiled scenes are probed, in world metres.
///
/// A grid rather than one point: displacement varies with the relief field, and
/// a single sample could land where it happens to be near zero.
/// Where the mesh and the analytic surface are compared.
///
/// ## Deliberately never on a lattice vertex
///
/// The gap between the two surfaces is a chord-versus-curve gap: it is exactly
/// zero at every mesh vertex and largest between them. The first version of
/// this stepped by half a metre, and the mesh lattice is a millimetre — so
/// every probe landed precisely on a vertex, where the two surfaces agree by
/// construction, and the guard below measured zero for the right reason and
/// called it a failure.
///
/// It passed for a while anyway, on floating-point luck. That is worse than
/// failing: a guard that holds by accident stops being a guard the moment the
/// accident does.
///
/// The step is therefore an awkward number with no small common factor with a
/// millimetre, and it is offset by a fraction of one so no probe can sit on a
/// vertex however the lattice is later re-spaced.
fn probes() -> Vec<WorldPoint> {
    const STEP_M: f64 = 0.417_3;
    const OFFSET_M: f64 = 0.000_37;
    let mut out = Vec::new();
    for i in 0..9 {
        for j in 0..9 {
            out.push(WorldPoint::new(
                -1.5 + OFFSET_M + i as f64 * STEP_M,
                -1.5 + OFFSET_M + j as f64 * STEP_M,
            ));
        }
    }
    out
}

#[test]
fn the_compilation_hands_back_the_evaluator_it_used() {
    // Not "an equal evaluator" — the same one. Two evaluators built from the
    // same inputs agree until somebody edits one construction site, and the
    // whole reason `SceneCompilation` carries this member is to make the second
    // construction site impossible rather than merely wrong.
    for name in COMPILABLE {
        let terrain = documents::prepare(&documents::shipped(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let request = meadow::baseline_request();
        let registry = terrain_generators::family_registry();
        let compiled = compile_scene(
            &terrain,
            &request,
            &registry,
            &SceneCompileOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));

        // The evaluator reads the same field stack the compilation returns,
        // rather than a copy sampled a second time.
        assert!(
            std::sync::Arc::ptr_eq(compiled.ground.fields(), &compiled.fields),
            "{name}: the evaluator is reading a different field stack from the \
             one the compilation returned"
        );
    }
}

#[test]
fn the_final_surface_is_the_matrix_surface_plus_profile_relief() {
    // The definition, asserted rather than trusted. If `final_surface_z_m` ever
    // stops being exactly this sum — by picking up shader bump, say — every
    // object placed with it moves to a height no mesh in the scene has.
    for name in COMPILABLE {
        let terrain = documents::prepare(&documents::shipped(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let request = meadow::baseline_request();
        let registry = terrain_generators::family_registry();
        let compiled = compile_scene(
            &terrain,
            &request,
            &registry,
            &SceneCompileOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));

        for at in probes() {
            let flat = Vec2::new(at.u_m as f32, at.v_m as f32);
            let matrix = compiled.fields.surface_height(at);
            let relief = compiled.ground.displacement(flat);
            let final_z = compiled.ground.final_surface_z_m(flat);
            assert!(
                (final_z - (matrix + relief)).abs() < 1.0e-6,
                "{name} at {at:?}: final {final_z} is not matrix {matrix} plus relief {relief}"
            );
        }
    }
}

#[test]
fn the_profile_relief_is_not_uniformly_zero() {
    // Guards the test above from being vacuous. If the shipped documents
    // happened to carry no ground relief, "final == matrix + 0" would pass
    // while proving nothing about the registration this phase fixes.
    let terrain = documents::prepare(&documents::shipped("meadow_path")).expect("meadow_path");
    let request = meadow::baseline_request();
    let registry = terrain_generators::family_registry();
    let compiled = compile_scene(
        &terrain,
        &request,
        &registry,
        &SceneCompileOptions::default(),
    )
    .expect("meadow_path compiles");

    let biggest = probes()
        .into_iter()
        .map(|at| {
            compiled
                .ground
                .displacement(Vec2::new(at.u_m as f32, at.v_m as f32))
                .abs()
        })
        .fold(0.0f32, f32::max);
    assert!(
        biggest > 1.0e-4,
        "the shipped meadow has no measurable profile relief ({biggest} m), so the \
         registration tests prove nothing"
    );
}

#[test]
fn the_mesh_surface_differs_from_the_analytic_one_by_enough_to_matter() {
    // Guards the test below from being a tautology. If the mesh's chord and the
    // analytic curve agreed everywhere, registering roots to one rather than
    // the other would be a change with no effect — and the gap this fixes is
    // precisely that they do not agree between vertices.
    let terrain = documents::prepare(&documents::shipped("meadow_path")).expect("meadow_path");
    let compiled = compile_scene(
        &terrain,
        &meadow::baseline_request(),
        &terrain_generators::family_registry(),
        &SceneCompileOptions::default(),
    )
    .expect("meadow_path compiles");

    let spacing = compiled.ground.mesh_spacing_m();
    let worst = probes()
        .into_iter()
        .map(|at| {
            let flat = Vec2::new(at.u_m as f32, at.v_m as f32);
            (compiled.ground.final_surface_z_m(flat)
                - compiled.ground.mesh_surface_z_m(flat, spacing))
            .abs()
        })
        .fold(0.0f32, f32::max);
    assert!(
        worst > 1.0e-4,
        "the mesh and the analytic surface differ by only {worst} m, so rooting \
         against one rather than the other proves nothing"
    );
}

#[test]
fn every_secondary_root_sits_on_the_mesh_the_renderer_draws() {
    // The acceptance criterion itself: nothing the compiler emitted is rooted
    // at a height the rendered ground does not have.
    //
    // Against the *mesh*, not the analytic surface. Between two lattice
    // vertices the rendered ground is the chord and the analytic surface is the
    // curve; over a crest the curve is above the chord by up to about half a
    // band amplitude, which is more than a stem is thick. A flower registered
    // to the curve therefore stands a visible gap above the ground it is
    // supposed to be growing out of, worst exactly where the ground is most
    // interesting.
    //
    // Checked against the mark roots the scene actually holds rather than
    // against a recomputed placement, so a recipe that ignored `surface_z_m`
    // and used the matrix directly would still be caught.
    for name in COMPILABLE {
        let terrain = documents::prepare(&documents::shipped(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let request = meadow::baseline_request();
        let registry = terrain_generators::family_registry();
        let compiled = compile_scene(
            &terrain,
            &request,
            &registry,
            &SceneCompileOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));

        let mut checked = 0usize;
        for mark in &compiled.scene.marks {
            // Curves and ribbons are rooted at the ground. Analytic marks are
            // centred on their own body and deliberately offset below it — a
            // stone is settled *into* the soil — so their root is not a surface
            // sample and this test does not claim it is.
            let terrain_scene::mark::SceneMark::Curve(curve) = mark else {
                continue;
            };
            let root = curve.root;
            let expected = compiled.ground.mesh_surface_z_m(
                Vec2::new(root.u_m as f32, root.v_m as f32),
                compiled.ground.mesh_spacing_m(),
            );
            assert!(
                (root.z_m as f32 - expected).abs() < 1.0e-4,
                "{name}: a stem is rooted at {} where the rendered ground is at {expected}",
                root.z_m
            );
            checked += 1;
        }
        assert!(checked > 0, "{name}: no rooted marks to check");
    }
}

#[test]
fn a_bare_evaluator_reports_the_matrix_surface_unchanged() {
    // The degenerate case, pinned because it is what a document with no ground
    // profiles gets. No profiles means no relief, so the final surface must be
    // the matrix surface exactly — not approximately, and not offset by a
    // default band amplitude that crept in.
    use std::sync::Arc;
    use terrain_scene::field::{FieldGridSpec, TerrainFieldStack};

    let fields = Arc::new(TerrainFieldStack::flat(FieldGridSpec::covering(
        terrain_core::coords::WorldRect::new(
            WorldPoint::new(-2.0, -2.0),
            WorldPoint::new(2.0, 2.0),
        ),
        0.05,
    )));
    let ground = GroundEvaluator::bare(
        Arc::clone(&fields),
        terrain_generators::TransitionProfile::default(),
        0x5a17_e33b_0c9d_2f14,
    );
    for at in probes() {
        let flat = Vec2::new(at.u_m as f32, at.v_m as f32);
        assert_eq!(
            ground.final_surface_z_m(flat).to_bits(),
            fields.surface_height(at).to_bits(),
            "a profileless ground moved its own surface at {at:?}"
        );
    }
}

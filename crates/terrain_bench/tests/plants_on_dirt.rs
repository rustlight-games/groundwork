//! Nothing that needs soil should be rooted in the track.
//!
//! Written because a render kept showing daisies and rosettes standing on bare
//! compacted earth after two separate fixes that should each have stopped it.
//! Looking at a picture cannot distinguish "the suppression is not working"
//! from "that ground is not as bare as it looks", and the difference decides
//! where to look next.

use glam::Vec2;
use terrain_bench::{documents, meadow};
use terrain_generators::compiler::{SceneCompileOptions, compile_scene};

#[test]
fn no_plant_roots_where_the_ground_supports_nothing() {
    let terrain = documents::prepare(&documents::shipped("meadow_path")).expect("meadow_path");
    let compiled = compile_scene(
        &terrain,
        &meadow::baseline_request(),
        &terrain_generators::family_registry(),
        &SceneCompileOptions::default(),
    )
    .expect("compiles");

    // Which appearances belong to plants. A stone does not care what grows
    // around it and is deliberately exempt.
    let plant = |key: &str| key.starts_with("flower.") || key.starts_with("plant.");

    let mut worst = 1.0f32;
    let mut offenders = 0usize;
    let mut checked = 0usize;
    for mark in &compiled.scene.marks {
        let Some(binding) = compiled.scene.materials.get(mark.material().0 as usize) else {
            continue;
        };
        if !plant(binding.appearance.as_str()) {
            continue;
        }
        checked += 1;
        let root = mark.root();
        let support = compiled.ground.support_for(
            &compiled
                .ground
                .substrates(Vec2::new(root.u_m as f32, root.v_m as f32)),
        );
        if support < 0.20 {
            offenders += 1;
            worst = worst.min(support);
        }
    }
    // What the vegetation channel actually says where the plants are. Printed
    // rather than asserted, because the question this run is answering is
    // "which control is not reaching them" and a bare pass/fail cannot say.
    let mut buckets = [0usize; 5];
    for mark in &compiled.scene.marks {
        let Some(binding) = compiled.scene.materials.get(mark.material().0 as usize) else {
            continue;
        };
        if !plant(binding.appearance.as_str()) {
            continue;
        }
        let root = mark.root();
        let density = compiled
            .ground
            .abundance(Vec2::new(root.u_m as f32, root.v_m as f32));
        buckets[((density * 4.0).clamp(0.0, 4.0) as usize).min(4)] += 1;
    }
    println!("vegetation_density at plant roots, by quintile: {buckets:?}");

    assert!(checked > 100, "only {checked} plant marks to check");
    assert_eq!(
        offenders, 0,
        "{offenders} of {checked} plant marks root on ground with as little as \
         {worst:.3} vegetation support"
    );
}

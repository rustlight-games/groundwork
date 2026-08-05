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
    // What the vegetation channel actually says where the plants are.
    //
    // This started as a printout, because the question was "which control is
    // not reaching them" and a bare pass/fail cannot say. It answered it: the
    // distribution was `[0, 35, 91, 438, 4605]`, so no plant was below a
    // quarter and the material controls were working — the ground looked bare
    // because *grass* is what covers ground, and the flower suppression band
    // was only at full strength inside the very centre of the track while the
    // dirt ran twice as wide.
    //
    // With the band covering the material band it is `[0, 0, 1, 27, ...]`, and
    // the printout became a gate.
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

    // Almost every plant stands where the author asked for a full meadow. A
    // handful in the fringe is right — grass thins before the ground stops
    // being grass, and a flower in that fringe is a flower — so the gate is on
    // the *lower half* rather than on perfection.
    let sparse: usize = buckets[..3].iter().sum();
    assert!(
        sparse * 100 < checked,
        "{sparse} of {checked} plant marks stand on ground with under \
         three quarters of its vegetation: {buckets:?}"
    );

    assert!(checked > 100, "only {checked} plants to check");
    assert_eq!(
        offenders, 0,
        "{offenders} of {checked} plant marks root on ground with as little as \
         {worst:.3} vegetation support"
    );
}

#[test]
fn no_plant_roots_on_a_substrate_its_population_did_not_name() {
    // The hard denial, asserted directly rather than inferred from the
    // distribution above.
    //
    // ## Why a ramp was not enough
    //
    // Every control before this one was continuous — a support smoothstep and
    // an abundance multiply — and a ramp always leaves a tail. A fortieth of
    // two flowers a square metre over ten square metres of track is still half
    // a flower, and half a flower rounds to one you can see standing on bare
    // compacted earth. Tuning the ramp harder only moves the tail; it never
    // removes it, and every attempt to remove it by tuning made the fringe
    // worse, because the fringe is what the ramp is *for*.
    //
    // So the rule is now categorical: a population that named its materials
    // places nothing at all where the dominant substrate is not one of them.
    // Grass may still thin through a transition, because a sward genuinely
    // does. A daisy does not grow in a footpath at any density.
    //
    // ## Why every document rather than one
    //
    // `meadow_path` alone cannot test this. Its own suppression bands are wide
    // enough that no plant candidate survives as far as the track in the first
    // place, so the denial is never reached and a single-document test would
    // pass vacuously — which is exactly the shape of test this file exists
    // because of. Swept across the whole compilable set, the count below is the
    // proof that the rule was actually exercised somewhere.
    let mut checked = 0usize;
    let mut in_the_fringe = 0usize;
    let mut trespassers = 0usize;

    for name in documents::COMPILABLE {
        let terrain = documents::prepare(&documents::shipped(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let compiled = compile_scene(
            &terrain,
            &meadow::baseline_request(),
            &terrain_generators::family_registry(),
            &SceneCompileOptions::default(),
        )
        .unwrap_or_else(|error| panic!("{name}: {error}"));

        let plant = |key: &str| key.starts_with("flower.") || key.starts_with("plant.");

        // ## The unit is the plant, not the mark
        //
        // A mark's own position is not where its plant is rooted. A petal sits
        // centimetres out from the stem and a rosette leaf reaches a hand's
        // width from its crown, so a plant standing on grass a centimetre from
        // the track has marks *over* the track — which is exactly what a plant
        // at the edge of a path does, and denying it would carve a plant-shaped
        // exclusion zone along every boundary.
        //
        // Measured before this was fixed: four of the seven and a half thousand
        // plant marks in the shipped set had their own centre over hostile
        // ground while every one of the plants that grew them was rooted in the
        // meadow. Four petals leaning over a track is not the bug this file was
        // written about.
        //
        // The placement anchor is also the unit the rule is written in: the
        // compiler denies a *candidate*, and a candidate becomes an anchor.
        let mut seen: std::collections::BTreeSet<usize> = Default::default();
        for mark in &compiled.scene.marks {
            let Some(binding) = compiled.scene.materials.get(mark.material().0 as usize) else {
                continue;
            };
            if !plant(binding.appearance.as_str()) {
                continue;
            }
            let anchor = mark.anchor();
            if anchor == terrain_scene::mark::AnchorIndex::UNGROUPED || !seen.insert(anchor.index())
            {
                continue;
            }
            let Some(placement) = compiled.scene.anchors.get(anchor.index()) else {
                continue;
            };
            checked += 1;
            let at = Vec2::new(placement.root.u_m as f32, placement.root.v_m as f32);
            let substrate = compiled.ground.substrates(at);

            // No plant population in any shipped document names anything but
            // `meadow_soil`, so "a material this plant did not name" is "a
            // material with no vegetation affinity". Asked of the ground rather
            // than of the population table because a mark carries its
            // appearance, not the key of the population that grew it.
            let hostile = |material: terrain_core::MaterialIndex| {
                compiled.ground.material_affinity(material) < 0.5
            };

            if substrate
                .iter()
                .any(|(material, weight)| hostile(material) && weight > 0.02)
            {
                in_the_fringe += 1;
            }
            if let Some((material, _)) = substrate.dominant()
                && hostile(material)
            {
                trespassers += 1;
            }
        }
    }

    println!("{in_the_fringe} of {checked} plants stand on ground with a hostile share");
    assert!(checked > 100, "only {checked} plants to check");
    // Twelve at the time of writing, across the five compilable documents.
    // Small, and it has to be checked rather than assumed: these are the plants
    // standing close enough to a track that the denial had a decision to make,
    // and if a document edit ever moves every plant a metre clear of every
    // boundary then the assertion below stops proving anything and this says so.
    assert!(
        in_the_fringe >= 5,
        "only {in_the_fringe} plants stand anywhere a hostile material has any \
         claim, so the denial below was never tested"
    );
    assert_eq!(
        trespassers, 0,
        "{trespassers} of {checked} plants root on ground whose dominant \
         substrate their population never named"
    );
}

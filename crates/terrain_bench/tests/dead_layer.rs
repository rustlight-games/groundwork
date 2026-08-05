//! Is there actually a dead layer down there, and is it free when nobody asked?
//!
//! ## The two questions, and why they need different instruments
//!
//! The dead bottom layer is an *optional* semantic: a document declares a
//! channel in the [`DeadLitter`] role and a share of the sward's floor is drawn
//! as straw instead of thatch. Optional features have a characteristic failure
//! mode — they are not free when switched off — and it is invisible by
//! inspection, because the code that runs at zero looks like the code that ran
//! before it existed.
//!
//! Here it would have been very easy to get wrong. The tuned generator draws
//! from a *sequential* stream, so a `chance(litter)` evaluated unconditionally
//! advances that stream whether or not it fires, and every blade after it in the
//! page shifts by one draw. The whole meadow would have moved for a feature
//! nobody switched on, and the only symptom would have been a fingerprint that
//! changed for no stated reason. `refactor_fingerprints` catches that, and this
//! file states the guarantee in the place a reader would look for it.
//!
//! The second question is the opposite one and needs a real document rather than
//! a synthetic field: with the channel authored, does the *floor* of the sward
//! go dry while the *standing* grass does not? A feature that made the whole
//! meadow browner would pass any "is it different" test and be exactly wrong —
//! the user asked for a layer, not a filter.
//!
//! [`DeadLitter`]: terrain_core::document::ModifierRole::DeadLitter

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::Vec2;
use terrain_bench::documents;
use terrain_generators::field::{SemanticOverlay, WorldField};
use terrain_generators::ground::GroundEvaluator;
use terrain_generators::interaction::InteractionField;
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;
use terrain_generators::stroke::Stroke;
use terrain_generators::style::GrassParams;
use terrain_generators::tone::Tone;
use terrain_generators::tuned::{TunedPass, TunedPopulationSet};

const SEED: u64 = 0x5a17_e33b_0c9d_2f14;

/// The tuned meadow over one document, as tone counts per pass.
fn tones(document: Option<&str>) -> BTreeMap<(TunedPass, Tone), usize> {
    let mut counts = BTreeMap::new();
    for mark in strokes(document) {
        *counts.entry((mark.pass, mark.tone)).or_default() += 1;
    }
    counts
}

/// Every tuned stroke on one pinned page, over one document or over none.
fn strokes(document: Option<&str>) -> Vec<Stroke> {
    let params = GrassParams {
        seed: SEED,
        ..GrassParams::default()
    };
    let field = WorldField::lit_by(params.seed, params.light);
    let field = match document {
        None => field,
        Some(name) => {
            let terrain = documents::prepare(&documents::shipped(name))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            let request = terrain_bench::meadow::baseline_request();
            let registry = terrain_generators::family_registry();
            let compiled = terrain_generators::compiler::compile_scene(
                &terrain,
                &request,
                &registry,
                &terrain_generators::compiler::SceneCompileOptions::default(),
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            field.with_overlay(Arc::new(SemanticOverlay {
                ground: Arc::clone(&compiled.ground),
                interactions: Arc::clone(&compiled.interactions),
                tuned: Arc::clone(&compiled.tuned),
            }))
        }
    };

    GrassScene::build(Page::new(Vec2::new(0.0, 0.0), 192, 192), &field, &params).marks
}

/// The share of one pass that is drawn dry.
fn dry_share(counts: &BTreeMap<(TunedPass, Tone), usize>, pass: TunedPass) -> f64 {
    let total: usize = counts
        .iter()
        .filter(|((p, _), _)| *p == pass)
        .map(|(_, n)| *n)
        .sum();
    if total == 0 {
        return 0.0;
    }
    counts.get(&(pass, Tone::Dry)).copied().unwrap_or(0) as f64 / total as f64
}

#[test]
fn a_meadow_nobody_asked_about_has_no_dead_layer() {
    // The laboratory meadow: no document, so no channel, so no litter. The
    // reference art this generator was tuned against has a dark green mat and
    // the thatch pass must still be entirely thatch.
    let plain = tones(None);
    assert_eq!(
        dry_share(&plain, TunedPass::Thatch),
        0.0,
        "the untouched laboratory meadow grew dry thatch, which means the \
         litter default is not zero"
    );
}

#[test]
fn an_authored_litter_dries_the_floor() {
    // `narrow_track` declares a third of its bottom layer dead. The thatch pass
    // is that layer, so a third of it — give or take the sampling of a
    // few-thousand-stroke page — must come out dry.
    let counts = tones(Some("narrow_track"));
    let share = dry_share(&counts, TunedPass::Thatch);
    assert!(
        (0.20..0.45).contains(&share),
        "the thatch pass is {:.1}% dry against an authored third",
        share * 100.0
    );
}

#[test]
fn it_is_a_layer_and_not_a_filter() {
    // The measurement that makes this feature what the user asked for rather
    // than a brown tint over everything. Real turf is green blades screening
    // straw, so the deadness has to be *underneath*.
    //
    // ## The pass that closes the surface must be untouched
    //
    // `TunedPass::Fine` is the largest population in the field by an order of
    // magnitude and the one that decides what the middle scale looks like. It
    // is also, by construction, entirely green: `fine_stroke` writes
    // `Tone::Grass` unconditionally and always has. So the strongest available
    // statement of "this is a layer and not a filter" is an absolute rather
    // than a comparison — not one dry stroke in the surface, in a document that
    // asked for a third of its floor to be dead.
    //
    // No control run is needed for that, which matters more than it looks:
    // two different documents differ in far more than their litter, and an
    // across-document comparison of dry shares reads the tuned field's own
    // rim-straw rule as this feature leaking.
    let authored = strokes(Some("narrow_track"));
    for pass in [TunedPass::Fine, TunedPass::Broadleaf] {
        let dry = authored
            .iter()
            .filter(|m| m.pass == pass && m.tone == Tone::Dry)
            .count();
        assert_eq!(
            dry, 0,
            "{pass:?} grew {dry} dry strokes — the dead layer is reaching the \
             canopy that closes the surface, which makes it a filter over the \
             whole sward rather than a layer under it"
        );
    }

    // ## What is deliberately not asserted here
    //
    // The tuft pass also takes litter, in its understorey — the short strokes
    // laid hard over the floor inside a clump — and that is intended: a
    // document that greyed the open mat while every clump stayed green at its
    // heart would produce dead ground with live islands on it rather than one
    // sward with an old bottom.
    //
    // It is not asserted because it cannot be isolated. The tuned field has
    // always scattered straw tillers around the rim of an opening, keyed on
    // `hue` and `bare`, and a document with a track in it has a great deal of
    // rim. Measured on `narrow_track` a tuft's buried strokes come out 11% dry
    // against its standing ones at 6%, and neither figure separates this
    // feature from that rule. The two statements above do not have that problem
    // and say the same thing: the floor went dry, the surface did not.
}

#[test]
fn the_wettest_document_carries_the_least() {
    // The check that this is a semantic and not a look. Dead matter in a wet
    // hollow rots rather than accumulating, and the three documents that
    // declare the channel were authored in that order — so if the ranking ever
    // inverts, something is driving litter from brightness or from taste rather
    // than from what the ground is.
    let wet = dry_share(&tones(Some("wet_hollow")), TunedPass::Thatch);
    let ordinary = dry_share(&tones(Some("narrow_track")), TunedPass::Thatch);
    let poor = dry_share(&tones(Some("stony_pasture")), TunedPass::Thatch);
    assert!(
        wet < ordinary && ordinary < poor,
        "wet {wet:.3}, ordinary {ordinary:.3}, poor {poor:.3} — the litter \
         ranking is not the moisture ranking reversed"
    );
}

#[test]
fn bare_ground_grows_no_litter() {
    // Straw is grass that grew and died. Ground that never grew anything has
    // none of it, and a track scattered with straw is litter blown onto it —
    // a different thing, which an author would place rather than inherit.
    //
    // Checked through the field rather than through the strokes, because the
    // question is about the *channel* and the strokes only sample it.
    let terrain = documents::prepare(&documents::shipped("meadow_path"))
        .unwrap_or_else(|error| panic!("{error}"));
    let request = terrain_bench::meadow::baseline_request();
    let registry = terrain_generators::family_registry();
    let compiled = terrain_generators::compiler::compile_scene(
        &terrain,
        &request,
        &registry,
        &terrain_generators::compiler::SceneCompileOptions::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let params = GrassParams {
        seed: SEED,
        ..GrassParams::default()
    };
    let field =
        WorldField::lit_by(params.seed, params.light).with_overlay(Arc::new(SemanticOverlay {
            ground: Arc::clone(&compiled.ground),
            interactions: Arc::new(InteractionField::default()),
            tuned: Arc::new(TunedPopulationSet::new()),
        }));

    let mut worst_on_bare: f32 = 0.0;
    let mut seen_bare = 0usize;
    for row in -40..=40 {
        for column in -40..=40 {
            let at = Vec2::new(column as f32 * 0.05, row as f32 * 0.05);
            let ground = field.sample(at);
            if ground.bare > 0.9 {
                seen_bare += 1;
                worst_on_bare = worst_on_bare.max(ground.litter);
            }
        }
    }

    assert!(
        seen_bare > 50,
        "only {seen_bare} of the sampled points were bare, so this proves \
         nothing — the document or the window moved"
    );
    assert!(
        worst_on_bare < 0.10,
        "fully bare ground carries a litter of {worst_on_bare:.3}"
    );
}

/// The evaluator reads zero where no channel claims the role.
#[test]
fn a_document_with_no_channel_reports_no_litter() {
    // The default asserted directly, so the guarantee does not rest on every
    // shipped document happening not to declare one.
    let fields = Arc::new(terrain_scene::field::TerrainFieldStack::flat(
        terrain_scene::field::FieldGridSpec::covering(
            terrain_core::coords::WorldRect::new(
                terrain_core::coords::WorldPoint::new(-2.0, -2.0),
                terrain_core::coords::WorldPoint::new(2.0, 2.0),
            ),
            0.05,
        ),
    ));
    let ground = GroundEvaluator::bare(fields, terrain_generators::TransitionProfile::SMOOTH, SEED);
    assert_eq!(ground.dead_litter(Vec2::new(0.3, -0.7)), 0.0);
}

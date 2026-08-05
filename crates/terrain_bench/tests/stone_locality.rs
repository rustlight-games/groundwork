//! Turning a stone off must move the grass beside it and nothing else.
//!
//! ## Why this is the test and not a picture
//!
//! "Grass grows around stones" is easy to check by looking and impossible to
//! check *rigorously* by looking: a render with stones and a render without them
//! differ everywhere, because the stones are in one of them. What matters is
//! whether they differ everywhere they should not.
//!
//! So this compares the two meadows stroke by stroke. Outside every declared
//! response reach the two must be **bit-identical** — not close, identical —
//! because in a deterministic generator the only thing that can make a distant
//! blade move is a dependence nobody intended. Inside the reach the plants may
//! differ, and the ones that differ are counted so the response can be shown to
//! be doing something at all.
//!
//! That is the difference between proving local causality and observing visual
//! plausibility, and it is the same distinction the seam tests make.

use std::sync::Arc;

use glam::Vec2;
use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_generators::field::{SemanticOverlay, WorldField};
use terrain_generators::ground::GroundEvaluator;
use terrain_generators::interaction::{InteractionField, ObstacleResponse};
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;
use terrain_generators::stroke::Stroke;
use terrain_generators::style::GrassParams;
use terrain_generators::tuned::TunedPass;
use terrain_scene::field::{FieldGridSpec, TerrainFieldStack};
use terrain_scene::mark::{AnchorIndex, MarkId};
use terrain_scene::scene::{InteractionChannels, InteractionPrimitive, InteractionShape};

/// Where the test stones sit, and how big they are.
const STONES: &[(f64, f64, f32, f32)] = &[
    (0.35, 0.20, 0.070, 0.052),
    (-0.20, 0.55, 0.048, 0.041),
    (0.10, -0.40, 0.095, 0.060),
];

const HARD_CLEARANCE_M: f32 = 0.008;
const RESPONSE_REACH_M: f32 = 0.14;

fn overlay(with_stones: bool) -> Arc<SemanticOverlay> {
    let fields = Arc::new(TerrainFieldStack::flat(FieldGridSpec::covering(
        WorldRect::new(WorldPoint::new(-4.0, -4.0), WorldPoint::new(4.0, 4.0)),
        0.05,
    )));
    let ground = Arc::new(GroundEvaluator::bare(
        fields,
        terrain_generators::TransitionProfile::SMOOTH,
        0x5a17_e33b_0c9d_2f14,
    ));
    let primitives = if with_stones {
        STONES
            .iter()
            .enumerate()
            .map(|(index, (u, v, a, b))| InteractionPrimitive {
                source: MarkId(index as u64 + 1),
                anchor: AnchorIndex::UNGROUPED,
                centre: WorldPoint::new(*u, *v),
                shape: InteractionShape::Ellipse {
                    semi_u_m: *a,
                    semi_v_m: *b,
                    yaw_rad: index as f32 * 0.9,
                },
                hard_clearance_m: HARD_CLEARANCE_M,
                response_reach_m: RESPONSE_REACH_M,
                channels: InteractionChannels::ALL_TUNED,
            })
            .collect()
    } else {
        Vec::new()
    };
    Arc::new(SemanticOverlay {
        ground,
        interactions: Arc::new(InteractionField::from_primitives(primitives)),
        // No document controls, so every pass factor is one and the tuned
        // meadow is exactly as tuned. The stones are the only variable.
        tuned: Arc::new(terrain_generators::tuned::TunedPopulationSet::new()),
    })
}

fn meadow(with_stones: bool) -> GrassScene {
    let params = GrassParams::default();
    let field = WorldField::lit_by(params.seed, params.light).with_overlay(overlay(with_stones));
    GrassScene::build(Page::new(Vec2::new(0.0, 0.0), 192, 192), &field, &params)
}

/// How far outside the *containing circle* of the nearest stone a point is.
///
/// A conservative under-estimate of the true elliptical clearance, because the
/// circle contains the ellipse. Used for "definitely far enough away".
fn distance_outside_nearest(root: Vec2) -> f32 {
    STONES
        .iter()
        .map(|(u, v, a, b)| (root - Vec2::new(*u as f32, *v as f32)).length() - a.max(*b))
        .fold(f32::MAX, f32::min)
}

/// How far inside the *inscribed* circle of the nearest stone a point is.
///
/// The other direction, and the other bound: a point inside the inscribed
/// circle is inside the ellipse whatever its orientation. Used for "definitely
/// inside", so the exclusion test cannot fail on a point that is inside a
/// circle and outside the ellipse it approximates.
fn depth_inside_nearest(root: Vec2) -> f32 {
    STONES
        .iter()
        .map(|(u, v, a, b)| a.min(*b) - (root - Vec2::new(*u as f32, *v as f32)).length())
        .fold(f32::NEG_INFINITY, f32::max)
}

/// How far a plant's furthest mark can sit from the root the response was
/// sampled at.
///
/// The response is queried once per *placement*, at the scatter cell's jittered
/// root, and then applied to every mark that placement grew. A tuft's blades
/// spread around their anchor and a leaf cluster spreads further, so a mark can
/// be this much further from a stone than the point that decided its response.
///
/// Measured against the vocabulary rather than assumed: the guard below is
/// generous enough that the "far" set is unambiguous, and the test asserts the
/// set is still large so the generosity has not emptied it.
const PLACEMENT_SPREAD_M: f32 = 0.45;

/// Every field of a stroke that describes it, as comparable bits.
fn describe(stroke: &Stroke) -> Vec<u32> {
    vec![
        stroke.root.x.to_bits(),
        stroke.root.y.to_bits(),
        stroke.root.z.to_bits(),
        stroke.azimuth.to_bits(),
        stroke.length.to_bits(),
        stroke.bend.to_bits(),
        stroke.curl.to_bits(),
        stroke.sway.to_bits(),
        stroke.kink.to_bits(),
        stroke.width.to_bits(),
        stroke.twist.to_bits(),
        stroke.maturity.to_bits(),
        stroke.base_light.to_bits(),
        stroke.glint.to_bits(),
    ]
}

#[test]
fn outside_every_reach_the_two_meadows_are_bit_identical() {
    // The acceptance criterion. In a deterministic generator the only thing
    // that can move a distant blade is a dependence nobody intended, so this is
    // an equality rather than a tolerance.
    let bare = meadow(false);
    let stony = meadow(true);

    // The response can only reach this far past a stone's own extent, plus
    // however far a mark can sit from the root that was sampled.
    let reach = HARD_CLEARANCE_M + RESPONSE_REACH_M + PLACEMENT_SPREAD_M;

    // Indexed by root position, because the stony meadow is missing the plants
    // whose roots fell inside a stone and the two lists are therefore not the
    // same length.
    let far_bare: Vec<&Stroke> = bare
        .marks
        .iter()
        .filter(|m| distance_outside_nearest(m.root.truncate()) > reach)
        .collect();
    let far_stony: Vec<&Stroke> = stony
        .marks
        .iter()
        .filter(|m| distance_outside_nearest(m.root.truncate()) > reach)
        .collect();

    assert!(
        far_bare.len() > 500,
        "only {} distant marks",
        far_bare.len()
    );
    assert_eq!(
        far_bare.len(),
        far_stony.len(),
        "the stones changed how many plants grew well away from them"
    );
    for (a, b) in far_bare.iter().zip(&far_stony) {
        assert_eq!(
            describe(a),
            describe(b),
            "a mark at {:?}, {:.3} m from the nearest stone, is not the same plant",
            a.root,
            distance_outside_nearest(a.root.truncate())
        );
    }
}

#[test]
fn no_single_mark_plant_roots_inside_a_stone() {
    // The hard half of the response, stated at the granularity it actually
    // holds.
    //
    // The fine and thatch passes plant one mark at the root that was tested, so
    // for them "no root inside a stone" is exact. The tuft and broadleaf passes
    // spread their blades up to eighteen centimetres around an anchor, and
    // widening the exclusion by that would carve an eighteen-centimetre bald
    // ring around a seven-centimetre stone — which is the artefact the whole
    // response exists to avoid. A clump rooted clear of a stone may lay a blade
    // across it, which is what real grass does.
    let stony = meadow(true);
    let mut checked = 0usize;
    for mark in &stony.marks {
        if !matches!(mark.pass, TunedPass::Fine | TunedPass::Thatch) {
            continue;
        }
        checked += 1;
        // Against the inscribed circle: a point inside that is inside the
        // ellipse whatever its orientation, so a failure here is a real root in
        // a real stone rather than the circular approximation disagreeing with
        // the elliptical one at the margin.
        let depth = depth_inside_nearest(mark.root.truncate());
        assert!(
            depth < 0.0,
            "a {} root sits {depth:.4} m inside a stone",
            mark.pass
        );
    }
    assert!(checked > 200, "only {checked} single-mark plants to check");
}

#[test]
fn far_fewer_blades_lie_over_a_stone_than_would_have() {
    // The clump passes, measured rather than excluded. A tuft rooted beside a
    // stone may lay a blade across it and should; what must not happen is the
    // stone having as much grass on it as the ground beside it.
    let bare = meadow(false);
    let stony = meadow(true);
    let over = |scene: &GrassScene| {
        scene
            .marks
            .iter()
            .filter(|m| depth_inside_nearest(m.root.truncate()) > 0.0)
            .count()
    };
    let before = over(&bare);
    let after = over(&stony);
    assert!(before > 10, "only {before} plants would have grown there");
    // Measured at about a third rather than chosen: excluding anchors removes
    // roughly two thirds of what would have grown over a stone, and the
    // remainder are blades belonging to clumps rooted outside it. The gate is
    // set loosely around that, because what it is guarding against is the
    // exclusion silently stopping working — which would put the figure back at
    // one.
    assert!(
        (after as f32) < before as f32 * 0.5,
        "{after} of {before} plants still root inside a stone"
    );
}

#[test]
fn the_bare_meadow_has_roots_where_the_stones_would_be() {
    // Guards the test above from being vacuous. If nothing ever grew there, the
    // exclusion would be untested and would pass by accident.
    let bare = meadow(false);
    let inside = bare
        .marks
        .iter()
        .filter(|m| depth_inside_nearest(m.root.truncate()) > 0.0)
        .count();
    assert!(
        inside > 5,
        "only {inside} plants would have grown where the stones are"
    );
}

#[test]
fn plants_near_a_stone_lean_away_from_it() {
    // The soft half, and the reason this is not a circular exclusion. A stone
    // dropped into a pre-cut lawn has full-length blades standing to attention
    // against a hard ring; a stone that has been *there* has shorter ones
    // leaning off it.
    //
    // Reported as a mean alignment rather than asserted per plant: forcing
    // every blade radially outward would itself be a visible ring, so what is
    // wanted is a bias, not a rule.
    let stony = meadow(true);
    let mut alignment = 0.0f32;
    let mut counted = 0usize;
    for mark in &stony.marks {
        let root = mark.root.truncate();
        let clearance = distance_outside_nearest(root);
        if clearance > HARD_CLEARANCE_M + RESPONSE_REACH_M * 0.5 {
            continue;
        }
        // The direction away from the nearest stone.
        let nearest = STONES
            .iter()
            .min_by(|a, b| {
                let da = (root - Vec2::new(a.0 as f32, a.1 as f32)).length();
                let db = (root - Vec2::new(b.0 as f32, b.1 as f32)).length();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("there are stones");
        let away = (root - Vec2::new(nearest.0 as f32, nearest.1 as f32)).normalize_or(Vec2::X);
        alignment += Vec2::from_angle(mark.azimuth).dot(away);
        counted += 1;
    }
    assert!(counted > 20, "only {counted} plants inside a response band");
    let mean = alignment / counted as f32;
    assert!(
        mean > 0.15,
        "plants near a stone lean outward by only {mean:.3} on average"
    );
}

#[test]
fn plants_near_a_stone_are_shorter_than_their_own_selves() {
    // Compared against the *same plant* in the meadow without stones, not
    // against the meadow's average. A plant that happened to be short anyway
    // proves nothing.
    let bare = meadow(false);
    let stony = meadow(true);

    let mut compared = 0usize;
    let mut shortened = 0usize;
    for near in stony
        .marks
        .iter()
        .filter(|m| distance_outside_nearest(m.root.truncate()) < HARD_CLEARANCE_M + 0.05)
    {
        let Some(twin) = bare
            .marks
            .iter()
            .find(|m| m.root.x == near.root.x && m.root.y == near.root.y && m.pass == near.pass)
        else {
            continue;
        };
        compared += 1;
        if near.length < twin.length {
            shortened += 1;
        }
    }
    assert!(compared > 20, "only {compared} plants to compare");
    assert_eq!(
        compared,
        shortened,
        "{} of {compared} plants near a stone were not shortened",
        compared - shortened
    );
}

#[test]
fn every_pass_declares_a_response_and_thatch_bends_least() {
    // The mat lies over whatever is there rather than growing around it, so it
    // is excluded from the footprint and barely turned.
    for pass in TunedPass::ALL {
        let response = ObstacleResponse::for_pass(pass);
        assert!(response.hard_exclusion, "{pass} lets roots inside a stone");
    }
    assert!(
        ObstacleResponse::for_pass(TunedPass::Thatch).direction_strength
            < ObstacleResponse::for_pass(TunedPass::Broadleaf).direction_strength
    );
}

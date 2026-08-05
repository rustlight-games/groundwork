//! Placement groups, prototype canonicalisation, and obstacle footprints.
//!
//! Three additions to the scene vocabulary, and one shared reason for all of
//! them: the compiled scene is about to reach Cycles, and everything a trace
//! slice decides has to be decidable *per plant* rather than per primitive.

use terrain_core::coords::{CellCoord, WorldPoint};
use terrain_core::digest::Fingerprint;
use terrain_core::document::ParameterObject;
use terrain_core::ids::RecipeKey;
use terrain_core::seed::{CandidateId, PopulationHash};
use terrain_scene::instance::{PrototypeBinding, PrototypeIndex, PrototypeInstance};
use terrain_scene::mark::{
    Aabb3, AnchorIndex, CurveMark, MarkAttributes, MarkId, PainterOrder, SceneMark,
    SceneMaterialIndex, Stratum,
};
use terrain_scene::projection::ScenePoint;
use terrain_scene::scene::{
    InteractionChannels, InteractionPrimitive, InteractionShape, PlacementAnchor, SceneBuilder,
    SceneRequest,
};

fn builder() -> SceneBuilder {
    SceneBuilder::new(
        SceneRequest::square(WorldPoint::ORIGIN, 4.0, 96.0),
        Fingerprint::from_u128(0x1234),
        1,
    )
}

fn candidate(rank: u16) -> CandidateId {
    CandidateId::new(
        PopulationHash::from_bits(0xabcd),
        CellCoord::new(1, 2),
        rank,
    )
}

fn anchor_at(x: f64, y: f64) -> PlacementAnchor {
    PlacementAnchor {
        candidate: candidate(0),
        root: ScenePoint::new(x, y, 0.0),
    }
}

fn curve(id: u64, anchor: AnchorIndex, root: ScenePoint, reach: f64) -> SceneMark {
    SceneMark::Curve(CurveMark {
        stable_id: MarkId(id),
        anchor,
        order: PainterOrder::new(Stratum::Emergent, 0.0, 0, MarkId(id)),
        material: SceneMaterialIndex(0),
        root,
        length_m: 0.2,
        azimuth_rad: 0.0,
        bend_rad: 0.0,
        radius_m: 0.002,
        tip_radius_m: 0.001,
        attributes: MarkAttributes::default(),
        bounds: Aabb3::around(root, reach),
    })
}

fn instance(id: u64, anchor: AnchorIndex, at: ScenePoint) -> PrototypeInstance {
    PrototypeInstance {
        stable_id: MarkId(id),
        anchor,
        order: PainterOrder::new(Stratum::Ground, 0.0, 0, MarkId(id)),
        prototype: PrototypeIndex(0),
        material: SceneMaterialIndex(0),
        position: at,
        yaw_rad: 0.0,
        tilt_rad: 0.0,
        tilt_azimuth_rad: 0.0,
        scale: [1.0, 1.0, 1.0],
        attributes: MarkAttributes::default(),
        bounds: Aabb3::around(at, 0.05),
    }
}

fn prototype(seed: u64, radius_m: f32) -> PrototypeBinding {
    PrototypeBinding {
        recipe: RecipeKey::new("prototype.granite_boulder").expect("valid"),
        seed,
        parameters: ParameterObject::new(),
        radius_m,
    }
}

#[test]
fn index_zero_is_always_a_real_entry() {
    // `AnchorIndex::UNGROUPED` has to be an index into something, or every
    // converted scene and every hand-built mark carries a dangling reference
    // that only the renderer discovers.
    let scene = builder().build();
    assert_eq!(scene.anchors.len(), 1);
    assert!(scene.anchors.first().is_some());
    // And it is not counted as a placement, because nothing was placed.
    assert_eq!(scene.placement_count(), 0);
}

#[test]
fn a_group_bound_contains_every_primitive_it_grew() {
    // The property slice selection rests on: classify the group's bound and you
    // have classified all of its parts. A bound that missed one would let a
    // flower head be dropped while its stem was kept.
    let mut builder = builder();
    let anchor = builder.bind_anchor(anchor_at(0.0, 0.0));
    builder.push_mark(curve(1, anchor, ScenePoint::new(0.0, 0.0, 0.0), 0.05));
    builder.push_mark(curve(2, anchor, ScenePoint::new(0.0, 0.0, 0.30), 0.02));
    builder.push_instance(instance(3, anchor, ScenePoint::new(0.01, 0.0, 0.31)));
    let scene = builder.build();

    let bounds = scene
        .group_bounds(anchor)
        .expect("the group grew something");
    for mark in scene.marks_for_anchor(anchor) {
        assert_eq!(
            bounds.union(mark.bounds()),
            bounds,
            "a mark escapes its group"
        );
    }
    for placed in scene.instances_for_anchor(anchor) {
        assert_eq!(
            bounds.union(placed.bounds),
            bounds,
            "an instance escapes its group"
        );
    }
    // The stem reaches 0.30 m and the head sits above it, so the group is
    // taller than any one primitive.
    assert!(bounds.ceiling_m() > 0.30);
}

#[test]
fn one_pass_group_bounds_agree_with_the_per_group_query() {
    // `all_group_bounds` exists because the per-group query is quadratic over a
    // plate with a hundred thousand placements. Two ways of computing one
    // answer is a place for them to drift, so they are compared rather than
    // trusted.
    let mut builder = builder();
    let mut anchors = Vec::new();
    for i in 0..8 {
        let anchor = builder.bind_anchor(anchor_at(i as f64 * 0.1, 0.0));
        anchors.push(anchor);
        builder.push_mark(curve(
            i as u64 * 2,
            anchor,
            ScenePoint::new(i as f64 * 0.1, 0.0, 0.0),
            0.05,
        ));
        builder.push_instance(instance(
            i as u64 * 2 + 1,
            anchor,
            ScenePoint::new(i as f64 * 0.1, 0.02, 0.0),
        ));
    }
    let scene = builder.build();

    let bulk = scene.all_group_bounds();
    assert_eq!(bulk.len(), anchors.len());
    for (anchor, bounds) in bulk {
        assert_eq!(
            Some(bounds),
            scene.group_bounds(anchor),
            "the bulk and per-group answers differ for {anchor:?}"
        );
    }
}

#[test]
fn binding_the_same_prototype_twice_returns_one_index() {
    // A stone population names its prototype per instance. Without
    // canonicalisation the table would hold one entry per stone, and Blender
    // would build a thousand copies of six meshes.
    let mut builder = builder();
    let first = builder.bind_prototype(prototype(1, 0.4));
    let again = builder.bind_prototype(prototype(1, 0.4));
    let other_seed = builder.bind_prototype(prototype(2, 0.4));
    let other_size = builder.bind_prototype(prototype(1, 0.5));
    assert_eq!(first, again);
    assert_ne!(first, other_seed);
    assert_ne!(
        first, other_size,
        "two shapes under one seed are two prototypes, not one"
    );
    assert_eq!(builder.build().prototypes.len(), 3);
}

#[test]
fn two_candidates_at_one_position_are_two_placements() {
    // Anchors are deliberately *not* deduplicated. Two accepted candidates that
    // happen to land on the same point are two plants, and folding them would
    // make one vanish from every per-group count while both still rendered.
    let mut builder = builder();
    let a = builder.bind_anchor(anchor_at(0.0, 0.0));
    let b = builder.bind_anchor(anchor_at(0.0, 0.0));
    assert_ne!(a, b);
    assert_eq!(builder.build().placement_count(), 2);
}

#[test]
fn grouping_reaches_the_fingerprint() {
    // Two scenes with identical primitives that grouped them differently render
    // the same still and slice differently. A fingerprint that could not tell
    // them apart would call that no change at all.
    let together = {
        let mut builder = builder();
        let anchor = builder.bind_anchor(anchor_at(0.0, 0.0));
        builder.push_mark(curve(1, anchor, ScenePoint::new(0.0, 0.0, 0.0), 0.05));
        builder.push_mark(curve(2, anchor, ScenePoint::new(0.0, 0.0, 0.3), 0.02));
        builder.build()
    };
    let apart = {
        let mut builder = builder();
        let first = builder.bind_anchor(anchor_at(0.0, 0.0));
        let second = builder.bind_anchor(anchor_at(0.0, 0.0));
        builder.push_mark(curve(1, first, ScenePoint::new(0.0, 0.0, 0.0), 0.05));
        builder.push_mark(curve(2, second, ScenePoint::new(0.0, 0.0, 0.3), 0.02));
        builder.build()
    };
    assert_ne!(together.fingerprint(), apart.fingerprint());
}

#[test]
fn prototypes_and_interactions_reach_the_fingerprint() {
    let base = builder().build();
    let reference = base.fingerprint();

    let with_prototype = {
        let mut builder = builder();
        builder.bind_prototype(prototype(1, 0.4));
        builder.build()
    };
    assert_ne!(reference, with_prototype.fingerprint(), "prototypes");

    let with_interaction = {
        let mut builder = builder();
        let anchor = builder.bind_anchor(anchor_at(0.0, 0.0));
        builder.push_interaction(InteractionPrimitive {
            source: MarkId(1),
            anchor,
            centre: WorldPoint::ORIGIN,
            shape: InteractionShape::Ellipse {
                semi_u_m: 0.05,
                semi_v_m: 0.03,
                yaw_rad: 0.4,
            },
            hard_clearance_m: 0.008,
            response_reach_m: 0.11,
            channels: InteractionChannels::ALL_TUNED,
        });
        builder.build()
    };
    assert_ne!(reference, with_interaction.fingerprint(), "interactions");
}

#[test]
fn interactions_are_ordered_by_their_source_rather_than_by_arrival() {
    // The query field built from these must not depend on which recipe emitted
    // first, or two compile windows that traversed their domains in a different
    // order would resolve a tie between two overlapping stones differently.
    let build = |ids: [u64; 3]| {
        let mut builder = builder();
        let anchor = builder.bind_anchor(anchor_at(0.0, 0.0));
        for id in ids {
            builder.push_interaction(InteractionPrimitive {
                source: MarkId(id),
                anchor,
                centre: WorldPoint::ORIGIN,
                shape: InteractionShape::Ellipse {
                    semi_u_m: 0.05,
                    semi_v_m: 0.03,
                    yaw_rad: 0.0,
                },
                hard_clearance_m: 0.0,
                response_reach_m: 0.1,
                channels: InteractionChannels::ALL_TUNED,
            });
        }
        builder.build()
    };
    let forward = build([1, 2, 3]);
    let backward = build([3, 2, 1]);
    assert_eq!(
        forward
            .interactions
            .iter()
            .map(|i| i.source.0)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(forward.fingerprint(), backward.fingerprint());
}

#[test]
fn interaction_channels_are_a_set_rather_than_a_boolean() {
    // Thatch is excluded from a stone's footprint but barely bends around it,
    // so the response has to be addressable per pass.
    let tuft_only = InteractionChannels::TUFT;
    assert!(tuft_only.contains(InteractionChannels::TUFT));
    assert!(!tuft_only.contains(InteractionChannels::THATCH));
    assert!(InteractionChannels::ALL_TUNED.contains(InteractionChannels::THATCH));
    assert!(InteractionChannels::ALL_TUNED.contains(InteractionChannels::BROADLEAF));
    assert!(InteractionChannels::default().is_empty());
    assert_eq!(
        InteractionChannels::TUFT.union(InteractionChannels::FINE),
        InteractionChannels(0b110)
    );
}

#[test]
fn a_group_that_grew_nothing_has_no_bounds() {
    // Rather than an empty box at the origin, which would place a phantom
    // object in every slice that contains the world origin.
    let mut builder = builder();
    let anchor = builder.bind_anchor(anchor_at(1.0, 1.0));
    let scene = builder.build();
    assert_eq!(scene.group_bounds(anchor), None);
    assert!(scene.all_group_bounds().is_empty());
}

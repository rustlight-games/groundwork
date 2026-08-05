//! What obstacles do to the things growing around them.
//!
//! ## Cutting a hole is not the same as growing around something
//!
//! The cheap version of this is a circular exclusion: reject any plant whose
//! root falls inside a stone. It is one line, it is fast, and it looks wrong in
//! a way that is hard to unsee — the stone reads as having been *dropped into a
//! pre-cut lawn*. The surviving grass keeps the height and the direction it
//! would have had if the stone were not there, so the boundary is a hard ring
//! with full-length blades standing to attention right up against it.
//!
//! What actually happens is that a plant near a stone grows *away* from it and
//! stays shorter, because the stone took its light and its root space. That is a
//! smooth, local, bounded response, and it has to happen at placement time: a
//! post-process can cut a hole but it cannot make the surviving plants lean.
//!
//! ## Bounded, and exactly zero outside the bound
//!
//! Beyond `hard_clearance + response_reach` the field returns *nothing* — not a
//! small number. That exactness is the contract that makes stones removable:
//! turning them off has to leave every distant blade bit-identical, or a
//! comparison between two renders can never attribute a difference to anything.
//!
//! ## The nearest boundary decides the direction, and it is not a sum
//!
//! A root between two stones has two outward directions, and adding them is the
//! obvious move and the wrong one: two roughly opposite stones cancel to nothing
//! and the plant between them leans nowhere, which is exactly where the
//! response should be strongest. The nearest boundary wins the direction; the
//! strongest influence wins the magnitude.

use std::collections::BTreeMap;

use glam::Vec2;
use terrain_core::coords::{CellCoord, CellGrid, WorldPoint};
use terrain_scene::mark::MarkId;
use terrain_scene::scene::{InteractionChannels, InteractionPrimitive, InteractionShape};

use crate::tuned::TunedPass;

/// The version the interaction response stamps on itself.
pub const INTERACTION_FIELD_VERSION: u32 = 1;

/// What the field says at one point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionSample {
    /// Inside an obstacle's hard footprint: nothing roots here.
    pub blocked: bool,
    /// How strongly the nearest obstacle acts, `0..1`.
    ///
    /// One at and inside the hard boundary, zero at and beyond the outer one,
    /// with zero first derivative at both ends so no kink appears where the
    /// response turns on or off.
    pub influence: f32,
    /// The unit direction *away* from the nearest obstacle.
    pub away: Vec2,
    /// Signed distance to the nearest obstacle's boundary, metres.
    pub clearance_m: f32,
    pub source: Option<MarkId>,
}

impl InteractionSample {
    /// Nothing near enough to matter.
    ///
    /// The exact value the field returns outside every reach, so a comparison
    /// against a scene with no obstacles is an equality rather than a
    /// tolerance.
    pub const NONE: Self = Self {
        blocked: false,
        influence: 0.0,
        away: Vec2::X,
        clearance_m: f32::MAX,
        source: None,
    };

    pub fn is_nothing(&self) -> bool {
        !self.blocked && self.influence == 0.0
    }
}

/// How a tuned pass answers an obstacle.
///
/// Starting values rather than claimed biological constants. Broadleaf reacts
/// most because a broad leaf competes for light directly; thatch barely bends
/// because a mat lies over whatever is there, though it is still kept out of the
/// footprint itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObstacleResponse {
    /// Whether a root inside the hard footprint is refused outright.
    pub hard_exclusion: bool,
    /// How far the growth direction is pulled toward "away", `0..1`.
    pub direction_strength: f32,
    /// The largest fraction of its length a plant loses.
    pub shortening: f32,
    /// Extra bend, radians, at full influence.
    pub extra_bend_rad: f32,
    /// A little extra clearance at the root, metres.
    ///
    /// ## Why this is small, and why it is not the clump radius
    ///
    /// The response is queried once per *placement*, and a placement is not
    /// always one mark: a tuned tuft spreads its blades up to
    /// [`crate::placement::TUFT_RADIUS`] — eighteen centimetres — around its
    /// anchor. Widening the hard exclusion by that would carve an
    /// eighteen-centimetre bald ring around a seven-centimetre stone, which is
    /// precisely the "dropped into a pre-cut lawn" artefact this whole module
    /// exists to avoid. The cure would be far worse than the symptom.
    ///
    /// So the exclusion is on the *root*, and a clump rooted clear of a stone
    /// may lay a blade across it. That is what real grass does — it grows up
    /// beside a stone and leans over it — and it is why the response also
    /// shortens and turns the plant rather than only removing it.
    ///
    /// What this covers is the tight bundle at the root itself, so a blade does
    /// not appear to sprout from the stone's own edge.
    pub root_spread_m: f32,
}

impl ObstacleResponse {
    pub fn for_pass(pass: TunedPass) -> Self {
        match pass {
            TunedPass::Tuft => Self {
                hard_exclusion: true,
                // The tiller bundle at the anchor, not the clump's reach.
                root_spread_m: 0.012,
                direction_strength: 0.75,
                shortening: 0.25,
                extra_bend_rad: 0.35,
            },
            TunedPass::Fine => Self {
                hard_exclusion: true,
                // One blade, near enough.
                root_spread_m: 0.004,
                direction_strength: 0.55,
                shortening: 0.18,
                extra_bend_rad: 0.25,
            },
            TunedPass::Broadleaf => Self {
                hard_exclusion: true,
                // A rosette's crown, not the leaves that radiate from it.
                root_spread_m: 0.010,
                direction_strength: 0.80,
                shortening: 0.30,
                extra_bend_rad: 0.30,
            },
            // Kept out of the footprint and barely bent: a mat lies over
            // whatever is there rather than growing around it.
            TunedPass::Thatch => Self {
                hard_exclusion: true,
                // Short and laid over, so barely wider than its own root.
                root_spread_m: 0.012,
                direction_strength: 0.10,
                shortening: 0.15,
                extra_bend_rad: 0.05,
            },
        }
    }

    /// Which channel a pass reads.
    pub fn channel(pass: TunedPass) -> InteractionChannels {
        match pass {
            TunedPass::Thatch => InteractionChannels::THATCH,
            TunedPass::Fine => InteractionChannels::FINE,
            TunedPass::Tuft => InteractionChannels::TUFT,
            TunedPass::Broadleaf => InteractionChannels::BROADLEAF,
        }
    }
}

/// A deterministic query structure over accepted obstacles.
///
/// It generates nothing. Every primitive in it was placed by a recipe that has
/// already been through acceptance and ownership, so the field is a *view* of
/// decisions already made — which is what lets it be queried at every
/// prospective grass root without changing any of them.
#[derive(Debug, Default)]
pub struct InteractionField {
    bucket_m: f64,
    buckets: BTreeMap<CellCoord, Vec<u32>>,
    primitives: Vec<InteractionPrimitive>,
}

impl InteractionField {
    /// An empty field: no obstacles, and every query returns exactly nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build from a compiled scene's interaction primitives.
    pub fn from_primitives(primitives: Vec<InteractionPrimitive>) -> Self {
        if primitives.is_empty() {
            return Self::default();
        }
        // Bucketed at the largest total reach, so a query inspects its own
        // bucket and its eight neighbours and cannot miss an obstacle whose
        // influence extends into it.
        // The bucket has to be at least as wide as the furthest a query can be
        // from an obstacle and still see it. With a first-order clearance that
        // is near-Euclidean, so the major axis plus the response band is a real
        // bound rather than an optimistic one.
        let bucket_m = primitives
            .iter()
            .map(|p| reach_of(p) as f64)
            .fold(0.0f64, f64::max)
            .max(0.05);

        let grid = CellGrid::new(bucket_m);
        let mut buckets: BTreeMap<CellCoord, Vec<u32>> = BTreeMap::new();
        for (index, primitive) in primitives.iter().enumerate() {
            // Inserted into every bucket its influence can touch, so a query
            // only ever looks at one bucket's list.
            let reach = reach_of(primitive) as f64;
            let low = grid.cell_at(WorldPoint::new(
                primitive.centre.u_m - reach,
                primitive.centre.v_m - reach,
            ));
            let high = grid.cell_at(WorldPoint::new(
                primitive.centre.u_m + reach,
                primitive.centre.v_m + reach,
            ));
            for y in low.y..=high.y {
                for x in low.x..=high.x {
                    buckets
                        .entry(CellCoord::new(x, y))
                        .or_default()
                        .push(index as u32);
                }
            }
        }
        // Sorted by the causing mark, so query order cannot change tie
        // handling. `MarkId` is derived from the candidate address, so this is
        // stable across windows.
        for list in buckets.values_mut() {
            list.sort_by_key(|index| primitives[*index as usize].source);
        }
        Self {
            bucket_m,
            buckets,
            primitives,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }

    pub fn len(&self) -> usize {
        self.primitives.len()
    }

    /// What the field says at a point, for one pass.
    pub fn sample(&self, at: Vec2, pass: TunedPass) -> InteractionSample {
        if self.primitives.is_empty() {
            return InteractionSample::NONE;
        }
        let grid = CellGrid::new(self.bucket_m);
        let cell = grid.cell_at(WorldPoint::new(at.x as f64, at.y as f64));
        let Some(indices) = self.buckets.get(&cell) else {
            return InteractionSample::NONE;
        };

        let channel = ObstacleResponse::channel(pass);
        let mut best: Option<InteractionSample> = None;
        for index in indices {
            let obstacle = &self.primitives[*index as usize];
            if !obstacle.channels.contains(channel) {
                continue;
            }
            let (clearance, away) = ellipse_clearance(obstacle, at);
            let beyond = (clearance - obstacle.hard_clearance_m).max(0.0);
            let reach = obstacle.response_reach_m.max(1.0e-6);
            if beyond >= reach {
                continue;
            }
            let influence = 1.0 - smoothstep(0.0, reach, beyond);
            let sample = InteractionSample {
                blocked: clearance <= obstacle.hard_clearance_m,
                influence,
                away,
                clearance_m: clearance,
                source: Some(obstacle.source),
            };
            // Nearest boundary wins the direction; the mark id breaks an exact
            // tie so two equidistant stones resolve the same way from any
            // traversal.
            best = Some(match best {
                None => sample,
                Some(current)
                    if clearance < current.clearance_m
                        || (clearance == current.clearance_m && sample.source < current.source) =>
                {
                    InteractionSample {
                        // The strongest influence, even when a different
                        // obstacle supplied the direction: a root between two
                        // stones is squeezed by both.
                        influence: influence.max(current.influence),
                        blocked: sample.blocked || current.blocked,
                        ..sample
                    }
                }
                Some(current) => InteractionSample {
                    influence: current.influence.max(influence),
                    blocked: current.blocked || sample.blocked,
                    ..current
                },
            });
        }
        best.unwrap_or(InteractionSample::NONE)
    }
}

/// How far from an obstacle's centre a query can still be influenced by it.
///
/// The major axis plus the whole response band. A genuine bound, because the
/// index is built from it: an obstacle inserted into too few buckets is one a
/// query near its edge never finds, and the symptom is a handful of blades
/// growing through a stone in a field of thousands that do not.
fn reach_of(primitive: &InteractionPrimitive) -> f32 {
    major_axis(primitive) + primitive.hard_clearance_m + primitive.response_reach_m
}

/// The largest semi-axis of an obstacle's footprint.
fn major_axis(primitive: &InteractionPrimitive) -> f32 {
    match primitive.shape {
        InteractionShape::Ellipse {
            semi_u_m, semi_v_m, ..
        } => semi_u_m.max(semi_v_m),
        // A shape this build does not know: treat it as reaching nothing rather
        // than as reaching everywhere, so an unrecognised obstacle is invisible
        // instead of blocking the whole plate.
        _ => 0.0,
    }
}

/// Approximate signed clearance to an ellipse, and the outward unit normal.
///
/// ## First-order, not axis-scaled
///
/// The normalised elliptical radius `ρ = √((qx/a)² + (qy/b)²)` is one on the
/// boundary, so `(ρ − 1)` is a *dimensionless* measure of how far outside a
/// point is. Turning it into metres needs a scale, and the obvious choice —
/// multiply by the minor axis — is only correct along the minor axis. Along the
/// major axis of an elongated footprint it understates the distance by the
/// aspect ratio, so an eleven-by-three-centimetre stone influenced grass more
/// than half a metre away off its ends while reaching four centimetres off its
/// sides. Nothing looked obviously wrong; a brute-force oracle found it.
///
/// The first-order estimate `(ρ − 1)/‖∇ρ‖` carries the local scale and is
/// accurate *near the boundary* in every direction. Far from an eccentric
/// footprint it degrades, and the same oracle caught that too: a point a
/// quarter of a metre from an eleven-by-three-centimetre stone read as twelve
/// centimetres away, which is inside the response band and outside anything the
/// spatial index could be sized for.
///
/// So the estimate is floored by `‖q‖ − max(a, b)`, the circle that contains
/// the footprint. That is a genuine lower bound on the true distance, which
/// makes the index bound provable: influence is nonzero only when
/// `‖q‖ < max(a, b) + hard + reach`, and that is exactly what
/// [`reach_of`] returns. Near the boundary the first-order term is the larger
/// of the two and the ellipse keeps its shape; far away the circle takes over,
/// where the shape no longer matters anyway.
///
/// Exact Euclidean distance to an ellipse still needs an iterative solve, and at
/// the centimetre tolerances involved it buys nothing this does not have.
fn ellipse_clearance(primitive: &InteractionPrimitive, at: Vec2) -> (f32, Vec2) {
    let InteractionShape::Ellipse {
        semi_u_m,
        semi_v_m,
        yaw_rad,
    } = primitive.shape
    else {
        return (f32::MAX, Vec2::X);
    };
    let a = semi_u_m.max(1.0e-5);
    let b = semi_v_m.max(1.0e-5);
    let (sin, cos) = yaw_rad.sin_cos();
    let offset = Vec2::new(
        at.x - primitive.centre.u_m as f32,
        at.y - primitive.centre.v_m as f32,
    );
    // Into the ellipse's own frame.
    let local = Vec2::new(
        offset.x * cos + offset.y * sin,
        -offset.x * sin + offset.y * cos,
    );
    let rho = ((local.x / a).powi(2) + (local.y / b).powi(2)).sqrt();
    // The outward gradient in local space, rotated back.
    let gradient = Vec2::new(local.x / (a * a), local.y / (b * b));
    // `‖∇ρ‖ = ‖gradient‖ / ρ`, so `(ρ − 1)/‖∇ρ‖` is `(ρ − 1)·ρ/‖gradient‖`. At
    // the centre both are zero; the fallback is the minor axis, which is the
    // right order for a point inside the footprint.
    let slope = gradient.length();
    let first_order = if slope > 1.0e-9 {
        (rho - 1.0) * rho / slope
    } else {
        -a.min(b)
    };
    // Floored by the containing circle. See the note above: this is what makes
    // the spatial index's reach bound provable rather than optimistic.
    let clearance = first_order.max(local.length() - a.max(b));

    let world = Vec2::new(
        gradient.x * cos - gradient.y * sin,
        gradient.x * sin + gradient.y * cos,
    );
    // At the exact centre the gradient is zero. An addressed fallback rather
    // than a zero direction, because a plant told to lean nowhere leans the way
    // it already was and the exclusion reads as a hole again.
    let away = world.normalize_or(fallback_direction(primitive.source));
    (clearance, away)
}

/// A stable direction for a query exactly at an obstacle's centre.
fn fallback_direction(source: MarkId) -> Vec2 {
    let angle = (source.0 as f32) * 0.618_034 * std::f32::consts::TAU;
    Vec2::new(angle.cos(), angle.sin())
}

fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    if (high - low).abs() < 1.0e-9 {
        return if x >= high { 1.0 } else { 0.0 };
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_scene::mark::AnchorIndex;

    fn stone(at: (f64, f64), a: f32, b: f32, yaw: f32, id: u64) -> InteractionPrimitive {
        InteractionPrimitive {
            source: MarkId(id),
            anchor: AnchorIndex::UNGROUPED,
            centre: WorldPoint::new(at.0, at.1),
            shape: InteractionShape::Ellipse {
                semi_u_m: a,
                semi_v_m: b,
                yaw_rad: yaw,
            },
            hard_clearance_m: 0.008,
            response_reach_m: 0.12,
            channels: InteractionChannels::ALL_TUNED,
        }
    }

    #[test]
    fn an_empty_field_returns_exactly_nothing() {
        // Exactly, not approximately. Turning stones off has to leave every
        // distant blade bit-identical or a comparison between two renders can
        // never attribute a difference to anything.
        let field = InteractionField::empty();
        let sample = field.sample(Vec2::ZERO, TunedPass::Tuft);
        assert_eq!(sample, InteractionSample::NONE);
        assert!(sample.is_nothing());
    }

    #[test]
    fn beyond_the_reach_the_field_is_exactly_nothing() {
        let field = InteractionField::from_primitives(vec![stone((0.0, 0.0), 0.06, 0.04, 0.0, 1)]);
        // Well past hard clearance plus reach plus the major axis.
        let far = field.sample(Vec2::new(3.0, 3.0), TunedPass::Tuft);
        assert!(far.is_nothing(), "{far:?}");
        assert_eq!(far.influence, 0.0);
    }

    #[test]
    fn influence_is_one_inside_and_zero_at_the_outer_boundary() {
        let field = InteractionField::from_primitives(vec![stone((0.0, 0.0), 0.06, 0.06, 0.0, 1)]);
        let inside = field.sample(Vec2::new(0.01, 0.0), TunedPass::Tuft);
        assert!(inside.blocked);
        assert!((inside.influence - 1.0).abs() < 1.0e-6);

        // The outer boundary: radius + hard clearance + reach.
        let outer = 0.06 + 0.008 + 0.12;
        let at = field.sample(Vec2::new(outer + 0.001, 0.0), TunedPass::Tuft);
        assert!(at.is_nothing(), "{at:?}");
    }

    #[test]
    fn influence_falls_smoothly_and_flattens_at_both_ends() {
        // A kink where the response turns on or off is visible as a ring, which
        // is the artefact the smoothstep exists to avoid.
        let field = InteractionField::from_primitives(vec![stone((0.0, 0.0), 0.06, 0.06, 0.0, 1)]);
        let at = |d: f32| field.sample(Vec2::new(d, 0.0), TunedPass::Tuft).influence;

        let mut previous = 1.0;
        for step in 0..40 {
            let d = 0.06 + step as f32 * 0.004;
            let value = at(d);
            assert!(value <= previous + 1.0e-6, "influence rose at {d}");
            previous = value;
        }
        // Flat at the near end: the derivative is zero at the hard boundary.
        let near = at(0.068) - at(0.070);
        let middle = at(0.120) - at(0.122);
        assert!(
            near < middle,
            "the response is steeper at the hard boundary than in the middle"
        );
    }

    #[test]
    fn the_outward_direction_points_away_from_the_stone() {
        let field = InteractionField::from_primitives(vec![stone((0.0, 0.0), 0.06, 0.06, 0.0, 1)]);
        for (dx, dy) in [(1.0f32, 0.0f32), (0.0, 1.0), (-0.7, -0.7), (0.5, -0.9)] {
            let direction = Vec2::new(dx, dy).normalize();
            let probe = direction * 0.09;
            let sample = field.sample(probe, TunedPass::Tuft);
            assert!(
                sample.away.dot(direction) > 0.95,
                "at {probe:?} the away direction was {:?}",
                sample.away
            );
        }
    }

    #[test]
    fn rotating_an_ellipse_rotates_its_normal() {
        // The frame transform, checked rather than assumed. A yaw applied in the
        // wrong direction gives a plant that leans across the stone instead of
        // away from it, which looks deliberate and is not.
        let turned = InteractionField::from_primitives(vec![stone(
            (0.0, 0.0),
            0.12,
            0.03,
            std::f32::consts::FRAC_PI_2,
            1,
        )]);
        // Turned a quarter turn, the long axis runs along `v`. A probe along `u`
        // is therefore near the *short* axis and its normal points along `u`.
        let sample = turned.sample(Vec2::new(0.05, 0.0), TunedPass::Tuft);
        assert!(sample.away.x > 0.95, "{:?}", sample.away);
    }

    #[test]
    fn a_query_at_the_exact_centre_gets_a_stable_direction_rather_than_zero() {
        // A plant told to lean nowhere leans the way it already was, and the
        // exclusion reads as a hole again.
        let field = InteractionField::from_primitives(vec![stone((0.0, 0.0), 0.06, 0.04, 0.3, 7)]);
        let sample = field.sample(Vec2::ZERO, TunedPass::Tuft);
        assert!(sample.blocked);
        assert!(
            (sample.away.length() - 1.0).abs() < 1.0e-5,
            "{:?}",
            sample.away
        );
    }

    #[test]
    fn the_nearest_boundary_supplies_the_direction_rather_than_a_sum() {
        // Two roughly opposite stones cancel to nothing if the directions are
        // added, and the plant between them leans nowhere — which is exactly
        // where the response should be strongest.
        let field = InteractionField::from_primitives(vec![
            stone((-0.10, 0.0), 0.05, 0.05, 0.0, 1),
            stone((0.14, 0.0), 0.05, 0.05, 0.0, 2),
        ]);
        let between = field.sample(Vec2::ZERO, TunedPass::Tuft);
        assert!(between.influence > 0.0);
        assert!(
            between.away.length() > 0.9,
            "the directions cancelled: {:?}",
            between.away
        );
        // The nearer stone is the one on the left, so the lean is to the right.
        assert!(between.away.x > 0.5, "{:?}", between.away);
    }

    #[test]
    fn a_pass_an_obstacle_does_not_affect_sees_nothing() {
        let mut only_tufts = stone((0.0, 0.0), 0.06, 0.06, 0.0, 1);
        only_tufts.channels = InteractionChannels::TUFT;
        let field = InteractionField::from_primitives(vec![only_tufts]);
        assert!(
            field
                .sample(Vec2::new(0.07, 0.0), TunedPass::Tuft)
                .influence
                > 0.0
        );
        assert!(
            field
                .sample(Vec2::new(0.07, 0.0), TunedPass::Fine)
                .is_nothing()
        );
    }

    #[test]
    fn the_bucket_query_finds_every_obstacle_a_brute_force_scan_would() {
        // The one bug in a bucketed field that produces a *nearly* correct
        // result: an obstacle just outside the queried bucket whose influence
        // reaches into it.
        let primitives: Vec<_> = (0..40)
            .map(|i| {
                let angle = i as f32 * 0.7;
                stone(
                    ((angle.cos() * 0.8) as f64, (angle.sin() * 0.8) as f64),
                    0.03 + (i % 5) as f32 * 0.02,
                    0.03,
                    angle,
                    i as u64,
                )
            })
            .collect();
        let field = InteractionField::from_primitives(primitives.clone());

        for step in 0..400 {
            let at = Vec2::new(
                -1.2 + (step % 20) as f32 * 0.12,
                -1.2 + (step / 20) as f32 * 0.12,
            );
            let bucketed = field.sample(at, TunedPass::Tuft);
            // The same rule, over every primitive.
            let mut brute = InteractionSample::NONE;
            for primitive in &primitives {
                let (clearance, away) = ellipse_clearance(primitive, at);
                let beyond = (clearance - primitive.hard_clearance_m).max(0.0);
                if beyond >= primitive.response_reach_m {
                    continue;
                }
                let influence = 1.0 - smoothstep(0.0, primitive.response_reach_m, beyond);
                if brute.source.is_none() || clearance < brute.clearance_m {
                    brute = InteractionSample {
                        blocked: brute.blocked || clearance <= primitive.hard_clearance_m,
                        influence: influence.max(brute.influence),
                        away,
                        clearance_m: clearance,
                        source: Some(primitive.source),
                    };
                } else {
                    brute.influence = brute.influence.max(influence);
                    brute.blocked = brute.blocked || clearance <= primitive.hard_clearance_m;
                }
            }
            assert_eq!(
                bucketed.blocked, brute.blocked,
                "at {at:?} the bucketed and brute-force answers disagree about blocking"
            );
            assert!(
                (bucketed.influence - brute.influence).abs() < 1.0e-5,
                "at {at:?}: bucketed {} against brute {}",
                bucketed.influence,
                brute.influence
            );
        }
    }

    #[test]
    fn every_pass_has_a_response_and_thatch_bends_least() {
        for pass in TunedPass::ALL {
            let response = ObstacleResponse::for_pass(pass);
            assert!(response.hard_exclusion, "{pass} lets roots inside a stone");
            assert!((0.0..=1.0).contains(&response.direction_strength));
            assert!((0.0..=1.0).contains(&response.shortening));
        }
        // A mat lies over whatever is there.
        assert!(
            ObstacleResponse::for_pass(TunedPass::Thatch).direction_strength
                < ObstacleResponse::for_pass(TunedPass::Tuft).direction_strength
        );
    }
}

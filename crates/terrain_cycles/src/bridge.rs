//! The compiled scene, lowered into geometry Cycles can trace.
//!
//! ## The connection the framework was missing
//!
//! `compile_scene` has always produced a `TerrainScene` full of flowers and
//! stones, and the CLI has always used it for two things: a fingerprint in the
//! progress report, and a mark count. Every accepted flower was computed,
//! owned, grown into marks, counted — and discarded. This is the arrow that was
//! not there.
//!
//! ## Selection is per plant, never per primitive
//!
//! A flower is a stem and a head. A trace slice that classified them
//! independently would keep one and drop the other, and half a flower is worse
//! than no flower: it reads as a rendering bug rather than as sparse content.
//! So the selector works on [`PlacementAnchor`] groups — a group's bound is the
//! union of everything it grew, and the whole group is kept, made halo-only, or
//! omitted together.
//!
//! ## Tuned grass never comes through here
//!
//! The compiled scene contains generic grass, fine grass and thatch as well.
//! They are `Tuned` populations and the compiler never emitted them, but that is
//! the compiler's guarantee rather than this module's — so this module checks
//! too. A second, lower-quality canopy drawn over the tuned one is the one
//! failure the whole meadow tier is arranged to prevent, and it would read as
//! "more detail" to anyone who did not know what to look for.

use std::collections::BTreeMap;

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_scene::mark::{AnchorIndex, SceneMark};
use terrain_scene::scene::TerrainScene;

use crate::secondary::{
    CurveSpan, Instance, MaterialBinding, Prototype, PrototypeFamily, SecondaryGeometry, Visibility,
};

/// How many points a bent stem's centreline is sampled at.
///
/// Chosen from the bend rather than fixed: a straight stem needs two points and
/// a hard-bent one needs enough that the bevel does not facet. See
/// [`stem_points`].
const MAX_STEM_SEGMENTS: usize = 12;

/// The appearance keys this bridge knows how to lower, and what they become.
///
/// Explicit rather than pattern-matched on the mark type, because the *same*
/// primitive means different things: an analytic mark is a flower head under
/// one appearance and a stone under another, and they need different
/// prototypes. A key this table does not know is reported rather than guessed.
fn lowering(appearance: &str) -> Option<Lowering> {
    Some(match appearance {
        "flower.stem" => Lowering::Curve,
        "flower.head" => Lowering::Disk,
        "flower.petal" => Lowering::Petal,
        "rock.granite" => Lowering::Stone,
        "soil.clod" | "soil.grit" => Lowering::Stone,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lowering {
    /// A bevelled centreline: a stem.
    Curve,
    /// A shallow disk instance: a flower head.
    Disk,
    /// A flattened lozenge instance: one petal.
    ///
    /// A superellipsoid rather than a swept ribbon, and the choice is a
    /// deliberate trade at this framing: a petal is three or four pixels
    /// across, so what it needs is a *silhouette* — a separate blade with gaps
    /// either side, catching the sun at its own angle — rather than a
    /// correctly cupped surface nobody can resolve. A flattened superellipsoid
    /// with squared shoulders gives that for one instance and no new
    /// tessellation path.
    Petal,
    /// A superellipsoid instance: a stone or a fragment.
    Stone,
}

/// What one lowering pass produced, and what it could not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BridgeReport {
    pub groups_camera: usize,
    pub groups_halo: usize,
    pub groups_omitted: usize,
    pub curves: usize,
    pub instances: usize,
    /// Appearance keys with no lowering, and how many marks named each.
    ///
    /// Reported rather than skipped. A flower that silently did not render
    /// looks exactly like a flower that was never placed, and the difference
    /// matters a great deal when the question is "why is my document empty".
    pub unsupported: BTreeMap<String, usize>,
}

impl BridgeReport {
    pub fn total_groups(&self) -> usize {
        self.groups_camera + self.groups_halo
    }
}

/// Lower every placement group that can reach one slice.
///
/// `visible` is the ground the slice photographs; `shadow_reach_m` is how far
/// outside it a caster can still darken it. A group outside both is omitted.
pub fn lower(
    scene: &TerrainScene,
    visible: WorldRect,
    shadow_reach_m: f64,
) -> (SecondaryGeometry, BridgeReport) {
    let mut out = SecondaryGeometry::default();
    let mut report = BridgeReport::default();

    // Materials first, so every span has an index to name. Bound for the
    // appearances actually in the scene rather than for a fixed list, so a
    // document that grows no flowers ships no flower shader.
    let mut material_of: BTreeMap<String, u16> = BTreeMap::new();
    for binding in &scene.materials {
        let key = binding.appearance.as_str();
        let Some(kind) = lowering(key) else { continue };
        let shader = match kind {
            Lowering::Curve => "plant.flower_stem",
            Lowering::Disk => "plant.flower_disk",
            Lowering::Petal => "plant.flower_petal",
            Lowering::Stone => "surface.stone",
        };
        material_of.entry(key.to_string()).or_insert_with(|| {
            out.materials.push(MaterialBinding {
                appearance: shader.to_string(),
                shader: shader.to_string(),
            });
            (out.materials.len() - 1) as u16
        });
    }

    let caster = visible.expanded(shadow_reach_m);
    let mut prototypes: BTreeMap<String, u32> = BTreeMap::new();

    for (anchor, bounds) in scene.all_group_bounds() {
        if anchor == AnchorIndex::UNGROUPED {
            continue;
        }
        let footprint = WorldRect::new(
            WorldPoint::new(bounds.min.u_m, bounds.min.v_m),
            WorldPoint::new(bounds.max.u_m, bounds.max.v_m),
        );
        let visibility = if overlaps(footprint, visible) {
            report.groups_camera += 1;
            Visibility::Camera
        } else if overlaps(footprint, caster) {
            report.groups_halo += 1;
            Visibility::Halo
        } else {
            report.groups_omitted += 1;
            continue;
        };

        for mark in scene.marks_for_anchor(anchor) {
            let appearance = scene
                .materials
                .get(mark.material().0 as usize)
                .map(|binding| binding.appearance.as_str().to_string())
                .unwrap_or_default();
            let Some(kind) = lowering(&appearance) else {
                *report.unsupported.entry(appearance).or_default() += 1;
                continue;
            };
            let material = material_of.get(&appearance).copied().unwrap_or(0);
            match (kind, mark) {
                (Lowering::Curve, SceneMark::Curve(curve)) => {
                    let offset = out.curve_points.len() as u32;
                    let points = stem_points(curve);
                    let count = points.len() as u32;
                    out.curve_points.extend(points);
                    out.curves.push(CurveSpan {
                        point_offset: offset,
                        point_count: count,
                        radius_root_m: curve.radius_m,
                        radius_tip_m: curve.tip_radius_m,
                        material,
                        visibility,
                    });
                    report.curves += 1;
                }
                (Lowering::Disk, SceneMark::Analytic(head)) => {
                    let prototype = bind(
                        &mut out,
                        &mut prototypes,
                        "flower.disk.v1",
                        PrototypeFamily::Disk,
                        material,
                    );
                    out.instances.push(Instance {
                        prototype,
                        material_variant: 0,
                        visibility,
                        translation: [
                            head.centre.u_m as f32,
                            head.centre.v_m as f32,
                            head.centre.z_m as f32,
                        ],
                        rotation_xyzw: yaw_quaternion(head.rotation_rad),
                        scale: [
                            head.radius_m[0],
                            head.radius_m[1],
                            head.height_m.max(1.0e-4),
                        ],
                        tint: tint_from(head.attributes.tint),
                        variation: head.attributes.variation,
                    });
                    report.instances += 1;
                }
                (Lowering::Petal, SceneMark::Analytic(petal)) => {
                    let prototype = bind(
                        &mut out,
                        &mut prototypes,
                        "flower.petal.v1",
                        PrototypeFamily::Petal,
                        material,
                    );
                    out.instances.push(Instance {
                        prototype,
                        material_variant: 0,
                        visibility,
                        translation: [
                            petal.centre.u_m as f32,
                            petal.centre.v_m as f32,
                            petal.centre.z_m as f32,
                        ],
                        rotation_xyzw: yaw_quaternion(petal.rotation_rad),
                        scale: [
                            petal.radius_m[0].max(1.0e-4),
                            petal.radius_m[1].max(1.0e-4),
                            petal.height_m.max(1.0e-5),
                        ],
                        tint: petal_tint(petal.attributes.tint),
                        variation: petal.attributes.variation,
                    });
                    report.instances += 1;
                }
                (Lowering::Stone, SceneMark::Analytic(stone)) => {
                    let prototype = bind(
                        &mut out,
                        &mut prototypes,
                        "stone.rounded.v1",
                        PrototypeFamily::Superellipsoid,
                        material,
                    );
                    out.instances.push(Instance {
                        prototype,
                        material_variant: 0,
                        visibility,
                        translation: [
                            stone.centre.u_m as f32,
                            stone.centre.v_m as f32,
                            stone.centre.z_m as f32,
                        ],
                        rotation_xyzw: yaw_quaternion(stone.rotation_rad),
                        scale: [
                            stone.radius_m[0].max(1.0e-4),
                            stone.radius_m[1].max(1.0e-4),
                            stone.height_m.max(1.0e-4),
                        ],
                        tint: tint_from(stone.attributes.tint),
                        variation: stone.attributes.variation,
                    });
                    report.instances += 1;
                }
                // A ribbon under a flower or stone appearance, or a stamp:
                // reported rather than dropped. Silently omitting a petal is
                // the failure this whole phase exists to remove.
                (_, other) => {
                    *report
                        .unsupported
                        .entry(format!("{appearance} as {}", other.kind_name()))
                        .or_default() += 1;
                }
            }
        }
    }

    (out, report)
}

/// Whether two world rectangles share any ground.
fn overlaps(a: WorldRect, b: WorldRect) -> bool {
    a.min.u_m < b.max.u_m && a.max.u_m > b.min.u_m && a.min.v_m < b.max.v_m && a.max.v_m > b.min.v_m
}

/// Register a prototype once and return its index.
fn bind(
    out: &mut SecondaryGeometry,
    seen: &mut BTreeMap<String, u32>,
    key: &str,
    family: PrototypeFamily,
    material: u16,
) -> u32 {
    if let Some(index) = seen.get(key) {
        return *index;
    }
    let prototype = match family {
        PrototypeFamily::Petal => Prototype {
            key: key.to_string(),
            family: PrototypeFamily::Superellipsoid,
            semi_axes_m: [1.0, 1.0, 1.0],
            // Squared shoulders and a blunt tip. An ellipsoid petal tapers to a
            // point at both ends and reads as a grain of rice; a meadow flower's
            // petal is broad for most of its length and rounded at the end.
            exponents: [0.55, 0.7],
            deformation: Vec::new(),
            clips: Vec::new(),
            // Coarse on purpose: a petal is a few pixels across and there are
            // six of them per flower and hundreds of flowers per plate.
            tessellation: [5, 8],
            material,
            unit_height_m: 1.0,
        },
        PrototypeFamily::Disk => Prototype {
            key: key.to_string(),
            family,
            // A unit prototype: the instance scale carries the real size, so
            // one datablock serves every flower in the scene.
            semi_axes_m: [1.0, 1.0, 1.0],
            exponents: [1.0, 1.0],
            deformation: Vec::new(),
            clips: Vec::new(),
            tessellation: [4, 12],
            material,
            unit_height_m: 1.0,
        },
        _ => Prototype {
            key: key.to_string(),
            family: PrototypeFamily::Superellipsoid,
            semi_axes_m: [1.0, 1.0, 1.0],
            // Slightly under one: squarer shoulders than an ellipsoid, which is
            // what stops a field of these reading as a field of balls.
            exponents: [0.85, 0.85],
            // One low-order term. High-frequency displacement turns a small
            // stone into a noisy potato at this scale, and the silhouette is
            // what makes a stone recognisable.
            deformation: vec![[0.07, 3.0, 0.6]],
            clips: Vec::new(),
            tessellation: [10, 16],
            material,
            unit_height_m: 1.0,
        },
    };
    let index = out.prototypes.len() as u32;
    out.prototypes.push(prototype);
    seen.insert(key.to_string(), index);
    index
}

/// A quaternion for a turn about the vertical axis.
fn yaw_quaternion(yaw_rad: f32) -> [f32; 4] {
    let half = yaw_rad * 0.5;
    [0.0, 0.0, half.sin(), half.cos()]
}

/// A petal's tint: warmer and paler than a stone's.
///
/// Separate from `tint_from` because they are doing different jobs. A stone's
/// tint stops a field of clones; a petal's carries the *species* variation a
/// meadow has — white, cream, and the occasional yellow — so it moves further
/// and in a different direction.
fn petal_tint(value: f32) -> [f32; 3] {
    let t = value.clamp(-1.0, 1.0);
    [
        1.0,
        (1.0 - 0.10 * t.max(0.0)).max(0.6),
        (1.0 - 0.45 * t.max(0.0)).max(0.25),
    ]
}

/// A bounded multiplicative tint from a `-1..1` attribute.
///
/// Multiplicative in linear light, and bounded well inside a factor of two:
/// a per-instance tint is meant to stop a field of stones reading as clones,
/// not to make one of them a different rock.
fn tint_from(value: f32) -> [f32; 3] {
    let t = value.clamp(-1.0, 1.0);
    [1.0 + 0.12 * t, 1.0 + 0.06 * t, (1.0 - 0.05 * t).max(0.5)]
}

/// The centreline of a bent stem, as world points.
///
/// ## The exact arc, not an approximation of one
///
/// A stem begins vertical and bends by a total angle `θ` toward a horizontal
/// direction. For arc length `L` and normalised position `s`:
///
/// ```text
/// horizontal(s) = L/θ · (1 − cos(θs))
/// vertical(s)   = L/θ · sin(θs)
/// ```
///
/// which is a circular arc of curvature `θ/L`. The closed form matters because
/// the arc has to have the length it claims: a stem tessellated by stepping a
/// fixed vertical rise and leaning it over comes out shorter than `L`, and a
/// field of them reads as uniformly stunted.
///
/// At `θ → 0` the divided form is `0/0`. The straight case is handled
/// explicitly rather than evaluated at a tiny angle and hoped to cancel.
fn stem_points(curve: &terrain_scene::mark::CurveMark) -> Vec<[f32; 3]> {
    let root = [
        curve.root.u_m as f32,
        curve.root.v_m as f32,
        curve.root.z_m as f32,
    ];
    let length = curve.length_m.max(1.0e-4);
    let bend = curve.bend_rad;
    let (sin_a, cos_a) = curve.azimuth_rad.sin_cos();

    if bend.abs() < 1.0e-3 {
        return vec![root, [root[0], root[1], root[2] + length]];
    }

    // Enough segments that no one of them turns more than about ten degrees,
    // which is where a bevelled curve stops faceting visibly at this scale.
    let segments = ((bend.abs() / 0.175).ceil() as usize).clamp(2, MAX_STEM_SEGMENTS);
    let radius = length / bend;
    (0..=segments)
        .map(|step| {
            let s = step as f32 / segments as f32;
            let angle = bend * s;
            let horizontal = radius * (1.0 - angle.cos());
            let vertical = radius * angle.sin();
            [
                root[0] + horizontal * cos_a,
                root[1] + horizontal * sin_a,
                root[2] + vertical,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::digest::Fingerprint;
    use terrain_core::seed::{CandidateId, PopulationHash};
    use terrain_scene::mark::{
        Aabb3, AnalyticMark, CurveMark, MarkAttributes, MarkId, PainterOrder, SceneMaterialBinding,
        Stratum,
    };
    use terrain_scene::projection::ScenePoint;
    use terrain_scene::scene::{PlacementAnchor, SceneBuilder, SceneRequest};

    fn candidate(rank: u16) -> CandidateId {
        CandidateId::new(
            PopulationHash::from_bits(0xabcd),
            terrain_core::coords::CellCoord::new(0, 0),
            rank,
        )
    }

    fn rect(min: (f64, f64), max: (f64, f64)) -> WorldRect {
        WorldRect::new(WorldPoint::new(min.0, min.1), WorldPoint::new(max.0, max.1))
    }

    /// A scene with one flower at `at` and one stone beside it.
    fn scene_with_flower(at: (f64, f64)) -> TerrainScene {
        let mut builder = SceneBuilder::new(
            SceneRequest::square(WorldPoint::ORIGIN, 4.0, 96.0),
            Fingerprint::from_u128(1),
            1,
        );
        let stem_material = builder.bind_material(SceneMaterialBinding {
            appearance: terrain_core::ids::AppearanceKey::new("flower.stem").expect("valid"),
            terrain_material: None,
        });
        let head_material = builder.bind_material(SceneMaterialBinding {
            appearance: terrain_core::ids::AppearanceKey::new("flower.head").expect("valid"),
            terrain_material: None,
        });

        let anchor = builder.bind_anchor(PlacementAnchor {
            candidate: candidate(0),
            root: ScenePoint::new(at.0, at.1, 0.0),
        });
        let root = ScenePoint::new(at.0, at.1, 0.0);
        builder.push_mark(SceneMark::Curve(CurveMark {
            stable_id: MarkId(1),
            anchor,
            order: PainterOrder::new(Stratum::Emergent, 0.0, 0, MarkId(1)),
            material: stem_material,
            root,
            length_m: 0.25,
            azimuth_rad: 0.6,
            bend_rad: 0.4,
            radius_m: 0.0016,
            tip_radius_m: 0.0011,
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(root, 0.3),
        }));
        let head = ScenePoint::new(at.0 + 0.05, at.1, 0.24);
        builder.push_mark(SceneMark::Analytic(AnalyticMark {
            stable_id: MarkId(2),
            anchor,
            order: PainterOrder::new(Stratum::Emergent, 0.0, 0, MarkId(2)),
            material: head_material,
            centre: head,
            radius_m: [0.013, 0.013],
            height_m: 0.006,
            rotation_rad: 0.2,
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(head, 0.015),
        }));
        builder.build()
    }

    #[test]
    fn a_flower_becomes_a_curve_and_an_instance() {
        // The connection this module exists to make. Before it, every one of
        // these was computed, counted, fingerprinted and thrown away.
        let scene = scene_with_flower((0.0, 0.0));
        let (geometry, report) = lower(&scene, rect((-2.0, -2.0), (2.0, 2.0)), 0.5);
        assert_eq!(report.curves, 1);
        assert_eq!(report.instances, 1);
        assert_eq!(report.groups_camera, 1);
        assert!(report.unsupported.is_empty(), "{:?}", report.unsupported);
        assert!(geometry.problems().is_empty(), "{:?}", geometry.problems());
    }

    #[test]
    fn a_group_is_kept_or_dropped_whole() {
        // Half a flower reads as a rendering bug rather than as sparse content,
        // so a slice classifies plants and never primitives.
        let scene = scene_with_flower((0.0, 0.0));
        let (geometry, report) = lower(&scene, rect((-2.0, -2.0), (2.0, 2.0)), 0.5);
        // One group, and both of its primitives came through with the same
        // visibility.
        assert_eq!(report.total_groups(), 1);
        assert_eq!(geometry.curves[0].visibility, Visibility::Camera);
        assert_eq!(geometry.instances[0].visibility, Visibility::Camera);
    }

    #[test]
    fn a_plant_outside_the_frame_but_inside_the_shadow_reach_is_halo() {
        // Dropping it instead takes its shadow with it and leaves a bright rim
        // exactly at the edge of the picture, which is where the eye goes.
        let scene = scene_with_flower((3.0, 0.0));
        let (geometry, report) = lower(&scene, rect((-2.0, -2.0), (2.0, 2.0)), 2.0);
        assert_eq!(report.groups_halo, 1);
        assert_eq!(report.groups_camera, 0);
        assert!(
            geometry
                .curves
                .iter()
                .all(|c| c.visibility == Visibility::Halo)
        );
        assert!(
            geometry
                .instances
                .iter()
                .all(|i| i.visibility == Visibility::Halo)
        );
    }

    #[test]
    fn a_plant_beyond_every_reach_is_omitted() {
        let scene = scene_with_flower((40.0, 0.0));
        let (geometry, report) = lower(&scene, rect((-2.0, -2.0), (2.0, 2.0)), 2.0);
        assert_eq!(report.groups_omitted, 1);
        assert!(geometry.is_empty());
    }

    #[test]
    fn one_prototype_serves_every_instance_of_its_kind() {
        // A thousand flowers must not be a thousand Blender datablocks. The
        // shape is a unit prototype and the instance scale carries the size.
        let mut builder = SceneBuilder::new(
            SceneRequest::square(WorldPoint::ORIGIN, 4.0, 96.0),
            Fingerprint::from_u128(1),
            1,
        );
        let material = builder.bind_material(SceneMaterialBinding {
            appearance: terrain_core::ids::AppearanceKey::new("rock.granite").expect("valid"),
            terrain_material: None,
        });
        for rank in 0..8u16 {
            let at = ScenePoint::new(rank as f64 * 0.2 - 0.8, 0.0, 0.0);
            let anchor = builder.bind_anchor(PlacementAnchor {
                candidate: candidate(rank),
                root: at,
            });
            builder.push_mark(SceneMark::Analytic(AnalyticMark {
                stable_id: MarkId(rank as u64),
                anchor,
                order: PainterOrder::new(Stratum::Ground, 0.0, 0, MarkId(rank as u64)),
                material,
                centre: at,
                radius_m: [0.05, 0.04],
                height_m: 0.03,
                rotation_rad: rank as f32,
                attributes: MarkAttributes::default(),
                bounds: Aabb3::around(at, 0.06),
            }));
        }
        let (geometry, report) = lower(&builder.build(), rect((-2.0, -2.0), (2.0, 2.0)), 0.5);
        assert_eq!(report.instances, 8);
        assert_eq!(geometry.prototypes.len(), 1, "eight stones, one shape");
        assert!(geometry.problems().is_empty(), "{:?}", geometry.problems());
    }

    #[test]
    fn an_appearance_with_no_lowering_is_reported_rather_than_dropped() {
        // A flower that silently did not render looks exactly like a flower
        // that was never placed, and the difference matters a great deal when
        // the question is "why is my document empty".
        let mut builder = SceneBuilder::new(
            SceneRequest::square(WorldPoint::ORIGIN, 4.0, 96.0),
            Fingerprint::from_u128(1),
            1,
        );
        let material = builder.bind_material(SceneMaterialBinding {
            appearance: terrain_core::ids::AppearanceKey::new("plant.grass_blade").expect("valid"),
            terrain_material: None,
        });
        let at = ScenePoint::new(0.0, 0.0, 0.0);
        let anchor = builder.bind_anchor(PlacementAnchor {
            candidate: candidate(0),
            root: at,
        });
        builder.push_mark(SceneMark::Analytic(AnalyticMark {
            stable_id: MarkId(1),
            anchor,
            order: PainterOrder::new(Stratum::Ground, 0.0, 0, MarkId(1)),
            material,
            centre: at,
            radius_m: [0.01, 0.01],
            height_m: 0.01,
            rotation_rad: 0.0,
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(at, 0.02),
        }));
        let (geometry, report) = lower(&builder.build(), rect((-2.0, -2.0), (2.0, 2.0)), 0.5);
        assert!(geometry.is_empty());
        assert_eq!(report.unsupported.get("plant.grass_blade"), Some(&1));
    }

    #[test]
    fn a_straight_stem_is_two_points_and_stands_its_own_length() {
        let mut scene = scene_with_flower((0.0, 0.0));
        let SceneMark::Curve(curve) = &mut scene.marks[0] else {
            panic!("the first mark is the stem")
        };
        curve.bend_rad = 0.0;
        let points = stem_points(curve);
        assert_eq!(points.len(), 2);
        assert!((points[1][2] - points[0][2] - curve.length_m).abs() < 1.0e-6);
    }

    #[test]
    fn a_bent_stem_keeps_the_arc_length_it_declares() {
        // The reason the closed form is used rather than a stepped
        // approximation: a stem tessellated by stepping a fixed rise and
        // leaning it over comes out shorter than `L`, and a field of them reads
        // as uniformly stunted.
        let scene = scene_with_flower((0.0, 0.0));
        let SceneMark::Curve(curve) = &scene.marks[0] else {
            panic!("the first mark is the stem")
        };
        let points = stem_points(curve);
        let walked: f32 = points
            .windows(2)
            .map(|pair| {
                let d = [
                    pair[1][0] - pair[0][0],
                    pair[1][1] - pair[0][1],
                    pair[1][2] - pair[0][2],
                ];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .sum();
        // A polyline through an arc is slightly shorter than the arc, by an
        // amount that falls with the segment count. Within a percent.
        assert!(
            (walked - curve.length_m).abs() < curve.length_m * 0.01,
            "walked {walked} against a declared {}",
            curve.length_m
        );
    }

    #[test]
    fn a_bent_stem_leans_the_way_it_was_told_to() {
        let scene = scene_with_flower((0.0, 0.0));
        let SceneMark::Curve(curve) = &scene.marks[0] else {
            panic!("the first mark is the stem")
        };
        let points = stem_points(curve);
        let tip = points.last().expect("a stem has a tip");
        let (sin_a, cos_a) = curve.azimuth_rad.sin_cos();
        let horizontal = ((tip[0] - points[0][0]).powi(2) + (tip[1] - points[0][1]).powi(2)).sqrt();
        assert!(horizontal > 0.0, "a bent stem did not lean at all");
        // And it leaned along its own azimuth rather than somewhere else.
        assert!((tip[0] - points[0][0] - horizontal * cos_a).abs() < 1.0e-5);
        assert!((tip[1] - points[0][1] - horizontal * sin_a).abs() < 1.0e-5);
    }

    #[test]
    fn a_yaw_quaternion_is_unit_length() {
        // A quaternion that is not unit length scales the object as well as
        // turning it, which reads as a stone of the wrong size.
        for turn in 0..16 {
            let q = yaw_quaternion(turn as f32 * 0.4);
            let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((length - 1.0).abs() < 1.0e-6, "{q:?}");
        }
    }

    #[test]
    fn a_tint_stays_bounded_however_extreme_the_attribute() {
        // A per-instance tint stops a field of stones reading as clones; it is
        // not meant to make one of them a different rock.
        for value in [-4.0f32, -1.0, 0.0, 1.0, 4.0] {
            let tint = tint_from(value);
            assert!(
                tint.iter().all(|c| (0.5..=1.5).contains(c)),
                "{value} gave {tint:?}"
            );
        }
        assert_eq!(tint_from(0.0), [1.0, 1.0, 1.0]);
    }
}

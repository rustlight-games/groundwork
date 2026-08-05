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
//! ## Everything crosses the mirror on the way out
//!
//! Blender is given a *reflected* world: the two ground axes are swapped, which
//! is what turns this framework's left-handed isometric convention into the
//! right-handed space a path tracer wants. Every blade the tuned exporter writes
//! goes through [`cycles::to_blender`], and so must every flower, stone and leaf
//! that leaves this module.
//!
//! This module did not, for its whole first life, and the symptom is worth
//! recording because it is the failure mode a reflection always has: nothing
//! looked broken. A reflection across `x = y` maps a meadow onto a meadow, so a
//! plate of scattered flowers came out looking exactly like a plate of scattered
//! flowers — until a document had a *track* in it, and then half the daisies
//! stood on bare compacted earth while an equal area of grass beside them held
//! none. Two rounds of tuning went into the suppression bands that were supposed
//! to prevent that, and the placement had been correct the entire time.
//!
//! So the rule is the one `terrain_cycles`'s own module note states: nothing
//! reaches Blender in game-world coordinates. It is applied here at the three
//! points where geometry is written — instance translations, curve points and
//! ribbon vertices — rather than at the call sites that build them, because a
//! builder that forgets is exactly what happened.
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
    CurveSpan, Instance, MaterialBinding, Prototype, PrototypeFamily, RibbonSpan, RibbonVertex,
    SecondaryGeometry, Visibility,
};

/// One scene point, reflected into the space Blender is given.
///
/// The whole of the handedness fix, and the only conversion in this module. See
/// the module note for what its absence looked like.
fn to_blender(point: terrain_scene::projection::ScenePoint) -> [f32; 3] {
    let swapped = crate::cycles::to_blender(glam::Vec3::new(
        point.u_m as f32,
        point.v_m as f32,
        point.z_m as f32,
    ));
    [swapped.x, swapped.y, swapped.z]
}

/// A raw position, already in world units, reflected the same way.
fn swap_xy(position: [f32; 3]) -> [f32; 3] {
    [position[1], position[0], position[2]]
}

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
        // A ground leaf is a swept ribbon: it arches out of the crown, rolls
        // past the horizontal and folds along a midrib, and none of those three
        // survive an instanced lozenge. See `Lowering::Leaf`.
        "plant.undergrowth_leaf" => Lowering::Leaf,
        "rock.rounded" => Lowering::Stone,
        "rock.fractured" => Lowering::Stone,
        "rock.flat" => Lowering::Stone,
        "rock.elongated" => Lowering::Stone,
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
    /// A tessellated ribbon: one broad ground leaf.
    ///
    /// The one lowering here that is not an instance, and the reason is that a
    /// ground leaf's shape is the point. It arches — leaving the crown steeply,
    /// rolling past the horizontal, dropping its tip back toward the soil — and
    /// it is folded along a midrib so the two halves take light differently.
    ///
    /// The first version instanced a flattened superellipsoid with a yaw, which
    /// gave every leaf in a plate the same pitch and the same normal. Both of
    /// the things above were missing and what rendered was a scatter of green
    /// stains rather than a plant. An instance transform cannot supply an arch:
    /// a rigid body has one orientation, and the leaf needs a different one at
    /// every point along its length.
    Leaf,
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
    pub ribbons: usize,
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
            Lowering::Leaf => "plant.undergrowth_leaf",
            Lowering::Stone => {
                // Broken soil is not granite. A clod is the ground it came out
                // of, so it takes an earth shader at earth's albedo — the stone
                // shader sits near a fifth against soil's four hundredths, and
                // grit lowered through it rendered as a scatter of white eggs.
                if key.starts_with("soil.") {
                    "surface.soil_fragment"
                } else {
                    "surface.stone"
                }
            }
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
        // Camera visibility is decided by the plant's *root*, not by its bound.
        //
        // The bound is a box around everything the plant grew, so a flower
        // rooted just outside the rendered ground still overlaps it — it leans
        // in — and the bound test called it camera-visible. What that produced
        // was a ring of flowers standing on nothing beyond the edge of the
        // plate, because the ground mesh ends exactly at the layout and its
        // edge *is* the picture's silhouette.
        //
        // The tuned blades have always used the root for the same decision, so
        // this is also what makes a flower and the grass beside it agree about
        // which of them is in the picture.
        //
        // The bound still decides *halo* membership: a plant whose geometry can
        // reach the frame casts into it even when its root cannot.
        let root = scene
            .anchors
            .get(anchor.index())
            .map(|placement| WorldPoint::new(placement.root.u_m, placement.root.v_m));
        let rooted_inside = root.is_some_and(|at| visible.contains(at));
        let visibility = if rooted_inside {
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
                        translation: to_blender(head.centre),
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
                (Lowering::Leaf, SceneMark::Ribbon(leaf)) => {
                    let vertex_offset = out.ribbon_vertices.len() as u32;
                    let index_offset = out.ribbon_indices.len() as u32;
                    tessellate_leaf(leaf, &mut out.ribbon_vertices, &mut out.ribbon_indices);
                    out.ribbons.push(RibbonSpan {
                        vertex_offset,
                        vertex_count: out.ribbon_vertices.len() as u32 - vertex_offset,
                        index_offset,
                        index_count: out.ribbon_indices.len() as u32 - index_offset,
                        material,
                        visibility,
                    });
                    report.ribbons += 1;
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
                        translation: to_blender(petal.centre),
                        rotation_xyzw: yaw_quaternion(petal.rotation_rad),
                        scale: [
                            petal.radius_m[0].max(1.0e-4),
                            petal.radius_m[1].max(1.0e-4),
                            petal.height_m.max(1.0e-5),
                        ],
                        tint: petal_tint(petal.attributes.tint, petal.attributes.variation),
                        variation: petal.attributes.variation,
                    });
                    report.instances += 1;
                }
                (Lowering::Stone, SceneMark::Analytic(stone)) => {
                    // One prototype per named silhouette. The appearance key is
                    // what the recipe chose, so the shape a stone gets is a
                    // decision Rust made and recorded rather than one Blender
                    // reached for.
                    let prototype = bind(
                        &mut out,
                        &mut prototypes,
                        &format!("{}.v1", appearance.replace('.', "_")),
                        PrototypeFamily::Superellipsoid,
                        material,
                    );
                    out.instances.push(Instance {
                        prototype,
                        material_variant: 0,
                        visibility,
                        translation: to_blender(stone.centre),
                        rotation_xyzw: yaw_quaternion(stone.rotation_rad),
                        scale: [
                            stone.radius_m[0].max(1.0e-4),
                            stone.radius_m[1].max(1.0e-4),
                            stone.height_m.max(1.0e-4),
                        ],
                        tint: if appearance.starts_with("soil.") {
                            soil_fragment_tint(stone.attributes.tint, stone.attributes.moisture)
                        } else {
                            tint_from(stone.attributes.tint)
                        },
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
        _ => {
            // The exponents *are* the silhouette. Barr's superquadrics span
            // rounded, blocky, flattened and pinched from two numbers, which is
            // why a handful of prototypes can carry a field of stones without
            // repeating visibly — and why a unique mesh per instance would be
            // paying for detail nobody can resolve at five centimetres.
            let (exponents, deformation, clips) = if key.starts_with("rock_fractured") {
                (
                    // Squarer, so the shoulders read as broken faces rather
                    // than as a weathered curve.
                    [0.60, 0.60],
                    vec![[0.04, 2.0, 1.1]],
                    // Two deterministic cuts. Part of the binding, so part of
                    // the prototype's fingerprint.
                    vec![[0.62, 0.34, 0.71, 0.74], [-0.48, 0.79, 0.38, 0.80]],
                )
            } else if key.starts_with("rock_flat") {
                ([0.80, 0.72], vec![[0.05, 2.0, 0.2]], Vec::new())
            } else if key.starts_with("rock_elongated") {
                ([0.78, 0.70], vec![[0.06, 2.0, 1.7]], Vec::new())
            } else {
                // Rounded: slightly under one, so the shoulders are fuller than
                // an ellipsoid's. A field of true ellipsoids reads as a field
                // of balls.
                ([0.88, 0.88], vec![[0.07, 3.0, 0.6]], Vec::new())
            };
            Prototype {
                key: key.to_string(),
                family: PrototypeFamily::Superellipsoid,
                semi_axes_m: [1.0, 1.0, 1.0],
                exponents,
                // Low-order only. High-frequency displacement turns a small
                // stone into a noisy potato at this scale, and the silhouette
                // is what makes a stone recognisable.
                deformation,
                clips,
                tessellation: [10, 16],
                material,
                unit_height_m: 1.0,
            }
        }
    };
    let index = out.prototypes.len() as u32;
    out.prototypes.push(prototype);
    seen.insert(key.to_string(), index);
    index
}

/// Cross-sections along one tessellated leaf.
///
/// Nine, and the number comes from the arch rather than from a budget. The
/// centreline turns through more than two radians between the crown and the tip,
/// so eight segments put about fifteen degrees between consecutive tangents —
/// close enough that the silhouette of a twelve-centimetre leaf reads as a curve
/// at any framing this project renders at, and few enough that a plate of two
/// hundred rosettes is a hundred thousand triangles rather than a million.
const LEAF_SECTIONS: usize = 9;

/// Points across one cross-section.
///
/// Five rather than three, because three cannot describe a fold: the midrib
/// would be a single crease with flat faces either side, and the highlight along
/// it would be a hard line. Five gives the fold two facets a side, which is
/// enough for the specular roll-off that makes it read as a curved surface.
const LEAF_ACROSS: usize = 5;

/// Turn one arching, folded ground leaf into triangles.
///
/// ## The frame, and why it is built from the azimuth rather than parallel
/// transported
///
/// A ribbon needs a coordinate frame at every point: a tangent along it and an
/// across direction to lay the width on. The general answer is a parallel
/// transported frame, which is what a swept tube wants — but a leaf is not a
/// tube. Its "across" is the direction it is *wide* in, and for a plant leaving
/// the ground on a fixed azimuth that direction is the horizontal perpendicular
/// to the azimuth, all the way along. Building it from the azimuth directly is
/// therefore both simpler and more correct: it is exactly perpendicular to the
/// tangent at every point (the tangent has no component along it, by
/// construction), and it never accumulates the drift a transported frame does.
///
/// The twist is then applied *on top* of that frame rather than being the frame,
/// which is what makes it an authored parameter with a meaning rather than an
/// artefact of the integration.
///
/// ## Why the centreline is integrated rather than evaluated
///
/// A constant-curvature arc has a closed form and the stems use it. This one
/// does not: the bend accelerates through the curl in the last third and the
/// azimuth drifts under the sway, so the tangent is a function of `s` with no
/// useful antiderivative. Eight forward steps at this segment count cost
/// nothing and are exact at the vertices, which is where the geometry is.
fn tessellate_leaf(
    leaf: &terrain_scene::mark::RibbonMark,
    vertices: &mut Vec<RibbonVertex>,
    indices: &mut Vec<u32>,
) {
    let geometry = &leaf.geometry;
    let tint = leaf_tint(leaf.attributes.tint);
    let variation = leaf.attributes.variation;
    let base = vertices.len() as u32;

    let mut position = [
        leaf.root.u_m as f32,
        leaf.root.v_m as f32,
        leaf.root.z_m as f32,
    ];

    for section in 0..LEAF_SECTIONS {
        let s = section as f32 / (LEAF_SECTIONS - 1) as f32;

        // The tangent's angle away from vertical. The curl is squared over the
        // last third so it arrives as an acceleration rather than as a step —
        // a discontinuity in curvature is visible as a crease across the leaf.
        let curl_at = ((s - 2.0 / 3.0) * 3.0).max(0.0);
        let bend = geometry.bend_rad * s + geometry.curl_rad * curl_at * curl_at;
        let azimuth = geometry.azimuth_rad + geometry.sway_rad * s;

        let (sin_bend, cos_bend) = bend.sin_cos();
        let (sin_az, cos_az) = azimuth.sin_cos();
        let tangent = [sin_bend * cos_az, sin_bend * sin_az, cos_bend];

        if section > 0 {
            // A midpoint would be more accurate; at fifteen degrees a step it
            // is not worth the second trigonometric evaluation. What matters is
            // that consecutive sections are joined by the tangent they share, so
            // the surface has no gaps.
            let step = geometry.length_m / (LEAF_SECTIONS - 1) as f32;
            for axis in 0..3 {
                position[axis] += tangent[axis] * step;
            }
        }

        // Across, before the twist: the horizontal perpendicular to the
        // azimuth. Exactly perpendicular to the tangent for every bend.
        let flat = [-sin_az, cos_az, 0.0];
        // And the face normal, completing the frame.
        let face = cross(tangent, flat);

        let (sin_twist, cos_twist) = (geometry.twist_rad * s).sin_cos();
        let across_dir = [
            flat[0] * cos_twist + face[0] * sin_twist,
            flat[1] * cos_twist + face[1] * sin_twist,
            flat[2] * cos_twist + face[2] * sin_twist,
        ];
        let face_dir = [
            face[0] * cos_twist - flat[0] * sin_twist,
            face[1] * cos_twist - flat[1] * sin_twist,
            face[2] * cos_twist - flat[2] * sin_twist,
        ];

        let half = (geometry.width_m * width_shape(geometry.profile, s)).max(geometry.tip_width_m);
        let ridge = geometry.ridge;

        for column in 0..LEAF_ACROSS {
            let t = column as f32 / (LEAF_ACROSS - 1) as f32 * 2.0 - 1.0;
            // The fold: a parabola standing proud at the midrib and meeting the
            // plane at both edges.
            let lift = ridge * half * (1.0 - t * t);
            let point = [
                position[0] + across_dir[0] * half * t + face_dir[0] * lift,
                position[1] + across_dir[1] * half * t + face_dir[1] * lift,
                position[2] + across_dir[2] * half * t + face_dir[2] * lift,
            ];

            // The surface normal, in closed form. With `N = T × S` the cross
            // products collapse: `T × ∂P/∂t = N + 2·ridge·t·S`, so no numerical
            // differencing is needed and the normal is exact at every vertex
            // including the edges, where a difference would be one-sided.
            let normal = normalise([
                face_dir[0] + 2.0 * ridge * t * across_dir[0],
                face_dir[1] + 2.0 * ridge * t * across_dir[1],
                face_dir[2] + 2.0 * ridge * t * across_dir[2],
            ]);

            vertices.push(RibbonVertex {
                // Across the mirror with everything else. The normal too: a
                // reflected surface with an unreflected normal is lit from the
                // wrong side, which on a translucent leaf is the difference
                // between glowing and being in shadow.
                position: swap_xy(point),
                normal: swap_xy(normal),
                along: s,
                across: t,
                tint,
                variation,
            });
        }
    }

    for section in 0..LEAF_SECTIONS - 1 {
        for column in 0..LEAF_ACROSS - 1 {
            let a = base + (section * LEAF_ACROSS + column) as u32;
            let b = a + 1;
            let c = a + LEAF_ACROSS as u32;
            let d = c + 1;
            // Wound the opposite way round, because the mirror flips
            // handedness. A reflected triangle with its original winding faces
            // backwards, and Cycles' geometric normal — which is what decides
            // which side of a thin sheet is the front — would point into the
            // ground.
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
}

/// A ribbon's half-width at `s`, as a fraction of its root half-width.
fn width_shape(profile: terrain_scene::mark::WidthProfile, s: f32) -> f32 {
    use terrain_scene::mark::WidthProfile;
    match profile {
        WidthProfile::Stem => 1.0,
        WidthProfile::Tapered => (1.0 - s).max(0.0),
        WidthProfile::Oval => (std::f32::consts::PI * s).sin(),
        // Narrow where it attaches, broadest a third of the way out, then a
        // long taper to a quick point. The shape of an actual ground leaf, and
        // the reason the variant is named `Leaf`.
        WidthProfile::Leaf => {
            const PEAK: f32 = 0.33;
            if s < PEAK {
                0.40 + 0.60 * (s / PEAK)
            } else {
                let u = (s - PEAK) / (1.0 - PEAK);
                (1.0 - u * u).max(0.0).powf(0.55)
            }
        }
        // A profile this build does not know narrows like a blade rather than
        // vanishing. Silently emitting a zero-width ribbon would look like the
        // leaf was never placed.
        _ => (1.0 - s).max(0.0),
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 1.0e-12 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// A quaternion for a turn about the vertical axis, reflected.
///
/// The bearing crosses the mirror with the position. Swapping the ground axes
/// turns an angle measured from `+u` toward `+v` into its complement, so an
/// instance placed correctly and rotated in the unreflected frame would sit in
/// the right spot facing the wrong way — which on a stone is a different stone
/// and on a petal whorl is a flower that no longer matches its own stem.
fn yaw_quaternion(yaw_rad: f32) -> [f32; 4] {
    let half = crate::cycles::bearing_to_blender(yaw_rad) * 0.5;
    [0.0, 0.0, half.sin(), half.cos()]
}

/// A soil fragment's tint: earth, and as wet as the ground it lies on.
///
/// A fragment is the ground it broke off, so it takes the ground's state as
/// well as its colour. Without the wet term a saturated hollow came out flecked
/// with light dry specks — every fragment drawn at its dry reflectance on mud
/// that had darkened around it, which reads as gravel scattered on mud rather
/// than as mud that has lumps in it.
///
/// The same square-free ratio the ground itself uses, at the same strength as a
/// mid stop: see `GroundMaterialProfile::wet_ratio`. It is applied here rather
/// than in the shader because an instance carries a colour and not a moisture,
/// and adding a per-instance state channel to carry one number is a wider change
/// than this earns.
fn soil_fragment_tint(value: f32, moisture: f32) -> [f32; 3] {
    let base = tint_from(value);
    // A third at full saturation, which is where a soil's own `wet_mid` sits
    // against its dry mid across the shipped set.
    let wet = 1.0 - 0.62 * moisture.clamp(0.0, 1.0);
    [base[0] * wet, base[1] * wet, base[2] * wet]
}

/// A ground leaf's tint: green, varying in how warm.
///
/// Not a hue wheel. A leaf is a leaf; what differs between plants of the same
/// sward is whether they are a yellow-green or a blue-green, which is a narrow
/// band and reads as species variation rather than as a paint chart.
fn leaf_tint(value: f32) -> [f32; 3] {
    let t = value.clamp(-1.0, 1.0);
    [
        (1.0 + 0.20 * t).clamp(0.6, 1.4),
        1.0,
        (1.0 - 0.22 * t).clamp(0.5, 1.4),
    ]
}

/// A petal's colour, from the hue and saturation the document authored.
///
/// ## The two channels mean something specific here
///
/// A `MarkAttributes` carries one scalar tint and a colour needs two numbers,
/// so for `flower.petal` — and only for it — `tint` is the hue mapped into
/// `-1..1` and `variation` is the saturation. `MeadowFlowers::emit` is the other
/// half of that agreement and says so in the same words.
///
/// Full saturation would be a poster rather than a meadow: even a buttercup is
/// mostly white light with a strong cast, so the saturation is applied as a
/// *pull away from white* rather than as an HSV value. That also means a
/// document that says nothing gets near-white petals, which is what a daisy is.
fn petal_tint(hue_attribute: f32, saturation: f32) -> [f32; 3] {
    let hue = (hue_attribute.clamp(-1.0, 1.0) + 1.0) * 0.5;
    let saturation = saturation.clamp(0.0, 1.0);
    // A fully saturated wheel, then blended toward white by the saturation.
    //
    // The standard HSV-to-RGB piecewise form. Written out rather than expressed
    // as one rotated function, because the rotated version is easy to get wrong
    // in a way that is hard to see: the first attempt put red's peak at hue
    // one-half instead of at zero, so a document asking for buttercup yellow got
    // white and nothing in the pipeline disagreed.
    let t = hue.rem_euclid(1.0) * 6.0;
    let pure = [
        ((t - 3.0).abs() - 1.0).clamp(0.0, 1.0),
        (2.0 - (t - 2.0).abs()).clamp(0.0, 1.0),
        (2.0 - (t - 4.0).abs()).clamp(0.0, 1.0),
    ];
    [
        1.0 + (pure[0] - 1.0) * saturation,
        1.0 + (pure[1] - 1.0) * saturation,
        1.0 + (pure[2] - 1.0) * saturation,
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
        return vec![swap_xy(root), swap_xy([root[0], root[1], root[2] + length])];
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
            swap_xy([
                root[0] + horizontal * cos_a,
                root[1] + horizontal * sin_a,
                root[2] + vertical,
            ])
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
        // Against the *reflected* bearing, because `stem_points` hands back the
        // centreline already across the mirror. Checking it against the raw
        // azimuth would be checking that the swap did not happen.
        let (sin_a, cos_a) = crate::cycles::bearing_to_blender(curve.azimuth_rad).sin_cos();
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
    fn the_hue_wheel_puts_each_primary_where_it_belongs() {
        // The first version of this put red's peak at hue one-half instead of
        // at zero, so a document asking for buttercup yellow got white and
        // nothing in the pipeline disagreed with it.
        let at = |hue: f32| petal_tint(hue * 2.0 - 1.0, 1.0);
        let red = at(0.0);
        assert!(red[0] > 0.99 && red[1] < 0.01 && red[2] < 0.01, "{red:?}");
        let green = at(1.0 / 3.0);
        assert!(
            green[1] > 0.99 && green[0] < 0.01 && green[2] < 0.01,
            "{green:?}"
        );
        let blue = at(2.0 / 3.0);
        assert!(
            blue[2] > 0.99 && blue[0] < 0.01 && blue[1] < 0.01,
            "{blue:?}"
        );
        // And the hue the shipped buttercups ask for is a warm yellow: red and
        // green high, blue absent.
        let buttercup = at(0.145);
        assert!(
            buttercup[0] > 0.8 && buttercup[1] > 0.8 && buttercup[2] < 0.1,
            "{buttercup:?}"
        );
    }

    #[test]
    fn zero_saturation_is_white_whatever_the_hue() {
        // What a document that says nothing about colour gets, and what a daisy
        // is. Full saturation would be a poster rather than a meadow.
        for hue in [0.0f32, 0.2, 0.5, 0.9] {
            let tint = petal_tint(hue * 2.0 - 1.0, 0.0);
            assert_eq!(tint, [1.0, 1.0, 1.0], "hue {hue}");
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

    #[test]
    fn every_lowered_position_crosses_the_mirror() {
        // ## The bug this exists because of
        //
        // Blender is given a reflected world — the two ground axes are swapped
        // — and this module wrote game-world coordinates straight through for
        // its whole first life. Nothing looked broken, because a reflection
        // across `x = y` maps a meadow onto a meadow: a plate of scattered
        // flowers came out as a plate of scattered flowers.
        //
        // It only became visible when a document had a *track* in it, and then
        // half the daisies stood on bare compacted earth while an equal area of
        // grass beside them held none. Two rounds of tuning went into the
        // suppression bands meant to stop that, and the placement had been
        // right the whole time.
        //
        // So the assertion is on an *asymmetric* point. A symmetric one — the
        // origin, or anything on `u = v` — is a fixed point of the reflection
        // and would have passed before the fix as happily as after it.
        let at = (1.25, -0.5);
        let scene = scene_with_flower(at);
        let (geometry, report) = lower(&scene, rect((-2.0, -2.0), (2.0, 2.0)), 0.2);
        assert_eq!(report.groups_camera, 1);

        // The stem's first point is its root, reflected.
        let stem = geometry
            .curve_points
            .first()
            .copied()
            .expect("the stem lowered");
        assert!(
            (stem[0] - at.1 as f32).abs() < 1.0e-5 && (stem[1] - at.0 as f32).abs() < 1.0e-5,
            "the stem root is at {stem:?}, not the swap of {at:?}"
        );

        // And the head instance, which sits five centimetres along `+u` from
        // the root — so after the swap it is five centimetres along `+y`.
        let head = geometry.instances.first().expect("the head lowered");
        assert!(
            (head.translation[0] - at.1 as f32).abs() < 1.0e-5
                && (head.translation[1] - (at.0 as f32 + 0.05)).abs() < 1.0e-5,
            "the head is at {:?}, not the swap of its own centre",
            head.translation
        );
    }

    #[test]
    fn a_yaw_crosses_the_mirror_with_its_position() {
        // A reflection turns a bearing into its complement. An instance placed
        // correctly and rotated in the unreflected frame sits in the right spot
        // facing the wrong way, which on a stone is a different stone and on a
        // petal whorl is a flower that no longer matches its own stem.
        for yaw in [0.0f32, 0.4, 1.2, -0.9] {
            let q = yaw_quaternion(yaw);
            let recovered = 2.0 * q[2].atan2(q[3]);
            let expected = std::f32::consts::FRAC_PI_2 - yaw;
            let gap = (recovered - expected).rem_euclid(std::f32::consts::TAU);
            let gap = gap.min(std::f32::consts::TAU - gap);
            assert!(
                gap < 1.0e-5,
                "yaw {yaw} became {recovered}, wanted {expected}"
            );
        }
    }
}

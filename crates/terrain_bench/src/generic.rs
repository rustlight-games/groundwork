//! The grass scene, expressed in the generic scene IR.
//!
//! ## What this is for, and what it is not
//!
//! This is a **bridge**, and it is temporary by design. The grass generator
//! still produces `Vec<Stroke>` in cache-pixel units; the framework's scene
//! speaks ribbons in world metres. This maps one onto the other, so that the
//! generic IR can be exercised against a real meadow of ten thousand marks
//! before the generator itself is moved into `terrain_generators::grass`.
//!
//! Which order those two things happen in matters more than it looks. Moving
//! twenty thousand lines of generator *and* changing what it emits, in one
//! commit, gives a fingerprint that has moved for two reasons at once and no way
//! to tell which. Building the bridge first means the IR is proven to carry the
//! meadow before anything is moved, and the move afterwards is a pure relocation
//! whose fingerprint must not change at all.
//!
//! ## The one conversion that is not mechanical
//!
//! A `Stroke` states its widths in **cache pixels against a 96-pixel metre** —
//! the units the reference art was drawn in — and a [`RibbonGeometry`] states
//! them in metres. So the widths are divided through by
//! [`terrain_generators::iso::PX_PER_METRE`] on the way across, and that is the whole of the
//! conversion.
//!
//! It is worth naming what that division fixes. Widths in cache pixels tie the
//! description of a blade to the resolution it happens to be drawn at: hand the
//! same scene to a renderer at a different scale and the blades come out the
//! wrong thickness, which is exactly why the Cycles export carries a
//! `blade_width` fudge factor at its boundary. Metres remove the fudge — but
//! removing it from the *rasteriser* would move every pixel, so the rasteriser
//! keeps its pixel widths and the bridge converts.

use glam::Vec2;
use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::digest::Fingerprint;
use terrain_scene::mark::{
    Aabb3, MarkAttributes, MarkId, PainterOrder, RibbonGeometry, RibbonMark, SceneMark,
    SceneMaterialBinding, SceneMaterialIndex, Stratum, TipShape, WidthProfile,
};
use terrain_scene::projection::ScenePoint;
use terrain_scene::scene::{SceneBuilder, SceneRequest, TerrainScene};

use terrain_generators::geometry::{Profile, TipProfile};
use terrain_generators::iso;
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;
use terrain_generators::stroke::Stroke;
use terrain_generators::tone::Tone;

/// The version this bridge stamps on the scenes it builds.
///
/// Separate from [`crate::fingerprint::GENERATOR_VERSION`], which versions the
/// *meadow*. This versions the mapping: a change here moves every scene
/// fingerprint without moving a single blade, and conflating the two would make
/// a bridge fix indistinguishable from a generator regression.
pub const BRIDGE_VERSION: u32 = 1;

/// Which appearance a tone binds to.
///
/// The tones are the rasteriser's palette families, and they map to
/// renderer-side appearance keys rather than to document materials — a blade of
/// grass is made of grass whatever the ground under it is composed of. See
/// [`SceneMaterialBinding`].
fn appearance_for(tone: Tone) -> &'static str {
    match tone {
        Tone::Soil => "surface.bare_soil",
        Tone::Thatch => "plant.thatch",
        Tone::Grass => "plant.grass_blade",
        Tone::Leaf => "plant.broad_leaf",
        Tone::Dry => "plant.dry_stem",
    }
}

/// Which band of the picture a tone belongs to.
///
/// Thatch and soil lie on the ground whatever their root heights say, which is
/// the job [`Stratum`] exists for: the mat is *under* the canopy as a matter of
/// what it is, not as a matter of where it happens to sit.
fn stratum_for(tone: Tone) -> Stratum {
    match tone {
        Tone::Soil | Tone::Thatch => Stratum::Ground,
        Tone::Grass | Tone::Leaf => Stratum::Canopy,
        Tone::Dry => Stratum::Emergent,
    }
}

fn width_profile(profile: Profile) -> WidthProfile {
    match profile {
        Profile::Tapered => WidthProfile::Tapered,
        Profile::Oval => WidthProfile::Oval,
        Profile::Stem => WidthProfile::Stem,
        Profile::Leaf => WidthProfile::Leaf,
    }
}

fn tip_shape(tip: TipProfile) -> TipShape {
    match tip {
        TipProfile::Pointed => TipShape::Pointed,
        TipProfile::Notched { depth } => TipShape::Notched { depth },
        TipProfile::Forked {
            split_at,
            opening,
            long,
            short,
        } => TipShape::Forked {
            split_at,
            opening_rad: opening,
            long,
            short,
        },
    }
}

/// One stroke as a ribbon.
///
/// `index` is the mark's position in the grass scene, and it becomes the stable
/// id. That is a *transitional* identity and is the one thing here that will
/// change: a real stable id comes from the candidate that produced the mark, and
/// the grass generator does not yet carry candidates. Using the index means the
/// id depends on how many marks came before it — the exact mistake this
/// framework rejects everywhere else — and it is acceptable only because the
/// bridge exists to be replaced.
pub fn ribbon_from_stroke(
    stroke: &Stroke,
    index: usize,
    material: SceneMaterialIndex,
) -> SceneMark {
    let stable_id = MarkId(terrain_core::seed::mix(index as u64));
    let root = ScenePoint::new(
        stroke.root.x as f64,
        stroke.root.y as f64,
        stroke.root.z as f64,
    );

    // Widths cross from cache pixels into metres here. See the module note.
    let to_metres = 1.0 / iso::PX_PER_METRE;
    let geometry = RibbonGeometry {
        length_m: stroke.length,
        azimuth_rad: stroke.azimuth,
        bend_rad: stroke.bend,
        curl_rad: stroke.curl,
        sway_rad: stroke.sway,
        kink_rad: stroke.kink,
        kink_at: stroke.kink_at,
        kink_turn_rad: stroke.kink_turn,
        twist_rad: stroke.twist,
        width_m: stroke.width * to_metres,
        tip_width_m: stroke.tip_width * to_metres,
        profile: width_profile(stroke.profile),
        tip: tip_shape(stroke.tip),
        ridge: stroke.ridge,
    };

    // The depth bias is a rasteriser trick — a mark pushed behind its own root
    // so only fragments of it survive — and it belongs in the order rather than
    // in the geometry, because it changes what draws over what and nothing else.
    let projection = terrain_scene::projection::Projection::default();
    let depth = projection.depth(root) - stroke.depth_bias as f64;
    let order = PainterOrder::new(stratum_for(stroke.tone), depth, 0, stable_id);

    let reach = geometry.reach_m() as f64;
    SceneMark::Ribbon(RibbonMark {
        stable_id,
        order,
        material,
        root,
        geometry,
        attributes: MarkAttributes {
            maturity: stroke.maturity,
            // The rasteriser's `base_light` is a position in a hand-authored
            // ramp rather than a measurement, and the nearest intrinsic thing it
            // stands for is how damp the ground under this mark is. Carried
            // across so nothing is lost; not claimed to be more than a proxy.
            moisture: stroke.base_light,
            exposure: stroke.tip_light,
            tint: 0.0,
            variation: stroke.glint,
        },
        bounds: Aabb3::around(root, reach),
    })
}

/// Turn a grown grass scene into the generic scene IR.
///
/// The ground grid is left flat: the grass generator carries its own
/// `WorldField` and does not yet sample a `PreparedTerrain`, so there is nothing
/// honest to put in the material channels. Filling them with a guess would be
/// worse than leaving them empty, because a consumer cannot tell a guess from a
/// measurement.
pub fn scene_from_grass(grass: &GrassScene, document_digest: Fingerprint) -> TerrainScene {
    let page = grass.page;
    let request = request_for_page(&page);
    let mut builder = SceneBuilder::new(request, document_digest, BRIDGE_VERSION);

    // Bind every appearance the meadow actually uses, in first-use order.
    let mut bindings: Vec<(Tone, SceneMaterialIndex)> = Vec::new();
    for stroke in &grass.marks {
        if bindings.iter().any(|(tone, _)| *tone == stroke.tone) {
            continue;
        }
        let index = builder.bind_material(SceneMaterialBinding {
            appearance: terrain_core::ids::AppearanceKey::new(appearance_for(stroke.tone))
                .expect("appearance keys are valid by construction"),
            terrain_material: None,
        });
        bindings.push((stroke.tone, index));
    }

    for (index, stroke) in grass.marks.iter().enumerate() {
        let material = bindings
            .iter()
            .find(|(tone, _)| *tone == stroke.tone)
            .map(|(_, index)| *index)
            .unwrap_or(SceneMaterialIndex(0));
        builder.push_mark(ribbon_from_stroke(stroke, index, material));
    }

    builder.build()
}

/// The scene request a grass page corresponds to.
///
/// A page is a rectangle in *cache pixels*, and its ground footprint is the
/// diamond that unprojects to. The request's bounds are the axis-aligned box
/// around that diamond, which is larger than the page's own area — the same
/// relationship [`terrain_scene::projection::Projection::screen_bounds`] runs in
/// the other direction.
pub fn request_for_page(page: &Page) -> SceneRequest {
    let corners = [
        Vec2::ZERO,
        Vec2::new(page.width as f32, 0.0),
        Vec2::new(0.0, page.height as f32),
        Vec2::new(page.width as f32, page.height as f32),
    ];
    let mut low = Vec2::splat(f32::INFINITY);
    let mut high = Vec2::splat(f32::NEG_INFINITY);
    for corner in corners {
        let ground = page.ground_at(corner);
        low = low.min(ground);
        high = high.max(ground);
    }

    // The page's own rectangle, in screen metres. Cache pixels count +Y down and
    // screen metres count +Y up, so the vertical span comes out negated and
    // `ScreenRect::new` puts the corners back in order.
    let scale = 1.0 / page.px_per_metre as f64;
    let viewport = terrain_scene::projection::ScreenRect::new(
        terrain_scene::projection::ScreenPoint::new(
            page.origin.x as f64 * scale,
            -(page.origin.y as f64) * scale,
        ),
        terrain_scene::projection::ScreenPoint::new(
            (page.origin.x as f64 + page.width as f64) * scale,
            -(page.origin.y as f64 + page.height as f64) * scale,
        ),
    );

    SceneRequest {
        bounds: WorldRect::new(
            WorldPoint::new(low.x as f64, low.y as f64),
            WorldPoint::new(high.x as f64, high.y as f64),
        ),
        viewport,
        projection: terrain_scene::projection::Projection::default(),
        output_size: [page.width as u32, page.height as u32],
        pixels_per_metre: page.px_per_metre,
        lod: 0,
        halo_m: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_generators::field::WorldField;
    use terrain_generators::style::GrassParams;

    fn grown() -> (GrassScene, WorldField, GrassParams) {
        let params = GrassParams::default();
        let field = WorldField::lit_by(params.seed, params.light);
        let page = Page::new(Vec2::new(-64.0, -64.0), 64, 64);
        let scene = GrassScene::build(page, &field, &params);
        (scene, field, params)
    }

    fn digest() -> Fingerprint {
        Fingerprint::from_u128(0x1234_5678)
    }

    #[test]
    fn every_grass_mark_becomes_a_scene_mark() {
        // The bridge's whole claim: nothing is dropped on the way across.
        let (grass, _, _) = grown();
        let scene = scene_from_grass(&grass, digest());
        assert_eq!(scene.mark_count(), grass.len());
        assert!(
            scene.mark_count() > 500,
            "only {} marks",
            scene.mark_count()
        );
    }

    #[test]
    fn the_scene_comes_out_in_painter_order() {
        let (grass, _, _) = grown();
        assert!(scene_from_grass(&grass, digest()).is_sorted());
    }

    #[test]
    fn the_same_meadow_produces_the_same_scene_fingerprint() {
        // What the migration is checked against. Two runs of the bridge over the
        // same meadow must agree exactly.
        let (grass, _, _) = grown();
        let first = scene_from_grass(&grass, digest());
        let second = scene_from_grass(&grass, digest());
        assert_eq!(first.fingerprint(), second.fingerprint());
    }

    #[test]
    fn a_regrown_meadow_produces_the_same_scene_fingerprint() {
        // Stronger: the grass is regenerated from scratch, and the scene still
        // matches. This is the statement that the IR carries the meadow without
        // losing or inventing anything.
        let (first, _, _) = grown();
        let (second, _, _) = grown();
        assert_eq!(
            scene_from_grass(&first, digest()).fingerprint(),
            scene_from_grass(&second, digest()).fingerprint()
        );
    }

    #[test]
    fn a_different_meadow_produces_a_different_scene_fingerprint() {
        let (grass, _, params) = grown();
        let other_field = WorldField::lit_by(params.seed ^ 1, params.light);
        let other = GrassScene::build(
            grass.page,
            &other_field,
            &GrassParams {
                seed: params.seed ^ 1,
                ..params
            },
        );
        assert_ne!(
            scene_from_grass(&grass, digest()).fingerprint(),
            scene_from_grass(&other, digest()).fingerprint()
        );
    }

    #[test]
    fn widths_arrive_in_metres_rather_than_cache_pixels() {
        // The one conversion that is not mechanical. If the division is ever
        // dropped, widths come back in the tens and every renderer downstream
        // draws straps instead of grass.
        //
        // The ceiling is a tenth of a metre rather than the few millimetres a
        // real blade measures, and the gap is worth naming rather than tightening
        // away. The rasteriser's widths are **stroke** widths — how much paint a
        // mark lays down — tuned so a 2D mark vocabulary fills the frame, and its
        // broadest marks are six centimetres of *paint* standing for a clump. A
        // botanically correct four-millimetre blade is under half a pixel at the
        // authoring scale and simply cannot be rasterised, which is why the
        // Cycles export carries a `blade_width` multiplier of 0.35 at its own
        // boundary. Both numbers are honest about different things; only the
        // Cycles one is a plant.
        let (grass, _, _) = grown();
        let scene = scene_from_grass(&grass, digest());
        let mut widest = 0.0f32;
        for mark in &scene.marks {
            let SceneMark::Ribbon(ribbon) = mark else {
                continue;
            };
            assert!(
                ribbon.geometry.width_m > 0.0 && ribbon.geometry.width_m < 0.1,
                "a mark {} m wide — the pixel-to-metre division looks wrong",
                ribbon.geometry.width_m
            );
            widest = widest.max(ribbon.geometry.width_m);
        }
        // And the meadow genuinely uses its width range, so this is measuring
        // something rather than passing on a field of hairlines.
        assert!(widest > 0.01, "the widest mark is only {widest} m");
    }

    #[test]
    fn lengths_and_angles_cross_unchanged() {
        // Everything except the widths is already in world units, so the bridge
        // must not touch it.
        let (grass, _, _) = grown();
        let scene = scene_from_grass(&grass, digest());
        // Marks are sorted, so find each by its stable id rather than by index.
        for (index, stroke) in grass.marks.iter().enumerate() {
            let id = MarkId(terrain_core::seed::mix(index as u64));
            let found = scene
                .marks
                .iter()
                .find(|mark| mark.stable_id() == id)
                .expect("every stroke crossed");
            let SceneMark::Ribbon(ribbon) = found else {
                panic!("a stroke did not become a ribbon");
            };
            assert_eq!(ribbon.geometry.length_m, stroke.length);
            assert_eq!(ribbon.geometry.bend_rad, stroke.bend);
            assert_eq!(ribbon.geometry.azimuth_rad, stroke.azimuth);
            assert_eq!(ribbon.geometry.twist_rad, stroke.twist);
            assert_eq!(ribbon.root.z_m, stroke.root.z as f64);
        }
    }

    #[test]
    fn the_mat_stays_under_the_canopy() {
        // Thatch is under the grass as a matter of what it is, not of where its
        // roots happen to sit — which is the job the stratum does.
        assert_eq!(stratum_for(Tone::Thatch), Stratum::Ground);
        assert_eq!(stratum_for(Tone::Soil), Stratum::Ground);
        assert_eq!(stratum_for(Tone::Grass), Stratum::Canopy);
        assert!(Stratum::Ground < Stratum::Canopy);
    }

    #[test]
    fn every_tone_binds_to_its_own_appearance() {
        // A collision here would mean two families of mark sharing one material
        // in every renderer.
        let mut seen: Vec<&str> = Vec::new();
        for tone in [Tone::Soil, Tone::Thatch, Tone::Grass, Tone::Leaf, Tone::Dry] {
            let appearance = appearance_for(tone);
            assert!(!seen.contains(&appearance), "{appearance} is bound twice");
            assert!(
                terrain_core::ids::AppearanceKey::new(appearance).is_ok(),
                "{appearance} is not a usable key"
            );
            seen.push(appearance);
        }
    }

    #[test]
    fn appearances_are_bound_once_however_many_marks_use_them() {
        // Ten thousand blades must not produce ten thousand material bindings,
        // or Cycles builds ten thousand identical shader graphs.
        let (grass, _, _) = grown();
        let scene = scene_from_grass(&grass, digest());
        assert!(
            scene.materials.len() <= 5,
            "{} bindings for five tones",
            scene.materials.len()
        );
        assert!(!scene.materials.is_empty());
    }

    #[test]
    fn a_pages_request_covers_the_ground_the_page_shows() {
        // A ground rectangle projects to a diamond, so the request's bounds are
        // larger than the page's own area — and every corner of the page has to
        // land inside them.
        let page = Page::new(Vec2::new(-64.0, -64.0), 64, 64);
        let request = request_for_page(&page);
        assert_eq!(request.output_size, [64, 64]);
        assert_eq!(request.pixels_per_metre, page.px_per_metre);
        for corner in [
            Vec2::ZERO,
            Vec2::new(64.0, 0.0),
            Vec2::new(0.0, 64.0),
            Vec2::new(64.0, 64.0),
        ] {
            let ground = page.ground_at(corner);
            let point = WorldPoint::new(ground.x as f64, ground.y as f64);
            // The maximum corner is on the half-open boundary, so test against a
            // rectangle grown by a hair rather than against the bounds exactly.
            assert!(
                request.bounds.expanded(1.0e-6).contains(point),
                "{point} is outside the request"
            );
        }
    }

    #[test]
    fn the_bridge_version_is_separate_from_the_generator_version() {
        // A change to the mapping moves every scene fingerprint without moving a
        // single blade. Conflating the two would make a bridge fix look exactly
        // like a generator regression.
        assert_eq!(BRIDGE_VERSION, 1);
        assert_eq!(crate::fingerprint::GENERATOR_VERSION, 1);
    }
}

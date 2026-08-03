//! What the sun cannot see.
//!
//! The field has had a directional term since it had lighting: march the canopy
//! height toward the key and see what blocks it. That is a real measurement and
//! it produces a real cue — it is what gives each mound a lit face and a dark
//! back — but it can only ever describe the canopy as a *surface*. It cannot
//! know that this particular blade is in front of that particular gap, so it
//! cannot draw the shadow of a blade, and it certainly cannot draw the shadow of
//! a forked tip.
//!
//! So the geometry is rendered a second time, from the sun.
//!
//! ## The same geometry, not the same shape twice
//!
//! Both passes walk [`crate::stroke::walk_blade`] over the marks in one
//! [`crate::scene::GrassScene`]. That is the whole reason the scene became a
//! value: regenerating the blades for the shadow pass would nearly work, because
//! placement is deterministic, and "nearly" is exactly the failure that produces
//! shadows which do not quite belong to the blades casting them.
//!
//! ## Light space
//!
//! An orthographic frame with the sun looking down its own axis:
//!
//! ```text
//!   u = a world axis perpendicular to the sun
//!   v = the other one
//!   d = distance along the sun's direction — the depth
//! ```
//!
//! A point is lit when nothing nearer the sun occupies its texel. Since the
//! camera is orthographic too, "nearer the sun" is a plain dot product and there
//! is no projective divide anywhere in this module.
//!
//! ## Why the map is bigger than the page
//!
//! A blade shades what is *away* from the sun, so the casters that matter are
//! the ones up-light of the page — and at the 35° sun this renderer is built
//! for, a blade shades ground one and a half times its own height away. The
//! volume therefore has to cover the page plus that reach plus the canopy's own
//! height, and getting it wrong does not clip a shadow visibly: it deletes one,
//! and only on the pages whose casters fell outside.

use bevy::prelude::*;

use crate::iso;
use crate::quality::GrassRenderQuality;
use crate::scene::GrassScene;
use crate::stroke::{Stroke, walk_blade};

/// A depth buffer rendered from the sun.
pub struct ShadowMap {
    /// Texels across and down.
    width: usize,
    height: usize,
    /// World metres per texel.
    texel: f32,
    /// The two world axes the map is laid out on, both perpendicular to the sun.
    u: Vec3,
    v: Vec3,
    /// Toward the sun, unit length.
    sun: Vec3,
    /// Light-space coordinate of texel `(0, 0)`.
    origin: Vec2,
    /// Distance along the sun of the nearest thing over each texel.
    depth: Vec<f32>,
}

/// How far a blade's shadow reaches along the ground, per unit of its height.
///
/// The number the guard band is sized from, and it is a function of the sun
/// alone: `|L.xy| / L.z`, which is one over the tangent of the elevation. At
/// 35° it is 1.43; at 20° it would be 2.75, and the band — and the cost of every
/// page — would nearly double with it.
#[inline]
pub fn reach_per_height(sun: Vec3) -> f32 {
    let plane = Vec2::new(sun.x, sun.y).length();
    plane / sun.z.abs().max(1.0e-3)
}

impl ShadowMap {
    /// Build the volume that covers a page's casters, and rasterise the scene
    /// into it.
    ///
    /// `ceiling` is how high anything in the scene stands, in world metres — a
    /// genuine bound rather than an estimate, because a caster clipped out of
    /// the volume is a shadow that simply is not there.
    pub fn cast(
        scene: &GrassScene,
        sun: Vec3,
        ceiling: f32,
        quality: GrassRenderQuality,
        jitter: Vec2,
    ) -> Option<Self> {
        let density = quality.shadow_density();
        if density <= 0.0 {
            return None;
        }
        let sun = sun.normalize_or(Vec3::Z);
        // Any axis not parallel to the sun will do for the first cross product;
        // the world's up is only degenerate for a sun directly overhead, which
        // this renderer does not support and which would cast no shadow anyway.
        let seed = if sun.z.abs() > 0.95 { Vec3::X } else { Vec3::Z };
        let u = sun.cross(seed).normalize_or(Vec3::X);
        let v = sun.cross(u).normalize_or(Vec3::Y);

        let page = &scene.page;
        let texel = 1.0 / (page.px_per_metre * density);

        // The world box everything that can shade this page lives in: the page's
        // own ground, widened by how far a shadow reaches, and as tall as the
        // canopy.
        let mut low = Vec2::splat(f32::INFINITY);
        let mut high = Vec2::splat(f32::NEG_INFINITY);
        for corner in [
            Vec2::ZERO,
            Vec2::new(page.width as f32, 0.0),
            Vec2::new(0.0, page.height as f32),
            Vec2::new(page.width as f32, page.height as f32),
        ] {
            let ground = page.ground_at(corner);
            low = low.min(ground);
            high = high.max(ground);
        }
        // Casters lie up-light of the page. Widening in both directions rather
        // than only the one costs a little map and removes a whole class of
        // sign error.
        let spread = ceiling * reach_per_height(sun) + ceiling;
        low -= Vec2::splat(spread);
        high += Vec2::splat(spread);

        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for x in [low.x, high.x] {
            for y in [low.y, high.y] {
                for z in [0.0, ceiling] {
                    let point = Vec3::new(x, y, z);
                    let flat = Vec2::new(point.dot(u), point.dot(v));
                    lo = lo.min(flat);
                    hi = hi.max(flat);
                }
            }
        }

        // Anchored to a whole number of texels in **world** space, so two pages
        // sharing ground quantise it identically. A map whose grid depended on
        // where the page happened to start would give the same blade a shadow
        // half a texel to the left on one page and half a texel to the right on
        // its neighbour, which is a seam that only appears once the world is
        // tiled.
        let origin = (lo / texel).floor() * texel + jitter * texel;
        let width = (((hi.x - origin.x) / texel).ceil() as usize + 2).min(MAX_SIDE);
        let height = (((hi.y - origin.y) / texel).ceil() as usize + 2).min(MAX_SIDE);

        let mut map = Self {
            width,
            height,
            texel,
            u,
            v,
            sun,
            origin,
            depth: vec![f32::NEG_INFINITY; width * height],
        };
        for mark in &scene.marks {
            map.draw(mark, quality);
        }
        Some(map)
    }

    /// Rasterise one mark into the depth buffer.
    fn draw(&mut self, stroke: &Stroke, quality: GrassRenderQuality) {
        // How many *shadow* texels the blade spans, which is what decides how
        // finely it is walked here. The camera's own step count is the wrong
        // answer in both directions — the map is denser than the page, and a page
        // baked for a distant camera is coarser than the map.
        let arc_texels = stroke.length / self.texel;
        // The fork is resolved against the shadow map's own resolution, so a
        // blade whose split the *page* cannot show may still cast a forked
        // shadow. That is the right way round: the map is the denser buffer.
        let tip = match stroke.tip {
            crate::geometry::TipProfile::Forked { long, short, .. } => stroke
                .tip
                .resolved_at(long.min(short) * stroke.length / self.texel),
            other => other,
        };

        let mut samples = Vec::new();
        walk_blade(
            stroke,
            arc_texels,
            quality.ribs_per_pixel(),
            tip,
            &mut |s| samples.push(s),
        );

        for sample in samples {
            let half = sample.half_reference / iso::PX_PER_METRE;
            // Widened by half a texel. A ribbon narrower than one texel either
            // hits its centre or misses it, so an unwidened thin blade casts a
            // dotted shadow — the same aliasing that makes a thin bright line
            // crawl, except it is the *absence* of light that flickers and that
            // reads far worse. Length is left exact; only the width is
            // conservative.
            let half = half.max(self.texel * 0.5);
            self.stamp(sample.position, sample.frame.binormal, half);
        }
    }

    /// Lay one rib of a blade into the depth buffer.
    fn stamp(&mut self, centre: Vec3, across: Vec3, half: f32) {
        let steps = ((2.0 * half / self.texel).ceil() as usize).max(1);
        let step = 2.0 * half / steps as f32;
        for i in 0..=steps {
            let point = centre + across * (-half + step * i as f32);
            let flat = Vec2::new(point.dot(self.u), point.dot(self.v));
            let cell = (flat - self.origin) / self.texel;
            if cell.x < 0.0 || cell.y < 0.0 {
                continue;
            }
            let (x, y) = (cell.x as usize, cell.y as usize);
            if x >= self.width || y >= self.height {
                continue;
            }
            let index = y * self.width + x;
            let depth = point.dot(self.sun);
            if depth > self.depth[index] {
                self.depth[index] = depth;
            }
        }
    }

    /// How much sun reaches a world point, `0..1`.
    ///
    /// Percentage-closer filtered over a small kernel rather than tested once.
    /// A single comparison on geometry this thin gives a binary, jagged shadow
    /// whose edge follows the texel grid; averaging a few neighbours turns that
    /// into the soft edge a blade actually casts, and costs nothing next to
    /// having built the map.
    pub fn visibility(&self, point: Vec3, normal: Vec3) -> f32 {
        let flat = Vec2::new(point.dot(self.u), point.dot(self.v));
        let cell = (flat - self.origin) / self.texel;
        let depth = point.dot(self.sun);

        // Slope-scaled, and offset along the normal as well.
        //
        // A surface nearly edge-on to the sun spans a lot of depth inside one
        // texel, so a fixed bias either fails to stop it shadowing itself or is
        // so large that shadows detach from the blades casting them. Scaling by
        // how grazing the surface is spends the bias only where it is needed.
        let facing = normal.dot(self.sun).abs().clamp(0.05, 1.0);
        let bias = self.texel * (BIAS + SLOPE_BIAS * (1.0 - facing) / facing);

        let mut lit = 0.0f32;
        let mut total = 0.0f32;
        for dy in -PCF..=PCF {
            for dx in -PCF..=PCF {
                let x = cell.x + dx as f32;
                let y = cell.y + dy as f32;
                total += 1.0;
                if x < 0.0 || y < 0.0 {
                    lit += 1.0;
                    continue;
                }
                let (xi, yi) = (x as usize, y as usize);
                if xi >= self.width || yi >= self.height {
                    lit += 1.0;
                    continue;
                }
                let blocker = self.depth[yi * self.width + xi];
                if depth + bias >= blocker {
                    lit += 1.0;
                }
            }
        }
        lit / total
    }

    /// Texels across, for reporting.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// World metres one texel covers.
    pub fn texel(&self) -> f32 {
        self.texel
    }
}

/// Half-width of the percentage-closer filter, in texels.
///
/// One, so a three-by-three. Grass shadows are already soft — the map is denser
/// than the page it lands on, and several sun directions are averaged on top —
/// so this is here to remove the texel grid rather than to be the penumbra.
const PCF: i32 = 1;

/// Constant depth bias, in texels.
const BIAS: f32 = 0.9;

/// How much more bias a grazing surface gets, in texels.
///
/// The large one, and it has to be. A blade lying nearly along the sun's
/// direction covers a wide range of depth within one texel, and biasing that
/// case with the constant term alone leaves it shadowing itself in stripes.
const SLOPE_BIAS: f32 = 2.6;

/// The most texels a shadow map may be on a side.
///
/// A backstop rather than a working limit. A page baked at a very fine scale for
/// a very low sun could otherwise ask for a map of hundreds of megabytes, and the
/// failure would be an allocation rather than a picture.
const MAX_SIDE: usize = 8192;

/// Deterministic offsets over the sun's disc, for softening.
///
/// A fixed low-discrepancy set anchored **globally**, never drawn per page. A
/// soft shadow whose sample pattern depends on which page it is in produces a
/// visible page grid, and it is the kind that survives every seam test because
/// it is not a step — it is a change of texture.
///
/// Returns unit-disc offsets to be scaled by the sun's angular radius.
pub fn sun_samples(count: usize) -> Vec<Vec2> {
    if count <= 1 {
        return vec![Vec2::ZERO];
    }
    // A Fibonacci spiral: even coverage of the disc at any count, with no
    // parameters to tune and no randomness to anchor.
    const GOLDEN: f32 = 2.399_963_2; // π(3 − √5)
    (0..count)
        .map(|i| {
            let radius = ((i as f32 + 0.5) / count as f32).sqrt();
            let angle = i as f32 * GOLDEN;
            Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

/// How wide the sun is, in radians.
///
/// The real sun is about half a degree across and casts shadows far crisper than
/// this. Grass does not read that way in a stylised isometric plate — a hard
/// shadow edge on a blade a few pixels wide is a jagged line, not a shadow — so
/// this is deliberately several times life size, which is the same licence the
/// art takes everywhere else.
pub const SUN_RADIUS: f32 = 0.055;

/// Turn the sun by a disc offset.
pub fn nudge(sun: Vec3, offset: Vec2, radius: f32) -> Vec3 {
    let seed = if sun.z.abs() > 0.95 { Vec3::X } else { Vec3::Z };
    let u = sun.cross(seed).normalize_or(Vec3::X);
    let v = sun.cross(u).normalize_or(Vec3::Y);
    (sun + (u * offset.x + v * offset.y) * radius).normalize_or(sun)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{BakeParams, Page};
    use crate::field::WorldField;

    fn sun_at(elevation: f32) -> Vec3 {
        Vec3::new(0.0, elevation.cos(), elevation.sin())
    }

    #[test]
    fn shadow_reach_is_one_over_the_tangent_of_the_elevation() {
        // The number every guard band is sized from. At the 35° this renderer is
        // built for a blade shades ground one and a half times its own height
        // away; at 20° it would be nearly three times, and the cost of a page
        // would go with it.
        for degrees in [20.0f32, 35.0, 55.0, 70.0] {
            let elevation = degrees.to_radians();
            let expected = 1.0 / elevation.tan();
            let measured = reach_per_height(sun_at(elevation));
            assert!(
                (measured - expected).abs() < 1.0e-4,
                "{degrees}°: {measured} against {expected}"
            );
        }
        assert!((reach_per_height(sun_at(35.0f32.to_radians())) - 1.428).abs() < 0.01);
    }

    #[test]
    fn a_blade_shadows_the_ground_away_from_the_sun() {
        // The claim, at its simplest: one upright blade, one flat floor, and the
        // dark side must be the side the sun is not on.
        let params = BakeParams::default();
        let page = Page::new(Vec2::new(-64.0, -64.0), 96, 96);
        let field = WorldField::lit_by(params.seed, params.light);
        let mut scene = GrassScene::build(page, &field, &params);
        scene.marks.clear();
        // Turned so its flat face is across the sun's bearing rather than
        // along it. A blade edge-on to the sun casts almost nothing, correctly,
        // and a fixture in that pose tests the pose rather than the shadow.
        scene.marks.push(Stroke {
            root: page.ground_at(Vec2::splat(48.0)).extend(0.0),
            azimuth: std::f32::consts::FRAC_PI_2,
            length: 0.30,
            bend: 0.0,
            width: 6.0,
            ..Default::default()
        });

        let elevation = 35.0f32.to_radians();
        let sun = sun_at(elevation);
        let map = ShadowMap::cast(&scene, sun, 0.35, GrassRenderQuality::Reference, Vec2::ZERO)
            .expect("reference quality casts shadows");

        let root = scene.marks[0].root;
        // The ground a little way *along* the sun's ground bearing is behind the
        // blade from the sun's point of view, so it is in shade.
        let bearing = Vec2::new(sun.x, sun.y).normalize();
        let shaded = root - (bearing * 0.10).extend(0.0);
        let open = root + (bearing * 0.40).extend(0.0);
        let shade = map.visibility(shaded, Vec3::Z);
        let light = map.visibility(open, Vec3::Z);
        assert!(
            shade < 0.5,
            "the ground behind the blade is {shade:.2} lit — no shadow"
        );
        assert!(
            light > 0.9,
            "the open ground in front is only {light:.2} lit — the shadow \
             fell the wrong way"
        );
    }

    #[test]
    fn a_lower_sun_casts_a_longer_shadow() {
        let params = BakeParams::default();
        let page = Page::new(Vec2::new(-96.0, -96.0), 128, 128);
        let field = WorldField::lit_by(params.seed, params.light);
        let mut scene = GrassScene::build(page, &field, &params);
        scene.marks.clear();
        scene.marks.push(Stroke {
            root: page.ground_at(Vec2::splat(64.0)).extend(0.0),
            azimuth: std::f32::consts::FRAC_PI_2,
            length: 0.30,
            bend: 0.0,
            width: 6.0,
            ..Default::default()
        });
        let root = scene.marks[0].root;

        let furthest = |degrees: f32| {
            let sun = sun_at(degrees.to_radians());
            let map = ShadowMap::cast(&scene, sun, 0.35, GrassRenderQuality::Reference, Vec2::ZERO)
                .unwrap();
            let bearing = Vec2::new(sun.x, sun.y).normalize();
            let mut reach = 0.0f32;
            for step in 1..120 {
                let along = step as f32 * 0.005;
                let at = root - (bearing * along).extend(0.0);
                if map.visibility(at, Vec3::Z) < 0.5 {
                    reach = along;
                }
            }
            reach
        };
        let high = furthest(60.0);
        let low = furthest(25.0);
        assert!(
            low > high * 1.4,
            "a 25° sun reaches {low:.3} m against {high:.3} m at 60°"
        );
    }

    #[test]
    fn sun_samples_cover_the_disc_evenly_and_never_move() {
        // Anchored globally and generated from nothing but the count, so two
        // pages soften their shadows with the same set. A per-page pattern is a
        // page grid that survives every seam test, because it is not a step in
        // brightness — it is a change of texture.
        let a = sun_samples(12);
        let b = sun_samples(12);
        assert_eq!(a, b);
        assert_eq!(sun_samples(1), vec![Vec2::ZERO]);
        for offset in &a {
            assert!(offset.length() <= 1.0 + 1.0e-5, "{offset:?} left the disc");
        }
        // The mean has to sit near the middle, or softening would also *move*
        // the shadow.
        let mean = a.iter().fold(Vec2::ZERO, |sum, o| sum + *o) / a.len() as f32;
        assert!(mean.length() < 0.2, "the sample set is lopsided: {mean:?}");
    }

    #[test]
    fn nudging_the_sun_keeps_it_a_unit_vector_and_near_where_it_was() {
        let sun = sun_at(35.0f32.to_radians());
        for offset in sun_samples(8) {
            let turned = nudge(sun, offset, SUN_RADIUS);
            assert!((turned.length() - 1.0).abs() < 1.0e-5);
            assert!(
                turned.dot(sun) > (2.0 * SUN_RADIUS).cos(),
                "the sun moved further than its own radius"
            );
        }
    }

    #[test]
    fn the_preview_tier_builds_no_map_at_all() {
        let params = BakeParams::default();
        let page = Page::new(Vec2::ZERO, 32, 32);
        let field = WorldField::lit_by(params.seed, params.light);
        let scene = GrassScene::build(page, &field, &params);
        assert!(
            ShadowMap::cast(
                &scene,
                sun_at(0.6),
                0.4,
                GrassRenderQuality::Preview,
                Vec2::ZERO
            )
            .is_none()
        );
    }
}

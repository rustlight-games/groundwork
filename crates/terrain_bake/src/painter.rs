//! Rasterising a mark onto a surface.
//!
//! Split out of `stroke.rs`, which held two things that had no business sharing
//! a module: the **description** of a mark, which the generator produces, and the
//! **rasteriser**, which is one renderer's way of drawing one. Keeping them
//! together meant that placing a blade required linking the code that fills
//! pixels — so the generator could not be a crate without the surface coming
//! with it.
//!
//! The split is also what makes the honest statement possible: a `Stroke` is
//! consumed by the rasteriser here, by the Cycles exporter, and by the shadow
//! pass, and none of those three is more canonical than the others. A
//! description that lives inside one of its consumers quietly becomes that
//! consumer's private format.

use glam::{Vec2, Vec3};

use crate::surface::{Fragment, Surface};
use terrain_generators::geometry::{self, Frame, TipProfile};
use terrain_generators::iso;
use terrain_generators::scene::GrassScene;
use terrain_generators::stroke::{BladeSample, Stroke, walk_blade};

/// One slice across a stroke, with everything the slice needs already worked
/// out.
struct Rib {
    centre: Vec2,
    /// Unit page direction the rib runs along — the blade's own width axis,
    /// projected.
    across: Vec2,
    half: f32,
    under: f32,
    depth: f32,
    top: f32,
    /// Root-to-tip position, `0..1`.
    along: f32,
    /// `base_light + tip - root_shade`: the mark's own albedo, with no lighting
    /// in it at all.
    body_light: f32,
    under_light: f32,
    /// Whether the owning stroke is rooted in the ground being rendered.
    visible: bool,
}

/// Rasterises strokes into one page's [`Surface`].
pub struct Painter<'a> {
    surface: &'a mut Surface,
    /// Cache-pixel position of the page's top-left corner.
    origin: Vec2,
    /// The light's direction on the screen plane, normalised.
    light_plane: Vec2,
    /// This page's cache pixels per world metre.
    px_per_metre: f32,
    /// That, as a fraction of the scale the art is authored at.
    ///
    /// Every width on a [`Stroke`] is in *reference* cache pixels — the units
    /// the reference art is drawn in — and is multiplied by this on the way to
    /// the page. Keeping the conversion here rather than at the two dozen places
    /// a stroke is built means a mark is described once and drawn correctly at
    /// any scale.
    detail: f32,
    /// The surface's supersampling factor, cached as a float because every
    /// world-to-page conversion multiplies by it.
    scale: f32,
    /// Ribs per supersampled pixel of blade length.
    ribs_per_pixel: f32,
    /// The ground the render is *about*, in world metres.
    ///
    /// `None` means the whole page is the picture, which is what a laboratory
    /// plate wants and is the behaviour every caller had before there were tile
    /// layouts. When it is set, a mark rooted outside it is still drawn — it
    /// occludes and it is part of the neighbourhood every shading term reads —
    /// but it is marked as not belonging to the silhouette. See
    /// [`crate::surface::Surface::canopy_coverage`].
    visible_ground: Option<(Vec2, Vec2)>,
    /// Reusable centreline buffer. See [`Painter::draw`].
    samples: Vec<BladeSample>,
}

impl<'a> Painter<'a> {
    /// A painter for a page baked at the authoring scale.
    pub fn new(surface: &'a mut Surface, origin: Vec2, light: Vec3) -> Self {
        Self::at_scale(surface, origin, light, iso::PX_PER_METRE)
    }

    /// A painter for a page baked at `px_per_metre` cache pixels to the metre.
    pub fn at_scale(
        surface: &'a mut Surface,
        origin: Vec2,
        light: Vec3,
        px_per_metre: f32,
    ) -> Self {
        let plane = Vec2::new(light.x, light.y);
        let scale = surface.supersample() as f32;
        Self {
            surface,
            origin,
            light_plane: plane.normalize_or_zero(),
            px_per_metre,
            detail: px_per_metre / iso::PX_PER_METRE,
            scale,
            ribs_per_pixel: 2.0,
            visible_ground: None,
            samples: Vec::new(),
        }
    }

    /// How finely centrelines are walked, in ribs per supersampled pixel.
    pub fn with_ribs_per_pixel(mut self, ribs: f32) -> Self {
        self.ribs_per_pixel = ribs.max(0.5);
        self
    }

    /// Which ground the render is about, in world metres.
    ///
    /// Half-open, like every other rectangle here.
    pub fn with_visible_ground(mut self, min: Vec2, max: Vec2) -> Self {
        self.visible_ground = Some((min, max));
        self
    }

    /// Whether a mark rooted here belongs to the picture.
    #[inline]
    fn roots_inside(&self, root: Vec3) -> bool {
        match self.visible_ground {
            None => true,
            Some((min, max)) => {
                root.x >= min.x && root.x < max.x && root.y >= min.y && root.y < max.y
            }
        }
    }

    /// Supersampled pixels per final pixel on this page.
    #[inline]
    pub fn supersample(&self) -> f32 {
        self.scale
    }

    /// World point to supersampled page pixel.
    #[inline]
    pub fn to_page(&self, world: Vec3) -> Vec2 {
        (iso::to_cache_at(world, self.px_per_metre) - self.origin) * self.scale
    }

    /// A world *direction* as a page-pixel direction.
    ///
    /// The projection is linear, so a direction maps without the origin term.
    /// Written out rather than differencing two projected points because the
    /// difference of two nearly equal projections is where precision goes to
    /// die, and the rib's width axis is exactly that case.
    #[inline]
    fn page_direction(&self, world: Vec3) -> Vec2 {
        let unit = self.px_per_metre * self.scale;
        Vec2::new(
            (world.x - world.y) * unit,
            ((world.x + world.y) * 0.5 - world.z) * unit,
        )
    }

    /// Supersampled page pixel back to the ground plane.
    #[inline]
    pub fn to_ground(&self, page: Vec2) -> Vec2 {
        iso::from_cache_ground_at(page / self.scale + self.origin, self.px_per_metre)
    }

    pub fn surface(&self) -> &Surface {
        self.surface
    }

    pub fn surface_mut(&mut self) -> &mut Surface {
        self.surface
    }

    #[inline]
    fn plot(&mut self, x: f32, y: f32, fragment: Fragment) {
        // `as` truncates toward zero, so a negative coordinate would fold back
        // onto the page's left edge as a bright smear. Reject before casting.
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        if x >= self.surface.width || y >= self.surface.height {
            return;
        }
        let index = self.surface.index(x, y);
        self.surface.write(index, fragment);
    }

    /// A bound, in cache pixels, on how far this stroke's marks can land from
    /// its own root. See [`Stroke::reach`].
    #[inline]
    pub fn reach(&self, stroke: &Stroke) -> f32 {
        stroke.reach(self.px_per_metre)
    }

    /// Draw one stroke, tip and all.
    pub fn draw(&mut self, stroke: &Stroke) {
        // How long the mark is on this page decides two things: how finely the
        // centreline is walked, and whether a fork is drawable at all.
        //
        // And it is linear in the *page's* scale, which is the whole of why a
        // distant page is cheap: the same blade of grass, drawn onto a page
        // baked at a quarter scale, is a quarter as long in pixels and so walks
        // a quarter of the ribs — each of which is a quarter as wide.
        let arc_pixels = stroke.length * self.px_per_metre * self.scale;
        let tip = stroke.tip.resolved_at(self.child_pixels(stroke));

        // Into a buffer the painter owns, rather than straight into the surface.
        //
        // The walk cannot borrow the surface while the painter does, and the
        // alternative — inlining the rib inside the walk's callback — is what
        // fused geometry and rasterisation together in the first place. A
        // reusable buffer costs one allocation for the life of the painter and
        // keeps the two apart, which is what lets the shadow pass walk the very
        // same centreline.
        let mut samples = std::mem::take(&mut self.samples);
        samples.clear();
        walk_blade(
            stroke,
            arc_pixels,
            self.ribs_per_pixel,
            tip,
            &mut |sample| samples.push(sample),
        );

        let pen = self.scale * self.detail;
        let under = stroke.under * pen;
        let under_light = (stroke.base_light - 0.22).max(0.0);
        // Decided once from the root, not per rib: a blade belongs to the
        // picture or it does not, and the parts of it that lean past the edge
        // belong with the rest of it.
        let visible = self.roots_inside(stroke.root);
        for sample in &samples {
            // The rib runs along the blade's own width axis, projected. That is
            // not quite perpendicular to the projected centreline once the blade
            // twists, and the difference is the point: a twisted blade presents
            // a skewed cross-section, which is exactly what the eye reads as a
            // surface turning.
            let across = self
                .page_direction(sample.frame.binormal)
                .normalize_or(Vec2::new(1.0, 0.0));
            let half = sample.half_reference * pen * geometry::foreshorten(sample.frame.normal);
            self.rib(
                stroke,
                &sample.frame,
                Rib {
                    centre: self.to_page(sample.position),
                    across,
                    half,
                    under,
                    depth: iso::depth(sample.position) - stroke.depth_bias,
                    // In *reference* pixels at every page scale, on purpose.
                    // Every shading term downstream keys on how tall the canopy
                    // stands, and grass of a given world height has to mean the
                    // same thing to those terms whether the page holding it was
                    // baked at full detail or a quarter of it.
                    top: sample.position.z * iso::Z_SCALE * iso::PX_PER_METRE,
                    along: sample.along,
                    body_light: stroke.base_light + sample.tip_light - sample.root_shade,
                    under_light,
                    visible,
                },
            );
        }
        self.samples = samples;
    }

    /// How many final page pixels one fork child would span.
    #[inline]
    fn child_pixels(&self, stroke: &Stroke) -> f32 {
        match stroke.tip {
            TipProfile::Forked { long, short, .. } => {
                long.min(short) * stroke.length * self.px_per_metre
            }
            _ => f32::INFINITY,
        }
    }

    /// One slice across the stroke.
    ///
    /// Records what the surface *is* rather than what it looks like. The old rib
    /// computed a lambert term here and stored the answer; this one stores the
    /// normal and lets [`crate::bake::resolve`] decide, which is what makes a
    /// cast shadow able to attenuate the direct light without also flattening
    /// the form, and transmission able to key on which way the leaf faces.
    fn rib(&mut self, stroke: &Stroke, frame: &Frame, rib: Rib) {
        let Rib {
            centre,
            across,
            half,
            under,
            depth,
            top,
            along,
            body_light,
            under_light,
            visible,
        } = rib;
        // The under-stroke goes on the side facing away from the light, which is
        // what makes it read as the blade's own shadow rather than as an
        // outline.
        let away = across.dot(self.light_plane) > 0.0;
        let (low, high) = if away {
            (-half - under, half)
        } else {
            (-half, half + under)
        };

        let span = high - low;
        let steps = (span.ceil() as usize).max(1);
        let step = span / steps as f32;

        // Whole ribs fall outside the page — the guard band exists so that
        // strokes rooted off the edge still lean in, and the parts of them that
        // do not lean in are most of their length. One rectangle test here
        // replaces a bounds check on every pixel of the rib.
        let extent = across.abs() * high.abs().max(low.abs());
        if centre.x + extent.x < 0.0
            || centre.y + extent.y < 0.0
            || centre.x - extent.x >= self.surface.width as f32
            || centre.y - extent.y >= self.surface.height as f32
        {
            return;
        }

        let inverse_half = 1.0 / half.max(1.0e-3);
        // Pushed a hair behind the body so a neighbouring blade at the same
        // depth still wins the pixel.
        let under_depth = depth - 1.0e-4;
        // Which face of the leaf the camera is looking at. Constant across the
        // rib, because the ridge tilts the normal but cannot turn it over.
        let underside = frame.normal.dot(iso::TOWARD_VIEWER) < 0.0;

        for i in 0..=steps {
            let offset = low + step * i as f32;
            let point = centre + across * offset;

            let fragment = if offset < -half || offset > half {
                // Under-stroke: darker by a fixed amount rather than by a
                // fraction, so a bright blade and a dim one cast the same
                // weight of shadow. It keeps the blade's own normal, so it
                // shades as part of the same surface rather than as a decal.
                Fragment {
                    depth: under_depth,
                    light: under_light,
                    normal: frame.normal,
                    tone: stroke.tone,
                    top,
                    along,
                    maturity: stroke.maturity,
                    underside,
                    visible,
                }
            } else {
                let u = (offset * inverse_half).clamp(-1.0, 1.0);
                Fragment {
                    depth,
                    light: body_light,
                    // The real thing: the world-space normal of the raised
                    // cross-section. This is what the old pseudo-cylindrical
                    // term was imitating, and the difference is that this one
                    // knows which way the blade points.
                    normal: frame.across(u, stroke.ridge),
                    tone: stroke.tone,
                    top,
                    along,
                    maturity: stroke.maturity,
                    underside,
                    visible,
                }
            };

            self.plot(point.x, point.y, fragment);
        }
    }
}

impl Painter<'_> {
    /// Draw every mark in a scene, in the order the scene holds them.
    ///
    /// On the painter rather than on the scene, and the direction matters: a
    /// scene is a description that several renderers consume, and a `draw`
    /// method on it would make one of those renderers the scene's own. The scene
    /// crate would then have to know what a `Surface` is.
    pub fn draw_scene(&mut self, scene: &GrassScene) {
        for mark in &scene.marks {
            self.draw(mark);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::painter::Painter;
    use crate::surface::Surface;
    use glam::{Vec2, Vec3};
    use terrain_generators::geometry::TipProfile;
    use terrain_generators::stroke::Stroke;

    use super::*;

    fn page(width: usize, height: usize) -> (Surface, Vec2) {
        (
            Surface::new(width, height),
            Vec2::new(-(width as f32) / 2.0, -(height as f32) * 0.75),
        )
    }

    #[test]
    fn a_stroke_marks_the_page() {
        let (mut surface, origin) = page(64, 64);
        let mut painter = Painter::new(
            &mut surface,
            origin,
            Vec3::new(-0.4, -0.4, 0.82).normalize(),
        );
        painter.draw(&Stroke {
            root: painter.to_ground(Vec2::new(96.0, 140.0)).extend(0.0),
            ..Default::default()
        });
        let heights = surface.height_map(64, 64);
        assert!(heights.iter().any(|&h| h > 1.0), "nothing was drawn");
    }

    #[test]
    fn bending_shortens_the_silhouette() {
        // The property that makes this a projected curve rather than a sheared
        // sprite. Two blades of equal length, one upright and one laid over,
        // must not reach the same height on screen.
        let measure = |bend: f32| {
            let (mut surface, origin) = page(64, 64);
            let mut painter = Painter::new(&mut surface, origin, Vec3::Z);
            let root = painter.to_ground(Vec2::new(96.0, 160.0)).extend(0.0);
            painter.draw(&Stroke {
                root,
                bend,
                length: 0.3,
                ..Default::default()
            });
            let heights = surface.height_map(64, 64);
            heights.iter().cloned().fold(0.0f32, f32::max)
        };
        let upright = measure(0.0);
        let leaning = measure(2.0);
        assert!(
            upright > leaning * 1.3,
            "upright {upright} vs leaning {leaning}"
        );
    }

    #[test]
    fn the_tip_is_brighter_than_the_root() {
        let (mut surface, origin) = page(48, 64);
        let mut painter = Painter::new(&mut surface, origin, Vec3::new(0.0, 0.0, 1.0));
        let root = painter.to_ground(Vec2::new(72.0, 170.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            bend: 0.0,
            length: 0.35,
            tip_light: 0.3,
            ..Default::default()
        });

        let mut brightest_high = 0.0f32;
        let mut brightest_low = 0.0f32;
        for y in 0..surface.height {
            for x in 0..surface.width {
                let index = surface.index(x, y);
                if surface.top_at(index) <= 0.0 {
                    continue;
                }
                let (light, _) = surface.pixel(index);
                if surface.top_at(index) > 24.0 {
                    brightest_high = brightest_high.max(light);
                } else if surface.top_at(index) < 6.0 {
                    brightest_low = brightest_low.max(light);
                }
            }
        }
        assert!(
            brightest_high > brightest_low + 0.1,
            "{brightest_high} vs {brightest_low}"
        );
    }

    #[test]
    fn a_stroke_off_the_page_does_not_wrap_onto_it() {
        // `as` truncates toward zero, so a negative page coordinate would land
        // on column zero. That reads as a bright vertical smear down the page
        // edge, and it is the sort of thing that only shows up once the field
        // is tiled.
        let (mut surface, origin) = page(32, 32);
        let mut painter = Painter::new(&mut surface, origin, Vec3::Z);
        let root = painter.to_ground(Vec2::new(-40.0, 48.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            ..Default::default()
        });
        let heights = surface.height_map(32, 32);
        for y in 0..32 {
            assert_eq!(heights[y * 32], 0.0, "row {y} picked up a wrapped stroke");
        }
    }

    /// The span of recorded normals across a blade, and how much page it covered.
    ///
    /// Normals rather than light. The rib stopped computing a lambert term when
    /// the surface became a G-buffer — shading is [`crate::bake::resolve`]'s job
    /// now — so what the rasteriser is responsible for is recording *which way
    /// the surface faces*, and that is what this measures.
    fn measure(twist: f32) -> (f32, f32) {
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let (mut surface, origin) = page(64, 64);
        let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
        let root = painter.to_ground(Vec2::new(96.0, 150.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            azimuth: 0.4,
            bend: 0.35,
            length: 0.3,
            width: 3.0,
            twist,
            side_light: 0.5,
            under: 0.0,
            ..Default::default()
        });
        // How far apart the two most opposed normals on the blade are, and how
        // much of the page it covered.
        let mut normals = Vec::new();
        let mut count = 0.0f32;
        for index in 0..surface.width * surface.height {
            if surface.top_at(index) <= 0.0 {
                continue;
            }
            normals.push(surface.normal_at(index));
            count += 1.0;
        }
        let mut spread = 0.0f32;
        for (step, a) in normals.iter().enumerate().step_by(7) {
            for b in normals.iter().skip(step).step_by(11) {
                spread = spread.max(1.0 - a.dot(*b));
            }
        }
        (spread, count)
    }

    #[test]
    fn a_flat_blade_still_faces_somewhere_definite() {
        // A ridge of zero must give one normal everywhere, not a degenerate one.
        let (mut surface, origin) = page(64, 64);
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
        let root = painter.to_ground(Vec2::new(96.0, 150.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            bend: 0.3,
            length: 0.3,
            width: 3.0,
            ridge: 0.0,
            under: 0.0,
            ..Default::default()
        });
        for index in 0..surface.width * surface.height {
            if surface.top_at(index) <= 0.0 {
                continue;
            }
            let normal = surface.normal_at(index);
            assert!(
                (normal.length() - 1.0).abs() < 0.03,
                "a recorded normal is {normal:?}"
            );
        }
    }

    #[test]
    fn a_forked_blade_paints_more_than_a_pointed_one() {
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let paint = |tip: TipProfile| {
            let (mut surface, origin) = page(96, 96);
            let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
            let root = painter.to_ground(Vec2::new(144.0, 240.0)).extend(0.0);
            painter.draw(&Stroke {
                root,
                bend: 0.3,
                length: 0.34,
                width: 3.0,
                tip,
                ..Default::default()
            });
            surface.canopy_coverage(96, 96, false).iter().sum::<f32>()
        };
        let pointed = paint(TipProfile::Pointed);
        let forked = paint(TipProfile::Forked {
            split_at: 0.78,
            opening: 0.35,
            long: 0.3,
            short: 0.18,
        });
        assert!(
            forked > pointed,
            "the fork added no paint: {forked} vs {pointed}"
        );
    }

    #[test]
    fn a_forks_children_start_where_the_parent_stopped() {
        // The property that makes it one blade separating rather than two glued
        // on: there must be no gap in the paint at the split.
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let (mut surface, origin) = page(96, 96);
        let mut painter = Painter::at_scale(&mut surface, origin, light, iso::PX_PER_METRE);
        let root = painter.to_ground(Vec2::new(144.0, 250.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            azimuth: 0.0,
            bend: 0.2,
            length: 0.36,
            width: 3.5,
            tip: TipProfile::Forked {
                split_at: 0.75,
                opening: 0.30,
                long: 0.30,
                short: 0.20,
            },
            ..Default::default()
        });
        // Walk up the painted column and check there is no empty row between the
        // lowest and highest paint — a gap is a fork whose children begin
        // somewhere other than where the parent ended.
        let painted = surface.canopy_coverage(96, 96, false);
        let rows: Vec<bool> = (0..96)
            .map(|y| (0..96).any(|x| painted[y * 96 + x] > 0.0))
            .collect();
        let first = rows.iter().position(|p| *p).expect("nothing painted");
        let last = rows.iter().rposition(|p| *p).unwrap();
        assert!(
            rows[first..=last].iter().all(|p| *p),
            "the fork left a gap between rows {first} and {last}"
        );
    }

    #[test]
    fn a_fork_too_small_to_draw_becomes_a_notch() {
        // A page baked for a distant camera cannot resolve a fork, and two
        // subpixel children flicker independently as the ground slides under
        // the sampling grid. This is the collapse, exercised through the
        // rasteriser rather than only through `TipProfile`.
        let light = Vec3::new(-0.42, -0.40, 0.81).normalize();
        let mut surface = Surface::new(64, 64);
        // An eighth of the authoring scale: a fork child is well under a pixel.
        let mut painter =
            Painter::at_scale(&mut surface, Vec2::ZERO, light, iso::PX_PER_METRE * 0.125);
        let root = painter.to_ground(Vec2::new(96.0, 120.0)).extend(0.0);
        painter.draw(&Stroke {
            root,
            length: 0.3,
            tip: TipProfile::Forked {
                split_at: 0.8,
                opening: 0.4,
                long: 0.22,
                short: 0.12,
            },
            ..Default::default()
        });
        // It still draws something — collapsing must not delete the blade.
        assert!(
            surface
                .canopy_coverage(64, 64, false)
                .iter()
                .any(|p| *p > 0.0)
        );
    }
    #[test]
    fn a_blade_records_normals_that_face_different_ways_across_its_width() {
        // The gate for the whole phase, at the level the rasteriser is
        // responsible for. The old lateral term was a screen-space fudge and
        // produced a lit edge that never moved when the key did; a real
        // cross-section records genuinely opposed normals, and *that* is what
        // makes the resolve able to swap which side is lit.
        let (spread, painted) = measure(0.0);
        assert!(painted > 0.0, "nothing was drawn");
        assert!(
            spread > 0.2,
            "the blade's normals span only {spread}, so it shades flat across \
             its width whatever the sun does"
        );
    }
}

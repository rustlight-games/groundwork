//! The cheap tier, over a generic `TerrainScene`.
//!
//! Takes a scene and its field stack and produces a picture. It never
//! constructs a `WorldField` or a `GrassScene`, never scatters anything, and
//! never asks the terrain a question — everything it draws was decided by the
//! compiler, which is what makes this picture and the path-traced one two
//! renders of *one* meadow rather than two meadows.
//!
//! ## Ground first, then marks, then the sky showing through
//!
//! ```text
//! for each pixel        unproject to the ground plane
//!                       realise the substrate there      ← the same function
//!                       blend the substrate colours          the compiler used
//!                       shade by the ground normal
//! for each mark         tessellate, project, depth-test
//! finally               resolve depth, composite, write alpha
//! ```
//!
//! The ground pass calls [`terrain_generators::transition::realise`] rather than
//! reading the substrate planes directly, and that is not an optimisation — it
//! is the whole reason the boundary works. The compiler decided which tufts
//! survive by asking that function; if the ground shading asked a *different*
//! question it would paint the mud in a slightly different place from where the
//! grass thinned, and the transition would read as two unrelated effects that
//! nearly line up.
//!
//! ## Supersampled, because a blade is about a pixel wide
//!
//! At the default framing a grass blade is roughly one pixel across, so an
//! unsupersampled raster turns every blade into a hard-edged stripe and the
//! canopy into aliasing. The scene is rasterised at a multiple and box-filtered
//! down, which is the cheapest thing that makes a blade read as a blade.
//!
//! ## Depth, not painter order, resolves overlap
//!
//! The marks arrive in painter order and it is not what decides the picture: a
//! depth buffer does, using exactly the depth
//! [`terrain_scene::projection::Projection::depth`] gives, so that this
//! rasteriser and a GPU pipeline resolve two overlapping blades the same way.
//! Painter order exists to break ties and to keep strata in the right
//! relationship.

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use terrain_core::coords::WorldPoint;
use terrain_core::ids::MaterialIndex;
use terrain_generators::transition::{TransitionProfile, realise};
use terrain_scene::field::TerrainFieldStack;
use terrain_scene::mark::{
    AnalyticMark, CurveMark, MarkAttributes, RibbonMark, SceneMark, TipShape, WidthProfile,
};
use terrain_scene::projection::{Projection, ScenePoint, ScreenPoint};
use terrain_scene::scene::TerrainScene;

/// The version this renderer stamps on its output.
pub const GENERIC_RASTER_VERSION: u32 = 1;

/// How the cheap tier is asked to draw.
#[derive(Clone, Debug)]
pub struct RasterProfile {
    /// Samples per output pixel on each axis.
    pub supersample: u32,
    /// Direction the sun comes *from*, as a unit vector in scene space.
    pub sun: [f32; 3],
    /// Colour and intensity of the key light.
    pub sun_colour: [f32; 3],
    /// Ambient fill, which is what keeps shadowed grass from going black.
    pub ambient: [f32; 3],
    /// The transition profile the compiler used.
    ///
    /// Passed in rather than defaulted, because the ground shading and the
    /// candidate ownership have to agree and the compiler is the one that knows.
    pub transition: TransitionProfile,
    /// The document's root seed, for the same reason.
    pub root_seed: u64,
}

impl Default for RasterProfile {
    fn default() -> Self {
        Self {
            supersample: 2,
            // High and to the left, which is where both reference plates put it
            // — and a low sun is what makes clods cast the shadows that give
            // bare ground its relief.
            sun: normalise([-0.55, -0.45, 0.70]),
            sun_colour: [1.24, 1.16, 0.98],
            ambient: [0.26, 0.31, 0.36],
            transition: TransitionProfile::default(),
            root_seed: 0,
        }
    }
}

fn normalise(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length <= 1.0e-6 {
        return [0.0, 0.0, 1.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

/// A finished picture, with whatever structural passes were asked for.
pub struct RenderBundle {
    pub width: usize,
    pub height: usize,
    /// Linear RGB, row-major.
    pub colour: Vec<Vec3>,
    /// Coverage, `0..1`. What makes the plate a diamond on nothing.
    pub alpha: Vec<f32>,
    /// Height of the visible surface above the datum, metres.
    pub height_m: Vec<f32>,
}

impl RenderBundle {
    /// Encode as 8-bit RGBA, gamma-corrected.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 4);
        for (colour, alpha) in self.colour.iter().zip(self.alpha.iter()) {
            for channel in 0..3 {
                // sRGB-ish. The picture is graded by eye against the reference
                // plates, which are sRGB, so the transfer curve has to match or
                // every comparison is off by a gamma.
                let linear = colour[channel].max(0.0);
                let encoded = if linear <= 0.0031308 {
                    linear * 12.92
                } else {
                    1.055 * linear.powf(1.0 / 2.4) - 0.055
                };
                out.push((encoded.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            out.push((alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        out
    }
}

/// One surface sample under construction.
#[derive(Clone, Copy)]
struct Fragment {
    depth: f64,
    colour: Vec3,
    height_m: f32,
    /// Whether anything at all was drawn here.
    covered: bool,
    /// Whether what won this sample is part of what the picture is *of*.
    ///
    /// The halo is drawn and then not counted: a blade rooted outside the layout
    /// still occludes and shades inward, but it is not in the silhouette. The
    /// bit follows the *winning* fragment rather than being sticky, so a halo
    /// blade in front of an inside one leaves a blade-wide gap — which against a
    /// transparent background reads as a gap between blades, because it is one.
    inside: bool,
}

impl Fragment {
    const EMPTY: Self = Self {
        depth: f64::NEG_INFINITY,
        colour: Vec3::ZERO,
        height_m: 0.0,
        covered: false,
        inside: false,
    };
}

/// Render a scene.
pub fn render_scene(
    scene: &TerrainScene,
    fields: &TerrainFieldStack,
    profile: &RasterProfile,
) -> RenderBundle {
    let width = scene.request.output_size[0].max(1) as usize;
    let height = scene.request.output_size[1].max(1) as usize;
    let ss = profile.supersample.clamp(1, 4) as usize;
    let (sw, sh) = (width * ss, height * ss);

    let viewport = scene.request.viewport;
    let projection = scene.request.projection;
    let visible = scene.request.bounds;
    // Screen metres per sample, and the screen point of sample (0, 0). Rows run
    // *down* the raster and `+y` runs up the screen, so the vertical step is
    // negative — the one conversion between the two conventions, done once.
    let step_x = viewport.width_m() / sw as f64;
    let step_y = viewport.height_m() / sh as f64;
    let origin = ScreenPoint::new(
        viewport.min.x_m + step_x * 0.5,
        viewport.max.y_m - step_y * 0.5,
    );
    let to_screen = |x: usize, y: usize| {
        ScreenPoint::new(
            origin.x_m + x as f64 * step_x,
            origin.y_m - y as f64 * step_y,
        )
    };

    // ---- Ground pass -------------------------------------------------------
    let mut buffer: Vec<Fragment> = (0..sh)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..sw).map(move |x| {
                let screen = to_screen(x, y);
                shade_ground(screen, projection, fields, visible, profile)
            })
        })
        .collect();

    // ---- Mark pass ---------------------------------------------------------
    // Serial over marks, because they write a shared depth buffer and the whole
    // point of the buffer is that overlap resolves by depth rather than by who
    // got there first. Marks are cheap; the ground pass is where the time goes.
    let mut raster = Raster {
        buffer: &mut buffer,
        width: sw,
        height: sh,
        origin,
        step: (step_x, step_y),
        projection,
        profile,
        inside: true,
    };
    for mark in &scene.marks {
        let root = mark.root();
        raster.inside = visible.contains(WorldPoint::new(root.u_m, root.v_m));
        match mark {
            SceneMark::Ribbon(ribbon) => raster.ribbon(ribbon),
            SceneMark::Curve(curve) => raster.curve(curve),
            SceneMark::Analytic(analytic) => raster.analytic(analytic),
            // Stamps need an image decoder, which this tier does not have.
            _ => {}
        }
    }

    // ---- Shadow pass -------------------------------------------------------
    cast_shadows(&mut buffer, sw, sh, projection, (step_x, step_y), profile);

    // ---- Resolve -----------------------------------------------------------
    let inverse = 1.0 / (ss * ss) as f32;
    let mut colour = vec![Vec3::ZERO; width * height];
    let mut alpha = vec![0.0f32; width * height];
    let mut height_m = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut sum = Vec3::ZERO;
            let mut coverage = 0.0f32;
            let mut top = 0.0f32;
            for sy in 0..ss {
                for sx in 0..ss {
                    let fragment = buffer[(y * ss + sy) * sw + (x * ss + sx)];
                    if fragment.covered && fragment.inside {
                        sum += fragment.colour;
                        coverage += 1.0;
                        top = top.max(fragment.height_m);
                    }
                }
            }
            let index = y * width + x;
            // Divided by the *covered* count rather than by the sample count,
            // so a partially covered pixel keeps its colour and expresses its
            // partialness through alpha. Dividing by the sample count would
            // premultiply, and a network trained on premultiplied targets
            // learns that partially covered grass is intrinsically darker.
            colour[index] = if coverage > 0.0 {
                sum / coverage
            } else {
                Vec3::ZERO
            };
            alpha[index] = coverage * inverse;
            height_m[index] = top;
        }
    }

    RenderBundle {
        width,
        height,
        colour,
        alpha,
        height_m,
    }
}

/// The colour of the ground under one screen sample.
fn shade_ground(
    screen: ScreenPoint,
    projection: Projection,
    fields: &TerrainFieldStack,
    visible: terrain_core::coords::WorldRect,
    profile: &RasterProfile,
) -> Fragment {
    // The ground plane at z = 0, then the height read at that point. Exact only
    // for flat ground, which is what this framework currently has — and the
    // approximation is stated here rather than hidden, because it is the first
    // thing that has to change when elevation arrives.
    let ground = projection.unproject_ground(screen);
    if !fields.grid.bounds().contains(ground) {
        return Fragment::EMPTY;
    }
    // A rectangle of ground projects to a *diamond*, so testing the visible
    // rectangle here is exactly what gives the plate its isometric silhouette.
    // The generated bounds are larger — that is the halo — and drawing ground
    // out there would make the plate a rectangle with a meadow in it.
    let inside = visible.contains(ground);

    let z = fields.surface_height(ground);

    // The same realisation the compiler used to decide ownership. One function,
    // called by both, or the mud is painted somewhere other than where the
    // grass thinned.
    let mut weights: Vec<(MaterialIndex, f32)> = Vec::new();
    fields.substrate_weights_into(ground, &mut weights);
    let substrate = realise(
        weights.iter().copied(),
        ground,
        &profile.transition,
        profile.root_seed,
    );

    let mut albedo = Vec3::ZERO;
    let mut total = 0.0f32;
    for (material, weight) in substrate.iter() {
        albedo += substrate_colour(material, ground, fields) * weight;
        total += weight;
    }
    if total <= 0.0 {
        return Fragment::EMPTY;
    }
    albedo /= total;

    let normal = fields.ground_normal(ground);
    let lit = shade(Vec3::from_array(normal), albedo, profile, 1.0);

    Fragment {
        depth: projection.depth(ScenePoint::new(ground.u_m, ground.v_m, z as f64)),
        colour: lit,
        height_m: z,
        covered: true,
        inside,
    }
}

/// The base colour of one substrate at a point.
///
/// Keyed by material index rather than by a stable appearance key, which is a
/// deliberate placeholder: the renderer-side palette belongs in the document's
/// `appearance` bindings and reading it here would be inventing a second one.
/// What matters for now is that the two substrates differ in the way the
/// reference plates differ — a cool, dark, damp earth and a warm dry one — and
/// that moisture darkens both.
fn substrate_colour(material: MaterialIndex, at: WorldPoint, fields: &TerrainFieldStack) -> Vec3 {
    // Moisture and compaction come from the matrix, so an author painting a damp
    // hollow gets a damp hollow without touching the renderer.
    let wetness = fields
        .derived
        .flow_accumulation
        .as_ref()
        .map(|plane| {
            let value = plane.sample(&fields.grid, at);
            (value / (value + 0.4)).clamp(0.0, 1.0)
        })
        .unwrap_or(0.4);

    let base = match material.0 {
        // Meadow soil: dark, cool, and mostly hidden by what grows on it.
        0 => Vec3::new(0.062, 0.072, 0.038),
        // Compacted dirt. Graded against `grass_to_mud_bumpy.jpg`, which is a
        // warm mid-brown — noticeably darker and less grey than a first guess,
        // because bare earth in daylight is much darker than it looks beside a
        // bright sky.
        1 => Vec3::new(0.150, 0.101, 0.062),
        _ => Vec3::new(0.12, 0.11, 0.09),
    };
    // Wet ground is darker and slightly more saturated, which is the tonal
    // sweep across the mud in both plates.
    base * (1.0 - 0.42 * wetness)
}

/// Lambert plus a wrapped fill.
///
/// Wrapped rather than clamped, because grass is thin and translucent: a blade
/// facing away from the sun is not black, it is lit through. A hard `max(0, n·l)`
/// is what makes procedural grass read as plastic.
fn shade(normal: Vec3, albedo: Vec3, profile: &RasterProfile, translucency: f32) -> Vec3 {
    let sun = Vec3::from_array(profile.sun);
    let raw = normal.dot(sun);
    let wrapped = ((raw + translucency) / (1.0 + translucency)).clamp(0.0, 1.0);
    let key = Vec3::from_array(profile.sun_colour) * wrapped;
    // A little more ambient from above than below, which stands in for a sky
    // dome without integrating one.
    let sky = 0.5 + 0.5 * normal.z;
    let fill = Vec3::from_array(profile.ambient) * sky;
    albedo * (key + fill)
}

/// The rasteriser's working state.
struct Raster<'a> {
    buffer: &'a mut [Fragment],
    width: usize,
    height: usize,
    origin: ScreenPoint,
    step: (f64, f64),
    projection: Projection,
    profile: &'a RasterProfile,
    /// Whether the mark being drawn is part of the picture or of the halo.
    inside: bool,
}

impl Raster<'_> {
    /// Where a scene point lands, in sample coordinates.
    fn to_sample(&self, point: ScenePoint) -> Vec2 {
        let screen = self.projection.project(point);
        Vec2::new(
            ((screen.x_m - self.origin.x_m) / self.step.0) as f32,
            ((self.origin.y_m - screen.y_m) / self.step.1) as f32,
        )
    }

    /// Write one sample if it is nearer than what is there.
    fn plot(&mut self, x: i64, y: i64, depth: f64, colour: Vec3, height_m: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let slot = &mut self.buffer[y as usize * self.width + x as usize];
        if depth >= slot.depth {
            slot.depth = depth;
            slot.colour = colour;
            slot.height_m = height_m;
            slot.covered = true;
            slot.inside = self.inside;
        }
    }

    /// Fill a screen-space quad with one colour and a per-corner depth.
    fn quad(&mut self, corners: [Vec2; 4], depth: f64, colour: Vec3, height_m: f32) {
        let min_x = corners
            .iter()
            .fold(f32::INFINITY, |a, c| a.min(c.x))
            .floor() as i64;
        let max_x = corners
            .iter()
            .fold(f32::NEG_INFINITY, |a, c| a.max(c.x))
            .ceil() as i64;
        let min_y = corners
            .iter()
            .fold(f32::INFINITY, |a, c| a.min(c.y))
            .floor() as i64;
        let max_y = corners
            .iter()
            .fold(f32::NEG_INFINITY, |a, c| a.max(c.y))
            .ceil() as i64;
        // A degenerate or absurd quad is dropped rather than scanned: a mark
        // whose geometry went wrong should cost nothing, not a full-screen fill.
        if max_x - min_x > self.width as i64 || max_y - min_y > self.height as i64 {
            return;
        }
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let point = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                if point_in_quad(point, &corners) {
                    self.plot(x, y, depth, colour, height_m);
                }
            }
        }
    }

    /// A tapered ribbon: a blade, a leaf, a strap.
    fn ribbon(&mut self, mark: &RibbonMark) {
        let g = &mark.geometry;
        if !(g.length_m.is_finite() && g.length_m > 0.0) {
            return;
        }
        // Segments by projected length, so a blade near the camera gets the
        // subdivision it needs and a distant one does not pay for it.
        let pixels = (g.length_m as f64 / self.step.1).abs();
        let segments = (pixels * 0.5).clamp(3.0, 24.0) as usize;

        let root = mark.root;
        let azimuth = g.azimuth_rad;
        let (mut previous, mut previous_width, mut previous_normal) = (None, 0.0f32, Vec3::Z);

        for step in 0..=segments {
            let t = step as f32 / segments as f32;
            let (point, tangent) = ribbon_point(root, g, t);
            let half = half_width(g, t);
            // The blade's face turns about its own axis along its length, which
            // is what stops every blade in a tuft catching the highlight in the
            // same place.
            let twist = g.twist_rad * t;
            let face = Vec3::new(
                -(azimuth + twist).sin() * tangent.z.abs().max(0.25),
                (azimuth + twist).cos() * tangent.z.abs().max(0.25),
                // A midrib: the centre stands proud, so the blade catches light
                // along its length rather than as a flat strip.
                0.35 + 0.65 * g.ridge,
            )
            .normalize_or_zero();

            if let Some(previous_point) = previous {
                self.ribbon_segment(
                    previous_point,
                    point,
                    previous_width,
                    half,
                    previous_normal,
                    face,
                    mark.attributes,
                    mark.material.0,
                );
            }
            previous = Some(point);
            previous_width = half;
            previous_normal = face;
        }

        // A forked tip continues past the parent rather than replacing it.
        if let TipShape::Forked {
            split_at,
            opening_rad,
            long,
            short,
        } = g.tip
        {
            for (side, extra) in [(1.0f32, long), (-1.0f32, short)] {
                let mut forked = *g;
                forked.length_m = g.length_m * extra;
                forked.azimuth_rad = azimuth + side * opening_rad;
                forked.bend_rad = g.bend_rad;
                let (base, _) = ribbon_point(root, g, split_at.clamp(0.0, 1.0));
                let mut child = *mark;
                child.geometry = forked;
                child.root = base;
                // Recursion depth one: a fork's children are plain ribbons, so
                // this cannot run away.
                child.geometry.tip = TipShape::Pointed;
                self.ribbon(&child);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn ribbon_segment(
        &mut self,
        from: ScenePoint,
        to: ScenePoint,
        from_half: f32,
        to_half: f32,
        from_normal: Vec3,
        to_normal: Vec3,
        attributes: MarkAttributes,
        material: u16,
    ) {
        let a = self.to_sample(from);
        let b = self.to_sample(to);
        let along = (b - a).normalize_or_zero();
        if along.length_squared() <= 0.0 {
            return;
        }
        let across = Vec2::new(-along.y, along.x);
        // Widths are in world metres; the screen scale is the same on both axes
        // for the horizontal component of an area-preserving projection.
        let scale = (1.0 / self.step.0) as f32;
        // At least half a sample wide, or a blade thinner than the raster
        // vanishes in stripes instead of thinning smoothly.
        let wa = (from_half * scale).max(0.35);
        let wb = (to_half * scale).max(0.35);

        let corners = [
            a + across * wa,
            b + across * wb,
            b - across * wb,
            a - across * wa,
        ];
        let depth = self.projection.depth(from).max(self.projection.depth(to));
        let normal = ((from_normal + to_normal) * 0.5).normalize_or_zero();
        let colour = shade(
            normal,
            appearance_albedo(material, attributes),
            self.profile,
            translucency_of(material),
        );
        self.quad(corners, depth, colour, from.z_m.max(to.z_m) as f32);
    }

    /// A round-sectioned curve: a stem, a twig.
    fn curve(&mut self, mark: &CurveMark) {
        let ribbon = RibbonMark {
            stable_id: mark.stable_id,
            order: mark.order,
            material: mark.material,
            root: mark.root,
            geometry: terrain_scene::mark::RibbonGeometry {
                length_m: mark.length_m,
                azimuth_rad: mark.azimuth_rad,
                bend_rad: mark.bend_rad,
                curl_rad: 0.0,
                sway_rad: 0.0,
                kink_rad: 0.0,
                kink_at: 0.5,
                kink_turn_rad: 0.0,
                twist_rad: 0.0,
                width_m: mark.radius_m,
                tip_width_m: mark.tip_radius_m,
                profile: WidthProfile::Stem,
                tip: TipShape::Pointed,
                ridge: 1.0,
            },
            attributes: mark.attributes,
            bounds: mark.bounds,
        };
        self.ribbon(&ribbon);
    }

    /// An analytic shape: a pebble, a clod, a flower head.
    ///
    /// Shaded as a hemisphere rather than filled flat, because these are the
    /// only things in the vocabulary with a smooth silhouette and a flat one
    /// reads as a sticker.
    fn analytic(&mut self, mark: &AnalyticMark) {
        let centre = self.to_sample(mark.centre);
        let scale = (1.0 / self.step.0) as f32;
        let rx = (mark.radius_m[0] * scale).max(0.6);
        // The projection squashes depth by half, so a circle of ground draws as
        // an ellipse and a shape that ignored it would look like a ball sitting
        // on a slope.
        let ry = (mark.radius_m[1] * scale * 0.5).max(0.4);
        let albedo = appearance_albedo(mark.material.0, mark.attributes);

        let (min_x, max_x) = (
            (centre.x - rx).floor() as i64,
            (centre.x + rx).ceil() as i64,
        );
        let (min_y, max_y) = (
            (centre.y - ry).floor() as i64,
            (centre.y + ry).ceil() as i64,
        );
        if max_x - min_x > self.width as i64 || max_y - min_y > self.height as i64 {
            return;
        }

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = (x as f32 + 0.5 - centre.x) / rx;
                let dy = (y as f32 + 0.5 - centre.y) / ry;
                let radial = dx * dx + dy * dy;
                if radial > 1.0 {
                    continue;
                }
                // A dome. The vertical component falls off toward the rim,
                // which is what gives the terminator its curve.
                let up = (1.0 - radial).max(0.0).sqrt();
                let normal = Vec3::new(dx, dy * 0.5, up.max(0.05)).normalize_or_zero();
                let rise = mark.height_m * up;
                let point = ScenePoint::new(
                    mark.centre.u_m,
                    mark.centre.v_m,
                    mark.centre.z_m + rise as f64,
                );
                let colour = shade(normal, albedo, self.profile, 0.15);
                self.plot(x, y, self.projection.depth(point), colour, point.z_m as f32);
            }
        }
    }
}

/// A point on a ribbon's centreline, and the direction it is heading.
fn ribbon_point(
    root: ScenePoint,
    g: &terrain_scene::mark::RibbonGeometry,
    t: f32,
) -> (ScenePoint, Vec3) {
    // The bend is spent along the arc rather than all at the root, and the curl
    // is concentrated in the last third — which is what makes a blade hook over
    // instead of describing a circle.
    let curl = g.curl_rad * (t * t * t);
    let kink = if t > g.kink_at { g.kink_rad } else { 0.0 };
    let angle = g.bend_rad * t + curl + kink;
    let azimuth =
        g.azimuth_rad + g.sway_rad * t + if t > g.kink_at { g.kink_turn_rad } else { 0.0 };

    // Integrated coarsely: the arc is short and the error is far below a pixel.
    let steps = 8;
    let (mut u, mut v, mut z) = (0.0f32, 0.0f32, 0.0f32);
    for step in 0..steps {
        let s = (step as f32 + 0.5) / steps as f32 * t;
        let a = g.bend_rad * s
            + g.curl_rad * (s * s * s)
            + if s > g.kink_at { g.kink_rad } else { 0.0 };
        let az = g.azimuth_rad + g.sway_rad * s + if s > g.kink_at { g.kink_turn_rad } else { 0.0 };
        let ds = g.length_m / steps as f32;
        u += a.sin() * az.cos() * ds;
        v += a.sin() * az.sin() * ds;
        z += a.cos() * ds;
    }

    let tangent = Vec3::new(
        angle.sin() * azimuth.cos(),
        angle.sin() * azimuth.sin(),
        angle.cos(),
    );
    (
        ScenePoint::new(
            root.u_m + u as f64,
            root.v_m + v as f64,
            root.z_m + z as f64,
        ),
        tangent,
    )
}

/// Half-width at a fraction along a ribbon.
fn half_width(g: &terrain_scene::mark::RibbonGeometry, t: f32) -> f32 {
    let shape = match g.profile {
        WidthProfile::Tapered => 1.0 - t,
        WidthProfile::Oval => (std::f32::consts::PI * t).sin(),
        WidthProfile::Stem => 1.0 - 0.25 * t,
        // Narrow where it attaches, broadest a third of the way up, then a long
        // taper. What actual grass does.
        WidthProfile::Leaf => {
            let rise = (t / 0.33).clamp(0.0, 1.0);
            let fall = 1.0 - ((t - 0.33) / 0.67).clamp(0.0, 1.0);
            (0.55 + 0.45 * rise) * (0.15 + 0.85 * fall * fall.sqrt())
        }
        _ => 1.0 - t,
    };
    (g.width_m * shape).max(g.tip_width_m * (1.0 - t) + g.tip_width_m * 0.25)
}

/// How much light passes through a material.
fn translucency_of(material: u16) -> f32 {
    match material {
        // Grass and leaves are thin and lit through.
        0 | 2 => 0.55,
        1 => 0.45,
        3 => 0.25,
        // Stone and soil are not.
        _ => 0.10,
    }
}

/// The albedo of one appearance, drifted by the mark's own tint.
///
/// Indexed by the scene's binding table order, which the compiler fills in
/// recipe order — see `terrain_generators::families`. A placeholder palette
/// living here rather than in the document is the one piece of this renderer
/// that should move once appearances carry their own colour.
fn appearance_albedo(material: u16, attributes: MarkAttributes) -> Vec3 {
    let base = match material {
        // plant.grass_blade — the vivid yellow-green of a lit meadow. Graded
        // against the reference plates, which are far more saturated than a
        // plausible-looking olive: lit grass is a strong green, and the olive
        // that procedural meadows drift toward is the average of a lit blade
        // and a shaded one rather than the colour of either.
        0 => Vec3::new(0.098, 0.275, 0.030),
        // plant.grass_dry — straw.
        1 => Vec3::new(0.310, 0.238, 0.078),
        // plant.broad_leaf — cooler and flatter than a blade.
        2 => Vec3::new(0.072, 0.205, 0.048),
        // plant.thatch — the dull mat between green and bare.
        3 => Vec3::new(0.118, 0.092, 0.046),
        // flower.stem
        4 => Vec3::new(0.090, 0.190, 0.045),
        // flower.head
        5 => Vec3::new(0.780, 0.720, 0.330),
        // rock.granite
        6 => Vec3::new(0.205, 0.200, 0.190),
        // soil.clod
        7 => Vec3::new(0.140, 0.096, 0.058),
        // soil.grit
        8 => Vec3::new(0.175, 0.128, 0.082),
        _ => Vec3::new(0.15, 0.15, 0.15),
    };

    // A per-mark drift within the material's own family, so a meadow is not one
    // colour. Hue rather than brightness: drifting brightness alone reads as
    // noise, and drifting hue reads as different plants.
    let tint = attributes.tint.clamp(-1.0, 1.0);
    let drifted = Vec3::new(
        base.x * (1.0 + 0.28 * tint),
        base.y * (1.0 + 0.10 * tint),
        base.z * (1.0 - 0.20 * tint),
    );
    // Older growth is duller; wet ground darkens what stands on it.
    let maturity = 0.88 + 0.24 * attributes.maturity;
    drifted * maturity * (1.0 - 0.15 * attributes.moisture)
}

/// Whether a point is inside a convex quad given in order.
fn point_in_quad(point: Vec2, corners: &[Vec2; 4]) -> bool {
    let mut sign = 0.0f32;
    for index in 0..4 {
        let a = corners[index];
        let b = corners[(index + 1) % 4];
        let cross = (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x);
        if cross.abs() < 1.0e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// How far a shadow ray marches, in metres.
///
/// Twenty-five centimetres. This is contact shadow — the dark under a tuft, the
/// shadow a clod throws across the ground beside it — not landscape shadow, and
/// those are the two things that give bare ground its relief under a low sun.
const SHADOW_REACH_M: f64 = 0.10;

/// How many samples along the ray.
const SHADOW_STEPS: usize = 12;

/// How much a shadowed sample keeps.
const SHADOW_FLOOR: f32 = 0.52;

/// Darken samples that cannot see the sun.
///
/// Screen space, marching the height buffer toward the light. That is an
/// approximation with a known failure — it cannot shadow from geometry that is
/// off-screen or hidden behind something nearer — and it is the right one here:
/// the alternative is a light-space depth pass over half a million marks, and
/// what this picture needs is the *contact* darkening that makes a canopy read
/// as having depth rather than being a flat field of strokes.
fn cast_shadows(
    buffer: &mut [Fragment],
    width: usize,
    height: usize,
    projection: Projection,
    step: (f64, f64),
    profile: &RasterProfile,
) {
    let sun = profile.sun;
    if sun[2] <= 0.05 {
        return;
    }
    // The screen displacement of one metre travelled toward the sun. The
    // projection is linear, so this is a constant rather than a per-sample
    // transform.
    let screen_x = (sun[0] as f64 - sun[1] as f64) * projection.half_width;
    let screen_y = -(sun[0] as f64 + sun[1] as f64) * projection.half_height
        + sun[2] as f64 * projection.height_scale;
    let per_metre = (screen_x / step.0, -screen_y / step.1);
    let ds = SHADOW_REACH_M / SHADOW_STEPS as f64;

    let heights: Vec<f32> = buffer.iter().map(|f| f.height_m).collect();
    let covered: Vec<bool> = buffer.iter().map(|f| f.covered).collect();

    let shade: Vec<f32> = (0..height)
        .into_par_iter()
        .flat_map_iter(|y| {
            let heights = &heights;
            let covered = &covered;
            (0..width).map(move |x| {
                let index = y * width + x;
                if !covered[index] {
                    return 1.0;
                }
                let origin = heights[index];
                let mut occlusion = 0.0f32;
                for step_index in 1..=SHADOW_STEPS {
                    let distance = step_index as f64 * ds;
                    let sx = x as f64 + per_metre.0 * distance;
                    let sy = y as f64 + per_metre.1 * distance;
                    if sx < 0.0 || sy < 0.0 {
                        break;
                    }
                    let (sx, sy) = (sx as usize, sy as usize);
                    if sx >= width || sy >= height {
                        break;
                    }
                    let probe = sy * width + sx;
                    if !covered[probe] {
                        continue;
                    }
                    // Where the ray is by now, and what is actually there.
                    let ray = origin + (distance * sun[2] as f64) as f32;
                    let blocker = heights[probe] - ray;
                    if blocker > 0.0 {
                        // Nearer blockers cast harder, which is what makes the
                        // shadow under a tuft dark and its far edge soft.
                        let falloff = 1.0 - (step_index as f32 / SHADOW_STEPS as f32);
                        occlusion = occlusion.max((blocker * 55.0).clamp(0.0, 1.0) * falloff);
                    }
                }
                1.0 - (1.0 - SHADOW_FLOOR) * occlusion
            })
        })
        .collect();

    for (fragment, factor) in buffer.iter_mut().zip(shade.iter()) {
        // Shadow tints as well as darkens: less key light means the fill, which
        // is sky-coloured, is proportionally more of what is left. Multiplying
        // all three channels equally is what makes procedural shadow read as
        // grey paint.
        let cool = Vec3::new(0.92, 0.97, 1.06);
        let lerp = *factor;
        fragment.colour = fragment.colour * lerp * (Vec3::ONE * lerp + cool * (1.0 - lerp));
    }
}

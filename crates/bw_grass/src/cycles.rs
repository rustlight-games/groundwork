//! Hand the light transport to a path tracer, and keep the world.
//!
//! This module is the boundary between the two halves of the renderer, and the
//! line it draws is the whole design:
//!
//! | Stays here | Goes to Cycles |
//! |---|---|
//! | Where every blade is, and why | How light reaches it |
//! | Guard bands, seams, page independence | Shadows, occlusion, scattering |
//! | Stable per-call-site random streams | Denoising, sampling, devices |
//! | The world being a pure function of a coordinate | — |
//!
//! **Rust stays the source of truth for placement.** That is not a preference,
//! it is the one property a renderer cannot give back once it is lost: two pages
//! that have never met agree along a shared edge only because every placement
//! decision is a pure function of a world coordinate. Let Blender's own
//! scattering decide where grass goes and the world becomes a finite set of
//! tiles with blend masks, which is a different game.
//!
//! What Cycles gets is an explicit list of curves in world metres. It has no
//! opinion about where they came from and cannot introduce one.
//!
//! ## Why this replaces six thousand lines
//!
//! The renderer this supersedes had five separate terms describing darkness —
//! horizon occlusion, optical occlusion, an interior density, a micro-occlusion
//! and a shade depth — because a rasteriser cannot integrate a hemisphere and
//! every one of those was an approximation of some part of doing so. They
//! interacted, so tuning one moved the others, and a whole phase of work went on
//! subtracting them from each other. A path tracer computes the quantity they
//! were approximating. That is the trade: an afternoon of render time for a
//! category of tuning that stops existing.
//!
//! ## The wire format
//!
//! A directory holding a JSON header and one binary blob per geometry kind.
//! JSON for the header because it is small, human-readable and hand-written here
//! rather than pulling in a serialiser; raw little-endian `f32` for the geometry
//! because a page holds hundreds of thousands of curve points and Python has to
//! read them with one `numpy.fromfile` rather than a loop.
//!
//! ```text
//! scene/
//!   scene.json      the header: camera, sun, render settings, counts
//!   blades.bin      f32 × count × points × 4   (x, y, z, radius)
//!   attributes.bin  f32 × count × 4            (maturity, moisture, tone, exposure)
//!   ground.bin      f32 × (rows × columns)     heights over the footprint AABB
//! ```
//!
//! Every length in the file is **world metres, Z up**. Cache pixels do not cross
//! this boundary; they are an artefact of the rasteriser and the path tracer has
//! no use for them.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use crate::bake::Page;
use crate::field::WorldField;
use crate::iso;
use crate::quality::GrassRenderQuality;
use crate::scene::GrassScene;
use crate::stroke::{BladeSample, walk_blade};

/// How many cross-sections each exported blade carries.
///
/// Seven. A blade is a cubic arc with at most one kink, so its centreline needs
/// few samples; what sets this is that the *silhouette* has to stay smooth under
/// magnification, and six segments is where a bent blade stops showing its
/// corners.
///
/// Fixed rather than per-blade because Python reads the whole buffer with one
/// `numpy.fromfile` and reshapes it. A variable-length format would need a
/// second pass and an index, and would cost more in Python loop time than it
/// saves in bytes.
pub const RIBS_PER_BLADE: usize = 7;

/// The fewest cross-sections a blade may be described with.
///
/// Three — a root, a middle and a tip, which is the least that can still bend.
/// Below that a blade is a straight quad and the silhouette that makes grass
/// read as grass is gone.
pub const MIN_RIBS_PER_BLADE: usize = 3;

/// How finely to describe a blade that will be shown at `px_per_metre`.
///
/// The second half of the wide-view mip, and the half that decides whether the
/// scene fits in memory at all. Thinning the *population* is the obvious lever
/// and it is not enough on its own: at the game's own framing the field still
/// holds twenty-three million blades, and at seven ribs each that is half a
/// billion vertices — which is not a slow render, it is an allocation failure.
///
/// A blade five pixels long does not need seven cross-sections. It needs enough
/// to bend once. So the rib count follows the scale the blade will be seen at,
/// the same way the count does, and for the same reason: describing detail finer
/// than a pixel is not detail.
pub fn ribs_for(px_per_metre: f32) -> usize {
    // The authoring scale gets the full count; halve the scale, lose a rib.
    let ratio = (px_per_metre / iso::PX_PER_METRE).clamp(0.0, 1.0);
    let ribs = (RIBS_PER_BLADE as f32 * ratio.sqrt()).round() as usize;
    ribs.clamp(MIN_RIBS_PER_BLADE, RIBS_PER_BLADE)
}

/// Vertices across one rib: left edge, raised centre, right edge.
///
/// Three, and this is the whole reason blades are exported as a mesh rather than
/// as Cycles curve primitives.
///
/// A Cycles `RIBBONS` curve is a camera-facing quad. Its shading normal is
/// derived to face the viewer, so **every blade in the field presents the same
/// normal to the sun** — the light lands on all of them identically and the
/// canopy shades flat. Measured, that render had seven tenths of a percent of
/// its pixels above the highlight threshold against the target's seven and a
/// half, and no amount of extra light fixes it, because the missing thing is
/// variation rather than brightness. There is no way to override that normal:
/// it is a property of the primitive.
///
/// Three vertices give a real fold. The centre stands proud by
/// [`crate::geometry::RIDGE`] of the half-width, so the two facets face
/// genuinely different directions and one can catch the sun while the other does
/// not — the value break *inside* a single blade that the reference art has
/// everywhere.
pub const VERTICES_PER_RIB: usize = 3;

/// Vertices one exported blade occupies at the full rib count.
pub const VERTICES_PER_BLADE: usize = RIBS_PER_BLADE * VERTICES_PER_RIB;

/// Per-blade attributes carried alongside the geometry.
///
/// These reach the shader as named point attributes, so the material can vary
/// without the geometry changing. Keeping them off the curve rather than baking
/// them into a colour is what lets one export be re-lit and re-shaded.
pub const ATTRIBUTES_PER_BLADE: usize = 4;

/// The scene as Cycles will receive it.
pub struct CyclesScene {
    pub page: Page,
    /// Ribbon vertices: `count × VERTICES_PER_BLADE` of `(x, y, z)`.
    pub points: Vec<[f32; 3]>,
    /// Per-blade `(maturity, moisture, tone, exposure)`.
    pub attributes: Vec<[f32; ATTRIBUTES_PER_BLADE]>,
    /// Ground heights over [`CyclesScene::footprint`], row-major.
    pub ground: Vec<f32>,
    pub ground_rows: usize,
    pub ground_columns: usize,
    /// World-space AABB the ground grid spans.
    pub footprint: (Vec2, Vec2),
    pub camera: Camera,
    pub settings: RenderSettings,
    /// Cross-sections each blade was described with.
    ribs: usize,
}

/// The orthographic camera that reproduces [`crate::iso`] exactly.
///
/// Derived rather than authored, because "looks about right" is not good enough:
/// the page is a texture the game samples under a fixed projection, and a camera
/// half a degree off puts the grass out of register with the tiles it sits on.
///
/// ## The projection is orthogonal but not isotropic
///
/// [`iso::project`] is `screen.x = (X − Y)` and `screen.y = −(X + Y)/2 + Z`.
/// Written as dot products those are the basis vectors `r = (1, −1, 0)` and
/// `u = (−½, −½, 1)`. They are perpendicular — `r · u = 0` — so this really is
/// an orthographic view, and the direction it looks from is `r × u` normalised,
/// which comes out at `(1, 1, 1)/√3`: the true isometric axis, 35.26° above the
/// ground.
///
/// But their *lengths* differ. `|r| = √2` and `|u| = √3/2`, so the projection
/// stretches horizontally by `√2 / √(3/2) = 2/√3 ≈ 1.1547` relative to a rigid
/// orthographic view. That factor is the entire difference between the 2:1
/// dimetric diamond the game draws its tiles as and true isometric.
///
/// A renderer cannot express that with a camera transform, because a transform
/// that scales one screen axis is not a rotation. Blender expresses it as a
/// non-square pixel: [`Camera::pixel_aspect_y`] carries the whole anisotropy,
/// and the camera itself stays a rigid orthographic view down the isometric
/// axis.
///
/// ## And the projection is a mirror, so the world is reflected instead
///
/// The second surprise, and it has to be handled or the sun comes out on the
/// wrong side. Take the basis at face value and `r × u` is `−(1, 1, 1)/√3`,
/// which puts the camera **underneath the ground looking up**. That is not a
/// sign slip in this module; it is a property of the projection.
///
/// Check it against a real overhead view. A camera above `+X+Y+Z` has right
/// vector `(−1, 1, 0)/√2`, so it sees `+X` go left. [`iso::project`] does the
/// opposite: `screen.x = X − Y` sends `+X` right. No rotation turns one into the
/// other, so the game's projection is left-handed — entirely normal for a
/// tile-based isometric game, where the convention is picked to suit the tile
/// grid rather than a physical camera, and self-consistent because everything in
/// the game lives inside it.
///
/// A path tracer cannot be handed a mirrored camera. The fix is to mirror the
/// *world* instead, by reflecting it across the plane `x = y` — which is exactly
/// [`to_blender`], a swap of the two ground axes. With `P' = (Pᵧ, Pₓ, P_z)` and
/// the physical basis above:
///
/// ```text
/// P' · R = (Pₓ − Pᵧ)/√2              iso: screen.x = Pₓ − Pᵧ
/// P' · U = (2P_z − Pₓ − Pᵧ)/√6       iso: screen.y = P_z − (Pₓ + Pᵧ)/2
/// ```
///
/// Both agree exactly, up to the axis lengths [`Camera::ortho_scale`] and
/// [`Camera::pixel_aspect_y`] already carry. So the render needs no flip, no
/// compositor and no post-processing: every point, the ground grid and the sun's
/// bearing go through the same swap on the way out, and the picture arrives in
/// the game's own convention.
///
/// The one rule this creates: **nothing may cross this boundary without
/// [`to_blender`]**. A blade reflected while its sun is not would be lit from
/// the wrong side, and it would look plausible.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Where it sits. Any distance along the view axis gives the same picture
    /// under orthographic projection; this one clears the canopy comfortably.
    pub location: Vec3,
    /// Right, up and backward — the columns of the camera's rotation matrix.
    ///
    /// Right is the *negation* of the projection's horizontal axis. See the
    /// note on the mirror above.
    pub basis: [Vec3; 3],
    /// World metres the horizontal axis of the frame spans.
    pub ortho_scale: f32,
    /// The anisotropy, as a pixel that is taller than it is wide.
    pub pixel_aspect_y: f32,
}

impl Camera {
    /// The camera that photographs `page` exactly as [`iso`] would draw it.
    pub fn for_page(page: &Page, canopy_ceiling: f32) -> Self {
        // A physical right-handed camera above the ground. The world arrives
        // reflected through `to_blender`, which is what makes this basis agree
        // with `iso::project` — see the note on the mirror.
        let right = Vec3::new(-1.0, 1.0, 0.0).normalize();
        let up = Vec3::new(-1.0, -1.0, 2.0).normalize();
        let backward = right.cross(up).normalize();

        // World metres the page spans on each screen axis. `screen.x` is a dot
        // with `r`, whose length is √2, so a screen-metre span of `s` is a world
        // span of `s / √2` along `r̂` — and likewise `√3/2` for the vertical.
        let screen_width = page.width as f32 / page.px_per_metre;
        let screen_height = page.height as f32 / page.px_per_metre;
        let world_width = screen_width / Vec3::new(1.0, -1.0, 0.0).length();
        let world_height = screen_height / Vec3::new(-0.5, -0.5, 1.0).length();

        // Blender derives the vertical extent from the horizontal one as
        // `ortho_scale · (res_y · aspect_y) / (res_x · aspect_x)`. Solving for
        // the aspect that yields `world_height` cancels the resolution entirely,
        // which is the check that this is a property of the projection and not
        // of how big the render happens to be.
        let pixel_aspect_y =
            (world_height * page.width as f32) / (world_width * page.height as f32);

        let centre_pixel = Vec2::new(page.width as f32 * 0.5, page.height as f32 * 0.5);
        let target = to_blender(page.ground_at(centre_pixel).extend(0.0));
        // Far enough back that nothing clips, near enough that the depth range
        // stays usable for the depth pass.
        let distance = 40.0 + canopy_ceiling * 4.0;

        Self {
            location: target + backward * distance,
            basis: [right, up, backward],
            ortho_scale: world_width,
            pixel_aspect_y,
        }
    }
}

/// What to ask the path tracer for.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    pub samples: u32,
    pub denoise: bool,
    /// `"GPU"` or `"CPU"`.
    pub device: String,
    /// Blender view transform. `"Standard"` keeps the render linear-to-sRGB;
    /// `"AgX"` applies Blender's filmic curve.
    ///
    /// Standard by default, and that is a considered choice rather than a
    /// default left alone. AgX desaturates and rolls off highlights to make a
    /// physically-lit render look photographic — but this crate already owns a
    /// measured colour policy, and stacking a second opinion on top of it means
    /// two curves fighting over the same pixels. See [`crate::palette`].
    pub view_transform: String,
    /// Sun elevation above the horizon, radians.
    pub sun_elevation: f32,
    /// Sun bearing in world space, radians, measured from +X toward +Y.
    pub sun_azimuth: f32,
    /// Angular diameter of the sun, radians.
    ///
    /// Three degrees is six times life size, which is the same licence the art
    /// takes elsewhere — a literal half-degree sun puts a hard edge on every
    /// blade shadow and the field fills with black confetti. What it must not
    /// become is *soft*: a wide sun is a second fill light, and fill is what was
    /// flattening the canopy.
    pub sun_angle: f32,
    pub sun_strength: f32,
    pub sun_colour: [f32; 3],
    pub sky_strength: f32,
    pub sky_colour: [f32; 3],
    /// Which AOVs to write beside the beauty pass.
    pub passes: bool,
    /// How many cross-sections each blade is described with.
    ///
    /// Zero means "choose from the page's scale" — see [`ribs_for`].
    pub ribs: usize,
    /// Multiplies every exported curve radius.
    ///
    /// The one honest fudge in the file. Blade half-widths are authored in cache
    /// pixels against a 96-pixel metre, where a botanically correct four
    /// millimetre blade is under half a pixel and simply cannot be drawn — so
    /// the rasteriser's widths grew to what it could rasterise, and carrying
    /// them across unchanged would hand Cycles blades several times too fat.
    /// Cycles has no such floor. See [`crate::cycles`] and the geometry phase.
    pub blade_width: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            samples: 256,
            denoise: true,
            device: "GPU".to_string(),
            view_transform: "Standard".to_string(),
            // The elevation the whole renderer is built around, and the lowest
            // it supports: below this a blade shades ground more than one and a
            // half times its own height away and the guard band grows faster
            // than the page.
            sun_elevation: 35.0f32.to_radians(),
            sun_azimuth: 125.0f32.to_radians(),
            sun_angle: 3.0f32.to_radians(),
            sun_strength: 18.0,
            sun_colour: [1.0, 0.92, 0.72],
            sky_strength: 1.15,
            sky_colour: [0.30, 0.44, 0.72],
            passes: false,
            ribs: 0,
            blade_width: 0.35,
        }
    }
}

impl CyclesScene {
    /// Turn a grown scene into curves a path tracer can trace.
    pub fn build(scene: &GrassScene, field: &WorldField, settings: RenderSettings) -> Self {
        let page = scene.page;
        let ribs = if settings.ribs == 0 {
            ribs_for(page.px_per_metre)
        } else {
            settings.ribs.clamp(MIN_RIBS_PER_BLADE, RIBS_PER_BLADE)
        };
        let mut points = Vec::with_capacity(scene.marks.len() * ribs * VERTICES_PER_RIB);
        let mut attributes = Vec::with_capacity(scene.marks.len());

        // How finely to walk a centreline. The rasteriser asks for ribs per
        // *pixel* because it is filling pixels; here the only requirement is
        // that the fixed point budget spans the blade, so the walk is asked for
        // roughly that many samples and then resampled to exactly that many.
        for mark in &scene.marks {
            let arc = mark.length * page.px_per_metre;
            let mut walked: Vec<BladeSample> = Vec::with_capacity(RIBS_PER_BLADE * 6);
            walk_blade(
                mark,
                arc.max(4.0),
                // Enough samples that the resample below is interpolating a
                // dense polyline rather than inventing points between sparse
                // ones.
                (RIBS_PER_BLADE as f32 * 3.0) / arc.max(4.0),
                mark.tip,
                &mut |sample| walked.push(sample),
            );
            if walked.len() < 2 {
                continue;
            }
            resample_into(&walked, settings.blade_width, ribs, &mut points);
            attributes.push([
                mark.maturity,
                mark.base_light,
                mark.tone as u8 as f32,
                mark.tip_light,
            ]);
        }

        let (footprint, ground, rows, columns) = sample_ground(&page, field);
        let camera = Camera::for_page(&page, scene.canopy_ceiling());

        Self {
            page,
            points,
            attributes,
            ground,
            ground_rows: rows,
            ground_columns: columns,
            footprint,
            camera,
            settings,
            ribs,
        }
    }

    /// How many blades this scene holds.
    pub fn blades(&self) -> usize {
        self.points.len() / (self.ribs * VERTICES_PER_RIB)
    }

    /// Cross-sections per blade in this export.
    pub fn ribs(&self) -> usize {
        self.ribs
    }

    /// Write the scene to a directory, returning the header's path.
    pub fn write(&self, directory: &Path) -> io::Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        write_f32(&directory.join("blades.bin"), self.points.iter().flatten())?;
        write_f32(
            &directory.join("attributes.bin"),
            self.attributes.iter().flatten(),
        )?;
        write_f32(&directory.join("ground.bin"), self.ground.iter())?;

        let header = directory.join("scene.json");
        std::fs::write(&header, self.header_json())?;
        Ok(header)
    }

    /// The header, hand-written because it is small and fixed.
    fn header_json(&self) -> String {
        let camera = &self.camera;
        let settings = &self.settings;
        let (low, high) = self.footprint;
        let basis = camera.basis;
        format!(
            r#"{{
  "version": 1,
  "page": {{
    "origin": [{:.6}, {:.6}],
    "width": {},
    "height": {},
    "px_per_metre": {:.6}
  }},
  "camera": {{
    "location": [{:.6}, {:.6}, {:.6}],
    "basis": [[{:.8}, {:.8}, {:.8}], [{:.8}, {:.8}, {:.8}], [{:.8}, {:.8}, {:.8}]],
    "ortho_scale": {:.8},
    "pixel_aspect_y": {:.8}
  }},
  "sun": {{
    "elevation": {:.6},
    "azimuth": {:.6},
    "angle": {:.6},
    "strength": {:.4},
    "colour": [{:.4}, {:.4}, {:.4}]
  }},
  "sky": {{ "strength": {:.4}, "colour": [{:.4}, {:.4}, {:.4}] }},
  "render": {{
    "resolution": [{}, {}],
    "samples": {},
    "denoise": {},
    "device": "{}",
    "view_transform": "{}",
    "passes": {}
  }},
  "blades": {{
    "path": "blades.bin",
    "attributes": "attributes.bin",
    "count": {},
    "ribs_per_blade": {},
    "vertices_per_rib": {},
    "attributes_per_blade": {}
  }},
  "ground": {{
    "path": "ground.bin",
    "rows": {},
    "columns": {},
    "low": [{:.6}, {:.6}],
    "high": [{:.6}, {:.6}]
  }}
}}
"#,
            self.page.origin.x,
            self.page.origin.y,
            self.page.width,
            self.page.height,
            self.page.px_per_metre,
            camera.location.x,
            camera.location.y,
            camera.location.z,
            basis[0].x,
            basis[0].y,
            basis[0].z,
            basis[1].x,
            basis[1].y,
            basis[1].z,
            basis[2].x,
            basis[2].y,
            basis[2].z,
            camera.ortho_scale,
            camera.pixel_aspect_y,
            settings.sun_elevation,
            bearing_to_blender(settings.sun_azimuth),
            settings.sun_angle,
            settings.sun_strength,
            settings.sun_colour[0],
            settings.sun_colour[1],
            settings.sun_colour[2],
            settings.sky_strength,
            settings.sky_colour[0],
            settings.sky_colour[1],
            settings.sky_colour[2],
            self.page.width,
            self.page.height,
            settings.samples,
            settings.denoise,
            settings.device,
            settings.view_transform,
            settings.passes,
            self.blades(),
            self.ribs,
            VERTICES_PER_RIB,
            ATTRIBUTES_PER_BLADE,
            self.ground_rows,
            self.ground_columns,
            low.x,
            low.y,
            high.x,
            high.y,
        )
    }
}

/// Reflect a game-world point into the space Blender is given.
///
/// A swap of the two ground axes, which is a reflection across `x = y`. This is
/// the *whole* of the handedness fix — see [`Camera`] for the derivation. Height
/// is untouched, because the mirror is horizontal.
#[inline]
pub fn to_blender(world: Vec3) -> Vec3 {
    Vec3::new(world.y, world.x, world.z)
}

/// The same reflection applied to a bearing.
///
/// Swapping the ground axes turns a bearing measured from `+X` toward `+Y` into
/// its complement. A sun that skipped this would light a reflected field from
/// the unreflected side, and the result would look perfectly plausible while
/// being wrong — which is why it lives next to [`to_blender`] rather than being
/// done at the call site.
#[inline]
pub fn bearing_to_blender(azimuth: f32) -> f32 {
    std::f32::consts::FRAC_PI_2 - azimuth
}

/// Turn a walked centreline into a folded ribbon of exactly
/// [`VERTICES_PER_BLADE`] vertices.
///
/// Resampled by arc length rather than by sample index. `walk_blade` spaces its
/// ribs to fill pixels, so its samples bunch where the blade turns; taking every
/// n-th one would put most of the cross-sections in the bend and leave the
/// straight run described by two.
///
/// A forked blade arrives as three concatenated runs — parent, long child, short
/// child — and this deliberately flattens them into one ribbon. A fork is a few
/// millimetres of silhouette at the tip, and splitting it into three meshes
/// would triple the vertex count of the most numerous object in the scene to
/// describe something the sun's own penumbra is wider than.
fn resample_into(walked: &[BladeSample], width_scale: f32, ribs: usize, out: &mut Vec<[f32; 3]>) {
    let mut lengths = Vec::with_capacity(walked.len());
    let mut total = 0.0f32;
    lengths.push(0.0);
    for pair in walked.windows(2) {
        total += pair[1].position.distance(pair[0].position);
        lengths.push(total);
    }

    let mut cursor = 0usize;
    for i in 0..ribs {
        let (position, frame, half) = if total <= 1.0e-6 {
            // A degenerate blade still has to contribute its vertices, or every
            // blade after it in the buffer shifts and the whole page shears.
            let sample = walked[0];
            (sample.position, sample.frame, sample.half_reference)
        } else {
            let wanted = total * i as f32 / (ribs - 1) as f32;
            while cursor + 2 < walked.len() && lengths[cursor + 1] < wanted {
                cursor += 1;
            }
            let span = (lengths[cursor + 1] - lengths[cursor]).max(1.0e-9);
            let t = ((wanted - lengths[cursor]) / span).clamp(0.0, 1.0);
            let a = walked[cursor];
            let b = walked[cursor + 1];
            (
                a.position.lerp(b.position, t),
                // The frame is taken from the nearer sample rather than
                // interpolated. Two frames a fraction of a blade apart differ by
                // a small rotation, and lerping their axes denormalises them —
                // which would show up as a seam of wrong-facing normals exactly
                // where the blade twists most.
                if t < 0.5 { a.frame } else { b.frame },
                a.half_reference + (b.half_reference - a.half_reference) * t,
            )
        };

        let half_width = (half / iso::PX_PER_METRE * width_scale).max(1.0e-5);
        let ridge = crate::geometry::RIDGE * half_width;
        for step in 0..VERTICES_PER_RIB {
            // −1, 0, +1 across the blade.
            let u = step as f32 - 1.0;
            // A parabolic crown: proud at the middle, flush at both edges.
            let lift = ridge * (1.0 - u * u);
            let vertex = position + frame.binormal * (u * half_width) + frame.normal * lift;
            let reflected = to_blender(vertex);
            out.push([reflected.x, reflected.y, reflected.z]);
        }
    }
}

/// How much of the mound field reaches the *visible* ground.
///
/// A third. The mound field's job is to decide which grass is vigorous, which
/// way the ground faces and where water collects — its relief reaches a quarter
/// of a metre, which over a mound a metre across is a twelve-degree slope. That
/// is a meadow's worth of swell when it is only steering the planting.
///
/// Rendered at full strength it is something else. A path tracer draws the mesh,
/// so a dome that size becomes a *silhouette*, and a dome with bare soil on top
/// stops reading as ground and reads as a boulder sitting in the field. The
/// rasteriser never had this problem because it only ever shaded the relief; it
/// never showed it in profile.
///
/// So the swell is kept and its profile is not. What survives still tilts the
/// surface toward and away from the sun, which is all the lighting ever wanted
/// from it.
const GROUND_RELIEF: f32 = 0.34;

/// Ground heights over the page's world footprint.
///
/// The footprint is a diamond in world space — a rectangle on screen unprojects
/// to one — so the grid covers its axis-aligned bound and the corners fall
/// outside the frame. That waste is the cheapest correct option: a grid aligned
/// to the diamond would have to be re-derived for every page shape, and the
/// corners cost four triangles.
fn sample_ground(page: &Page, field: &WorldField) -> ((Vec2, Vec2), Vec<f32>, usize, usize) {
    let mut low = Vec2::splat(f32::INFINITY);
    let mut high = Vec2::splat(f32::NEG_INFINITY);
    for corner in [
        Vec2::ZERO,
        Vec2::new(page.width as f32, 0.0),
        Vec2::new(0.0, page.height as f32),
        Vec2::new(page.width as f32, page.height as f32),
    ] {
        // Reflected, like everything else that crosses this boundary.
        let ground = to_blender(page.ground_at(corner).extend(0.0)).truncate();
        low = low.min(ground);
        high = high.max(ground);
    }
    // A little beyond the frame, so the ground plane never ends inside the
    // picture and the sun always has something to land on behind the grass.
    low -= Vec2::splat(1.0);
    high += Vec2::splat(1.0);

    // A grid step of about four centimetres. The mound field's finest feature is
    // a good deal broader than that, and the blades hide the surface almost
    // everywhere it matters.
    const STEP: f32 = 0.04;
    let span = high - low;
    let columns = ((span.x / STEP).ceil() as usize + 1).clamp(2, 2048);
    let rows = ((span.y / STEP).ceil() as usize + 1).clamp(2, 2048);

    let mut heights = Vec::with_capacity(rows * columns);
    for row in 0..rows {
        for column in 0..columns {
            let blender = Vec2::new(
                low.x + span.x * column as f32 / (columns - 1) as f32,
                low.y + span.y * row as f32 / (rows - 1) as f32,
            );
            // The grid is laid out in Blender's reflected space, so the field —
            // which only knows the game's — is asked about the swapped point.
            let world = Vec2::new(blender.y, blender.x);
            heights.push(field.sample(world).height * GROUND_RELIEF);
        }
    }
    ((low, high), heights, rows, columns)
}

fn write_f32<'a>(path: &Path, values: impl Iterator<Item = &'a f32>) -> io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    for value in values {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()
}

/// Render an exported scene by handing it to Blender.
///
/// A subprocess rather than a linked library, and that is the recommendation
/// from every direction: Cycles' standalone XML interface is explicitly not a
/// stable API, and linking its C++ internals means owning Embree, OpenImageIO,
/// OpenColorIO, OIDN and a device abstraction across four GPU backends. A
/// process boundary costs a few seconds of startup and buys immunity from all
/// of it.
///
/// Startup is real and is paid per call here. See the worker phase for the
/// persistent version; this one exists to be obviously correct.
pub fn render(header: &Path, output: &Path, blender: &Path) -> io::Result<std::process::Output> {
    let script = script_path();
    std::process::Command::new(blender)
        .arg("--background")
        .arg("--factory-startup")
        .arg("--python")
        .arg(&script)
        .arg("--")
        .arg(header)
        .arg(output)
        .output()
}

/// Where the Blender-side script lives, relative to the workspace.
fn script_path() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is this crate; the script is a workspace tool because
    // it is not Rust and does not belong inside a crate's source tree.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/bw_cycles/render.py")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("tools/bw_cycles/render.py"))
}

/// Where Blender is, honouring an override.
pub fn blender_path() -> PathBuf {
    if let Ok(path) = std::env::var("BW_BLENDER") {
        return PathBuf::from(path);
    }
    for candidate in [
        "/Applications/Blender.app/Contents/MacOS/Blender",
        "/usr/local/bin/blender",
        "/usr/bin/blender",
    ] {
        let path = Path::new(candidate);
        if path.exists() {
            return path.to_path_buf();
        }
    }
    PathBuf::from("blender")
}

/// Quality tiers, expressed as path-tracer budgets.
impl GrassRenderQuality {
    /// Path-tracing samples per pixel for this tier.
    ///
    /// The rasteriser's tiers measured supersampling and shadow-map density.
    /// Those quantities do not exist here — there is one number that trades
    /// noise against time, and the denoiser moves where the useful part of that
    /// curve sits.
    pub const fn cycles_samples(self) -> u32 {
        match self {
            Self::Preview => 48,
            Self::Dataset => 256,
            Self::Reference => 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Page {
        Page::new(Vec2::ZERO, 512, 512)
    }

    #[test]
    fn the_camera_looks_down_the_isometric_axis() {
        let camera = Camera::for_page(&page(), 1.0);
        let backward = camera.basis[2];
        // Above the ground, not below it. The whole point of negating right.
        let expected = Vec3::splat(1.0).normalize();
        assert!(
            backward.distance(expected) < 1.0e-5,
            "camera looks from {backward:?}, not the isometric axis {expected:?}"
        );
        // 35.264°, which is asin(1/√3) and the elevation the whole crate assumes.
        let elevation = backward.z.asin().to_degrees();
        assert!(
            (elevation - 35.264).abs() < 0.01,
            "elevation came out at {elevation}"
        );
    }

    #[test]
    fn the_camera_basis_is_orthonormal() {
        let [right, up, backward] = Camera::for_page(&page(), 1.0).basis;
        for axis in [right, up, backward] {
            assert!((axis.length() - 1.0).abs() < 1.0e-5);
        }
        assert!(right.dot(up).abs() < 1.0e-6);
        assert!(right.dot(backward).abs() < 1.0e-6);
        assert!(up.dot(backward).abs() < 1.0e-6);
    }

    #[test]
    fn the_pixel_aspect_carries_the_dimetric_stretch() {
        // 2/√3. The whole difference between the game's 2:1 diamond and true
        // isometric, and the reason this cannot be a camera transform.
        let expected = 2.0 / 3.0f32.sqrt();
        for (w, h) in [(512, 512), (1024, 512), (300, 700)] {
            let camera = Camera::for_page(&Page::new(Vec2::ZERO, w, h), 1.0);
            assert!(
                (camera.pixel_aspect_y - expected).abs() < 1.0e-5,
                "{w}x{h} gave {} rather than {expected}",
                camera.pixel_aspect_y
            );
        }
    }

    #[test]
    fn the_frame_spans_the_world_the_page_covers() {
        // A page 512 pixels wide at 96 to the metre shows 512/96 screen metres
        // across, and a screen metre along the horizontal is 1/√2 world metres.
        let page = Page::new(Vec2::ZERO, 512, 256);
        let camera = Camera::for_page(&page, 1.0);
        let expected = (512.0 / 96.0) / 2.0f32.sqrt();
        assert!(
            (camera.ortho_scale - expected).abs() < 1.0e-4,
            "ortho scale {} rather than {expected}",
            camera.ortho_scale
        );
    }

    #[test]
    fn a_walked_blade_resamples_to_a_fixed_point_count() {
        use crate::stroke::Stroke;
        let stroke = Stroke {
            root: Vec3::new(1.0, 2.0, 0.0),
            length: 0.3,
            bend: 0.3,
            ..default()
        };
        let mut walked = Vec::new();
        walk_blade(&stroke, 30.0, 2.0, stroke.tip, &mut |sample| {
            walked.push(sample)
        });
        assert!(walked.len() > 2, "the fixture blade produced no walk");

        let mut points = Vec::new();
        resample_into(&walked, 1.0, RIBS_PER_BLADE, &mut points);
        assert_eq!(points.len(), VERTICES_PER_BLADE);

        // The ends have to be the ends: a resample that drifts off the root
        // detaches every blade in the page from the ground it grows out of.
        // Compared against the *reflected* walk, because the export crosses the
        // handedness boundary — see `to_blender`.
        // The middle vertex of a rib sits on the centreline plus the ridge, so
        // the root is checked against the rib's own centre rather than against a
        // corner of it.
        let centre = Vec3::new(points[1][0], points[1][1], points[1][2]);
        let root = to_blender(walked[0].position);
        assert!(
            centre.distance(root) < 0.01,
            "root rib centred at {centre:?}, not {root:?}"
        );
        let last = points[VERTICES_PER_BLADE - 2];
        let tip = Vec3::new(last[0], last[1], last[2]);
        let end = to_blender(walked[walked.len() - 1].position);
        assert!(tip.distance(end) < 0.01, "tip rib at {tip:?}, not {end:?}");
    }

    #[test]
    fn the_reflection_makes_the_camera_agree_with_iso_exactly() {
        // The claim the whole handedness fix rests on, checked numerically
        // rather than trusted from the derivation: reflect a world point, view
        // it through the physical camera, and recover `iso::project`.
        let camera = Camera::for_page(&page(), 1.0);
        let [right, up, _] = camera.basis;
        let horizontal_scale = Vec3::new(1.0, -1.0, 0.0).length();
        let vertical_scale = Vec3::new(-0.5, -0.5, 1.0).length();
        for world in [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, -1.0, 0.4),
            Vec3::new(-2.5, 4.0, 1.2),
            Vec3::new(1.0, 1.0, 0.0),
        ] {
            let reflected = to_blender(world);
            let screen = iso::project(world);
            let x = reflected.dot(right) * horizontal_scale;
            let y = reflected.dot(up) * vertical_scale;
            assert!((x - screen.x).abs() < 1.0e-5, "x {x} vs {}", screen.x);
            assert!((y - screen.y).abs() < 1.0e-5, "y {y} vs {}", screen.y);
        }
    }

    #[test]
    fn the_sun_is_reflected_with_the_world() {
        // A bearing that skipped the swap would light a reflected field from the
        // unreflected side. Reflecting the sun's own direction vector and
        // rebuilding the bearing from it has to give the same answer.
        for degrees in [0.0f32, 35.0, 125.0, 210.0, 340.0] {
            let azimuth = degrees.to_radians();
            let direction = Vec3::new(azimuth.cos(), azimuth.sin(), 0.0);
            let reflected = to_blender(direction);
            let expected = reflected.y.atan2(reflected.x);
            let got = bearing_to_blender(azimuth);
            let difference = (got - expected).sin().abs();
            assert!(difference < 1.0e-5, "{degrees}° gave {got} not {expected}");
        }
    }

    #[test]
    fn the_same_scene_exports_the_same_bytes_twice() {
        // The property the whole pipeline rests on, and the one a path tracer
        // could quietly take away: a scene is reproducible, so the picture can
        // be rebuilt from a seed rather than archived.
        use crate::bake::BakeParams;
        use crate::field::WorldField;

        let params = BakeParams::default();
        let page = Page::new(Vec2::new(37.0, -19.0), 96, 96);
        let field = WorldField::lit_by(params.seed, params.light);

        let build = || {
            let grown = GrassScene::build(page, &field, &params);
            CyclesScene::build(&grown, &field, RenderSettings::default())
        };
        let (first, second) = (build(), build());

        assert_eq!(first.blades(), second.blades());
        assert_eq!(
            first.points, second.points,
            "the geometry moved between runs"
        );
        assert_eq!(first.attributes, second.attributes);
        assert_eq!(first.ground, second.ground);
    }

    #[test]
    fn two_pages_that_overlap_agree_about_the_ground_they_share() {
        // Page independence, checked through the export rather than through the
        // rasteriser. The ground grid is a pure function of world position, so
        // two pages covering overlapping world must report the same heights
        // where they overlap — if they did not, a traced page would disagree
        // with its neighbour along their shared edge and no guard band could
        // hide it.
        use crate::bake::BakeParams;
        use crate::field::WorldField;

        let params = BakeParams::default();
        let field = WorldField::lit_by(params.seed, params.light);
        let here = Page::new(Vec2::ZERO, 128, 128);
        let there = Page::new(Vec2::new(64.0, 0.0), 128, 128);

        let sample = |page: Page| {
            let grown = GrassScene::build(page, &field, &params);
            CyclesScene::build(&grown, &field, RenderSettings::default())
        };
        let (a, b) = (sample(here), sample(there));

        // Probe world points inside both footprints and confirm the two grids
        // interpolate to the same height.
        let low = a.footprint.0.max(b.footprint.0);
        let high = a.footprint.1.min(b.footprint.1);
        assert!(low.x < high.x && low.y < high.y, "the pages do not overlap");

        let height_at = |scene: &CyclesScene, world: Vec2| -> f32 {
            let (lo, hi) = scene.footprint;
            let u = ((world.x - lo.x) / (hi.x - lo.x) * (scene.ground_columns - 1) as f32)
                .round()
                .clamp(0.0, (scene.ground_columns - 1) as f32) as usize;
            let v = ((world.y - lo.y) / (hi.y - lo.y) * (scene.ground_rows - 1) as f32)
                .round()
                .clamp(0.0, (scene.ground_rows - 1) as f32) as usize;
            scene.ground[v * scene.ground_columns + u]
        };

        for step in 0..8 {
            let t = (step as f32 + 0.5) / 8.0;
            let world = low.lerp(high, t);
            let (mine, theirs) = (height_at(&a, world), height_at(&b, world));
            // Loose, because the two grids land on different sample points; what
            // is being checked is that they describe one surface, not that they
            // sampled it identically.
            assert!(
                (mine - theirs).abs() < 0.02,
                "at {world:?} one page says {mine} and the other {theirs}"
            );
        }
    }

    #[test]
    fn a_ribbon_is_folded_so_its_two_facets_face_different_ways() {
        // The property the whole mesh export exists for. A flat ribbon presents
        // one normal and shades uniformly; this checks the fold is real by
        // measuring the angle between the two facets of one rib.
        use crate::geometry::Frame;
        let frame = Frame::build(0.0, 1.0, 0.0, 1.0, 0.0);
        let walk: Vec<BladeSample> = [0.0f32, 0.5, 1.0]
            .iter()
            .map(|t| BladeSample {
                position: Vec3::new(0.0, 0.0, *t),
                frame,
                half_reference: 6.0,
                along: *t,
                tip_light: 0.0,
                root_shade: 0.0,
            })
            .collect();
        let mut points = Vec::new();
        resample_into(&walk, 1.0, RIBS_PER_BLADE, &mut points);

        let at = |i: usize| Vec3::new(points[i][0], points[i][1], points[i][2]);
        let (left, centre, right) = (at(0), at(1), at(2));
        // The centre stands proud of the chord between the edges.
        let chord = left.lerp(right, 0.5);
        assert!(
            centre.distance(chord) > 1.0e-4,
            "the rib is flat: centre {centre:?} sits on the chord {chord:?}"
        );

        // And the two facets genuinely diverge.
        let along = Vec3::Z;
        let left_normal = (centre - left).cross(along).normalize();
        let right_normal = (right - centre).cross(along).normalize();
        let angle = left_normal.dot(right_normal).clamp(-1.0, 1.0).acos();
        assert!(
            angle.to_degrees() > 15.0,
            "the facets differ by only {:.1}°",
            angle.to_degrees()
        );
    }

    #[test]
    fn resampling_spaces_points_by_arc_length() {
        // A straight run with samples deliberately bunched at one end. Even
        // spacing on the output is what says the resample read distance rather
        // than sample index.
        use crate::geometry::Frame;
        let frame = Frame::build(0.0, 1.0, 0.0, 1.0, 0.0);
        let bunched: Vec<BladeSample> = [0.0f32, 0.01, 0.02, 0.03, 0.04, 0.5, 1.0]
            .iter()
            .map(|t| BladeSample {
                position: Vec3::new(0.0, 0.0, *t),
                frame,
                half_reference: 1.0,
                along: *t,
                tip_light: 0.0,
                root_shade: 0.0,
            })
            .collect();
        let mut points = Vec::new();
        resample_into(&bunched, 1.0, RIBS_PER_BLADE, &mut points);

        // Read the centre vertex of each rib; the edges are offset sideways by
        // construction and would not sit on the centreline.
        let step = 1.0 / (RIBS_PER_BLADE - 1) as f32;
        for rib in 0..RIBS_PER_BLADE {
            let z = points[rib * VERTICES_PER_RIB + 1][2];
            let wanted = step * rib as f32;
            assert!(
                (z - wanted).abs() < 1.0e-3,
                "rib {rib} landed at {z} rather than {wanted}"
            );
        }
    }
}

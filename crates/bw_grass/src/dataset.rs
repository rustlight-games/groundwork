//! Paired renders, for training something to do this faster.
//!
//! The renderer's expensive path now costs seconds a page. That is fine for
//! masters and useless for a game, so the plan is to learn it: feed a network a
//! cheap render plus the structure the cheap render cannot see, and have it
//! produce the expensive one.
//!
//! Which puts one requirement above all the others.
//!
//! ## The pair must be one meadow
//!
//! ```text
//!   wrong                              right
//!   ─────                              ─────
//!   cheap  → generate scene A          generate scene ────┬─→ render cheap
//!   costly → generate scene B                             └─→ render costly
//! ```
//!
//! Generating twice looks safe, because placement is a pure function of world
//! coordinates and both runs would agree. It is not the agreement that matters;
//! it is that nothing can *later* make them disagree. A quality tier that
//! skipped a fork, a step count that moved a rib, an optimisation that reordered
//! a draw — any of those turns the pair into two photographs of two different
//! fields, and a network trained on that learns to hallucinate rather than to
//! reconstruct. The failure is silent: the loss simply stops falling, and no
//! image in the corpus looks wrong.
//!
//! So [`Pair::bake`] builds one [`GrassScene`] and renders it twice. That is
//! also why [`crate::quality::GrassRenderQuality`] is forbidden from changing
//! what grows where — the tier is allowed to decide how finely something is
//! measured and never whether it exists.
//!
//! ## What travels with the target
//!
//! The expensive path decides a great deal from hashes the cheap input cannot
//! see. Whether this broad blade forked, which way its face turned, how much
//! canopy is stacked behind it — none of that is in a low-resolution plate, and
//! a network given only pixels has no choice but to average over the
//! possibilities. Averaged forks are soft tips and averaged occlusion is a flat
//! interior, which are precisely the failures this renderer was built to remove.
//! [`crate::bake::Passes`] is the structure, exported beside the picture.

use std::io;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use rayon::prelude::*;

use crate::bake::{BakeParams, Macro, Page, Passes, cast_shadows, lay_floor, resolve_passes};
use crate::cycles::{self, CyclesScene, RenderSettings};
use crate::field::WorldField;
use crate::iso;
use crate::quality::GrassRenderQuality;
use crate::scene::GrassScene;
use crate::stroke::Painter;
use crate::surface::Surface;

/// One rendering of a scene, at one budget.
pub struct Render {
    /// Final-resolution linear colour.
    pub colour: Vec<Vec3>,
    /// What the renderer knew while producing it.
    pub passes: Passes,
    pub width: usize,
    pub height: usize,
}

/// A cheap render and an expensive one, of the same ground.
pub struct Pair {
    pub input: Render,
    pub target: Render,
    /// How many marks the shared scene held. Worth recording: a crop grown from
    /// a nearly empty scene is not a hard example, it is a useless one.
    pub marks: usize,
}

impl Pair {
    /// Build one scene and render it at both budgets.
    pub fn bake(page: Page, params: &BakeParams, input: GrassRenderQuality) -> Self {
        let field = WorldField::lit_by(params.seed, params.light);
        let lattice = Macro::build(&page, &field);
        // Once. See the module note for why this is the whole point.
        let scene = GrassScene::build(page, &field, params);

        let render = |quality: GrassRenderQuality| {
            let params = BakeParams { quality, ..*params };
            let mut surface =
                Surface::at_supersample(page.width, page.height, quality.supersample());
            lay_floor(&mut surface, &page, &field, &lattice);
            {
                let mut painter =
                    Painter::at_scale(&mut surface, page.origin, params.light, page.px_per_metre)
                        .with_ribs_per_pixel(quality.ribs_per_pixel());
                scene.draw(&mut painter);
            }
            let shadows = cast_shadows(&scene, &params);
            let mut passes = Passes::default();
            let colour = resolve_passes(
                &surface,
                &page,
                &lattice,
                &params,
                shadows.as_deref(),
                Some(&mut passes),
            );
            Render {
                colour,
                passes,
                width: page.width,
                height: page.height,
            }
        };

        Self {
            input: render(input),
            target: render(params.quality),
            marks: scene.len(),
        }
    }

    /// Cut the middle out of both renders.
    ///
    /// Training crops are taken from the centre of a larger bake rather than
    /// baked at their own size, and the reason is the same one that made
    /// `bake_padded` necessary: every neighbourhood-reading term — occlusion,
    /// the relief comparison, the shadows themselves — is wrong near an edge.
    /// A corpus of crops baked at their own size teaches the network that page
    /// borders exist, and it will faithfully reproduce them.
    pub fn crop(&self, margin: usize) -> (Vec<Vec3>, Vec<Vec3>, usize, usize) {
        let (w, h) = (self.input.width, self.input.height);
        let margin = margin.min(w / 2).min(h / 2);
        let (cw, ch) = (w - margin * 2, h - margin * 2);
        let cut = |source: &[Vec3]| {
            let mut out = Vec::with_capacity(cw * ch);
            for row in 0..ch {
                let start = (row + margin) * w + margin;
                out.extend_from_slice(&source[start..start + cw]);
            }
            out
        };
        (cut(&self.input.colour), cut(&self.target.colour), cw, ch)
    }
}

/// A cheap raster render and the path-traced target of the same ground.
///
/// The pairing this corpus actually wants, and the reason is the whole hybrid
/// split: the rasteriser is fast enough to run in a game and cannot integrate a
/// hemisphere; Cycles integrates the hemisphere and takes seconds. Learning the
/// second from the first is the point of the exercise, so the target has to be
/// the *traced* image rather than an expensive rasterisation of the same
/// approximations.
///
/// ## The "one meadow" rule survives, and gets stronger
///
/// [`Pair`] renders one [`GrassScene`] twice through one renderer. This renders
/// one [`GrassScene`] through two *different* renderers, which sounds like it
/// weakens the guarantee and does the opposite: both sides consume the identical
/// `Vec<Stroke>`, and the Cycles export walks those strokes with the same
/// [`crate::stroke::walk_blade`] the rasteriser draws them with. There is one
/// geometry source and no second generation to drift.
///
/// ## Why the image is not read here
///
/// [`TracedPair::trace`] leaves a PNG on disk and stops. Decoding it would put
/// an image codec in a crate the game links, to serve a path that only ever runs
/// offline. The caller — an example, a corpus job — already has a decoder.
pub struct TracedPair {
    /// The cheap render, from the rasteriser.
    pub input: Render,
    /// The same scene, prepared for the path tracer.
    pub scene: CyclesScene,
    pub marks: usize,
}

impl TracedPair {
    /// Grow one scene, rasterise it cheaply, and prepare it for tracing.
    pub fn build(
        page: Page,
        params: &BakeParams,
        input: GrassRenderQuality,
        settings: RenderSettings,
    ) -> Self {
        let field = WorldField::lit_by(params.seed, params.light);
        let lattice = Macro::build(&page, &field);
        // Once. Both renderers read this and nothing regenerates it.
        let grown = GrassScene::build(page, &field, params);

        let cheap = BakeParams {
            quality: input,
            ..*params
        };
        let mut surface = Surface::at_supersample(page.width, page.height, input.supersample());
        lay_floor(&mut surface, &page, &field, &lattice);
        {
            let mut painter =
                Painter::at_scale(&mut surface, page.origin, cheap.light, page.px_per_metre)
                    .with_ribs_per_pixel(input.ribs_per_pixel());
            grown.draw(&mut painter);
        }
        let shadows = cast_shadows(&grown, &cheap);
        let mut passes = Passes::default();
        let colour = resolve_passes(
            &surface,
            &page,
            &lattice,
            &cheap,
            shadows.as_deref(),
            Some(&mut passes),
        );

        let marks = grown.len();
        Self {
            input: Render {
                colour,
                passes,
                width: page.width,
                height: page.height,
            },
            scene: CyclesScene::build(&grown, &field, settings),
            marks,
        }
    }

    /// Write the scene out and trace it, leaving a PNG at `output`.
    ///
    /// Blender exits zero when its Python raises, so a successful status proves
    /// nothing on its own; the presence of the file is the only evidence a
    /// render happened.
    pub fn trace(&self, directory: &Path, output: &Path, blender: &Path) -> io::Result<()> {
        let header = self.scene.write(directory)?;
        let result = cycles::render(&header, output, blender)?;
        let produced = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
        if !result.status.success() || produced == 0 {
            return Err(io::Error::other(format!(
                "cycles produced no image at {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                output.display(),
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr),
            )));
        }
        Ok(())
    }

    /// Cut the middle out of the cheap render, matching [`Pair::crop`].
    ///
    /// The traced side is cropped by the caller once it has decoded it, from the
    /// same margin — see [`Pair::crop`] for why a crop is taken from the middle
    /// of a larger bake rather than baked at its own size.
    pub fn crop_input(&self, margin: usize) -> (Vec<Vec3>, usize, usize) {
        let (w, h) = (self.input.width, self.input.height);
        let margin = margin.min(w / 2).min(h / 2);
        let (cw, ch) = (w - margin * 2, h - margin * 2);
        let mut out = Vec::with_capacity(cw * ch);
        for row in 0..ch {
            let start = (row + margin) * w + margin;
            out.extend_from_slice(&self.input.colour[start..start + cw]);
        }
        (out, cw, ch)
    }
}

/// Everything needed to reproduce a shard, recorded beside it.
///
/// A training corpus that cannot say what produced it is a corpus that can only
/// be regenerated by guessing, and a renderer under active development will not
/// produce the same bytes next month. The point of this is not provenance for
/// its own sake — it is that a model whose targets came from two renderer
/// versions has learned the average of two looks, and nothing in the loss curve
/// says so.
#[derive(Clone, Debug)]
pub struct ShardMetadata {
    /// The renderer's own version, from the crate.
    pub renderer: &'static str,
    /// Which tier produced the target.
    pub target_quality: &'static str,
    /// Which produced the input.
    pub input_quality: &'static str,
    pub seed: u64,
    /// Cache-pixel origin of the page.
    pub origin: (f32, f32),
    pub page_pixels: (usize, usize),
    pub px_per_metre: f32,
    /// The key light, in world space, and its height above the ground.
    pub sun: (f32, f32, f32),
    pub sun_elevation_degrees: f32,
    pub sun_radius: f32,
    pub marks: usize,
}

impl ShardMetadata {
    pub fn of(page: &Page, params: &BakeParams, input: GrassRenderQuality, marks: usize) -> Self {
        let sun = crate::iso::image_to_world(params.light).normalize_or(Vec3::Z);
        Self {
            renderer: env!("CARGO_PKG_VERSION"),
            target_quality: params.quality.name(),
            input_quality: input.name(),
            seed: params.seed,
            origin: (page.origin.x, page.origin.y),
            page_pixels: (page.width, page.height),
            px_per_metre: page.px_per_metre,
            sun: (sun.x, sun.y, sun.z),
            sun_elevation_degrees: crate::iso::elevation_of(sun).to_degrees(),
            sun_radius: params.sun_radius,
            marks,
        }
    }

    /// As RON, which is what the rest of this repository stores data in.
    pub fn to_ron(&self) -> String {
        format!(
            "(\n    renderer: \"{}\",\n    target_quality: \"{}\",\n    \
             input_quality: \"{}\",\n    seed: {},\n    origin: ({}, {}),\n    \
             page_pixels: ({}, {}),\n    px_per_metre: {},\n    \
             sun: ({}, {}, {}),\n    sun_elevation_degrees: {},\n    \
             sun_radius: {},\n    marks: {},\n)\n",
            self.renderer,
            self.target_quality,
            self.input_quality,
            self.seed,
            self.origin.0,
            self.origin.1,
            self.page_pixels.0,
            self.page_pixels.1,
            self.px_per_metre,
            self.sun.0,
            self.sun.1,
            self.sun.2,
            self.sun_elevation_degrees,
            self.sun_radius,
            self.marks,
        )
    }
}

/// How much of each edge is thrown away when a crop is cut.
///
/// Not decoration. Every neighbourhood-reading term in the renderer — occlusion,
/// the relief comparison, the shadows themselves — is wrong near a page edge,
/// and a corpus of crops baked at their own size teaches a network that page
/// borders exist. It will then faithfully reproduce them.
pub const CROP_MARGIN: usize = 96;

/// A whole corpus job, as a value.
///
/// This lived in the `grass_dataset` example until a second caller wanted it.
/// Keeping it there had the same defect the tiling arithmetic did: the rules
/// that decide whether a corpus is *usable* — one scene per pair, a crop taken
/// from the middle of a larger bake, a fresh world per shard — were reachable
/// only by running an example, and a second entry point would have had to
/// reimplement them and get all three right again.
#[derive(Clone, Debug)]
pub struct CorpusRequest {
    pub shards: usize,
    /// Side of the bake, in final pixels. Larger than the crop, deliberately.
    pub page: usize,
    /// Side of the crop actually kept.
    pub crop: usize,
    pub px_per_metre: f32,
    /// The root seed. Each shard derives its own world from it.
    pub seed: u64,
    pub target: GrassRenderQuality,
    pub input: GrassRenderQuality,
    /// Write the structural channels beside the picture.
    pub aovs: bool,
    /// Pair the cheap render against an expensive *rasterisation* rather than
    /// against Cycles. The older pairing, kept for when Blender is absent.
    pub raster: bool,
    pub samples: u32,
    pub density: f32,
    pub length: f32,
    pub out: PathBuf,
}

impl Default for CorpusRequest {
    fn default() -> Self {
        Self {
            shards: 8,
            page: 448,
            crop: 256,
            px_per_metre: iso::PX_PER_METRE,
            seed: 0x9a55_0001,
            target: GrassRenderQuality::Dataset,
            input: GrassRenderQuality::Preview,
            aovs: false,
            raster: false,
            samples: 192,
            // The tuned canopy. See `crate::plate` for why the rasteriser's own
            // counts are the wrong quantity for a path tracer.
            density: 8.0,
            length: 1.6,
            out: PathBuf::from("target/grass-dataset"),
        }
    }
}

impl CorpusRequest {
    /// Pixels thrown away from each edge.
    pub fn margin(&self) -> usize {
        CROP_MARGIN.min(self.page.saturating_sub(self.crop) / 2)
    }

    /// A stable seed per shard.
    ///
    /// Every shard is its own *world* rather than its own patch of one world,
    /// which is deliberate. Crops from one world share its regional hue, its
    /// density and its flow, so a corpus drawn from a single seed is far less
    /// varied than its size suggests — and a validation split cut from the same
    /// world is not a held-out sample at all.
    pub fn seed_for(&self, shard: usize) -> u64 {
        self.seed
            .wrapping_add((shard as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    /// Where in that world to stand.
    pub fn origin_for(&self, shard: usize) -> Vec2 {
        let step = (shard as f32) * 977.0;
        Vec2::new(step % 8191.0 - 4096.0, (step * 1.618) % 7817.0 - 3908.0)
    }

    /// The parameters one shard is grown under.
    pub fn params_for(&self, shard: usize) -> BakeParams {
        let mut params = BakeParams {
            seed: self.seed_for(shard),
            quality: self.target,
            ..BakeParams::default()
        };
        params.tufts *= self.density;
        params.fine *= self.density;
        params.thatch *= self.density;
        params.leaves *= self.density;
        params.blade_length.0 *= self.length;
        params.blade_length.1 *= self.length;
        params
    }

    /// The page one shard is baked on.
    pub fn page_for(&self, shard: usize) -> Page {
        Page::at_detail(
            self.origin_for(shard),
            self.page,
            self.page,
            self.px_per_metre / iso::PX_PER_METRE,
        )
    }
}

/// What a corpus job produced.
#[derive(Clone, Copy, Debug, Default)]
pub struct CorpusReport {
    pub shards: usize,
    pub images: usize,
    /// Shards that failed and wrote nothing.
    pub failed: usize,
}

/// Generate a corpus, writing shards under `request.out`.
///
/// `progress` is called once per finished shard, with the shard's index and how
/// many images it wrote.
pub fn generate(
    request: &CorpusRequest,
    progress: &mut (dyn FnMut(usize, usize) + Send + Sync),
) -> io::Result<CorpusReport> {
    std::fs::create_dir_all(&request.out)?;

    if request.raster {
        // The rasteriser is threaded and holds no subprocess, so shards fan out.
        let counts: Vec<usize> = (0..request.shards)
            .into_par_iter()
            .map(|shard| raster_shard(request, shard))
            .collect();
        for (shard, images) in counts.iter().enumerate() {
            progress(shard, *images);
        }
        return Ok(tally(request.shards, &counts));
    }

    // Cycles renders on the GPU and Blender is a process, so shards are traced
    // one at a time while the rasteriser's own work stays threaded inside each.
    // Fanning out subprocesses here would contend for one device and make the
    // whole job slower, not faster.
    let blender = cycles::blender_path();
    let mut counts = Vec::with_capacity(request.shards);
    for shard in 0..request.shards {
        let images = traced_shard(request, shard, &blender);
        progress(shard, images);
        counts.push(images);
    }
    Ok(tally(request.shards, &counts))
}

fn tally(shards: usize, counts: &[usize]) -> CorpusReport {
    CorpusReport {
        shards,
        images: counts.iter().sum(),
        failed: counts.iter().filter(|&&n| n == 0).count(),
    }
}

/// One shard, with Cycles as the target.
fn traced_shard(request: &CorpusRequest, shard: usize, blender: &Path) -> usize {
    let params = request.params_for(shard);
    let page = request.page_for(shard);
    let settings = RenderSettings {
        samples: request.samples,
        passes: request.aovs,
        ..RenderSettings::default()
    };
    let pair = TracedPair::build(page, &params, request.input, settings);

    let stem = request.out.join(format!("{shard:05}"));
    let target_path = request.out.join(format!("{shard:05}-target.png"));
    let scene_dir = request.out.join(format!(".scene-{shard:05}"));
    if let Err(error) = pair.trace(&scene_dir, &target_path, blender) {
        eprintln!("shard {shard}: {error}");
        let _ = std::fs::remove_dir_all(&scene_dir);
        return 0;
    }
    let _ = std::fs::remove_dir_all(&scene_dir);

    let margin = request.margin();
    let (input, w, h) = pair.crop_input(margin);
    let mut wrote = write_rgb(&stem, "input", &input, w, h);
    // The traced target arrives full-page; crop it to match the input.
    wrote += crop_png_in_place(&target_path, margin);
    if request.aovs {
        wrote += write_passes(&stem, &pair.input.passes, request.page, margin, w, h);
    }

    let meta = ShardMetadata::of(&page, &params, request.input, pair.marks);
    let _ = std::fs::write(stem.with_extension("ron"), meta.to_ron());
    wrote
}

/// One shard, with an expensive rasterisation as the target.
fn raster_shard(request: &CorpusRequest, shard: usize) -> usize {
    let params = request.params_for(shard);
    let page = request.page_for(shard);
    let pair = Pair::bake(page, &params, request.input);
    let (input, target, w, h) = pair.crop(request.margin());

    let stem = request.out.join(format!("{shard:05}"));
    let mut wrote = write_rgb(&stem, "input", &input, w, h);
    wrote += write_rgb(&stem, "target", &target, w, h);
    if request.aovs {
        wrote += write_passes(
            &stem,
            &pair.input.passes,
            request.page,
            request.margin(),
            w,
            h,
        );
    }

    let meta = ShardMetadata::of(&page, &params, request.input, pair.marks);
    let _ = std::fs::write(stem.with_extension("ron"), meta.to_ron());
    wrote
}

/// Write every structural channel beside the picture.
fn write_passes(
    stem: &Path,
    passes: &Passes,
    side: usize,
    margin: usize,
    w: usize,
    h: usize,
) -> usize {
    let mut wrote = 0;
    for (name, channel) in passes.scalars() {
        let cropped = crop_channel(channel, side, margin);
        wrote += write_grey(stem, name, &cropped, w, h);
    }
    for (name, channel) in passes.vectors() {
        let cropped = crop_channel(channel, side, margin);
        // Signed directions into an unsigned image. Decoded as `2v - 1`.
        let encoded: Vec<Vec3> = cropped
            .iter()
            .map(|n| *n * 0.5 + Vec3::splat(0.5))
            .collect();
        wrote += write_rgb(stem, name, &encoded, w, h);
    }
    wrote
}

fn crop_channel<T: Copy>(source: &[T], side: usize, margin: usize) -> Vec<T> {
    let cw = side - margin * 2;
    let mut out = Vec::with_capacity(cw * cw);
    for row in 0..cw {
        let start = (row + margin) * side + margin;
        out.extend_from_slice(&source[start..start + cw]);
    }
    out
}

/// Crop a written PNG to its middle, in place.
///
/// The traced target arrives at the full page size because Cycles photographs
/// the whole camera frame, and the crop has to match the input's exactly — see
/// [`Pair::crop`] for why a training crop is cut from the middle of a larger
/// bake rather than baked at its own size.
fn crop_png_in_place(path: &Path, margin: usize) -> usize {
    let Ok(image) = image::open(path) else {
        eprintln!("cannot reread {}", path.display());
        return 0;
    };
    let image = image.to_rgb8();
    let (w, h) = (image.width(), image.height());
    let margin = margin as u32;
    if margin * 2 >= w.min(h) {
        return 1;
    }
    let cropped = image::imageops::crop_imm(&image, margin, margin, w - margin * 2, h - margin * 2)
        .to_image();
    match cropped.save(path) {
        Ok(()) => 1,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            0
        }
    }
}

fn channel_path(stem: &Path, name: &str) -> PathBuf {
    stem.with_file_name(format!(
        "{}-{name}.png",
        stem.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn write_rgb(stem: &Path, name: &str, colours: &[Vec3], w: usize, h: usize) -> usize {
    let bytes = crate::surface::to_rgb8(colours);
    let path = channel_path(stem, name);
    match image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgb8) {
        Ok(()) => 1,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            0
        }
    }
}

/// A scalar channel, scaled to its own range and written as grey.
///
/// Normalised per channel rather than globally, and the normalisation is *not*
/// recorded — because these are for looking at. A trainer reading them back
/// would want the raw floats; that is a different exporter, and this one is an
/// instrument.
fn write_grey(stem: &Path, name: &str, values: &[f32], w: usize, h: usize) -> usize {
    let low = values.iter().cloned().fold(f32::INFINITY, f32::min);
    let high = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let span = (high - low).max(1.0e-6);
    let bytes: Vec<u8> = values
        .iter()
        .map(|v| (((v - low) / span).clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    let path = channel_path(stem, name);
    match image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::L8) {
        Ok(()) => 1,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_is_two_renders_of_one_meadow() {
        // The property everything else here rests on. Not "the two look alike" —
        // they should not, that is the whole point — but that the geometry under
        // them is the same, which is what makes the expensive one a *target* for
        // the cheap one rather than a different picture of a different field.
        let page = Page::new(Vec2::new(-32.0, -32.0), 64, 64);
        let params = BakeParams {
            quality: GrassRenderQuality::Dataset,
            ..default()
        };
        let pair = Pair::bake(page, &params, GrassRenderQuality::Preview);
        assert!(pair.marks > 100, "the scene grew {} marks", pair.marks);
        assert_eq!(pair.input.colour.len(), 64 * 64);
        assert_eq!(pair.target.colour.len(), 64 * 64);

        // The canopy is the geometry, seen from the camera. If the two renders
        // were of different meadows this is where it would show, and it would
        // show far more loudly than in the colour.
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (low, high) = (
            mean(&pair.input.passes.height),
            mean(&pair.target.passes.height),
        );
        assert!(
            (low - high).abs() < high * 0.06 + 0.5,
            "the two renders disagree about the canopy: {low:.2} against {high:.2}"
        );

        // And they are genuinely different pictures, or the pair teaches nothing.
        let difference = pair
            .input
            .colour
            .iter()
            .zip(&pair.target.colour)
            .map(|(a, b)| (*a - *b).length() as f64)
            .sum::<f64>()
            / pair.input.colour.len() as f64;
        assert!(
            difference > 0.005,
            "the cheap and costly renders differ by {difference:.5} — there is \
             nothing here to learn"
        );
    }

    #[test]
    fn every_pass_is_filled_and_finite() {
        // A channel of zeroes or NaNs trains a network to produce zeroes or
        // NaNs, and neither shows up as anything but a loss that will not fall.
        let page = Page::new(Vec2::ZERO, 48, 48);
        let params = BakeParams {
            quality: GrassRenderQuality::Dataset,
            ..default()
        };
        let pair = Pair::bake(page, &params, GrassRenderQuality::Preview);
        let passes = &pair.target.passes;
        for (name, channel) in passes.scalars() {
            assert_eq!(channel.len(), 48 * 48, "{name} is the wrong size");
            assert!(
                channel.iter().all(|v| v.is_finite()),
                "{name} holds a non-finite value"
            );
            let spread = channel.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - channel.iter().cloned().fold(f32::INFINITY, f32::min);
            assert!(spread > 0.0, "{name} is constant across the whole page");
        }
        for (name, channel) in passes.vectors() {
            assert_eq!(channel.len(), 48 * 48, "{name} is the wrong size");
            assert!(
                channel.iter().all(|v| (v.length() - 1.0).abs() < 0.05),
                "{name} holds a normal that is not unit length"
            );
        }
    }

    #[test]
    fn a_crop_takes_the_middle_of_both() {
        let page = Page::new(Vec2::ZERO, 64, 64);
        let params = BakeParams {
            quality: GrassRenderQuality::Preview,
            ..default()
        };
        let pair = Pair::bake(page, &params, GrassRenderQuality::Preview);
        let (input, target, w, h) = pair.crop(8);
        assert_eq!((w, h), (48, 48));
        assert_eq!(input.len(), 48 * 48);
        assert_eq!(target.len(), 48 * 48);
        // The crop's top-left is the page's (8, 8).
        assert_eq!(input[0], pair.input.colour[8 * 64 + 8]);
        assert_eq!(target[0], pair.target.colour[8 * 64 + 8]);
        // And a margin larger than the page does not panic.
        let (_, _, w, _) = pair.crop(999);
        assert_eq!(w, 0);
    }

    #[test]
    fn metadata_records_where_the_sun_actually_was() {
        // In world degrees, not in whatever the image vector's `z` happens to
        // be. A corpus whose metadata says 35° and whose targets were lit at 55°
        // is worse than one with no metadata at all.
        let page = Page::new(Vec2::ZERO, 32, 32);
        let params = BakeParams::default();
        let meta = ShardMetadata::of(&page, &params, GrassRenderQuality::Preview, 1234);
        assert!(
            (meta.sun_elevation_degrees - crate::lab::DEFAULT_ELEVATION.to_degrees()).abs() < 0.5,
            "metadata says {}°",
            meta.sun_elevation_degrees
        );
        let ron = meta.to_ron();
        assert!(ron.starts_with('('), "{ron}");
        assert!(ron.contains("marks: 1234"));
        assert!(ron.contains("sun_elevation_degrees:"));
    }
}

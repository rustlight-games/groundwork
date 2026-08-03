//! Bake a plate of grass to a PNG, with no window and no GPU.
//!
//! This is the **eyeball loop**: one plate, one file, one number for how long it
//! took, and then you go and look at it. Getting there through a running game
//! means waiting on a window, a swapchain and a frame clock to see a picture
//! that never moves.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_bake
//! cargo run --release -p bw_grass --example grass_bake -- \
//!     --out /tmp/plate.png --size 1448x1086 --seed 7
//! ```
//!
//! It deliberately scores nothing. The suite that decides whether a change was
//! an improvement is `benches/bake.rs` for speed and the `grass_snapshot`
//! example for whether the picture moved; this is the thing you run in between,
//! when the question is still "what does it look like".
//!
//! ## Looking at it the size the player will
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_bake -- --view 13,26,35,48
//! ```
//!
//! Every art constant in this crate is expressed in cache pixels, and the cache
//! is baked at one fixed zoom — 96 pixels to the screen metre. The camera then
//! scales the finished page like any other sprite. So the size a plate is
//! *authored* at and the size it is *seen* at are different numbers, and only
//! one of them is the one that matters.
//!
//! They are further apart than they look. `BattleCamera::view_height` is world
//! metres visible vertically and defaults to 26, so on a 1080-pixel window the
//! ground shows at `1080 / 26 / 96` — about **43 percent**. At 35 metres it is
//! 32 percent. A judgement made on a 1:1 plate is being made at somewhere over
//! twice the scale anyone will ever see, and detail that reads as articulation
//! at 1:1 reliably reads as noise at a third of it.
//!
//! `--view` closes that gap. It works out how much world the screen covers at a
//! given camera height, bakes exactly that, and resamples it to the pixels the
//! window actually has.

use bevy::prelude::*;
use bw_grass::bake::{BakeParams, Page, bake, bake_grid};
use bw_grass::iso;
use bw_grass::surface::resample;

fn main() {
    let options = Options::parse();
    let mut params = BakeParams {
        // The example exists to judge the picture, so it renders the tier the
        // picture is judged at.
        quality: options.quality,
        seed: options.seed,
        ..default()
    };
    if let Some(scale) = options.density {
        params.tufts *= scale;
        params.thatch *= scale;
    }

    if !options.views.is_empty() {
        for metres in &options.views {
            render_view(&options, &params, *metres);
        }
        return;
    }

    println!(
        "baking {}x{} at seed {:#x}{}",
        options.width,
        options.height,
        options.seed,
        if options.tiled { ", tiled" } else { "" }
    );

    // `Instant` is denied workspace-wide because a wall clock inside the
    // simulation destroys determinism. This is a benchmark harness reporting how
    // long a bake took, which is the one job that genuinely needs one, and none
    // of what it measures feeds back into the plate.
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let region = Page::new(options.origin, options.width, options.height);
    let colours = if options.tiled {
        bake_grid(region, &params)
    } else {
        bake(region, &params)
    };
    let elapsed = started.elapsed();

    let pixels = (options.width * options.height) as f64;
    println!(
        "grass.page_bake  {:.3} s for {:.2} Mpx  ({:.1} ns/px, {:.1} ms per 256px page)",
        elapsed.as_secs_f64(),
        pixels / 1.0e6,
        elapsed.as_secs_f64() * 1.0e9 / pixels,
        elapsed.as_secs_f64() * 1.0e3 * (256.0 * 256.0) / pixels,
    );

    write_png(&options.out, &colours, options.width, options.height);
    println!("wrote {}", options.out);

    report_fields(&options, &params);
}

/// Bake exactly the ground one screenful covers at a camera height, and resample
/// it to the pixels that screen has.
///
/// `--ruler` is what this exists for beyond looking: an image of grass with
/// nothing in it has no scale, so the camera height cannot be chosen from it.
/// The snapshot suite renders the same views without the overlay; this one is
/// for deciding what the framing should be.
fn render_view(options: &Options, params: &BakeParams, metres: f32) {
    let (screen_w, screen_h) = options.screen;
    let (width, height, scale) = iso::view_pixels(metres, options.screen);

    println!(
        "\nview {metres:.0} m  →  {screen_w}x{screen_h} px of window over \
         {:.1}x{metres:.1} m of ground\n\
         \x20    baking {width}x{height} cache px, shown at {:.0}% ({:.1} screen px per metre)",
        metres * screen_w as f32 / screen_h as f32,
        scale * 100.0,
        screen_h as f32 / metres,
    );

    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let colours = bake_grid(Page::new(options.origin, width, height), params);
    let elapsed = started.elapsed();
    println!(
        "\x20    {:.2} s for {:.1} Mpx",
        elapsed.as_secs_f64(),
        (width * height) as f64 / 1.0e6
    );

    let mut shown = resample(&colours, width, height, screen_w, screen_h);
    if options.ruler {
        draw_ruler(&mut shown, screen_w, screen_h, metres);
    }
    let out = format!("{}_{metres:.0}m.png", options.out.trim_end_matches(".png"));
    write_png(&out, &shown, screen_w, screen_h);
    println!("\x20    wrote {out}");
}

/// Lay unit-sized markers and a ten-metre bar over a rendered view.
///
/// Choosing a camera height by looking at bare ground is guesswork, because an
/// image of grass with nothing in it has no scale — the same picture is a lawn
/// from two metres or a meadow from fifty, and the eye will happily read it
/// either way. What settles it is a thing of known size standing in the frame.
///
/// So these are 1.8 m tall and 0.6 m wide, a person, drawn at the same screen
/// scale the ground is. Height uses `Z_SCALE`, which is 1, so a metre straight
/// up is as long on screen as a metre along the projected X axis — the property
/// that makes a cube look like a cube in this projection, and the reason the
/// marker can be a plain rectangle rather than needing to be projected.
///
/// Crude on purpose. It answers one question — is a unit the right size on
/// screen at this camera height — and anything more finished would invite it to
/// be mistaken for art.
fn draw_ruler(pixels: &mut [Vec3], width: usize, height: usize, metres: f32) {
    let px_per_metre = height as f32 / metres;
    let body = Vec3::new(0.06, 0.05, 0.09);
    let trim = Vec3::new(0.74, 0.19, 0.16);

    let mut block = |x: f32, y: f32, w: f32, h: f32, colour: Vec3| {
        let (x0, y0) = (x.max(0.0) as usize, y.max(0.0) as usize);
        let (x1, y1) = (
            ((x + w) as usize).min(width),
            ((y + h) as usize).min(height),
        );
        for py in y0..y1 {
            for px in x0..x1 {
                pixels[py * width + px] = colour;
            }
        }
    };

    // A loose scatter rather than a row, so the markers sit at several depths
    // and the eye can judge crowding as well as size.
    let unit_w = 0.6 * px_per_metre;
    let unit_h = 1.8 * px_per_metre;
    for (fx, fy) in [
        (0.14, 0.30),
        (0.20, 0.34),
        (0.17, 0.40),
        (0.46, 0.55),
        (0.52, 0.60),
        (0.49, 0.66),
        (0.74, 0.28),
        (0.80, 0.44),
        (0.66, 0.78),
        (0.30, 0.80),
    ] {
        let x = fx * width as f32;
        let y = fy * height as f32;
        block(x, y, unit_w, unit_h, body);
        // A head, so the silhouette reads as a figure rather than as a post.
        block(
            x + unit_w * 0.2,
            y - unit_h * 0.22,
            unit_w * 0.6,
            unit_h * 0.24,
            trim,
        );
    }

    // A ten-metre bar, bottom left, with a tick at every metre.
    let bar_y = height as f32 - px_per_metre * 1.2;
    block(
        px_per_metre,
        bar_y,
        10.0 * px_per_metre,
        (px_per_metre * 0.08).max(2.0),
        Vec3::ZERO,
    );
    for step in 0..=10 {
        block(
            px_per_metre + step as f32 * px_per_metre,
            bar_y - px_per_metre * 0.22,
            (px_per_metre * 0.06).max(2.0),
            px_per_metre * 0.28,
            Vec3::ZERO,
        );
    }
}

/// What the composition fields are actually doing over this plate.
///
/// The fields decide where everything goes, and when one of them is quietly
/// clamped to nothing the plate does not look broken — it just looks a bit
/// plain, which is much harder to notice. Bare ground in particular went missing
/// twice while every other number in the table stayed healthy.
fn report_fields(options: &Options, params: &BakeParams) {
    let field = bw_grass::WorldField::new(params.seed);
    let (mut height, mut density, mut bare, mut crown) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let (mut lit_low, mut lit_high, mut lit_sq) = (0.0f32, 0.0f32, 0.0f64);
    let (mut peak_bare, mut exposed, mut fringe) = (0.0f32, 0usize, 0usize);
    let (mut voice, mut quiet, mut hero) = (0.0f64, 0usize, 0usize);
    let mut samples = 0usize;

    for y in (0..options.height).step_by(4) {
        for x in (0..options.width).step_by(4) {
            let ground = field.sample(bw_grass::iso::from_cache_ground(
                options.origin + Vec2::new(x as f32, y as f32),
            ));
            height += ground.height as f64;
            density += ground.density as f64;
            bare += ground.bare as f64;
            crown += ground.crown as f64;
            peak_bare = peak_bare.max(ground.bare);
            lit_low = lit_low.min(ground.lit);
            lit_high = lit_high.max(ground.lit);
            lit_sq += (ground.lit as f64) * (ground.lit as f64);
            if ground.bare > 0.35 {
                exposed += 1;
            } else if ground.bare > 0.08 {
                fringe += 1;
            }
            // The three intensity classes, at the thresholds the eye reads them
            // at rather than at thirds. `resolution` decides how loud a passage
            // is — how long the blades are, how dark their separations, how many
            // of them catch the light — and a field that is all one class is a
            // field speaking at one volume, which is the failure this split
            // exists to catch. It cannot be seen in any descriptor: a plate with
            // no quiet ground and a plate with plenty measure almost identically
            // on the ladders, because the ladders sum over the whole image.
            voice += ground.resolution as f64;
            if ground.resolution < 0.25 {
                quiet += 1;
            } else if ground.resolution > 0.75 {
                hero += 1;
            }
            samples += 1;
        }
    }

    let n = samples.max(1) as f64;
    let share = |count: usize| count as f32 / samples.max(1) as f32 * 100.0;
    println!(
        "fields: height {:.4} m  density {:.3}  crown {:.3}  bare {:.4} (peak {peak_bare:.3})  \
         exposed {:.2}%  fringe {:.2}%\n        lit rms {:.3} range {lit_low:.2}..{lit_high:.2}\
         \n        voice {:.3}  quiet {:.1}%  ordinary {:.1}%  hero {:.1}%",
        height / n,
        density / n,
        crown / n,
        bare / n,
        share(exposed),
        share(fringe),
        (lit_sq / n).sqrt(),
        voice / n,
        share(quiet),
        share(samples - quiet - hero),
        share(hero),
    );
}

fn write_png(path: &str, colours: &[Vec3], width: usize, height: usize) {
    let bytes = bw_grass::surface::to_rgb8(colours);
    image::save_buffer(
        path,
        &bytes,
        width as u32,
        height as u32,
        image::ColorType::Rgb8,
    )
    .expect("could not write the plate");
}

struct Options {
    /// Which tier to render. The example exists to judge the picture, so it
    /// defaults to the tier the picture is judged at.
    quality: bw_grass::GrassRenderQuality,
    out: String,
    width: usize,
    height: usize,
    seed: u64,
    origin: Vec2,
    tiled: bool,
    density: Option<f32>,
    /// Camera heights to render, in world metres visible vertically.
    ///
    /// The same unit as `bw_render::BattleCamera::view_height`, deliberately, so
    /// a number that looks right here can be typed straight into the camera.
    views: Vec<f32>,
    /// Overlay unit-sized markers and a ten-metre bar, to judge the framing.
    ruler: bool,
    /// Window size in pixels. Decides the scale as much as the height does.
    screen: (usize, usize),
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            quality: bw_grass::GrassRenderQuality::Reference,
            out: "grass_plate.png".to_string(),
            width: 1448,
            height: 1086,
            seed: 0x5eed_1234,
            origin: Vec2::new(-700.0, -540.0),
            tiled: false,
            density: None,
            views: Vec::new(),
            ruler: false,
            screen: (1920, 1080),
        };
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let mut index = 0;
        while index < arguments.len() {
            let value = |i: usize| arguments.get(i + 1).cloned().unwrap_or_default();
            match arguments[index].as_str() {
                "--out" => {
                    options.out = value(index);
                    index += 1;
                }
                "--size" => {
                    let raw = value(index);
                    if let Some((w, h)) = raw.split_once('x') {
                        options.width = w.parse().unwrap_or(options.width);
                        options.height = h.parse().unwrap_or(options.height);
                    }
                    index += 1;
                }
                "--seed" => {
                    options.seed = value(index).parse().unwrap_or(options.seed);
                    index += 1;
                }
                "--origin" => {
                    let raw = value(index);
                    if let Some((x, y)) = raw.split_once(',') {
                        options.origin =
                            Vec2::new(x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
                    }
                    index += 1;
                }
                "--density" => {
                    options.density = value(index).parse().ok();
                    index += 1;
                }
                "--view" => {
                    options.views = value(index)
                        .split(',')
                        .filter_map(|m| m.trim().parse::<f32>().ok())
                        .filter(|m| *m > 0.0)
                        .collect();
                    index += 1;
                }
                "--screen" => {
                    let raw = value(index);
                    if let Some((w, h)) = raw.split_once('x') {
                        options.screen = (
                            w.parse().unwrap_or(options.screen.0),
                            h.parse().unwrap_or(options.screen.1),
                        );
                    }
                    index += 1;
                }
                "--ruler" => options.ruler = true,
                "--tiled" => options.tiled = true,
                "--quality" => {
                    options.quality = match value(index).as_str() {
                        "preview" => bw_grass::GrassRenderQuality::Preview,
                        "dataset" => bw_grass::GrassRenderQuality::Dataset,
                        _ => bw_grass::GrassRenderQuality::Reference,
                    };
                    index += 1;
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
            index += 1;
        }
        options
    }
}

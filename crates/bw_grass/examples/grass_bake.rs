//! Bake a plate of grass to a PNG, with no window and no GPU.
//!
//! This is the iteration loop. Matching a piece of reference art is a numerical
//! exercise, and doing it through a running game means waiting on a window, a
//! swapchain and a frame clock to look at a picture that never moves.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_bake
//! cargo run --release -p bw_grass --example grass_bake -- \
//!     --out /tmp/plate.png --size 1448x1086 --seed 7 \
//!     --reference benchmarks/reference/grass_target.png
//! ```
//!
//! With `--reference`, it prints the descriptor table from
//! [`bw_grass::metrics`] side by side with the target's. Without it, just the
//! candidate's own numbers and the time the bake took.
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
use bw_grass::bake::{BakeParams, Page, bake};
use bw_grass::iso::PX_PER_METRE;
use bw_grass::metrics;
use rayon::prelude::*;

fn main() {
    let options = Options::parse();
    let mut params = BakeParams {
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
    let colours = if options.tiled {
        bake_tiled(&options, &params)
    } else {
        bake(
            Page::new(options.origin, options.width, options.height),
            &params,
        )
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

    let candidate = metrics::describe(&colours, options.width, options.height);
    match options.reference.as_ref().and_then(|path| read_png(path)) {
        Some((target, width, height)) => {
            let target = metrics::describe(&target, width, height);
            println!("\n{}", metrics::compare(&candidate, &target));
            println!(
                "grass.match.distance  {:.4}",
                metrics::distance(&candidate, &target)
            );
        }
        None => println!("\n{candidate:#?}"),
    }
}

/// Bake exactly the ground one screenful covers at a camera height, and resample
/// it to the pixels that screen has.
///
/// The two numbers this reconciles are set a long way apart in the codebase.
/// `bw_render::BattleCamera::view_height` is world metres visible vertically;
/// `bw_grass::iso::PX_PER_METRE` is how many cache pixels a screen metre is baked
/// at. Neither knows about the other, and the ratio between them — how much the
/// finished page is scaled down before anyone sees it — is not written anywhere
/// because it belongs to neither.
///
/// It is not a small ratio. At the default 26-metre camera on a 1080-pixel
/// window the ground is displayed at about 43 percent; at 35 metres, under a
/// third. Judging the plate at 1:1 is judging it at more than twice the size it
/// will ever be presented at, which is exactly the size at which "richly
/// detailed" and "busy" are hardest to tell apart.
fn render_view(options: &Options, params: &BakeParams, metres: f32) {
    let (screen_w, screen_h) = options.screen;
    // How much world the window covers. The projection is area-preserving and
    // 2:1 dimetric, so a screen metre is a world metre and the horizontal extent
    // is just the aspect ratio times the vertical one.
    let across = metres * screen_w as f32 / screen_h as f32;
    let width = (across * PX_PER_METRE).round() as usize;
    let height = (metres * PX_PER_METRE).round() as usize;
    let scale = screen_h as f32 / height as f32;

    println!(
        "\nview {metres:.0} m  →  {screen_w}x{screen_h} px of window over \
         {across:.1}x{metres:.1} m of ground\n\
         \x20    baking {width}x{height} cache px, shown at {:.0}% ({:.1} screen px per metre)",
        scale * 100.0,
        screen_h as f32 / metres,
    );

    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let colours = bake_grid(params, options.origin, width, height);
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

    // The detail ladder before and after, which is the only honest way to say
    // how much of the work survives to the screen. The reference comparison is
    // deliberately not run here: the target art is itself a 96-pixel-per-metre
    // image, so measuring a downsampled plate against it would compare two
    // different scales and report the difference as a defect.
    let full = metrics::describe(&colours, width, height);
    let seen = metrics::describe(&shown, screen_w, screen_h);
    println!(
        "\x20    detail r2/r4/r8   baked {:.4} {:.4} {:.4}   seen {:.4} {:.4} {:.4}",
        full.detail[0],
        full.detail[1],
        full.detail[2],
        seen.detail[0],
        seen.detail[1],
        seen.detail[2]
    );
    println!(
        "\x20    luma mean/dev     baked {:.4} {:.4}          seen {:.4} {:.4}",
        full.luma_mean, full.luma_deviation, seen.luma_mean, seen.luma_deviation
    );
    println!(
        "\x20    bright/soil share baked {:.4} {:.4}          seen {:.4} {:.4}",
        full.bright, full.soil, seen.bright, seen.soil
    );
}

/// Bake a large area as independent pages in parallel and stitch them.
///
/// A screenful at a wide camera is twenty megapixels, which is thirteen seconds
/// single-threaded. It is also the exact thing page independence is for, so the
/// tiled path is the honest one to use here as well as the fast one.
fn bake_grid(params: &BakeParams, origin: Vec2, width: usize, height: usize) -> Vec<Vec3> {
    const TILE: usize = 256;
    let across = width.div_ceil(TILE);
    let down = height.div_ceil(TILE);

    let tiles: Vec<(usize, usize, Vec<Vec3>)> = (0..across * down)
        .into_par_iter()
        .map(|index| {
            let (tx, ty) = (index % across, index / across);
            let w = TILE.min(width - tx * TILE);
            let h = TILE.min(height - ty * TILE);
            let at = origin + Vec2::new((tx * TILE) as f32, (ty * TILE) as f32);
            (tx, ty, bake(Page::new(at, w, h), params))
        })
        .collect();

    let mut plate = vec![Vec3::ZERO; width * height];
    for (tx, ty, tile) in tiles {
        let w = TILE.min(width - tx * TILE);
        let h = TILE.min(height - ty * TILE);
        for y in 0..h {
            let source = y * w;
            let target = (ty * TILE + y) * width + tx * TILE;
            plate[target..target + w].copy_from_slice(&tile[source..source + w]);
        }
    }
    plate
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

/// Area-average a plate down to a target size.
///
/// A box filter over each output pixel's exact footprint, fractional edges
/// included. That is what a correct mip chain converges to, so this shows the
/// *best case* for how the surface minifies — worth stating plainly, because
/// baked pages currently have no mip chain at all, and a GPU point-sampling a
/// page at a third of its size will alias considerably worse than this.
fn resample(
    source: &[Vec3],
    width: usize,
    height: usize,
    target_w: usize,
    target_h: usize,
) -> Vec<Vec3> {
    let sx = width as f32 / target_w as f32;
    let sy = height as f32 / target_h as f32;
    let mut out = vec![Vec3::ZERO; target_w * target_h];

    for y in 0..target_h {
        let (top, bottom) = (y as f32 * sy, (y as f32 + 1.0) * sy);
        for x in 0..target_w {
            let (left, right) = (x as f32 * sx, (x as f32 + 1.0) * sx);
            let mut total = Vec3::ZERO;
            let mut weight = 0.0f32;
            for py in top.floor() as usize..(bottom.ceil() as usize).min(height) {
                // Vertical overlap of this source row with the output pixel.
                let cover_y = (bottom.min(py as f32 + 1.0) - top.max(py as f32)).max(0.0);
                if cover_y <= 0.0 {
                    continue;
                }
                for px in left.floor() as usize..(right.ceil() as usize).min(width) {
                    let cover_x = (right.min(px as f32 + 1.0) - left.max(px as f32)).max(0.0);
                    if cover_x <= 0.0 {
                        continue;
                    }
                    let w = cover_x * cover_y;
                    total += source[py * width + px] * w;
                    weight += w;
                }
            }
            out[y * target_w + x] = if weight > 0.0 {
                total / weight
            } else {
                Vec3::ZERO
            };
        }
    }
    out
}

/// Bake the plate as a grid of independent pages and stitch them.
///
/// The point is not speed, though it is faster. It is that a stitched plate
/// shows page seams if there are any, and seams are the one failure mode of this
/// design that a single-page bake cannot possibly reveal.
fn bake_tiled(options: &Options, params: &BakeParams) -> Vec<Vec3> {
    const TILE: usize = 256;
    let across = options.width.div_ceil(TILE);
    let down = options.height.div_ceil(TILE);

    let tiles: Vec<(usize, usize, Vec<Vec3>)> = (0..across * down)
        .into_par_iter()
        .map(|index| {
            let (tx, ty) = (index % across, index / across);
            let width = TILE.min(options.width - tx * TILE);
            let height = TILE.min(options.height - ty * TILE);
            let origin = options.origin + Vec2::new((tx * TILE) as f32, (ty * TILE) as f32);
            (tx, ty, bake(Page::new(origin, width, height), params))
        })
        .collect();

    let mut plate = vec![Vec3::ZERO; options.width * options.height];
    for (tx, ty, tile) in tiles {
        let width = TILE.min(options.width - tx * TILE);
        let height = TILE.min(options.height - ty * TILE);
        for y in 0..height {
            let source = y * width;
            let target = (ty * TILE + y) * options.width + tx * TILE;
            plate[target..target + width].copy_from_slice(&tile[source..source + width]);
        }
    }
    plate
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
            samples += 1;
        }
    }

    let n = samples.max(1) as f64;
    println!(
        "fields: height {:.4} m  density {:.3}  crown {:.3}  bare {:.4} (peak {peak_bare:.3})  \
         exposed {:.2}%  fringe {:.2}%\n        lit rms {:.3} range {lit_low:.2}..{lit_high:.2}",
        height / n,
        density / n,
        crown / n,
        bare / n,
        exposed as f32 / samples.max(1) as f32 * 100.0,
        fringe as f32 / samples.max(1) as f32 * 100.0,
        (lit_sq / n).sqrt(),
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

fn read_png(path: &str) -> Option<(Vec<Vec3>, usize, usize)> {
    let image = image::open(path).ok()?.to_rgb8();
    let (width, height) = (image.width() as usize, image.height() as usize);
    let pixels = image
        .pixels()
        .map(|p| Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0)
        .collect();
    Some((pixels, width, height))
}

struct Options {
    out: String,
    width: usize,
    height: usize,
    seed: u64,
    origin: Vec2,
    reference: Option<String>,
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
            out: "grass_plate.png".to_string(),
            width: 1448,
            height: 1086,
            seed: 0x5eed_1234,
            origin: Vec2::new(-700.0, -540.0),
            reference: None,
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
                "--reference" => {
                    options.reference = Some(value(index));
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
                other => eprintln!("ignoring unknown argument {other}"),
            }
            index += 1;
        }
        options
    }
}

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

use bevy::prelude::*;
use bw_grass::bake::{BakeParams, Page, bake};
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
                "--tiled" => options.tiled = true,
                other => eprintln!("ignoring unknown argument {other}"),
            }
            index += 1;
        }
        options
    }
}

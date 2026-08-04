//! Trace the pages the game will ask for, so `./run` shows the good grass.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_prebake
//! cargo run --release -p bw_grass --example grass_prebake -- --radius 4 --samples 256
//! ```
//!
//! Cycles takes seconds a page and the game has a frame, so the path tracer can
//! never run in the render loop. It does not have to: a page is a cache whose
//! content is a pure function of the world coordinate and the seed, so a page
//! traced now is exactly the page the game would draw if it had the time.
//!
//! This walks the same page grid [`bw_grass::plugin`] streams, traces each one,
//! and stores it where [`bw_grass::cache`] will find it. Ground that has not
//! been traced falls back to the rasteriser, so the game always runs — the
//! picture just improves as the cache fills.
//!
//! Pages already in the cache are skipped, so an interrupted run resumes.

use std::path::PathBuf;

use bevy::prelude::*;
use bw_grass::bake::{BakeParams, Page};
use bw_grass::cache;
use bw_grass::cycles::{self, RenderSettings};
use bw_grass::field::WorldField;
use bw_grass::plugin::PAGE_PIXELS;
use bw_grass::scene::GrassScene;

fn main() {
    let options = Options::parse();
    let blender = cycles::blender_path();
    if !blender.exists() && blender != PathBuf::from("blender") {
        eprintln!("no Blender at {}; set BW_BLENDER", blender.display());
        std::process::exit(1);
    }

    let mut params = BakeParams {
        seed: options.seed,
        ..default()
    };
    // The same canopy the look was tuned to. See `grass_cycles` for why the
    // rasteriser's own counts are the wrong quantity for a path tracer.
    params.tufts *= options.density;
    params.fine *= options.density;
    params.thatch *= options.density;
    params.leaves *= options.density;
    params.blade_length.0 *= options.length;
    params.blade_length.1 *= options.length;

    let side = options.radius * 2 + 1;
    let total = side * side;
    println!(
        "tracing {total} pages of {PAGE_PIXELS}² around the origin at {} spp",
        options.samples
    );
    println!("  cache {}", cache::directory().display());
    println!("  blender {}", blender.display());

    let field = WorldField::lit_by(params.seed, params.light);
    let scratch = std::env::temp_dir().join("bw-grass-prebake");
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();

    let (mut traced, mut skipped, mut failed) = (0usize, 0usize, 0usize);
    let span = PAGE_PIXELS as f32;
    for row in 0..side {
        for column in 0..side {
            let coordinate = IVec2::new(
                column as i32 - options.radius as i32,
                row as i32 - options.radius as i32,
            );
            // Exactly the page the streaming plugin will ask for. Any drift here
            // and every trace misses its own cache entry.
            let page = Page::new(coordinate.as_vec2() * span, PAGE_PIXELS, PAGE_PIXELS);

            if !options.force && cache::load(&page, &params).is_some() {
                skipped += 1;
                continue;
            }

            let settings = RenderSettings {
                samples: options.samples,
                ..default()
            };
            let grown = GrassScene::build(page, &field, &params);
            let scene = cycles::CyclesScene::build(&grown, &field, settings);
            let png = scratch.join("page.png");
            let _ = std::fs::remove_file(&png);

            let header = match scene.write(&scratch) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("  {coordinate}: cannot write scene: {error}");
                    failed += 1;
                    continue;
                }
            };
            match cycles::render(&header, &png, &blender) {
                Ok(output) if png.exists() && output.status.success() => {}
                Ok(output) => {
                    eprintln!(
                        "  {coordinate}: blender produced nothing\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    failed += 1;
                    continue;
                }
                Err(error) => {
                    eprintln!("  {coordinate}: cannot run blender: {error}");
                    failed += 1;
                    continue;
                }
            }

            match to_rgba(&png, PAGE_PIXELS) {
                Some(bytes) => match cache::store(&page, &params, &bytes) {
                    Ok(_) => traced += 1,
                    Err(error) => {
                        eprintln!("  {coordinate}: cannot store: {error}");
                        failed += 1;
                    }
                },
                None => failed += 1,
            }

            let done = traced + skipped + failed;
            if done % 4 == 0 || done == total {
                println!(
                    "  {done}/{total}  traced {traced}  skipped {skipped}  failed {failed}  \
                     ({:.0} s elapsed)",
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }

    let _ = std::fs::remove_dir_all(&scratch);
    println!(
        "\n{traced} traced, {skipped} already cached, {failed} failed in {:.0} s",
        started.elapsed().as_secs_f64()
    );
    println!("{} pages in the cache", cache::count());
    if failed > 0 {
        std::process::exit(1);
    }
    println!("\n./run will now show traced pages where they exist.");
}

/// Read the traced PNG as the RGBA bytes a texture wants.
fn to_rgba(path: &std::path::Path, side: usize) -> Option<Vec<u8>> {
    let image = match image::open(path) {
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            eprintln!("  cannot read {}: {error}", path.display());
            return None;
        }
    };
    if image.width() as usize != side || image.height() as usize != side {
        eprintln!(
            "  {} is {}x{}, expected {side}²",
            path.display(),
            image.width(),
            image.height()
        );
        return None;
    }
    Some(image.into_raw())
}

struct Options {
    radius: usize,
    seed: u64,
    samples: u32,
    density: f32,
    length: f32,
    force: bool,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            // Nine pages, which is about what one screen covers at the default
            // camera height — enough to see the difference without committing an
            // afternoon to it.
            radius: 1,
            seed: BakeParams::default().seed,
            samples: 192,
            density: 8.0,
            length: 1.6,
            force: false,
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--radius" => options.radius = value().parse().unwrap_or(options.radius),
                "--seed" => options.seed = value().parse().unwrap_or(options.seed),
                "--samples" => options.samples = value().parse().unwrap_or(options.samples),
                "--density" => options.density = value().parse().unwrap_or(options.density),
                "--length" => options.length = value().parse().unwrap_or(options.length),
                "--force" => options.force = true,
                "--help" | "-h" => {
                    println!(
                        "grass_prebake [--radius N] [--seed N] [--samples N]\n\
                         \x20             [--density N] [--length N] [--force]"
                    );
                    std::process::exit(0);
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
        }
        options
    }
}

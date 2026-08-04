//! Trace the pages the game will ask for, so `./run` shows the good grass.
//!
//! ```sh
//! cargo run --release -p terrain_bevy --example grass_prebake
//! cargo run --release -p terrain_bevy --example grass_prebake -- --radius 4 --samples 256
//! ```
//!
//! Cycles takes seconds a page and the game has a frame, so the path tracer can
//! never run in the render loop. It does not have to: a page is a cache whose
//! content is a pure function of the world coordinate and the seed, so a page
//! traced now is exactly the page the game would draw if it had the time.
//!
//! This walks the same page grid [`terrain_bevy::plugin`] streams, traces each one,
//! and stores it where [`terrain_bevy::cache`] will find it. Ground that has not
//! been traced falls back to the rasteriser, so the game always runs — the
//! picture just improves as the cache fills.
//!
//! Pages already in the cache are skipped, so an interrupted run resumes.

use glam::IVec2;

use terrain_bake::bake::BakeParams;
use terrain_bevy::cache;
use terrain_bevy::plugin::PAGE_PIXELS;
use terrain_cycles::cycles::{self, RenderSettings};
use terrain_generators::field::WorldField;
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;

fn main() {
    let options = Options::parse();
    let blender = cycles::blender_path();
    if !blender.exists() && blender.as_os_str() != "blender" {
        eprintln!("no Blender at {}; set TERRAIN_BLENDER", blender.display());
        std::process::exit(1);
    }

    let mut params = BakeParams {
        seed: options.seed,
        ..Default::default()
    };
    // The same canopy the look was tuned to. See `grass_cycles` for why the
    // rasteriser's own counts are the wrong quantity for a path tracer.
    params.style.tufts *= options.density;
    params.style.fine *= options.density;
    params.style.thatch *= options.density;
    params.style.leaves *= options.density;
    params.style.blade_length.0 *= options.length;
    params.style.blade_length.1 *= options.length;

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

            // ## Why the trace is not done at the page's own resolution
            //
            // A page is baked at 96 pixels to the metre, and a grass blade is a
            // few millimetres wide — under half a pixel. The rasteriser copes
            // because it draws *marks*, which have a minimum width by
            // construction. Cycles draws geometry, and geometry thinner than a
            // pixel does not become a fine blade: it becomes a partially covered
            // pixel, which at this density averages the whole canopy into a flat
            // wash. The first traced page shipped like that and read as a square
            // of grey-green felt dropped into the field.
            //
            // So the same patch of world is traced at several times the density
            // and box-filtered down. Every output pixel then integrates a dozen
            // real blades instead of sampling one badly.
            let fine = Page::at_detail(
                page.origin * options.supersample as f32,
                PAGE_PIXELS * options.supersample,
                PAGE_PIXELS * options.supersample,
                options.supersample as f32,
            );

            if !options.force && cache::load_from(&cache::directory(), &page, &params).is_some() {
                skipped += 1;
                continue;
            }

            let settings = RenderSettings {
                samples: options.samples,
                ..Default::default()
            };
            let grown = GrassScene::build(fine, &field, &params.grass());
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

            match to_rgba(&png, PAGE_PIXELS * options.supersample)
                .map(|bytes| downsample(&bytes, PAGE_PIXELS, options.supersample))
            {
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
    println!("\nTERRAIN_GRASS_TRACED=1 ./run   to see them.");
    println!("Pages outside the traced region fall back to the rasteriser, which");
    println!("is a different picture — trace a radius that covers the view.");
}

/// Box-filter a traced page down to the size the game stores.
///
/// A plain box average, which is the right filter here rather than a lazy one: the
/// samples being combined are a regular grid over one output pixel's own footprint,
/// so every one of them belongs to it equally. Anything sharper would be inventing
/// contrast the trace did not produce.
fn downsample(rgba: &[u8], side: usize, factor: usize) -> Vec<u8> {
    if factor <= 1 {
        return rgba.to_vec();
    }
    let source = side * factor;
    let area = (factor * factor) as u32;
    let mut out = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let mut sums = [0u32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let index = ((y * factor + dy) * source + x * factor + dx) * 4;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += rgba[index + channel] as u32;
                    }
                }
            }
            for sum in sums {
                out.push((sum / area) as u8);
            }
        }
    }
    out
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
    supersample: usize,
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
            // Three. A blade is then a pixel and a half rather than a third of
            // one, which is the point where the canopy stops averaging itself
            // away. Four is visibly better still and costs nearly twice as much.
            supersample: 3,
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
                "--supersample" => {
                    options.supersample =
                        value().parse().unwrap_or(options.supersample).clamp(1, 6);
                }
                "--seed" => options.seed = value().parse().unwrap_or(options.seed),
                "--samples" => options.samples = value().parse().unwrap_or(options.samples),
                "--density" => options.density = value().parse().unwrap_or(options.density),
                "--length" => options.length = value().parse().unwrap_or(options.length),
                "--force" => options.force = true,
                "--help" | "-h" => {
                    println!(
                        "grass_prebake [--radius N] [--supersample N] [--seed N] [--samples N]\n\
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

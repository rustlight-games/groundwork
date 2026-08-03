//! Photograph the laboratory plate.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_lab
//! cargo run --release -p bw_grass --example grass_lab -- --sweep
//! cargo run --release -p bw_grass --example grass_lab -- --azimuth 90 --elevation 20
//! cargo run --release -p bw_grass --example grass_lab -- --quality preview
//! ```
//!
//! `--sweep` is the one to run after any lighting change. It photographs the
//! same plate with the key at four bearings and lays them out in a row, so the
//! question "does the lit side follow the sun" is answered by looking at one
//! image rather than by remembering what the last one looked like.
//!
//! Everything lands under `target/grass-lab/`. Working state, not committed
//! art — the plate is an instrument, and its readings belong with the build.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bw_grass::bake::BakeParams;
use bw_grass::lab::{self, Fixture, Key, Lab};
use bw_grass::quality::GrassRenderQuality;

fn main() {
    let options = Options::parse();
    let directory = PathBuf::from("target/grass-lab");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!("cannot create {}: {error}", directory.display());
        std::process::exit(1);
    }

    let params = BakeParams::default();
    println!(
        "laboratory plate — {} quality, {} fixtures, {:.0}px/m",
        options.quality.name(),
        Fixture::ALL.len(),
        options.px_per_metre,
    );
    println!();
    for (index, fixture) in Fixture::ALL.iter().enumerate() {
        let (column, row) = (index % lab::COLUMNS, index / lab::COLUMNS);
        println!("  r{row} c{column}  {}", fixture.name());
    }
    println!();

    if options.sweep {
        sweep(&options, &params, &directory);
    } else {
        let lab = options.lab(options.azimuth);
        // `Instant` is denied workspace-wide because a wall clock inside the
        // simulation would break determinism. A headless example that only
        // prints how long it took is exactly the case the rule is not for.
        #[allow(clippy::disallowed_types)]
        let started = std::time::Instant::now();
        let colours = lab::bake_lab(&lab, &params);
        let (width, height) = lab.size();
        let elapsed = started.elapsed();
        let path = directory.join(format!(
            "plate-{}-az{:03.0}-el{:02.0}.png",
            options.quality.name(),
            options.azimuth.to_degrees(),
            options.elevation.to_degrees(),
        ));
        write_png(&path, &colours, width, height).expect("write plate");
        println!(
            "{width}×{height} in {:.2} s → {}",
            elapsed.as_secs_f64(),
            path.display()
        );
    }
}

/// Four bearings of the same plate, laid out left to right.
fn sweep(options: &Options, params: &BakeParams, directory: &Path) {
    const BEARINGS: [f32; 4] = [0.0, 90.0, 180.0, 270.0];
    let mut plates = Vec::new();
    let mut size = (0usize, 0usize);
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    for bearing in BEARINGS {
        let lab = options.lab(bearing.to_radians());
        size = lab.size();
        plates.push(lab::bake_lab(&lab, params));
    }

    // A one-pixel gap between plates, in the palette's darkest grass, so the
    // boundary is legible without being a white line that drags the eye.
    const GAP: usize = 4;
    let (cell_w, cell_h) = size;
    let width = cell_w * BEARINGS.len() + GAP * (BEARINGS.len() - 1);
    let divider = bw_grass::palette::shade(bw_grass::palette::Tone::Thatch, 0.0);
    let mut strip = vec![divider; width * cell_h];
    for (index, plate) in plates.iter().enumerate() {
        let left = index * (cell_w + GAP);
        for y in 0..cell_h {
            let source = y * cell_w;
            let target = y * width + left;
            strip[target..target + cell_w].copy_from_slice(&plate[source..source + cell_w]);
        }
    }

    let path = directory.join(format!("sweep-{}.png", options.quality.name()));
    write_png(&path, &strip, width, cell_h).expect("write sweep");
    println!(
        "four bearings {:?}° in {:.2} s → {}",
        BEARINGS,
        started.elapsed().as_secs_f64(),
        path.display()
    );
    println!();
    println!("Read it as: the lit edge of every blade in `twist-fan` should walk");
    println!("round the compass from panel to panel. If it does not move, the");
    println!("lighting is not reading a normal.");
}

fn write_png(path: &Path, colours: &[Vec3], width: usize, height: usize) -> image::ImageResult<()> {
    let bytes = bw_grass::surface::to_rgb8(colours);
    image::save_buffer(
        path,
        &bytes,
        width as u32,
        height as u32,
        image::ColorType::Rgb8,
    )
}

struct Options {
    azimuth: f32,
    elevation: f32,
    quality: GrassRenderQuality,
    px_per_metre: f32,
    seed: u64,
    sweep: bool,
}

impl Options {
    fn lab(&self, azimuth: f32) -> Lab {
        Lab {
            seed: self.seed,
            key: Key {
                azimuth,
                elevation: self.elevation,
            },
            quality: self.quality,
            px_per_metre: self.px_per_metre,
        }
    }

    fn parse() -> Self {
        let default = Lab::default();
        let mut options = Self {
            azimuth: 0.0,
            elevation: lab::DEFAULT_ELEVATION,
            quality: default.quality,
            px_per_metre: default.px_per_metre,
            seed: default.seed,
            sweep: false,
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--sweep" => options.sweep = true,
                "--azimuth" => {
                    options.azimuth = value().parse::<f32>().unwrap_or(0.0).to_radians();
                }
                "--elevation" => {
                    options.elevation = value().parse::<f32>().unwrap_or(35.0).to_radians();
                }
                "--px-per-metre" => {
                    options.px_per_metre = value().parse().unwrap_or(options.px_per_metre);
                }
                "--seed" => {
                    let text = value();
                    options.seed = text
                        .strip_prefix("0x")
                        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                        .or_else(|| text.parse().ok())
                        .unwrap_or(options.seed);
                }
                "--quality" => {
                    options.quality = match value().as_str() {
                        "preview" => GrassRenderQuality::Preview,
                        "dataset" => GrassRenderQuality::Dataset,
                        _ => GrassRenderQuality::Reference,
                    }
                }
                "--help" | "-h" => {
                    println!("grass_lab [--sweep] [--azimuth DEG] [--elevation DEG]");
                    println!("          [--quality preview|dataset|reference]");
                    println!("          [--px-per-metre N] [--seed N]");
                    std::process::exit(0);
                }
                other => eprintln!("ignoring {other}"),
            }
        }
        options
    }
}

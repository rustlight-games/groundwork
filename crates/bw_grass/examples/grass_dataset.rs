//! Generate paired training data.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_dataset
//! cargo run --release -p bw_grass --example grass_dataset -- --shards 64 --aovs
//! cargo run --release -p bw_grass --example grass_dataset -- --out target/corpus
//! ```
//!
//! Each shard is one patch of ground rendered twice — cheaply and expensively —
//! from **one scene**, cropped to its middle, with the structure the cheap
//! render cannot see written out beside it. See [`bw_grass::dataset`] for why
//! each of those three things is a requirement rather than a convenience.
//!
//! Shards land under `target/grass-dataset/` by default. They are working state:
//! a corpus is regenerated from a seed and a renderer version, not archived.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bw_grass::bake::{BakeParams, Page};
use bw_grass::dataset::{Pair, ShardMetadata};
use bw_grass::quality::GrassRenderQuality;
use rayon::prelude::*;

fn main() {
    let options = Options::parse();
    if let Err(error) = std::fs::create_dir_all(&options.out) {
        eprintln!("cannot create {}: {error}", options.out.display());
        std::process::exit(1);
    }

    println!(
        "{} shards of {}² at {}, cropped from {}²",
        options.shards,
        options.crop,
        options.target.name(),
        options.page,
    );
    println!(
        "  input {} · margin {} px · {} px/m",
        options.input.name(),
        options.margin(),
        options.px_per_metre,
    );
    println!();

    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let done: usize = (0..options.shards)
        .into_par_iter()
        .map(|shard| {
            let params = BakeParams {
                seed: options.seed_for(shard),
                quality: options.target,
                ..default()
            };
            let page = Page::at_detail(
                options.origin_for(shard),
                options.page,
                options.page,
                options.px_per_metre / bw_grass::iso::PX_PER_METRE,
            );
            let pair = Pair::bake(page, &params, options.input);
            let (input, target, w, h) = pair.crop(options.margin());

            let stem = options.out.join(format!("{shard:05}"));
            let mut wrote = 0usize;
            wrote += write_rgb(&stem, "input", &input, w, h);
            wrote += write_rgb(&stem, "target", &target, w, h);
            if options.aovs {
                let passes = &pair.target.passes;
                let margin = options.margin();
                for (name, channel) in passes.scalars() {
                    let cropped = crop_scalar(channel, options.page, margin);
                    wrote += write_grey(&stem, name, &cropped, w, h);
                }
                for (name, channel) in passes.vectors() {
                    let cropped = crop_vector(channel, options.page, margin);
                    // Normals are signed; encoded the usual way so they can be
                    // looked at as well as read back.
                    let encoded: Vec<Vec3> = cropped
                        .iter()
                        .map(|n| *n * 0.5 + Vec3::splat(0.5))
                        .collect();
                    wrote += write_rgb(&stem, name, &encoded, w, h);
                }
            }

            let meta = ShardMetadata::of(&page, &params, options.input, pair.marks);
            let _ = std::fs::write(stem.with_extension("ron"), meta.to_ron());
            wrote
        })
        .sum();

    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "{} shards, {done} images, {:.1} s ({:.2} s per shard) → {}",
        options.shards,
        elapsed,
        elapsed / options.shards.max(1) as f64,
        options.out.display()
    );
}

/// How much of each edge is thrown away.
///
/// Not decoration. Every neighbourhood-reading term in the renderer — occlusion,
/// the relief comparison, the shadows themselves — is wrong near a page edge,
/// and a corpus of crops baked at their own size teaches a network that page
/// borders exist. It will then faithfully reproduce them.
const CROP_MARGIN: usize = 96;

fn crop_scalar(source: &[f32], side: usize, margin: usize) -> Vec<f32> {
    let cw = side - margin * 2;
    let mut out = Vec::with_capacity(cw * cw);
    for row in 0..cw {
        let start = (row + margin) * side + margin;
        out.extend_from_slice(&source[start..start + cw]);
    }
    out
}

fn crop_vector(source: &[Vec3], side: usize, margin: usize) -> Vec<Vec3> {
    let cw = side - margin * 2;
    let mut out = Vec::with_capacity(cw * cw);
    for row in 0..cw {
        let start = (row + margin) * side + margin;
        out.extend_from_slice(&source[start..start + cw]);
    }
    out
}

fn write_rgb(stem: &Path, name: &str, colours: &[Vec3], w: usize, h: usize) -> usize {
    let bytes = bw_grass::surface::to_rgb8(colours);
    let path = stem.with_file_name(format!(
        "{}-{name}.png",
        stem.file_name().unwrap().to_string_lossy()
    ));
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
/// Normalised per channel rather than globally, and the range goes in the
/// metadata-free filename only because these are for *looking at*. A trainer
/// reading them back would want the raw floats; that is a different exporter and
/// this one is an instrument.
fn write_grey(stem: &Path, name: &str, values: &[f32], w: usize, h: usize) -> usize {
    let low = values.iter().cloned().fold(f32::INFINITY, f32::min);
    let high = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let span = (high - low).max(1.0e-6);
    let bytes: Vec<u8> = values
        .iter()
        .map(|v| (((v - low) / span).clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    let path = stem.with_file_name(format!(
        "{}-{name}.png",
        stem.file_name().unwrap().to_string_lossy()
    ));
    match image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::L8) {
        Ok(()) => 1,
        Err(error) => {
            eprintln!("{}: {error}", path.display());
            0
        }
    }
}

struct Options {
    shards: usize,
    page: usize,
    crop: usize,
    px_per_metre: f32,
    seed: u64,
    target: GrassRenderQuality,
    input: GrassRenderQuality,
    aovs: bool,
    out: PathBuf,
}

impl Options {
    fn margin(&self) -> usize {
        CROP_MARGIN.min((self.page.saturating_sub(self.crop)) / 2)
    }

    /// A stable seed per shard.
    ///
    /// Every shard is its own *world* rather than its own patch of one world,
    /// which is deliberate. Crops from one world share its regional hue, its
    /// density and its flow, so a corpus drawn from a single seed is far less
    /// varied than its size suggests — and a validation split cut from the same
    /// world is not a held-out sample at all.
    fn seed_for(&self, shard: usize) -> u64 {
        self.seed
            .wrapping_add((shard as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    /// Where in that world to stand.
    fn origin_for(&self, shard: usize) -> Vec2 {
        let step = (shard as f32) * 977.0;
        Vec2::new(step % 8191.0 - 4096.0, (step * 1.618) % 7817.0 - 3908.0)
    }

    fn parse() -> Self {
        let mut options = Self {
            shards: 8,
            page: 448,
            crop: 256,
            px_per_metre: bw_grass::iso::PX_PER_METRE,
            seed: 0x9a55_0001,
            target: GrassRenderQuality::Dataset,
            input: GrassRenderQuality::Preview,
            aovs: false,
            out: PathBuf::from("target/grass-dataset"),
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--shards" => options.shards = value().parse().unwrap_or(options.shards),
                "--page" => options.page = value().parse().unwrap_or(options.page),
                "--crop" => options.crop = value().parse().unwrap_or(options.crop),
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
                "--reference" => options.target = GrassRenderQuality::Reference,
                "--aovs" => options.aovs = true,
                "--out" => options.out = PathBuf::from(value()),
                "--help" | "-h" => {
                    println!("grass_dataset [--shards N] [--page PX] [--crop PX]");
                    println!("              [--px-per-metre N] [--seed N]");
                    println!("              [--reference] [--aovs] [--out DIR]");
                    std::process::exit(0);
                }
                other => eprintln!("ignoring {other}"),
            }
        }
        options
    }
}

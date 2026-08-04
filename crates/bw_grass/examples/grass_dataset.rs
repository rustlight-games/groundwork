//! Generate paired training data.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_dataset
//! cargo run --release -p bw_grass --example grass_dataset -- --shards 64 --aovs
//! cargo run --release -p bw_grass --example grass_dataset -- --out target/corpus
//! ```
//!
//! Each shard is one patch of ground rendered twice from **one scene**, cropped
//! to its middle, with the structure the cheap render cannot see written out
//! beside it. See [`bw_grass::dataset`] for why each of those three things is a
//! requirement rather than a convenience.
//!
//! The two renders are not two budgets of one renderer. The input is the
//! rasteriser — fast enough for a game, and unable to integrate a hemisphere.
//! The target is Cycles, which integrates it and takes seconds. Learning the
//! second from the first is the entire point, so a corpus whose target was an
//! expensive *rasterisation* would be teaching a network to reproduce the
//! approximations rather than to replace them. `--raster` falls back to that
//! older pairing for when Blender is not available.
//!
//! Like `grass_cycles`, this is now an argument parser over a library driver:
//! the rules that decide whether a corpus is usable live in
//! [`bw_grass::dataset::generate`], where a second entry point can reach them.
//!
//! Shards land under `target/grass-dataset/` by default. They are working state:
//! a corpus is regenerated from a seed and a renderer version, not archived.

use std::path::PathBuf;

use bw_grass::cycles;
use bw_grass::dataset::{self, CorpusRequest};
use bw_grass::quality::GrassRenderQuality;

fn main() {
    let request = parse();

    println!(
        "{} shards of {}² at {}, cropped from {}²",
        request.shards,
        request.crop,
        request.target.name(),
        request.page,
    );
    println!(
        "  input {} (raster) · target {} · margin {} px · {} px/m",
        request.input.name(),
        if request.raster {
            "raster".to_string()
        } else {
            format!("cycles {} spp", request.samples)
        },
        request.margin(),
        request.px_per_metre,
    );
    if !request.raster {
        println!("  tracing with {}", cycles::blender_path().display());
    }
    println!();

    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let mut progress = |_shard: usize, _images: usize| {};
    let report = match dataset::generate(&request, &mut progress) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("cannot write {}: {error}", request.out.display());
            std::process::exit(1);
        }
    };

    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "{} shards, {} images, {elapsed:.1} s ({:.2} s per shard) → {}",
        report.shards,
        report.images,
        elapsed / report.shards.max(1) as f64,
        request.out.display()
    );
    if report.failed > 0 {
        eprintln!("{} shards produced nothing", report.failed);
        std::process::exit(1);
    }
}

fn parse() -> CorpusRequest {
    let mut request = CorpusRequest::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().unwrap_or_default();
        match argument.as_str() {
            "--shards" => request.shards = value().parse().unwrap_or(request.shards),
            "--page" => request.page = value().parse().unwrap_or(request.page),
            "--crop" => request.crop = value().parse().unwrap_or(request.crop),
            "--px-per-metre" => {
                request.px_per_metre = value().parse().unwrap_or(request.px_per_metre);
            }
            "--seed" => {
                let text = value();
                request.seed = text
                    .strip_prefix("0x")
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                    .or_else(|| text.parse().ok())
                    .unwrap_or(request.seed);
            }
            "--reference" => {
                request.target = GrassRenderQuality::Reference;
                request.samples = 512;
            }
            "--samples" => request.samples = value().parse().unwrap_or(request.samples),
            "--density" => request.density = value().parse().unwrap_or(request.density),
            "--length" => request.length = value().parse().unwrap_or(request.length),
            "--raster" => request.raster = true,
            "--aovs" => request.aovs = true,
            "--out" => request.out = PathBuf::from(value()),
            "--help" | "-h" => {
                println!("grass_dataset [--shards N] [--page PX] [--crop PX]");
                println!("              [--px-per-metre N] [--seed N] [--samples N]");
                println!("              [--density N] [--length N]");
                println!("              [--reference] [--aovs] [--raster] [--out DIR]");
                std::process::exit(0);
            }
            other => eprintln!("ignoring {other}"),
        }
    }
    request
}

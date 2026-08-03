//! Score the grass across all ten fixed seeds and write a comparable report.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_score
//! cargo run --release -p bw_grass --example grass_score -- --out benchmarks/grass.ron
//! ```
//!
//! This is the durable measurement; `grass_bake` is the iteration loop. It bakes
//! one plate per seed in [`bw_bench::SEEDS`], describes each against the
//! reference plate, prints the per-seed table and the across-seed variety, and
//! writes a [`bw_bench::Report`] that can be compared against a committed
//! baseline.
//!
//! ## What is worth watching here
//!
//! `grass.match.distance` is the headline, and on its own it is not enough: it
//! is a weighted mean, and a mean hides a generator that got the tone right
//! while losing its stroke language. The two ladders underneath it are what
//! diagnose that, and `grass.variety.*` is what catches the failure no
//! comparison against a single plate ever will — a generator whose ten seeds all
//! produce the same field. A variety near zero means the seed stopped mattering,
//! and that is invisible in every other number in this file.

use bevy::prelude::*;
use bw_bench::{Measurement, Report, SEEDS, Unit};
use bw_grass::PAGE_PIXELS;
use bw_grass::bake::{BakeParams, Page, bake};
use bw_grass::metrics::{self, Descriptors};
use rayon::prelude::*;

/// Plate size each seed is scored at, in cache pixels.
///
/// The reference plate's own size. Every descriptor here is measured in pixels,
/// so scoring at a different size would compare a texture against itself at the
/// wrong scale and quietly report the stroke language as wrong.
const WIDTH: usize = 1448;
const HEIGHT: usize = 1086;

/// Where in each world the plates are taken from, in cache pixels.
///
/// Far enough apart that no two share a mound, a regional drift or a clump
/// field. Append-only for the same reason the seeds are: a place that moves
/// makes this month's numbers incomparable with last month's.
const PLACES: [Vec2; 3] = [
    Vec2::new(-724.0, -543.0),
    Vec2::new(4800.0, 2600.0),
    Vec2::new(-9100.0, 5300.0),
];

fn main() {
    let options = Options::parse();

    let Some((target, target_width, target_height)) = read_png(&options.reference) else {
        eprintln!(
            "could not read the reference plate at {}",
            options.reference
        );
        std::process::exit(1);
    };
    let target = metrics::describe(&target, target_width, target_height);

    println!("scoring {} seeds at {WIDTH}x{HEIGHT}", SEEDS.len());

    // Timed on its own, before the parallel sweep and on one thread. Dividing
    // the sweep's wall clock by ten plates would measure throughput on a fully
    // loaded machine and call it latency, and latency is the number that decides
    // whether a page can be ready before the camera reaches it.
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let probe = BakeParams {
        seed: SEEDS[0],
        ..default()
    };
    let _ = bake(Page::new(Vec2::ZERO, PAGE_PIXELS, PAGE_PIXELS), &probe);
    let per_page = started.elapsed().as_secs_f64();

    // Three plates per seed, from three widely separated corners of the world.
    //
    // One plate per seed measures the generator and the *place* together, and
    // cannot tell them apart. Measured here, two regions of a single world
    // differ in mean luminance by as much as two worlds do — which is the
    // regional field doing exactly its job, and is indistinguishable from a
    // seed-dependent generator if you only ever look at one patch of each. It
    // matters because the two call for opposite repairs: regional spread should
    // be left alone, and a seed-dependent generator should be fixed.
    let scored: Vec<(u64, Descriptors)> = SEEDS
        .par_iter()
        .flat_map(|seed| {
            let params = BakeParams {
                seed: *seed,
                ..default()
            };
            PLACES
                .par_iter()
                .map(move |place| {
                    let plate = bake(Page::new(*place, WIDTH, HEIGHT), &params);
                    (*seed, metrics::describe(&plate, WIDTH, HEIGHT))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let plates = scored.len().max(1) as f64;

    println!(
        "\n{:>18}  {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "seed", "distance", "luma", "detail.r4", "detail.r32", "struct.r16", "bright"
    );
    for (index, (seed, candidate)) in scored.iter().enumerate() {
        if index % PLACES.len() != 0 {
            continue;
        }
        println!(
            "{seed:#018x}  {:>9.4} {:>9.4} {:>9.4} {:>9.4} {:>9.4} {:>9.4}",
            metrics::distance(candidate, &target),
            candidate.luma_mean,
            candidate.detail[1],
            candidate.detail[4],
            candidate.structure[3],
            candidate.bright,
        );
    }

    let mean = |pick: &dyn Fn(&Descriptors) -> f32| {
        scored.iter().map(|(_, d)| pick(d) as f64).sum::<f64>() / plates
    };
    let spread = |pick: &dyn Fn(&Descriptors) -> f32| {
        let m = mean(pick);
        (scored
            .iter()
            .map(|(_, d)| (pick(d) as f64 - m).powi(2))
            .sum::<f64>()
            / plates)
            .sqrt()
    };

    let distance = scored
        .iter()
        .map(|(_, d)| metrics::distance(d, &target) as f64)
        .sum::<f64>()
        / plates;
    let worst = scored
        .iter()
        .map(|(_, d)| metrics::distance(d, &target) as f64)
        .fold(0.0f64, f64::max);

    // Variety across seeds, the same way `score-rocks` reports it: how much the
    // plates differ from each other, normalised by how bright they are. A field
    // whose seed stops mattering scores near zero here and perfectly everywhere
    // else.
    let variety = spread(&|d: &Descriptors| d.luma_mean)
        / mean(&|d: &Descriptors| d.luma_mean).max(1.0e-6)
        + spread(&|d: &Descriptors| d.detail[1]) / mean(&|d: &Descriptors| d.detail[1]).max(1.0e-6);

    println!(
        "\nmean distance {distance:.4}   worst seed {worst:.4}   variety across seeds {variety:.4}"
    );
    println!("grass.page_bake {:.1} ms per 256px page", per_page * 1000.0);

    let mut report = Report::new();
    report
        .push(Measurement::new(
            "grass.page_bake",
            "seeds",
            per_page * 1.0e9,
            Unit::Nanoseconds,
            false,
        ))
        .push(Measurement::new(
            "grass.match.distance",
            "seeds",
            distance,
            Unit::Ratio,
            false,
        ))
        .push(Measurement::new(
            "grass.match.worst_seed",
            "seeds",
            worst,
            Unit::Ratio,
            false,
        ))
        .push(Measurement::new(
            "grass.variety.across_seeds",
            "seeds",
            variety,
            Unit::Ratio,
            true,
        ))
        .push(Measurement::new(
            "grass.tone.luma_mean",
            "seeds",
            mean(&|d: &Descriptors| d.luma_mean),
            Unit::Ratio,
            true,
        ))
        .push(Measurement::new(
            "grass.tone.saturation",
            "seeds",
            mean(&|d: &Descriptors| d.saturation),
            Unit::Ratio,
            true,
        ))
        .push(Measurement::new(
            "grass.canopy.bright_share",
            "seeds",
            mean(&|d: &Descriptors| d.bright),
            Unit::Ratio,
            true,
        ))
        .push(Measurement::new(
            "grass.ground.soil_share",
            "seeds",
            mean(&|d: &Descriptors| d.soil),
            Unit::Ratio,
            true,
        ))
        .push(Measurement::new(
            "grass.canopy.busyness",
            "seeds",
            mean(&|d: &Descriptors| d.busyness),
            Unit::Ratio,
            true,
        ));

    for (slot, radius) in metrics::RADII.iter().enumerate() {
        report
            .push(Measurement::new(
                format!("grass.detail.r{radius}"),
                "seeds",
                mean(&|d: &Descriptors| d.detail[slot]),
                Unit::Ratio,
                true,
            ))
            .push(Measurement::new(
                format!("grass.structure.r{radius}"),
                "seeds",
                mean(&|d: &Descriptors| d.structure[slot]),
                Unit::Ratio,
                true,
            ));
    }

    match report.save(std::path::Path::new(&options.out)) {
        Ok(()) => println!("wrote {}", options.out),
        Err(error) => eprintln!("could not write {}: {error}", options.out),
    }
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
    reference: String,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            out: "benchmarks/grass.ron".to_string(),
            reference: "benchmarks/reference/grass_target.png".to_string(),
        };
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let mut index = 0;
        while index < arguments.len() {
            let value = arguments.get(index + 1).cloned().unwrap_or_default();
            match arguments[index].as_str() {
                "--out" => {
                    options.out = value;
                    index += 1;
                }
                "--reference" => {
                    options.reference = value;
                    index += 1;
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
            index += 1;
        }
        options
    }
}

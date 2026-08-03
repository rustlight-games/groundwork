//! Photograph the grass, and say how far it has moved since last time.
//!
//! ```sh
//! # Before optimising: take the pictures and keep them.
//! cargo run --release -p bw_grass --example grass_snapshot -- --accept
//!
//! # After: same command without --accept. It prints what changed.
//! cargo run --release -p bw_grass --example grass_snapshot
//!
//! # Record a new committed performance baseline, deliberately.
//! cargo run --release -p bw_grass --example grass_snapshot -- --accept-perf
//! ```
//!
//! ## What this replaced, and why
//!
//! The grass used to be scored against a piece of reference art: a bag of
//! descriptors — tone, saturation, a detail ladder, a structure ladder —
//! computed on both images and compared, because a generated plate and a painted
//! one share no placement and cannot be compared pixel for pixel. That was the
//! right measurement while the question was *"does this look like the art"*.
//!
//! It is the wrong measurement now, because that question is answered. The look
//! is where it should be, and it got there by spending a great deal of time per
//! pixel. What matters from here is the opposite exchange: **how much speed can
//! be bought, and what does the picture pay for it.** Descriptors cannot answer
//! that. They are lossy by construction — a plate can lose a fifth of its stroke
//! texture and hold every descriptor inside its band — and they were never meant
//! to be a gate.
//!
//! Comparing a bake against *its own previous output* is not lossy. Same seed,
//! same place, same scale, so every pixel has a counterpart and "unchanged"
//! means zero. See [`bw_grass::compare`].
//!
//! ## Zoom levels are the point
//!
//! A page is baked at one fixed scale and displayed at many. At the shipping
//! camera height the ground shows at about 43 percent, and at the widest it is
//! under a quarter — so an optimisation that throws away fine texture is nearly
//! invisible at 48 metres and obvious at 13, while one that coarsens the mound
//! field is exactly the other way round. Photographing a single height would
//! certify half the changes that damage the look.
//!
//! So every place in [`bw_grass::fixtures::PLACES`] is photographed at every
//! height in [`bw_grass::fixtures::ZOOMS`], and the report carries the worst row
//! as well as the mean. The mean is the summary; the worst row is the finding.
//!
//! ## Snapshots are temporary, the performance baseline is not
//!
//! The pictures live under `target/` and are never committed — they are working
//! state for one round of optimisation, they are megabytes each, and a promoted
//! baseline is meaningless to anyone but the machine that took it. The timings
//! go to `benchmarks/grass.ron` and are compared against the committed
//! `benchmarks/baseline/grass.ron`, which is the durable record.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bw_bench::{Measurement, Report, SEEDS, Unit};
use bw_grass::bake::{BakeParams, Page, TILE_PIXELS, bake, bake_grid};
use bw_grass::compare::{self, Similarity, Verdict};
use bw_grass::fixtures::{PLACES, SCREEN, ZOOMS, place_name};
use bw_grass::iso;
use bw_grass::surface::resample;

/// How much a timing may move before it is called a regression.
///
/// Fifteen percent, because a timing on a laptop under thermal load moves ten
/// between runs of identical code. Tighter than that and the suite cries wolf
/// often enough to be ignored, which is worse than not having it.
const TIMING_TOLERANCE: f64 = 0.15;

/// How much a structural count may move: not at all.
///
/// Page counts and pixel counts are arithmetic, not measurements. When one moves
/// it is because something changed on purpose or something is broken.
const EXACT_TOLERANCE: f64 = 0.0;

fn main() {
    let options = Options::parse();
    let current = options.dir.join("current");
    let baseline = options.dir.join("baseline");

    if options.accept {
        return match promote(&current, &baseline) {
            Ok(count) => println!("promoted {count} snapshots to {}", baseline.display()),
            Err(error) => {
                eprintln!("could not promote snapshots: {error}");
                std::process::exit(1);
            }
        };
    }

    if let Err(error) = std::fs::create_dir_all(&current) {
        eprintln!("could not create {}: {error}", current.display());
        std::process::exit(1);
    }

    let params = BakeParams {
        seed: options.seed,
        ..default()
    };

    println!(
        "snapshotting seed {:#x} at {} places x {} zooms, composed for {}x{}",
        options.seed,
        PLACES.len(),
        options.zooms.len(),
        SCREEN.0,
        SCREEN.1,
    );
    println!("  into {}", current.display());

    let latency = page_latency(&params);
    println!(
        "\ngrass.page_bake  {:.1} ms for one {TILE_PIXELS}px page, one thread  \
         ({:.2} µs/px)",
        latency * 1.0e3,
        latency * 1.0e6 / (TILE_PIXELS * TILE_PIXELS) as f64,
    );

    let views = shoot(&options, &params, &current, &baseline);

    println!("\n{}", timing_table(&views));
    let compared: Vec<(String, Similarity)> = views
        .iter()
        .filter_map(|v| v.similarity.map(|s| (v.name.clone(), s)))
        .collect();
    if compared.is_empty() {
        println!(
            "no baseline snapshots in {} — run with --accept to make this set the reference",
            baseline.display()
        );
    } else {
        println!("{}", compare::table(&compared));
        println!("{}", read_the_room(&compared));
    }

    let report = build_report(latency, &views, &compared);
    match report.save(Path::new(&options.out)) {
        Ok(()) => println!("\nwrote {}", options.out),
        Err(error) => eprintln!("\ncould not write {}: {error}", options.out),
    }
    if options.accept_perf {
        // The similarity rows are deliberately left out of the committed
        // baseline. They are measured against a snapshot directory under
        // `target/` that exists only on this machine, so committing them would
        // ask every future run to compare its local pictures against a number
        // taken from somebody else's.
        let durable = Report {
            measurements: report
                .measurements
                .iter()
                .filter(|m| !m.name.starts_with("grass.similarity"))
                .cloned()
                .collect(),
        };
        match durable.save(Path::new(&options.perf_baseline)) {
            Ok(()) => println!(
                "accepted {} of {} measurements as the baseline in {}",
                durable.len(),
                report.len(),
                options.perf_baseline
            ),
            Err(error) => eprintln!("could not write {}: {error}", options.perf_baseline),
        }
        return;
    }
    report_against_baseline(&report, &options.perf_baseline);
}

/// One page, one thread — the number that decides whether streaming keeps up.
///
/// Timed on its own and before anything parallel starts. Dividing a parallel
/// sweep's wall clock by the pages it baked would measure throughput on a fully
/// loaded machine and print it where latency belongs; the two differ by the core
/// count, and only this one says whether a page is ready before the camera
/// reaches it.
fn page_latency(params: &BakeParams) -> f64 {
    // Three runs, best of. A single run catches whatever the scheduler was doing
    // at that moment, and the minimum is the least noisy estimator of a cost
    // that noise can only ever add to.
    #[allow(clippy::disallowed_types)]
    let mut best = f64::INFINITY;
    for place in &PLACES {
        #[allow(clippy::disallowed_types)]
        let started = std::time::Instant::now();
        let page = bake(Page::new(*place, TILE_PIXELS, TILE_PIXELS), params);
        let elapsed = started.elapsed().as_secs_f64();
        std::hint::black_box(&page);
        best = best.min(elapsed);
    }
    best
}

/// One photographed view.
struct View {
    name: String,
    metres: f32,
    /// Cache pixels baked to fill the screen.
    baked: usize,
    /// Pages the streaming renderer would hold to show it — the draw calls.
    pages: usize,
    /// Wall clock of the parallel bake, in seconds.
    seconds: f64,
    similarity: Option<Similarity>,
}

/// Bake, write and compare every place at every zoom.
fn shoot(options: &Options, params: &BakeParams, current: &Path, baseline: &Path) -> Vec<View> {
    let mut views = Vec::new();
    for metres in &options.zooms {
        let (width, height, scale) = iso::view_pixels(*metres, SCREEN);
        for (index, place) in PLACES.iter().enumerate() {
            let name = format!("{}_{metres:.0}m", place_name(index));

            #[allow(clippy::disallowed_types)]
            let started = std::time::Instant::now();
            let plate = bake_grid(Page::new(*place, width, height), params);
            let seconds = started.elapsed().as_secs_f64();

            let shown = resample(&plate, width, height, SCREEN.0, SCREEN.1);
            drop(plate);

            let file = current.join(format!("{name}.png"));
            if let Err(error) = write_png(&file, &shown, SCREEN.0, SCREEN.1) {
                eprintln!("could not write {}: {error}", file.display());
            }

            // Against the previous accepted picture, if there is one. A view
            // whose baseline is a different size is skipped rather than
            // resampled to fit: comparing two scales would report the resampler
            // as a change in the grass.
            let similarity = read_png(&baseline.join(format!("{name}.png")))
                .filter(|(_, w, h)| *w == SCREEN.0 && *h == SCREEN.1)
                .map(|(before, w, h)| compare::compare(&shown, &before, w, h));

            println!(
                "  {name:<12} {width}x{height} cache px at {:.0}%  {seconds:>6.2} s{}",
                scale * 100.0,
                similarity
                    .map(|s| format!("  {}", s.verdict()))
                    .unwrap_or_default(),
            );

            views.push(View {
                name,
                metres: *metres,
                baked: width * height,
                pages: width.div_ceil(TILE_PIXELS) * height.div_ceil(TILE_PIXELS),
                seconds,
                similarity,
            });
        }
    }
    views
}

/// What the views cost, grouped by the number that drives them.
fn timing_table(views: &[View]) -> String {
    let mut out = format!(
        "{:<10} {:>10} {:>8} {:>10} {:>10}\n",
        "view", "cache Mpx", "pages", "bake s", "µs/px"
    );
    for metres in ZOOMS
        .iter()
        .filter(|m| views.iter().any(|v| v.metres == **m))
    {
        let rows: Vec<&View> = views.iter().filter(|v| v.metres == *metres).collect();
        if rows.is_empty() {
            continue;
        }
        let seconds: f64 = rows.iter().map(|v| v.seconds).sum::<f64>() / rows.len() as f64;
        let baked = rows[0].baked;
        out.push_str(&format!(
            "{:<10} {:>10.1} {:>8} {:>10.2} {:>10.3}\n",
            format!("{metres:.0} m"),
            baked as f64 / 1.0e6,
            rows[0].pages,
            seconds,
            seconds * 1.0e6 / baked as f64,
        ));
    }
    out.push_str(
        "\nbake s is wall clock on every core — a throughput figure, and the one that says \
         how long a\ncold view takes to fill. pages is the draw calls a 1080p view costs today.\n",
    );
    out
}

/// The one-line reading of a set of comparisons.
fn read_the_room(compared: &[(String, Similarity)]) -> String {
    let worst = compared
        .iter()
        .min_by(|a, b| a.1.ssim.total_cmp(&b.1.ssim))
        .expect("compared is not empty");
    let verdict = compared
        .iter()
        .map(|(_, s)| s.verdict())
        .max()
        .expect("compared is not empty");
    match verdict {
        Verdict::Identical => "every view is byte for byte what it was. Nothing moved.".to_string(),
        Verdict::Imperceptible => format!(
            "the picture is intact: {} is the worst at ssim {:.5}, which is rounding.",
            worst.0, worst.1.ssim
        ),
        Verdict::Close => format!(
            "visibly the same field. {} moved most (ssim {:.5}, detail {:.3}x) — worth a look \
             before accepting.",
            worst.0, worst.1.ssim, worst.1.detail_ratio
        ),
        Verdict::Drifted | Verdict::Changed => format!(
            "the look has changed. {} is at ssim {:.5} with detail {:.3}x — open it beside its \
             baseline before believing any speed number here.",
            worst.0, worst.1.ssim, worst.1.detail_ratio
        ),
    }
}

/// Everything worth keeping, in the format the baseline comparison reads.
fn build_report(latency: f64, views: &[View], compared: &[(String, Similarity)]) -> Report {
    let mut report = Report::new();
    report
        .push(Measurement::new(
            "grass.page_bake",
            "one_thread",
            latency * 1.0e9,
            Unit::Nanoseconds,
            false,
        ))
        .push(Measurement::new(
            "grass.page_bake.per_pixel",
            "one_thread",
            latency * 1.0e9 / (TILE_PIXELS * TILE_PIXELS) as f64,
            Unit::Nanoseconds,
            false,
        ));

    for metres in ZOOMS {
        let rows: Vec<&View> = views.iter().filter(|v| v.metres == metres).collect();
        if rows.is_empty() {
            continue;
        }
        let scenario = format!("{metres:.0}m");
        let seconds: f64 = rows.iter().map(|v| v.seconds).sum::<f64>() / rows.len() as f64;
        report
            .push(Measurement::new(
                "grass.view_fill",
                &scenario,
                seconds * 1.0e9,
                Unit::Nanoseconds,
                false,
            ))
            // Structural, and the reason the draw-call problem has a number at
            // all: this is how many textures a 1080p view holds at this height.
            .push(Measurement::new(
                "grass.view_pages",
                &scenario,
                rows[0].pages as f64,
                Unit::Count,
                false,
            ))
            .push(Measurement::new(
                "grass.view_pixels",
                &scenario,
                rows[0].baked as f64,
                Unit::Count,
                false,
            ));
    }

    if !compared.is_empty() {
        let count = compared.len() as f64;
        let mean = compared.iter().map(|(_, s)| s.ssim as f64).sum::<f64>() / count;
        let worst = compared
            .iter()
            .map(|(_, s)| s.ssim as f64)
            .fold(f64::INFINITY, f64::min);
        let detail = compared
            .iter()
            .map(|(_, s)| s.detail_ratio as f64)
            .sum::<f64>()
            / count;
        report
            .push(Measurement::new(
                "grass.similarity.ssim",
                "vs_snapshot",
                mean,
                Unit::Ratio,
                true,
            ))
            .push(Measurement::new(
                "grass.similarity.worst_view",
                "vs_snapshot",
                worst,
                Unit::Ratio,
                true,
            ))
            .push(Measurement::new(
                "grass.similarity.detail_ratio",
                "vs_snapshot",
                detail,
                Unit::Ratio,
                true,
            ));
    }
    report
}

/// The before/after table, which is the whole reason the report exists.
fn report_against_baseline(current: &Report, path: &str) {
    let baseline = match Report::load(Path::new(path)) {
        Ok(report) => report,
        Err(error) => {
            println!("\nno baseline to compare against ({error})");
            println!("run with --accept-perf once the numbers are worth keeping");
            return;
        }
    };

    println!(
        "\n{:<28} {:>14} {:>14} {:>10}",
        "", "baseline", "current", "change"
    );
    let mut new = 0usize;
    for measurement in &current.measurements {
        let Some(previous) = baseline.get(&measurement.name, &measurement.scenario) else {
            new += 1;
            continue;
        };
        let delta = if previous.value.abs() > f64::EPSILON {
            (measurement.value - previous.value) / previous.value.abs() * 100.0
        } else {
            f64::NAN
        };
        let (scale, suffix) = match measurement.unit {
            Unit::Nanoseconds => (1.0e6, " ms"),
            _ => (1.0, ""),
        };
        println!(
            "{:<28} {:>14.3}{suffix} {:>14.3}{suffix} {delta:>+9.1}%",
            format!("{}[{}]", measurement.name, measurement.scenario),
            previous.value / scale,
            measurement.value / scale,
        );
    }

    // Two tolerances, because the families are not noisy in the same way, and
    // one number that covers both would either miss real timing regressions or
    // report arithmetic as noise.
    let timings = current.regressions_against(&baseline, TIMING_TOLERANCE);
    let exact: Vec<_> = current
        .regressions_against(&baseline, EXACT_TOLERANCE)
        .into_iter()
        .filter(|change| change.name.contains("pixels") || change.name.contains("pages"))
        .collect();

    println!();
    for change in timings.iter().filter(|c| !c.name.contains("view_p")) {
        println!(
            "REGRESSION  {}[{}]  {:.1}% worse",
            change.name,
            change.scenario,
            -change.relative * 100.0
        );
    }
    for change in &exact {
        println!(
            "STRUCTURAL  {}[{}]  {} -> {}",
            change.name, change.scenario, change.baseline, change.current
        );
    }
    if timings.is_empty() && exact.is_empty() {
        println!(
            "no regressions past {:.0}% on timings.",
            TIMING_TOLERANCE * 100.0
        );
    }
    if new > 0 {
        // Said out loud, because a run where most of the suite is new has a "no
        // regressions" line that means much less than it looks like.
        println!("{new} measurements were new and not compared.");
    }
}

/// Make the current set the one future runs are judged against.
fn promote(current: &Path, baseline: &Path) -> std::io::Result<usize> {
    if !current.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "{} does not exist — take a snapshot first",
                current.display()
            ),
        ));
    }
    // Cleared rather than overwritten. Leaving stale files behind means a view
    // that was renamed or dropped keeps being compared against a picture of
    // something else.
    if baseline.exists() {
        std::fs::remove_dir_all(baseline)?;
    }
    std::fs::create_dir_all(baseline)?;

    let mut count = 0;
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "png") {
            std::fs::copy(entry.path(), baseline.join(entry.file_name()))?;
            count += 1;
        }
    }
    Ok(count)
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

fn read_png(path: &Path) -> Option<(Vec<Vec3>, usize, usize)> {
    let image = image::open(path).ok()?.to_rgb8();
    let (width, height) = (image.width() as usize, image.height() as usize);
    let pixels = image
        .pixels()
        .map(|p| Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0)
        .collect();
    Some((pixels, width, height))
}

struct Options {
    /// Where the snapshots live. Under `target/` by default, and deliberately:
    /// they are working state for one round of optimisation, not a record.
    dir: PathBuf,
    out: String,
    perf_baseline: String,
    seed: u64,
    zooms: Vec<f32>,
    accept: bool,
    accept_perf: bool,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            dir: PathBuf::from("target/grass-snapshots"),
            out: "benchmarks/grass.ron".to_string(),
            perf_baseline: "benchmarks/baseline/grass.ron".to_string(),
            // One seed, not ten. Ten seeds of four zooms of three places is two
            // hundred megapixels of baking to answer a question that does not
            // need it: this suite asks whether a *code change* moved the
            // picture, and a change that moves one world moves all of them.
            // Coverage across seeds is the criterion suite's `seed_spread`
            // group, where it costs seconds rather than minutes.
            seed: SEEDS[1],
            zooms: ZOOMS.to_vec(),
            accept: false,
            accept_perf: false,
        };
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let mut index = 0;
        while index < arguments.len() {
            let value = arguments.get(index + 1).cloned().unwrap_or_default();
            match arguments[index].as_str() {
                "--dir" => {
                    options.dir = PathBuf::from(value);
                    index += 1;
                }
                "--out" => {
                    options.out = value;
                    index += 1;
                }
                "--seed" => {
                    options.seed = value.parse().unwrap_or(options.seed);
                    index += 1;
                }
                "--zooms" => {
                    let zooms: Vec<f32> = value
                        .split(',')
                        .filter_map(|m| m.trim().parse::<f32>().ok())
                        .filter(|m| *m > 0.0)
                        .collect();
                    if !zooms.is_empty() {
                        options.zooms = zooms;
                    }
                    index += 1;
                }
                "--accept" => options.accept = true,
                "--accept-perf" => options.accept_perf = true,
                other => eprintln!("ignoring unknown argument {other}"),
            }
            index += 1;
        }
        options
    }
}

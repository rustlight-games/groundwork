//! The grass benchmark suite.
//!
//! `cargo bench -p bw_grass`
//!
//! Three kinds of measurement, all in one table because they trade against each
//! other and looking at any one alone is misleading.
//!
//! **Performance** is the obvious half: what a step costs, what a chunk costs,
//! what it all weighs.
//!
//! **Physics** is the half that catches the failures nobody notices. A solver
//! that quietly gains energy, a field that responds differently to a shove from
//! the north than from the east, a blast that comes out egg-shaped because
//! something crept into screen space — none of these break a test that only
//! asks whether the grass moved. They are also exactly the failures that are
//! agonising to diagnose from a screen recording weeks later.
//!
//! **Aesthetics** scores the things that make generated grass look generated:
//! blades clumping instead of spreading, every blade the same height, a palette
//! with no contrast in it. See `docs/BENCHMARKS.md`.
//!
//! Every number carries its direction of improvement, so a baseline comparison
//! does not have to guess which half of the table wants to go up.

// Timing is the entire point of this file, and `Instant` is the only way to get
// it. The workspace-wide ban exists to keep wall-clock time out of the
// simulation, where it would break reproducibility; a benchmark is the one
// place it belongs.
#![allow(clippy::disallowed_types)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy::math::{IVec2, Vec2};
use bw_bench::{Measurement, Report, Scenario, Unit, blue_noise_score, silhouette_variety};
use bw_core::{Real, Vec2Fx};
use bw_grass::blade;
use bw_grass::disturbance::{GrassInteractor, Shockwave, stamp_interactor, stamp_shockwave};
use bw_grass::field::{GrassField, SIM_STEP};
use bw_grass::material::GrassSettings;
use bw_grass::wind::WindField;

/// Field resolutions per scenario. A grass field covers the ground near the
/// camera, so these are areas rather than unit counts.
fn resolution(scenario: Scenario) -> usize {
    match scenario {
        Scenario::Small => 128,
        Scenario::Medium => 256,
        Scenario::Large => 512,
    }
}

fn calm() -> WindField {
    WindField {
        speed: 0.0,
        turbulence: 0.0,
        gust_strength: 0.0,
        ..Default::default()
    }
}

fn uniform_field(resolution: usize) -> GrassField {
    let mut field = GrassField::new(resolution, 0.15, bw_bench::SEEDS[0] as u32);
    field.make_uniform(0.24, 1.0);
    field
}

fn main() {
    let mut report = Report::new();
    performance(&mut report);
    physics(&mut report);
    aesthetics(&mut report);

    print_table(&report);
    compare_to_baseline(&report);

    let path = workspace_path("benchmarks/grass.ron");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match report.save(&path) {
        Ok(()) => println!("\nwrote {}", path.display()),
        Err(error) => eprintln!("\ncould not write {}: {error}", path.display()),
    }
}

/// Resolve a path against the workspace root.
///
/// `cargo bench` runs with the *package* directory as the working directory, so
/// a bare relative path would scatter reports under `crates/bw_grass/` instead
/// of the one place `docs/BENCHMARKS.md` says baselines live.
fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

// --- performance ------------------------------------------------------------

fn performance(report: &mut Report) {
    for scenario in Scenario::ALL {
        let cells = resolution(scenario);
        let mut field = uniform_field(cells);
        let wind = WindField::default();

        // Under load rather than at rest: a field with nothing happening in it
        // takes the cheap path through every branch, which is not the number
        // that matters.
        let mut body = GrassInteractor::default();
        body.move_to(Vec2::ZERO);

        // Warm up, so the first-touch page faults land outside the measurement.
        for _ in 0..8 {
            stamp_interactor(&mut field, &body, SIM_STEP);
            field.step(SIM_STEP, &wind);
        }

        let steps = 60;
        let start = Instant::now();
        for step in 0..steps {
            let along = step as f32 * 0.05;
            body.move_to(Vec2::new(-1.5 + along, 0.0));
            stamp_interactor(&mut field, &body, SIM_STEP);
            field.step(SIM_STEP, &wind);
        }
        let per_step = start.elapsed().as_secs_f64() / steps as f64;

        report.push(Measurement::new(
            "grass.field.step",
            scenario.name(),
            per_step * 1.0e9,
            Unit::Nanoseconds,
            false,
        ));
        report.push(Measurement::new(
            "grass.field.cells_per_second",
            scenario.name(),
            (cells * cells) as f64 / per_step,
            Unit::Count,
            true,
        ));
        // The number that actually decides whether this ships: a step has to
        // disappear inside a frame next to everything else the game does.
        report.push(Measurement::new(
            "grass.field.frame_share_at_60hz",
            scenario.name(),
            per_step / (1.0 / 60.0),
            Unit::Ratio,
            false,
        ));
        report.push(Measurement::new(
            "grass.field.state_bytes",
            scenario.name(),
            field.byte_size() as f64,
            Unit::Bytes,
            false,
        ));
        report.push(Measurement::new(
            "grass.field.upload_bytes_per_frame",
            scenario.name(),
            field.upload_bytes() as f64,
            Unit::Bytes,
            false,
        ));
    }

    // Chunk building is a one-off cost per chunk, but it happens while the
    // player is looking at the game, so a slow one is a visible hitch.
    let field = uniform_field(256);
    let chunks = 16;
    let start = Instant::now();
    let mut blades = 0u32;
    let mut bytes = 0usize;
    for index in 0..chunks {
        let batch = blade::build_chunk(&field, IVec2::new(index % 4, index / 4), 1.0, 7);
        blades += batch.blades();
        bytes += batch.byte_size();
    }
    let per_chunk = start.elapsed().as_secs_f64() / chunks as f64;

    report.push(Measurement::new(
        "grass.chunk.build",
        "medium",
        per_chunk * 1.0e9,
        Unit::Nanoseconds,
        false,
    ));
    report.push(Measurement::new(
        "grass.chunk.blades",
        "medium",
        blades as f64 / chunks as f64,
        Unit::Count,
        true,
    ));
    report.push(Measurement::new(
        "grass.chunk.bytes",
        "medium",
        bytes as f64 / chunks as f64,
        Unit::Bytes,
        false,
    ));
    report.push(Measurement::new(
        "grass.chunk.bytes_per_blade",
        "medium",
        bytes as f64 / blades.max(1) as f64,
        Unit::Bytes,
        false,
    ));
}

// --- physics ----------------------------------------------------------------

fn physics(report: &mut Report) {
    let mut push = |name: &str, value: f64, unit: Unit, higher: bool| {
        report.push(Measurement::new(name, "medium", value, unit, higher));
    };

    push(
        "grass.physics.timestep_invariance",
        timestep_invariance(),
        Unit::Ratio,
        true,
    );
    push(
        "grass.physics.energy_monotonicity",
        energy_monotonicity(),
        Unit::Ratio,
        true,
    );
    push(
        "grass.physics.direction_isotropy",
        direction_isotropy(),
        Unit::Ratio,
        true,
    );
    push(
        "grass.physics.blast_isotropy",
        blast_isotropy(),
        Unit::Ratio,
        true,
    );
    push(
        "grass.physics.recovery_half_life",
        recovery_half_life(),
        Unit::Ratio,
        false,
    );
    push(
        "grass.physics.root_pinning",
        root_pinning(),
        Unit::Ratio,
        false,
    );
    push(
        "grass.physics.coupling_locality",
        coupling_locality(),
        Unit::Ratio,
        true,
    );

    let (reinforce, cancel) = nematic_behaviour();
    push(
        "grass.physics.axis_reinforcement",
        reinforce,
        Unit::Ratio,
        true,
    );
    push(
        "grass.physics.polar_cancellation",
        cancel,
        Unit::Ratio,
        true,
    );

    push(
        "grass.wind.divergence",
        wind_divergence(),
        Unit::Ratio,
        false,
    );
    let (spread, contrast) = wind_variation();
    push("grass.wind.direction_spread", spread, Unit::Ratio, true);
    push("grass.wind.gust_contrast", contrast, Unit::Ratio, true);
}

/// Agreement between the peak response at 30, 60 and 120 Hz. One is identical.
///
/// Guards the choice of an implicit solver. An explicit one at these contact
/// stiffnesses does not merely disagree across timesteps — it diverges at the
/// larger ones.
fn timestep_invariance() -> f64 {
    let peaks: Vec<f32> = [1.0 / 30.0, 1.0 / 60.0, 1.0 / 120.0]
        .iter()
        .map(|&dt| {
            let mut field = uniform_field(96);
            let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
            field.add_impulse(x, y, Vec2::X * 10.0);
            let mut peak: f32 = 0.0;
            for _ in 0..(1.0 / dt) as u32 {
                field.step(dt, &calm());
                peak = peak.max(field.max_bend());
            }
            peak
        })
        .collect();

    let min = peaks.iter().cloned().fold(f32::MAX, f32::min);
    let max = peaks.iter().cloned().fold(0.0, f32::max);
    if max <= 0.0 { 0.0 } else { (min / max) as f64 }
}

/// Fraction of unforced steps in which total energy did not rise.
///
/// One means the integrator never manufactured energy. Anything meaningfully
/// below it is grass that will eventually start vibrating on its own.
fn energy_monotonicity() -> f64 {
    let mut field = uniform_field(96);
    let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
    field.add_impulse(x, y, Vec2::new(9.0, -4.0));
    field.step(SIM_STEP, &calm());

    let mut previous = field.energy();
    let mut good = 0;
    let total = 400;
    for _ in 0..total {
        field.step(SIM_STEP, &calm());
        let now = field.energy();
        if now <= previous * 1.002 + 1e-9 {
            good += 1;
        }
        previous = now;
    }
    good as f64 / total as f64
}

/// Agreement between identical shoves aimed north, south, east and west.
///
/// The measurement that would catch anyone simulating in screen space. One
/// means the world has no preferred direction; the isometric projection is
/// applied only when drawing.
fn direction_isotropy() -> f64 {
    let peaks: Vec<f32> = [Vec2::X, Vec2::Y, -Vec2::X, -Vec2::Y]
        .iter()
        .map(|&direction| {
            let mut field = uniform_field(96);
            let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
            field.add_impulse(x, y, direction * 10.0);
            let mut peak: f32 = 0.0;
            for _ in 0..30 {
                field.step(SIM_STEP, &calm());
                peak = peak.max(field.max_bend());
            }
            peak
        })
        .collect();

    let min = peaks.iter().cloned().fold(f32::MAX, f32::min);
    let max = peaks.iter().cloned().fold(0.0, f32::max);
    if max <= 0.0 { 0.0 } else { (min / max) as f64 }
}

/// Roundness of a blast, sampled at eight bearings. One is a perfect circle.
fn blast_isotropy() -> f64 {
    let mut field = uniform_field(160);
    let mut wave = Shockwave {
        width: 0.4,
        ..Default::default()
    };
    let mut peaks = [0.0f32; 8];
    for _ in 0..40 {
        stamp_shockwave(&mut field, &wave);
        field.step(SIM_STEP, &calm());
        wave.age += SIM_STEP;
        for (index, peak) in peaks.iter_mut().enumerate() {
            let angle = index as f32 / 8.0 * std::f32::consts::TAU;
            let probe = Vec2::new(angle.cos(), angle.sin()) * 1.5;
            *peak = peak.max(field.bend_at(probe).length());
        }
    }

    let min = peaks.iter().cloned().fold(f32::MAX, f32::min);
    let max = peaks.iter().cloned().fold(0.0, f32::max);
    if max <= 0.0 { 0.0 } else { (min / max) as f64 }
}

/// Seconds for a shoved patch to fall to half its peak bend.
///
/// The number that decides whether grass reads as grass. Too short and it snaps
/// back like a spring toy; too long and it behaves like cloth.
fn recovery_half_life() -> f64 {
    let mut field = uniform_field(96);
    let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
    field.add_impulse(x, y, Vec2::X * 12.0);

    let mut peak = 0.0f32;
    let mut peak_step = 0;
    let mut history = Vec::new();
    for step in 0..600 {
        field.step(SIM_STEP, &calm());
        let bend = field.bend_at(Vec2::ZERO).length();
        history.push(bend);
        if bend > peak {
            peak = bend;
            peak_step = step;
        }
    }
    if peak <= 0.0 {
        return 0.0;
    }
    for (step, bend) in history.iter().enumerate().skip(peak_step) {
        if *bend <= peak * 0.5 {
            return ((step - peak_step) as f32 * SIM_STEP) as f64;
        }
    }
    (history.len() as f32 * SIM_STEP) as f64
}

/// How far a blade's root moves, as a fraction of its length. Zero is correct.
///
/// Every deformation is weighted to zero at the base, so this is exact rather
/// than merely small — but it is worth measuring, because a root that drifts
/// even slightly makes the whole field look like a texture being dragged over
/// the ground, and the cause is not obvious when you see it.
fn root_pinning() -> f64 {
    let settings = GrassSettings::default();
    // The shader's bend profile is flat zero below `root_stiffness` and rises
    // from there; the root sits at height zero, so its displacement is the
    // profile evaluated at zero.
    let profile_at_root = if settings.root_stiffness > 0.0 {
        0.0
    } else {
        1.0
    };
    profile_at_root as f64
}

/// How much more a kick moves the cell next door than a cell ten away.
///
/// High is good. Weak coupling is what keeps the field from behaving like a
/// rubber sheet, and this is the number that says whether it stayed weak.
fn coupling_locality() -> f64 {
    let mut field = uniform_field(128);
    let (x, y) = field.cell_at(Vec2::ZERO).unwrap();
    field.add_impulse(x, y, Vec2::X * 14.0);

    let (mut near, mut far) = (0.0f32, 0.0f32);
    for _ in 0..90 {
        field.step(SIM_STEP, &calm());
        near = near.max(field.bend_at(Vec2::new(0.15, 0.0)).length());
        far = far.max(field.bend_at(Vec2::new(1.5, 0.0)).length());
    }
    if near <= 0.0 {
        return 0.0;
    }
    (1.0 - (far / near)).clamp(0.0, 1.0) as f64
}

/// The controlled experiment that justifies storing an unsigned axis alongside
/// a direction.
///
/// Two traversals of the same path, carrying identical contact: once as a
/// convoy going the same way twice, and once as one pass each way. The two
/// leave genuinely different grass, and a single displacement value cannot tell
/// them apart — walking back cancels the average direction exactly, which would
/// claim the path is undisturbed while anyone looking at it can see a flattened
/// track.
///
/// - `axis_reinforcement` is how much of the flattening axis survives the
///   reversal. Near one means the axis is unchanged, which is correct: the
///   grass is lying along the path either way.
/// - `polar_cancellation` is how much of the *signed* direction the reversal
///   removes. Near one means the two passes cancelled, which is also correct.
///
/// Both being high at once is the whole point. Either alone is easy.
fn nematic_behaviour() -> (f64, f64) {
    let sweep = |field: &mut GrassField, from: Vec2, to: Vec2| {
        // Slow and heavy, so a pass leaves a mark worth measuring rather than
        // a trace that has faded before the second pass arrives.
        let steps = 90;
        let mut body = GrassInteractor {
            radius: 0.34,
            falloff: 0.3,
            mass: 260.0,
            previous: from,
            current: from,
        };
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            body.move_to(from.lerp(to, t));
            stamp_interactor(field, &body, SIM_STEP);
            field.step(SIM_STEP, &calm());
        }
    };

    let west = Vec2::new(-1.6, 0.0);
    let east = Vec2::new(1.6, 0.0);

    let mut convoy = uniform_field(96);
    sweep(&mut convoy, west, east);
    sweep(&mut convoy, west, east);

    let mut return_trip = uniform_field(96);
    sweep(&mut return_trip, west, east);
    sweep(&mut return_trip, east, west);

    let axis_convoy = convoy.axis_at(Vec2::ZERO).length();
    let axis_return = return_trip.axis_at(Vec2::ZERO).length();
    let polar_convoy = convoy.slow_memory_at(Vec2::ZERO).length();
    let polar_return = return_trip.slow_memory_at(Vec2::ZERO).length();

    let reinforcement = if axis_convoy > 1e-6 {
        (axis_return / axis_convoy).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cancellation = if polar_convoy > 1e-6 {
        (1.0 - polar_return / polar_convoy).clamp(0.0, 1.0)
    } else {
        0.0
    };

    (reinforcement as f64, cancellation as f64)
}

/// Divergence of the turbulent wind, which curl noise makes zero by
/// construction. Non-zero means grass is being sucked toward fixed points.
fn wind_divergence() -> f64 {
    let wind = WindField {
        speed: 0.0,
        gust_strength: 0.0,
        ..Default::default()
    };
    let h = 0.01;
    let mut worst = 0.0f32;
    for index in 0..64 {
        let probe = Vec2::new(index as f32 * 0.77 - 24.0, index as f32 * -0.41 + 9.0);
        let dx = (wind.velocity_at(probe + Vec2::new(h, 0.0)).x
            - wind.velocity_at(probe - Vec2::new(h, 0.0)).x)
            / (2.0 * h);
        let dy = (wind.velocity_at(probe + Vec2::new(0.0, h)).y
            - wind.velocity_at(probe - Vec2::new(0.0, h)).y)
            / (2.0 * h);
        worst = worst.max((dx + dy).abs());
    }
    worst as f64
}

/// How much the wind varies across a field, in direction and in strength.
///
/// Both numbers exist to catch the same failure from two sides: a field driven
/// by one global wind vector, where every blade leans identically and the whole
/// meadow moves like a rigid sheet.
///
/// - `direction_spread` is the circular spread of lean angles. A real prevailing
///   wind legitimately keeps this smallish — grass genuinely does mostly lean
///   downwind — so it is the weaker of the two signals.
/// - `gust_contrast` is the coefficient of variation of lean *magnitude*, and is
///   the one that matters. It is what makes gust fronts read as waves crossing
///   the field rather than as an even shimmer. Exactly zero means one sheet.
fn wind_variation() -> (f64, f64) {
    let mut field = uniform_field(128);
    let wind = WindField::default();
    for _ in 0..240 {
        field.step(SIM_STEP, &wind);
    }

    let bends: Vec<Vec2> = field
        .theta()
        .iter()
        .copied()
        .filter(|t| t.length() > 0.02)
        .collect();
    if bends.len() < 2 {
        return (0.0, 0.0);
    }

    // Circular spread: one minus the length of the mean unit vector. An
    // ordinary standard deviation would get the wrap-around at pi badly wrong.
    let mean_direction: Vec2 =
        bends.iter().map(|t| t.normalize_or_zero()).sum::<Vec2>() / bends.len() as f32;
    let spread = (1.0 - mean_direction.length()).clamp(0.0, 1.0);

    let magnitudes: Vec<f32> = bends.iter().map(|t| t.length()).collect();
    let mean = magnitudes.iter().sum::<f32>() / magnitudes.len() as f32;
    let variance = magnitudes
        .iter()
        .map(|m| (m - mean) * (m - mean))
        .sum::<f32>()
        / magnitudes.len() as f32;
    let contrast = if mean > 1e-6 {
        (variance.sqrt() / mean).clamp(0.0, 1.0)
    } else {
        0.0
    };

    (spread as f64, contrast as f64)
}

// --- aesthetics -------------------------------------------------------------

fn aesthetics(report: &mut Report) {
    let field = GrassField::new(256, 0.15, bw_bench::SEEDS[0] as u32);

    // Averaged over the standard seeds: one seed can flatter or punish a
    // layout by luck.
    let mut spread = 0.0;
    let mut variety = 0.0;
    for (index, seed) in bw_bench::SEEDS.iter().enumerate() {
        let batch = blade::build_chunk(&field, IVec2::new(index as i32, 0), 0.08, *seed as u32);
        let points: Vec<Vec2Fx> = batch
            .roots()
            .map(|p| Vec2Fx::new(Real::from_num(p.x), Real::from_num(p.y)))
            .collect();
        spread += blue_noise_score(&points);

        let lengths: Vec<f64> = batch.lengths().map(|l| l as f64).collect();
        variety += silhouette_variety(&lengths);
    }
    let seeds = bw_bench::SEEDS.len() as f64;

    report.push(Measurement::new(
        "grass.blade.placement_spread",
        "seeds",
        spread / seeds,
        Unit::Ratio,
        true,
    ));
    report.push(Measurement::new(
        "grass.blade.length_variety",
        "seeds",
        variety / seeds,
        Unit::Ratio,
        true,
    ));

    // Contrast between the darkest and lightest part of a blade. Too little and
    // the canopy reads as one flat green no matter how much geometry is in it.
    let settings = GrassSettings::default();
    let to_bytes = |c: bevy::math::Vec4| {
        [
            (c.x.clamp(0.0, 1.0) * 255.0) as u8,
            (c.y.clamp(0.0, 1.0) * 255.0) as u8,
            (c.z.clamp(0.0, 1.0) * 255.0) as u8,
        ]
    };
    report.push(Measurement::new(
        "grass.blade.luminance_spread",
        "palette",
        bw_bench::luminance_spread(&[
            to_bytes(settings.base_color),
            to_bytes(settings.tip_color),
            to_bytes(settings.crushed_color),
        ]),
        Unit::Ratio,
        true,
    ));
}

// --- reporting --------------------------------------------------------------

fn print_table(report: &Report) {
    println!(
        "\n{:<42} {:>10} {:>14}  {:<3}",
        "measurement", "scenario", "value", "dir"
    );
    println!("{}", "-".repeat(74));

    let mut group = String::new();
    for measurement in &report.measurements {
        let prefix: String = measurement
            .name
            .split('.')
            .take(2)
            .collect::<Vec<_>>()
            .join(".");
        if prefix != group {
            if !group.is_empty() {
                println!();
            }
            group = prefix;
        }
        println!(
            "{:<42} {:>10} {:>14}  {:<3}",
            measurement.name,
            measurement.scenario,
            format_value(measurement.value, measurement.unit),
            if measurement.higher_is_better {
                "up"
            } else {
                "down"
            },
        );
    }
}

fn format_value(value: f64, unit: Unit) -> String {
    match unit {
        Unit::Nanoseconds if value >= 1_000_000.0 => format!("{:.3} ms", value / 1.0e6),
        Unit::Nanoseconds if value >= 1_000.0 => format!("{:.1} us", value / 1.0e3),
        Unit::Nanoseconds => format!("{value:.0} ns"),
        Unit::Bytes if value >= 1_048_576.0 => format!("{:.2} MiB", value / 1_048_576.0),
        Unit::Bytes if value >= 1024.0 => format!("{:.1} KiB", value / 1024.0),
        Unit::Bytes => format!("{value:.0} B"),
        Unit::Count if value >= 1_000_000.0 => format!("{:.2} M", value / 1.0e6),
        Unit::Count if value >= 1000.0 => format!("{:.1} k", value / 1.0e3),
        Unit::Count => format!("{value:.0}"),
        Unit::Ratio => format!("{value:.4}"),
        Unit::TicksPerSecond => format!("{value:.0} tps"),
    }
}

fn compare_to_baseline(report: &Report) {
    let path = workspace_path("benchmarks/baseline/grass.ron");
    let Ok(baseline) = Report::load(&path) else {
        println!("\nno baseline at {} — nothing to compare", path.display());
        return;
    };
    // Performance is noisy on a laptop under thermal load; aesthetic and
    // physical numbers are averages and move less. One tolerance for all of
    // them would either cry wolf or miss real regressions.
    let regressions = report.regressions_against(&baseline, 0.10);
    if regressions.is_empty() {
        println!("\nno regressions against the baseline");
        return;
    }
    println!("\n{} regressions against the baseline:", regressions.len());
    for change in regressions {
        println!(
            "  {:<42} {:>12.4} -> {:<12.4} ({:+.1}%)",
            format!("{} [{}]", change.name, change.scenario),
            change.baseline,
            change.current,
            change.relative * 100.0,
        );
    }
}

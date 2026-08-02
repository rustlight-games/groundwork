//! What the grass costs.
//!
//! The design goal this section exists to defend: **grass is background.** The
//! ambition is StarCraft's creep — a surface that reacts, spreads and reads as
//! alive while costing a fraction of a frame, because everything else in the
//! game needs the rest of it. So the headline is not milliseconds, it is
//! [`share`](Section::share): what fraction of a 60 Hz frame the field eats.
//!
//! Four things are measured, and skipping any one of them lets a regression
//! hide:
//!
//! **Granularly.** A step is six phases. Timing only the whole thing tells you
//! the grass got slower and nothing else; the phase breakdown says which of the
//! six to open. They are also wildly unequal — the solver is six Jacobi sweeps
//! over the whole grid and everything else is one — so a change that doubles a
//! cheap phase is invisible in the total and a change that adds 5% to the solve
//! is not.
//!
//! **Under pressure.** A field with nothing happening in it takes the cheap
//! path through every branch. `finalise` in particular skips its seven
//! exponentials on any cell that has never been touched, which is almost every
//! cell in a quiet field and almost none in a battle. The at-rest number is
//! therefore the *least* interesting one in this section, and
//! `step.trampled_multiplier` — the ratio between them — is the number that
//! says how bad a battle gets.
//!
//! **At the margin.** `pressure.marginal_unit` is the cost of one more unit
//! walking through grass. It is what tells you whether forty units is fine and
//! two hundred is not, without having to benchmark every count.
//!
//! **Worst case, not average.** Every timing reports its p95 and its jitter as
//! well as its median. Background systems fail by hitching, and a mean hides
//! exactly that.

use std::hint::black_box;

use bevy::math::{IVec2, Vec2};
use bw_bench::{Report, Scenario};
use bw_grass::blade;
use bw_grass::clump;
use bw_grass::disturbance::{GrassInteractor, Shockwave, stamp_interactor, stamp_shockwave};
use bw_grass::field::SIM_STEP;
use bw_grass::scene::GrassScene;
use bw_grass::wind::WindField;

use crate::harness::{self, Section, Timing};

/// A frame at the rate the game runs at.
const FRAME: f64 = 1.0 / 60.0;

pub fn run(report: &mut Report) {
    step_cost(report);
    step_phases(report);
    pressure(report);
    scaling(report);
    building(report);
    bandwidth(report);
}

/// Report a timing three ways.
///
/// Always three, never one. The median is the cost, the p95 is the frame that
/// stutters, and the jitter is whether there is a stutter at all.
fn timing(section: &mut Section, name: &str, timing: &Timing) {
    section.seconds(&format!("{name}.median"), timing.median());
    section.seconds(&format!("{name}.p95"), timing.p95());
    section.ratio(&format!("{name}.jitter"), timing.jitter(), false);
    section.ratio(
        &format!("{name}.frame_share"),
        timing.median() / FRAME,
        false,
    );
}

// --- the step, whole --------------------------------------------------------

fn step_cost(report: &mut Report) {
    for scenario in Scenario::ALL {
        let cells = harness::resolution(scenario);
        let mut section = Section::new(report, scenario.name());

        // Three loads on the same field, because the spread between them is
        // the interesting part.
        let rest = time_step(cells, Load::Rest);
        let walked = time_step(cells, Load::Walked);
        let trampled = time_step(cells, Load::Trampled);

        timing(&mut section, "grass.step.at_rest", &rest);
        timing(&mut section, "grass.step.walked", &walked);
        timing(&mut section, "grass.step.trampled", &trampled);

        // The number that says how much worse a battle is than a still meadow.
        // One would mean the field costs the same whatever is happening on it,
        // which is the ideal for something budgeted as background.
        section.ratio(
            "grass.step.trampled_multiplier",
            if rest.median() > 0.0 {
                trampled.median() / rest.median()
            } else {
                0.0
            },
            false,
        );

        section.count(
            "grass.step.cells_per_second",
            (cells * cells) as f64 / walked.median().max(1e-12),
            true,
        );
        // Ground covered per millisecond of CPU. The number to quote when
        // deciding how large a battlefield can be.
        let metres = (cells as f64 * harness::CELL as f64).powi(2);
        section.count(
            "grass.step.square_metres_per_ms",
            metres / (walked.median() * 1000.0).max(1e-12),
            true,
        );
    }
}

enum Load {
    /// Nothing touching it. The cheap path through every branch.
    Rest,
    /// One body walking, which is the ordinary case.
    Walked,
    /// Every cell contacted, so no cell can take the cheap path.
    Trampled,
}

fn time_step(cells: usize, load: Load) -> Timing {
    let mut field = harness::uniform_field(cells);
    let mut wind = WindField::default();
    let mut body = harness::walker(Vec2::new(-1.5, 0.0));

    harness::sample(8, 40, |run| {
        wind.time += SIM_STEP;
        match load {
            Load::Rest => {}
            Load::Walked => {
                body.move_to(Vec2::new(-1.5 + run as f32 * 0.05, 0.0));
                stamp_interactor(&mut field, &body, SIM_STEP);
            }
            Load::Trampled => {
                // Written straight into every cell rather than by crowding the
                // field with bodies. The point is to price a step in which no
                // cell can take the cheap path, and a plausible-looking army
                // large enough to guarantee that would spend most of the
                // measurement inside the stamp — which is a different number,
                // measured separately.
                let angle = Vec2::new(0.2, 0.1);
                let direction = Vec2::new(1.0, 0.0);
                for y in 0..cells {
                    for x in 0..cells {
                        field.accumulate_contact(x, y, angle, direction, 0.6, 0.4);
                    }
                }
            }
        }
        field.step(SIM_STEP, &wind);
        black_box(field.steps_taken());
    })
}

// --- the step, in pieces ----------------------------------------------------

/// Time the six phases of a step individually.
///
/// Run on a walked field rather than a still one: the phase whose cost depends
/// most on what is happening — `finalise`, which skips its memory machinery on
/// untouched cells — would otherwise be measured in the one condition where it
/// does nothing.
fn step_phases(report: &mut Report) {
    let cells = harness::resolution(Scenario::Medium);
    let mut section = Section::new(report, "medium");

    let mut field = harness::uniform_field(cells);
    let mut wind = WindField::default();
    let mut body = harness::walker(Vec2::new(-1.5, 0.0));
    for step in 0..30 {
        body.move_to(Vec2::new(-1.5 + step as f32 * 0.05, 0.0));
        stamp_interactor(&mut field, &body, SIM_STEP);
        field.step(SIM_STEP, &wind);
    }

    // Each phase is timed inside a complete, correctly ordered step, so the
    // state each one sees is the state it sees in the game. Timing a phase in
    // isolation would run it against a field that never advances, which for
    // `finalise` in particular is a different amount of work.
    let mut totals = [0.0f64; 6];
    let mut stamp_total = 0.0f64;
    let runs = 40;
    for step in 0..runs + 8 {
        wind.time += SIM_STEP;
        body.move_to(Vec2::new(0.0 + step as f32 * 0.05, 0.0));

        let mut slice = [0.0f64; 6];
        let stamp = harness::sample(0, 1, |_| stamp_interactor(&mut field, &body, SIM_STEP));
        slice[0] = harness::sample(0, 1, |_| field.bake_wind(&wind)).median();
        slice[1] = harness::sample(0, 1, |_| field.build_system(SIM_STEP, &wind)).median();
        slice[2] = harness::sample(0, 1, |_| field.build_coupling()).median();
        slice[3] = harness::sample(0, 1, |_| field.solve_jacobi()).median();
        slice[4] = harness::sample(0, 1, |_| field.finalise(SIM_STEP)).median();
        slice[5] = harness::sample(0, 1, |_| field.clear_accumulators()).median();

        if step >= 8 {
            for (total, part) in totals.iter_mut().zip(slice) {
                *total += part;
            }
            stamp_total += stamp.median();
        }
    }

    let names = [
        "bake_wind",
        "build_system",
        "build_coupling",
        "solve_jacobi",
        "finalise",
        "clear_accumulators",
    ];
    let whole: f64 = totals.iter().sum();
    for (name, total) in names.iter().zip(totals) {
        section.seconds(&format!("grass.phase.{name}"), total / runs as f64);
        // The share matters more than the absolute time when reading a
        // regression: a phase that grew from 3% to 4% of a step is noise, and
        // one that grew from 40% to 55% is the whole story.
        section.ratio(
            &format!("grass.phase.{name}_share"),
            if whole > 0.0 { total / whole } else { 0.0 },
            false,
        );
    }
    section.seconds("grass.phase.stamp_interactor", stamp_total / runs as f64);
}

// --- pressure ---------------------------------------------------------------

/// What a battle costs.
///
/// The scenarios are named after what they represent rather than after their
/// parameters, so that when the shape of a battle changes the scenario can be
/// updated without the measurement losing its meaning.
fn pressure(report: &mut Report) {
    let mut section = Section::new(report, "battle");

    // Forty units skirmishing across a medium field with four blasts going off
    // — `bw_bench`'s medium scenario, translated into grass.
    let battle = time_crowd(harness::resolution(Scenario::Medium), 40, 4);
    timing(&mut section, "grass.pressure.battle", &battle);

    section.scenario("siege");
    // Two hundred units on a large field: the large scenario, and roughly the
    // worst case the game is meant to support.
    let siege = time_crowd(harness::resolution(Scenario::Large), 200, 8);
    timing(&mut section, "grass.pressure.siege", &siege);

    section.scenario("marginal");
    // The slope of cost against unit count, fitted rather than differenced, so
    // one noisy point cannot set the answer. This is what says whether the
    // system scales with the battle or with the map.
    let counts = [0usize, 8, 24, 48, 96];
    let points: Vec<(f64, f64)> = counts
        .iter()
        .map(|&count| {
            (
                count as f64,
                time_crowd(harness::resolution(Scenario::Medium), count, 0).median(),
            )
        })
        .collect();
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / points.len() as f64;
    let top: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let bottom: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let slope = if bottom > 0.0 { top / bottom } else { 0.0 };

    section.seconds("grass.pressure.marginal_unit", slope.max(0.0));
    // How many units the field could carry before the grass alone spent a tenth
    // of the frame. A budget, expressed as a headroom.
    let base = points[0].1;
    section.count(
        "grass.pressure.units_within_tenth_frame",
        if slope > 1e-12 {
            ((FRAME * 0.10 - base) / slope).max(0.0)
        } else {
            f64::from(u16::MAX)
        },
        true,
    );
}

fn time_crowd(cells: usize, units: usize, blasts: usize) -> Timing {
    let mut field = harness::uniform_field(cells);
    let mut wind = WindField::default();
    let extent = cells as f32 * harness::CELL * 0.42;

    // Spread over the field on a coprime lattice so they neither pile into one
    // cell nor line up into a row, either of which would measure a special case.
    let mut bodies: Vec<GrassInteractor> = (0..units)
        .map(|index| {
            let angle = index as f32 * 2.399_963_2;
            let radius = extent * ((index as f32 + 0.5) / units.max(1) as f32).sqrt();
            harness::walker(Vec2::new(angle.cos(), angle.sin()) * radius)
        })
        .collect();

    let mut waves: Vec<Shockwave> = (0..blasts)
        .map(|index| Shockwave {
            origin: Vec2::new(
                (index as f32 * 1.7).sin() * extent * 0.6,
                (index as f32 * 2.3).cos() * extent * 0.6,
            ),
            // Staggered, so they are at different radii and the measurement
            // catches a ring mid-flight rather than all of them at birth.
            age: index as f32 * 0.35,
            seed: 0x51A5_5EED ^ index as u32,
            ..Default::default()
        })
        .collect();

    harness::sample(8, 32, |run| {
        wind.time += SIM_STEP;
        let drift = run as f32 * SIM_STEP;
        for (index, body) in bodies.iter_mut().enumerate() {
            let heading = index as f32 * 1.107_148_7;
            let start = body.current;
            body.move_to(start + Vec2::new(heading.cos(), heading.sin()) * 1.4 * SIM_STEP);
            stamp_interactor(&mut field, body, SIM_STEP);
        }
        for wave in &mut waves {
            wave.age += SIM_STEP;
            if wave.finished() {
                wave.age = 0.0;
            }
            stamp_shockwave(&mut field, wave);
        }
        black_box(drift);
        field.step(SIM_STEP, &wind);
    })
}

// --- scaling ----------------------------------------------------------------

fn scaling(report: &mut Report) {
    let mut section = Section::new(report, "medium");
    let cells = harness::resolution(Scenario::Medium);

    // Three of the six phases are threaded by row. Measuring the same step on
    // one thread says how much of the cost that actually removes — and whether
    // a machine with fewer cores than this one still fits the budget, which is
    // not a hypothetical for a game.
    let threads = rayon::current_num_threads();
    let single = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("a one-thread pool");

    let mut field = harness::uniform_field(cells);
    let mut wind = WindField::default();
    let mut body = harness::walker(Vec2::ZERO);

    let parallel = harness::sample(8, 32, |run| {
        wind.time += SIM_STEP;
        body.move_to(Vec2::new(-1.5 + run as f32 * 0.03, 0.0));
        stamp_interactor(&mut field, &body, SIM_STEP);
        field.step(SIM_STEP, &wind);
    });

    let serial = single.install(|| {
        harness::sample(8, 32, |run| {
            wind.time += SIM_STEP;
            body.move_to(Vec2::new(-1.5 + run as f32 * 0.03, 0.0));
            stamp_interactor(&mut field, &body, SIM_STEP);
            field.step(SIM_STEP, &wind);
        })
    });

    section.count("grass.scale.threads", threads as f64, true);
    section.seconds("grass.scale.single_thread_step", serial.median());
    section.ratio(
        "grass.scale.parallel_speedup",
        if parallel.median() > 0.0 {
            serial.median() / parallel.median()
        } else {
            0.0
        },
        true,
    );
    // Speedup over cores. Below about a half means the threading is mostly
    // paying for itself in overhead, and the honest fix is a cheaper step
    // rather than more threads.
    section.ratio(
        "grass.scale.thread_efficiency",
        if parallel.median() > 0.0 && threads > 0 {
            serial.median() / parallel.median() / threads as f64
        } else {
            0.0
        },
        true,
    );

    // Cost against area, across the three resolutions. Perfectly linear in
    // cells is the best a field solver can be; anything superlinear means
    // something is scaling with the wrong dimension.
    let mut costs = Vec::new();
    for scenario in Scenario::ALL {
        let cells = harness::resolution(scenario);
        let mut field = harness::uniform_field(cells);
        let mut wind = WindField::default();
        let timing = harness::sample(4, 16, |_| {
            wind.time += SIM_STEP;
            field.step(SIM_STEP, &wind);
        });
        costs.push(((cells * cells) as f64, timing.median()));
    }
    // Exponent of a power-law fit through the three points.
    let logs: Vec<(f64, f64)> = costs
        .iter()
        .filter(|(_, t)| *t > 0.0)
        .map(|(n, t)| (n.ln(), t.ln()))
        .collect();
    let exponent = if logs.len() >= 2 {
        let mean_x = logs.iter().map(|p| p.0).sum::<f64>() / logs.len() as f64;
        let mean_y = logs.iter().map(|p| p.1).sum::<f64>() / logs.len() as f64;
        let top: f64 = logs.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
        let bottom: f64 = logs.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
        if bottom > 0.0 { top / bottom } else { 0.0 }
    } else {
        0.0
    };
    section.scenario("scaling");
    section.ratio("grass.scale.cost_exponent", exponent, false);
}

// --- building ---------------------------------------------------------------

/// What it costs to bring grass into existence, as opposed to keeping it alive.
///
/// All one-off costs, and all of them happen while the player is looking at the
/// game — the atlas bake on startup, a chunk build whenever the view moves. A
/// slow one is a visible hitch rather than a lower frame rate, which is worse.
fn building(report: &mut Report) {
    let mut section = Section::new(report, "startup");

    let atlas = harness::sample(1, 3, |_| {
        black_box(clump::bake(&clump::Style::default(), 0x6A72_A551));
    });
    section.seconds("grass.build.atlas_bake", atlas.median());
    let baked = clump::bake(&clump::Style::default(), 0x6A72_A551);
    section.bytes(
        "grass.build.atlas_bytes",
        (baked.width * baked.height * 4) as f64,
        false,
    );
    section.ratio("grass.build.atlas_coverage", baked.coverage() as f64, true);

    // The first frame of the shipped scene: every chunk in the default extent,
    // built at full detail, because chunk streaming is not wired up yet. This
    // is the startup hitch, and it is the honest way to price it.
    let scene = GrassScene::default();
    // Sized to cover the scene rather than to match a benchmark scenario.
    // Clumps are gated on the field's density, so a field smaller than the
    // scene silently rejects every clump outside it — which priced the startup
    // build at a third of the chunks it really does.
    let cells = ((scene.half_extent * 2.0 / harness::CELL).ceil() as usize).next_power_of_two();
    let field = harness::uniform_field(cells);
    let radius = (scene.half_extent / blade::CHUNK_METRES).ceil() as i32;

    let mut clumps = 0u64;
    let mut bytes = 0u64;
    let mut chunks = 0u64;
    let whole = harness::sample(0, 1, |_| {
        for y in -radius..radius {
            for x in -radius..radius {
                let batch = clump::build_chunk(&field, IVec2::new(x, y), 1.0, scene.seed);
                if batch.is_empty() {
                    continue;
                }
                clumps += u64::from(batch.clumps());
                bytes += batch.byte_size() as u64;
                chunks += 1;
                black_box(batch.clumps());
            }
        }
    });
    section.seconds("grass.build.scene_first_frame", whole.median());
    section.count("grass.build.scene_chunks", chunks as f64, false);
    section.count("grass.build.scene_clumps", clumps as f64, false);
    section.bytes("grass.build.scene_mesh_bytes", bytes as f64, false);

    section.scenario("chunk");
    let clump_chunk = harness::sample(4, 24, |run| {
        black_box(clump::build_chunk(
            &field,
            IVec2::new(run as i32 % 8, run as i32 / 8),
            1.0,
            scene.seed,
        ));
    });
    section.seconds("grass.build.clump_chunk", clump_chunk.median());
    section.ratio(
        "grass.build.clump_chunk_jitter",
        clump_chunk.jitter(),
        false,
    );

    let sample_batch = clump::build_chunk(&field, IVec2::ZERO, 1.0, scene.seed);
    section.count(
        "grass.build.clumps_per_chunk",
        f64::from(sample_batch.clumps()),
        false,
    );
    section.bytes(
        "grass.build.bytes_per_clump",
        sample_batch.byte_size() as f64 / f64::from(sample_batch.clumps().max(1)),
        false,
    );

    // The ribbon path is not what ships, but it is still in the tree and its
    // simulation-facing tests describe the field the clumps are driven by.
    // Priced so that a decision to revive it is made against a number.
    let blade_chunk = harness::sample(4, 16, |run| {
        black_box(blade::build_chunk(
            &field,
            IVec2::new(run as i32 % 4, run as i32 / 4),
            1.0,
            7,
        ));
    });
    section.seconds("grass.build.blade_chunk", blade_chunk.median());
}

// --- bandwidth --------------------------------------------------------------

/// What the field costs to hand to the GPU.
///
/// Two full-field passes and two texture uploads every frame, whether anything
/// moved or not. For a system budgeted as background this is the cost that does
/// not go away when the battle stops, which makes it the one most worth
/// knowing.
fn bandwidth(report: &mut Report) {
    for scenario in Scenario::ALL {
        let cells = harness::resolution(scenario);
        let mut section = Section::new(report, scenario.name());
        let field = harness::uniform_field(cells);

        let mut bend = vec![0.0f32; cells * cells * 4];
        let mut state = vec![0.0f32; cells * cells];

        let pack_bend = harness::sample(4, 32, |_| field.pack_bend(black_box(&mut bend)));
        let pack_state = harness::sample(4, 32, |_| field.pack_state(black_box(&mut state)));

        section.seconds("grass.upload.pack_bend", pack_bend.median());
        section.seconds("grass.upload.pack_state", pack_state.median());
        section.ratio(
            "grass.upload.pack_frame_share",
            (pack_bend.median() + pack_state.median()) / FRAME,
            false,
        );
        section.bytes(
            "grass.upload.bytes_per_frame",
            field.upload_bytes() as f64,
            false,
        );
        section.bytes(
            "grass.upload.bytes_per_second",
            field.upload_bytes() as f64 * 60.0,
            false,
        );
        section.bytes("grass.memory.field_state", field.byte_size() as f64, false);
        // Per square metre of ground, which is the form the number takes when
        // deciding how much world can be resident at once.
        let metres = (cells as f64 * harness::CELL as f64).powi(2);
        section.bytes(
            "grass.memory.bytes_per_square_metre",
            field.byte_size() as f64 / metres,
            false,
        );
    }
}

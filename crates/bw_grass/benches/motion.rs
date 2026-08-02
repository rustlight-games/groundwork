//! Whether the motion reads as the thing it is meant to be.
//!
//! Three events, and each one has a specific impression it is supposed to
//! leave. Wind should read as a gust crossing a field. A unit walking should
//! read as a body parting grass. A blast should read as a blast. All three can
//! be physically defensible and read as none of those things — a field that
//! sways together is a water surface, a unit whose grass answers a beat late is
//! a unit sliding over the ground, a blast that ripples outward in rings is a
//! stone dropped in a pond.
//!
//! The metrics here are the difference between those pairs, written down.
//!
//! ## Bands, not maxima
//!
//! Almost nothing in this file wants to be maximised, which is the main way
//! aesthetic metrics differ from performance ones. Wind coherence at 1.0 is a
//! rigid sheet; at 0.0 it is static. Contact latency near zero is right, but
//! contact *spill* near zero means a person walking through grass disturbs
//! exactly their own footprint and nothing around it, which looks like the
//! grass is afraid of them. Each measurement below records the direction of its
//! nearest failure, and says in a comment what the other end looks like.
//!
//! The one thing every metric here is protected against is scoring stillness as
//! success: `dynamic_area` is reported beside the coherence numbers for the
//! same reason `motion_share` sits beside the flicker numbers.

use bevy::math::Vec2;
use bw_bench::Report;
use bw_grass::disturbance::{Shockwave, stamp_interactor, stamp_shockwave};
use bw_grass::field::{GrassField, SIM_STEP};

use crate::harness::{self, Section};

/// Field resolution the motion captures run at.
const CELLS: usize = 192;

/// Bend below which a cell is not visibly doing anything, in radians.
///
/// About three degrees. Under the shader's stiction threshold, so a cell below
/// this is not merely subtle — it is not drawn as moving at all.
const VISIBLE: f32 = 0.05;

pub fn run(report: &mut Report) {
    wind(report);
    contact(report);
    impact(report);
}

// --- wind -------------------------------------------------------------------

/// Does the wind read as weather crossing a field?
fn wind(report: &mut Report) {
    let mut section = Section::new(report, "gusting");

    let breeze = harness::breeze();
    let mut field = harness::settled(CELLS, &breeze, 5.0);
    let mut wind = breeze;
    wind.time += 5.0;

    let frames = 360;
    let resolution = field.resolution();

    // Probes strung out along the wind, to catch a gust arriving at each in
    // turn, and a transverse line to see whether the front is straight.
    let direction = breeze.direction.normalize();
    let across = Vec2::new(-direction.y, direction.x);
    let spacing = 2.0;
    let downwind: Vec<Vec2> = (0..9)
        .map(|index| direction * (index as f32 - 4.0) * spacing)
        .collect();
    let transverse: Vec<Vec2> = (0..9)
        .map(|index| across * (index as f32 - 4.0) * spacing)
        .collect();
    // Close pairs, scattered rather than strung along the front, for the
    // phase-diversity question. The transverse line cannot answer it: a gust
    // front arrives at every point along itself simultaneously by construction,
    // so probes on it correlate at one however varied the field is, and the
    // metric would report a perfect carpet for a perfectly good gust.
    let nearby: Vec<Vec2> = (0..24)
        .map(|index| {
            let angle = index as f32 * 2.399_963_2;
            let radius = 3.0 + (index % 5) as f32 * 1.1;
            Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect();

    let mut downwind_series = vec![Vec::with_capacity(frames); downwind.len()];
    let mut transverse_series = vec![Vec::with_capacity(frames); transverse.len()];
    let mut nearby_series = vec![Vec::with_capacity(frames); nearby.len()];
    let mut field_mean = Vec::with_capacity(frames);
    let mut dynamic = Vec::with_capacity(frames);
    let mut local = Vec::new();
    let mut global = Vec::new();
    let mut micro = Vec::new();

    for frame in 0..frames {
        wind.time += SIM_STEP;
        field.step(SIM_STEP, &wind);

        for (index, probe) in downwind.iter().enumerate() {
            downwind_series[index].push(field.bend_at(*probe).length());
        }
        for (index, probe) in transverse.iter().enumerate() {
            transverse_series[index].push(field.bend_at(*probe).length());
        }
        for (index, probe) in nearby.iter().enumerate() {
            nearby_series[index].push(field.bend_at(*probe).length());
        }

        let theta = field.theta();
        field_mean.push(theta.iter().map(|t| t.length()).sum::<f32>() / theta.len() as f32);
        dynamic.push(
            theta.iter().filter(|t| t.length() > VISIBLE).count() as f64 / theta.len() as f64,
        );

        // Coherence is expensive, so it is sampled rather than run every frame.
        if frame % 30 == 0 {
            local.push(local_coherence(theta, resolution));
            global.push(harness::resultant(theta.iter().map(|&t| (t, t.length()))));
            micro.push(micro_share(theta, resolution));
        }
    }

    // Is anything happening at all? Every number below is meaningless without
    // this one, and several of them improve as it falls.
    section.ratio("grass.wind.dynamic_area", harness::mean(&dynamic), true);
    section.ratio(
        "grass.wind.mean_bend",
        harness::mean(&field_mean.iter().map(|&v| v as f64).collect::<Vec<_>>()),
        true,
    );

    let local = harness::mean(&local);
    let global = harness::mean(&global);
    // Neighbours agreeing with each other. Near zero is noise — every cell
    // leaning its own way, which reads as static rather than as wind.
    section.ratio("grass.wind.local_coherence", local, true);
    // The whole field agreeing. Not a defect on its own: a prevailing wind
    // genuinely does make a meadow lean one way.
    section.ratio("grass.wind.global_coherence", global, true);
    // The two together are the diagnosis. Local coherence with global
    // disagreement is a gust; both high is a carpet — one rigid sheet, which is
    // the single most common way a grass field ends up reading as water.
    section.ratio(
        "grass.wind.carpetness",
        if local > 1e-6 { global / local } else { 0.0 },
        false,
    );
    // Energy in detail finer than a metre, as a share of the total. Low means
    // broad readable waves; high means a shimmer with no shape to it. The
    // stylised target wants macro dominant, so this should be well under a half.
    section.ratio("grass.wind.micro_share", harness::mean(&micro), false);

    // Does a gust travel? Cross-correlate each downwind probe against the first
    // and fit the lags against distance. A field driven by one global vector
    // peaks at zero lag everywhere and fits nothing.
    let mut points = Vec::new();
    for (index, series) in downwind_series.iter().enumerate().skip(1) {
        let (lag, score) = harness::best_lag(&downwind_series[0], series, 120);
        if score > 0.3 {
            points.push((index as f64 * spacing as f64, lag as f64 * SIM_STEP as f64));
        }
    }
    let (speed, fit) = fit_speed(&points);
    section.count("grass.wind.travel_speed", speed, true);
    section.ratio("grass.wind.travel_fit", fit, true);
    // The gust generator's own front speed, for the fitted number to be read
    // against. They should be close; a large gap means the field is smearing
    // the front rather than carrying it.
    section.ratio(
        "grass.wind.travel_speed_error",
        if breeze.gust_speed > 0.0 {
            ((speed - breeze.gust_speed as f64) / breeze.gust_speed as f64).abs()
        } else {
            0.0
        },
        false,
    );

    // Is the front straight? Perfectly straight looks synthetic, so this is not
    // a number to drive to zero — but a large residual means the front has
    // broken up and there is no readable wave left.
    let mut arrivals = Vec::new();
    for (index, series) in transverse_series.iter().enumerate() {
        let peak = series.iter().cloned().fold(0.0f32, f32::max);
        if peak <= VISIBLE {
            continue;
        }
        if let Some(at) = series.iter().position(|&v| v > peak * 0.5) {
            arrivals.push((index as f64 * spacing as f64, at as f64 * SIM_STEP as f64));
        }
    }
    section.count(
        "grass.wind.front_residual",
        straight_line_residual(&arrivals),
        false,
    );

    // How often a gust comes round. Under a second reads as a shimmer; over ten
    // and the field looks becalmed between events.
    section.count(
        "grass.wind.gust_period",
        harness::dominant_period(&field_mean, 60.0),
        true,
    );

    // Do neighbouring plants keep their own phase? One means they move as one
    // object. Near zero would mean they share nothing, which is noise — but the
    // failure this system is actually exposed to is the first one, because
    // every clump reads the same field.
    let mut correlations = Vec::new();
    for a in 0..nearby_series.len() {
        for b in a + 1..nearby_series.len() {
            correlations.push(harness::correlation(&nearby_series[a], &nearby_series[b]));
        }
    }
    section.ratio(
        "grass.wind.phase_diversity",
        1.0 - harness::mean(&correlations),
        true,
    );
}

/// Magnitude-weighted agreement of bend direction within a five-cell window.
fn local_coherence(theta: &[Vec2], resolution: usize) -> f64 {
    let mut total = 0.0;
    let mut counted = 0.0;
    let step = 7;
    for y in (3..resolution - 3).step_by(step) {
        for x in (3..resolution - 3).step_by(step) {
            let mut window = Vec::with_capacity(25);
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let index = ((y as i32 + dy) as usize) * resolution + (x as i32 + dx) as usize;
                    window.push((theta[index], theta[index].length()));
                }
            }
            if window.iter().map(|w| w.1).sum::<f32>() < VISIBLE {
                continue;
            }
            total += harness::resultant(window.into_iter());
            counted += 1.0;
        }
    }
    if counted > 0.0 { total / counted } else { 0.0 }
}

/// Share of bend-magnitude energy in detail finer than about a metre.
fn micro_share(theta: &[Vec2], resolution: usize) -> f64 {
    let radius = (1.0 / harness::CELL).round() as i32;
    let magnitudes: Vec<f32> = theta.iter().map(|t| t.length()).collect();

    let mut macro_energy = 0.0f64;
    let mut micro_energy = 0.0f64;
    let step = 5;
    for y in (radius..resolution as i32 - radius).step_by(step) {
        for x in (radius..resolution as i32 - radius).step_by(step) {
            // Box blur over the window, which is the cheap low-pass. Cheap
            // enough to run over a whole field, and the distinction it draws —
            // broad shapes against fine grain — does not need a better filter.
            let mut sum = 0.0f32;
            let mut count = 0.0f32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    sum += magnitudes[((y + dy) as usize) * resolution + (x + dx) as usize];
                    count += 1.0;
                }
            }
            let low = sum / count;
            let high = magnitudes[(y as usize) * resolution + x as usize] - low;
            macro_energy += (low * low) as f64;
            micro_energy += (high * high) as f64;
        }
    }
    let total = macro_energy + micro_energy;
    if total > 1e-12 {
        micro_energy / total
    } else {
        0.0
    }
}

/// Fit `lag = distance / speed`, returning `(speed, r squared)`.
fn fit_speed(points: &[(f64, f64)]) -> (f64, f64) {
    if points.len() < 3 {
        return (0.0, 0.0);
    }
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / points.len() as f64;
    let top: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let bottom: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    if bottom <= 1e-12 {
        return (0.0, 0.0);
    }
    let slope = top / bottom;

    let residual: f64 = points
        .iter()
        .map(|p| (p.1 - (mean_y + slope * (p.0 - mean_x))).powi(2))
        .sum();
    let variance: f64 = points.iter().map(|p| (p.1 - mean_y).powi(2)).sum();
    let fit = if variance > 1e-12 {
        (1.0 - residual / variance).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Lag is distance over speed, so the speed is the reciprocal of the slope.
    // A slope at or below zero means the "gust" arrives everywhere at once or
    // travels backwards, and neither has a speed worth reporting.
    if slope > 1e-6 {
        (1.0 / slope, fit)
    } else {
        (0.0, 0.0)
    }
}

/// RMS deviation of arrival times from a straight line, in seconds.
fn straight_line_residual(points: &[(f64, f64)]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / points.len() as f64;
    let top: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let bottom: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let slope = if bottom > 1e-12 { top / bottom } else { 0.0 };
    (points
        .iter()
        .map(|p| (p.1 - (mean_y + slope * (p.0 - mean_x))).powi(2))
        .sum::<f64>()
        / points.len() as f64)
        .sqrt()
}

// --- contact ----------------------------------------------------------------

/// Does a body walking through grass read as a body walking through grass?
fn contact(report: &mut Report) {
    let mut section = Section::new(report, "walk");

    let calm = harness::calm();
    let mut field = harness::uniform_field(CELLS);
    let mut body = harness::walker(Vec2::new(-2.4, 0.0));

    // A person's walking pace. Slower and the swept capsule degenerates to a
    // circle, which measures a different thing entirely.
    let pace = 1.4;
    let steps = (4.8 / pace / SIM_STEP) as usize;
    let watch = Vec2::ZERO;

    let mut response = Vec::with_capacity(steps);
    let mut forcing = Vec::with_capacity(steps);
    let mut dose = 0.0f32;
    // Peak dose ever seen in each cell, because dose leaks away and a cell the
    // body crossed at the start of the walk has forgotten by the end of it.
    // Asking "was this cell disturbed" of the final frame answers "is it still
    // disturbed", which is a different and much less useful question.
    let mut peak_dose = vec![0.0f32; field.resolution() * field.resolution()];
    for step in 0..steps {
        let along = -2.4 + step as f32 * pace * SIM_STEP;
        body.move_to(Vec2::new(along, 0.0));
        stamp_interactor(&mut field, &body, SIM_STEP);
        field.step(SIM_STEP, &calm);

        response.push(field.bend_at(watch).length());
        // The forcing signal: how much *new* dose arrived this step. Dose
        // itself is an integral, so its rise is the force.
        let now = field.dose_at(watch);
        forcing.push((now - dose).max(0.0));
        dose = now;
        for (peak, value) in peak_dose.iter_mut().zip(field.dose()) {
            *peak = peak.max(*value);
        }
        let _ = step;
    }

    // Latency, as the lag at which the grass's *rate of change* best matches
    // the force arriving.
    //
    // Not "time from first touch to a tenth of the peak", which is what this
    // measured first and which was wrong in a way worth recording: at a walking
    // pace the force at a point rises over the couple of hundred milliseconds
    // the body takes to arrive, so that version was reporting the unit's
    // transit time and would have stayed at 180 ms however instantly the grass
    // responded. Correlating rates asks only how far behind the force the
    // response runs, which is the actual question.
    let rate: Vec<f32> = response.windows(2).map(|w| w[1] - w[0]).collect();
    let (lag, confidence) = harness::best_lag(&forcing[..rate.len()], &rate, 45);
    section.count(
        "grass.contact.response_lag_ms",
        (lag as f32 * SIM_STEP * 1000.0) as f64,
        false,
    );
    // The lag is only meaningful if the two signals resemble each other at all.
    section.ratio("grass.contact.response_fit", confidence, true);

    let peak = response.iter().cloned().fold(0.0f32, f32::max);
    let peak_at = response
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(step, _)| step)
        .unwrap_or(0);
    let force_at = forcing
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(step, _)| step)
        .unwrap_or(0);
    section.count(
        "grass.contact.peak_lag_ms",
        ((peak_at as f32 - force_at as f32) * SIM_STEP * 1000.0) as f64,
        false,
    );
    section.count("grass.contact.peak_bend", peak as f64, true);

    // Coverage and spill, against the capsule the body actually swept. Coverage
    // near one means the grass under the unit is disturbed; low means a unit
    // walks over grass without troubling it. Spill is the mirror: how much of
    // the disturbance landed outside the sweep. Some is right — grass parts
    // ahead of a body — but a lot means a small character generating a large
    // invisible force field.
    let (coverage, spill) =
        coverage_and_spill(&field, &peak_dose, Vec2::new(-2.4, 0.0), body.current, 0.30);
    section.ratio("grass.contact.coverage", coverage, true);
    section.ratio("grass.contact.spill", spill, false);

    // Which way the grass went. Expected is a blend of the direction of travel
    // and straight out from the path, which is what parting looks like.
    section.ratio(
        "grass.contact.direction_agreement",
        parting_agreement(&field, Vec2::X),
        true,
    );

    // How wide a track the body left, over how wide the body is. Under one
    // means the trail is narrower than the unit, which reads as the grass
    // closing behind it too eagerly.
    section.ratio(
        "grass.contact.track_width_ratio",
        track_width(&field) / (2.0 * 0.30),
        true,
    );

    // The wake: does the slow memory lie along the path? This is the channel
    // that makes a trail persist, and if it does not align with travel the
    // trail reads as damage rather than as a track.
    let mut wake = Vec::new();
    for step in 0..12 {
        let at = Vec2::new(-2.0 + step as f32 * 0.3, 0.0);
        let memory = field.slow_memory_at(at);
        if memory.length() > 1e-4 {
            wake.push(memory.normalize().dot(Vec2::X) as f64);
        }
    }
    section.ratio("grass.contact.wake_alignment", harness::mean(&wake), true);

    // Recovery, once the body has gone. Two timescales, because the system has
    // two: the fast spring back that happens in under a second, and the slow
    // memory that keeps a path visible for a battle.
    section.scenario("recovery");
    let mut relax = Vec::new();
    let mut compaction = Vec::new();
    let settled_peak = field.compaction_at(watch);
    for _ in 0..(30.0 / SIM_STEP) as usize {
        field.step(SIM_STEP, &calm);
        relax.push(field.bend_at(watch).length());
        compaction.push(field.compaction_at(watch));
    }

    section.count(
        "grass.contact.recovery_half_life",
        harness::decay_time(&relax, SIM_STEP, 0.5),
        false,
    );
    section.count(
        "grass.contact.recovery_fast_tau",
        harness::fitted_tau(&relax, SIM_STEP, 0, (1.5 / SIM_STEP) as usize),
        false,
    );
    // Persistence, the other way round: how much of the flattening survives.
    // This one wants to be *high* — a trail that has vanished after ten seconds
    // never happened as far as a player is concerned.
    let remaining = |seconds: f32| -> f64 {
        let index = ((seconds / SIM_STEP) as usize).min(compaction.len().saturating_sub(1));
        if settled_peak > 1e-6 {
            (compaction[index] / settled_peak) as f64
        } else {
            0.0
        }
    };
    section.ratio("grass.contact.trail_at_10s", remaining(10.0), true);
    section.ratio("grass.contact.trail_at_30s", remaining(30.0), true);
    section.count(
        "grass.contact.trail_tau",
        harness::fitted_tau(&compaction, SIM_STEP, 0, compaction.len()),
        true,
    );
}

/// `(coverage, spill)` of the disturbance against the swept capsule.
fn coverage_and_spill(
    field: &GrassField,
    peak_dose: &[f32],
    from: Vec2,
    to: Vec2,
    radius: f32,
) -> (f64, f64) {
    let resolution = field.resolution();
    // Scaled against the strongest dose anywhere, so coverage answers "was this
    // cell disturbed like the rest of the track" rather than "did this cell
    // reach one severity-second", which is a threshold with no meaning.
    let strongest = peak_dose.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    let (mut inside, mut want, mut outside, mut all) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for y in 0..resolution {
        for x in 0..resolution {
            let at = field.cell_center(x, y);
            let dose = (peak_dose[y * resolution + x] / strongest).min(1.0) as f64;
            // Distance to the swept segment, which is what the stamp used.
            let along = to - from;
            let t = if along.length_squared() > 1e-9 {
                ((at - from).dot(along) / along.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let expected = if (at - (from + along * t)).length() <= radius {
                1.0
            } else {
                0.0
            };
            inside += dose.min(expected);
            want += expected;
            outside += dose * (1.0 - expected);
            all += dose;
        }
    }
    (
        if want > 0.0 { inside / want } else { 0.0 },
        if all > 0.0 { outside / all } else { 0.0 },
    )
}

/// Agreement between the bend and the direction grass ought to part in.
fn parting_agreement(field: &GrassField, travel: Vec2) -> f64 {
    let mut total = 0.0;
    let mut weight = 0.0;
    for step in -8i32..=8 {
        for side in [-1.0f32, 1.0] {
            let at = Vec2::new(step as f32 * 0.25, side * 0.28);
            let bend = field.bend_at(at);
            if bend.length() < VISIBLE {
                continue;
            }
            // Grass under a body goes partly the way the body is going and
            // partly straight out of its way. Half and half; the exact blend is
            // an art choice, and the metric exists to notice it drifting rather
            // than to prescribe it.
            let outward = Vec2::new(0.0, side);
            let expected = (travel.normalize() * 0.5 + outward * 0.5).normalize();
            total += bend.normalize().dot(expected) as f64 * bend.length() as f64;
            weight += bend.length() as f64;
        }
    }
    if weight > 1e-9 { total / weight } else { 0.0 }
}

/// Width of the flattened track, in metres.
fn track_width(field: &GrassField) -> f64 {
    let peak = field.compaction_at(Vec2::ZERO);
    if peak <= 1e-6 {
        return 0.0;
    }
    let mut width = 0.0;
    for step in 0..40 {
        let offset = step as f32 * harness::CELL;
        if field.compaction_at(Vec2::new(0.0, offset)) < peak * 0.5 {
            break;
        }
        width = offset;
    }
    (width * 2.0) as f64
}

// --- impact -----------------------------------------------------------------

/// Does a blast read as a blast?
fn impact(report: &mut Report) {
    let mut section = Section::new(report, "blast");

    let calm = harness::calm();
    let mut field = harness::uniform_field(CELLS);
    let mut wave = Shockwave::default();

    let frames = (2.5 / SIM_STEP) as usize;
    let mut energy = Vec::with_capacity(frames);
    let mut local = Vec::with_capacity(frames);
    let mut centroid = Vec::with_capacity(frames);
    let mut width = Vec::with_capacity(frames);
    let mut alignment = Vec::new();
    let mut sectors_at_peak = Vec::new();
    let mut peak_frame = 0;

    for frame in 0..frames {
        if !wave.finished() {
            stamp_shockwave(&mut field, &wave);
        }
        field.step(SIM_STEP, &calm);
        wave.age += SIM_STEP;

        let (total, radius, spread) = radial_profile(&field, wave.origin);
        energy.push(total);
        // Energy near the origin, separately from the whole field's. The two
        // answer different questions and conflating them was reporting a rise
        // time of a second: whole-field energy keeps climbing for as long as
        // the ring keeps covering new ground, so its peak is when the blast
        // *ends*, not when it lands. Punch is local and immediate.
        local.push(local_energy(&field, wave.origin, 2.5));
        centroid.push(radius);
        width.push(spread);
        if total >= energy.iter().cloned().fold(0.0f32, f32::max) {
            peak_frame = frame;
            sectors_at_peak = sector_energy(&field, wave.origin, 12);
        }
        // Only while the ring is still expanding: during the rebound, grass
        // swinging back through the middle is *supposed* to point inward, and
        // scoring that as misalignment would punish the recovery for existing.
        if !wave.finished() {
            alignment.push(radial_alignment(&field, wave.origin));
        }
    }

    // Does it push outward? Near one is a clean expanding front; near zero is a
    // patch of grass being shaken.
    section.ratio(
        "grass.impact.radial_alignment",
        harness::mean(&alignment),
        true,
    );

    // How fast the disturbance travels, against the speed the wave was given.
    // Slower means the field is lagging the front and the ring reads as soft.
    let expansion = {
        // The window is taken from the blast's own lifetime rather than being
        // fixed in seconds, which it was — and that broke the moment the blast
        // was retuned from a slow travelling ring into a fast one. The old
        // window ran from 0.15 s to 0.9 s, and a front that crosses its whole
        // radius in 0.16 s has finished expanding before the window opens, so
        // the metric reported a front speed of exactly zero for a blast that
        // had got *six times faster*. A measurement that reads zero when the
        // thing it measures goes up is worse than no measurement.
        let life = (wave.max_radius / wave.speed.max(1e-3)) as f64;
        let from = ((life * 0.15) / SIM_STEP as f64) as usize;
        let to = ((life * 0.85) / SIM_STEP as f64) as usize;
        if to > from && to < centroid.len() && centroid[to] > centroid[from] {
            ((centroid[to] - centroid[from]) as f64 / ((to - from) as f64 * SIM_STEP as f64))
        } else {
            0.0
        }
    };
    section.count("grass.impact.front_speed", expansion, true);
    section.ratio(
        "grass.impact.front_speed_error",
        ((expansion - wave.speed as f64) / wave.speed as f64).abs(),
        false,
    );
    // How thick the ring is. A narrow ring is a ripple on water; a broad one is
    // a shove. Grass wants the shove.
    section.count(
        "grass.impact.ring_width",
        width.get(peak_frame).copied().unwrap_or(0.0) as f64,
        true,
    );

    // Round, or lopsided? Not to be driven to zero — a perfectly even ring is a
    // procedural shockwave texture, and the generator adds raggedness on
    // purpose — but a large value means the blast has a side.
    section.ratio(
        "grass.impact.sector_variation",
        harness::variation(&sectors_at_peak),
        false,
    );

    // Punch, measured where the blast went off. How long to peak, and how much
    // is there when it arrives.
    let local_peak_frame = local
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(frame, _)| frame)
        .unwrap_or(0);
    section.count(
        "grass.impact.rise_time_ms",
        (local_peak_frame as f32 * SIM_STEP * 1000.0) as f64,
        false,
    );
    section.count(
        "grass.impact.peak_energy",
        local.iter().cloned().fold(0.0f32, f32::max) as f64,
        true,
    );
    section.count(
        "grass.impact.field_peak_energy",
        energy.iter().cloned().fold(0.0f32, f32::max) as f64,
        true,
    );

    // The crater, which is most of what a player sees.
    //
    // A blast arrives in two frames and is gone in ten, which is far too fast
    // to read as anything but a flash. What actually says "something went off
    // here" is the patch of laid-over grass it leaves and the second or so it
    // takes to stand back up — so the mark is a first-class measurement, not an
    // afterthought to the kick.
    let mut crater = Vec::new();
    let mut cells = 0.0f64;
    let mut total = 0.0f64;
    for y in 0..field.resolution() {
        for x in 0..field.resolution() {
            if (field.cell_center(x, y) - wave.origin).length() <= wave.max_radius {
                total += field.compaction()[y * field.resolution() + x] as f64;
                cells += 1.0;
            }
        }
    }
    crater.push(total / cells.max(1.0));
    section.count("grass.impact.crater_compaction", crater[0], true);
    section.count(
        "grass.impact.crater_axis",
        field.axis_at(wave.origin + Vec2::new(2.0, 0.0)).length() as f64,
        true,
    );

    // Afterwards. One soft overshoot is good; several is a spring toy, and a
    // rising envelope is a solver about to come apart.
    let tail: Vec<f32> = local[local_peak_frame.min(local.len() - 1)..].to_vec();
    section.count("grass.impact.overshoots", local_maxima(&tail) as f64, false);
    section.count(
        "grass.impact.settle_time",
        harness::decay_time(&tail, SIM_STEP, 0.10),
        false,
    );

    // Three blasts in the same place. The field should saturate — the third
    // cannot flatten what is already flat — but not go inert, and certainly not
    // amplify.
    section.scenario("repeated");
    let mut field = harness::uniform_field(CELLS);
    let mut peaks = Vec::new();
    for round in 0..3 {
        let mut wave = Shockwave {
            seed: 0x51A5_5EED ^ round,
            ..Default::default()
        };
        let mut peak = 0.0f32;
        for _ in 0..(1.6 / SIM_STEP) as usize {
            if !wave.finished() {
                stamp_shockwave(&mut field, &wave);
            }
            field.step(SIM_STEP, &calm);
            wave.age += SIM_STEP;
            peak = peak.max(local_energy(&field, wave.origin, 2.5));
        }
        peaks.push(peak as f64);
    }
    section.ratio(
        "grass.impact.saturation",
        if peaks[0] > 1e-9 {
            peaks[2] / peaks[0]
        } else {
            0.0
        },
        // Lower is the *safe* direction: above one means repeated blasts are
        // adding energy, which is the failure that ends with a field vibrating.
        false,
    );
}

/// `(total energy, energy-weighted radius, radial spread)` around a point.
fn radial_profile(field: &GrassField, origin: Vec2) -> (f32, f32, f32) {
    let resolution = field.resolution();
    let theta = field.theta();
    let (mut total, mut weighted, mut second) = (0.0f32, 0.0f32, 0.0f32);
    for y in (0..resolution).step_by(2) {
        for x in (0..resolution).step_by(2) {
            let energy = theta[y * resolution + x].length_squared();
            if energy <= 1e-8 {
                continue;
            }
            let radius = (field.cell_center(x, y) - origin).length();
            total += energy;
            weighted += energy * radius;
            second += energy * radius * radius;
        }
    }
    if total <= 1e-9 {
        return (0.0, 0.0, 0.0);
    }
    let mean = weighted / total;
    let spread = (second / total - mean * mean).max(0.0).sqrt();
    (total, mean, spread)
}

/// Bend energy within `radius` of a point.
fn local_energy(field: &GrassField, origin: Vec2, radius: f32) -> f32 {
    let resolution = field.resolution();
    let theta = field.theta();
    let mut total = 0.0;
    for y in 0..resolution {
        for x in 0..resolution {
            if (field.cell_center(x, y) - origin).length() <= radius {
                total += theta[y * resolution + x].length_squared();
            }
        }
    }
    total
}

/// How much of the bend points away from the origin.
fn radial_alignment(field: &GrassField, origin: Vec2) -> f64 {
    let resolution = field.resolution();
    let theta = field.theta();
    let (mut total, mut weight) = (0.0f64, 0.0f64);
    for y in (0..resolution).step_by(2) {
        for x in (0..resolution).step_by(2) {
            let bend = theta[y * resolution + x];
            if bend.length() < VISIBLE {
                continue;
            }
            let offset = field.cell_center(x, y) - origin;
            if offset.length() < 0.2 {
                continue;
            }
            total += bend.normalize().dot(offset.normalize()) as f64 * bend.length() as f64;
            weight += bend.length() as f64;
        }
    }
    if weight > 1e-9 { total / weight } else { 0.0 }
}

/// Bend energy in each of `count` angular sectors around a point.
fn sector_energy(field: &GrassField, origin: Vec2, count: usize) -> Vec<f64> {
    let resolution = field.resolution();
    let theta = field.theta();
    let mut sectors = vec![0.0f64; count];
    for y in (0..resolution).step_by(2) {
        for x in (0..resolution).step_by(2) {
            let energy = theta[y * resolution + x].length_squared() as f64;
            if energy <= 1e-8 {
                continue;
            }
            let offset = field.cell_center(x, y) - origin;
            if offset.length() < 0.2 {
                continue;
            }
            let angle = offset.y.atan2(offset.x) + std::f32::consts::PI;
            let sector = ((angle / std::f32::consts::TAU) * count as f32) as usize;
            sectors[sector.min(count - 1)] += energy;
        }
    }
    sectors
}

/// Count the humps in a decaying signal.
fn local_maxima(signal: &[f32]) -> usize {
    let peak = signal.iter().cloned().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return 0;
    }
    let mut count = 0;
    for window in signal.windows(3) {
        // Only humps worth seeing. Numerical wobble a thousandth of the peak
        // high is not an overshoot, and counting it would make the metric
        // measure floating-point noise.
        if window[1] > window[0] && window[1] >= window[2] && window[1] > peak * 0.05 {
            count += 1;
        }
    }
    count
}

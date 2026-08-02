//! Smoothness: whether the grass moves, or merely changes.
//!
//! This is the section that exists because the field visibly flickers, and the
//! reason a flicker is hard to chase is that it can be born in any of four
//! places, each of which looks innocent from the other three:
//!
//! | Born in | Looks like | Measured by |
//! |---|---|---|
//! | The wind | Everything shimmering at once | `wind_hf_ratio` |
//! | The solver | Cells vibrating against their neighbours | `field_hf_ratio`, `ambient_jerk` |
//! | The pixel grid | Edges crawling; motion that stutters rather than glides | `pixel_chatter`, `silhouette_churn` |
//! | The sampler | Sparkle inside the sprite, worse when the camera moves | `atlas_minification`, `subpixel_*` |
//!
//! A single "is it flickering" score would have told you it flickers, which you
//! already knew from looking at it. Four numbers tell you where to go.
//!
//! ## Stillness is not stability
//!
//! Every measurement here is paired with a *required motion* number, and the
//! pairing is the load-bearing part. A field that does not move at all scores
//! perfectly on all of these: no jerk, no chatter, no churn, nothing to sample.
//! Optimise against them alone and the correct answer is to switch the wind
//! off. `motion_share` and `tip_travel` are what stop that: they say how much
//! of the field moved and how far, and a stability improvement that also drops
//! those has not improved anything.
//!
//! ## The threshold
//!
//! High-frequency energy is measured above 8 Hz, which is not arbitrary. Grass
//! motion the eye reads as motion — a gust crossing, a plant springing back —
//! lives below about 3 Hz. Between 3 and 8 Hz is the fast end of legitimate
//! response. Above 8 Hz, at 60 frames a second, is at most seven frames per
//! cycle, and nothing in a stylised field is supposed to oscillate that fast.
//! Whatever energy is up there is a defect regardless of which stage put it
//! there.

use bevy::math::{UVec2, Vec2};
use bw_bench::Report;
use bw_grass::clump;
use bw_grass::field::SIM_STEP;
use bw_grass::pixel;
use bw_grass::wind::WindField;
use bw_render::BattleCamera;

use crate::harness::{self, Section};
use crate::mirror;

/// Above this, motion is not motion. See the module docs.
const FLICKER_HZ: f32 = 8.0;

/// Frames a temporal capture runs for. Five seconds at 60 Hz — long enough for
/// a gust to cross and for the spectrum to have resolution down to 0.2 Hz.
const FRAMES: usize = 300;

/// Field resolution the stability captures run at.
///
/// The shipped one. Resolution changes the spatial frequency the solver can
/// hold, so measuring smoothness at a different one measures a different
/// renderer.
const CELLS: usize = 256;

pub fn run(report: &mut Report) {
    // Fail loudly if the CPU model of the shader has drifted, before anything
    // downstream of it is reported as a measurement.
    mirror::assert_matches_shader(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/clump.wgsl"
        ))
        .expect("the clump shader must exist"),
    );

    sources(report);
    field(report);
    screen(report);
    sampling(report);
}

// --- where the motion comes from --------------------------------------------

/// Is the wind itself smooth?
///
/// First, because everything downstream inherits whatever is here. Curl noise
/// and travelling gusts are both analytic and both continuous in time, so this
/// should be very clean — and if it is not, nothing further down can be.
fn sources(report: &mut Report) {
    let mut section = Section::new(report, "wind");

    for (name, mut wind) in [
        ("ambient", harness::ambient()),
        ("gusting", harness::breeze()),
    ] {
        // Sampled at three places rather than one: a single probe can sit on a
        // node of the turbulence and report a calm that is not there.
        let probes = [
            Vec2::new(0.0, 0.0),
            Vec2::new(7.3, -4.1),
            Vec2::new(-5.7, 9.2),
        ];
        let mut series: Vec<Vec<f32>> = vec![Vec::with_capacity(FRAMES); probes.len()];
        let mut speeds = Vec::with_capacity(FRAMES);
        for _ in 0..FRAMES {
            wind.time += SIM_STEP;
            for (index, probe) in probes.iter().enumerate() {
                let velocity = wind.velocity_at(*probe);
                series[index].push(velocity.length());
            }
            speeds.push(wind.velocity_at(probes[0]).length() as f64);
        }

        let hf = harness::mean(
            &series
                .iter()
                .map(|s| harness::high_frequency_ratio(s, 60.0, FLICKER_HZ))
                .collect::<Vec<_>>(),
        );
        section.scenario(name);
        section.ratio("grass.stability.wind_hf_ratio", hf, false);
        // Paired with the amount of variation there is to be smooth about: a
        // constant wind is perfectly smooth and completely dead.
        section.ratio(
            "grass.stability.wind_variation",
            harness::variation(&speeds),
            true,
        );
        // Slower is the safe direction: a wind whose dominant rhythm is under a
        // second is a shimmer, not weather.
        section.count(
            "grass.stability.wind_period",
            harness::dominant_period(&series[0], 60.0),
            true,
        );
    }
}

// --- the solver -------------------------------------------------------------

/// Is the field smooth?
fn field(report: &mut Report) {
    let mut section = Section::new(report, "calm");

    // Rest drift: with no wind and nothing touching it, a settled field must
    // stop. Anything above the solver's own quiet threshold is grass that
    // twitches on an empty screen, which is the most damning flicker there is
    // because there is nothing to blame it on.
    let calm = harness::calm();
    let mut field = harness::settled(CELLS, &calm, 4.0);
    let mut worst: f32 = 0.0;
    let mut previous: Vec<Vec2> = field.theta().to_vec();
    for _ in 0..120 {
        field.step(SIM_STEP, &calm);
        for (now, before) in field.theta().iter().zip(&previous) {
            worst = worst.max((*now - *before).length());
        }
        previous.copy_from_slice(field.theta());
    }
    section.count("grass.stability.rest_drift", worst as f64, false);

    // Under ambient wind: mean flow and turbulence, no gust fronts. The
    // condition a stability metric belongs in — under gusts the field is
    // supposed to be moving hard, and a metric that cannot tell wanted motion
    // from unwanted would call a gale a defect.
    section.scenario("ambient");
    let (probes, magnitudes, angles) = probe_field(&harness::ambient());

    section.ratio(
        "grass.stability.field_hf_ratio",
        harness::mean(
            &magnitudes
                .iter()
                .map(|s| harness::high_frequency_ratio(s, 60.0, FLICKER_HZ))
                .collect::<Vec<_>>(),
        ),
        false,
    );
    // Direction separately from magnitude, because they fail differently and
    // look different. A blade whose lean *strength* jitters shimmers; a blade
    // whose lean *direction* jitters wags, and wagging is far more visible at
    // small amplitudes than shimmer is.
    section.ratio(
        "grass.stability.direction_hf_ratio",
        harness::mean(
            &angles
                .iter()
                .map(|s| harness::high_frequency_ratio(s, 60.0, FLICKER_HZ))
                .collect::<Vec<_>>(),
        ),
        false,
    );

    // Jerk: the third derivative, at the 95th percentile so that one cell
    // cannot set it. Normalised by the amplitude actually present, so a calmer
    // field does not score better simply by moving less.
    let mut jerks = Vec::new();
    let mut amplitude = 0.0f64;
    for series in &magnitudes {
        let mut local = Vec::with_capacity(series.len());
        for window in series.windows(4) {
            let jerk = (window[3] - 3.0 * window[2] + 3.0 * window[1] - window[0]).abs();
            local.push(jerk as f64);
        }
        jerks.push(harness::percentile(&local, 0.95));
        amplitude += harness::deviation(&series.iter().map(|&v| v as f64).collect::<Vec<_>>());
    }
    let amplitude = amplitude / magnitudes.len().max(1) as f64;
    section.ratio(
        "grass.stability.jerk_p95",
        if amplitude > 1e-9 {
            harness::mean(&jerks) / amplitude
        } else {
            0.0
        },
        false,
    );

    // The required-motion pair. Without it every number above can be improved
    // to perfection by not moving.
    section.ratio(
        "grass.stability.field_motion",
        harness::mean(
            &magnitudes
                .iter()
                .map(|s| harness::deviation(&s.iter().map(|&v| v as f64).collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
        ) / harness::mean(
            &magnitudes
                .iter()
                .map(|s| harness::mean(&s.iter().map(|&v| v as f64).collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
        )
        .max(1e-9),
        true,
    );
    section.count("grass.stability.field_probes", probes as f64, true);
}

/// Bend magnitude and bend angle over time, at a grid of probes.
fn probe_field(wind: &WindField) -> (usize, Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut field = harness::settled(CELLS, wind, 4.0);
    let mut wind = *wind;
    wind.time += 4.0;

    // Spread across the field rather than clustered, and offset off the lattice
    // so no probe sits exactly on a cell centre — the wind is baked onto a
    // coarser grid than the field, and probing exactly on its nodes would miss
    // the interpolation entirely.
    let side = 8;
    let span = CELLS as f32 * harness::CELL * 0.35;
    let mut points = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let u = (x as f32 + 0.37) / side as f32 * 2.0 - 1.0;
            let v = (y as f32 + 0.61) / side as f32 * 2.0 - 1.0;
            points.push(Vec2::new(u, v) * span);
        }
    }

    let mut magnitudes = vec![Vec::with_capacity(FRAMES); points.len()];
    let mut angles = vec![Vec::with_capacity(FRAMES); points.len()];
    for _ in 0..FRAMES {
        wind.time += SIM_STEP;
        field.step(SIM_STEP, &wind);
        for (index, point) in points.iter().enumerate() {
            let bend = field.bend_at(*point);
            magnitudes[index].push(bend.length());
            // Unwrapped against the previous sample, so a lean crossing due
            // south does not register as a full turn.
            let raw = bend.y.atan2(bend.x);
            let previous = angles[index].last().copied().unwrap_or(raw);
            let mut unwrapped = raw;
            while unwrapped - previous > std::f32::consts::PI {
                unwrapped -= std::f32::consts::TAU;
            }
            while previous - unwrapped > std::f32::consts::PI {
                unwrapped += std::f32::consts::TAU;
            }
            angles[index].push(unwrapped);
        }
    }
    (points.len(), magnitudes, angles)
}

// --- the screen -------------------------------------------------------------

/// Is what gets drawn smooth?
///
/// Everything here runs through [`mirror`], which is the shader's vertex stage
/// in Rust. The distance between this section and the one above it is the
/// distance between the simulation and the picture, and the two disagree in
/// both directions: stiction can hide field jitter, and the pixel grid can
/// manufacture flicker out of perfectly smooth field motion.
fn screen(report: &mut Report) {
    let mut section = Section::new(report, "gusting");

    // The shipped wind, gusts and all — not the ambient breeze the field
    // section runs under, and the difference is not a detail. The shader
    // ignores bend below `STICTION`, which under an ambient breeze is *all* of
    // it: measured against a light breeze, every clump in the field is frozen
    // and every metric below reads a perfect zero for the worst possible
    // reason. `stiction_share` is reported so that fact stays visible rather
    // than being something the scenario name quietly hides.
    let wind = harness::breeze();
    let mut field = harness::settled(CELLS, &wind, 4.0);
    let mut wind = wind;
    wind.time += 4.0;

    let settings = mirror::Settings::shipped(&field);
    let clumps = mirror::sample(&field, 3, 0x6A72_A551);

    // Canvas pixels per world unit at the framing the game actually ships, so
    // "one pixel" here means one pixel there.
    let (_, canvas) = pixel::canvas_geometry(UVec2::new(1920, 1080));
    let pixels_per_unit = canvas.y as f32 / BattleCamera::default().view_height;

    // Spectral analysis is quadratic in the capture length, so it runs on a
    // subset. The per-frame counters below run on every clump.
    let stride = (clumps.len() / 192).max(1);

    let mut tip_series: Vec<Vec<f32>> = Vec::new();
    let mut pixel_history: Vec<Vec<(i32, i32)>> = vec![Vec::with_capacity(FRAMES); clumps.len()];
    let mut silhouette_history: Vec<Vec<i32>> = vec![Vec::with_capacity(FRAMES); clumps.len()];
    let mut depth_history: Vec<Vec<f32>> = vec![Vec::with_capacity(FRAMES); clumps.len()];
    let mut rest = Vec::with_capacity(clumps.len());
    let mut ever_responded = vec![false; clumps.len()];

    for frame in 0..FRAMES {
        wind.time += SIM_STEP;
        field.step(SIM_STEP, &wind);

        for (index, clump) in clumps.iter().enumerate() {
            let placed = mirror::place(clump, &field, &settings);
            if mirror::response(clump, &field, &settings).1 > 1e-4 {
                ever_responded[index] = true;
            }
            if frame == 0 {
                rest.push(placed);
            }
            let pixel = (
                (placed.tip.x * pixels_per_unit).round() as i32,
                (placed.tip.y * pixels_per_unit).round() as i32,
            );
            pixel_history[index].push(pixel);
            silhouette_history[index].push((placed.silhouette * pixels_per_unit).round() as i32);
            depth_history[index].push(placed.depth);
            if index % stride == 0 {
                let slot = index / stride;
                if tip_series.len() <= slot {
                    tip_series.push(Vec::with_capacity(FRAMES));
                }
                // Displacement from rest along the screen, as one signal. The
                // sign carries the direction, so an oscillation shows up as an
                // oscillation rather than as a rectified hump.
                let offset = placed.tip - rest[index].tip;
                tip_series[slot].push(offset.x + offset.y);
            }
        }
    }

    // Continuous motion, before the pixel grid touches it.
    section.ratio(
        "grass.stability.tip_hf_ratio",
        harness::mean(
            &tip_series
                .iter()
                .map(|s| harness::high_frequency_ratio(s, 60.0, FLICKER_HZ))
                .collect::<Vec<_>>(),
        ),
        false,
    );

    // How much of the field the shader lets move at all. The stiction
    // threshold is deliberate — grass at rest is still, and a plant that
    // answers every ripple is a liquid — but it is also the number that decides
    // whether a gust reads as crossing a *field* or as crossing a few plants in
    // it. Low here with a healthy `grass.wind.dynamic_area` means the
    // simulation is doing work the renderer is throwing away.
    section.ratio(
        "grass.stability.responsive_share",
        ever_responded.iter().filter(|&&on| on).count() as f64 / clumps.len().max(1) as f64,
        true,
    );

    // Do neighbours disagree? Every clump reads the same bend field, and the
    // only thing that stops a gust turning the whole surface into one sheet is
    // the wide per-clump stiffness spread the shader applies. This is that
    // spread, measured on the output: one means neighbouring plants move in
    // lockstep, which is what water does.
    let mut neighbour = Vec::new();
    for slot in 1..tip_series.len() {
        neighbour.push(harness::correlation(
            &tip_series[slot - 1],
            &tip_series[slot],
        ));
    }
    section.ratio(
        "grass.stability.neighbour_agreement",
        harness::mean(&neighbour),
        false,
    );

    // Pixel cadence. At this scale a plant's tip moves a fraction of a pixel
    // per frame, so what a viewer sees is a sequence of one-pixel steps. The
    // three ways that goes wrong:
    //
    // - **chatter**: a step immediately reversed. Reads as a pixel vibrating,
    //   and is the single most visible defect on this list.
    // - **jump**: more than one pixel in a frame. Reads as a snap.
    // - **hold**: no step at all. Correct and desirable in moderation — pixel
    //   art holds — but a field that holds almost always is a still field.
    let mut chatter = 0u64;
    let mut jumps = 0u64;
    let mut holds = 0u64;
    let mut steps = 0u64;
    let mut moved = 0u64;
    let mut travel = Vec::with_capacity(clumps.len());
    for history in &pixel_history {
        let mut previous_delta = (0i32, 0i32);
        let mut far = 0i32;
        for pair in history.windows(2) {
            let delta = (pair[1].0 - pair[0].0, pair[1].1 - pair[0].1);
            steps += 1;
            if delta == (0, 0) {
                holds += 1;
            } else {
                far = far.max(delta.0.abs().max(delta.1.abs()));
                if delta.0.abs() > 1 || delta.1.abs() > 1 {
                    jumps += 1;
                }
                if previous_delta != (0, 0) && delta == (-previous_delta.0, -previous_delta.1) {
                    chatter += 1;
                }
                previous_delta = delta;
            }
        }
        if far > 0 {
            moved += 1;
        }
        let span = history.iter().fold((i32::MAX, i32::MIN), |acc, p| {
            (acc.0.min(p.0), acc.1.max(p.0))
        });
        travel.push((span.1 - span.0) as f64);
    }
    let steps = steps.max(1) as f64;

    section.ratio(
        "grass.stability.pixel_chatter",
        chatter as f64 / steps,
        false,
    );
    section.ratio("grass.stability.pixel_jump", jumps as f64 / steps, false);
    section.ratio("grass.stability.pixel_hold", holds as f64 / steps, false);
    let travel_mean = harness::mean(&travel).max(1e-6);
    // The required-motion pair for all three.
    section.ratio(
        "grass.stability.motion_share",
        moved as f64 / clumps.len().max(1) as f64,
        true,
    );
    section.count("grass.stability.tip_travel_pixels", travel_mean, true);

    // Silhouette churn: the drawn height changing by a whole pixel. Every one
    // of these is a row of pixels appearing or vanishing along the top of a
    // plant, which is exactly what "the grass is sparkling" looks like when you
    // slow it down.
    let mut churn = 0u64;
    for history in &silhouette_history {
        for pair in history.windows(2) {
            if pair[0] != pair[1] {
                churn += 1;
            }
        }
    }
    let churn_rate = churn as f64 / steps;
    section.ratio("grass.stability.silhouette_churn", churn_rate, false);
    // Flicker per pixel of motion, and this is the pair of numbers to read
    // first.
    //
    // The raw rates above cannot be compared between two builds that move by
    // different amounts, and comparing them anyway is the single easiest way to
    // draw the wrong conclusion from this suite: a field that has been made
    // *stiller* will always show less chatter and less churn, because there is
    // less of everything. Dividing by how far a plant actually travels asks the
    // question that survives the comparison — for the motion this build
    // produces, how much of it arrives as a wobble rather than as movement.
    section.ratio(
        "grass.stability.chatter_per_pixel",
        chatter as f64 / steps / travel_mean,
        false,
    );
    section.ratio(
        "grass.stability.churn_per_pixel",
        churn_rate / travel_mean,
        false,
    );

    // Depth pops. Two plants that overlap on screen and swap sort order mid-gust
    // is a whole sprite jumping in front of another — far more visible than any
    // per-pixel defect, and invisible to every field-side measurement.
    let pairs = overlapping_pairs(&clumps, &rest);
    let mut flips = 0u64;
    for &(a, b) in &pairs {
        let mut sign = 0i8;
        for (near, far) in depth_history[a].iter().zip(&depth_history[b]) {
            let now = (near - far).signum() as i8;
            if sign != 0 && now != 0 && now != sign {
                flips += 1;
            }
            if now != 0 {
                sign = now;
            }
        }
    }
    section.ratio(
        "grass.stability.depth_pop_rate",
        flips as f64 / (pairs.len().max(1) * FRAMES) as f64,
        false,
    );
    // Sample sizes, so a run that measured almost nothing says so.
    section.count(
        "grass.stability.overlapping_pairs",
        pairs.len() as f64,
        true,
    );
    section.count("grass.stability.clumps_sampled", clumps.len() as f64, true);
}

/// Pairs of clumps whose sprites overlap on screen in the rest pose.
///
/// Only these can pop: two plants that never overlap can be sorted in any order
/// without a viewer being able to tell. Capped at a few neighbours each, so the
/// count stays linear rather than quadratic in a field of thousands.
fn overlapping_pairs(clumps: &[mirror::Clump], rest: &[mirror::Placement]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..clumps.len()).collect();
    order.sort_by(|&a, &b| rest[a].root.x.total_cmp(&rest[b].root.x));

    let mut pairs = Vec::new();
    for (position, &a) in order.iter().enumerate() {
        let mut found = 0;
        for &b in order.iter().skip(position + 1) {
            let gap = rest[b].root.x - rest[a].root.x;
            let reach = (clumps[a].width + clumps[b].width) * 0.5;
            if gap > reach {
                break;
            }
            // Vertically too: the sprites run from the root up to the tip.
            let overlap = rest[a].tip.y.min(rest[b].tip.y) - rest[a].root.y.max(rest[b].root.y);
            if overlap > 0.0 {
                pairs.push((a, b));
                found += 1;
                if found >= 4 {
                    break;
                }
            }
        }
    }
    pairs
}

// --- the sampler ------------------------------------------------------------

/// Is the sprite stable when it moves?
///
/// The stage nothing else in the suite can see, and the one most likely to be
/// responsible for what the field looks like on screen right now. The atlas is
/// baked at 64 pixels a cell, the sampler is linear, and there are no mipmaps —
/// so if a clump is drawn smaller than 64 pixels, every screen pixel is a blend
/// of a *shifting subset* of atlas texels. Move the camera or the plant a
/// fraction of a pixel and a different subset is picked, and the sprite's
/// interior sparkles even though nothing about it changed.
///
/// The measurement is direct: resample a real sprite at the size it is really
/// drawn, across a full pixel of sub-pixel offsets, and count what changes.
fn sampling(report: &mut Report) {
    let mut section = Section::new(report, "shipped");

    let atlas = clump::bake(&clump::Style::default(), 0x6A72_A551);
    let (_, canvas) = pixel::canvas_geometry(UVec2::new(1920, 1080));
    let pixels_per_unit = canvas.y as f32 / BattleCamera::default().view_height;

    // The size range a clump is actually drawn at, in canvas pixels.
    let smallest = clump::SIZE.0 * pixels_per_unit;
    let largest = clump::SIZE.1 * pixels_per_unit;
    let typical = (smallest + largest) * 0.5;

    section.count("grass.stability.clump_pixels_small", smallest as f64, true);
    section.count("grass.stability.clump_pixels_large", largest as f64, true);

    // The mip chain the GPU gets to choose from, and the level it lands on at
    // the typical drawn size.
    let raw_minification = clump::CELL as f32 / typical;
    let level = raw_minification
        .log2()
        .floor()
        .clamp(0.0, (clump::MIP_LEVELS - 1) as f32);
    section.count("grass.stability.mip_levels", clump::MIP_LEVELS as f64, true);

    // Atlas texels crossing one screen pixel *after* the hardware has picked a
    // level. Above one the sprite is minified, and a minified sprite sampled
    // with one bilinear tap resamples a different subset of texels every time
    // it moves — which is aliasing, and looks like sparkle.
    section.ratio(
        "grass.stability.atlas_minification",
        (raw_minification / 2.0f32.powf(level)) as f64,
        false,
    );

    // Share of the sprite sitting within a whisker of the alpha cut. These are
    // the pixels whose existence is decided by a coin flip the moment the
    // sprite moves: a nudge either way pushes them across the discard threshold
    // and a pixel appears or vanishes.
    let mut near_cut = 0u64;
    let mut covered = 0u64;
    for pixel in &atlas.pixels {
        if pixel[3] > 0.02 {
            covered += 1;
        }
        if (pixel[3] - mirror::ALPHA_CUT).abs() < 0.05 {
            near_cut += 1;
        }
    }
    section.ratio(
        "grass.stability.alpha_rim_share",
        near_cut as f64 / covered.max(1) as f64,
        false,
    );

    // The direct test. Eight sub-pixel offsets across one pixel, resampling a
    // spread of variants at the size they ship at — from the mip level the
    // hardware would pick, not from the full-size sheet, because sampling
    // level zero is precisely the thing the chain exists to stop.
    let mut sampled = atlas;
    for _ in 0..level as usize {
        sampled = sampled.downsample();
    }
    let cell = clump::CELL >> level as usize;

    let offsets = 8;
    let target = typical.round().max(4.0) as usize;
    let mut silhouette_toggles = Vec::new();
    let mut interior_churn = Vec::new();
    for variant in (0..clump::VARIANTS).step_by(5) {
        let (toggle, churn) = resample_sweep(&sampled, cell, variant, target, offsets);
        silhouette_toggles.push(toggle);
        interior_churn.push(churn);
    }
    // Fraction of the sprite's silhouette pixels that switch on or off as it
    // slides one eighth of a pixel. This is the crawling edge.
    section.ratio(
        "grass.stability.subpixel_silhouette_toggle",
        harness::mean(&silhouette_toggles),
        false,
    );
    // Mean brightness change of pixels that stay inside the silhouette. This is
    // the sparkle in the middle.
    section.ratio(
        "grass.stability.subpixel_interior_churn",
        harness::mean(&interior_churn),
        false,
    );
}

/// Slide one sprite across a pixel and see what changes.
fn resample_sweep(
    atlas: &clump::Atlas,
    cell: usize,
    variant: usize,
    target: usize,
    offsets: usize,
) -> (f64, f64) {
    let index = variant % clump::VARIANTS;
    let (cell_x, cell_y) = (
        (index % clump::COLUMNS) * cell,
        (index / clump::COLUMNS) * cell,
    );
    let scale = cell as f32 / target as f32;

    let sample = |offset: f32| -> Vec<[f32; 2]> {
        let mut out = Vec::with_capacity(target * target);
        for y in 0..target {
            for x in 0..target {
                // Exactly what the GPU does: one bilinear tap per screen pixel,
                // no filtering across the texels it skips over.
                let u = (x as f32 + 0.5 + offset) * scale - 0.5;
                let v = (y as f32 + 0.5 + offset) * scale - 0.5;
                let (alpha, luma) = bilinear(atlas, cell, cell_x, cell_y, u, v);
                out.push([alpha, luma]);
            }
        }
        out
    };

    let mut toggles = 0u64;
    let mut compared = 0u64;
    let mut churn = 0.0f64;
    let mut interior = 0u64;
    let mut previous = sample(0.0);
    for step in 1..=offsets {
        let now = sample(step as f32 / offsets as f32);
        for (before, after) in previous.iter().zip(&now) {
            let was = before[0] >= mirror::ALPHA_CUT;
            let is = after[0] >= mirror::ALPHA_CUT;
            compared += 1;
            if was != is {
                toggles += 1;
            } else if was {
                churn += (after[1] - before[1]).abs() as f64;
                interior += 1;
            }
        }
        previous = now;
    }

    (
        toggles as f64 / compared.max(1) as f64,
        churn / interior.max(1) as f64,
    )
}

/// One bilinear tap into an atlas cell, returning `(alpha, luminance)`.
fn bilinear(
    atlas: &clump::Atlas,
    cell: usize,
    cell_x: usize,
    cell_y: usize,
    u: f32,
    v: f32,
) -> (f32, f32) {
    let last = cell as f32 - 1.0;
    let u = u.clamp(0.0, last);
    let v = v.clamp(0.0, last);
    let x0 = u.floor() as usize;
    let y0 = v.floor() as usize;
    let x1 = (x0 + 1).min(cell - 1);
    let y1 = (y0 + 1).min(cell - 1);
    let fx = u - x0 as f32;
    let fy = v - y0 as f32;

    let at =
        |x: usize, y: usize| -> [f32; 4] { atlas.pixels[(cell_y + y) * atlas.width + cell_x + x] };
    let mix = |a: [f32; 4], b: [f32; 4], t: f32| -> [f32; 4] {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ]
    };
    let bottom = mix(at(x0, y0), at(x1, y0), fx);
    let top = mix(at(x0, y1), at(x1, y1), fx);
    let value = mix(bottom, top, fy);
    let luma = 0.2126 * value[0] + 0.7152 * value[1] + 0.0722 * value[2];
    (value[3], luma)
}

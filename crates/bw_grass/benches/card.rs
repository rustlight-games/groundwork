//! The card itself: its geometry, what it costs to rasterise, and its tone.
//!
//! Three questions the rest of the suite cannot ask, because each of them lives
//! in the gap between a thing the code says and a thing the picture does.
//!
//! ## Does the bend actually bend?
//!
//! `ClumpSettings::root_stiffness` is documented as the exponent that keeps a
//! plant's base planted while its tip curls over, and for as long as a clump was
//! four vertices it could not possibly have done that. `up` took two values
//! there, zero and one, and `pow(0, k)` and `pow(1, k)` are zero and one for
//! every `k` — so the exponent was applied to the only two inputs on which it is
//! the identity, and the rasteriser filled in a straight line between them. The
//! parameter was inert, the shader's own comment claimed it was not, and no test
//! in the project could tell.
//!
//! [`geometry`] measures it the only way that cannot be fooled: place a clump at
//! full bend, ask where the sprite sits a third of the way up, and compare
//! against the same clump with the exponent forced to one.
//! `grass.card.stiffness_effect` is the difference. It reads exactly zero on a
//! quad and cannot read anything else, whatever the exponent is set to.
//!
//! ## What does a card cost to draw?
//!
//! Nothing in the suite priced a fragment before this. That is a real hole for a
//! field of alpha-tested sprites, because the fragment count is the product of
//! three things that are each individually reasonable — density, card size and
//! how much of a card is actually opaque — and the third is the one nobody
//! looks at. A sprite that covers two fifths of its rectangle throws away three
//! fifths of every fragment it rasterises, and it pays the rasteriser and the
//! texture fetch for all of them before the discard.
//!
//! [`overdraw`] estimates depth complexity from geometry rather than from a
//! capture, so it runs headless. It is a proxy and is named like one, but it is
//! a proxy for the cost that dominates this renderer and it moves for exactly
//! the reasons the real thing would.
//!
//! ## Is the field's tone as wide as the target's?
//!
//! The most specific known gap in the whole look, and the one already written
//! down in `ClumpSettings::tint_floor`: the field spans 1.33 of the art target's
//! ten tones where the target spans 2.41. Right average brightness, half the
//! range.
//!
//! [`tone`] measures the two numbers side by side so the gap is a row in the
//! table rather than a sentence in a doc comment. It measures *between clumps*
//! rather than within them, because a clump is thirty pixels at the battle
//! camera and everything that varies inside one is gone by the time it reaches
//! the eye. What survives is whole plants disagreeing with each other.

use bevy::math::{UVec2, Vec2};
use bw_bench::Report;
use bw_grass::{clump, palette, pixel};
use bw_render::BattleCamera;

use crate::harness::{self, Section};
use crate::mirror;

pub fn run(report: &mut Report) {
    let atlas = clump::bake(&clump::Style::default(), 0x6A72_A551);
    geometry(report);
    overdraw(report, &atlas);
    mips(report, &atlas);
    tone(report, &atlas);
}

/// Canvas pixels per world metre, at the shipped camera and a 1080p window.
fn pixels_per_metre() -> f32 {
    let (_, canvas) = pixel::canvas_geometry(UVec2::new(1920, 1080));
    canvas.y as f32 / BattleCamera::default().view_height
}

// --- geometry ---------------------------------------------------------------

/// Where the sprite goes when the wind blows, up its own height.
///
/// Driven through [`mirror`], which is checked against the WGSL every run, so
/// these numbers describe the shipped shader rather than a second opinion about
/// it.
fn geometry(report: &mut Report) {
    let mut section = Section::new(report, "card");

    section.count("grass.card.rows", mirror::CARD_ROWS as f64, true);
    section.bytes(
        "grass.card.bytes_per_clump",
        mirror::bytes_per_clump(),
        false,
    );

    // One clump, bent as hard as the field can bend it. The interesting numbers
    // are all shape rather than magnitude, so a single plant at full lean says
    // everything a field of them would.
    let clump = mirror::Clump {
        root: Vec2::ZERO,
        width: 0.5,
        height: 1.0,
        shade: 1.0,
        random: 0.0,
    };

    // Measured at full response — a plant giving the wind everything it has.
    // That is the design point, and it is the only bend at which the numbers
    // mean the same thing from one run to the next: driving through a compliance
    // roll instead made every figure depend on what `hash11` returned for one
    // arbitrary clump.
    let profile = mirror::profile(&clump, 1.0);
    let tip = profile.last().copied().unwrap_or(Vec2::ZERO);
    let reach = tip.x.abs().max(1e-6);

    // How much of the lean the bottom third takes.
    //
    // A shear gives exactly the height fraction, because the rasteriser
    // interpolates linearly between a pinned base and a displaced top. That is
    // recorded beside it rather than left implicit, so the table carries what
    // the old card did without needing a baseline run to remember it.
    let sample = profile.len() / 3;
    let height_fraction = sample as f64 / (profile.len() - 1) as f64;
    let third = profile[sample];
    section.ratio("grass.card.shear_lean_share", height_fraction, false);
    section.ratio(
        "grass.card.base_lean_share",
        (third.x.abs() / reach) as f64,
        false,
    );

    // The same measurement against the same card with the exponent forced flat.
    //
    // Zero means the exponent changes nothing, which is the state this benchmark
    // was written to catch and the state it found.
    let linear = mirror::profile_with_exponent(&clump, 1.0, 1.0);
    let drift: f64 = profile
        .iter()
        .zip(&linear)
        .map(|(a, b)| (a.x - b.x).abs() as f64)
        .sum::<f64>()
        / profile.len() as f64;
    section.ratio("grass.card.stiffness_effect", drift / reach as f64, true);

    // Does the silhouette shorten as it leans?
    //
    // Something bending over gets shorter, and a sheared rectangle does not —
    // which is the whole reason there used to be a large ad-hoc squash term to
    // fake it. A card whose centreline is integrated from a tangent shortens on
    // its own, by the cosine, and this is where that shows up.
    let upright = mirror::profile(&clump, 0.0);
    let standing = upright.last().map(|p| p.y).unwrap_or(1.0).max(1e-6);
    section.ratio(
        "grass.card.lean_shortening",
        (1.0 - tip.y / standing) as f64,
        true,
    );
    // What the geometry contributes on its own, with the residual squash term
    // divided back out. A shear contributes exactly nothing here — a sheared
    // rectangle is the same height it started — so every point of this is the
    // centreline doing the job the squash term used to fake.
    let squash = mirror::Settings::shipped_defaults().squash;
    section.ratio(
        "grass.card.cosine_shortening",
        (1.0 - tip.y / (standing * (1.0 - squash))) as f64,
        true,
    );

    // Is the drawn plant still its own length?
    //
    // A rooted plant pivots; it does not stretch, and a card that gains length
    // as it bends is rubber however good the silhouette is. Measured on the bare
    // centreline with the squash divided out, because the squash is a deliberate
    // camera effect and folding it in here would score foreshortening as
    // stretching.
    //
    // Paired with what a shear does at the same reach, which is the thing this
    // replaced: a sheared rectangle's drawn edge is the hypotenuse, so it grows
    // by exactly the amount a bending plant must not.
    let bare = mirror::Settings {
        squash: 0.0,
        ..mirror::Settings::shipped_defaults()
    };
    let straight = mirror::profile_with(&clump, 1.0, &bare, bare.root_stiffness);
    let length = |points: &[Vec2]| {
        let mut total = 0.0f32;
        let mut previous = Vec2::ZERO;
        for point in points {
            total += (*point - previous).length();
            previous = *point;
        }
        total
    };
    let arc = length(&straight);
    section.ratio(
        "grass.card.length_error",
        ((arc - clump.height) / clump.height).abs() as f64,
        false,
    );
    let shear = (reach * reach + clump.height * clump.height).sqrt();
    section.ratio(
        "grass.card.shear_length_error",
        ((shear - clump.height) / clump.height) as f64,
        false,
    );

    section.ratio("grass.card.tip_reach", reach as f64, true);
}

// --- overdraw ---------------------------------------------------------------

/// What a screenful of cards asks the rasteriser for.
///
/// Every number here is derived from the shipped density, the shipped card size
/// and the shipped atlas, so it moves when any of those move and for no other
/// reason. It does not need a GPU and cannot replace one: it counts fragments
/// offered, not fragments executed, and a real capture would show the depth test
/// rejecting some share of them. That share is roughly constant, which is what
/// makes the proxy useful — a change that halves this halves the real thing too.
fn overdraw(report: &mut Report, atlas: &clump::Atlas) {
    let mut section = Section::new(report, "screenful");

    let per_metre = pixels_per_metre();

    // Per-variant occupancy: of the rectangle the card rasterises, how much
    // survives the alpha test.
    let mut occupancies = Vec::with_capacity(clump::VARIANTS);
    let mut trimmed = Vec::with_capacity(clump::VARIANTS);
    for variant in 0..clump::VARIANTS {
        let (column, row) = (variant % clump::COLUMNS, variant / clump::COLUMNS);
        let (x0, y0) = (column * clump::CELL, row * clump::CELL);
        let mut drawn = 0u32;
        let (mut left, mut right, mut top, mut bottom) = (clump::CELL, 0usize, clump::CELL, 0usize);
        for y in 0..clump::CELL {
            for x in 0..clump::CELL {
                let alpha = atlas.pixels[(y0 + y) * atlas.width + x0 + x][3];
                if alpha >= mirror::ALPHA_CUT {
                    drawn += 1;
                    left = left.min(x);
                    right = right.max(x + 1);
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        let cell = (clump::CELL * clump::CELL) as f64;
        occupancies.push(drawn as f64 / cell);
        // The rectangle a tight card would use, as a share of the one it uses.
        let box_area = if right > left && bottom > top {
            ((right - left) * (bottom - top)) as f64
        } else {
            0.0
        };
        trimmed.push(box_area / cell);
    }

    let occupancy = harness::mean(&occupancies);
    let tight = harness::mean(&trimmed);
    section.ratio("grass.overdraw.card_occupancy", occupancy, true);
    // What a trim would remove: rectangle the card covers, minus the rectangle
    // it needs. Pure waste, and the cheapest fragment saving available because
    // it changes nothing about what is drawn.
    section.ratio("grass.overdraw.untouched_border", 1.0 - tight, false);
    // Of the fragments inside even a tight card, how many still discard.
    section.ratio(
        "grass.overdraw.trimmed_occupancy",
        if tight > 1e-9 { occupancy / tight } else { 0.0 },
        true,
    );

    // A screenful, in canvas pixels and world metres.
    let (_, canvas) = pixel::canvas_geometry(UVec2::new(1920, 1080));
    let screen_pixels = (canvas.x as f64) * (canvas.y as f64);
    let view_height = BattleCamera::default().view_height as f64;
    let view_width = view_height * canvas.x as f64 / canvas.y as f64;
    // The isometric projection halves the vertical, so a screen's worth of
    // ground is twice as deep as it is tall.
    let ground = view_width * view_height * 2.0;
    let clumps = ground * clump::PER_SQUARE_METRE as f64;

    // Mean card rectangle, in canvas pixels. Width tracks height through the
    // shipped aspect range, so the mean of the product is not the product of the
    // means and this integrates rather than guesses.
    let mut card_area = 0.0;
    let samples = 64;
    for index in 0..samples {
        let t = index as f64 / (samples - 1) as f64;
        let height = clump::SIZE.0 as f64 + (clump::SIZE.1 - clump::SIZE.0) as f64 * t;
        let width = height * (0.95 + 0.40 * t);
        card_area += width * height;
    }
    card_area = card_area / samples as f64 * (per_metre as f64).powi(2);

    let layers = clumps * card_area / screen_pixels;
    section.ratio("grass.overdraw.layers_per_pixel", layers, false);
    section.ratio("grass.overdraw.shaded_layers", layers * occupancy, false);
    // Fragments rasterised, sampled and thrown away. The one a trim attacks.
    section.ratio(
        "grass.overdraw.discarded_layers",
        layers * (1.0 - occupancy),
        false,
    );
    section.count("grass.overdraw.card_pixels", card_area, false);
    section.count("grass.overdraw.clumps_on_screen", clumps, false);

    // The totals, which are the only numbers here that move with the canvas.
    //
    // Everything above is per-pixel, and per-pixel figures are deliberately
    // scale-invariant: halve the canvas and both the card and the screen shrink
    // by the same factor, so `layers_per_pixel` reads identically. That is the
    // right way to measure the *art* — a field is fifteen cards deep whatever
    // resolution you draw it at — and it is exactly the wrong way to measure the
    // *cost*, which is what a change to the canvas moves.
    //
    // This was a real hole. The canvas dropped from 1080 rows to 540, cutting
    // the fragment count fourfold, and not one measurement in the suite moved.
    // A benchmark that cannot see a fourfold change in the thing it exists to
    // count is worse than no benchmark, because it reports "no change" with the
    // same confidence it reports everything else.
    section.count("grass.overdraw.screen_pixels", screen_pixels, false);
    let rasterised = screen_pixels * layers;
    section.count("grass.overdraw.rasterised_fragments", rasterised, false);
    section.count(
        "grass.overdraw.shaded_fragments",
        rasterised * occupancy,
        false,
    );

    depth_rejection(&mut section, occupancy, rasterised);
}

/// How many of those fragments the depth test kills before they run.
///
/// The one number that says whether the draw order is right, and it cannot be
/// read off the geometry — it depends entirely on what has already been written
/// to the depth buffer when a fragment arrives. So this runs the depth test.
///
/// A real chunk is projected, rasterised into a screen-space buffer in the order
/// the index buffer will present it, and each covered pixel is depth-tested
/// against what is already there. Alpha survival is decided by a hash at the
/// measured card occupancy — unbiased, and repeatable, which a random draw
/// would not be.
///
/// It is a model and it leaves things out: no mip-varying coverage, no partial
/// pixels, one depth per card rather than per fragment. What it captures is the
/// thing that actually decides the cost — whether a fragment arrives before or
/// after the thing that hides it — and that is a property of the order alone.
fn depth_rejection(section: &mut Section, occupancy: f64, rasterised: f64) {
    let field = harness::uniform_field(128);
    let per_metre = pixels_per_metre();
    let batch = clump::build_chunk(&field, bevy::math::IVec2::ZERO, 1.0, 0x6A72_A551);

    // The cards, in the order the mesh presents them.
    let cards: Vec<(Vec2, f32, f32, f32)> = batch
        .roots()
        .zip(batch.shapes())
        .map(|(root, shape)| {
            let screen = bw_grass::iso::project(root.extend(0.0)) * per_metre;
            (
                screen,
                shape[0] * per_metre,
                shape[1] * per_metre,
                root.x + root.y,
            )
        })
        .collect();
    if cards.is_empty() {
        return;
    }

    let (mut low, mut high) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (screen, width, height, _) in &cards {
        low = low.min(*screen - Vec2::new(width * 0.5, 0.0));
        high = high.max(*screen + Vec2::new(width * 0.5, *height));
    }
    let size = (high - low).ceil().max(Vec2::ONE);
    let (w, h) = (size.x as usize + 1, size.y as usize + 1);

    // The shipped order, and the same cards reversed. Both are reported,
    // because "a quarter of fragments are rejected" only means something beside
    // the number the other order would have given — and the other order is what
    // this shipped with for as long as the sprites were blended.
    let shipped = run_depth_test(&cards, low, w, h, occupancy);
    let flipped: Vec<_> = cards.iter().rev().copied().collect();
    let reversed = run_depth_test(&flipped, low, w, h, occupancy);

    section.ratio("grass.overdraw.early_z_rejected", shipped, true);
    section.ratio("grass.overdraw.early_z_other_order", reversed, true);
    section.ratio("grass.overdraw.shader_invocations", 1.0 - shipped, false);
    section.count(
        "grass.overdraw.modelled_fragments",
        cards.len() as f64,
        false,
    );

    // The bottom line: fragments that survive the depth test *and* the alpha
    // test, per frame. Every saving in this section — the draw order, the
    // canvas height, a card trim — has to show up here or it did not happen.
    section.count(
        "grass.overdraw.executed_fragments",
        rasterised * (1.0 - shipped) * occupancy,
        false,
    );
}

/// Rasterise the cards in the given order and return the share depth rejects.
fn run_depth_test(
    cards: &[(Vec2, f32, f32, f32)],
    low: Vec2,
    w: usize,
    h: usize,
    occupancy: f64,
) -> f64 {
    let mut depth = vec![f32::MIN; w * h];
    let (mut rasterised, mut rejected) = (0u64, 0u64);
    for (index, (screen, width, height, near)) in cards.iter().enumerate() {
        let base = *screen - low;
        let x0 = (base.x - width * 0.5).max(0.0) as usize;
        let x1 = ((base.x + width * 0.5) as usize + 1).min(w);
        let y0 = base.y.max(0.0) as usize;
        let y1 = ((base.y + height) as usize + 1).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                rasterised += 1;
                let cell = y * w + x;
                // Depth test first, exactly as the hardware does it: a fragment
                // hidden by something already drawn never reaches the shader,
                // whether or not it would have survived the alpha test.
                if depth[cell] > *near {
                    rejected += 1;
                    continue;
                }
                // Alpha decided by position and by which card this is, so the
                // same card makes the same decision about the same pixel
                // however the draw order shuffles around it.
                let hash = (x as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
                    .wrapping_add((near.to_bits() as u64).wrapping_mul(0x1656_67B1));
                let _ = index;
                let roll = ((hash >> 33) as f64) / (1u64 << 31) as f64;
                if roll < occupancy {
                    depth[cell] = *near;
                }
            }
        }
    }
    rejected as f64 / rasterised.max(1) as f64
}

// --- mip coverage -----------------------------------------------------------

/// Does the sprite keep its shape when it is drawn small?
///
/// Alpha mipmaps have a specific and well known failure: a box filter averages
/// coverage, the alpha test then thresholds the average, and the fraction of
/// texels that survive falls at every level. Thin leaves are the first thing to
/// go, so a clump does not merely soften as it is minified — it *thins*, and a
/// field of them opens up and shows the ground.
///
/// The suite added a mip chain before it measured this, which is the wrong order
/// and is why it is here. `grass.mip.coverage_drift` is the number: how far the
/// coarsest level's surviving share has fallen from the finest level's.
fn mips(report: &mut Report, atlas: &clump::Atlas) {
    let mut section = Section::new(report, "shipped");

    let coverage = |level: &clump::Atlas| -> f64 {
        let above = level
            .pixels
            .iter()
            .filter(|p| p[3] >= mirror::ALPHA_CUT)
            .count();
        above as f64 / level.pixels.len() as f64
    };

    let base = coverage(atlas);
    let mut level = atlas.clone();
    let mut worst = 0.0f64;
    for index in 1..clump::MIP_LEVELS {
        level = level.downsample();
        let here = coverage(&level);
        section.ratio(&format!("grass.mip.coverage_l{index}"), here, true);
        worst = worst.max((here - base).abs() / base.max(1e-9));
    }
    section.ratio("grass.mip.coverage_l0", base, true);
    section.ratio("grass.mip.coverage_drift", worst, false);
}

// --- tone -------------------------------------------------------------------

/// How wide the field's tonal band is, against the art target's.
///
/// Both sides measured in the same unit — the target's own ten tones — so the
/// comparison is a subtraction rather than an argument. `target_spread` is a
/// property of the reference plate and never moves; `clump_spread` is what the
/// renderer produces and is the number to push.
fn tone(report: &mut Report, atlas: &clump::Atlas) {
    let mut section = Section::new(report, "target");

    // The reference's own spread, from its share column. Constant, and here so
    // the table carries the goal next to the measurement.
    let mean: f64 = (0..palette::TARGET_TONES)
        .map(|i| i as f64 * palette::TARGET[i].1 as f64)
        .sum();
    let target_spread: f64 = (0..palette::TARGET_TONES)
        .map(|i| palette::TARGET[i].1 as f64 * (i as f64 - mean).powi(2))
        .sum::<f64>()
        .sqrt();
    section.ratio("grass.tone.target_spread", target_spread, true);

    // Each variant's own brightness, over the pixels that survive the alpha cut,
    // plus the whole histogram of those pixels so the field can be scored per
    // pixel as well as per plant.
    let mut variant_luma = [0.0f32; clump::VARIANTS];
    let mut variant_pixels: Vec<Vec<f32>> = vec![Vec::new(); clump::VARIANTS];
    for variant in 0..clump::VARIANTS {
        let (column, row) = (variant % clump::COLUMNS, variant / clump::COLUMNS);
        let (x0, y0) = (column * clump::CELL, row * clump::CELL);
        let (mut sum, mut count) = (0.0f32, 0u32);
        for y in 0..clump::CELL {
            for x in 0..clump::CELL {
                let pixel = atlas.pixels[(y0 + y) * atlas.width + x0 + x];
                if pixel[3] >= mirror::ALPHA_CUT {
                    let luma = palette::encode_srgb(luminance(pixel));
                    sum += luma;
                    count += 1;
                    // Every ninth, to keep the histogram to a few hundred
                    // thousand samples without biasing it — the stride is
                    // coprime with the cell width, so it sweeps the sprite
                    // rather than sampling one column of it.
                    if (y * clump::CELL + x).is_multiple_of(9) {
                        variant_pixels[variant].push(luma);
                    }
                }
            }
        }
        variant_luma[variant] = if count > 0 { sum / count as f32 } else { 0.0 };
    }

    // A real field's worth of clumps, with the shades the shader gives them.
    let field = harness::uniform_field(128);
    let clumps = mirror::sample(&field, 4, 0x6A72_A551);
    let cells: Vec<usize> = mirror::sample_variants(&field, 4, 0x6A72_A551);

    // Every brightness the palette can actually produce. A rendered tone that
    // does not land on one of these is a colour the art direction never chose.
    let mut rungs = Vec::with_capacity(palette::PALETTE_SIZE);
    for ramp in 0..palette::RAMPS {
        for step in 0..palette::RAMP_STEPS {
            let entry = palette::entry(ramp, step);
            rungs.push(palette::encode_srgb(
                0.2126 * entry.x + 0.7152 * entry.y + 0.0722 * entry.z,
            ));
        }
    }

    let mut tones = Vec::with_capacity(clumps.len());
    let mut shades = Vec::with_capacity(clumps.len());
    let mut pixel_tones: Vec<f64> = Vec::new();
    let mut on_palette = 0usize;
    for (clump, &variant) in clumps.iter().zip(&cells) {
        let shade = clump.shade;
        shades.push(shade);
        let variant = variant.min(clump::VARIANTS - 1);
        let luma = variant_luma[variant] * shade;
        tones.push(palette::target_tone(luma) as f64);
        // Every pixel of every clump, which is the quantity the art target's own
        // 2.41 was measured over — so `field_spread` and `target_spread` are the
        // same measurement of two different pictures, and subtracting them means
        // something. Sampled every ninth clump to keep it to a few million.
        if tones.len() % 9 == 0 {
            for &pixel in &variant_pixels[variant] {
                pixel_tones.push(palette::target_tone(pixel * shade) as f64);
            }
        }
        let nearest = rungs
            .iter()
            .map(|rung| (rung - luma).abs())
            .fold(f32::MAX, f32::min);
        // A rung of the palette's own ramp is about 1/16 of its range apart, so
        // this is a fifth of a step: close enough that the eye reads the
        // rendered colour as the authored one.
        if nearest <= 0.008 {
            on_palette += 1;
        }
    }

    // Between plants — the component that survives to the battle camera, where
    // a clump is thirty pixels and nothing inside one is resolvable.
    section.ratio("grass.tone.clump_spread", harness::deviation(&tones), true);
    // Every pixel, which is what the target's own figure counts. This one is
    // directly comparable to `target_spread`; the one above is not, and is the
    // more interesting of the two anyway.
    let field_spread = harness::deviation(&pixel_tones);
    section.ratio("grass.tone.field_spread", field_spread, true);
    section.ratio(
        "grass.tone.spread_ratio",
        field_spread / target_spread,
        true,
    );

    // How many distinct brightnesses the field actually contains.
    //
    // A continuous multiplier gives one per clump, which is not variety — it is
    // a gradient, and at eight bits a gradient of near-identical greens is what
    // makes hand-drawn art look like a photograph someone posterised badly. The
    // opposite failure is one level, which is a flat field. Somewhere near the
    // palette's own step count is the target.
    let mut levels: Vec<u32> = shades.iter().map(|s| (s * 255.0).round() as u32).collect();
    levels.sort_unstable();
    levels.dedup();
    section.count("grass.tone.shade_levels", levels.len() as f64, false);
    section.ratio(
        "grass.tone.palette_compliance",
        on_palette as f64 / clumps.len().max(1) as f64,
        true,
    );

    // The share landing in each of the target's buckets, scored the way the
    // atlas already scores itself.
    let mut shares = [0.0f32; palette::TARGET_TONES];
    for tone in &tones {
        shares[*tone as usize] += 1.0;
    }
    for share in &mut shares {
        *share /= tones.len().max(1) as f32;
    }
    section.ratio(
        "grass.tone.divergence",
        palette::tone_divergence(&shares) as f64,
        false,
    );
}

fn luminance(pixel: [f32; 4]) -> f32 {
    0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]
}

//! The sprite sheet, judged as artwork.
//!
//! Every scrap of detail in the field was drawn once, into 48 sprites, and then
//! instanced. So the atlas is the artwork — the shader places and tints it, but
//! it cannot add a leaf. That makes this the cheapest place in the whole system
//! to catch a look going wrong: no GPU, no window, no frame of simulation, just
//! a pure function of [`clump::Style`] and a seed.
//!
//! Four families of question, in the order they matter.
//!
//! **Is the silhouette clean?** At the size these ship — twenty to forty pixels
//! — the outline is most of what a viewer reads. Isolated pixels, one-pixel
//! spurs and speckled holes are what separate a drawn plant from a resized
//! photograph of one.
//!
//! **Is the edge decidable?** The fragment shader discards below
//! [`mirror::ALPHA_CUT`], so a soft edge is not soft on screen — it is a hard
//! edge in a position decided by whichever side of the threshold each pixel
//! happens to land on. The softer the bake, the more pixels are near the
//! threshold, and the more of the outline changes when the sprite moves a
//! fraction of a pixel. `soft_rim_share` prices that, and
//! `grass.stability.subpixel_silhouette_toggle` is the same fact measured as
//! motion.
//!
//! **Is the colour on target?** Two independent things, and passing one says
//! nothing about the other: whether the colours used are the target's colours
//! (`off_target_share`), and whether they appear in the target's *proportions*
//! (`tone_divergence`). A sprite can sit exactly on the reference palette and
//! still read far too bright, because its shading spends most of its pixels at
//! the top of the ramp.
//!
//! **Are 48 variants actually 48?** `variant_diversity` is the guard against
//! the failure that no correctness test can see: every sprite valid, and all of
//! them the same plant.

use bw_bench::Report;
use bw_grass::clump;
use bw_grass::palette;

use crate::harness::{self, Section};
use crate::mirror;

/// How far a colour may sit from a palette entry and still count as on-palette,
/// in sRGB units summed across three channels.
///
/// Loose enough to absorb the eight-bit quantisation and the premultiply
/// round-trip, tight enough that a colour off the ramp shows up.
const COLOUR_TOLERANCE: f32 = 0.06;

pub fn run(report: &mut Report) {
    let atlas = clump::bake(&clump::Style::default(), 0x6A72_A551);
    silhouette(report, &atlas);
    colour(report, &atlas);
    variety(report, &atlas);
}

/// Whether a pixel is part of the drawn shape.
fn drawn(atlas: &clump::Atlas, x: usize, y: usize) -> bool {
    atlas.pixels[y * atlas.width + x][3] >= mirror::ALPHA_CUT
}

// --- silhouette -------------------------------------------------------------

fn silhouette(report: &mut Report, atlas: &clump::Atlas) {
    let mut section = Section::new(report, "atlas");

    let mut above = 0u64;
    let mut soft = 0u64;
    let mut any = 0u64;
    for pixel in &atlas.pixels {
        let alpha = pixel[3];
        if alpha > 0.02 {
            any += 1;
        }
        if alpha >= mirror::ALPHA_CUT {
            above += 1;
        } else if alpha > 0.02 {
            soft += 1;
        }
    }

    // The share of a cell the plant actually occupies once the discard has run.
    // A clump that fills its cell has nothing to overlap into and reads as a
    // block; one that barely marks it is a wisp.
    section.ratio(
        "grass.atlas.silhouette_share",
        above as f64 / atlas.pixels.len() as f64,
        true,
    );
    // Coverage that was baked and then thrown away. This is wasted work in the
    // bake, but far more importantly it is the population of pixels whose
    // presence on screen is decided by sub-pixel luck.
    section.ratio(
        "grass.atlas.soft_rim_share",
        soft as f64 / any.max(1) as f64,
        false,
    );

    // Connected-component analysis of the drawn shape. A plant should be a few
    // connected masses; a cloud of specks is what an automatic conversion
    // produces and what a person never draws.
    let labels = components(atlas);
    let mut sizes = std::collections::BTreeMap::new();
    for &label in &labels {
        if label > 0 {
            *sizes.entry(label).or_insert(0u64) += 1;
        }
    }
    let areas: Vec<f64> = sizes.values().map(|&v| v as f64).collect();
    let tiny: f64 = areas.iter().filter(|&&a| a < 8.0).sum();

    section.count("grass.atlas.clusters", areas.len() as f64, false);
    section.count("grass.atlas.mean_cluster_size", harness::mean(&areas), true);
    // Pixels living in clusters too small to read as anything. Every one is a
    // speck that appears and disappears as the sprite moves.
    section.ratio(
        "grass.atlas.tiny_cluster_share",
        tiny / above.max(1) as f64,
        false,
    );

    // Pixels drawn with no drawn neighbour at all: confetti, in the plainest
    // possible form.
    let mut isolated = 0u64;
    let mut spurs = 0u64;
    let mut boundary = 0u64;
    for y in 1..atlas.height - 1 {
        for x in 1..atlas.width - 1 {
            if !drawn(atlas, x, y) {
                continue;
            }
            let neighbours = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .filter(|(dx, dy)| drawn(atlas, (x as i32 + dx) as usize, (y as i32 + dy) as usize))
                .count();
            if neighbours == 0 {
                isolated += 1;
            }
            if neighbours < 4 {
                boundary += 1;
            }
            // A one-pixel protrusion. Some are wanted — a leaf tip is exactly
            // this — so the interesting form of the number is its share of the
            // outline rather than its raw count, and it belongs beside
            // `silhouette_churn`, which says whether they hold still.
            if neighbours <= 1 {
                spurs += 1;
            }
        }
    }
    section.ratio(
        "grass.atlas.isolated_share",
        isolated as f64 / above.max(1) as f64,
        false,
    );
    section.ratio(
        "grass.atlas.spur_share",
        spurs as f64 / boundary.max(1) as f64,
        false,
    );

    // How many pixels the alpha takes to go from nothing to solid, per pixel of
    // outline. This is `Style::softness` measured on the output rather than
    // read off the input, which matters because the leaf-drawing code
    // accumulates coverage from overlapping strokes and the result is not the
    // parameter.
    section.ratio(
        "grass.atlas.edge_width",
        soft as f64 / boundary.max(1) as f64,
        false,
    );

    // Holes: undrawn pixels fully surrounded by drawn ones. A few are gaps
    // between leaves and are correct; many is a sprite eaten by noise.
    let mut holes = 0u64;
    for y in 1..atlas.height - 1 {
        for x in 1..atlas.width - 1 {
            if drawn(atlas, x, y) {
                continue;
            }
            let ringed = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .all(|(dx, dy)| drawn(atlas, (x as i32 + dx) as usize, (y as i32 + dy) as usize));
            if ringed {
                holes += 1;
            }
        }
    }
    section.ratio(
        "grass.atlas.hole_share",
        holes as f64 / above.max(1) as f64,
        false,
    );
}

/// Label the drawn pixels into 4-connected components, per cell.
///
/// Per cell rather than across the whole sheet, because two variants that touch
/// at a cell boundary are not one plant — they are two, and merging them would
/// report the atlas as far better connected than it is.
fn components(atlas: &clump::Atlas) -> Vec<u32> {
    let mut labels = vec![0u32; atlas.pixels.len()];
    let mut next = 1u32;
    let mut stack = Vec::new();

    for variant in 0..clump::VARIANTS {
        let (origin_x, origin_y) = clump::Atlas::cell(variant);
        for y in 0..clump::CELL {
            for x in 0..clump::CELL {
                let (px, py) = (origin_x + x, origin_y + y);
                let index = py * atlas.width + px;
                if labels[index] != 0 || !drawn(atlas, px, py) {
                    continue;
                }
                let label = next;
                next += 1;
                labels[index] = label;
                stack.push((px, py));
                while let Some((cx, cy)) = stack.pop() {
                    for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                        let nx = cx as i32 + dx;
                        let ny = cy as i32 + dy;
                        // Clamped to the cell, not to the sheet.
                        if nx < origin_x as i32
                            || ny < origin_y as i32
                            || nx >= (origin_x + clump::CELL) as i32
                            || ny >= (origin_y + clump::CELL) as i32
                        {
                            continue;
                        }
                        let (nx, ny) = (nx as usize, ny as usize);
                        let neighbour = ny * atlas.width + nx;
                        if labels[neighbour] == 0 && drawn(atlas, nx, ny) {
                            labels[neighbour] = label;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
        }
    }
    labels
}

// --- colour -----------------------------------------------------------------

fn colour(report: &mut Report, atlas: &clump::Atlas) {
    let mut section = Section::new(report, "atlas");

    // Un-premultiplied, so a half-covered edge pixel is scored as the green it
    // is rather than as the darker green coverage made of it.
    let mut colours: Vec<[f32; 3]> = Vec::new();
    let mut lumas: Vec<f64> = Vec::new();
    for pixel in &atlas.pixels {
        let alpha = pixel[3];
        if alpha < mirror::ALPHA_CUT {
            continue;
        }
        let colour = [pixel[0] / alpha, pixel[1] / alpha, pixel[2] / alpha];
        lumas.push((0.2126 * colour[0] + 0.7152 * colour[1] + 0.0722 * colour[2]) as f64);
        colours.push(colour);
    }
    if colours.is_empty() {
        return;
    }

    // Distinct colours at eight bits. The count a pixel artist would quote, and
    // a number that should stay small — the whole point of a fixed palette is
    // that a sprite is built from a handful of decided tones.
    let mut distinct = std::collections::BTreeSet::new();
    for colour in &colours {
        distinct.insert([
            (colour[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (colour[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (colour[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        ]);
    }
    section.count("grass.atlas.colour_count", distinct.len() as f64, false);

    // Off the palette entirely. This should be zero: the bake indexes the
    // palette directly, so anything here is a colour arriving from somewhere
    // else — a blend, an interpolation, a stray constant.
    let entries: Vec<[f32; 3]> = (0..palette::RAMPS)
        .flat_map(|ramp| {
            (0..palette::RAMP_STEPS).map(move |step| {
                let [r, g, b] = palette::channels(ramp, step);
                [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
            })
        })
        .collect();
    let off = |colour: &[f32; 3], against: &[[f32; 3]]| -> bool {
        against
            .iter()
            .map(|entry| (0..3).map(|c| (colour[c] - entry[c]).abs()).sum::<f32>())
            .fold(f32::MAX, f32::min)
            > COLOUR_TOLERANCE
    };
    section.ratio(
        "grass.atlas.off_palette_share",
        colours.iter().filter(|c| off(c, &entries)).count() as f64 / colours.len() as f64,
        false,
    );

    // Off the *art target*, which is a different and much harder question. The
    // palette is fitted to the target but not equal to it, and this says how
    // much of what actually gets drawn lands on a colour the reference uses.
    let target: Vec<[f32; 3]> = palette::TARGET
        .iter()
        .map(|([r, g, b], _)| [*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0])
        .collect();
    section.ratio(
        "grass.atlas.off_target_share",
        colours.iter().filter(|c| off(c, &target)).count() as f64 / colours.len() as f64,
        false,
    );

    // The other half of matching a reference: not which colours, but how much
    // of each. Zero is an exact match to the target's tone distribution.
    section.ratio(
        "grass.atlas.tone_divergence",
        palette::tone_divergence(&atlas.tone_shares()) as f64,
        false,
    );

    // Reach. A sprite whose brightest and darkest are close together reads as
    // one flat material however much shape is in it, and against the reference
    // this is the first thing to go.
    let spread = harness::percentile(&lumas, 0.95) - harness::percentile(&lumas, 0.05);
    section.ratio("grass.atlas.luminance_spread", spread, true);
    let (target_low, target_high) = palette::target_range();
    section.ratio(
        "grass.atlas.luminance_reach",
        harness::ratio_similarity(spread, (target_high - target_low) as f64),
        true,
    );

    // Internal relief. The references are not flat blobs of green — every clump
    // has legible leaves inside its outline, and this is the number that moves
    // when they stop being legible.
    let mut contrast = 0.0f64;
    let mut counted = 0u64;
    for y in 0..atlas.height - 1 {
        for x in 0..atlas.width - 1 {
            if !drawn(atlas, x, y) || !drawn(atlas, x + 1, y) || !drawn(atlas, x, y + 1) {
                continue;
            }
            let luma = |x: usize, y: usize| {
                let p = atlas.pixels[y * atlas.width + x];
                let alpha = p[3].max(1e-4);
                (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) / alpha
            };
            let here = luma(x, y);
            contrast += ((here - luma(x + 1, y)).abs() + (here - luma(x, y + 1)).abs()) as f64;
            counted += 2;
        }
    }
    section.ratio(
        "grass.atlas.interior_contrast",
        contrast / counted.max(1) as f64,
        true,
    );
}

// --- variety ----------------------------------------------------------------

fn variety(report: &mut Report, atlas: &clump::Atlas) {
    let mut section = Section::new(report, "atlas");

    // Each variant's silhouette as a bitmask, so overlap is a cheap AND.
    let masks: Vec<Vec<bool>> = (0..clump::VARIANTS)
        .map(|variant| {
            let (origin_x, origin_y) = clump::Atlas::cell(variant);
            let mut mask = Vec::with_capacity(clump::CELL * clump::CELL);
            for y in 0..clump::CELL {
                for x in 0..clump::CELL {
                    mask.push(drawn(atlas, origin_x + x, origin_y + y));
                }
            }
            mask
        })
        .collect();

    // One minus the mean pairwise overlap. Near zero means 48 copies of one
    // plant, which is invisible to every correctness test — each sprite is
    // perfectly valid — and glaringly obvious on screen the moment a field of
    // them tiles.
    let mut overlaps = Vec::new();
    for a in 0..masks.len() {
        for b in a + 1..masks.len() {
            let (mut both, mut either) = (0u64, 0u64);
            for (left, right) in masks[a].iter().zip(&masks[b]) {
                if *left && *right {
                    both += 1;
                }
                if *left || *right {
                    either += 1;
                }
            }
            overlaps.push(both as f64 / either.max(1) as f64);
        }
    }
    section.ratio(
        "grass.atlas.variant_diversity",
        1.0 - harness::mean(&overlaps),
        true,
    );

    // Variety of size as well as of shape. Two silhouettes can overlap little
    // and still be the same plant at two rotations; differing bulk is the other
    // axis, and the cheaper one to lose.
    let areas: Vec<f64> = masks
        .iter()
        .map(|mask| mask.iter().filter(|&&on| on).count() as f64)
        .collect();
    section.ratio(
        "grass.atlas.variant_size_spread",
        harness::variation(&areas),
        true,
    );

    // Do the leaves fan upward, or radiate? The reference art fans up out of a
    // root; a clump that opens the whole way round is a starburst, and a field
    // of starbursts reads as stamped. Measured as how far the drawn mass sits
    // above the middle of its cell.
    let mut lift = Vec::new();
    for mask in &masks {
        let mut sum = 0.0f64;
        let mut count = 0.0f64;
        for (index, &on) in mask.iter().enumerate() {
            if on {
                // Rows run top-down in the atlas, so a small row index is high
                // up the plant.
                sum += 1.0 - (index / clump::CELL) as f64 / (clump::CELL - 1) as f64;
                count += 1.0;
            }
        }
        if count > 0.0 {
            lift.push(sum / count);
        }
    }
    section.ratio("grass.atlas.upward_bias", harness::mean(&lift), true);
}

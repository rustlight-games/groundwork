//! Pre-baked grass clumps.
//!
//! A clump is a small plant — a dozen or so leaves fanning up out of one root —
//! drawn **once** into a sprite and then instanced across the field. That is the
//! architectural difference from drawing every blade as geometry every frame,
//! and it buys two things that pull in the same direction.
//!
//! **Speed.** A field is tens of thousands of clumps rather than hundreds of
//! thousands of ribbons, and the per-frame cost stops scaling with how much
//! detail each plant has.
//!
//! **Detail.** A baked clump can carry as much internal structure as the
//! rasteriser cares to draw: overlapping leaves, soft edges, a shaded interior,
//! bright tips. Geometry has to pay for every one of those per frame, so it
//! never gets them. This is why the reference art looks painted and a
//! ribbon-per-blade renderer looks like fur.
//!
//! ## Style lives in [`Style`]
//!
//! Every decision about how a plant looks is a field on one struct, and the
//! atlas is a pure function of it plus a seed. Comparing two looks is comparing
//! two `Style` values, and [`Atlas::write_png`] dumps the result without a GPU,
//! a window or a frame of simulation — which is the whole point while the art
//! direction is still moving.
//!
//! ## Soft edges, on purpose
//!
//! Coverage is accumulated per pixel rather than tested, so a leaf edge lands
//! between colours. That is the opposite of the hard-edged pixel discipline the
//! blades use, and it is deliberate: the references are painted, and a painted
//! edge is the single clearest difference between the two looks.

use rayon::prelude::*;

use crate::noise::unit_from_hash;
use crate::palette;

/// Pixels along each edge of one clump cell.
pub const CELL: usize = 64;

/// Levels in the atlas's mip chain, counting the full-size one.
///
/// See `atlas_image` for why there is a chain at all.
pub const MIP_LEVELS: usize = 3;

/// Clump variants baked into the atlas.
///
/// Enough that a screenful never shows an obvious repeat, few enough that the
/// atlas stays small. Variety within a variant comes from the placement side —
/// scale, mirroring and tint — so this is the count of distinct *silhouettes*,
/// which is the thing the eye actually latches onto.
pub const VARIANTS: usize = 48;

/// Cells across the atlas.
pub const COLUMNS: usize = 6;

/// Cells down the atlas.
pub const ROWS: usize = VARIANTS.div_ceil(COLUMNS);

/// How a clump is drawn.
///
/// One value describes one look. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// Leaves in a clump, fewest to most.
    pub leaves: (usize, usize),
    /// Leaf length as a fraction of the cell, shortest to longest.
    pub length: (f32, f32),
    /// Leaf half-width at its base, in pixels.
    pub width: (f32, f32),
    /// Half-angle the fan opens to, in radians, measured from straight up.
    ///
    /// The references fan *upward*, not radially. A clump that opens the whole
    /// way round is a starburst, and a field of starbursts reads as stamped.
    pub fan: f32,
    /// How far a leaf curves over along its length, in radians.
    pub curve: (f32, f32),
    /// Ramp step a leaf starts at, as a fraction of the ramp.
    pub root_shade: f32,
    /// Ramp step a leaf tip reaches.
    pub tip_shade: f32,
    /// Exponent shaping how a leaf travels from [`Style::root_shade`] to
    /// [`Style::tip_shade`].
    ///
    /// One is linear. Above one the leaf stays dark for most of its length and
    /// brightens late, which is both what a shaded canopy does and what the art
    /// target's share column asks for — its three darkest greens are 39% of the
    /// reference and its brightest is 2%. A linear sweep cannot produce that
    /// distribution from any pair of endpoints, because it spends equal length
    /// on every tone by construction.
    pub shade_curve: f32,
    /// Fraction of leaves drawn on the shadow ramp.
    pub shadow_share: f32,
    /// Fraction of leaves drawn on the highlight ramp.
    ///
    /// The rest take the body ramp. Together with [`Style::shade_curve`] these
    /// are what [`crate::palette::tone_divergence`] is fitted against — the
    /// palette settles which greens exist, this settles how much of each is
    /// seen, and the two are entirely independent.
    pub highlight_share: f32,
    /// Softness of a leaf edge, in pixels.
    ///
    /// The painterly half of the look. At zero the sprite is hard-edged and
    /// reads as pixel art; around a pixel it reads as painted.
    pub softness: f32,
    /// How much of the fan's spread the leaves lean, rather than splaying
    /// symmetrically. Gives a clump a direction.
    pub sway: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            leaves: (95, 160),
            length: (0.38, 0.95),
            width: (0.75, 1.55),
            fan: 0.62,
            curve: (0.15, 0.85),
            root_shade: 0.0,
            tip_shade: 0.70,
            shade_curve: 2.60,
            shadow_share: 0.579,
            highlight_share: 0.220,
            softness: 0.9,
            sway: 0.35,
        }
    }
}

/// A baked sheet of clump sprites, RGBA with straight (un-premultiplied) alpha.
pub struct Atlas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 4]>,
}

impl Atlas {
    /// Where a variant sits, in pixels.
    pub fn cell(index: usize) -> (usize, usize) {
        let index = index % VARIANTS;
        ((index % COLUMNS) * CELL, (index / COLUMNS) * CELL)
    }

    /// The atlas as eight-bit RGBA, ready for upload or for a PNG.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 4);
        for pixel in &self.pixels {
            for channel in pixel {
                out.push((channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        out
    }

    /// A half-size copy, for the mip chain.
    ///
    /// Two details make this correct rather than merely smaller.
    ///
    /// **Filtered premultiplied.** Averaging straight colour across an edge
    /// weights a barely-covered pixel as heavily as a solid one, which drags
    /// the average toward whatever colour happens to sit in the transparent
    /// gaps. Weighting by coverage and dividing back out afterwards is the
    /// whole reason premultiplied alpha exists.
    ///
    /// **Filtered in linear light.** The stored values are sRGB, and the mean
    /// of two sRGB numbers is not the sRGB of their mean — averaging them
    /// directly darkens every edge in the sheet, which at this sprite size is
    /// most of the sheet.
    ///
    /// Block-aligned, so it never mixes one variant's cell into its neighbour's:
    /// [`CELL`] is a power of two, so every level's cell boundary lands on a
    /// block boundary.
    pub fn downsample(&self) -> Atlas {
        let width = (self.width / 2).max(1);
        let height = (self.height / 2).max(1);
        let mut pixels = vec![[0.0f32; 4]; width * height];

        for y in 0..height {
            for x in 0..width {
                let mut sum = [0.0f32; 4];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let source = self.pixels[(y * 2 + dy) * self.width + x * 2 + dx];
                        let alpha = source[3];
                        for channel in 0..3 {
                            sum[channel] += palette::decode_srgb(source[channel]) * alpha;
                        }
                        sum[3] += alpha;
                    }
                }
                let alpha = sum[3] * 0.25;
                let mut out = [0.0f32; 4];
                if sum[3] > 1e-6 {
                    for channel in 0..3 {
                        out[channel] = palette::encode_srgb(sum[channel] / sum[3]);
                    }
                }
                out[3] = alpha;
                pixels[y * width + x] = out;
            }
        }

        Atlas {
            width,
            height,
            pixels,
        }
    }

    /// Fraction of the atlas that any leaf covers.
    ///
    /// A clump that fills its cell has nothing to overlap into and reads as a
    /// block; one that barely marks it is a wisp. Worth measuring because it is
    /// the first thing to drift when the style changes.
    pub fn coverage(&self) -> f32 {
        let covered = self.pixels.iter().filter(|p| p[3] > 0.02).count();
        covered as f32 / self.pixels.len().max(1) as f32
    }

    /// Share of the sprite's visible pixels falling in each of the art target's
    /// ten tones, darkest first.
    ///
    /// This is the other half of matching a reference palette, and the half that
    /// is easy to skip. [`crate::palette`] settles what colours exist; this
    /// settles *how much of each one you see*, and the two are independent — a
    /// palette can sit exactly on the target and still produce an image that
    /// reads far too bright, because the shading spends most of its pixels at
    /// the top of the ramp. The reference is 39% dark greens and 2% brightest
    /// highlight, and nothing in the palette bake knows that.
    ///
    /// Measured on pixels above [`ALPHA_CUT`], because that is the silhouette
    /// the fragment shader actually keeps; the soft rim below it is discarded
    /// and counting it would score tones that never reach the screen.
    ///
    /// Two things it deliberately does not account for. Overlap between clumps
    /// on screen resolves per fragment against the depth buffer, so which sprite
    /// wins is placement, not shading — and one clump's tone distribution is the
    /// same whichever clump is behind it. Per-clump tint darkens by up to a
    /// seventh, which shifts the whole distribution down by roughly one bucket's
    /// width; that is a bias, not noise, and the tolerance carries it.
    pub fn tone_shares(&self) -> [f32; palette::TARGET_TONES] {
        let mut counts = [0.0f32; palette::TARGET_TONES];
        let mut total = 0.0f32;
        for pixel in &self.pixels {
            let alpha = pixel[3];
            if alpha < ALPHA_CUT {
                continue;
            }
            let luma = 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2];
            counts[palette::target_tone(luma)] += 1.0;
            total += 1.0;
        }
        if total > 0.0 {
            for count in &mut counts {
                *count /= total;
            }
        }
        counts
    }
}

/// Bake the atlas.
///
/// Deterministic in `seed` and `style`, which is what makes the look
/// reproducible and the benchmark meaningful.
pub fn bake(style: &Style, seed: u32) -> Atlas {
    let width = COLUMNS * CELL;
    let height = ROWS * CELL;
    let mut atlas = Atlas {
        width,
        height,
        pixels: vec![[0.0; 4]; width * height],
    };

    // Each variant is drawn into its own cell-sized buffer and then blitted in.
    //
    // Threaded because the bake is 30 milliseconds on the startup path and
    // forty-eight variants are forty-eight independent drawings; going through
    // private buffers rather than straight into the sheet is what makes them
    // *provably* independent to the compiler, and costs one copy of 16 KB per
    // variant. `stamp` already clamps to the cell, so a variant drawn at the
    // origin of its own buffer is identical to one drawn at its offset in the
    // sheet — the atlas is unchanged, byte for byte.
    let cells: Vec<Atlas> = (0..VARIANTS)
        .into_par_iter()
        .map(|variant| {
            let mut cell = Atlas {
                width: CELL,
                height: CELL,
                pixels: vec![[0.0; 4]; CELL * CELL],
            };
            draw_clump(
                &mut cell,
                0,
                0,
                style,
                seed ^ (variant as u32).wrapping_mul(0x9E37_79B9),
            );
            cell
        })
        .collect();

    for (variant, cell) in cells.iter().enumerate() {
        let (x0, y0) = Atlas::cell(variant);
        for y in 0..CELL {
            let source = y * CELL;
            let target = (y0 + y) * width + x0;
            atlas.pixels[target..target + CELL]
                .copy_from_slice(&cell.pixels[source..source + CELL]);
        }
    }

    // Composited premultiplied, stored straight.
    //
    // [`stamp`] has to accumulate premultiplied — that is what makes "a later
    // leaf covers an earlier one" a running weighted sum rather than a stack of
    // blends. But the texture this becomes is `Rgba8UnormSrgb` and is read by a
    // shader that alpha-*clips*: it uses the alpha to decide whether the
    // fragment exists and then writes the colour opaquely, never dividing it
    // back out.
    //
    // Leaving it premultiplied is therefore not a slightly-wrong blend, it is a
    // fragment drawn at `alpha` times its own brightness — and because the store
    // is sRGB the hardware linearises the *product*, which makes it `alpha^2.2`
    // in linear light. A fringe pixel at the clip threshold came out six times
    // too dark. With leaves under two pixels wide and nearly a pixel of edge
    // softness, most of a sprite is fringe, so the whole field was dragged dark
    // and grainy and lost its highlights entirely.
    for pixel in &mut atlas.pixels {
        if pixel[3] > 1e-4 {
            let inverse = 1.0 / pixel[3];
            pixel[0] *= inverse;
            pixel[1] *= inverse;
            pixel[2] *= inverse;
        }
    }
    atlas
}

fn draw_clump(atlas: &mut Atlas, x0: usize, y0: usize, style: &Style, seed: u32) {
    let span = (style.leaves.1 - style.leaves.0 + 1) as f32;
    let count = style.leaves.0 + ((unit_from_hash(seed) * span) as usize).min(span as usize - 1);

    // Which way this clump leans as a whole. A field of clumps that all lean
    // the same way reads as combed; all upright reads as printed.
    let lean = (unit_from_hash(seed.wrapping_mul(0xc2b2_ae35)) - 0.5) * 2.0 * style.sway;

    // The root sits on the bottom edge, centred: the sprite is a plant standing
    // on the ground, and the quad it is drawn on has its base at the ground.
    let root_x = CELL as f32 * 0.5;
    let root_y = CELL as f32 - 1.5;

    for leaf in 0..count {
        let h = seed
            .wrapping_mul(0x9e37_79b9)
            .wrapping_add((leaf as u32).wrapping_mul(0x85eb_ca6b) ^ 0x1656_67b1);
        let a = unit_from_hash(h);
        let b = unit_from_hash(h.wrapping_mul(0xc2b2_ae35));
        let c = unit_from_hash(h.wrapping_mul(0x27d4_eb2f));
        let d = unit_from_hash(h.wrapping_mul(0x1656_67b1));

        // Spread across the fan rather than at random: random angles leave gaps
        // and doubled-up leaves, and a gap makes one clump read as two.
        let across = if count > 1 {
            leaf as f32 / (count - 1) as f32 - 0.5
        } else {
            0.0
        };
        let angle = (across * 2.0 + (a - 0.5) * 0.45) * style.fan + lean;

        // Squared, so most leaves are short and a few stand proud. An even
        // spread gives a clump a flat crown.
        let length = lerp(style.length.0, style.length.1, b * b) * CELL as f32;
        let width = lerp(style.width.0, style.width.1, c);
        // Outer leaves curve over hardest, which is what gives a clump its
        // splayed silhouette instead of a bundle of spikes.
        let curve = lerp(
            style.curve.0,
            style.curve.1,
            (across.abs() * 1.6 + d * 0.5).min(1.0),
        );
        let curve = curve * if across < 0.0 { -1.0 } else { 1.0 };

        draw_leaf(
            atlas,
            x0,
            y0,
            &Leaf {
                root: (root_x, root_y),
                angle,
                length,
                width,
                curve,
            },
            style,
            h,
        );
    }
}

/// Steps taken along a leaf's centreline.
///
/// Enough that consecutive stamps overlap at the widest leaf, so the edge comes
/// out continuous rather than beaded.
const STEPS: usize = 40;

/// One leaf's shape, gathered so the rasteriser takes a subject rather than a
/// list of loose numbers.
struct Leaf {
    root: (f32, f32),
    angle: f32,
    length: f32,
    width: f32,
    curve: f32,
}

fn draw_leaf(atlas: &mut Atlas, x0: usize, y0: usize, leaf: &Leaf, style: &Style, hash: u32) {
    let (root_x, root_y) = leaf.root;
    let (angle, length, width, curve) = (leaf.angle, leaf.length, leaf.width, leaf.curve);
    // Which ramp this leaf sits on. Mostly the body; a minority run light or
    // dark, which is what stops a clump reading as one flat colour.
    let pick = unit_from_hash(hash.wrapping_mul(0x7feb_352d));
    let ramp = if pick < style.shadow_share {
        palette::SHADOW
    } else if pick > 1.0 - style.highlight_share {
        palette::HIGHLIGHT
    } else {
        palette::BODY
    };

    let mut x = root_x;
    let mut y = root_y;
    for step in 0..STEPS {
        let t = step as f32 / (STEPS - 1) as f32;
        // The leaf bends progressively, so it is straight at the root and
        // curls at the tip — a leaf hinged at its base reads as a stick.
        let bend = angle + curve * t * t;
        let advance = length / STEPS as f32;
        x += bend.sin() * advance;
        y -= bend.cos() * advance;

        // Near parallel-sided then drawn to a point, which is the shape of a
        // real grass leaf. Tapering from the base gives needles.
        let taper = (1.0 - smoothstep(0.55, 1.0, t)).max(0.0);
        let radius = width * (0.35 + 0.65 * taper);
        if radius <= 0.05 {
            continue;
        }

        let shade = lerp(style.root_shade, style.tip_shade, t.powf(style.shade_curve));
        let step_index =
            ((shade * palette::RAMP_STEPS as f32) as usize).min(palette::RAMP_STEPS - 1);
        let [r, g, b] = palette::channels(ramp, step_index);
        let colour = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];

        stamp(
            atlas,
            x0,
            y0,
            &Dab {
                at: (x, y),
                radius,
                softness: style.softness,
                colour,
            },
        );
    }
}

/// One soft disc of colour.
struct Dab {
    at: (f32, f32),
    radius: f32,
    softness: f32,
    colour: [f32; 3],
}

/// Lay a dab into a cell.
fn stamp(atlas: &mut Atlas, x0: usize, y0: usize, dab: &Dab) {
    let (cx, cy) = dab.at;
    let (radius, softness, colour) = (dab.radius, dab.softness, dab.colour);
    let reach = radius + softness;
    let min_x = (cx - reach).floor().max(0.0) as usize;
    let max_x = (cx + reach).ceil().min(CELL as f32 - 1.0) as usize;
    let min_y = (cy - reach).floor().max(0.0) as usize;
    let max_y = (cy + reach).ceil().min(CELL as f32 - 1.0) as usize;

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let distance = (dx * dx + dy * dy).sqrt();
            // Coverage falls off across `softness`, which is what makes the
            // edge painted rather than cut.
            let coverage = 1.0 - smoothstep(radius - softness, radius + softness, distance);
            if coverage <= 0.004 {
                continue;
            }

            let index = (y0 + py) * atlas.width + x0 + px;
            let pixel = &mut atlas.pixels[index];
            // Painter's algorithm: a later leaf covers an earlier one rather
            // than blending with it, so overlapping leaves read as separate
            // leaves instead of as a wash.
            let keep = 1.0 - coverage;
            pixel[0] = pixel[0] * keep + colour[0] * coverage;
            pixel[1] = pixel[1] * keep + colour[1] * coverage;
            pixel[2] = pixel[2] * keep + colour[2] * coverage;
            pixel[3] = (pixel[3] * keep + coverage).min(1.0);
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 - edge0 <= 1e-6 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p bw_grass --lib -- --ignored --nocapture show_the_atlas`
    ///
    /// Writes the baked sheet to `benchmarks/capture/clump_atlas.png`. Not an
    /// assertion — it is the fast loop. Changing a `Style` field and looking at
    /// the result takes seconds and needs no GPU, no window and no simulation,
    /// which is what makes comparing two looks practical while the art
    /// direction is still moving.
    #[test]
    #[ignore = "writes the clump atlas for inspection"]
    fn show_the_atlas() {
        let style = Style::default();
        let atlas = bake(&style, 0x6A72_A551);
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../benchmarks/capture/clump_atlas.png"
        );
        let _ = std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap());
        image::save_buffer(
            path,
            &atlas.to_rgba8(),
            atlas.width as u32,
            atlas.height as u32,
            image::ColorType::Rgba8,
        )
        .expect("the atlas must be writable");
        println!("wrote {path}");
        println!("coverage {:.3}", atlas.coverage());
        println!();
        let shares = atlas.tone_shares();
        println!("      tone     ours   target");
        for (index, (colour, target)) in palette::TARGET.iter().enumerate() {
            let [r, g, b] = colour;
            println!(
                "  #{r:02x}{g:02x}{b:02x}   {:6.1}%  {:6.1}%",
                shares[index] * 100.0,
                target * 100.0
            );
        }
        println!("  divergence {:.3}", palette::tone_divergence(&shares));
    }

    /// `cargo test -p bw_grass --lib -- --ignored --nocapture fit_the_tones`
    ///
    /// Searches the shading half of [`Style`] against the art target's share
    /// column and prints the result, which is then pasted into
    /// [`Style::default`]. The colour half of the same job lives in
    /// `palette::tests::fit_to_the_target`; this is the half that decides how
    /// much of each colour is actually seen.
    ///
    /// Only the five shading knobs move. Leaf count, length, width, fan, curve
    /// and softness are silhouette, and a search allowed to touch them would
    /// happily reach the right histogram by growing spindlier plants — the
    /// distribution is a proxy for the look, and a proxy optimised against
    /// directly stops being one.
    #[test]
    #[ignore = "searches the clump shading against the art target"]
    fn fit_the_tones() {
        let write = |v: &[f32]| Style {
            root_shade: v[0],
            tip_shade: v[1],
            shade_curve: v[2],
            shadow_share: v[3],
            highlight_share: v[4],
            ..Style::default()
        };
        let bound = |index: usize| match index {
            0 | 1 => (0.0f32, 1.0f32),
            2 => (0.4, 4.0),
            _ => (0.0, 0.85),
        };
        let scale = |index: usize| match index {
            2 => 0.08,
            _ => 0.02,
        };
        let loss = |v: &[f32]| {
            // A tip darker than the root is not a shading choice, it is a bug.
            if v[1] <= v[0] {
                return f32::MAX;
            }
            // The body ramp keeps a fifth of the leaves. Left free the search
            // takes it to nothing — the palette's three living ramps all lie on
            // the target's single curve and differ mainly in *range*, so the
            // widest-reaching pair can always cover the histogram on their own.
            // That scores better and looks worse: within one clump the ramps are
            // the only hue variation there is, and two of them is a plant made
            // of a dark green and a light green with nothing in between.
            if v[3] + v[4] > 0.80 {
                return f32::MAX;
            }
            // Averaged over seeds, like every other measurement here. One seed's
            // atlas is 48 clumps and its histogram wobbles by a percent or two.
            let mut total = 0.0;
            for seed in [0x6A72_A551u32, 0x0000_0001, 0x5EED_1234] {
                let shares = bake(&write(v), seed).tone_shares();
                total += palette::tone_divergence(&shares);
            }
            total / 3.0
        };

        let style = Style::default();
        let mut best = vec![
            style.root_shade,
            style.tip_shade,
            style.shade_curve,
            style.shadow_share,
            style.highlight_share,
        ];
        let mut best_loss = loss(&best);
        let start = best_loss;
        let mut step = 1.0f32;
        while step > 0.05 {
            let mut improved = false;
            for index in 0..best.len() {
                for direction in [1.0f32, -1.0] {
                    let (low, high) = bound(index);
                    let mut candidate = best.clone();
                    candidate[index] =
                        (candidate[index] + direction * step * scale(index)).clamp(low, high);
                    let value = loss(&candidate);
                    if value < best_loss - 1e-5 {
                        best = candidate;
                        best_loss = value;
                        improved = true;
                    }
                }
            }
            if !improved {
                step *= 0.5;
            }
        }

        println!("divergence {start:.3} -> {best_loss:.3}");
        println!("            root_shade: {:.3},", best[0]);
        println!("            tip_shade: {:.3},", best[1]);
        println!("            shade_curve: {:.3},", best[2]);
        println!("            shadow_share: {:.3},", best[3]);
        println!("            highlight_share: {:.3},", best[4]);
    }

    #[test]
    fn the_atlas_tones_match_the_art_target() {
        // The guard on the fit above. It is loose against the fitted 0.16
        // because two things sit between this atlas and the screen — per-clump
        // tint darkens by up to a seventh, and the ground wash fills the gaps —
        // so being exactly on the target here is not the goal. What it catches
        // is the failure the eye is worst at: a shading change that quietly
        // moves the whole field a shade brighter or a shade darker while every
        // colour in it stays perfectly on palette.
        for seed in [0x6A72_A551u32, 0x0000_0001, 0x5EED_1234] {
            let shares = bake(&Style::default(), seed).tone_shares();
            let divergence = palette::tone_divergence(&shares);
            assert!(
                divergence < 0.25,
                "seed {seed:#x} is off the target's tone distribution: {divergence:.3} {shares:?}"
            );
        }
    }

    #[test]
    fn the_clump_shading_reaches_the_dark_end_of_the_target() {
        // Called out separately because it is a *reachability* failure, not a
        // distribution one, and total variation is a poor detector of it: an
        // atlas that never produces the target's two darkest greens at all was
        // only scoring 0.24 on divergence while missing a quarter of the
        // reference outright. That was the state before `root_shade` came down
        // to zero — every leaf started two steps up its ramp, so the darkest
        // pixel in the whole sheet was the target's *third* tone.
        let shares = bake(&Style::default(), 0x6A72_A551).tone_shares();
        assert!(
            shares[0] > 0.02 && shares[1] > 0.04,
            "the canopy has no deep shade in it: {shares:?}"
        );
    }

    #[test]
    fn the_ramp_shares_leave_room_for_the_body_ramp() {
        // The two shares are thresholds on one value and `draw_leaf` tests them
        // in order, so a pair that sums past one does not fail — it silently
        // starves the ramp in between. The fit found exactly that and scored
        // well on it.
        let style = Style::default();
        assert!(
            style.shadow_share + style.highlight_share < 0.9,
            "the body ramp has been squeezed out"
        );
    }

    #[test]
    fn the_atlas_stores_straight_alpha() {
        // The sprite is composited premultiplied and stored straight, and the
        // shader alpha-*clips* rather than blending — it reads the colour and
        // writes it opaquely, so a premultiplied store is drawn at `alpha` times
        // its own brightness. Worse, the texture is sRGB, so the hardware
        // linearises the product and the error compounds to `alpha^2.2`.
        //
        // It is invisible in every other measurement. Coverage, silhouette,
        // determinism and the tone histogram are all computed from this module's
        // own buffer, where the two conventions are one divide apart; only the
        // GPU ever saw the difference, as a field dragged dark and grainy with
        // its highlights gone.
        //
        // So: a half-covered pixel must be about as bright as a fully covered
        // one. Not identical — fringes sit on leaf edges and edges skew a little
        // darker for real reasons — but nowhere near half.
        let atlas = bake(&Style::default(), 0x6A72_A551);
        let mean = |low: f32, high: f32| {
            let mut total = 0.0f32;
            let mut count = 0.0f32;
            for pixel in &atlas.pixels {
                if pixel[3] >= low && pixel[3] < high {
                    total += 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2];
                    count += 1.0;
                }
            }
            total / count.max(1.0)
        };
        let fringe = mean(ALPHA_CUT, 0.60);
        let core = mean(0.95, 1.01);
        assert!(
            fringe > core * 0.75,
            "the atlas is still premultiplied: fringe {fringe:.3} against core {core:.3}"
        );
    }

    #[test]
    fn the_atlas_is_the_size_its_layout_implies() {
        let atlas = bake(&Style::default(), 1);
        assert_eq!(atlas.width, COLUMNS * CELL);
        assert_eq!(atlas.height, ROWS * CELL);
        assert_eq!(atlas.pixels.len(), atlas.width * atlas.height);
    }

    #[test]
    fn every_variant_draws_something() {
        // A blank cell would show as a hole in the field, and with placement
        // picking variants at random it would be an intermittent one.
        let atlas = bake(&Style::default(), 7);
        for variant in 0..VARIANTS {
            let (x0, y0) = Atlas::cell(variant);
            let covered = (0..CELL)
                .flat_map(|y| (0..CELL).map(move |x| (x, y)))
                .filter(|(x, y)| atlas.pixels[(y0 + y) * atlas.width + x0 + x][3] > 0.05)
                .count();
            assert!(
                covered > CELL * 4,
                "variant {variant} covers only {covered} pixels"
            );
        }
    }

    #[test]
    fn variants_differ_from_one_another() {
        // The whole reason for baking more than one.
        let atlas = bake(&Style::default(), 3);
        let alpha = |variant: usize| {
            let (x0, y0) = Atlas::cell(variant);
            (0..CELL)
                .flat_map(|y| (0..CELL).map(move |x| (x, y)))
                .map(|(x, y)| atlas.pixels[(y0 + y) * atlas.width + x0 + x][3])
                .collect::<Vec<f32>>()
        };
        let first = alpha(0);
        for variant in 1..VARIANTS {
            let other = alpha(variant);
            let difference: f32 = first
                .iter()
                .zip(&other)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / first.len() as f32;
            assert!(difference > 0.01, "variant {variant} matches variant 0");
        }
    }

    #[test]
    fn clumps_are_rooted_at_the_bottom_of_their_cell() {
        // The sprite is a plant standing on the ground. If the mass drifted up
        // the cell it would hover; if it drifted down it would be cut off.
        let atlas = bake(&Style::default(), 5);
        let (x0, y0) = Atlas::cell(0);
        let row_alpha = |y: usize| {
            (0..CELL)
                .map(|x| atlas.pixels[(y0 + y) * atlas.width + x0 + x][3])
                .sum::<f32>()
        };
        // The bottom half carries the root mass.
        let lower: f32 = (CELL / 2..CELL).map(row_alpha).sum();
        let upper: f32 = (0..CELL / 2).map(row_alpha).sum();
        assert!(lower > upper, "clump is top-heavy: {lower} vs {upper}");
    }

    #[test]
    fn leaves_fan_upward_rather_than_radially() {
        // The references fan up. A clump that opens the whole way round is a
        // starburst, and a field of starbursts reads as stamped decals — this
        // is the single most recognisable tell of procedural grass.
        assert!(Style::default().fan < std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn edges_are_soft() {
        // The painterly half of the look. Hard edges here would put us back at
        // pixel art, which is a different style from the references.
        let style = Style::default();
        assert!(style.softness > 0.3);

        // And the softness reaches the image: some pixels sit part-covered.
        let atlas = bake(&style, 11);
        let partial = atlas
            .pixels
            .iter()
            .filter(|p| p[3] > 0.08 && p[3] < 0.92)
            .count();
        assert!(partial > 400, "only {partial} soft pixels — edges are cut");
    }

    #[test]
    fn coverage_leaves_room_to_overlap() {
        // A clump filling its cell reads as a block; one barely marking it is a
        // wisp. Neither is a plant.
        let coverage = bake(&Style::default(), 13).coverage();
        assert!(
            (0.12..0.62).contains(&coverage),
            "coverage {coverage} is outside the useful band"
        );
    }

    #[test]
    fn the_bake_is_reproducible() {
        // It feeds a committed baseline and, later, a cached texture.
        let a = bake(&Style::default(), 17);
        let b = bake(&Style::default(), 17);
        assert_eq!(a.to_rgba8(), b.to_rgba8());
    }

    #[test]
    fn style_changes_the_result() {
        // Guards the one property that makes this a comparison tool: the atlas
        // is a function of the style, so two styles must not bake the same.
        let wide = Style {
            fan: 1.1,
            ..Style::default()
        };
        assert_ne!(
            bake(&Style::default(), 19).to_rgba8(),
            bake(&wide, 19).to_rgba8()
        );
    }

    #[test]
    fn nothing_is_drawn_outside_its_own_cell() {
        // Bleed between cells would show as a fragment of a neighbouring plant
        // hanging off every sprite.
        let atlas = bake(&Style::default(), 23);
        for variant in 0..VARIANTS {
            let (x0, y0) = Atlas::cell(variant);
            for y in 0..CELL {
                for x in 0..CELL {
                    let index = (y0 + y) * atlas.width + x0 + x;
                    assert!(atlas.pixels[index][3] <= 1.0 + 1e-6);
                }
            }
        }
        // And the seams between cells stay clear on at least one side, which is
        // what stamping clamped to the cell guarantees.
        let (x0, y0) = Atlas::cell(1);
        let left_edge: f32 = (0..CELL)
            .map(|y| atlas.pixels[(y0 + y) * atlas.width + x0][3])
            .sum();
        assert!(
            left_edge < CELL as f32 * 0.5,
            "cell 1 bleeds at its left edge"
        );
    }
}

// --- placement ---------------------------------------------------------------

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{
    Indices, Mesh, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues, VertexFormat,
};
use bevy::prelude::*;

use crate::field::GrassField;
use crate::iso;
use crate::noise::{fbm, hash_2d};

/// Clumps per square metre at full detail.
///
/// Set by fill rate, not by taste. These are blended sprites about a metre and
/// a half across, so each one covers roughly three thousand pixels, and blended
/// geometry cannot skip a fragment the way opaque geometry can — every overlap
/// is paid for in full.
///
/// At eleven per square metre a screenful was some sixty layers deep and the
/// frame rate showed it. At three the field is still comfortably opaque,
/// because a clump's footprint is about a square metre and three of them
/// overlapping is plenty to hide the ground.
///
/// If more density is ever wanted, the answer is not this number: it is fewer,
/// larger sprites, or an opaque pass for the dense interior with blending kept
/// for the silhouette.
pub const PER_SQUARE_METRE: f32 = 19.5;

/// World height of a clump sprite, shortest to tallest, in metres.
///
/// Fourteen to twenty-nine pixels at the battle camera's thirty-four pixels to
/// the metre. Large enough that a clump is a legible plant, small enough that a
/// screenful is a field of them rather than a dozen shrubs — at twice this they
/// read as bushes and the ground stops having a scale.
///
/// The tall end came down from 1.30. A clump that stands a metre and a third is
/// waist-high on a unit, and grass that reaches a soldier's waist is a set
/// dressing the battle has to be read *through* rather than fought on. It also
/// stacks: sprites this tall reach far up the screen from roots well behind the
/// pixel they cover, so the canopy piles into a wall on the far side of the
/// field rather than a surface.
/// Smaller and denser than it was, which is one decision rather than two.
///
/// These two constants only mean anything together: shrinking a plant without
/// growing the count thins the canopy until the ground shows through, and
/// growing the count without shrinking the plant piles bushes on top of each
/// other. Here the linear size drops by a quarter — so a sprite covers a bit
/// over half the ground it did — and the count rises by three quarters, which
/// leaves the canopy about as closed as before while making the individual
/// plant a smaller unit of the picture.
///
/// The cost is real and lands on the mesh rather than the simulation: the field
/// solver does not know how many clumps are drawn over it, so a step is
/// unchanged, but chunk building and vertex memory scale directly with the
/// count. `grass.build.scene_clumps` and `grass.build.scene_mesh_bytes` are
/// where that shows up.
pub const SIZE: (f32, f32) = (0.36, 0.76);

/// Metres per cycle of the field that varies clump size and tint.
pub const VARIATION_METRES: f32 = 9.0;

/// Strata a clump may be jittered across.
///
/// Greater than one, which is unusual and is the point. Stratified points
/// jittered *within* their own cell still sit on a lattice — every point is
/// somewhere in a known square — and while that is invisible for scattered
/// blades it is very visible here, because the isometric depth sort runs along
/// X+Y and turns any residual lattice into diagonal seams through the overlaps.
///
/// Over-jittering lets neighbours swap cells, which destroys the lattice
/// outright. The cost is occasional clustering, and clumps are supposed to
/// clump.
const JITTER: f32 = 1.8;

/// Metres per cycle of the field that varies how many clumps grow.
///
/// Separate from the size-and-tint drift: density and appearance vary
/// independently in a real meadow, and driving both from one map is what makes
/// procedural ground look like a mask applied twice.
pub const DENSITY_METRES: f32 = 6.5;

/// How far that field swings the count, in 0..1.
const DENSITY_SWING: f32 = 0.45;

/// Fraction of candidate clumps drawn at full detail.
///
/// Every candidate carries a stable random rank and is drawn when its rank
/// falls below this. The stability is the whole point: lowering the fraction
/// removes clumps without moving the ones that remain, so a density change is
/// a *subset* of the denser field rather than an unrelated field. Thinning by
/// re-rolling instead makes every plant jump the moment the camera moves.
pub const DENSITY: f32 = 1.0;

/// World position of the clump's root, shared by all four corners.
pub const ATTRIBUTE_ROOT: MeshVertexAttribute =
    MeshVertexAttribute::new("ClumpRoot", 0x6a72_0021, VertexFormat::Float32x2);

/// `(corner x, corner y, atlas column, atlas row)`.
pub const ATTRIBUTE_CORNER: MeshVertexAttribute =
    MeshVertexAttribute::new("ClumpCorner", 0x6a72_0022, VertexFormat::Float32x4);

/// `(width, height, tint, per-clump random)`.
pub const ATTRIBUTE_SHAPE: MeshVertexAttribute =
    MeshVertexAttribute::new("ClumpShape", 0x6a72_0023, VertexFormat::Float32x4);

/// A chunk's worth of clumps.
pub struct Batch {
    positions: Vec<[f32; 3]>,
    roots: Vec<[f32; 2]>,
    corners: Vec<[f32; 4]>,
    shapes: Vec<[f32; 4]>,
    indices: Vec<u32>,
    clumps: u32,
}

impl Batch {
    pub fn clumps(&self) -> u32 {
        self.clumps
    }

    pub fn is_empty(&self) -> bool {
        self.clumps == 0
    }

    /// Bytes of vertex and index data.
    pub fn byte_size(&self) -> usize {
        self.positions.len() * 12
            + self.roots.len() * 8
            + self.corners.len() * 16
            + self.shapes.len() * 16
            + self.indices.len() * 4
    }

    /// One root per clump.
    pub fn roots(&self) -> impl Iterator<Item = Vec2> + '_ {
        self.roots.iter().step_by(4).map(|&r| Vec2::from(r))
    }

    /// One world height per clump, in metres.
    pub fn heights(&self) -> impl Iterator<Item = f32> + '_ {
        self.shapes.iter().step_by(4).map(|s| s[1])
    }

    /// One `(width, height, tint, random)` per clump, exactly as the vertex
    /// shader receives it.
    ///
    /// `random` is the interesting one: it seeds the per-clump stiffness, which
    /// is what decides how much of the field's bend a plant takes. Any analysis
    /// that wants to reproduce what a clump will *do* — as opposed to where it
    /// sits — needs it.
    pub fn shapes(&self) -> impl Iterator<Item = [f32; 4]> + '_ {
        self.shapes.iter().copied().step_by(4)
    }

    /// One `(column, row)` atlas cell per clump.
    pub fn cells(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.corners
            .iter()
            .step_by(4)
            .map(|c| (c[2] as usize, c[3] as usize))
    }

    pub fn into_mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(ATTRIBUTE_ROOT, self.roots)
        .with_inserted_attribute(
            ATTRIBUTE_CORNER,
            VertexAttributeValues::Float32x4(self.corners),
        )
        .with_inserted_attribute(
            ATTRIBUTE_SHAPE,
            VertexAttributeValues::Float32x4(self.shapes),
        )
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// Build the clumps for one chunk.
///
/// `chunk` is a chunk coordinate covering `CHUNK_METRES` square. `detail`
/// scales the count for level of detail.
pub fn build_chunk(field: &GrassField, chunk: IVec2, detail: f32, seed: u32) -> Batch {
    let side = crate::blade::CHUNK_METRES;
    let origin = chunk.as_vec2() * side;
    let target = (side * side * PER_SQUARE_METRE * detail.clamp(0.0, 1.0))
        .round()
        .max(0.0) as usize;

    let mut batch = Batch {
        positions: Vec::with_capacity(target * 4),
        roots: Vec::with_capacity(target * 4),
        corners: Vec::with_capacity(target * 4),
        shapes: Vec::with_capacity(target * 4),
        indices: Vec::with_capacity(target * 6),
        clumps: 0,
    };
    if target == 0 {
        return batch;
    }

    let mut placed: Vec<Placed> = Vec::with_capacity(target);
    let strata = (target as f32).sqrt().ceil().max(1.0) as i32;
    let stride = side / strata as f32;
    let chunk_seed = seed ^ hash_2d(chunk.x, chunk.y, 0x51A5_5EED);

    for sy in 0..strata {
        for sx in 0..strata {
            let hash = hash_2d(sx, sy, chunk_seed);
            // Jittered hard. Clumps are meant to pile up, so the stratification
            // is only here to stop them leaving holes — the remaining lattice
            // disappears under the overlap.
            let jitter = Vec2::new(unit(hash), unit(hash.wrapping_mul(0x9e37_79b9)));
            let jitter = Vec2::splat(0.5) + (jitter - Vec2::splat(0.5)) * JITTER;
            let row = if sy % 2 == 0 { 0.0 } else { 0.5 };
            let root = origin + (Vec2::new(sx as f32 + row, sy as f32) + jitter) * stride;

            // Two gates: the simulation's own density map, and a Perlin field
            // of this layer's own that thins and thickens the planting.
            let thickness = 1.0 - DENSITY_SWING
                + DENSITY_SWING
                    * 2.0
                    * fbm(
                        root.x / DENSITY_METRES,
                        root.y / DENSITY_METRES,
                        chunk_seed ^ 0x4D65_1F0B,
                        3,
                    );
            // The stable rank. Drawn from the clump's own hash and never
            // re-rolled, so it means the same thing at every density.
            let rank = unit(hash.wrapping_mul(0x85eb_ca6b));
            if rank > DENSITY {
                continue;
            }
            if rank > field.density_at_world(root) * thickness {
                continue;
            }

            // Size and colour drift across the field together, so a region
            // reads as taller and greener rather than as random plants.
            let drift = fbm(
                root.x / VARIATION_METRES,
                root.y / VARIATION_METRES,
                chunk_seed ^ 0x2B7E_1516,
                3,
            );
            let a = unit(hash.wrapping_mul(0xc2b2_ae35));
            let height = lerp(SIZE.0, SIZE.1, (a * 0.55 + drift * 0.45).clamp(0.0, 1.0));
            // Held at the atlas cell's own proportions rather than widened to
            // recover the footprint [`SIZE`] gave up. Widening was tried and is
            // a trap: the cell is square and holds an upright plant, so a wider,
            // shorter quad does not draw a squatter tuft, it draws the same tuft
            // smeared sideways. A screenful of those is a fine even noise with
            // no plants in it at all.
            let width = height * lerp(0.95, 1.35, unit(hash.wrapping_mul(0x27d4_eb2f)));

            let variant = (hash.wrapping_mul(0x1656_67b1) as usize) % VARIANTS;
            let column = (variant % COLUMNS) as f32;
            let row_index = (variant / COLUMNS) as f32;
            let tint = (unit(hash.wrapping_mul(0x7feb_352d)) * 0.6 + drift * 0.4).clamp(0.0, 1.0);
            let random = unit(hash.wrapping_mul(0x846c_a68b));

            placed.push(Placed {
                root,
                width,
                height,
                column,
                row: row_index,
                tint,
                random,
            });
        }
    }

    // Far to near, along the isometric depth axis.
    //
    // Blended sprites have no depth test to fall back on, so they composite in
    // the order they are drawn — and the order they were *placed* in is the
    // scan order of the stratification grid, which runs along X and Y. Those
    // project to diagonals, so a near clump drawn before a far one leaves a
    // seam, and every such seam lines up into a visible isometric lattice
    // across the whole field. Sorting is the fix, and it is free: it happens
    // once when the chunk is built, not per frame.
    placed.sort_by(|a, b| {
        let depth = |p: &Placed| p.root.x + p.root.y;
        depth(a)
            .partial_cmp(&depth(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for clump in &placed {
        push(&mut batch, clump);
    }

    batch
}

/// A clump decided on but not yet written into the mesh.
struct Placed {
    root: Vec2,
    width: f32,
    height: f32,
    column: f32,
    row: f32,
    tint: f32,
    random: f32,
}

fn push(batch: &mut Batch, clump: &Placed) {
    let (root, width, height) = (clump.root, clump.width, clump.height);
    let (column, row, tint, random) = (clump.column, clump.row, clump.tint, clump.random);
    let base = batch.positions.len() as u32;
    // A rest-pose bounding position per corner. The vertex shader recomputes
    // the real one, so this exists only to give Bevy an honest bounding box in
    // the space the shader outputs.
    let rest = iso::project_to_vertex(root.extend(height));

    // (-1, 0) and (1, 1) are the bottom-left and top-right of the sprite, with
    // the root on the bottom edge, centred.
    for corner in [[-1.0, 0.0], [1.0, 0.0], [1.0, 1.0], [-1.0, 1.0]] {
        batch.positions.push(rest.to_array());
        batch.roots.push(root.to_array());
        batch.corners.push([corner[0], corner[1], column, row]);
        batch.shapes.push([width, height, tint, random]);
    }
    batch
        .indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    batch.clumps += 1;
}

fn unit(hash: u32) -> f32 {
    crate::noise::unit_from_hash(hash)
}

// --- material ----------------------------------------------------------------

use bevy::image::{ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDataOrder, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dKey};

/// Coverage below which a clump fragment is discarded.
///
/// Mirrored in `clump.wgsl`. Around a half rather than near zero: the point of
/// clipping is a clean silhouette the depth buffer can sort, and a low
/// threshold keeps the soft rim that made sorting necessary in the first place.
pub const ALPHA_CUT: f32 = 0.45;

/// Where the clump shader lives.
pub const SHADER_PATH: &str = "shaders/clump.wgsl";

/// Per-frame constants shared by every clump.
///
/// Field order matches `clump.wgsl`; the twelve scalars after the two vectors
/// round the header to a whole number of sixteen-byte rows.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct ClumpSettings {
    pub field_origin: Vec2,
    pub field_inverse_extent: Vec2,
    pub field_resolution: f32,
    pub time: f32,
    pub max_angle: f32,
    /// How far the top of a clump slides, in sprite heights, at full bend.
    pub lean: f32,
    /// How much a fully leaned clump loses in height.
    ///
    /// Paired with the lean on purpose. A sheared sprite whose silhouette never
    /// shortens is the classic way grass ends up looking like rubber; something
    /// bending over genuinely does get shorter.
    pub squash: f32,
    /// Amplitude of the per-clump idle sway, in sprite heights.
    ///
    /// Zero, and it should stay there. A per-clump sine looked like the obvious
    /// way to keep a still field alive and it is the single change that made
    /// the grass read as a water surface: smooth, continuous, everywhere at
    /// once. Grass at rest is still. Liveliness belongs to the wind field,
    /// which already gusts.
    pub sway: f32,
    /// How far tint shifts a clump's colour.
    pub tint_strength: f32,
    /// Shade multiplier at the darkest tint.
    ///
    /// This pair is the field's only source of *large-scale* tone. Everything
    /// else varies within a sprite, and a sprite is thirty pixels: shade one
    /// leaf differently from its neighbour and the difference is gone by the
    /// time it reaches the eye. What survives at viewing distance is whole
    /// clumps disagreeing with each other.
    ///
    /// Together they currently put every clump within eight percent of every
    /// other, and that is measurably too narrow: scored against the art target's
    /// share column with `palette::tests::score_the_capture`, the rendered field
    /// spans 1.33 tones against the target's 2.41. Right average brightness, in
    /// a band half as wide as it should be. Widening it is the obvious next
    /// move and is deliberately not made here — it is a visible change to the
    /// look, and this pass was colour.
    pub tint_floor: f32,
    /// Exponent on how far up a sprite the lean is applied.
    ///
    /// One is a linear shear, and linear is wrong: it puts half the lean at
    /// half the height, so the sprite slides as a unit and the plant reads as
    /// moving across the ground rather than bending out of it. Grass is stiff
    /// near the ground, so the exponent pushes the motion into the top.
    pub root_stiffness: f32,
    pub _pad1: f32,
}

impl Default for ClumpSettings {
    fn default() -> Self {
        Self {
            field_origin: Vec2::ZERO,
            field_inverse_extent: Vec2::ONE,
            field_resolution: 1.0,
            time: 0.0,
            max_angle: 84.0_f32.to_radians(),
            lean: 0.30,
            squash: 0.22,
            sway: 0.0,
            tint_strength: 0.55,
            tint_floor: 0.86,
            root_stiffness: 2.6,
            _pad1: 0.0,
        }
    }
}

/// The material every clump draws with.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct ClumpMaterial {
    #[uniform(0)]
    pub settings: ClumpSettings,
    #[texture(1)]
    #[sampler(2)]
    pub atlas: Handle<Image>,
    #[texture(3, sample_type = "float", filterable = false)]
    pub bend: Handle<Image>,
}

impl Material2d for ClumpMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    /// Clipped, not blended — and that choice fixes two separate problems.
    ///
    /// Blending was the obvious reading of "the sprites have soft edges", and
    /// it cost more than the softness was worth:
    ///
    /// - **Ordering.** Blended fragments composite in draw order, so overlap
    ///   had to be sorted, and any residual order error lined up along the
    ///   isometric depth axis into a visible lattice. Clipped grass writes
    ///   depth, so the hardware sorts per fragment and the lattice becomes
    ///   impossible rather than something to be sorted around.
    /// - **Fill rate.** Blending pays for every overlapped fragment. Clipping
    ///   lets the depth test reject them, which is what makes a dense field
    ///   affordable at all.
    ///
    /// The cost is a hard silhouette. That is much less of a loss than it
    /// sounds: the atlas still carries all its interior shading, and what gets
    /// cut is only the transparent rim.
    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Mask(ALPHA_CUT)
    }

    fn specialize(
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            ATTRIBUTE_ROOT.at_shader_location(1),
            ATTRIBUTE_CORNER.at_shader_location(2),
            ATTRIBUTE_SHAPE.at_shader_location(3),
        ])?];
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// The baked atlas as a GPU image.
#[derive(Resource, Debug, Clone)]
pub struct ClumpAtlas {
    pub image: Handle<Image>,
}

impl FromWorld for ClumpAtlas {
    fn from_world(world: &mut World) -> Self {
        let atlas = bake(&Style::default(), 0x6A72_A551);
        let mut images = world.resource_mut::<Assets<Image>>();
        Self {
            image: images.add(atlas_image(&atlas)),
        }
    }
}

fn atlas_image(atlas: &Atlas) -> Image {
    // The mip chain, and it is not an optimisation — it is the fix for a
    // visible defect.
    //
    // A cell is baked at 64 pixels and a clump is drawn at about 31, so every
    // screen pixel covers roughly four atlas texels. A linear sampler with no
    // mips takes *one* bilinear tap out of those four, so which texels a pixel
    // is made of changes the moment the sprite moves a fraction of a pixel —
    // and a fraction of a pixel is all a leaning plant ever moves. The result
    // is a sprite whose interior sparkles while its shape sits still, which is
    // the other half of what "the grass flickers" meant.
    //
    // Three levels is enough to cover the whole size range a clump is drawn at
    // (roughly 20 to 42 pixels) and stops well before a cell is small enough
    // for filtering to bleed one variant into the next.
    let mut levels = vec![atlas.to_rgba8()];
    let mut current = atlas.downsample();
    for _ in 1..MIP_LEVELS {
        levels.push(current.to_rgba8());
        current = current.downsample();
    }
    let data: Vec<u8> = levels.concat();

    Image {
        data: Some(data),
        data_order: TextureDataOrder::default(),
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d {
                width: atlas.width as u32,
                height: atlas.height as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: MIP_LEVELS as u32,
            sample_count: 1,
            dimension: TextureDimension::D2,
            // The sprite holds sRGB colour, so the hardware should linearise it
            // on read the same way it does for every other colour texture.
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        // Linear, unlike everything else in this crate. A clump's edges are
        // soft by design and nearest sampling would step them back into the
        // hard silhouette baking them was meant to avoid.
        sampler: ImageSampler::Descriptor(ImageSamplerDescriptor {
            mipmap_filter: bevy::image::ImageFilterMode::Linear,
            // Never coarser than the chain actually holds. Past that the
            // hardware would clamp anyway, but saying so keeps the intent
            // visible next to the level count it depends on.
            lod_max_clamp: (MIP_LEVELS - 1) as f32,
            ..ImageSamplerDescriptor::linear()
        }),
        texture_view_descriptor: None,
        asset_usage: RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        copy_on_resize: false,
    }
}

/// Keep the clump material's view of the field current.
pub fn upload_clumps(
    field: Res<GrassField>,
    wind: Res<crate::wind::WindField>,
    mut materials: ResMut<Assets<ClumpMaterial>>,
) {
    let extent = field.extent().max(1e-3);
    for (_, material) in materials.iter_mut() {
        material.settings.field_origin = field.origin();
        material.settings.field_inverse_extent = Vec2::splat(1.0 / extent);
        material.settings.field_resolution = field.resolution() as f32;
        material.settings.time = wind.time;
        material.settings.max_angle = field.params().max_angle;
    }
}

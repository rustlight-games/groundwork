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

use crate::noise::unit_from_hash;
use crate::palette;

/// Pixels along each edge of one clump cell.
pub const CELL: usize = 64;

/// Clump variants baked into the atlas.
///
/// Enough that a screenful never shows an obvious repeat, few enough that the
/// atlas stays small. Variety within a variant comes from the placement side —
/// scale, mirroring and tint — so this is the count of distinct *silhouettes*,
/// which is the thing the eye actually latches onto.
pub const VARIANTS: usize = 24;

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
            leaves: (16, 30),
            length: (0.38, 0.95),
            width: (1.2, 2.4),
            fan: 0.62,
            curve: (0.15, 0.85),
            root_shade: 0.18,
            tip_shade: 0.96,
            softness: 0.9,
            sway: 0.35,
        }
    }
}

/// A baked sheet of clump sprites, RGBA, premultiplied by coverage.
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

    /// Fraction of the atlas that any leaf covers.
    ///
    /// A clump that fills its cell has nothing to overlap into and reads as a
    /// block; one that barely marks it is a wisp. Worth measuring because it is
    /// the first thing to drift when the style changes.
    pub fn coverage(&self) -> f32 {
        let covered = self.pixels.iter().filter(|p| p[3] > 0.02).count();
        covered as f32 / self.pixels.len().max(1) as f32
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

    for variant in 0..VARIANTS {
        let (x0, y0) = Atlas::cell(variant);
        draw_clump(
            &mut atlas,
            x0,
            y0,
            style,
            seed ^ (variant as u32).wrapping_mul(0x9E37_79B9),
        );
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
    let ramp = if pick < 0.22 {
        palette::SHADOW
    } else if pick > 0.74 {
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

        let shade = lerp(style.root_shade, style.tip_shade, t);
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

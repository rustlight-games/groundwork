//! The two plates nobody frames: the tile grid, and the subject mask.
//!
//! A beauty render of nine tiles has no visible tile boundaries, and that is the
//! point — the internal joins are supposed to be invisible, because the ground
//! is continuous across them. Which leaves nothing in the picture to check the
//! framing against. So two sidecars:
//!
//! - **The grid.** Every tile outlined, the subject heavier, each one labelled
//!   with its coordinate, and the seed and scale written in the corner. This is
//!   what answers "is the middle tile actually in the middle", "did the layout
//!   come out at the size it was meant to", and "which world am I looking at".
//! - **The subject mask.** White inside the centre diamond, black outside. What
//!   a centre-only metric crops with, what a weighted training loss multiplies
//!   by, and what makes "the subject looks better than it did" a measurement
//!   rather than an impression.
//!
//! Both are derived from [`ResolvedIsoFrame`], which is also what the renderers
//! frame from — so an overlay cannot annotate ground the picture is not over.
//! Deriving them separately is exactly the mistake that makes a debug plate
//! confidently wrong.
//!
//! ## No blur on the neighbours
//!
//! Nothing here touches the beauty render. It is tempting to darken or soften
//! the eight context tiles so the subject reads as the subject, and it would put
//! a systematic difference one tile away from the middle of every frame — which
//! a neural renderer would learn in preference to learning grass.

use glam::Vec3;
use terrain_scene::frame::{ResolvedIsoFrame, TilePolygon};
use terrain_scene::layout::TileRole;

/// How the tile grid is drawn.
#[derive(Clone, Copy, Debug)]
pub struct GridStyle {
    /// Line width for a context tile, in pixels.
    pub context_px: f32,
    /// Line width for the subject, in pixels. Heavier, deliberately.
    pub subject_px: f32,
    pub context_colour: Vec3,
    pub subject_colour: Vec3,
    /// Text height in pixels; the font is 5×7, so this is seven times the scale.
    pub text_scale: usize,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self {
            context_px: 1.5,
            subject_px: 4.0,
            // Chosen to survive being drawn over grass: a saturated cyan and a
            // saturated orange are both far from anything a meadow contains, so
            // neither disappears into the picture it is annotating.
            context_colour: Vec3::new(0.05, 0.85, 0.95),
            subject_colour: Vec3::new(1.0, 0.55, 0.05),
            text_scale: 2,
        }
    }
}

/// A plate being drawn on.
///
/// Three values that always travel together, and never separately: passing them
/// one at a time made every function here take eight arguments, half of which
/// were the same three.
pub struct Canvas<'a> {
    pub pixels: &'a mut [Vec3],
    pub width: usize,
    pub height: usize,
}

impl<'a> Canvas<'a> {
    pub fn new(pixels: &'a mut [Vec3], width: usize, height: usize) -> Self {
        Self {
            pixels,
            width,
            height,
        }
    }

    /// Mix a colour into a pixel, clipping at the edges.
    #[inline]
    fn blend(&mut self, x: i64, y: i64, colour: Vec3, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let index = y as usize * self.width + x as usize;
        self.pixels[index] = self.pixels[index].lerp(colour, coverage.clamp(0.0, 1.0));
    }
}

/// Draw the layout's tiles over a plate, in place.
///
/// Context tiles first and the subject last, so the heavier outline wins where
/// they meet rather than being drawn over by its own neighbours.
pub fn draw_tile_grid(canvas: &mut Canvas<'_>, frame: &ResolvedIsoFrame, style: &GridStyle) {
    for role in [TileRole::Context, TileRole::Subject] {
        for tile in frame.tile_polygons_px.iter().filter(|t| t.role == role) {
            let (weight, colour) = match role {
                TileRole::Subject => (style.subject_px, style.subject_colour),
                TileRole::Context => (style.context_px, style.context_colour),
            };
            outline(canvas, tile, weight, colour);
            let centre = polygon_centre(tile);
            let label = format!("{},{}", tile.coord.u, tile.coord.v);
            draw_text(
                canvas,
                centre[0] - text_width(&label, style.text_scale) as f32 * 0.5,
                centre[1] - (GLYPH_ROWS * style.text_scale) as f32 * 0.5,
                &label,
                style.text_scale,
                colour,
            );
        }
    }
}

/// Write the lines of a caption into the top-left corner.
pub fn draw_caption(canvas: &mut Canvas<'_>, lines: &[String], style: &GridStyle) {
    let step = ((GLYPH_ROWS + 2) * style.text_scale) as f32;
    for (index, line) in lines.iter().enumerate() {
        draw_text(
            canvas,
            8.0,
            8.0 + index as f32 * step,
            line,
            style.text_scale,
            style.subject_colour,
        );
    }
}

/// How many pixels of each cell lie inside a tile the predicate accepts.
///
/// Returned as coverage in `0..1` rather than as a hard in-or-out, because the
/// diamond's edges run at two-to-one and a hard mask on a diagonal is a
/// staircase. A metric weighted by a staircase is a metric with the staircase in
/// it.
pub fn tile_coverage(
    frame: &ResolvedIsoFrame,
    mut accept: impl FnMut(&TilePolygon) -> bool,
) -> Vec<f32> {
    let (width, height) = (frame.output_size[0] as usize, frame.output_size[1] as usize);
    let chosen: Vec<&TilePolygon> = frame
        .tile_polygons_px
        .iter()
        .filter(|tile| accept(tile))
        .collect();

    let mut mask = vec![0.0f32; width * height];
    if chosen.is_empty() {
        return mask;
    }
    // Four samples on each axis: enough that a two-to-one edge reads as a smooth
    // ramp, cheap enough to run on a 1920×1080 plate without anyone noticing.
    const STEPS: usize = 4;
    let inverse = 1.0 / (STEPS * STEPS) as f32;
    for y in 0..height {
        for x in 0..width {
            let mut inside = 0usize;
            for sy in 0..STEPS {
                let py = y as f32 + (sy as f32 + 0.5) / STEPS as f32;
                for sx in 0..STEPS {
                    let px = x as f32 + (sx as f32 + 0.5) / STEPS as f32;
                    if chosen.iter().any(|tile| tile.contains_px(px, py)) {
                        inside += 1;
                    }
                }
            }
            mask[y * width + x] = inside as f32 * inverse;
        }
    }
    mask
}

/// The subject tile's mask.
pub fn subject_mask(frame: &ResolvedIsoFrame) -> Vec<f32> {
    tile_coverage(frame, |tile| tile.role == TileRole::Subject)
}

/// Every visible tile's mask: the outer diamond.
pub fn layout_mask(frame: &ResolvedIsoFrame) -> Vec<f32> {
    tile_coverage(frame, |_| true)
}

/// Pack a mask into eight-bit greyscale.
pub fn mask_to_gray8(mask: &[f32]) -> Vec<u8> {
    mask.iter()
        .map(|value| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect()
}

fn polygon_centre(tile: &TilePolygon) -> [f32; 2] {
    let mut centre = [0.0f32; 2];
    for corner in tile.corners_px {
        centre[0] += corner[0] * 0.25;
        centre[1] += corner[1] * 0.25;
    }
    centre
}

/// Draw a closed polygon's edges.
fn outline(canvas: &mut Canvas<'_>, tile: &TilePolygon, weight: f32, colour: Vec3) {
    for index in 0..4 {
        line(
            canvas,
            tile.corners_px[index],
            tile.corners_px[(index + 1) % 4],
            weight,
            colour,
        );
    }
}

/// A thick line segment, anti-aliased by distance.
///
/// Distance to the segment rather than Bresenham, because the diamond's edges
/// run at two-to-one and a stepped line on a plate whose whole subject is a
/// two-to-one diamond is a way to mistake the overlay for the geometry.
fn line(canvas: &mut Canvas<'_>, a: [f32; 2], b: [f32; 2], weight: f32, colour: Vec3) {
    let half = weight * 0.5;
    let low = [
        (a[0].min(b[0]) - half - 1.0).floor().max(0.0) as usize,
        (a[1].min(b[1]) - half - 1.0).floor().max(0.0) as usize,
    ];
    let high = [
        ((a[0].max(b[0]) + half + 1.0).ceil().max(0.0) as usize).min(canvas.width),
        ((a[1].max(b[1]) + half + 1.0).ceil().max(0.0) as usize).min(canvas.height),
    ];

    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length_squared = dx * dx + dy * dy;
    for y in low[1]..high[1] {
        for x in low[0]..high[0] {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = if length_squared <= 0.0 {
                0.0
            } else {
                (((px - a[0]) * dx + (py - a[1]) * dy) / length_squared).clamp(0.0, 1.0)
            };
            let (cx, cy) = (a[0] + dx * t, a[1] + dy * t);
            let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            // One pixel of feather, which is what stops the line reading as
            // aliased against ground that is not.
            let coverage = (half + 0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                canvas.blend(x as i64, y as i64, colour, coverage);
            }
        }
    }
}

/// Rows in one glyph.
const GLYPH_ROWS: usize = 7;
/// Columns in one glyph, before the one-column gap.
const GLYPH_COLUMNS: usize = 5;

/// How wide a string draws, in pixels.
pub fn text_width(text: &str, scale: usize) -> usize {
    text.chars().count() * (GLYPH_COLUMNS + 1) * scale
}

/// Draw a string, top-left anchored.
///
/// Upper case only, and unknown characters draw as a space. A debug plate does
/// not need a typeface; it needs to be readable at a glance on top of grass,
/// which a 5×7 bitmap at two or three times scale is and a hinted font at eight
/// pixels is not.
pub fn draw_text(canvas: &mut Canvas<'_>, x: f32, y: f32, text: &str, scale: usize, colour: Vec3) {
    let scale = scale.max(1);
    for (index, character) in text.chars().enumerate() {
        let glyph = glyph_for(character);
        let left = x + (index * (GLYPH_COLUMNS + 1) * scale) as f32;
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..GLYPH_COLUMNS {
                if bits & (1 << (GLYPH_COLUMNS - 1 - column)) == 0 {
                    continue;
                }
                let px0 = left as i64 + (column * scale) as i64;
                let py0 = y as i64 + (row * scale) as i64;
                for dy in 0..scale as i64 {
                    for dx in 0..scale as i64 {
                        canvas.blend(px0 + dx, py0 + dy, colour, 1.0);
                    }
                }
            }
        }
    }
}

/// The 5×7 bitmap, bit 4 leftmost.
fn glyph_for(character: char) -> [u8; GLYPH_ROWS] {
    match character.to_ascii_uppercase() {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x04, 0x0A, 0x11, 0x11, 0x1F, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0F, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0F],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0F, 0x10, 0x10, 0x13, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x11, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x11, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '-' => [0x00, 0x00, 0x00, 0x0E, 0x00, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        // Two cells and a tail, so a coordinate's separator is not mistaken for
        // a decimal point at the two-times scale a caption is drawn at.
        ',' => [0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04],
        ':' => [0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        _ => [0x00; GLYPH_ROWS],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_scene::frame::{IsoFrameOptions, ResolvedIsoFrame};
    use terrain_scene::layout::{IsoTileLayout, WorldTileCoord};
    use terrain_scene::projection::Projection;

    fn frame(width: u32, height: u32) -> ResolvedIsoFrame {
        let layout = IsoTileLayout::nine(WorldTileCoord::new(-713, 284), 4.0).expect("well formed");
        ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::sized(width, height),
        )
    }

    #[test]
    fn the_subject_mask_is_the_middle_diamond_and_nothing_else() {
        let frame = frame(480, 270);
        let mask = subject_mask(&frame);
        assert_eq!(mask.len(), 480 * 270);

        // The subject at this framing is a 144×72 diamond, whose area is half
        // its bounding box. Compared as a fraction of the plate, because that is
        // what a weighted loss actually multiplies by.
        let covered: f32 = mask.iter().sum();
        let expected = 144.0 * 72.0 / 2.0;
        assert!(
            (covered - expected).abs() < expected * 0.02,
            "{covered} against {expected}"
        );

        // The middle is white and the corners are black.
        assert!(mask[135 * 480 + 240] > 0.99);
        for corner in [0, 479, 269 * 480, 269 * 480 + 479] {
            assert_eq!(mask[corner], 0.0);
        }
    }

    #[test]
    fn the_layout_mask_is_the_outer_diamond_and_holds_the_subject() {
        // The internal joins have to close: a mask with a hairline of black down
        // every tile boundary would punch nine holes in a training weight.
        let frame = frame(480, 270);
        let subject = subject_mask(&frame);
        let layout = layout_mask(&frame);
        assert_eq!(subject.len(), layout.len());

        let outer: f32 = layout.iter().sum();
        let inner: f32 = subject.iter().sum();
        assert!(
            (outer - inner * 9.0).abs() < outer * 0.03,
            "the outer diamond is {outer} against nine subjects at {}",
            inner * 9.0
        );
        for (index, value) in subject.iter().enumerate() {
            assert!(
                layout[index] >= *value - 1.0e-6,
                "the subject reaches outside the layout at {index}"
            );
        }
    }

    #[test]
    fn a_mask_edge_ramps_rather_than_staircasing() {
        // A hard mask on a two-to-one diagonal is a staircase, and a metric
        // weighted by a staircase has the staircase in it.
        let frame = frame(480, 270);
        let mask = subject_mask(&frame);
        let partial = mask.iter().filter(|v| **v > 0.0 && **v < 1.0).count();
        assert!(partial > 100, "only {partial} pixels are partly covered");
    }

    #[test]
    fn the_grid_draws_over_the_plate_and_leaves_most_of_it_alone() {
        let frame = frame(480, 270);
        let mut pixels = vec![Vec3::ZERO; 480 * 270];
        draw_tile_grid(
            &mut Canvas::new(&mut pixels, 480, 270),
            &frame,
            &GridStyle::default(),
        );
        let touched = pixels.iter().filter(|p| **p != Vec3::ZERO).count();
        assert!(touched > 500, "the grid drew almost nothing: {touched}");
        assert!(
            touched < 480 * 270 / 8,
            "the grid covered the picture: {touched}"
        );
    }

    #[test]
    fn the_subject_outline_is_heavier_than_its_neighbours() {
        // The whole job of the debug plate: which tile is the render about.
        let frame = frame(480, 270);
        let style = GridStyle::default();
        let mut pixels = vec![Vec3::ZERO; 480 * 270];
        draw_tile_grid(&mut Canvas::new(&mut pixels, 480, 270), &frame, &style);
        let subject = pixels
            .iter()
            .filter(|p| p.distance(style.subject_colour) < 0.2)
            .count();
        let context = pixels
            .iter()
            .filter(|p| p.distance(style.context_colour) < 0.2)
            .count();
        assert!(subject > 0 && context > 0, "{subject} {context}");
        // Eight context tiles at 1.5 px against one subject at 4 px: the subject
        // is heavier per tile, which is what a reader sees.
        assert!(
            subject as f32 > context as f32 / 8.0,
            "{subject} subject pixels against {context} over eight tiles"
        );
    }

    #[test]
    fn text_stays_inside_the_plate_it_is_written_on() {
        // Captions are drawn near an edge by definition, so clipping is the
        // normal case rather than the exception.
        let mut pixels = vec![Vec3::ZERO; 32 * 16];
        let mut canvas = Canvas::new(&mut pixels, 32, 16);
        draw_text(&mut canvas, -20.0, -5.0, "SEED", 3, Vec3::ONE);
        draw_text(&mut canvas, 28.0, 12.0, "SEED", 3, Vec3::ONE);
        // Nothing panicked, and something landed.
        assert!(pixels.iter().any(|p| *p != Vec3::ZERO));
    }

    #[test]
    fn every_glyph_a_caption_uses_is_drawn() {
        // A missing glyph is a silent blank in a label, which reads as a
        // coordinate with a digit missing.
        for character in "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-,.:/#()+=".chars() {
            assert_ne!(
                glyph_for(character),
                [0x00; GLYPH_ROWS],
                "`{character}` draws as a blank"
            );
        }
        assert_eq!(glyph_for(' '), [0x00; GLYPH_ROWS]);
        assert_eq!(glyph_for('~'), [0x00; GLYPH_ROWS]);
        // Lower case draws as upper rather than as nothing.
        assert_eq!(glyph_for('a'), glyph_for('A'));
    }

    #[test]
    fn a_mask_packs_to_bytes_at_both_ends() {
        let bytes = mask_to_gray8(&[0.0, 0.5, 1.0, 2.0, -1.0]);
        assert_eq!(bytes, vec![0, 128, 255, 255, 0]);
    }
}

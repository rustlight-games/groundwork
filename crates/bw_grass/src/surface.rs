//! The buffers a page is composited into, and how they resolve to pixels.
//!
//! ## Why this is a depth buffer and not alpha-over
//!
//! Compositing thousands of grass stamps with ordinary alpha-over produces a
//! collage: every stroke sits flatly on top of the one before it, and the field
//! reads as a stack of decals rather than as something with an inside. So each
//! pixel remembers the isometric depth of whatever is currently on top of it,
//! and a stroke arriving later only takes the pixel if it is genuinely in front.
//! Losing fragments carry no visible payload. Canopy shadowing comes from the
//! winning height field, so recording every failed depth test only added a hot
//! read-modify-write that resolve discarded.
//!
//! Depth, not height, is the test. [`crate::iso::depth`] folds "how far down the
//! screen is this rooted" together with "how high does it stand", which is the
//! only ordering that lets a short blade in front cover a tall blade behind
//! while a tall blade still covers its own roots.
//!
//! ## Supersampling
//!
//! The reference art is painted, not pixel art: its strokes have soft two- and
//! three-pixel edges, and hard alpha would throw that away. Compositing at
//! [`SUPERSAMPLE`]× and box-filtering down gives every stroke edge ten levels of
//! coverage, which is enough to read as a brush mark rather than as a polygon.

use bevy::prelude::*;

use crate::palette::{self, Tone};

/// Linear scale factor the page is composited at before downsampling.
pub const SUPERSAMPLE: usize = 3;

/// Everything one supersampled pixel remembers, in one place.
///
/// Six parallel arrays would be the natural shape for this and it was the shape
/// it had. The rasteriser is what argues against it: a stroke plots along a rib
/// perpendicular to itself, so it walks one short run of pixels and then jumps a
/// row, and with six arrays that is six cache lines fetched for every run
/// instead of one. Interleaving them made the stroke pass measurably faster for
/// no change to a single pixel of output.
///
/// Twelve bytes, and the byte fields are bytes on purpose — `top` is a height in
/// final pixels and nothing in this field stands 255 of them tall, and `soil` is
/// a blend the eye reads to perhaps six bits.
#[derive(Clone, Copy)]
struct Cell {
    /// Isometric depth of whatever currently owns this pixel.
    depth: f32,
    /// The owning stroke's own light index, before any world-scale shading.
    light: f32,
    /// Height of the owning stroke above the soil, in final pixels.
    top: u8,
    /// Which ramp the owning stroke shades through.
    tone: u8,
    /// How far the floor at this pixel has turned to bare earth, `0..255`.
    ///
    /// A blend rather than a choice, and that is the whole point of it being a
    /// separate channel. Switching from the thatch ramp to the soil ramp at a
    /// threshold puts a hard edge around every bare patch — the two ramps differ
    /// in hue, so no light index makes them meet — and a hard-edged patch reads
    /// as a stone lying on the grass rather than as ground showing through it.
    soil: u8,
}

/// The composited state of one page, at supersampled resolution.
pub struct Surface {
    /// Supersampled width in pixels.
    pub width: usize,
    /// Supersampled height in pixels.
    pub height: usize,
    cells: Vec<Cell>,
}

impl Surface {
    /// An empty page, filled with soil at the ground plane.
    pub fn new(final_width: usize, final_height: usize) -> Self {
        let width = final_width * SUPERSAMPLE;
        let height = final_height * SUPERSAMPLE;
        Self {
            width,
            height,
            cells: vec![
                Cell {
                    depth: f32::NEG_INFINITY,
                    light: 0.0,
                    top: 0,
                    tone: Tone::Soil as u8,
                    soil: 0,
                };
                width * height
            ],
        }
    }

    /// Offer a pixel to the surface, taking it only if it is in front.
    ///
    /// `top` is in final pixels rather than supersampled ones so it fits a byte
    /// with room to spare; nothing in this field stands 255 pixels tall.
    ///
    /// The index is not bounds-checked against a slice a second time — the
    /// caller has already clamped it — but it is still a safe indexing
    /// operation, so a mistake panics rather than corrupting the page.
    #[inline]
    pub fn write(&mut self, index: usize, depth: f32, light: f32, tone: Tone, top: f32) {
        let cell = &mut self.cells[index];
        if depth >= cell.depth {
            cell.depth = depth;
            cell.light = light;
            cell.tone = tone as u8;
            cell.top = (top.clamp(0.0, 255.0)) as u8;
            // A blade covering bare earth is a blade, not earth.
            cell.soil = 0;
        }
    }

    /// Fill every pixel unconditionally — the floor pass, and nothing else.
    ///
    /// `soil` is how far this patch of floor has turned to bare earth: nought is
    /// the dark mat under a thick canopy, one is exposed ground.
    #[inline]
    pub fn lay(&mut self, index: usize, light: f32, soil: f32) {
        let cell = &mut self.cells[index];
        cell.depth = f32::NEG_INFINITY;
        cell.light = light;
        cell.tone = Tone::Thatch as u8;
        cell.top = 0;
        cell.soil = (soil.clamp(0.0, 1.0) * 255.0) as u8;
    }

    /// Lay a whole run of floor pixels that share a colour.
    ///
    /// The floor pass fills a supersampled block per final pixel, so its inner
    /// loop is [`SUPERSAMPLE`] identical writes to consecutive addresses. Handing
    /// the run over whole lets it be one bounds check and one straight-line
    /// store instead of nine of each.
    #[inline]
    pub fn lay_run(&mut self, index: usize, count: usize, light: f32, soil: f32) {
        let cell = Cell {
            depth: f32::NEG_INFINITY,
            light,
            top: 0,
            tone: Tone::Thatch as u8,
            soil: (soil.clamp(0.0, 1.0) * 255.0) as u8,
        };
        self.cells[index..index + count].fill(cell);
    }

    /// How far toward bare earth this pixel's floor has gone, `0..1`.
    #[inline]
    pub fn soil_at(&self, index: usize) -> f32 {
        self.cells[index].soil as f32 / 255.0
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// Canopy height, box-filtered to final resolution.
    pub fn height_map(&self, final_width: usize, final_height: usize) -> Vec<f32> {
        let mut heights = vec![0.0f32; final_width * final_height];
        let inverse = 1.0 / (SUPERSAMPLE * SUPERSAMPLE) as f32;
        for y in 0..final_height {
            for x in 0..final_width {
                let mut height = 0u32;
                for sy in 0..SUPERSAMPLE {
                    let row = (y * SUPERSAMPLE + sy) * self.width + x * SUPERSAMPLE;
                    for cell in &self.cells[row..row + SUPERSAMPLE] {
                        height += cell.top as u32;
                    }
                }
                heights[y * final_width + x] = height as f32 * inverse;
            }
        }
        heights
    }

    /// Coverage helper for shape-bound tests. Production pages lay a floor
    /// before strokes, so this intentionally remains test-only.
    #[cfg(test)]
    pub(crate) fn painted_map(&self, final_width: usize, final_height: usize) -> Vec<f32> {
        let mut painted = vec![0.0; final_width * final_height];
        let inverse = 1.0 / (SUPERSAMPLE * SUPERSAMPLE) as f32;
        for y in 0..final_height {
            for x in 0..final_width {
                let mut count = 0usize;
                for sy in 0..SUPERSAMPLE {
                    let row = (y * SUPERSAMPLE + sy) * self.width + x * SUPERSAMPLE;
                    count += self.cells[row..row + SUPERSAMPLE]
                        .iter()
                        .filter(|cell| cell.depth.is_finite())
                        .count();
                }
                painted[y * final_width + x] = count as f32 * inverse;
            }
        }
        painted
    }

    /// The stroke light index and tone at a supersampled pixel.
    #[inline]
    pub fn pixel(&self, index: usize) -> (f32, Tone) {
        let cell = &self.cells[index];
        let tone = match cell.tone {
            0 => Tone::Soil,
            1 => Tone::Thatch,
            2 => Tone::Grass,
            3 => Tone::Leaf,
            _ => Tone::Dry,
        };
        (cell.light, tone)
    }

    /// Height above the soil at a supersampled pixel, in final pixels.
    #[inline]
    pub fn top_at(&self, index: usize) -> f32 {
        self.cells[index].top as f32
    }

    /// Average colour of the supersampled block behind one final pixel.
    ///
    /// `shade` turns one supersampled pixel into a colour; averaging afterwards
    /// rather than averaging the light index first matters, because two pixels
    /// on different ramps have no meaningful average index — soil at 0.5 and
    /// grass at 0.5 are not the same colour, and blending the indices would
    /// invent a third material that exists nowhere in the palette.
    pub fn resolve_pixel(&self, x: usize, y: usize, mut shade: impl FnMut(usize) -> Vec3) -> Vec3 {
        let mut total = Vec3::ZERO;
        for sy in 0..SUPERSAMPLE {
            for sx in 0..SUPERSAMPLE {
                total += shade(self.index(x * SUPERSAMPLE + sx, y * SUPERSAMPLE + sy));
            }
        }
        total / (SUPERSAMPLE * SUPERSAMPLE) as f32
    }
}

/// A separable box blur, run twice, which is close enough to a Gaussian for
/// shading terms and a great deal cheaper.
pub fn blur(source: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 {
        return source.to_vec();
    }
    let mut a = source.to_vec();
    let mut b = vec![0.0; source.len()];
    for _ in 0..2 {
        // Horizontal.
        for y in 0..height {
            let row = y * width;
            let mut sum = 0.0;
            for x in 0..=radius.min(width - 1) {
                sum += a[row + x];
            }
            let mut count = radius.min(width - 1) + 1;
            for x in 0..width {
                b[row + x] = sum / count as f32;
                if x >= radius {
                    sum -= a[row + x - radius];
                    count -= 1;
                }
                if x + radius + 1 < width {
                    sum += a[row + x + radius + 1];
                    count += 1;
                }
            }
        }
        // Vertical.
        for x in 0..width {
            let mut sum = 0.0;
            for y in 0..=radius.min(height - 1) {
                sum += b[y * width + x];
            }
            let mut count = radius.min(height - 1) + 1;
            for y in 0..height {
                a[y * width + x] = sum / count as f32;
                if y >= radius {
                    sum -= b[(y - radius) * width + x];
                    count -= 1;
                }
                if y + radius + 1 < height {
                    sum += b[(y + radius + 1) * width + x];
                    count += 1;
                }
            }
        }
    }
    a
}

/// Area-average a plate down to a target size.
///
/// A box filter over each output pixel's exact footprint, fractional edges
/// included, which is what a correct mip chain converges to. So this shows the
/// **best case** for how the surface minifies — worth stating plainly, because
/// baked pages currently have no mip chain at all, and a GPU point-sampling a
/// page at a third of its size will alias considerably worse than this.
///
/// It matters to the snapshot suite for one reason beyond correctness: it is
/// deterministic and independent of the machine. A snapshot taken through a GPU
/// would differ between two computers by more than most optimisations do, and
/// the comparison would be measuring the driver.
pub fn resample(
    source: &[Vec3],
    width: usize,
    height: usize,
    target_w: usize,
    target_h: usize,
) -> Vec<Vec3> {
    if width == 0 || height == 0 || target_w == 0 || target_h == 0 {
        return vec![Vec3::ZERO; target_w * target_h];
    }
    let sx = width as f32 / target_w as f32;
    let sy = height as f32 / target_h as f32;
    let mut out = vec![Vec3::ZERO; target_w * target_h];

    for y in 0..target_h {
        let (top, bottom) = (y as f32 * sy, (y as f32 + 1.0) * sy);
        for x in 0..target_w {
            let (left, right) = (x as f32 * sx, (x as f32 + 1.0) * sx);
            let mut total = Vec3::ZERO;
            let mut weight = 0.0f32;
            for py in top.floor() as usize..(bottom.ceil() as usize).min(height) {
                // Vertical overlap of this source row with the output pixel.
                let cover_y = (bottom.min(py as f32 + 1.0) - top.max(py as f32)).max(0.0);
                if cover_y <= 0.0 {
                    continue;
                }
                for px in left.floor() as usize..(right.ceil() as usize).min(width) {
                    let cover_x = (right.min(px as f32 + 1.0) - left.max(px as f32)).max(0.0);
                    if cover_x <= 0.0 {
                        continue;
                    }
                    let w = cover_x * cover_y;
                    total += source[py * width + px] * w;
                    weight += w;
                }
            }
            out[y * target_w + x] = if weight > 0.0 {
                total / weight
            } else {
                Vec3::ZERO
            };
        }
    }
    out
}

/// Pack linear colours into sRGB bytes.
pub fn to_rgb8(colours: &[Vec3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colours.len() * 3);
    for colour in colours {
        bytes.extend_from_slice(&palette::to_bytes(*colour));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nearer_stroke_takes_the_pixel() {
        let mut surface = Surface::new(2, 2);
        surface.write(0, 1.0, 0.4, Tone::Grass, 10.0);
        surface.write(0, 2.0, 0.9, Tone::Leaf, 20.0);
        let (light, tone) = surface.pixel(0);
        assert_eq!(tone, Tone::Leaf);
        assert!((light - 0.9).abs() < 1e-6);
        assert_eq!(surface.top_at(0), 20.0);
    }

    #[test]
    fn a_further_stroke_does_not_steal_the_pixel() {
        let mut surface = Surface::new(2, 2);
        surface.write(0, 5.0, 0.9, Tone::Grass, 20.0);
        surface.write(0, 1.0, 0.1, Tone::Thatch, 4.0);
        let (light, tone) = surface.pixel(0);
        assert_eq!(tone, Tone::Grass, "the buried stroke stole the pixel");
        assert!((light - 0.9).abs() < 1e-6);
    }

    #[test]
    fn blurring_preserves_the_mean() {
        let source: Vec<f32> = (0..64 * 64)
            .map(|i| ((i * 37) % 101) as f32 / 100.0)
            .collect();
        let mean = source.iter().sum::<f32>() / source.len() as f32;
        let blurred = blur(&source, 64, 64, 3);
        let after = blurred.iter().sum::<f32>() / blurred.len() as f32;
        assert!((mean - after).abs() < 0.02, "{mean} vs {after}");
        // And it actually smoothed: variance must fall.
        let spread = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32
        };
        assert!(spread(&blurred) < spread(&source) * 0.5);
    }

    #[test]
    fn a_zero_radius_blur_is_the_identity() {
        let source = vec![0.25, 0.5, 0.75, 1.0];
        assert_eq!(blur(&source, 2, 2, 0), source);
    }

    #[test]
    fn resampling_preserves_the_mean() {
        // The property the snapshot comparison depends on. A downsample that
        // shifted the average would make every view look like a tone regression
        // at the zoom levels it was worst at.
        let source: Vec<Vec3> = (0..120 * 90)
            .map(|i| Vec3::splat(((i * 41) % 97) as f32 / 96.0))
            .collect();
        let mean = |v: &[Vec3]| v.iter().fold(Vec3::ZERO, |a, b| a + *b) / v.len() as f32;
        let shrunk = resample(&source, 120, 90, 40, 30);
        assert_eq!(shrunk.len(), 40 * 30);
        assert!((mean(&shrunk) - mean(&source)).length() < 0.01);
    }

    #[test]
    fn resampling_to_the_same_size_changes_nothing() {
        let source: Vec<Vec3> = (0..8 * 8).map(|i| Vec3::splat(i as f32 / 64.0)).collect();
        let same = resample(&source, 8, 8, 8, 8);
        for (a, b) in same.iter().zip(&source) {
            assert!((*a - *b).length() < 1.0e-5);
        }
    }

    #[test]
    fn resampling_a_degenerate_plate_does_not_panic() {
        assert_eq!(resample(&[], 0, 0, 4, 4).len(), 16);
        assert!(resample(&[Vec3::ONE], 1, 1, 0, 0).is_empty());
    }
}

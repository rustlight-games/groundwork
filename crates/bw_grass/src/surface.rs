//! The buffers a page is composited into, and how they resolve to pixels.
//!
//! ## Why this is a depth buffer and not alpha-over
//!
//! Compositing thousands of grass stamps with ordinary alpha-over produces a
//! collage: every stroke sits flatly on top of the one before it, and the field
//! reads as a stack of decals rather than as something with an inside. So each
//! pixel remembers the isometric depth of whatever is currently on top of it,
//! and a stroke arriving later only takes the pixel if it is genuinely in front.
//! Everything that loses still counts — it is recorded as occlusion — which is
//! where the dark interiors of the mounds come from.
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

/// The composited state of one page, at supersampled resolution.
pub struct Surface {
    /// Supersampled width in pixels.
    pub width: usize,
    /// Supersampled height in pixels.
    pub height: usize,
    /// Isometric depth of whatever currently owns each pixel.
    depth: Vec<f32>,
    /// The owning stroke's own light index, before any world-scale shading.
    light: Vec<f32>,
    /// Which ramp the owning stroke shades through.
    tone: Vec<u8>,
    /// Height of the owning stroke above the soil, in final pixels.
    top: Vec<u8>,
    /// How many strokes have been buried at this pixel, saturating.
    ///
    /// The cheapest possible measure of "how much grass is there", and the only
    /// one available for free: a pixel that twenty strokes fought over is deep
    /// inside a clump, and a pixel nothing contested is a gap.
    buried: Vec<u8>,
    /// How far the floor at this pixel has turned to bare earth, `0..255`.
    ///
    /// A blend rather than a choice, and that is the whole point of it being a
    /// separate channel. Switching from the thatch ramp to the soil ramp at a
    /// threshold puts a hard edge around every bare patch — the two ramps differ
    /// in hue, so no light index makes them meet — and a hard-edged patch reads
    /// as a stone lying on the grass rather than as ground showing through it.
    soil: Vec<u8>,
}

impl Surface {
    /// An empty page, filled with soil at the ground plane.
    pub fn new(final_width: usize, final_height: usize) -> Self {
        let width = final_width * SUPERSAMPLE;
        let height = final_height * SUPERSAMPLE;
        let count = width * height;
        Self {
            width,
            height,
            depth: vec![f32::NEG_INFINITY; count],
            light: vec![0.0; count],
            tone: vec![Tone::Soil as u8; count],
            top: vec![0; count],
            buried: vec![0; count],
            soil: vec![0; count],
        }
    }

    /// Offer a pixel to the surface, taking it only if it is in front.
    ///
    /// `top` is in final pixels rather than supersampled ones so it fits a byte
    /// with room to spare; nothing in this field stands 255 pixels tall.
    #[inline]
    pub fn write(&mut self, index: usize, depth: f32, light: f32, tone: Tone, top: f32) {
        if depth >= self.depth[index] {
            self.depth[index] = depth;
            self.light[index] = light;
            self.tone[index] = tone as u8;
            self.top[index] = (top.clamp(0.0, 255.0)) as u8;
            // A blade covering bare earth is a blade, not earth.
            self.soil[index] = 0;
        }
        self.buried[index] = self.buried[index].saturating_add(1);
    }

    /// Fill every pixel unconditionally — the floor pass, and nothing else.
    ///
    /// `soil` is how far this patch of floor has turned to bare earth: nought is
    /// the dark mat under a thick canopy, one is exposed ground.
    #[inline]
    pub fn lay(&mut self, index: usize, light: f32, soil: f32) {
        self.depth[index] = f32::NEG_INFINITY;
        self.light[index] = light;
        self.tone[index] = Tone::Thatch as u8;
        self.top[index] = 0;
        self.soil[index] = (soil.clamp(0.0, 1.0) * 255.0) as u8;
    }

    /// How far toward bare earth this pixel's floor has gone, `0..1`.
    #[inline]
    pub fn soil_at(&self, index: usize) -> f32 {
        self.soil[index] as f32 / 255.0
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// Canopy height and contested-ness, box-filtered to final resolution.
    ///
    /// Both of the derived shading terms — local occlusion and the fixed
    /// directional shadow — want a smooth height field rather than the jagged
    /// per-stroke one, and neither needs supersampled detail.
    pub fn height_maps(&self, final_width: usize, final_height: usize) -> (Vec<f32>, Vec<f32>) {
        let mut heights = vec![0.0f32; final_width * final_height];
        let mut density = vec![0.0f32; final_width * final_height];
        let inverse = 1.0 / (SUPERSAMPLE * SUPERSAMPLE) as f32;
        for y in 0..final_height {
            for x in 0..final_width {
                let (mut height, mut buried) = (0.0f32, 0.0f32);
                for sy in 0..SUPERSAMPLE {
                    for sx in 0..SUPERSAMPLE {
                        let i = self.index(x * SUPERSAMPLE + sx, y * SUPERSAMPLE + sy);
                        height += self.top[i] as f32;
                        buried += self.buried[i] as f32;
                    }
                }
                heights[y * final_width + x] = height * inverse;
                density[y * final_width + x] = buried * inverse;
            }
        }
        (heights, density)
    }

    /// The stroke light index and tone at a supersampled pixel.
    #[inline]
    pub fn pixel(&self, index: usize) -> (f32, Tone) {
        let tone = match self.tone[index] {
            0 => Tone::Soil,
            1 => Tone::Thatch,
            2 => Tone::Grass,
            3 => Tone::Leaf,
            _ => Tone::Dry,
        };
        (self.light[index], tone)
    }

    /// Height above the soil at a supersampled pixel, in final pixels.
    #[inline]
    pub fn top_at(&self, index: usize) -> f32 {
        self.top[index] as f32
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
    fn a_further_stroke_is_buried_rather_than_lost() {
        let mut surface = Surface::new(2, 2);
        surface.write(0, 5.0, 0.9, Tone::Grass, 20.0);
        surface.write(0, 1.0, 0.1, Tone::Thatch, 4.0);
        let (light, tone) = surface.pixel(0);
        assert_eq!(tone, Tone::Grass, "the buried stroke stole the pixel");
        assert!((light - 0.9).abs() < 1e-6);
        // But it still counted: this is where cavity darkness comes from.
        let (_, density) = surface.height_maps(2, 2);
        assert!(density[0] > 0.0);
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
}

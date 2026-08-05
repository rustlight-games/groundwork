//! A renderer-agnostic plate: colour, alpha, and the 8-bit round trip.
//!
//! Nothing here draws. [`RenderImage`] is the shape a finished plate takes
//! once it exists — Cycles fills one, a PNG round-trips through
//! [`to_bytes`]/[`from_bytes_rgb`], and a debug overlay or a comparison metric
//! reads one back. Keeping the container here, apart from any renderer, is
//! what lets a benchmark compare two plates without caring which produced
//! them.

use glam::Vec3;

/// A plate: colour and coverage, quantised the same way a stored page is.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderImage {
    pub colour: Vec<Vec3>,
    /// How much of each pixel is picture rather than background, `0..1`.
    pub alpha: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

impl RenderImage {
    /// A fully opaque plate, for a caller that has no silhouette.
    pub fn opaque(colour: Vec<Vec3>, width: usize, height: usize) -> Self {
        Self {
            alpha: vec![1.0; colour.len()],
            colour,
            width,
            height,
        }
    }

    /// Interleave into the RGBA bytes a PNG wants.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.colour.len() * 4);
        for (colour, alpha) in self.colour.iter().zip(&self.alpha) {
            bytes.extend_from_slice(&to_bytes(*colour));
            bytes.push((alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        bytes
    }

    /// What fraction of the plate is covered at all.
    pub fn coverage(&self) -> f32 {
        if self.alpha.is_empty() {
            return 0.0;
        }
        self.alpha.iter().sum::<f32>() / self.alpha.len() as f32
    }
}

/// Quantise a colour to the 8-bit values a page is stored in.
pub fn to_bytes(colour: Vec3) -> [u8; 3] {
    [
        (colour.x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (colour.y.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (colour.z.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

/// Quantise a whole plate to packed RGB bytes, three a pixel.
pub fn to_rgb8(colours: &[Vec3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colours.len() * 3);
    for colour in colours {
        bytes.extend_from_slice(&to_bytes(*colour));
    }
    bytes
}

/// The inverse of [`to_bytes`], over a whole packed plate.
///
/// Only a rescale, because [`to_bytes`] is only a quantise — there is no gamma
/// conversion in either direction. What this is for is a plate that arrived as
/// bytes from somewhere else, usually Cycles by way of a PNG: an overlay drawn
/// on it has to be drawn in the same space it was read in, or the annotation
/// and the picture disagree about what a value means.
pub fn from_bytes_rgb(bytes: &[u8]) -> Vec<Vec3> {
    bytes
        .chunks_exact(3)
        .map(|pixel| {
            Vec3::new(
                pixel[0] as f32 / 255.0,
                pixel[1] as f32 / 255.0,
                pixel[2] as f32 / 255.0,
            )
        })
        .collect()
}

/// A separable box blur, run twice, which is close enough to a Gaussian for
/// a comparison metric and a great deal cheaper.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_round_trips_through_bytes_within_one_step() {
        let colour = Vec3::new(0.2, 0.5, 0.9);
        let bytes = to_bytes(colour);
        let back = from_bytes_rgb(&bytes);
        for (channel, original) in back[0].to_array().iter().zip(colour.to_array()) {
            assert!((channel - original).abs() < 1.0 / 255.0 + 1.0e-6);
        }
    }

    #[test]
    fn a_flat_field_is_unchanged_by_a_blur() {
        let flat = vec![0.4f32; 16 * 16];
        let blurred = blur(&flat, 16, 16, 3);
        for value in blurred {
            assert!((value - 0.4).abs() < 1.0e-5);
        }
    }
}

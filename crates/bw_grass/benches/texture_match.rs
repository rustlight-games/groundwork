//! How close the rendered grass is to the reference plate.
//!
//! `benchmarks/reference/pixel_grass_target.png` is the art target: a piece of
//! hand-authored pixel-art turf. The goal is not to reproduce it — the shader
//! generates unique, unbounded, non-tiling grass, and copying a 1254-pixel
//! square would defeat the entire system. The goal is for a screenshot of our
//! field to be **indistinguishable from it in character**.
//!
//! So nothing here compares pixels by position. Every metric is a *descriptor*
//! of the texture as a whole, computed identically on both images and then
//! compared. Two images with the same descriptors look like the same material
//! even when they share no pixel.
//!
//! | Descriptor | What it catches |
//! |---|---|
//! | [`value_curve`] | The value hierarchy: how dark the darks, how bright the brights |
//! | [`chroma`] | Hue and saturation distribution — grey drift, wrong green |
//! | [`detail_spectrum`] | Detail at each scale: noisy vs mushy vs banded |
//! | [`local_contrast`] | Whether the surface has relief or is flat |
//! | [`anisotropy`] | Whether the grass has a grain or is directionless |
//! | [`cluster_sizes`] | Pixel-cluster discipline: confetti vs connected marks |
//!
//! Each returns a similarity in 0..1 against the reference, and
//! [`Match::overall`] is their weighted mean. That single number is the one to
//! watch: it is the answer to "does this look like the target yet".
//!
//! ## Why a screenshot
//!
//! There is no way to evaluate a shader's output without running the shader.
//! The frame comes from the sandbox's scripted capture, which is deterministic
//! — same seed, same wind clock, same picture every run — so the score is
//! reproducible even though a GPU produced it. If no capture is present the
//! benchmark reports that and skips this section rather than inventing numbers.

use std::path::Path;

/// A grayscale view of an image, plus its colour statistics.
pub struct Plate {
    pub width: usize,
    pub height: usize,
    /// Perceptual luminance, 0..1.
    pub luma: Vec<f32>,
    /// Per-pixel `(hue angle as unit vector, saturation)`.
    pub chroma: Vec<[f32; 3]>,
}

impl Plate {
    /// Load a PNG.
    pub fn load(path: &Path) -> Option<Self> {
        let image = image::open(path).ok()?.to_rgb8();
        let (width, height) = (image.width() as usize, image.height() as usize);
        let mut luma = Vec::with_capacity(width * height);
        let mut chroma = Vec::with_capacity(width * height);

        for pixel in image.pixels() {
            let r = pixel[0] as f32 / 255.0;
            let g = pixel[1] as f32 / 255.0;
            let b = pixel[2] as f32 / 255.0;
            luma.push(0.2126 * r + 0.7152 * g + 0.0722 * b);

            // Hue as a vector rather than an angle, so averaging it does not
            // break at the wrap point, plus saturation.
            let high = r.max(g).max(b);
            let low = r.min(g).min(b);
            let saturation = if high > 0.0 { (high - low) / high } else { 0.0 };
            // Standard chromaticity axes; no need for a full Lab conversion to
            // tell a green from a grey.
            let a = r - g;
            let c = g - b;
            chroma.push([a, c, saturation]);
        }

        Some(Self {
            width,
            height,
            luma,
            chroma,
        })
    }

    /// Crop to a centred square, so plates of different shapes compare fairly.
    ///
    /// A screenshot is 16:9 and the reference is square. Comparing detail
    /// spectra between different aspect ratios measures the crop as much as the
    /// content.
    pub fn centre_square(&self, size: usize) -> Self {
        let size = size.min(self.width).min(self.height);
        let x0 = (self.width - size) / 2;
        let y0 = (self.height - size) / 2;
        let mut luma = Vec::with_capacity(size * size);
        let mut chroma = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                let index = (y0 + y) * self.width + x0 + x;
                luma.push(self.luma[index]);
                chroma.push(self.chroma[index]);
            }
        }
        Self {
            width: size,
            height: size,
            luma,
            chroma,
        }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.luma[y * self.width + x]
    }
}

/// How far apart two brightnesses may be before they score zero.
const VALUE_TOLERANCE: f32 = 0.25;

/// Percentiles the value curve is sampled at.
const VALUE_POINTS: [f32; 9] = [0.02, 0.10, 0.25, 0.40, 0.50, 0.60, 0.75, 0.90, 0.98];

/// Scales the detail spectrum is measured at, in pixels.
const DETAIL_SCALES: [usize; 5] = [1, 2, 4, 8, 16];

/// Brightness at a spread of percentiles, darkest first.
///
/// Percentiles rather than a histogram, and the difference is not academic.
/// Histogram intersection compares bucket-for-bucket, so two images whose value
/// distributions are the *same shape* but shifted by less than a bucket score
/// badly — and it was doing exactly that here. A frame matching the target's
/// median to within 0.017 and its standard deviation to within 0.004 scored
/// 0.42, while a visibly worse frame scored 0.56. Nobody compares two pictures
/// bucket by bucket; they compare how dark the darks are and how bright the
/// brights are, which is this.
pub fn value_curve(plate: &Plate) -> Vec<f32> {
    let mut sorted = plate.luma.clone();
    sorted.sort_by(f32::total_cmp);
    if sorted.is_empty() {
        return vec![0.0; VALUE_POINTS.len()];
    }
    let last = sorted.len() - 1;
    VALUE_POINTS
        .iter()
        .map(|&point| sorted[((last as f32) * point) as usize])
        .collect()
}

/// Mean chromaticity and saturation.
pub fn chroma(plate: &Plate) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for value in &plate.chroma {
        for (accumulator, component) in sum.iter_mut().zip(value) {
            *accumulator += component;
        }
    }
    let count = plate.chroma.len().max(1) as f32;
    [sum[0] / count, sum[1] / count, sum[2] / count]
}

/// How much contrast survives blurring at each scale.
///
/// This is what separates a fine felted texture from a smooth wash or from
/// static: it says where in the frequency range the picture keeps its
/// information. Two grasses with matching spectra have the same *grain*.
pub fn detail_spectrum(plate: &Plate) -> Vec<f32> {
    DETAIL_SCALES
        .iter()
        .map(|&scale| {
            // Standard deviation of a box-downsampled copy. Downsampling is a
            // blur, so the deviation that remains is the energy at or below
            // this scale.
            let blocks_x = plate.width / scale;
            let blocks_y = plate.height / scale;
            if blocks_x == 0 || blocks_y == 0 {
                return 0.0;
            }
            let mut values = Vec::with_capacity(blocks_x * blocks_y);
            for by in 0..blocks_y {
                for bx in 0..blocks_x {
                    let mut total = 0.0;
                    for y in 0..scale {
                        for x in 0..scale {
                            total += plate.at(bx * scale + x, by * scale + y);
                        }
                    }
                    values.push(total / (scale * scale) as f32);
                }
            }
            deviation(&values)
        })
        .collect()
}

/// Mean absolute difference between neighbouring pixels.
///
/// Low means flat. This is the number that moves when a field "looks like a
/// texture rather than a place".
pub fn local_contrast(plate: &Plate) -> f32 {
    let mut total = 0.0;
    let mut count = 0.0;
    for y in 0..plate.height.saturating_sub(1) {
        for x in 0..plate.width.saturating_sub(1) {
            let here = plate.at(x, y);
            total += (here - plate.at(x + 1, y)).abs() + (here - plate.at(x, y + 1)).abs();
            count += 2.0;
        }
    }
    if count > 0.0 { total / count } else { 0.0 }
}

/// How directional the texture is, in 0..1.
///
/// Grass has a grain — it is combed. A directionless surface scores near zero,
/// which is what moss and noise both look like.
pub fn anisotropy(plate: &Plate) -> f32 {
    let mut horizontal = 0.0;
    let mut vertical = 0.0;
    for y in 0..plate.height.saturating_sub(1) {
        for x in 0..plate.width.saturating_sub(1) {
            let here = plate.at(x, y);
            horizontal += (here - plate.at(x + 1, y)).abs();
            vertical += (here - plate.at(x, y + 1)).abs();
        }
    }
    let total = horizontal + vertical;
    if total <= 0.0 {
        return 0.0;
    }
    ((horizontal - vertical) / total).abs()
}

/// Mean size of a connected run of above-average pixels, in pixels.
///
/// The doc-directed "pixel-cluster discipline": marks should be connected
/// shapes, not isolated dots. A field of one-pixel confetti scores near one.
pub fn cluster_sizes(plate: &Plate) -> f32 {
    let mean = plate.luma.iter().sum::<f32>() / plate.luma.len().max(1) as f32;
    // Horizontal run lengths of bright pixels. A full connected-component pass
    // would be more precise and much slower; run length correlates with it
    // closely enough to catch confetti, which is what this is for.
    let mut runs = Vec::new();
    for y in 0..plate.height {
        let mut run = 0usize;
        for x in 0..plate.width {
            if plate.at(x, y) > mean {
                run += 1;
            } else if run > 0 {
                runs.push(run as f32);
                run = 0;
            }
        }
        if run > 0 {
            runs.push(run as f32);
        }
    }
    if runs.is_empty() {
        return 0.0;
    }
    runs.iter().sum::<f32>() / runs.len() as f32
}

/// Whether a plate carries no information at all.
///
/// Guards against a failed screenshot being scored as a failed renderer — see
/// the caller. One flat colour is never a legitimate frame of this scene.
pub fn is_degenerate(plate: &Plate) -> bool {
    let Some(&first) = plate.luma.first() else {
        return true;
    };
    plate.luma.iter().all(|&v| (v - first).abs() < 1e-6)
}

/// The full comparison.
pub struct Match {
    pub value: f32,
    pub chroma: f32,
    pub detail: f32,
    pub contrast: f32,
    pub grain: f32,
    pub clusters: f32,
    pub overall: f32,
}

/// Score a rendered frame against the reference plate.
pub fn compare(rendered: &Plate, reference: &Plate) -> Match {
    // Both cropped to the same square, so the descriptors measure content
    // rather than aspect ratio.
    let size = rendered
        .width
        .min(rendered.height)
        .min(reference.width)
        .min(reference.height);
    let a = rendered.centre_square(size);
    let b = reference.centre_square(size);

    // Mean closeness across the value curve, with a tolerance of a quarter of
    // the full range: a tenth of the range apart still scores 0.6, which is
    // about how forgiving an eye is about overall exposure.
    let value = {
        let ours = value_curve(&a);
        let theirs = value_curve(&b);
        ours.iter()
            .zip(&theirs)
            .map(|(x, y)| (1.0 - (x - y).abs() / VALUE_TOLERANCE).clamp(0.0, 1.0))
            .sum::<f32>()
            / ours.len() as f32
    };
    let chroma = vector_similarity(&chroma(&a), &chroma(&b), 0.25);
    let detail = ratio_similarity_all(&detail_spectrum(&a), &detail_spectrum(&b));
    let contrast = ratio_similarity(local_contrast(&a), local_contrast(&b));
    let grain = 1.0 - (anisotropy(&a) - anisotropy(&b)).abs().min(1.0);
    let clusters = ratio_similarity(cluster_sizes(&a), cluster_sizes(&b));

    // Weighted toward what a person notices first. Value hierarchy and colour
    // dominate the impression of "same material"; grain and cluster shape are
    // what separate good pixel art from a resized photograph.
    let overall = 0.28 * value
        + 0.24 * chroma
        + 0.20 * detail
        + 0.12 * contrast
        + 0.08 * grain
        + 0.08 * clusters;

    Match {
        value,
        chroma,
        detail,
        contrast,
        grain,
        clusters,
        overall,
    }
}

/// How close two scalars are, as the smaller over the larger.
fn ratio_similarity(a: f32, b: f32) -> f32 {
    let (low, high) = (a.min(b), a.max(b));
    if high <= 1e-9 {
        return 1.0;
    }
    (low / high).clamp(0.0, 1.0)
}

fn ratio_similarity_all(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| ratio_similarity(*x, *y))
        .sum::<f32>()
        / a.len() as f32
}

/// Similarity of two short vectors, where `tolerance` is the distance at which
/// they score zero.
fn vector_similarity(a: &[f32; 3], b: &[f32; 3], tolerance: f32) -> f32 {
    let distance = a
        .iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt();
    (1.0 - distance / tolerance).clamp(0.0, 1.0)
}

fn deviation(values: &[f32]) -> f32 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32).sqrt()
}

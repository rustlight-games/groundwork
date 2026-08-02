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
//! | [`feature_scale`] | How big a tuft reads — the same material at the wrong zoom |
//! | [`orientation_histogram`] | Which way the marks run: fanning blades or directionless mush |
//! | [`repetition`] | Visible tiling, in either image |
//! | [`patch_entropy`] | How many distinct little arrangements the texture is built from |
//! | [`tone_shares`] | The share of the image spent in each of the target's ten tones |
//!
//! Each returns a similarity in 0..1 against the reference, and
//! [`Match::overall`] is their weighted mean. That single number is the one to
//! watch: it is the answer to "does this look like the target yet".
//!
//! ## The five added descriptors
//!
//! The first six answer "is this the same *material*". They are all statistics
//! of value and of local difference, and between them they can be satisfied by
//! an image with the right colours and the right amount of contrast arranged
//! into nothing in particular. Which is roughly what a texture regresses into.
//!
//! The five added below answer "is it the same material *at the same scale*,
//! made of the same marks". [`feature_scale`] is the one that has no substitute:
//! a field of correct grass drawn twice too large matches every value and
//! frequency statistic the original six compute — the histogram does not know
//! how big anything is — and looks obviously wrong beside the plate.
//! [`tone_shares`] is the same idea applied to brightness rather than to size,
//! and it is deliberately measured against the *same ten buckets* the palette
//! bake is fitted to, so that the sprite-level and image-level answers to "are
//! the tones distributed like the reference" can be read side by side.
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

/// Offsets the repetition search runs over, in pixels.
///
/// Starts well above zero: every image correlates with itself at a shift of one
/// pixel, and reporting that as tiling would say every texture ever made
/// repeats.
const REPEAT_RANGE: std::ops::Range<usize> = 8..96;

/// Size of the patches the vocabulary is built from.
const PATCH: usize = 4;

/// Normalised autocorrelation of the luminance at one offset.
fn autocorrelation(plate: &Plate, dx: usize, dy: usize) -> f32 {
    if dx >= plate.width || dy >= plate.height {
        return 0.0;
    }
    let mean = plate.luma.iter().sum::<f32>() / plate.luma.len().max(1) as f32;
    let (mut top, mut bottom) = (0.0f32, 0.0f32);
    // Strided, because this runs over dozens of offsets on a megapixel plate
    // and every third pixel measures the same correlation to three decimals.
    for y in (0..plate.height - dy).step_by(3) {
        for x in (0..plate.width - dx).step_by(3) {
            let a = plate.at(x, y) - mean;
            let b = plate.at(x + dx, y + dy) - mean;
            top += a * b;
            bottom += a * a;
        }
    }
    if bottom <= 1e-9 { 0.0 } else { top / bottom }
}

/// Strongest self-similarity at a non-trivial offset, in 0..1.
///
/// A texture that tiles has a sharp peak here at its tile size. A texture built
/// from placed, jittered, varied marks does not. Worth measuring on both plates
/// rather than only on ours: the reference is itself a tiling swatch, so the
/// honest question is whether we repeat *more* than it does.
pub fn repetition(plate: &Plate) -> f32 {
    let mut worst = 0.0f32;
    for offset in REPEAT_RANGE.step_by(4) {
        worst = worst
            .max(autocorrelation(plate, offset, 0))
            .max(autocorrelation(plate, 0, offset))
            .max(autocorrelation(plate, offset, offset));
    }
    worst.clamp(0.0, 1.0)
}

/// How big the marks are, in pixels.
///
/// The correlation *length*: the area under the autocorrelation curve. A texture
/// made of features roughly `n` pixels across stops correlating with itself at
/// about `n`, so the integral of the curve is proportional to `n`.
///
/// The obvious implementation — the offset at which correlation first drops
/// below a half — was tried first and is not usable here. Both plates are fine
/// grained enough that the crossing happens between one and three pixels, so
/// the metric had three possible values and quantised any real difference away
/// to nothing. An integral keeps its resolution at any grain, which matters
/// because this is the one descriptor in the file with no substitute: every
/// value and frequency statistic here is blind to scale, and grass drawn at
/// twice the right size satisfies all of them.
pub fn feature_scale(plate: &Plate) -> f32 {
    let mut length = 0.5;
    for offset in 1..48 {
        let across = (autocorrelation(plate, offset, 0) + autocorrelation(plate, 0, offset)) * 0.5;
        // Stop at the first crossing into anti-correlation. Past it the curve
        // is describing the *next* feature along rather than this one, and
        // integrating through it adds the gap to the mark.
        if across <= 0.0 {
            break;
        }
        length += across;
    }
    length
}

/// Distribution of edge directions, in eight bins.
///
/// Grass is combed: its marks run in a family of directions rather than in all
/// of them. This is [`anisotropy`] with the detail put back — anisotropy says
/// only whether horizontal beats vertical, which cannot tell a texture of
/// upward-fanning leaves from one of diagonal streaks.
pub fn orientation_histogram(plate: &Plate) -> [f32; 8] {
    let mut bins = [0.0f32; 8];
    for y in 1..plate.height.saturating_sub(1) {
        for x in 1..plate.width.saturating_sub(1) {
            let gx = plate.at(x + 1, y) - plate.at(x - 1, y);
            let gy = plate.at(x, y + 1) - plate.at(x, y - 1);
            let magnitude = (gx * gx + gy * gy).sqrt();
            if magnitude < 0.02 {
                continue;
            }
            // Folded to half a turn: an edge has an axis, not a direction, and
            // a mark running north-east is the same mark seen from the other
            // end.
            let mut angle = gy.atan2(gx);
            if angle < 0.0 {
                angle += std::f32::consts::PI;
            }
            let bin = ((angle / std::f32::consts::PI) * 8.0) as usize;
            bins[bin.min(7)] += magnitude;
        }
    }
    let total: f32 = bins.iter().sum();
    if total > 0.0 {
        for bin in &mut bins {
            *bin /= total;
        }
    }
    bins
}

/// Entropy of the texture's vocabulary of small patches, in bits.
///
/// Every four-by-four patch is quantised to a coarse pattern and counted. A
/// texture stamped from a handful of shapes has a small vocabulary; noise has an
/// enormous one. Neither extreme is the target — the reference is somewhere in
/// between, which is why this is compared rather than maximised.
pub fn patch_entropy(plate: &Plate) -> f32 {
    let mean = plate.luma.iter().sum::<f32>() / plate.luma.len().max(1) as f32;
    let mut counts = std::collections::BTreeMap::new();
    let mut total = 0u64;
    for y in (0..plate.height.saturating_sub(PATCH)).step_by(PATCH) {
        for x in (0..plate.width.saturating_sub(PATCH)).step_by(PATCH) {
            // Quantised against the patch's own mean rather than the image's,
            // so the vocabulary describes *shape* and is not simply a second
            // reading of the brightness histogram.
            let mut local = 0.0f32;
            for dy in 0..PATCH {
                for dx in 0..PATCH {
                    local += plate.at(x + dx, y + dy);
                }
            }
            local /= (PATCH * PATCH) as f32;
            let mut key = 0u16;
            for dy in 0..PATCH {
                for dx in 0..PATCH {
                    key <<= 1;
                    if plate.at(x + dx, y + dy) > local {
                        key |= 1;
                    }
                }
            }
            let _ = mean;
            *counts.entry(key).or_insert(0u64) += 1;
            total += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    let mut entropy = 0.0f32;
    for count in counts.values() {
        let p = *count as f32 / total as f32;
        entropy -= p * p.log2();
    }
    entropy
}

/// Share of the image spent in each of the art target's ten tones.
///
/// The same buckets `crate::atlas` scores the sprite sheet against, applied to
/// the finished frame. Both are needed: the sprites can be distributed exactly
/// right and the frame still come out wrong, because the ground shows between
/// them and overlapping clumps darken each other.
pub fn tone_shares(plate: &Plate) -> [f32; bw_grass::palette::TARGET_TONES] {
    let mut shares = [0.0f32; bw_grass::palette::TARGET_TONES];
    for &luma in &plate.luma {
        shares[bw_grass::palette::target_tone(luma)] += 1.0;
    }
    let total = plate.luma.len().max(1) as f32;
    for share in &mut shares {
        *share /= total;
    }
    shares
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
    pub scale: f32,
    pub orientation: f32,
    pub repetition: f32,
    pub vocabulary: f32,
    pub tones: f32,
    pub overall: f32,
    /// Feature size of the rendered frame and of the plate, in pixels.
    ///
    /// Carried out alongside the similarity because the similarity alone says
    /// only that they disagree, and the fix depends entirely on which way: too
    /// large is a camera or a sprite-size change, too small is a detail budget.
    pub scale_pixels: (f32, f32),
    /// Raw repetition of each, for the same reason.
    pub repetition_raw: (f32, f32),
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

    let scale_pixels = (feature_scale(&a), feature_scale(&b));
    let scale = ratio_similarity(scale_pixels.0, scale_pixels.1);
    let orientation =
        histogram_intersection(&orientation_histogram(&a), &orientation_histogram(&b));
    let repetition_raw = (repetition(&a), repetition(&b));
    // Repeating *less* than the plate is not a defect — the plate is a tiling
    // swatch and the field is not — so this is one-sided. Only excess counts.
    let repetition = if repetition_raw.0 <= repetition_raw.1 {
        1.0
    } else {
        ratio_similarity(repetition_raw.1, repetition_raw.0)
    };
    let vocabulary = ratio_similarity(patch_entropy(&a), patch_entropy(&b));
    let tones = histogram_intersection(&tone_shares(&a), &tone_shares(&b));

    // Weighted toward what a person notices first, and reweighted when the
    // scale and orientation descriptors were added — the original six between
    // them could be fully satisfied by an image made of the right colours
    // arranged into nothing in particular, which is precisely how a generated
    // texture regresses.
    //
    // Value and chroma still lead, because "wrong green" and "wrong exposure"
    // are what anyone says first. Scale is third and heavy: grass drawn at the
    // wrong size is the most obvious failure on this list and the one every
    // other descriptor is blind to.
    let overall = 0.20 * value
        + 0.18 * chroma
        + 0.14 * scale
        + 0.12 * detail
        + 0.10 * tones
        + 0.08 * contrast
        + 0.06 * orientation
        + 0.05 * clusters
        + 0.04 * grain
        + 0.03 * vocabulary;

    Match {
        value,
        chroma,
        detail,
        contrast,
        grain,
        clusters,
        scale,
        orientation,
        repetition,
        vocabulary,
        tones,
        overall,
        scale_pixels,
        repetition_raw,
    }
}

/// Overlap of two normalised histograms, in 0..1.
fn histogram_intersection(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| x.min(*y))
        .sum::<f32>()
        .clamp(0.0, 1.0)
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

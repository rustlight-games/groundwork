//! How far a plate has moved from the plate it used to be.
//!
//! This is the measurement an optimisation is judged by. It answers one
//! question — *did the picture change, and by how much* — and it answers it
//! against **our own previous output**, not against reference art. That
//! distinction is the whole design.
//!
//! Scoring against art is a taste exercise: it needs descriptors, because the
//! candidate and the target share no placement and cannot be compared pixel for
//! pixel, and descriptors can all agree while the image looks wrong. Scoring
//! against yesterday's own bake is not a taste exercise at all. The two images
//! are the same seed at the same place at the same scale, so every pixel has a
//! counterpart, and "unchanged" has an exact meaning: zero.
//!
//! That makes the numbers here far stricter than any aesthetic metric, and it
//! makes them usable as a gate. An optimisation that reorders arithmetic moves
//! a handful of pixels by one 8-bit step. An optimisation that drops a
//! supersampling level, or coarsens a lattice, or skips a shading term, does
//! not — and it shows up here immediately, at every zoom level it damaged.
//!
//! ## What each number is for
//!
//! | Number | Catches |
//! |---|---|
//! | [`Similarity::rmse`] / [`Similarity::psnr`] | Overall magnitude of the change |
//! | [`Similarity::ssim`] | Structural change — blurring, smearing, lost edges |
//! | [`Similarity::p99_delta`] | The typical worst pixel, without one outlier setting it |
//! | [`Similarity::max_delta`] | The single worst pixel |
//! | [`Similarity::changed`] | How *much* of the plate moved, rather than how far |
//! | [`Similarity::luma_drift`] | Signed: the whole field went pale or muddy |
//! | [`Similarity::detail_ratio`] | Fine texture lost or gained — the blur-it-away failure |
//!
//! [`Similarity::detail_ratio`] deserves its place beside SSIM rather than
//! under it. SSIM punishes blur, but it punishes it as a fraction of local
//! contrast, so a plate that lost a fifth of its stroke texture uniformly can
//! still score well. The ratio says so directly, in the one direction that
//! matters: below one is smoother than it was, and smoother than it was is the
//! shape almost every grass optimisation takes when it goes wrong.
//!
//! ## Everything is compared after quantisation
//!
//! Both sides are rounded through the 8-bit encoding a page is stored in before
//! anything is measured — see [`quantise`]. A baseline read back from a PNG has
//! already been through that rounding, and a candidate still in memory has not,
//! so comparing them directly would report half a step of encoding noise as a
//! difference. Rounding both puts the comparison in the space the player
//! actually sees, and makes [`Similarity::changed`] a true count of stored bytes
//! that moved.

use bevy::prelude::*;

use crate::palette;
use crate::surface::blur;

/// Radius of the local window structural similarity is computed over.
///
/// Eleven pixels across. Large enough to hold a stroke and its neighbours,
/// which is the scale the comparison has to work at: a window smaller than one
/// blade measures whether that blade moved, and every blade moves when anything
/// does.
pub const SSIM_RADIUS: usize = 5;

/// Radius separating fine texture from structure for [`Similarity::detail_ratio`].
///
/// Four pixels, which is the stroke language — a blade is about twenty-four
/// cache pixels long and two or three wide, so this is the scale at which
/// "there are individual marks here" stops being true.
pub const DETAIL_RADIUS: usize = 4;

/// How far a channel must move for a pixel to count as changed, in 0..1.
///
/// Two 8-bit steps. One step is what rounding alone produces when arithmetic is
/// reassociated, and counting that would make every honest refactor look like a
/// visual change.
pub const CHANGED_STEP: f32 = 2.0 / 255.0;

/// How much two plates of the same size differ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Similarity {
    pub width: usize,
    pub height: usize,
    /// Root mean square error across all three channels, in 0..1.
    pub rmse: f32,
    /// Peak signal-to-noise ratio in decibels; infinite when identical.
    pub psnr: f32,
    /// Mean structural similarity over luminance, 1.0 being identical.
    pub ssim: f32,
    /// 99th percentile of the per-pixel largest channel difference.
    pub p99_delta: f32,
    /// The single largest channel difference anywhere on the plate.
    pub max_delta: f32,
    /// Share of pixels where any channel moved by at least [`CHANGED_STEP`].
    pub changed: f32,
    /// Signed change in mean luminance: positive is brighter than the baseline.
    pub luma_drift: f32,
    /// Fine texture energy relative to the baseline; below one is smoother.
    pub detail_ratio: f32,
}

/// A plain-language reading of a [`Similarity`].
///
/// Bands rather than a threshold, because the useful question is never "did it
/// pass" — it is "how much did I spend". An optimisation landing at
/// [`Verdict::Identical`] cost nothing; one at [`Verdict::Close`] cost something
/// a person should look at before accepting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// Byte for byte the same picture.
    Identical,
    /// Different only where arithmetic rounds differently. Nothing to see.
    Imperceptible,
    /// Visibly the same field. Worth a glance, not worth an argument.
    Close,
    /// The look has moved. Look at it before accepting the speed.
    Drifted,
    /// A different picture.
    Changed,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Verdict::Identical => "identical",
            Verdict::Imperceptible => "imperceptible",
            Verdict::Close => "close",
            Verdict::Drifted => "drifted",
            Verdict::Changed => "changed",
        })
    }
}

impl Similarity {
    /// Which band this comparison falls in.
    ///
    /// Structural similarity leads, because it is the number that tracks what a
    /// person notices; the percentile delta is the tie-break, because SSIM is
    /// generous about a change that is uniform and small, and a uniform small
    /// change is exactly how a tone shift arrives.
    pub fn verdict(&self) -> Verdict {
        if self.max_delta == 0.0 {
            return Verdict::Identical;
        }
        let steps = self.p99_delta * 255.0;
        if self.ssim >= 0.9995 && steps <= 1.0 {
            Verdict::Imperceptible
        } else if self.ssim >= 0.990 && steps <= 6.0 {
            Verdict::Close
        } else if self.ssim >= 0.960 {
            Verdict::Drifted
        } else {
            Verdict::Changed
        }
    }
}

/// Round a plate through the 8-bit encoding it is stored in.
///
/// The comparison is only meaningful in one space, and this is it: what a page
/// actually holds after `palette::to_bytes`. Everything measured below runs on
/// the output of this function on both sides.
pub fn quantise(colours: &[Vec3]) -> Vec<Vec3> {
    colours
        .iter()
        .map(|c| {
            let bytes = palette::to_bytes(*c);
            Vec3::new(bytes[0] as f32, bytes[1] as f32, bytes[2] as f32) / 255.0
        })
        .collect()
}

#[inline]
fn luma(c: Vec3) -> f32 {
    c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
}

fn deviation(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32).sqrt()
}

/// Compare a candidate plate against a baseline of the same size.
///
/// Both are quantised first, so it does not matter whether either side came
/// from a PNG or straight out of the baker.
pub fn compare(candidate: &[Vec3], baseline: &[Vec3], width: usize, height: usize) -> Similarity {
    let empty = Similarity {
        width,
        height,
        rmse: 0.0,
        psnr: f32::INFINITY,
        ssim: 1.0,
        p99_delta: 0.0,
        max_delta: 0.0,
        changed: 0.0,
        luma_drift: 0.0,
        detail_ratio: 1.0,
    };
    let count = width * height;
    if count == 0 || candidate.len() < count || baseline.len() < count {
        return empty;
    }

    let candidate = quantise(&candidate[..count]);
    let baseline = quantise(&baseline[..count]);

    let mut square_error = 0.0f64;
    let mut worst = Vec::with_capacity(count);
    let mut changed = 0usize;
    for (a, b) in candidate.iter().zip(&baseline) {
        let difference = *a - *b;
        square_error += (difference.length_squared() / 3.0) as f64;
        let peak = difference.abs().max_element();
        if peak >= CHANGED_STEP {
            changed += 1;
        }
        worst.push(peak);
    }
    let rmse = (square_error / count as f64).sqrt() as f32;
    worst.sort_by(f32::total_cmp);

    let candidate_luma: Vec<f32> = candidate.iter().map(|c| luma(*c)).collect();
    let baseline_luma: Vec<f32> = baseline.iter().map(|c| luma(*c)).collect();
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;

    // Fine texture on each side, measured identically: how much luminance
    // survives having its own neighbourhood subtracted.
    let residual = |source: &[f32]| {
        let smooth = blur(source, width, height, DETAIL_RADIUS);
        let residual: Vec<f32> = source.iter().zip(&smooth).map(|(a, b)| a - b).collect();
        deviation(&residual)
    };
    let baseline_detail = residual(&baseline_luma);

    Similarity {
        width,
        height,
        rmse,
        psnr: if rmse > 0.0 {
            20.0 * (1.0 / rmse).log10()
        } else {
            f32::INFINITY
        },
        ssim: ssim(&candidate_luma, &baseline_luma, width, height),
        p99_delta: worst[(count as f32 * 0.99) as usize % count],
        max_delta: worst[count - 1],
        changed: changed as f32 / count as f32,
        luma_drift: mean(&candidate_luma) - mean(&baseline_luma),
        detail_ratio: if baseline_detail > 1.0e-6 {
            residual(&candidate_luma) / baseline_detail
        } else {
            1.0
        },
    }
}

/// Mean structural similarity over two luminance images.
///
/// The standard formulation, with the local statistics taken through
/// [`crate::surface::blur`] rather than a Gaussian. The window shape moves the
/// third decimal and nothing else; using the blur already in the crate means
/// the comparison has no machinery of its own to go wrong.
pub fn ssim(candidate: &[f32], baseline: &[f32], width: usize, height: usize) -> f32 {
    if width == 0 || height == 0 {
        return 1.0;
    }
    // The usual stabilising constants for data on 0..1.
    const C1: f32 = 0.01 * 0.01;
    const C2: f32 = 0.03 * 0.03;

    let square = |v: &[f32]| -> Vec<f32> { v.iter().map(|x| x * x).collect() };
    let product: Vec<f32> = candidate.iter().zip(baseline).map(|(a, b)| a * b).collect();

    let mean_a = blur(candidate, width, height, SSIM_RADIUS);
    let mean_b = blur(baseline, width, height, SSIM_RADIUS);
    let mean_aa = blur(&square(candidate), width, height, SSIM_RADIUS);
    let mean_bb = blur(&square(baseline), width, height, SSIM_RADIUS);
    let mean_ab = blur(&product, width, height, SSIM_RADIUS);

    let mut total = 0.0f64;
    for i in 0..width * height {
        let (ma, mb) = (mean_a[i], mean_b[i]);
        // Clamped at zero: these are differences of blurred squares, and a
        // patch of constant colour can land a few ulps below it.
        let va = (mean_aa[i] - ma * ma).max(0.0);
        let vb = (mean_bb[i] - mb * mb).max(0.0);
        let covariance = mean_ab[i] - ma * mb;
        let numerator = (2.0 * ma * mb + C1) * (2.0 * covariance + C2);
        let denominator = (ma * ma + mb * mb + C1) * (va + vb + C2);
        total += (numerator / denominator) as f64;
    }
    (total / (width * height) as f64) as f32
}

/// A printable table of named comparisons.
///
/// Ordered worst first, because a run of twenty views has one that matters and
/// nineteen that do not, and the one that matters is always the one that moved.
pub fn table(rows: &[(String, Similarity)]) -> String {
    let mut rows: Vec<&(String, Similarity)> = rows.iter().collect();
    rows.sort_by(|a, b| a.1.ssim.total_cmp(&b.1.ssim));

    let width = rows
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let mut out = format!(
        "{:<width$} {:>8} {:>8} {:>8} {:>8} {:>9} {:>8} {:>7}  {}\n",
        "view", "ssim", "psnr", "rmse", "p99Δ", "changed", "detail", "lumaΔ", "verdict"
    );
    for (name, s) in rows {
        let psnr = if s.psnr.is_finite() {
            format!("{:.1}", s.psnr)
        } else {
            "  ∞".to_string()
        };
        out.push_str(&format!(
            "{name:<width$} {:>8.5} {psnr:>8} {:>8.5} {:>7.1}× {:>8.3}% {:>7.3}× {:>+7.4}  {}\n",
            s.ssim,
            s.rmse,
            s.p99_delta * 255.0,
            s.changed * 100.0,
            s.detail_ratio,
            s.luma_drift,
            s.verdict(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(f: impl Fn(usize, usize) -> f32, size: usize) -> Vec<Vec3> {
        (0..size * size)
            .map(|i| Vec3::splat(f(i % size, i / size)))
            .collect()
    }

    fn noisy(size: usize) -> Vec<Vec3> {
        plate(|x, y| 0.4 + ((x * 7 + y * 13) % 23) as f32 * 0.012, size)
    }

    #[test]
    fn a_plate_is_identical_to_itself() {
        let a = noisy(64);
        let s = compare(&a, &a, 64, 64);
        assert_eq!(s.rmse, 0.0);
        assert!(s.psnr.is_infinite());
        assert!((s.ssim - 1.0).abs() < 1.0e-5, "{}", s.ssim);
        assert_eq!(s.changed, 0.0);
        assert_eq!(s.verdict(), Verdict::Identical);
        assert!((s.detail_ratio - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn rounding_noise_reads_as_imperceptible() {
        // What an honest refactor produces: arithmetic reassociated, a scattering
        // of pixels landing one 8-bit step away. If this reported as a visual
        // change the gate would be useless.
        let a = noisy(64);
        let b: Vec<Vec3> = a
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i % 7 == 0 {
                    *c + Vec3::splat(1.0 / 255.0)
                } else {
                    *c
                }
            })
            .collect();
        let s = compare(&b, &a, 64, 64);
        assert_ne!(s.max_delta, 0.0);
        assert_eq!(s.verdict(), Verdict::Imperceptible, "{s:?}");
    }

    #[test]
    fn blurring_a_plate_shows_up_as_lost_detail() {
        // The failure this exists to catch: an optimisation that is faster
        // because it stopped resolving fine texture.
        let a = noisy(96);
        let luma: Vec<f32> = a.iter().map(|c| luma(*c)).collect();
        let smoothed = blur(&luma, 96, 96, 3);
        let b: Vec<Vec3> = smoothed.iter().map(|v| Vec3::splat(*v)).collect();

        let s = compare(&b, &a, 96, 96);
        assert!(s.detail_ratio < 0.5, "detail_ratio {}", s.detail_ratio);
        assert!(s.ssim < 0.99, "ssim {}", s.ssim);
        assert!(s.verdict() >= Verdict::Drifted, "{s:?}");
        // And it says which way it went, which "different" alone would not.
        assert!(s.detail_ratio < 1.0);
    }

    #[test]
    fn a_uniform_lift_is_reported_as_signed_drift() {
        let a = noisy(64);
        let b: Vec<Vec3> = a.iter().map(|c| *c + Vec3::splat(0.05)).collect();
        let s = compare(&b, &a, 64, 64);
        assert!(s.luma_drift > 0.04, "{}", s.luma_drift);
        assert!(compare(&a, &b, 64, 64).luma_drift < -0.04);
        // Texture is untouched by a constant offset, and must read that way.
        assert!((s.detail_ratio - 1.0).abs() < 0.02, "{}", s.detail_ratio);
    }

    #[test]
    fn changed_counts_pixels_and_p99_measures_them() {
        // A tenth of the plate moved a long way; the rest not at all. The two
        // numbers separate "how much moved" from "how far", which no single
        // measure does.
        let a = noisy(100);
        let b: Vec<Vec3> = a
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i % 10 == 0 {
                    *c + Vec3::splat(0.2)
                } else {
                    *c
                }
            })
            .collect();
        let s = compare(&b, &a, 100, 100);
        assert!((s.changed - 0.10).abs() < 0.01, "{}", s.changed);
        assert!(s.max_delta > 0.19);
    }

    #[test]
    fn quantisation_alone_is_not_a_difference() {
        // A baseline read back from a PNG has been rounded; a candidate in
        // memory has not. Comparing them must not report the rounding.
        let raw: Vec<Vec3> = (0..64 * 64)
            .map(|i| Vec3::splat(0.4 + (i % 37) as f32 * 0.0031))
            .collect();
        let stored = quantise(&raw);
        let s = compare(&raw, &stored, 64, 64);
        assert_eq!(s.max_delta, 0.0);
        assert_eq!(s.verdict(), Verdict::Identical);
    }

    #[test]
    fn mismatched_sizes_do_not_panic() {
        let a = noisy(16);
        assert_eq!(compare(&a, &[], 16, 16).verdict(), Verdict::Identical);
        assert_eq!(compare(&a, &a, 0, 0).width, 0);
    }
}

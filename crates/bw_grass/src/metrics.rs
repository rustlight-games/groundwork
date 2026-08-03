//! Descriptors for comparing a baked plate against the reference art.
//!
//! Matching a painting is a numerical exercise pretending to be a taste
//! exercise, and it fails the same way generators always fail: by looking
//! roughly right while three separate properties drift. So every judgement is
//! computed identically on both images and reported side by side.
//!
//! Two of these carry more weight than the rest.
//!
//! **The detail ladder.** Standard deviation of luminance minus its own blur, at
//! six radii. Each rung diagnoses a different subsystem: two to four pixels is
//! the stroke language, twelve to twenty is clumps and cavities, fifty and up is
//! mound distribution and regional colour. A plate that matches at 64 and misses
//! at 4 has the right composition and the wrong brush, and adding more blades
//! will not fix it — which is precisely the mistake this ladder exists to stop
//! anyone making.
//!
//! **The blurred ladder.** Standard deviation of the blur itself, same radii.
//! The detail figure says how much local texture there is; this says how much
//! *structure* survives it. Grass with the right texture and no structure reads
//! as carpet, and the two numbers separate those cases cleanly.
//!
//! These are proxies. They catch drift between the times a person looks at the
//! output; they do not decide whether it is good.

use bevy::prelude::*;

/// Blur radii the ladders are measured at, in pixels.
pub const RADII: [usize; 6] = [2, 4, 8, 16, 32, 64];

/// One image, described.
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptors {
    pub width: usize,
    pub height: usize,
    /// Mean, standard deviation of luminance.
    pub luma_mean: f32,
    pub luma_deviation: f32,
    /// Luminance at the 1st, 5th, 50th, 95th and 99th percentiles.
    pub luma_percentiles: [f32; 5],
    /// Mean of each linear channel.
    pub channel_means: Vec3,
    /// Mean HSV saturation, and the 5th and 95th percentiles of hue in degrees.
    pub saturation: f32,
    pub hue_mean: f32,
    pub hue_spread: (f32, f32),
    /// Standard deviation of luminance minus its blur, per [`RADII`].
    pub detail: [f32; 6],
    /// Standard deviation of the blur itself, per [`RADII`].
    pub structure: [f32; 6],
    /// Share of pixels brighter than 0.62 luminance — the tip glints.
    ///
    /// Read this one against [`Descriptors::luma_percentiles`] rather than on its
    /// own. The threshold sits almost exactly at the reference's own 95th
    /// percentile, so the measure is a knife edge there: moving `p95` by four
    /// percent moves this by a third. That makes it good at catching a plate
    /// whose highlights have genuinely run away, and badly behaved as a tuning
    /// target — chase it directly and it will walk the whole tonal range around
    /// to satisfy a difference that is within a few percent on every percentile.
    pub bright: f32,
    /// Share darker than 0.20 — the cavities.
    pub dark: f32,
    /// Share reading as exposed soil rather than as vegetation.
    pub soil: f32,
    /// Mean gradient magnitude: how busy the plate is overall.
    pub busyness: f32,
}

#[inline]
fn luma(c: Vec3) -> f32 {
    c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = (fraction * (sorted.len() - 1) as f32).round() as usize;
    sorted[position.min(sorted.len() - 1)]
}

fn deviation(values: &[f32]) -> f32 {
    let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
    (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len().max(1) as f32)
        .sqrt()
}

/// Describe an image.
pub fn describe(pixels: &[Vec3], width: usize, height: usize) -> Descriptors {
    let brightness: Vec<f32> = pixels.iter().map(|c| luma(*c)).collect();
    let mut sorted = brightness.clone();
    sorted.sort_by(f32::total_cmp);

    let count = pixels.len().max(1) as f32;
    let channel_means = pixels.iter().fold(Vec3::ZERO, |a, b| a + *b) / count;

    let (mut saturation, mut hue_total) = (0.0f32, 0.0f32);
    let mut hues = Vec::with_capacity(pixels.len());
    let (mut bright, mut dark, mut soil) = (0usize, 0usize, 0usize);
    for (colour, &l) in pixels.iter().zip(&brightness) {
        let high = colour.x.max(colour.y).max(colour.z);
        let low = colour.x.min(colour.y).min(colour.z);
        let delta = (high - low).max(1.0e-6);
        saturation += delta / high.max(1.0e-6);
        let hue = if high == colour.x {
            ((colour.y - colour.z) / delta).rem_euclid(6.0)
        } else if high == colour.y {
            (colour.z - colour.x) / delta + 2.0
        } else {
            (colour.x - colour.y) / delta + 4.0
        } * 60.0;
        hues.push(hue);
        hue_total += hue;
        if l > 0.62 {
            bright += 1;
        }
        if l < 0.20 {
            dark += 1;
        }
        // Exposed earth: the green channel stops leading by much, and the
        // colour desaturates. Both conditions, because a bright tip satisfies
        // the first on its own.
        if colour.y < colour.x * 1.20 && delta / high.max(1.0e-6) < 0.80 {
            soil += 1;
        }
    }
    hues.sort_by(f32::total_cmp);

    let mut detail = [0.0f32; 6];
    let mut structure = [0.0f32; 6];
    for (slot, radius) in RADII.iter().enumerate() {
        let blurred = crate::surface::blur(&brightness, width, height, *radius);
        let residual: Vec<f32> = brightness
            .iter()
            .zip(&blurred)
            .map(|(a, b)| a - b)
            .collect();
        detail[slot] = deviation(&residual);
        structure[slot] = deviation(&blurred);
    }

    let mut gradient = 0.0f32;
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let i = y * width + x;
            let dx = brightness[i + 1] - brightness[i - 1];
            let dy = brightness[i + width] - brightness[i - width];
            gradient += (dx * dx + dy * dy).sqrt() * 0.5;
        }
    }
    let interior = ((width.saturating_sub(2)) * (height.saturating_sub(2))).max(1) as f32;

    Descriptors {
        width,
        height,
        luma_mean: brightness.iter().sum::<f32>() / count,
        luma_deviation: deviation(&brightness),
        luma_percentiles: [
            percentile(&sorted, 0.01),
            percentile(&sorted, 0.05),
            percentile(&sorted, 0.50),
            percentile(&sorted, 0.95),
            percentile(&sorted, 0.99),
        ],
        channel_means,
        saturation: saturation / count,
        hue_mean: hue_total / count,
        hue_spread: (percentile(&hues, 0.05), percentile(&hues, 0.95)),
        detail,
        structure,
        bright: bright as f32 / count,
        dark: dark as f32 / count,
        soil: soil as f32 / count,
        busyness: gradient / interior,
    }
}

/// A printable comparison of two descriptions.
///
/// The candidate first, the target second, because the question being asked is
/// always "how far off am I", never "how do these two differ".
pub fn compare(candidate: &Descriptors, target: &Descriptors) -> String {
    let mut out = format!(
        "{:<22} {:>9} {:>9} {:>9} {:>9}\n",
        "descriptor", "candidate", "target", "delta", "relative"
    );
    let mut row = |name: &str, a: f32, b: f32| {
        let delta = a - b;
        let relative = if b.abs() > 1.0e-6 {
            delta / b.abs() * 100.0
        } else {
            f32::NAN
        };
        out.push_str(&format!(
            "{name:<22} {a:>9.4} {b:>9.4} {delta:>+9.4} {relative:>+8.1}%\n"
        ));
    };
    row("luma.mean", candidate.luma_mean, target.luma_mean);
    row(
        "luma.deviation",
        candidate.luma_deviation,
        target.luma_deviation,
    );
    for (slot, label) in ["p01", "p05", "p50", "p95", "p99"].iter().enumerate() {
        row(
            &format!("luma.{label}"),
            candidate.luma_percentiles[slot],
            target.luma_percentiles[slot],
        );
    }
    row(
        "channel.red",
        candidate.channel_means.x,
        target.channel_means.x,
    );
    row(
        "channel.green",
        candidate.channel_means.y,
        target.channel_means.y,
    );
    row(
        "channel.blue",
        candidate.channel_means.z,
        target.channel_means.z,
    );
    row("saturation", candidate.saturation, target.saturation);
    row("hue.mean", candidate.hue_mean, target.hue_mean);
    row("hue.p05", candidate.hue_spread.0, target.hue_spread.0);
    row("hue.p95", candidate.hue_spread.1, target.hue_spread.1);
    for (slot, radius) in RADII.iter().enumerate() {
        row(
            &format!("detail.r{radius}"),
            candidate.detail[slot],
            target.detail[slot],
        );
    }
    for (slot, radius) in RADII.iter().enumerate() {
        row(
            &format!("structure.r{radius}"),
            candidate.structure[slot],
            target.structure[slot],
        );
    }
    row("bright.share", candidate.bright, target.bright);
    row("dark.share", candidate.dark, target.dark);
    row("soil.share", candidate.soil, target.soil);
    row("busyness", candidate.busyness, target.busyness);
    out
}

/// One number for how far a candidate is from a target.
///
/// A weighted mean of relative errors over the descriptors that actually decide
/// whether two plates read as the same art. Useful for hill-climbing a parameter
/// and useless as a verdict; a low score is a licence to go and look, not a
/// substitute for looking.
pub fn distance(candidate: &Descriptors, target: &Descriptors) -> f32 {
    let relative = |a: f32, b: f32| {
        if b.abs() > 1.0e-6 {
            ((a - b) / b.abs()).abs()
        } else {
            0.0
        }
    };
    let mut total = 0.0;
    let mut weight = 0.0;
    let mut add = |value: f32, w: f32| {
        total += value * w;
        weight += w;
    };

    add(relative(candidate.luma_mean, target.luma_mean), 3.0);
    add(
        relative(candidate.luma_deviation, target.luma_deviation),
        2.0,
    );
    add(relative(candidate.saturation, target.saturation), 2.0);
    add(relative(candidate.hue_mean, target.hue_mean), 2.0);
    for slot in 0..RADII.len() {
        add(relative(candidate.detail[slot], target.detail[slot]), 2.0);
        add(
            relative(candidate.structure[slot], target.structure[slot]),
            1.5,
        );
    }
    add(relative(candidate.bright, target.bright), 1.5);
    add(relative(candidate.dark, target.dark), 0.5);
    total / weight.max(1.0e-6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(f: impl Fn(usize, usize) -> Vec3, size: usize) -> Vec<Vec3> {
        (0..size * size).map(|i| f(i % size, i / size)).collect()
    }

    #[test]
    fn an_image_is_zero_distance_from_itself() {
        let pixels = plate(
            |x, y| Vec3::new(0.2, 0.4 + ((x * y) % 7) as f32 * 0.02, 0.05),
            64,
        );
        let d = describe(&pixels, 64, 64);
        assert!(distance(&d, &d) < 1.0e-4);
    }

    #[test]
    fn detail_separates_a_busy_plate_from_a_smooth_one() {
        // Deliberately not a one-pixel checkerboard. `busyness` is a *central*
        // difference, and on a checkerboard both taps land on the same parity,
        // so the busiest possible image measures as perfectly flat. Three-pixel
        // bars are the smallest pattern the measurement can actually see.
        let busy = describe(
            &plate(
                |x, y| Vec3::splat(((x / 3 + y / 3) % 2) as f32 * 0.5 + 0.2),
                64,
            ),
            64,
            64,
        );
        let smooth = describe(&plate(|_, _| Vec3::splat(0.45), 64), 64, 64);
        assert!(busy.detail[0] > smooth.detail[0] + 0.1);
        assert!(busy.busyness > smooth.busyness);
    }

    #[test]
    fn structure_separates_a_mounded_plate_from_a_flat_one() {
        // The distinction the two ladders exist to make: same local texture,
        // different large-scale organisation.
        let noisy = |x: usize, y: usize| ((x * 7 + y * 13) % 11) as f32 * 0.01;
        let flat = describe(&plate(|x, y| Vec3::splat(0.4 + noisy(x, y)), 96), 96, 96);
        let mounded = describe(
            &plate(
                |x, y| {
                    let wave = ((x as f32 / 24.0).sin() * (y as f32 / 24.0).cos()) * 0.12;
                    Vec3::splat(0.4 + noisy(x, y) + wave)
                },
                96,
            ),
            96,
            96,
        );
        assert!(mounded.structure[4] > flat.structure[4] + 0.02);
        assert!((mounded.detail[0] - flat.detail[0]).abs() < 0.02);
    }

    #[test]
    fn soil_is_detected_but_grass_is_not() {
        let grass = describe(&plate(|_, _| Vec3::new(0.29, 0.44, 0.04), 32), 32, 32);
        let earth = describe(&plate(|_, _| Vec3::new(0.47, 0.44, 0.17), 32), 32, 32);
        assert!(grass.soil < 0.01);
        assert!(earth.soil > 0.99);
    }
}

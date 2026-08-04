//! What a plate looks like, in absolute numbers, against a target.
//!
//! This is the second of the two measurements and it is the opposite of the
//! first. [`crate::compare`] asks *did the picture move*, against our own
//! previous output, and it is the right gate for an optimisation. It is the
//! wrong gate for a deliberate look change: the answer is always "yes,
//! completely", which carries no information about whether the new look is the
//! one we wanted.
//!
//! So this module asks *what is the picture*, in numbers that can be computed
//! for reference art and for our own bake alike, and compared. No pixel
//! correspondence is needed or wanted — the two images share no placement.
//!
//! ## Why these numbers and not descriptors
//!
//! The crate docs warn that descriptors are useless for deciding what an
//! optimisation cost, and that stands. This is the case they *are* for: a look
//! change, judged against art. The set is chosen so each entry fails for a
//! different reason and none of them can be satisfied by cheating at another's
//! expense.
//!
//! | Number | Fails when |
//! |---|---|
//! | [`Critique::median_luminance`] | The whole field sits too bright — the "lumo" failure |
//! | [`Critique::deep_shadow`] | There is no true dark: no root channels, no canopy interior |
//! | [`Critique::highlight`] | Bright paint has spread from tips onto whole blades |
//! | [`Critique::chroma`] | Over-saturated, and usually over-yellow with it |
//! | [`Critique::hue_spread`] | One green everywhere; shadow and light share a hue |
//! | [`Critique::gradient_energy`] | Blade silhouettes are soft or absent |
//! | [`Critique::detail_energy`] | The plate is diffuse — a carpet rather than marks |
//! | [`Critique::coherence`] | Blades do not align into tufts; direction is per-blade noise |
//!
//! The last one is the one that is hard to fake and easy to lose. It is a
//! structure-tensor measurement over windows a few tufts wide, so it reads the
//! *middle* scale — the one the carpet failure destroys while leaving both the
//! blade scale and the region scale intact.
//!
//! ## Everything tonal is measured in CIE L\*
//!
//! Not in the 0..1 the plate is stored in, and not in linear light. Percentages
//! of an image below a darkness threshold are a statement about what a viewer
//! sees, and sRGB's 8-bit code values are not proportional to that while L\* is
//! by construction. The two luminance figures are linear because they are ratios
//! — "twice as bright" has to mean twice the light.

use glam::Vec3;

/// The measured character of one plate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Critique {
    pub width: usize,
    pub height: usize,
    /// Median relative luminance, linear. The single most diagnostic number.
    pub median_luminance: f32,
    /// Mean relative luminance, linear.
    pub mean_luminance: f32,
    /// Fraction of pixels below [`DEEP_SHADOW_LSTAR`].
    pub deep_shadow: f32,
    /// Fraction of pixels above [`HIGHLIGHT_LSTAR`].
    pub highlight: f32,
    /// Mean CIE Lab chroma, `sqrt(a² + b²)`.
    pub chroma: f32,
    /// Circular standard deviation of Lab hue, in degrees.
    pub hue_spread: f32,
    /// Mean Lab hue angle, in degrees. 90° is yellow, 135° is yellow-green.
    pub hue_mean: f32,
    /// Mean Sobel gradient magnitude of L\*, per pixel.
    pub gradient_energy: f32,
    /// Mean absolute Laplacian of L\*, per pixel.
    pub detail_energy: f32,
    /// Structure-tensor coherence over [`COHERENCE_WINDOW`] windows, 0..1.
    pub coherence: f32,
    /// L\* at the 5th, 25th, 50th, 75th and 95th percentiles.
    pub ladder: [f32; 5],
    /// Mean Lab chroma of the highlight population alone.
    ///
    /// Separate from [`Critique::chroma`] because the two fail apart, and the
    /// whole-image figure hides it. A canopy can hold a perfectly good average
    /// saturation while every one of its *bright* pixels is white — which is
    /// what a specular lobe on a coloured surface produces, and what tells the
    /// difference between a blade lit through green tissue and one varnished
    /// with white light.
    pub highlight_chroma: f32,
    /// Fraction of pixels with any channel at or above full scale.
    ///
    /// Clipping is not a look, it is lost information, and on a training corpus
    /// it is lost information the network will faithfully reproduce.
    pub clipped: f32,
}

/// Below this L\*, a pixel counts as true shadow.
///
/// Twenty is dark enough that no amount of ordinary shading reaches it — it has
/// to come from occlusion or from a cast shadow. That is exactly why it is the
/// threshold: it cannot be satisfied by grading the whole plate down, because
/// grading down moves the median with it and the median has its own band.
pub const DEEP_SHADOW_LSTAR: f32 = 20.0;

/// Above this L\*, a pixel counts as a highlight.
pub const HIGHLIGHT_LSTAR: f32 = 55.0;

/// Side of the window local directional coherence is measured over.
///
/// Thirty-two pixels holds a few blades and about one tuft at the scale the
/// plate is authored at. Smaller windows measure whether a single blade is
/// straight, which is nearly always true and tells you nothing; much larger
/// ones average across tufts whose directions genuinely differ and report
/// incoherence that is not a fault.
pub const COHERENCE_WINDOW: usize = 32;

impl Critique {
    /// Measure a plate stored as display-referred RGB in 0..1.
    pub fn of(pixels: &[Vec3], width: usize, height: usize) -> Self {
        assert_eq!(pixels.len(), width * height, "critique: size mismatch");

        let mut luminance = Vec::with_capacity(pixels.len());
        let mut lstar = Vec::with_capacity(pixels.len());
        let mut chroma_sum = 0.0f64;
        // Hue is an angle, so it is averaged as a unit vector. Averaging degrees
        // directly puts the mean of 350° and 10° at 180°.
        let (mut hue_x, mut hue_y) = (0.0f64, 0.0f64);

        for pixel in pixels {
            let lab = lab_of(*pixel);
            luminance.push(relative_luminance(*pixel));
            lstar.push(lab[0]);
            let c = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
            chroma_sum += c as f64;
            if c > 1.0e-4 {
                hue_x += (lab[1] / c) as f64;
                hue_y += (lab[2] / c) as f64;
            }
        }

        let count = pixels.len().max(1);
        let n = count as f64;
        let resultant = ((hue_x / n).powi(2) + (hue_y / n).powi(2)).sqrt();
        // Circular standard deviation. A resultant length of one means every
        // pixel shares a hue; zero means they are spread over the whole wheel.
        let hue_spread = (-2.0 * resultant.max(1.0e-9).ln()).sqrt().to_degrees() as f32;
        let hue_mean = (hue_y.atan2(hue_x).to_degrees() as f32 + 360.0) % 360.0;

        let deep = lstar.iter().filter(|l| **l < DEEP_SHADOW_LSTAR).count();
        let bright = lstar.iter().filter(|l| **l > HIGHLIGHT_LSTAR).count();

        let mut highlight_chroma = 0.0f64;
        let mut clipped = 0usize;
        for (pixel, l) in pixels.iter().zip(&lstar) {
            if *l > HIGHLIGHT_LSTAR {
                let lab = lab_of(*pixel);
                highlight_chroma += (lab[1] * lab[1] + lab[2] * lab[2]).sqrt() as f64;
            }
            if pixel.x >= 1.0 || pixel.y >= 1.0 || pixel.z >= 1.0 {
                clipped += 1;
            }
        }
        let highlight_chroma = if bright == 0 {
            0.0
        } else {
            (highlight_chroma / bright as f64) as f32
        };
        let mean_luminance = (luminance.iter().map(|l| *l as f64).sum::<f64>() / n) as f32;

        let mut sorted_luminance = luminance.clone();
        sorted_luminance.sort_by(f32::total_cmp);
        let mut sorted_lstar = lstar.clone();
        sorted_lstar.sort_by(f32::total_cmp);

        Self {
            width,
            height,
            median_luminance: percentile(&sorted_luminance, 0.50),
            mean_luminance,
            deep_shadow: deep as f32 / count as f32,
            highlight: bright as f32 / count as f32,
            chroma: (chroma_sum / n) as f32,
            hue_spread,
            hue_mean,
            gradient_energy: gradient_energy(&lstar, width, height),
            detail_energy: detail_energy(&lstar, width, height),
            coherence: coherence(&lstar, width, height),
            highlight_chroma,
            clipped: clipped as f32 / count as f32,
            ladder: [
                percentile(&sorted_lstar, 0.05),
                percentile(&sorted_lstar, 0.25),
                percentile(&sorted_lstar, 0.50),
                percentile(&sorted_lstar, 0.75),
                percentile(&sorted_lstar, 0.95),
            ],
        }
    }

    /// A one-line-per-number table, with a second plate beside it if given.
    pub fn table(&self, against: Option<&Critique>) -> String {
        /// A label, the value, and how to print it.
        type Row = (&'static str, f32, fn(f32) -> String);

        let mut out = String::new();
        let rows: [Row; 13] = [
            ("median luminance", self.median_luminance, three),
            ("mean luminance", self.mean_luminance, three),
            ("deep shadow L*<20", self.deep_shadow * 100.0, percent),
            ("highlight L*>55", self.highlight * 100.0, percent),
            ("Lab chroma", self.chroma, one),
            ("Lab hue mean", self.hue_mean, degrees),
            ("Lab hue spread", self.hue_spread, degrees),
            ("gradient energy", self.gradient_energy, two),
            ("detail energy", self.detail_energy, two),
            ("coherence @32px", self.coherence, three),
            ("highlight chroma", self.highlight_chroma, one),
            ("clipped", self.clipped * 100.0, percent),
            ("L* median", self.ladder[2], one),
        ];
        let theirs: Option<[f32; 13]> = against.map(|o| {
            [
                o.median_luminance,
                o.mean_luminance,
                o.deep_shadow * 100.0,
                o.highlight * 100.0,
                o.chroma,
                o.hue_mean,
                o.hue_spread,
                o.gradient_energy,
                o.detail_energy,
                o.coherence,
                o.highlight_chroma,
                o.clipped * 100.0,
                o.ladder[2],
            ]
        });

        out.push_str(&format!("{:<22}{:>12}", "", "this"));
        if theirs.is_some() {
            out.push_str(&format!("{:>12}{:>10}", "target", "ratio"));
        }
        out.push('\n');
        for (index, (name, value, format)) in rows.iter().enumerate() {
            out.push_str(&format!("{name:<22}{:>12}", format(*value)));
            if let Some(other) = theirs {
                let ratio = if other[index].abs() > 1.0e-6 {
                    format!("{:.2}x", value / other[index])
                } else {
                    "-".to_string()
                };
                out.push_str(&format!("{:>12}{ratio:>10}", format(other[index])));
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "L* ladder p05/25/50/75/95   {:.1} {:.1} {:.1} {:.1} {:.1}\n",
            self.ladder[0], self.ladder[1], self.ladder[2], self.ladder[3], self.ladder[4]
        ));
        if let Some(other) = against {
            out.push_str(&format!(
                "                     target  {:.1} {:.1} {:.1} {:.1} {:.1}\n",
                other.ladder[0], other.ladder[1], other.ladder[2], other.ladder[3], other.ladder[4]
            ));
        }
        out
    }
}

fn three(v: f32) -> String {
    format!("{v:.3}")
}
fn two(v: f32) -> String {
    format!("{v:.2}")
}
fn one(v: f32) -> String {
    format!("{v:.1}")
}
fn percent(v: f32) -> String {
    format!("{v:.1}%")
}
fn degrees(v: f32) -> String {
    format!("{v:.0}°")
}

/// The acceptance band for one number.
///
/// Bands rather than targets because none of these should be hit exactly. An
/// image forced to an exact median is being graded, not built, and grading is
/// the failure this whole exercise is trying to avoid.
#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub name: &'static str,
    pub low: f32,
    pub high: f32,
}

impl Band {
    pub const fn holds(&self, value: f32) -> bool {
        value >= self.low && value <= self.high
    }
}

/// The bands, measured from the target art and widened to where a different but
/// equally correct plate would still land.
///
/// ## They are a floor, not a destination
///
/// Worth stating plainly, because it changed once the renderer caught up. These
/// numbers were calibrated to *converge on* the reference, and convergence is
/// only the right goal while the reference is the ceiling. It no longer is —
/// the path-traced field has real inter-blade scattering, real contact shadows
/// and a colony structure the painting only implies, and in several respects it
/// is simply better than what it was measured against.
///
/// So read a band as "at least as good as the art, and recognisably the same
/// kind of picture", never as "identical to it". A plate that sits at the top of
/// the highlight band is not failing to match; it is brighter than the painting
/// and has earned the right to be. What the bands still catch — and the reason
/// they are kept — is the *direction* of a regression: a field that goes flat,
/// grey, black-crushed or incoherent leaves them, and it leaves them long before
/// anyone notices by eye.
///
/// The centre of each is a real measurement of `docs/art/grass-target.png` over
/// a 1024² crop, not a number chosen to be achievable:
///
/// | | Measured | Band |
/// |---|---:|---|
/// | median luminance | 0.057 | 0.042..0.080 |
/// | deep shadow | 23.9% | 15..32% |
/// | highlight | 6.7% | 4..10.5% |
/// | coherence | 0.497 | 0.36..0.62 |
/// | chroma | 38.6 | 32..46 |
///
/// Five, and deliberately few. These cannot be traded against one another: a
/// plate can be dark, shadowed, sparsely highlighted, flowing and correctly
/// saturated all at once, or it fails one of them, and each failure has its own
/// distinct repair. The rest of [`Critique`] is reported and not gated, because
/// a band on gradient energy would be a band on how many blades to draw.
///
/// The pairing that does the most work is median luminance against deep shadow.
/// Grading a plate down satisfies the second and immediately breaks the first,
/// so the only way to hold both is to put the darkness where darkness belongs —
/// in occlusion and cast shadow — rather than in the exposure.
pub const BANDS: [Band; 6] = [
    Band {
        name: "median luminance",
        low: 0.042,
        high: 0.080,
    },
    Band {
        name: "deep shadow L*<20",
        low: 0.15,
        high: 0.32,
    },
    Band {
        name: "highlight L*>55",
        low: 0.040,
        high: 0.105,
    },
    Band {
        name: "coherence @32px",
        low: 0.36,
        high: 0.62,
    },
    Band {
        name: "Lab chroma",
        low: 32.0,
        high: 46.0,
    },
    Band {
        name: "highlight chroma",
        low: 30.0,
        high: 58.0,
    },
];

impl Critique {
    /// The gated numbers, in [`BANDS`] order.
    pub fn gated(&self) -> [f32; 6] {
        [
            self.median_luminance,
            self.deep_shadow,
            self.highlight,
            self.coherence,
            self.chroma,
            self.highlight_chroma,
        ]
    }

    /// Which bands this plate is outside, if any.
    pub fn failures(&self) -> Vec<String> {
        let values = self.gated();
        BANDS
            .iter()
            .zip(values)
            .filter(|(band, value)| !band.holds(*value))
            .map(|(band, value)| {
                let direction = if value < band.low { "below" } else { "above" };
                format!(
                    "{} is {value:.3}, {direction} {:.3}..{:.3}",
                    band.name, band.low, band.high
                )
            })
            .collect()
    }
}

/// Relative luminance in linear light, from a display-referred colour.
#[inline]
pub fn relative_luminance(colour: Vec3) -> f32 {
    let linear = Vec3::new(
        to_linear(colour.x),
        to_linear(colour.y),
        to_linear(colour.z),
    );
    0.2126 * linear.x + 0.7152 * linear.y + 0.0722 * linear.z
}

/// sRGB transfer function, inverted.
#[inline]
pub fn to_linear(channel: f32) -> f32 {
    let c = channel.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// CIE L\*a\*b\* under D65, from a display-referred colour.
pub fn lab_of(colour: Vec3) -> [f32; 3] {
    let r = to_linear(colour.x);
    let g = to_linear(colour.y);
    let b = to_linear(colour.z);
    // sRGB primaries to CIE XYZ, D65.
    let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
    // D65 white.
    let fx = lab_f(x / 0.950_47);
    let fy = lab_f(y);
    let fz = lab_f(z / 1.088_83);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

#[inline]
fn lab_f(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f32 * fraction).round() as usize;
    sorted[index]
}

/// Mean Sobel magnitude, in L\* units per pixel.
fn gradient_energy(lstar: &[f32], width: usize, height: usize) -> f32 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    let mut total = 0.0f64;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let at = |dx: isize, dy: isize| {
                lstar[(y as isize + dy) as usize * width + (x as isize + dx) as usize]
            };
            let gx =
                at(1, -1) + 2.0 * at(1, 0) + at(1, 1) - at(-1, -1) - 2.0 * at(-1, 0) - at(-1, 1);
            let gy =
                at(-1, 1) + 2.0 * at(0, 1) + at(1, 1) - at(-1, -1) - 2.0 * at(0, -1) - at(1, -1);
            total += ((gx * gx + gy * gy).sqrt() * 0.25) as f64;
        }
    }
    (total / ((width - 2) * (height - 2)) as f64) as f32
}

/// Mean absolute Laplacian, in L\* units per pixel.
fn detail_energy(lstar: &[f32], width: usize, height: usize) -> f32 {
    if width < 3 || height < 3 {
        return 0.0;
    }
    let mut total = 0.0f64;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let index = y * width + x;
            let laplacian = 4.0 * lstar[index]
                - lstar[index - 1]
                - lstar[index + 1]
                - lstar[index - width]
                - lstar[index + width];
            total += laplacian.abs() as f64;
        }
    }
    (total / ((width - 2) * (height - 2)) as f64) as f32
}

/// Mean structure-tensor coherence over [`COHERENCE_WINDOW`] windows.
///
/// Per window: accumulate the gradient outer product, then read the eigenvalue
/// split `(λ₁ - λ₂) / (λ₁ + λ₂)`. One means every gradient in the window shares
/// an axis — a bundle of parallel blades. Zero means they point everywhere,
/// which is what a field of independently oriented marks produces however
/// crisp each mark is.
fn coherence(lstar: &[f32], width: usize, height: usize) -> f32 {
    let window = COHERENCE_WINDOW;
    if width < window + 2 || height < window + 2 {
        return 0.0;
    }
    let mut total = 0.0f64;
    let mut windows = 0usize;
    let mut y0 = 1;
    while y0 + window < height {
        let mut x0 = 1;
        while x0 + window < width {
            let (mut jxx, mut jxy, mut jyy) = (0.0f64, 0.0f64, 0.0f64);
            for y in y0..y0 + window {
                for x in x0..x0 + window {
                    let index = y * width + x;
                    let gx = 0.5 * (lstar[index + 1] - lstar[index - 1]);
                    let gy = 0.5 * (lstar[index + width] - lstar[index - width]);
                    jxx += (gx * gx) as f64;
                    jxy += (gx * gy) as f64;
                    jyy += (gy * gy) as f64;
                }
            }
            let trace = jxx + jyy;
            if trace > 1.0e-6 {
                // (λ₁ - λ₂) for a 2×2 symmetric matrix, without the eigensolve.
                let split = ((jxx - jyy) * (jxx - jyy) + 4.0 * jxy * jxy).sqrt();
                total += split / trace;
                windows += 1;
            }
            x0 += window;
        }
        y0 += window;
    }
    if windows == 0 {
        0.0
    } else {
        (total / windows as f64) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(colour: Vec3, side: usize) -> Vec<Vec3> {
        vec![colour; side * side]
    }

    #[test]
    fn a_flat_plate_has_no_gradient_and_no_detail() {
        let pixels = plate(Vec3::new(0.2, 0.4, 0.1), 64);
        let measured = Critique::of(&pixels, 64, 64);
        assert!(measured.gradient_energy < 1.0e-4);
        assert!(measured.detail_energy < 1.0e-4);
    }

    #[test]
    fn black_and_white_land_at_the_ends_of_l_star() {
        assert!(lab_of(Vec3::ZERO)[0] < 0.01);
        assert!((lab_of(Vec3::ONE)[0] - 100.0).abs() < 0.01);
    }

    #[test]
    fn grass_green_reads_as_yellow_green_hue() {
        // Lab hue for foliage sits between 100° and 145°; anything outside that
        // means the conversion is wrong, not that the colour is unusual.
        let lab = lab_of(Vec3::new(0.25, 0.40, 0.05));
        let hue = lab[2].atan2(lab[1]).to_degrees();
        assert!(
            (95.0..150.0).contains(&hue),
            "grass hue landed at {hue}, outside foliage"
        );
    }

    #[test]
    fn parallel_stripes_are_coherent_and_noise_is_not() {
        let side = 128;
        let mut stripes = Vec::with_capacity(side * side);
        for y in 0..side {
            for _ in 0..side {
                let v = if (y / 3) % 2 == 0 { 0.2 } else { 0.5 };
                stripes.push(Vec3::splat(v));
            }
        }
        let striped = Critique::of(&stripes, side, side);

        // A deterministic hash, so the "noise" is the same noise every run.
        let mut noise = Vec::with_capacity(side * side);
        for i in 0..side * side {
            let h = (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let h = h ^ (h >> 31);
            noise.push(Vec3::splat((h % 1000) as f32 / 1000.0));
        }
        let noisy = Critique::of(&noise, side, side);

        assert!(
            striped.coherence > 0.9,
            "stripes measured {:.3}",
            striped.coherence
        );
        assert!(
            noisy.coherence < 0.3,
            "noise measured {:.3}",
            noisy.coherence
        );
    }

    #[test]
    fn a_dark_plate_registers_as_deep_shadow_and_a_bright_one_does_not() {
        let dark = Critique::of(&plate(Vec3::new(0.05, 0.09, 0.02), 32), 32, 32);
        let bright = Critique::of(&plate(Vec3::new(0.35, 0.55, 0.12), 32), 32, 32);
        assert_eq!(dark.deep_shadow, 1.0);
        assert_eq!(bright.deep_shadow, 0.0);
        assert!(dark.median_luminance < bright.median_luminance);
    }

    #[test]
    fn the_bands_reject_the_plate_this_work_started_from() {
        // The pre-overhaul plate: bright, chromatic, and with no true dark in it
        // at all. Recorded so the gate is known to be able to fail.
        let stale = Critique::of(&plate(Vec3::new(0.29, 0.44, 0.09), 40), 40, 40);
        let failures = stale.failures();
        assert!(
            failures.len() >= 2,
            "a flat mid-green should fail several bands, got {failures:?}"
        );
    }
}

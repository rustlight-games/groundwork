//! What scales the ground actually has structure at.
//!
//! ## Why the spectrum and not just the roughness
//!
//! A profile declares bands: "five centimetres at four millimetres of amplitude,
//! one centimetre at one, two millimetres at a quarter". That is a claim about
//! *where the energy is*, and the only instrument that can check it is a
//! spectrum. The scalar metrics next door say how much total relief there is;
//! this says whether it is in the places the author asked for.
//!
//! It is also the instrument that catches the two failures that look fine in
//! every other measurement:
//!
//! - **energy appearing twice**, when a band is drawn as both geometry and bump;
//! - **axis-aligned grid energy**, when a value-noise lattice leaks through as a
//!   quilted pattern the eye finds immediately and no scalar statistic sees.
//!
//! ## Parseval is the self-test
//!
//! A PSD implementation is easy to get wrong by a constant factor, and a
//! constant factor is invisible in a log plot. The discrete integral of this PSD
//! must equal the detrended variance the scalar half computed, and
//! [`SpectralMetrics::parseval_relative_error`] is that comparison. If it is not
//! tiny, no other number here means anything.
//!
//! ## The transform
//!
//! A radix-2 Cooley–Tukey FFT, written here rather than taken as a dependency.
//! The sizes are powers of two by construction — the analysis window is chosen
//! to make them so — and the whole transform is forty lines. A dependency would
//! be more code in the tree, not less.

use std::f64::consts::TAU;

/// One complex value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    fn magnitude_squared(self) -> f64 {
        self.re * self.re + self.im * self.im
    }
}

/// In-place radix-2 FFT. `values.len()` must be a power of two.
pub fn fft(values: &mut [Complex]) {
    let n = values.len();
    if n <= 1 {
        return;
    }
    debug_assert!(n.is_power_of_two(), "the FFT is radix-2");

    // Bit-reversal permutation.
    let mut target = 0usize;
    for source in 1..n {
        let mut bit = n >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
        if source < target {
            values.swap(source, target);
        }
    }

    let mut length = 2usize;
    while length <= n {
        let angle = -TAU / length as f64;
        let (step_re, step_im) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(length) {
            let (mut w_re, mut w_im) = (1.0f64, 0.0f64);
            for offset in 0..length / 2 {
                let a = values[start + offset];
                let b = values[start + offset + length / 2];
                let t = Complex {
                    re: b.re * w_re - b.im * w_im,
                    im: b.re * w_im + b.im * w_re,
                };
                values[start + offset] = Complex {
                    re: a.re + t.re,
                    im: a.im + t.im,
                };
                values[start + offset + length / 2] = Complex {
                    re: a.re - t.re,
                    im: a.im - t.im,
                };
                let next_re = w_re * step_re - w_im * step_im;
                w_im = w_re * step_im + w_im * step_re;
                w_re = next_re;
            }
        }
        length <<= 1;
    }
}

/// A two-dimensional FFT over a row-major field.
pub fn fft2(values: &mut [Complex], columns: usize, rows: usize) {
    let mut row_buffer = vec![Complex::default(); columns];
    for row in 0..rows {
        row_buffer.copy_from_slice(&values[row * columns..(row + 1) * columns]);
        fft(&mut row_buffer);
        values[row * columns..(row + 1) * columns].copy_from_slice(&row_buffer);
    }
    let mut column_buffer = vec![Complex::default(); rows];
    for column in 0..columns {
        for row in 0..rows {
            column_buffer[row] = values[row * columns + column];
        }
        fft(&mut column_buffer);
        for row in 0..rows {
            values[row * columns + column] = column_buffer[row];
        }
    }
}

/// The separable Hann window, and the power normaliser it implies.
///
/// A window is not optional. The transform assumes the field is periodic, and a
/// field that is not wraps a discontinuity into every spectrum — which appears
/// as broadband energy indistinguishable from real fine structure.
fn hann(columns: usize, rows: usize) -> (Vec<f64>, f64) {
    let one = |n: usize, i: usize| {
        if n <= 1 {
            1.0
        } else {
            0.5 - 0.5 * (TAU * i as f64 / (n - 1) as f64).cos()
        }
    };
    let mut window = Vec::with_capacity(columns * rows);
    let mut power = 0.0;
    for row in 0..rows {
        for column in 0..columns {
            let w = one(columns, column) * one(rows, row);
            power += w * w;
            window.push(w);
        }
    }
    (window, power / (columns * rows) as f64)
}

/// Energy measured in one authored band's neighbourhood.
#[derive(Clone, Debug, PartialEq)]
pub struct BandEnergy {
    pub key: String,
    pub declared_wavelength_m: f64,
    pub dominant_wavelength_m: f64,
    pub energy_m2: f64,
    /// This band's share of the field's total energy.
    ///
    /// Named for what it is. It was called a ratio to a reference, which it
    /// never was — and the difference matters: halving an isolated band's
    /// amplitude leaves this number unchanged, because it halves the numerator
    /// and the denominator together. `energy_m2` is the absolute quantity, and
    /// it is what an amplitude regression moves.
    pub energy_share: f64,
    pub out_of_band_fraction: f64,
}

/// What a spectrum says about a field.
#[derive(Clone, Debug, PartialEq)]
pub struct SpectralMetrics {
    /// The variance the PSD integrates to.
    pub variance_from_psd_m2: f64,
    /// How far that is from the same variance computed in the spatial domain.
    ///
    /// The self-test, and it is deliberately an *identity* rather than a
    /// comparison against something else. Both sides are the window-corrected
    /// variance `Σ(x·w)²/(N·U)`, so a correct transform makes them agree to
    /// rounding — which is exactly what catches a wrong normaliser, a wrong bin
    /// area, or a broken bit-reversal. Comparing against the *unwindowed*
    /// variance instead would be measuring whether the field is stationary over
    /// the window, which is a different and much looser question, and a
    /// difference there would be blamed on the transform.
    pub parseval_relative_error: f64,
    /// The window-corrected variance against the plain one.
    ///
    /// Reported rather than gated. Far from one means the field's structure is
    /// comparable in scale to the window — which for a coarse band in a small
    /// laboratory is expected, and is the reason the identity above is the
    /// self-test rather than this.
    pub windowed_variance_ratio: f64,
    /// Energy near the two frequency axes, *relative to an isotropic field*.
    ///
    /// The signature of an axis-aligned lattice leaking through. A raw fraction
    /// cannot say that on its own, and the first version of this metric was
    /// wrong for a subtle reason: at small spectral radii there are very few
    /// integer bins, and most of them lie in the axis strips whatever the field
    /// is doing. A perfectly isotropic ring at radius three reports most of its
    /// energy "on the axes" simply because there is nowhere else for it to be.
    ///
    /// So this is a *ratio* against what an isotropic field of the same radial
    /// spectrum would put there. One means "as axis-aligned as isotropy",
    /// meaningfully above one means a grid is showing through.
    pub axis_grid_energy_fraction: f64,
    /// Energy above the four-samples-per-wavelength policy cutoff.
    ///
    /// Deliberately **not** called aliasing, because it is not. Once a field is
    /// sampled, aliased energy has already folded into the baseband and cannot
    /// be told apart from energy that belongs there — a tone at `0.8/Δ` appears
    /// at `0.2/Δ` and this metric never sees it. Detecting real aliasing needs
    /// an oversampled reference and a controlled downsample, which is a
    /// different experiment.
    ///
    /// What this does measure is useful anyway: the generator's own policy is
    /// that nothing lives above `1/(4Δ)`, so energy up there means a band is
    /// carrying content finer than it declared.
    pub above_policy_cutoff_fraction: f64,
    /// `(λ1 − λ2)/(λ1 + λ2)` of the spectral orientation tensor.
    pub anisotropy: f64,
    pub principal_wavevector_rad: f64,
    /// Radially binned spectrum: `(wavelength_m, energy_m2)`.
    pub radial: Vec<(f64, f64)>,
    pub bands: Vec<BandEnergy>,
}

/// A band to measure energy in.
#[derive(Clone, Debug, PartialEq)]
pub struct BandQuery {
    pub key: String,
    pub wavelength_m: f64,
    /// How wide a neighbourhood counts as "in band", as a factor either side.
    ///
    /// An octave by default: half to twice the declared wavelength. Narrower
    /// and a band whose realised frequency is a few percent off reports zero
    /// energy; wider and two adjacent bands in a ladder overlap.
    pub relative_width: f64,
}

impl BandQuery {
    pub fn new(key: impl Into<String>, wavelength_m: f64) -> Self {
        Self {
            key: key.into(),
            wavelength_m,
            relative_width: 2.0,
        }
    }
}

/// Compute the spectrum of a detrended field.
///
/// `spacing_m` is the lattice pitch. `columns` and `rows` must be powers of two;
/// the caller crops to a power-of-two window rather than padding, because
/// zero-padding a field that does not decay to zero at its edges introduces the
/// very discontinuity the window exists to remove.
pub fn measure(
    residual: &[f32],
    columns: usize,
    rows: usize,
    spacing_m: f64,
    bands: &[BandQuery],
) -> SpectralMetrics {
    // Four is the smallest window with a non-degenerate symmetric Hann: at two
    // the window is all zeros, its mean power is zero, and every normalised
    // quantity below becomes a NaN that the `> 0.0` guards then report as a
    // clean pass.
    if columns < 4
        || rows < 4
        || !columns.is_power_of_two()
        || !rows.is_power_of_two()
        || residual.len() < columns * rows
    {
        return SpectralMetrics {
            variance_from_psd_m2: 0.0,
            parseval_relative_error: 0.0,
            windowed_variance_ratio: 0.0,
            axis_grid_energy_fraction: 0.0,
            above_policy_cutoff_fraction: 0.0,
            anisotropy: 0.0,
            principal_wavevector_rad: 0.0,
            radial: Vec::new(),
            bands: Vec::new(),
        };
    }

    let (window, window_power) = hann(columns, rows);
    // The *window-weighted* mean, not the plain one.
    //
    // The DC bin of the transform is `Σ(x − m)·w`, and the analysis below skips
    // that bin because a field's offset is not part of its roughness. Skipping
    // it is only exact when it is actually zero, which needs `m = Σ(x·w)/Σ(w)`.
    // Subtracting the plain mean instead leaves real energy in DC — a fifth of
    // a percent of the total on a cloddy field — and Parseval then fails by
    // exactly that, which reads as a broken transform.
    let window_sum: f64 = window.iter().sum();
    let mean = if window_sum > 0.0 {
        residual[..columns * rows]
            .iter()
            .zip(window.iter())
            .map(|(v, w)| *v as f64 * w)
            .sum::<f64>()
            / window_sum
    } else {
        0.0
    };
    let mut spectrum: Vec<Complex> = (0..columns * rows)
        .map(|index| Complex {
            re: (residual[index] as f64 - mean) * window[index],
            im: 0.0,
        })
        .collect();

    // The spatial-domain side of the identity, computed from the exact values
    // the transform is about to see.
    let windowed_variance: f64 = spectrum.iter().map(|c| c.re * c.re).sum::<f64>()
        / ((columns * rows) as f64 * window_power);
    let plain_variance: f64 = residual[..columns * rows]
        .iter()
        .map(|v| (*v as f64 - mean).powi(2))
        .sum::<f64>()
        / (columns * rows) as f64;

    fft2(&mut spectrum, columns, rows);

    // PSD = ΔuΔv |F|² / (Nx Ny U), and the frequency-bin areas are
    // 1/(Nx Δu) and 1/(Ny Δv), so the discrete integral of the PSD is
    // Σ|F|² / (Nx Ny)² / U — which is the windowed variance.
    let scale = spacing_m * spacing_m / ((columns * rows) as f64 * window_power);
    let bin_area = 1.0 / (columns as f64 * spacing_m) * (1.0 / (rows as f64 * spacing_m));

    let frequency = |index: usize, n: usize, spacing: f64| {
        let signed = if index <= n / 2 {
            index as f64
        } else {
            index as f64 - n as f64
        };
        signed / (n as f64 * spacing)
    };

    let mut total = 0.0;
    let mut axis_energy = 0.0;
    let mut axis_expected = 0.0;
    let mut above_cutoff = 0.0;
    let mut tensor = [[0.0f64; 2]; 2];
    // The finest wavelength a lattice can carry without aliasing is four
    // samples, matching `SAMPLES_PER_WAVELENGTH` on the generator side.
    let representable = 1.0 / (4.0 * spacing_m);

    let mut radial_bins: Vec<(f64, f64)> = Vec::new();
    let bin_count = 48usize;
    let mut radial_energy = vec![0.0f64; bin_count];
    let mut radial_wavelength = vec![0.0f64; bin_count];
    let mut radial_weight = vec![0.0f64; bin_count];
    let nyquist = 1.0 / (2.0 * spacing_m);

    let mut band_energy = vec![0.0f64; bands.len()];
    let mut band_peak = vec![(0.0f64, 0.0f64); bands.len()];
    let mut band_total = vec![0.0f64; bands.len()];

    for row in 0..rows {
        for column in 0..columns {
            let fu = frequency(column, columns, spacing_m);
            let fv = frequency(row, rows, spacing_m);
            let magnitude = spectrum[row * columns + column].magnitude_squared();
            let psd = magnitude * scale;
            let energy = psd * bin_area;
            if column == 0 && row == 0 {
                // The DC bin is the mean, which was removed. Counting it would
                // put the field's offset into its roughness.
                continue;
            }
            total += energy;

            // Within one bin of an axis, not exactly on it. The window is
            // separable, so a tone varying only in `u` is windowed in `v` as
            // well and its energy smears along the `v` axis into the
            // neighbouring bins. Counting only the exact axis would report a
            // purely axis-aligned field as two-thirds diagonal.
            let near_u = column <= 1 || column >= columns - 1;
            let near_v = row <= 1 || row >= rows - 1;
            let on_axis = near_u || near_v;
            if on_axis {
                axis_energy += energy;
            }
            // The isotropic expectation, accumulated alongside: for each bin,
            // the share of its own radial ring that lies in the axis strips.
            // Summing that weighted by the bin's energy gives what an isotropic
            // field with this radial spectrum would have put on the axes.
            let radius_bins =
                ((column.min(columns - column)).pow(2) + (row.min(rows - row)).pow(2)) as f64;
            let radius_bins = radius_bins.sqrt();
            if radius_bins > 0.0 {
                // A ring of radius r holds about 2πr integer bins; the strips
                // are four arms two bins wide, so about 16 of them, capped at
                // the whole ring.
                let ring = (std::f64::consts::TAU * radius_bins).max(1.0);
                axis_expected += energy * (16.0 / ring).min(1.0);
            }
            let radius = (fu * fu + fv * fv).sqrt();
            if radius > representable {
                above_cutoff += energy;
            }

            // Orientation tensor, normalised by |k|² so that the direction of a
            // wavevector counts rather than its length.
            if radius > 0.0 {
                let inverse = 1.0 / (radius * radius);
                tensor[0][0] += energy * fu * fu * inverse;
                tensor[0][1] += energy * fu * fv * inverse;
                tensor[1][1] += energy * fv * fv * inverse;
            }

            if radius > 0.0 && radius <= nyquist {
                let bin = ((radius / nyquist) * (bin_count - 1) as f64).round() as usize;
                radial_energy[bin] += energy;
                radial_wavelength[bin] += (1.0 / radius) * energy;
                radial_weight[bin] += energy;
            }

            for (slot, band) in bands.iter().enumerate() {
                band_total[slot] += energy;
                if radius <= 0.0 {
                    continue;
                }
                let wavelength = 1.0 / radius;
                let low = band.wavelength_m / band.relative_width;
                let high = band.wavelength_m * band.relative_width;
                if wavelength >= low && wavelength <= high {
                    band_energy[slot] += energy;
                    if energy > band_peak[slot].1 {
                        band_peak[slot] = (wavelength, energy);
                    }
                }
            }
        }
    }
    tensor[1][0] = tensor[0][1];

    for bin in 0..bin_count {
        if radial_weight[bin] <= 0.0 {
            continue;
        }
        radial_bins.push((
            radial_wavelength[bin] / radial_weight[bin],
            radial_energy[bin],
        ));
    }

    // Eigenvalues of a symmetric two-by-two, closed form.
    let trace = tensor[0][0] + tensor[1][1];
    let determinant = tensor[0][0] * tensor[1][1] - tensor[0][1] * tensor[1][0];
    let discriminant = (trace * trace * 0.25 - determinant).max(0.0).sqrt();
    let (major, minor) = (trace * 0.5 + discriminant, trace * 0.5 - discriminant);
    let anisotropy = if major + minor > 0.0 {
        (major - minor) / (major + minor)
    } else {
        0.0
    };
    let principal = if tensor[0][1].abs() > 1.0e-18 {
        (major - tensor[0][0]).atan2(tensor[0][1])
    } else if tensor[0][0] >= tensor[1][1] {
        0.0
    } else {
        std::f64::consts::FRAC_PI_2
    };

    let parseval = if windowed_variance > 0.0 {
        ((total - windowed_variance) / windowed_variance).abs()
    } else {
        0.0
    };

    SpectralMetrics {
        variance_from_psd_m2: total,
        parseval_relative_error: parseval,
        windowed_variance_ratio: if plain_variance > 0.0 {
            windowed_variance / plain_variance
        } else {
            0.0
        },
        axis_grid_energy_fraction: if axis_expected > 0.0 {
            axis_energy / axis_expected
        } else {
            0.0
        },
        above_policy_cutoff_fraction: if total > 0.0 {
            above_cutoff / total
        } else {
            0.0
        },
        anisotropy,
        principal_wavevector_rad: principal,
        radial: radial_bins,
        bands: bands
            .iter()
            .enumerate()
            .map(|(slot, band)| BandEnergy {
                key: band.key.clone(),
                declared_wavelength_m: band.wavelength_m,
                dominant_wavelength_m: band_peak[slot].0,
                energy_m2: band_energy[slot],
                energy_share: if total > 0.0 {
                    band_energy[slot] / total
                } else {
                    0.0
                },
                out_of_band_fraction: if total > 0.0 {
                    (total - band_energy[slot]) / total
                } else {
                    0.0
                },
            })
            .collect(),
    }
}

/// The largest power-of-two window that fits, centred.
///
/// Cropped rather than padded. Zero-padding a field that does not decay to zero
/// at its edges introduces exactly the discontinuity the window exists to
/// remove, and the resulting broadband energy is indistinguishable from real
/// fine structure.
pub fn crop_to_power_of_two(
    values: &[f32],
    columns: usize,
    rows: usize,
) -> (Vec<f32>, usize, usize) {
    let side = columns.min(rows);
    if side < 2 {
        return (values.to_vec(), columns, rows);
    }
    let target = 1usize << (usize::BITS - 1 - side.leading_zeros()) as usize;
    let offset_u = (columns - target) / 2;
    let offset_v = (rows - target) / 2;
    let mut out = Vec::with_capacity(target * target);
    for row in 0..target {
        let start = (row + offset_v) * columns + offset_u;
        out.extend_from_slice(&values[start..start + target]);
    }
    (out, target, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(
        columns: usize,
        rows: usize,
        spacing: f64,
        wavelength: f64,
        amplitude: f64,
    ) -> Vec<f32> {
        let mut out = Vec::with_capacity(columns * rows);
        for row in 0..rows {
            for column in 0..columns {
                let u = column as f64 * spacing;
                out.push((amplitude * (TAU * u / wavelength).sin()) as f32);
                let _ = row;
            }
        }
        out
    }

    #[test]
    fn the_transform_inverts_a_known_impulse() {
        // A unit impulse transforms to a flat spectrum of magnitude one. The
        // cheapest possible check that the butterfly and the bit reversal agree.
        let mut values = vec![Complex::default(); 8];
        values[0] = Complex { re: 1.0, im: 0.0 };
        fft(&mut values);
        for value in &values {
            assert!((value.re - 1.0).abs() < 1.0e-12, "{value:?}");
            assert!(value.im.abs() < 1.0e-12, "{value:?}");
        }
    }

    #[test]
    fn a_pure_tone_lands_in_one_bin() {
        // A sine at exactly bin `k` puts all of its energy in `±k` and nothing
        // anywhere else. If the frequency mapping were off by one this is where
        // it shows.
        let n = 32;
        let k = 5;
        let mut values: Vec<Complex> = (0..n)
            .map(|i| Complex {
                re: (TAU * k as f64 * i as f64 / n as f64).cos(),
                im: 0.0,
            })
            .collect();
        fft(&mut values);
        for (index, value) in values.iter().enumerate() {
            let magnitude = value.magnitude_squared().sqrt();
            if index == k || index == n - k {
                assert!(
                    (magnitude - n as f64 / 2.0).abs() < 1.0e-9,
                    "{index}: {magnitude}"
                );
            } else {
                assert!(magnitude < 1.0e-9, "{index}: {magnitude}");
            }
        }
    }

    #[test]
    fn parseval_holds_for_a_known_sine() {
        // The self-test the whole module rests on. A PSD wrong by a constant is
        // invisible in a log plot and makes every band energy meaningless.
        let spacing = 0.005;
        let wavelength = 0.08;
        let amplitude = 0.004;
        let side = 128;
        let values = sine(side, side, spacing, wavelength, amplitude);
        let metrics = measure(&values, side, side, spacing, &[]);
        assert!(
            metrics.parseval_relative_error < 1.0e-9,
            "Parseval error {}",
            metrics.parseval_relative_error
        );
    }

    #[test]
    fn a_sine_reports_its_own_wavelength_as_dominant() {
        let spacing = 0.005;
        let wavelength = 0.08;
        let side = 128;
        let values = sine(side, side, spacing, wavelength, 0.004);
        let metrics = measure(
            &values,
            side,
            side,
            spacing,
            &[BandQuery::new("clod", wavelength)],
        );
        let band = &metrics.bands[0];
        assert!(
            (band.dominant_wavelength_m - wavelength).abs() < wavelength * 0.1,
            "dominant {} against {wavelength}",
            band.dominant_wavelength_m
        );
        // Nearly all the energy is in band, because there is only one tone.
        assert!(
            band.energy_share > 0.9,
            "in-band fraction {}",
            band.energy_share
        );
    }

    #[test]
    fn a_one_dimensional_wave_is_strongly_anisotropic() {
        // A ripple field should report high anisotropy at its own frequency and
        // not at unrelated bands; a control that varies in one axis only is the
        // extreme case of that.
        let side = 128;
        let values = sine(side, side, 0.005, 0.08, 0.004);
        let metrics = measure(&values, side, side, 0.005, &[]);
        assert!(
            metrics.anisotropy > 0.9,
            "anisotropy {}",
            metrics.anisotropy
        );
    }

    #[test]
    fn an_isotropic_field_reports_low_anisotropy() {
        // The control the ripple laboratory is measured against.
        let side = 64;
        let spacing = 0.01;
        let mut values = Vec::with_capacity(side * side);
        for row in 0..side {
            for column in 0..side {
                let u = column as f64 * spacing;
                let v = row as f64 * spacing;
                // A sum of waves in many directions, which is as close to
                // isotropic as a deterministic field gets.
                let mut total = 0.0;
                for k in 0..8 {
                    let angle = std::f64::consts::PI * k as f64 / 8.0;
                    total += ((u * angle.cos() + v * angle.sin()) * TAU / 0.08).sin();
                }
                values.push((total * 0.001) as f32);
            }
        }
        let metrics = measure(&values, side, side, spacing, &[]);
        assert!(
            metrics.anisotropy < 0.35,
            "anisotropy {} for a many-direction field",
            metrics.anisotropy
        );
    }

    #[test]
    fn an_axis_aligned_grid_shows_up_as_axis_energy() {
        // The quilted-square failure, made measurable. A value-noise lattice
        // sampled without a turn puts a cross through the middle of its own
        // spectrum, and no scalar statistic sees it.
        let side = 64;
        let spacing = 0.01;
        let mut values = Vec::with_capacity(side * side);
        for row in 0..side {
            for column in 0..side {
                let u = column as f64 * spacing;
                let v = row as f64 * spacing;
                values.push((0.002 * ((TAU * u / 0.08).sin() + (TAU * v / 0.08).sin())) as f32);
            }
        }
        let metrics = measure(&values, side, side, spacing, &[]);
        assert!(
            metrics.axis_grid_energy_fraction > 0.8,
            "axis energy {} for a purely axis-aligned field",
            metrics.axis_grid_energy_fraction
        );
    }

    #[test]
    fn a_non_power_of_two_window_returns_empty_rather_than_wrong() {
        // Silently transforming the wrong size would produce a spectrum that
        // looks plausible and describes nothing.
        let metrics = measure(&vec![0.0f32; 30 * 30], 30, 30, 0.01, &[]);
        assert!(metrics.radial.is_empty());
        assert_eq!(metrics.variance_from_psd_m2, 0.0);
    }

    #[test]
    fn cropping_takes_the_largest_centred_power_of_two() {
        let values: Vec<f32> = (0..(100 * 90)).map(|i| i as f32).collect();
        let (cropped, columns, rows) = crop_to_power_of_two(&values, 100, 90);
        assert_eq!((columns, rows), (64, 64));
        assert_eq!(cropped.len(), 64 * 64);
        // Centred: the first cropped value is at offset (18, 13) of the original.
        assert_eq!(cropped[0], (13 * 100 + 18) as f32);
    }
}

//! How far apart two points have to be before they stop resembling each other.
//!
//! ## What this catches that the spectrum does not
//!
//! A spectrum says where the energy is. A semivariogram says how the field
//! *decorrelates* with distance, and the two differ in one useful way: the
//! **nugget** — the variance the curve extrapolates back to at zero lag — has no
//! spectral equivalent that is as easy to read.
//!
//! In a stochastic field a nugget is real microscale variation. In a
//! *deterministic analytic field* like this one it is not, and it should be
//! close to zero. A large nugget therefore means something specific and
//! unwelcome: aliasing, a discontinuity, or a bad handoff between representation
//! tiers. It is the cheapest instrument in the suite for the class of bug where a
//! band moved from geometry to bump and changed its phase on the way.
//!
//! The **practical range** — where the curve reaches 95% of its sill — should
//! track the declared feature wavelength's order of magnitude. It does not have
//! to equal it; a field of five-centimetre clods decorrelates over something
//! like a clod, not over exactly one.

/// One lag's worth of the curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VariogramSample {
    pub lag_m: f64,
    pub gamma_m2: f64,
    pub pair_count: usize,
}

/// The curve, and what it implies.
#[derive(Clone, Debug, PartialEq)]
pub struct Semivariogram {
    pub direction_rad: f64,
    /// The value the curve extrapolates to at zero lag.
    ///
    /// Should be near zero for a deterministic field. See the module note.
    pub nugget_m2: f64,
    /// The plateau: the field's own variance, reached once points are far
    /// enough apart to be unrelated.
    pub sill_m2: f64,
    /// Where the curve first reaches 95% of the sill.
    pub practical_range_m: f64,
    /// Where autocorrelation falls to `1/e`.
    pub autocorrelation_1e_m: f64,
    pub samples: Vec<VariogramSample>,
}

/// Compute a directional semivariogram over a detrended field.
///
/// `direction_rad` of zero is along `u`; `FRAC_PI_2` is along `v`. Omnidirectional
/// is not a special case here — the caller averages the four directions it asked
/// for, which keeps the anisotropy visible rather than averaging it away.
pub fn measure(
    residual: &[f32],
    columns: usize,
    rows: usize,
    spacing_m: f64,
    direction_rad: f64,
    max_lag_steps: usize,
) -> Semivariogram {
    let mut samples = Vec::new();
    let (unit_u, unit_v) = (direction_rad.cos(), direction_rad.sin());

    for step in 1..=max_lag_steps {
        let du = (unit_u * step as f64).round() as i64;
        let dv = (unit_v * step as f64).round() as i64;
        if du == 0 && dv == 0 {
            continue;
        }
        let lag = ((du * du + dv * dv) as f64).sqrt() * spacing_m;
        let mut total = 0.0;
        let mut pairs = 0usize;
        for row in 0..rows {
            for column in 0..columns {
                let (c, r) = (column as i64 + du, row as i64 + dv);
                if c < 0 || r < 0 || c as usize >= columns || r as usize >= rows {
                    continue;
                }
                let here = residual[row * columns + column] as f64;
                let there = residual[r as usize * columns + c as usize] as f64;
                total += (there - here) * (there - here);
                pairs += 1;
            }
        }
        if pairs < 16 {
            continue;
        }
        samples.push(VariogramSample {
            lag_m: lag,
            gamma_m2: total / (2.0 * pairs as f64),
            pair_count: pairs,
        });
    }

    // The sill is the plateau. Taken as the median of the far half rather than
    // the maximum, because the far end has the fewest pairs and therefore the
    // noisiest estimate — a maximum would systematically overstate it.
    let sill = if samples.is_empty() {
        0.0
    } else {
        let mut tail: Vec<f64> = samples[samples.len() / 2..]
            .iter()
            .map(|s| s.gamma_m2)
            .collect();
        tail.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        tail[tail.len() / 2]
    };

    // The nugget by linear extrapolation through the first two samples. Fitting
    // a model would be more principled and would also hide the thing being
    // looked for: a real nugget in a deterministic field is a discontinuity, and
    // a discontinuity shows in the first two lags.
    let nugget = match samples.as_slice() {
        [] | [_] => 0.0,
        [first, second, ..] => {
            let slope =
                (second.gamma_m2 - first.gamma_m2) / (second.lag_m - first.lag_m).max(1.0e-12);
            (first.gamma_m2 - slope * first.lag_m).max(0.0)
        }
    };

    let practical_range = samples
        .iter()
        .find(|s| s.gamma_m2 >= sill * 0.95)
        .map(|s| s.lag_m)
        .unwrap_or_else(|| samples.last().map(|s| s.lag_m).unwrap_or(0.0));

    // γ(h) = σ²(1 − ρ(h)), so ρ = 1 − γ/sill and `1/e` is γ = sill(1 − 1/e).
    let target = sill * (1.0 - std::f64::consts::E.recip());
    let autocorrelation = samples
        .iter()
        .find(|s| s.gamma_m2 >= target)
        .map(|s| s.lag_m)
        .unwrap_or(0.0);

    Semivariogram {
        direction_rad,
        nugget_m2: nugget,
        sill_m2: sill,
        practical_range_m: practical_range,
        autocorrelation_1e_m: autocorrelation,
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine(side: usize, spacing: f64, wavelength: f64, amplitude: f64) -> Vec<f32> {
        let mut out = Vec::with_capacity(side * side);
        for row in 0..side {
            for column in 0..side {
                let u = column as f64 * spacing;
                out.push((amplitude * (TAU * u / wavelength).sin()) as f32);
                let _ = row;
            }
        }
        out
    }

    #[test]
    fn a_deterministic_field_has_almost_no_nugget() {
        // The property the whole module is here for. A nugget in an analytic
        // field is aliasing, a discontinuity, or a bad tier handoff — never real
        // microscale variation, because there is no microscale.
        let side = 64;
        let values = sine(side, 0.005, 0.1, 0.004);
        let curve = measure(&values, side, side, 0.005, 0.0, 24);
        assert!(
            curve.nugget_m2 < curve.sill_m2 * 0.02,
            "nugget {} against sill {}",
            curve.nugget_m2,
            curve.sill_m2
        );
    }

    #[test]
    fn a_sines_sill_is_about_its_own_variance() {
        // γ for a sine is `A²/2 · (1 − cos(2πh/λ))`, which *oscillates about*
        // the variance rather than plateauing at it: a periodic field never
        // decorrelates. So the sill estimate is checked to the right order
        // rather than to a tight tolerance, and the tight check lives on the
        // nugget, which a sine does pin exactly.
        let side = 96;
        let amplitude = 0.004;
        let values = sine(side, 0.005, 0.08, amplitude);
        let curve = measure(&values, side, side, 0.005, 0.0, 40);
        let expected = amplitude * amplitude / 2.0;
        assert!(
            curve.sill_m2 > expected * 0.5 && curve.sill_m2 < expected * 2.0,
            "sill {} against a variance of {expected}",
            curve.sill_m2
        );
    }

    #[test]
    fn the_range_tracks_the_feature_wavelength() {
        // Not equal to it — a field decorrelates over something like a feature,
        // not over exactly one — but the same order of magnitude, which is what
        // a band list claiming five centimetres has to produce.
        let side = 96;
        let wavelength = 0.08;
        let values = sine(side, 0.005, wavelength, 0.004);
        let curve = measure(&values, side, side, 0.005, 0.0, 40);
        assert!(
            curve.practical_range_m > wavelength * 0.15
                && curve.practical_range_m < wavelength * 1.5,
            "range {} against wavelength {wavelength}",
            curve.practical_range_m
        );
    }

    #[test]
    fn a_field_that_varies_in_one_axis_is_flat_along_the_other() {
        // The directional half. A field varying only in `u` has zero
        // semivariance along `v` at every lag, which is what makes a directional
        // ripple measurable.
        let side = 64;
        let values = sine(side, 0.005, 0.08, 0.004);
        let across = measure(&values, side, side, 0.005, std::f64::consts::FRAC_PI_2, 20);
        assert!(
            across.sill_m2 < 1.0e-12,
            "semivariance {} across a field that does not vary that way",
            across.sill_m2
        );
    }

    #[test]
    fn a_flat_field_reports_zeros_rather_than_dividing_by_them() {
        let values = vec![0.25f32; 32 * 32];
        let curve = measure(&values, 32, 32, 0.01, 0.0, 8);
        assert_eq!(curve.sill_m2, 0.0);
        assert_eq!(curve.nugget_m2, 0.0);
        assert!(curve.practical_range_m.is_finite());
    }

    #[test]
    fn a_window_too_small_for_the_lag_reports_nothing_rather_than_noise() {
        // A measurement over three sample pairs is noise wearing a number's
        // clothes, so a lag with too few pairs is omitted rather than reported.
        let values = vec![0.0f32; 4 * 4];
        let curve = measure(&values, 4, 4, 0.01, 0.0, 20);
        assert!(curve.samples.iter().all(|s| s.pair_count >= 16));
    }
}

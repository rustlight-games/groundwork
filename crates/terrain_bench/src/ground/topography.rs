//! How rough the ground is, at every scale it claims to have features at.
//!
//! ## Why one roughness number is not enough
//!
//! `Sq` — the RMS of the detrended height — is the number everyone reaches for,
//! and it is necessary and insufficient in a specific way: **two surfaces can
//! share an `Sq` and have completely different feature scales.** A field of
//! five-centimetre clods and a field of five-millimetre crumbs at ten times the
//! count have the same height dispersion and look nothing alike.
//!
//! A procedural band list is itself a scale model. It says "there is structure at
//! five centimetres, at one centimetre, and at two millimetres", and the only way
//! to check that claim is to measure at those scales. So this module reports
//! scalar dispersion *and* scale-dependent height difference, slope and
//! curvature, and the spectral half lives next door in [`super::psd`].
//!
//! ## Detrending is not optional
//!
//! Roughness measured on a tilted plate is dominated by the tilt. The
//! least-squares plane is removed first, and for a scenario with intentional
//! macroform the declared component is removed rather than fitted away blindly —
//! otherwise the analysis quietly deletes the thing being tested.

/// A distribution, summarised the way the report schema wants it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarSummary {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub stddev: f64,
    /// Median absolute deviation. Reported beside the standard deviation
    /// because a procedural field with one pathological seed has a long tail,
    /// and the two numbers disagreeing is the signal.
    pub mad: f64,
    pub p01: f64,
    pub p05: f64,
    pub p95: f64,
    pub p99: f64,
}

impl ScalarSummary {
    pub fn of(values: &[f32]) -> Self {
        if values.is_empty() {
            return Self::empty();
        }
        let mut sorted: Vec<f64> = values.iter().map(|v| *v as f64).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let count = sorted.len();
        let mean = sorted.iter().sum::<f64>() / count as f64;
        let variance = sorted.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / count as f64;
        let median = percentile(&sorted, 0.5);
        let mut deviations: Vec<f64> = sorted.iter().map(|v| (v - median).abs()).collect();
        deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Self {
            count,
            min: sorted[0],
            max: sorted[count - 1],
            mean,
            median,
            stddev: variance.sqrt(),
            mad: percentile(&deviations, 0.5),
            p01: percentile(&sorted, 0.01),
            p05: percentile(&sorted, 0.05),
            p95: percentile(&sorted, 0.95),
            p99: percentile(&sorted, 0.99),
        }
    }

    pub fn empty() -> Self {
        Self {
            count: 0,
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            median: 0.0,
            stddev: 0.0,
            mad: 0.0,
            p01: 0.0,
            p05: 0.0,
            p95: 0.0,
            p99: 0.0,
        }
    }
}

/// Linear-interpolated percentile of a sorted slice.
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = fraction.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    let weight = position - low as f64;
    sorted[low] * (1.0 - weight) + sorted[high] * weight
}

/// The least-squares plane `z = a·u + b·v + c` through a sampled field.
///
/// Returned as well as applied, because a scenario with a deliberate slope wants
/// to check the fit against what it authored rather than trust that the fit
/// removed exactly that and nothing else.
pub fn fit_plane(values: &[f32], columns: usize, rows: usize, spacing_m: f64) -> [f64; 3] {
    if values.is_empty() || columns == 0 || rows == 0 {
        return [0.0; 3];
    }
    // Centred coordinates make the normal equations diagonal, which turns a
    // three-by-three solve into three divisions and removes the ill-conditioning
    // a large world offset would otherwise introduce.
    let half_u = (columns - 1) as f64 * 0.5;
    let half_v = (rows - 1) as f64 * 0.5;
    let (mut sum_z, mut sum_uz, mut sum_vz, mut sum_uu, mut sum_vv) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for row in 0..rows {
        for column in 0..columns {
            let z = values[row * columns + column] as f64;
            let u = (column as f64 - half_u) * spacing_m;
            let v = (row as f64 - half_v) * spacing_m;
            sum_z += z;
            sum_uz += u * z;
            sum_vz += v * z;
            sum_uu += u * u;
            sum_vv += v * v;
        }
    }
    let count = (columns * rows) as f64;
    let a = if sum_uu > 0.0 { sum_uz / sum_uu } else { 0.0 };
    let b = if sum_vv > 0.0 { sum_vz / sum_vv } else { 0.0 };
    // The fit is `z = a·(u − u0) + b·(v − v0) + mean`, where `(u0, v0)` is the
    // field's centre. The caller indexes from the corner, so the intercept it
    // wants is that expression evaluated at `u = v = 0`.
    let mean = sum_z / count;
    let (u0, v0) = (half_u * spacing_m, half_v * spacing_m);
    [a, b, mean - a * u0 - b * v0]
}

/// Remove the fitted plane, returning the residuals.
pub fn detrend(
    values: &[f32],
    columns: usize,
    rows: usize,
    spacing_m: f64,
) -> (Vec<f32>, [f64; 3]) {
    let plane = fit_plane(values, columns, rows, spacing_m);
    let mut out = Vec::with_capacity(values.len());
    for row in 0..rows {
        for column in 0..columns {
            let u = column as f64 * spacing_m;
            let v = row as f64 * spacing_m;
            let fitted = plane[0] * u + plane[1] * v + plane[2];
            out.push((values[row * columns + column] as f64 - fitted) as f32);
        }
    }
    (out, plane)
}

/// Roughness measured at one displacement.
///
/// The three curves that reveal what a band list actually produced: a supposedly
/// five-centimetre clod band that secretly contains two-and-a-half-centimetre
/// detail shows up as slope that keeps rising below its own wavelength.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleMetric {
    pub direction_rad: f64,
    pub lag_m: f64,
    /// `sqrt(½ E[(z(x+l) − z(x))²])`, the height-difference function.
    pub height_difference_rms_m: f64,
    pub slope_rms: f64,
    pub curvature_rms_per_m: f64,
}

/// Everything scalar about a height field.
#[derive(Clone, Debug, PartialEq)]
pub struct TopographyMetrics {
    pub height_m: ScalarSummary,
    /// Mean absolute deviation of the detrended height.
    pub sa_m: f64,
    /// RMS of the detrended height.
    pub sq_m: f64,
    /// Skewness. Positive means peaks dominate, negative means pits do.
    pub ssk: f64,
    /// Kurtosis. Three is Gaussian; higher means a spikier distribution.
    pub sku: f64,
    pub rms_slope: f64,
    pub positive_area_fraction: f64,
    /// How well cavity tracks the hollows it claims to describe.
    ///
    /// The single most important correlation in the whole soil system. Cavity is
    /// occlusion between crumbs, so it must be *negatively* correlated with
    /// height: a shader whose tone comes from noise uncorrelated with its own
    /// relief reads as painted paper however much geometry the mesh carries.
    pub cavity_height_pearson: f64,
    pub cavity_height_spearman: f64,
    pub detrend_plane: [f64; 3],
    pub scale_dependent: Vec<ScaleMetric>,
}

/// Measure one height field, with an optional cavity field beside it.
pub fn measure(
    height: &[f32],
    cavity: Option<&[f32]>,
    columns: usize,
    rows: usize,
    spacing_m: f64,
    lags_m: &[f64],
) -> TopographyMetrics {
    let (residual, plane) = detrend(height, columns, rows, spacing_m);
    let count = residual.len().max(1) as f64;

    let sa = residual.iter().map(|v| (*v as f64).abs()).sum::<f64>() / count;
    let variance = residual.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / count;
    let sq = variance.sqrt();
    let third = residual.iter().map(|v| (*v as f64).powi(3)).sum::<f64>() / count;
    let fourth = residual.iter().map(|v| (*v as f64).powi(4)).sum::<f64>() / count;
    // The epsilon guards a perfectly flat field, where skewness is undefined
    // rather than infinite. Reporting zero says "no asymmetry", which is true.
    let epsilon = 1.0e-30;
    let positive = residual.iter().filter(|v| **v > 0.0).count() as f64 / count;

    let (pearson, spearman) = match cavity {
        None => (0.0, 0.0),
        Some(cavity) => (
            pearson_correlation(&residual, cavity),
            spearman_correlation(&residual, cavity),
        ),
    };

    TopographyMetrics {
        height_m: ScalarSummary::of(height),
        sa_m: sa,
        sq_m: sq,
        ssk: third / (sq.powi(3) + epsilon),
        sku: fourth / (sq.powi(4) + epsilon),
        rms_slope: rms_gradient(&residual, columns, rows, spacing_m),
        positive_area_fraction: positive,
        cavity_height_pearson: pearson,
        cavity_height_spearman: spearman,
        detrend_plane: plane,
        scale_dependent: lags_m
            .iter()
            .flat_map(|lag| {
                // Four directions: the two axes and the two diagonals. Axes
                // alone cannot see a lattice leaking through at 45°, which is
                // exactly the artefact an axis-aligned value-noise grid
                // produces.
                [
                    0.0,
                    std::f64::consts::FRAC_PI_4,
                    std::f64::consts::FRAC_PI_2,
                    3.0 * std::f64::consts::FRAC_PI_4,
                ]
                .into_iter()
                .map(move |direction| (direction, *lag))
            })
            .filter_map(|(direction, lag)| {
                scale_metric(&residual, columns, rows, spacing_m, direction, lag)
            })
            .collect(),
    }
}

/// RMS gradient from centred finite differences.
fn rms_gradient(values: &[f32], columns: usize, rows: usize, spacing_m: f64) -> f64 {
    if columns < 3 || rows < 3 {
        return 0.0;
    }
    let mut total = 0.0;
    let mut count = 0usize;
    for row in 1..rows - 1 {
        for column in 1..columns - 1 {
            let at = |c: usize, r: usize| values[r * columns + c] as f64;
            let du = (at(column + 1, row) - at(column - 1, row)) / (2.0 * spacing_m);
            let dv = (at(column, row + 1) - at(column, row - 1)) / (2.0 * spacing_m);
            total += du * du + dv * dv;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    (total / count as f64).sqrt()
}

/// Height difference, slope and curvature at one lag in one direction.
///
/// Returns `None` when the lag does not fit in the field, which is honest: a
/// measurement over three sample pairs is noise wearing a number's clothes.
fn scale_metric(
    values: &[f32],
    columns: usize,
    rows: usize,
    spacing_m: f64,
    direction_rad: f64,
    lag_m: f64,
) -> Option<ScaleMetric> {
    let steps = (lag_m / spacing_m).round() as i64;
    if steps < 1 {
        return None;
    }
    let du = (direction_rad.cos() * steps as f64).round() as i64;
    let dv = (direction_rad.sin() * steps as f64).round() as i64;
    if du == 0 && dv == 0 {
        return None;
    }
    // The realised lag, not the requested one. A diagonal step of `n` cells is
    // `n√2` metres, and reporting the request would make the diagonal curves
    // sit at the wrong place on every plot.
    let realised = ((du * du + dv * dv) as f64).sqrt() * spacing_m;

    let mut first = 0.0;
    let mut second = 0.0;
    let mut pairs = 0usize;
    let mut triples = 0usize;
    for row in 0..rows {
        for column in 0..columns {
            let forward = (column as i64 + du, row as i64 + dv);
            let backward = (column as i64 - du, row as i64 - dv);
            let inside =
                |c: i64, r: i64| c >= 0 && r >= 0 && (c as usize) < columns && (r as usize) < rows;
            let here = values[row * columns + column] as f64;
            if inside(forward.0, forward.1) {
                let there = values[forward.1 as usize * columns + forward.0 as usize] as f64;
                first += (there - here) * (there - here);
                pairs += 1;
            }
            if inside(forward.0, forward.1) && inside(backward.0, backward.1) {
                let ahead = values[forward.1 as usize * columns + forward.0 as usize] as f64;
                let behind = values[backward.1 as usize * columns + backward.0 as usize] as f64;
                let curve = ahead - 2.0 * here + behind;
                second += curve * curve;
                triples += 1;
            }
        }
    }
    if pairs < 16 {
        return None;
    }
    let mean_first = first / pairs as f64;
    let mean_second = if triples > 0 {
        second / triples as f64
    } else {
        0.0
    };
    Some(ScaleMetric {
        direction_rad,
        lag_m: realised,
        height_difference_rms_m: (0.5 * mean_first).sqrt(),
        slope_rms: mean_first.sqrt() / realised,
        curvature_rms_per_m: mean_second.sqrt() / (realised * realised),
    })
}

/// Pearson correlation between two equal-length planes.
pub fn pearson_correlation(a: &[f32], b: &[f32]) -> f64 {
    let count = a.len().min(b.len());
    if count < 2 {
        return 0.0;
    }
    let mean_a = a[..count].iter().map(|v| *v as f64).sum::<f64>() / count as f64;
    let mean_b = b[..count].iter().map(|v| *v as f64).sum::<f64>() / count as f64;
    let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for index in 0..count {
        let da = a[index] as f64 - mean_a;
        let db = b[index] as f64 - mean_b;
        covariance += da * db;
        var_a += da * da;
        var_b += db * db;
    }
    if var_a <= 0.0 || var_b <= 0.0 {
        return 0.0;
    }
    covariance / (var_a.sqrt() * var_b.sqrt())
}

/// Spearman rank correlation.
///
/// Reported beside Pearson because the cavity-to-height relationship is
/// monotone but not linear — cavity saturates at the bottom of a hollow — and a
/// linear coefficient understates a relationship that is in fact perfect.
pub fn spearman_correlation(a: &[f32], b: &[f32]) -> f64 {
    let count = a.len().min(b.len());
    if count < 2 {
        return 0.0;
    }
    pearson_correlation(&ranks(&a[..count]), &ranks(&b[..count]))
}

/// Fractional ranks, ties averaged.
fn ranks(values: &[f32]) -> Vec<f32> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|x, y| {
        values[*x]
            .partial_cmp(&values[*y])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![0.0f32; values.len()];
    let mut index = 0usize;
    while index < order.len() {
        let mut last = index;
        while last + 1 < order.len() && values[order[last + 1]] == values[order[index]] {
            last += 1;
        }
        let rank = (index + last) as f32 * 0.5;
        for slot in &order[index..=last] {
            out[*slot] = rank;
        }
        index = last + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field of `f(u, v)` over a grid.
    fn field(columns: usize, rows: usize, spacing: f64, f: impl Fn(f64, f64) -> f64) -> Vec<f32> {
        let mut out = Vec::with_capacity(columns * rows);
        for row in 0..rows {
            for column in 0..columns {
                out.push(f(column as f64 * spacing, row as f64 * spacing) as f32);
            }
        }
        out
    }

    #[test]
    fn a_plane_is_fitted_exactly_and_leaves_no_residual() {
        // Roughness measured on a tilted plate is dominated by the tilt, so a
        // detrend that did not remove a plane exactly would report the slope as
        // roughness.
        let values = field(24, 24, 0.05, |u, v| 0.3 * u - 0.7 * v + 1.25);
        let plane = fit_plane(&values, 24, 24, 0.05);
        assert!((plane[0] - 0.3).abs() < 1.0e-6, "{plane:?}");
        assert!((plane[1] + 0.7).abs() < 1.0e-6, "{plane:?}");
        assert!((plane[2] - 1.25).abs() < 1.0e-6, "{plane:?}");

        let (residual, _) = detrend(&values, 24, 24, 0.05);
        let worst = residual.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(worst < 1.0e-4, "residual of {worst} after removing a plane");
    }

    #[test]
    fn a_sine_reports_its_own_amplitude_as_roughness() {
        // `Sq` of a sine of amplitude `A` is `A/√2`. Checked against the closed
        // form rather than against a previous run, so a change to the estimator
        // fails here rather than moving a baseline.
        let amplitude = 0.03;
        let wavelength = 0.4;
        let values = field(129, 129, wavelength / 32.0, |u, _| {
            amplitude * (std::f64::consts::TAU * u / wavelength).sin()
        });
        let metrics = measure(&values, None, 129, 129, wavelength / 32.0, &[]);
        let expected = amplitude / 2.0f64.sqrt();
        assert!(
            (metrics.sq_m - expected).abs() < expected * 0.05,
            "Sq {} against {expected}",
            metrics.sq_m
        );
        // And `Sa` of a sine is `2A/π`.
        let expected_sa = 2.0 * amplitude / std::f64::consts::PI;
        assert!(
            (metrics.sa_m - expected_sa).abs() < expected_sa * 0.05,
            "Sa {} against {expected_sa}",
            metrics.sa_m
        );
    }

    #[test]
    fn a_symmetric_field_has_no_skew() {
        // Exactly four periods across the window, with no duplicated endpoint:
        // sixty-four samples at a sixteenth of a wavelength. A window holding a
        // fractional number of periods is genuinely asymmetric, and the skew it
        // reports would be a property of the crop rather than of the surface.
        let wavelength = 0.2;
        let spacing = wavelength / 16.0;
        let values = field(64, 64, spacing, |u, _| {
            (std::f64::consts::TAU * u / wavelength).sin()
        });
        let metrics = measure(&values, None, 64, 64, spacing, &[]);
        assert!(metrics.ssk.abs() < 0.05, "skew {}", metrics.ssk);
        // A sine's kurtosis is 1.5, well below a Gaussian's three. The
        // tolerance is a fifth rather than a tenth because the detrend removes
        // a small spurious plane: a linear ramp correlates weakly with a
        // periodic signal over a finite window even when the window holds whole
        // periods, and removing that correlation reshapes the distribution a
        // little. Real, and worth knowing about rather than tuned away.
        assert!((metrics.sku - 1.5).abs() < 0.2, "kurtosis {}", metrics.sku);
    }

    #[test]
    fn the_rms_slope_of_a_sine_matches_its_derivative() {
        // `d/du A·sin(2πu/λ)` has RMS `A·2π/(λ√2)`.
        let amplitude = 0.02;
        let wavelength = 0.5;
        let spacing = wavelength / 64.0;
        let values = field(129, 129, spacing, |u, _| {
            amplitude * (std::f64::consts::TAU * u / wavelength).sin()
        });
        let metrics = measure(&values, None, 129, 129, spacing, &[]);
        let expected = amplitude * std::f64::consts::TAU / (wavelength * 2.0f64.sqrt());
        assert!(
            (metrics.rms_slope - expected).abs() < expected * 0.05,
            "slope {} against {expected}",
            metrics.rms_slope
        );
    }

    #[test]
    fn scale_dependent_slope_falls_as_the_lag_passes_the_wavelength() {
        // The curve that reveals what a band list actually produced. Below its
        // own wavelength a band's apparent slope is roughly constant; above it,
        // the height difference saturates and the slope falls as `1/lag`.
        let wavelength = 0.2;
        let spacing = wavelength / 40.0;
        let values = field(161, 161, spacing, |u, _| {
            0.01 * (std::f64::consts::TAU * u / wavelength).sin()
        });
        let metrics = measure(
            &values,
            None,
            161,
            161,
            spacing,
            &[wavelength / 8.0, wavelength, wavelength * 3.0],
        );
        let axis: Vec<&ScaleMetric> = metrics
            .scale_dependent
            .iter()
            .filter(|m| m.direction_rad == 0.0)
            .collect();
        assert_eq!(axis.len(), 3);
        assert!(
            axis[0].slope_rms > axis[2].slope_rms,
            "slope did not fall with lag: {:?}",
            axis.iter().map(|m| m.slope_rms).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cavity_that_tracks_the_hollows_correlates_negatively_with_height() {
        // The single most important correlation in the soil system: the dark in
        // soil is occlusion, not pigment, so cavity has to be high where the
        // ground is low.
        let columns = 48;
        let rows = 48;
        let height = field(columns, rows, 0.01, |u, v| {
            0.01 * ((u * 30.0).sin() + (v * 21.0).cos())
        });
        let cavity: Vec<f32> = height.iter().map(|h| 0.5 - h * 20.0).collect();
        let metrics = measure(&height, Some(&cavity), columns, rows, 0.01, &[]);
        assert!(
            metrics.cavity_height_pearson < -0.95,
            "pearson {}",
            metrics.cavity_height_pearson
        );
        assert!(
            metrics.cavity_height_spearman < -0.95,
            "spearman {}",
            metrics.cavity_height_spearman
        );
    }

    #[test]
    fn spearman_sees_a_monotone_relationship_pearson_understates() {
        // Cavity saturates at the bottom of a hollow, so the relationship is
        // monotone but not linear. Reporting only a linear coefficient would
        // understate a relationship that is in fact perfect.
        let a: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let b: Vec<f32> = a.iter().map(|v| v.powi(3)).collect();
        assert!((spearman_correlation(&a, &b) - 1.0).abs() < 1.0e-6);
        assert!(pearson_correlation(&a, &b) < 0.95);
    }

    #[test]
    fn a_flat_field_reports_zero_rather_than_a_division_by_zero() {
        let values = vec![0.5f32; 64];
        let metrics = measure(&values, None, 8, 8, 0.01, &[0.02]);
        assert_eq!(metrics.sq_m, 0.0);
        assert!(metrics.ssk.is_finite(), "{}", metrics.ssk);
        assert!(metrics.sku.is_finite(), "{}", metrics.sku);
        assert_eq!(metrics.rms_slope, 0.0);
    }

    #[test]
    fn a_summary_reports_the_percentiles_it_claims() {
        let values: Vec<f32> = (0..=100).map(|i| i as f32).collect();
        let summary = ScalarSummary::of(&values);
        assert_eq!(summary.count, 101);
        assert_eq!(summary.min, 0.0);
        assert_eq!(summary.max, 100.0);
        assert!((summary.median - 50.0).abs() < 1.0e-9);
        assert!((summary.p05 - 5.0).abs() < 1.0e-9);
        assert!((summary.p95 - 95.0).abs() < 1.0e-9);
    }
}

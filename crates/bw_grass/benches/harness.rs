//! Shared machinery for the grass benchmarks.
//!
//! Three unrelated things live here because every section of the suite needs
//! all three: a way to time something honestly, a way to build the same scene
//! twice, and a small pile of signal analysis.
//!
//! ## Timing
//!
//! [`sample`] records every run rather than the mean of a batch, because the
//! number that decides whether this ships is not the average cost of a step —
//! it is the *worst* step. Grass is background. A field that averages 0.4 ms
//! and spikes to 6 ms once a second has dropped a frame, and an average would
//! report that as cheaper than a steady 0.9 ms. [`Timing::jitter`] is the ratio
//! that catches it.
//!
//! ## Signals
//!
//! Most of the aesthetic half of this suite is really asking a question about a
//! time series: is this motion smooth, does that gust travel, how long does a
//! trail last. [`high_frequency_ratio`] is the one to understand first — it is
//! how "flicker" becomes a number. A blade that swings through a wide arc over
//! two seconds and a blade that vibrates a hundredth of that arc at 20 Hz can
//! have the same variance, and only one of them is a bug.

// Timing is the point of half this file, and `Instant` is the only way to get
// it. The workspace ban keeps wall-clock time out of the simulation, where it
// would break reproducibility; a benchmark is the one place it belongs.
#![allow(clippy::disallowed_types)]

use std::f32::consts::TAU;
use std::time::Instant;

use bevy::math::Vec2;
use bw_bench::{Measurement, Report, Unit};
use bw_grass::disturbance::GrassInteractor;
use bw_grass::field::{GrassField, SIM_STEP};
use bw_grass::wind::WindField;

// --- timing -----------------------------------------------------------------

/// Per-run durations, in seconds.
pub struct Timing {
    sorted: Vec<f64>,
}

impl Timing {
    /// Typical cost. The median rather than the mean, so one scheduler hiccup
    /// does not move the headline number.
    pub fn median(&self) -> f64 {
        self.quantile(0.50)
    }

    /// The cost of a bad frame.
    pub fn p95(&self) -> f64 {
        self.quantile(0.95)
    }

    /// How uneven the cost is, as p95 over median.
    ///
    /// One is a metronome. Anything much above about 1.5 is a system that will
    /// show as hitching however good its average looks, which for background
    /// grass is the failure that actually matters.
    pub fn jitter(&self) -> f64 {
        let median = self.median();
        if median <= 0.0 {
            return 0.0;
        }
        self.p95() / median
    }

    fn quantile(&self, q: f64) -> f64 {
        if self.sorted.is_empty() {
            return 0.0;
        }
        let index = ((self.sorted.len() - 1) as f64 * q).round() as usize;
        self.sorted[index]
    }
}

/// Time `body` `runs` times, discarding `warmup` runs first.
///
/// The warm-up is not ceremony: the first touch of a freshly allocated field
/// takes page faults that have nothing to do with the cost of a step, and at
/// small resolutions those faults are most of the first measurement.
pub fn sample(warmup: usize, runs: usize, mut body: impl FnMut(usize)) -> Timing {
    for run in 0..warmup {
        body(run);
    }
    let mut samples = Vec::with_capacity(runs);
    for run in 0..runs {
        let start = Instant::now();
        body(warmup + run);
        samples.push(start.elapsed().as_secs_f64());
    }
    samples.sort_by(f64::total_cmp);
    Timing { sorted: samples }
}

// --- reporting --------------------------------------------------------------

/// A report plus a fixed scenario name, so a section of the suite does not have
/// to repeat itself on every line.
pub struct Section<'a> {
    report: &'a mut Report,
    scenario: String,
}

impl<'a> Section<'a> {
    pub fn new(report: &'a mut Report, scenario: impl Into<String>) -> Self {
        Self {
            report,
            scenario: scenario.into(),
        }
    }

    /// Rename the scenario without starting a new borrow.
    pub fn scenario(&mut self, scenario: impl Into<String>) -> &mut Self {
        self.scenario = scenario.into();
        self
    }

    fn push(&mut self, name: &str, value: f64, unit: Unit, higher_is_better: bool) {
        self.report.push(Measurement::new(
            name,
            self.scenario.clone(),
            value,
            unit,
            higher_is_better,
        ));
    }

    /// A dimensionless score. `higher` records which way is better.
    pub fn ratio(&mut self, name: &str, value: f64, higher: bool) {
        self.push(name, value, Unit::Ratio, higher);
    }

    pub fn count(&mut self, name: &str, value: f64, higher: bool) {
        self.push(name, value, Unit::Count, higher);
    }

    pub fn bytes(&mut self, name: &str, value: f64, higher: bool) {
        self.push(name, value, Unit::Bytes, higher);
    }

    /// A duration in seconds, recorded in nanoseconds. Lower is always better.
    pub fn seconds(&mut self, name: &str, value: f64) {
        self.push(name, value * 1.0e9, Unit::Nanoseconds, false);
    }
}

// --- scenes -----------------------------------------------------------------

/// Field resolutions per scenario.
///
/// A grass field covers ground near the camera, so these are areas rather than
/// unit counts. At the shipped cell size of 0.15 m these are 19, 38 and 77
/// metres square.
pub fn resolution(scenario: bw_bench::Scenario) -> usize {
    match scenario {
        bw_bench::Scenario::Small => 128,
        bw_bench::Scenario::Medium => 256,
        bw_bench::Scenario::Large => 512,
    }
}

/// Cell size every benchmark field is built at.
///
/// The shipped one, deliberately. Cell size is not a free parameter of the
/// measurement: the solver's correlation length is expressed in *cells*, so a
/// field built at half the shipped cell size has half the coupling distance in
/// metres and answers a gust differently. Benchmarking at 0.15 m, as this suite
/// did at first, measured a field the game does not run.
pub const CELL: f32 = bw_grass::field::DEFAULT_CELL_SIZE;

/// Dead air. The control condition for every stability measurement.
pub fn calm() -> WindField {
    WindField {
        speed: 0.0,
        turbulence: 0.0,
        gust_strength: 0.0,
        ..Default::default()
    }
}

/// The wind the game ships with.
pub fn breeze() -> WindField {
    WindField::default()
}

/// A light day: mean flow and turbulence, no gust fronts.
///
/// The condition a stability metric should be judged in. Under gusts the field
/// is *supposed* to move a great deal, and a metric that cannot tell wanted
/// motion from unwanted motion will read a gale as a defect.
pub fn ambient() -> WindField {
    WindField {
        speed: 0.9,
        turbulence: 0.45,
        gust_strength: 0.0,
        ..Default::default()
    }
}

/// A uniform, fully grassed field. No generated patchiness, so one seed
/// measures the same thing as the next.
///
/// For **performance** measurements only. Uniformity is what makes a timing
/// comparable — every cell carries the same work — and it is exactly what makes
/// the field useless for judging how the grass looks, because it is not the
/// grass the game grows. Use [`grown_field`] for anything aesthetic.
pub fn uniform_field(resolution: usize) -> GrassField {
    let mut field = GrassField::new(resolution, CELL, bw_bench::SEEDS[0] as u32);
    // The generator's mean, not its floor. This was 0.24 m, which is a hair
    // over the shortest grass the generator produces, so every controlled
    // experiment in the suite was run on grass shorter and therefore stiffer
    // than anything on screen.
    field.make_uniform(0.31, 1.0);
    field
}

/// The field the game actually generates: patchy density, varied blade length
/// and varied stiffness.
///
/// The distinction matters more than it looks. The uniform fixture pins blade
/// length at 0.24 m; the generator runs 0.21 to 0.41 with a mean of 0.31, and
/// wind torque grows with length while natural frequency falls with its square.
/// Measured on the uniform fixture the field's mean bend is 6.7 degrees, and on
/// the generated one it is roughly twice that — so every threshold tuned
/// against the uniform fixture was tuned against grass half as lively as the
/// grass on screen.
pub fn grown_field(resolution: usize) -> GrassField {
    GrassField::new(resolution, CELL, bw_bench::SEEDS[0] as u32)
}

/// A generated field run forward until it has forgotten how it started.
///
/// Every measurement of steady-state behaviour needs this. A field stepped from
/// rest spends its first second answering the fact that it began perfectly
/// upright, which is a transient nothing in the game ever sees.
pub fn settled(resolution: usize, wind: &WindField, seconds: f32) -> GrassField {
    let mut field = grown_field(resolution);
    let mut wind = *wind;
    for _ in 0..(seconds / SIM_STEP) as u32 {
        wind.time += SIM_STEP;
        field.step(SIM_STEP, &wind);
    }
    field
}

/// Something person-sized walking through grass.
pub fn walker(at: Vec2) -> GrassInteractor {
    let mut body = GrassInteractor {
        radius: 0.30,
        falloff: 0.34,
        mass: 90.0,
        previous: at,
        current: at,
    };
    body.move_to(at);
    body
}

// --- statistics -------------------------------------------------------------

pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

pub fn deviation(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = mean(values);
    (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / values.len() as f64).sqrt()
}

/// Coefficient of variation: spread relative to size.
pub fn variation(values: &[f64]) -> f64 {
    let mean = mean(values);
    if mean.abs() <= 1e-12 {
        return 0.0;
    }
    deviation(values) / mean.abs()
}

/// The `q`th quantile of an unsorted slice.
pub fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

/// Smallest over largest, as a similarity in 0..1.
pub fn ratio_similarity(a: f64, b: f64) -> f64 {
    let (low, high) = (a.min(b), a.max(b));
    if high <= 1e-12 {
        return 1.0;
    }
    (low / high).clamp(0.0, 1.0)
}

/// Pearson correlation of two equal-length signals.
pub fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    if n < 2 {
        return 0.0;
    }
    let mean_a = a[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let mean_b = b[..n].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let (mut top, mut left, mut right) = (0.0, 0.0, 0.0);
    for index in 0..n {
        let da = a[index] as f64 - mean_a;
        let db = b[index] as f64 - mean_b;
        top += da * db;
        left += da * da;
        right += db * db;
    }
    let bottom = (left * right).sqrt();
    if bottom <= 1e-12 { 0.0 } else { top / bottom }
}

/// The lag at which `b` best matches `a`, and the correlation there.
///
/// How a travelling gust is measured: probes downwind of each other see the
/// same signal at increasing delays, and the delay against distance is the wave
/// speed. A field that has no wave at all peaks at zero lag.
pub fn best_lag(a: &[f32], b: &[f32], max_lag: usize) -> (usize, f64) {
    let mut best = (0usize, f64::MIN);
    for lag in 0..=max_lag.min(a.len().saturating_sub(2)) {
        let score = correlation(&a[..a.len() - lag], &b[lag..]);
        if score > best.1 {
            best = (lag, score);
        }
    }
    if best.1 == f64::MIN { (0, 0.0) } else { best }
}

/// Fraction of a signal's temporal power above `cut_hz`.
///
/// This is how flicker becomes a number, and the two details that make it
/// trustworthy are both about not lying:
///
/// - The mean is removed first. A signal sitting at a constant offset has all
///   its power at DC, which would swamp the ratio and report every real defect
///   as negligible.
/// - A Hann window is applied. Without it, a signal that simply drifts over the
///   capture — which every gust does — leaks broadband energy across the whole
///   spectrum, and the metric reports smooth slow motion as high-frequency
///   noise. That failure mode is worse than having no metric: it would push
///   tuning toward a *stiller* field to fix a number that was never measuring
///   what it claimed.
pub fn high_frequency_ratio(signal: &[f32], sample_hz: f32, cut_hz: f32) -> f64 {
    let n = signal.len();
    if n < 8 {
        return 0.0;
    }
    let mean = signal.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let windowed: Vec<f64> = signal
        .iter()
        .enumerate()
        .map(|(index, &value)| {
            let hann = 0.5 - 0.5 * (TAU * index as f32 / (n - 1) as f32).cos();
            (value as f64 - mean) * hann as f64
        })
        .collect();

    let (mut total, mut high) = (0.0, 0.0);
    for bin in 1..=n / 2 {
        let frequency = bin as f32 * sample_hz / n as f32;
        let angle = -TAU as f64 * bin as f64 / n as f64;
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, &value) in windowed.iter().enumerate() {
            let phase = angle * index as f64;
            real += value * phase.cos();
            imaginary += value * phase.sin();
        }
        let power = real * real + imaginary * imaginary;
        total += power;
        if frequency >= cut_hz {
            high += power;
        }
    }
    if total <= 1e-18 { 0.0 } else { high / total }
}

/// Period of the strongest oscillation in a signal, in seconds.
///
/// Zero when the signal has no dominant rhythm. Used to check that gusts arrive
/// at a readable rate rather than as a shimmer too fast to follow or a swell too
/// slow to notice.
pub fn dominant_period(signal: &[f32], sample_hz: f32) -> f64 {
    let n = signal.len();
    if n < 8 {
        return 0.0;
    }
    let mean = signal.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
    let centred: Vec<f64> = signal.iter().map(|&v| v as f64 - mean).collect();

    let (mut best_bin, mut best_power) = (0usize, 0.0);
    for bin in 1..=n / 2 {
        let angle = -TAU as f64 * bin as f64 / n as f64;
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (index, &value) in centred.iter().enumerate() {
            let phase = angle * index as f64;
            real += value * phase.cos();
            imaginary += value * phase.sin();
        }
        let power = real * real + imaginary * imaginary;
        if power > best_power {
            best_power = power;
            best_bin = bin;
        }
    }
    if best_bin == 0 {
        return 0.0;
    }
    n as f64 / (best_bin as f64 * sample_hz as f64)
}

/// Seconds for a decaying signal to fall from its peak to `fraction` of it.
///
/// Returns the full capture length when it never gets there, which reads as
/// "longer than we watched" rather than as zero — the opposite of the truth.
pub fn decay_time(signal: &[f32], dt: f32, fraction: f32) -> f64 {
    let Some((peak_index, &peak)) = signal.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))
    else {
        return 0.0;
    };
    if peak <= 0.0 {
        return 0.0;
    }
    for (index, &value) in signal.iter().enumerate().skip(peak_index) {
        if value <= peak * fraction {
            return ((index - peak_index) as f32 * dt) as f64;
        }
    }
    (signal.len() as f32 * dt) as f64
}

/// Exponential time constant fitted over a window, in seconds.
///
/// Least squares on the log of the signal, which is the honest thing to do for
/// something that decays exponentially and much more robust than reading two
/// points off the curve. Samples at or below zero are skipped rather than
/// clamped: a clamp would invent a floor and bend the fit toward it.
pub fn fitted_tau(signal: &[f32], dt: f32, from: usize, to: usize) -> f64 {
    let to = to.min(signal.len());
    if from + 4 > to {
        return 0.0;
    }
    let mut points = Vec::with_capacity(to - from);
    for (offset, &value) in signal[from..to].iter().enumerate() {
        if value > 1e-6 {
            points.push(((from + offset) as f64 * dt as f64, (value as f64).ln()));
        }
    }
    if points.len() < 4 {
        return 0.0;
    }
    let mean_t = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / points.len() as f64;
    let mut top = 0.0;
    let mut bottom = 0.0;
    for (t, y) in &points {
        top += (t - mean_t) * (y - mean_y);
        bottom += (t - mean_t) * (t - mean_t);
    }
    if bottom <= 1e-12 {
        return 0.0;
    }
    let slope = top / bottom;
    // A rising signal has no decay constant to report.
    if slope >= -1e-9 { 0.0 } else { -1.0 / slope }
}

/// Mean of a set of angles as a resultant length in 0..1, weighted.
///
/// One means every sample points the same way; zero means they cancel. Angles
/// cannot be averaged arithmetically — the mean of 179° and -179° is 0°, which
/// is the opposite of the answer — so they are summed as unit vectors.
pub fn resultant(vectors: impl Iterator<Item = (Vec2, f32)>) -> f64 {
    let mut sum = Vec2::ZERO;
    let mut weight = 0.0;
    for (vector, w) in vectors {
        sum += vector.normalize_or_zero() * w;
        weight += w;
    }
    if weight <= 1e-9 {
        return 0.0;
    }
    (sum.length() / weight).clamp(0.0, 1.0) as f64
}

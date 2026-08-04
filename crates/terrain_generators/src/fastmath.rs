//! The transcendentals in the rasteriser's inner loop, done cheaply.
//!
//! A page is a couple of hundred thousand marks and a mark is a hundred-odd
//! ribs, so anything evaluated per rib is evaluated tens of millions of times
//! per page. At that rate `powf` and `sin_cos` stop being function calls and
//! become the shape of the whole cost: a correctly-rounded `powf` is a branchy
//! out-of-line routine with a table in it, and the profile said four of them and
//! two `sin_cos` pairs per rib were most of the stroke pass.
//!
//! ## Why an approximation is honest here
//!
//! These feed brush geometry, not simulation state. The numbers they produce are
//! a blade's bend angle and a taper width, both of which are then multiplied by
//! a random draw and rasterised onto a grid whose smallest addressable step is a
//! third of a screen pixel. An error of one part in a million is four orders of
//! magnitude below anything the page can represent.
//!
//! That is a claim, so it is measured rather than asserted: the tests below
//! bound every routine against `std` across its whole working range, and the
//! bounds are tight enough that a regression in the polynomials fails them
//! rather than quietly softening the grass. The page-level check is the snapshot
//! suite, which compares whole plates pixel for pixel.
//!
//! **Determinism is unaffected.** These are pure `f32` arithmetic — no
//! platform-dependent libm, no fused-multiply-add that some targets contract and
//! others do not (Rust never contracts without `mul_add`), no lookup tables that
//! could be built differently. Two runs of the same binary agree exactly, which
//! is the property the crate actually needs; the baked page was never bitwise
//! portable across architectures because `sinf` itself is not.
//!
//! ## What is here
//!
//! [`log2`] and [`exp2`] as separate halves rather than only a fused [`pow`],
//! because the caller usually wants several powers of the *same* base — a blade
//! needs `s^1.55` and `s^1.4`, and `(1-s)^1.2` and `(1-s)^2.5` — and sharing the
//! logarithm is the difference between four of them and two.

/// Base-two logarithm.
///
/// Splits the float, reduces the mantissa to `[1/√2, √2)` so the series that
/// follows converges fast, and sums four terms of `atanh`. The reduction is
/// what makes four terms enough: without it the mantissa reaches `2`, `t` reaches
/// a third instead of a sixth, and the same accuracy needs roughly twice as
/// many.
///
/// Returns [`f32::NEG_INFINITY`] at zero and [`f32::NAN`] below it, matching
/// `f32::log2` closely enough that callers do not need a different guard.
#[inline]
pub fn log2(x: f32) -> f32 {
    if x <= 0.0 {
        return if x == 0.0 {
            f32::NEG_INFINITY
        } else {
            f32::NAN
        };
    }
    if !x.is_finite() {
        return x;
    }
    let bits = x.to_bits();
    // Subnormals have no implicit leading one, so scale them into the normal
    // range and pay for it in the exponent afterwards.
    let (bits, bias) = if bits < (1 << 23) {
        ((x * 8_388_608.0).to_bits(), 23.0)
    } else {
        (bits, 0.0)
    };

    let mut exponent = ((bits >> 23) & 0xff) as i32 - 127;
    let mut mantissa = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    // Centre on one rather than on 1.5: `t` below is then at most 0.1716 instead
    // of 0.3333, and its seventh power — the first term dropped — is four
    // hundred times smaller.
    if mantissa > std::f32::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }

    // log2(m) = (2/ln2)·atanh((m-1)/(m+1)), truncated after the seventh power.
    let t = (mantissa - 1.0) / (mantissa + 1.0);
    let t2 = t * t;
    const TWO_OVER_LN2: f32 = 2.885_39;
    let series = t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0))));
    exponent as f32 - bias + TWO_OVER_LN2 * series
}

/// Two raised to a power.
///
/// Integer part by assembling an exponent, fractional part by a Taylor series
/// for `exp` over `[-ln2/2, ln2/2]`, where degree six is already past `f32`.
#[inline]
pub fn exp2(x: f32) -> f32 {
    // Past the ends of `f32` itself, where the two-half assembly below would
    // wrap an exponent rather than saturate. Inside them it is exact, including
    // through the subnormal range — which the first version of this threw away
    // at -126 because it assembled the whole exponent at once.
    if x < -150.0 {
        return 0.0;
    }
    if x > 128.0 {
        return f32::INFINITY;
    }
    let whole = x.round_ties_even();
    let fraction = x - whole;

    let z = fraction * std::f32::consts::LN_2;
    let series = 1.0
        + z * (1.0
            + z * (1.0 / 2.0
                + z * (1.0 / 6.0 + z * (1.0 / 24.0 + z * (1.0 / 120.0 + z * (1.0 / 720.0))))));

    // Assembled in two halves, and multiplied in this order on purpose. A single
    // `2^whole` is not representable at either end of the range — `2^128` is
    // already infinity — even where `2^whole · series` comfortably is, because
    // `series` is at most √2 and at least 1/√2. Scaling by half the exponent,
    // then the rest, keeps every intermediate in range. Both halves are exact
    // powers of two, so neither multiply rounds.
    let half = (whole * 0.5).floor();
    let low = f32::from_bits((((half as i32) + 127) as u32) << 23);
    let high = f32::from_bits((((whole - half) as i32 + 127) as u32) << 23);
    series * low * high
}

/// `base^exponent`, for a non-negative base.
///
/// Zero to any positive power is zero, which is the case the callers here
/// actually hit — a blade's parameter starts at exactly zero on every mark.
#[inline]
pub fn pow(base: f32, exponent: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    exp2(exponent * log2(base))
}

/// `2^(exponent · log2_base)`, when the caller already holds the logarithm.
///
/// The reason [`log2`] and [`exp2`] are public. Four powers of two shared bases
/// cost two logarithms this way and four the obvious way.
///
/// **For a positive exponent only.** A base of zero returns zero, which is right
/// for every power the rasteriser raises it to and wrong for the zeroth — where
/// the answer is one — and wrong for a negative one, where it is infinity.
/// Narrowing the contract rather than branching for two cases nothing asks for:
/// the guard below is on the inner loop of the whole crate.
#[inline]
pub fn pow_from_log2(log2_base: f32, exponent: f32) -> f32 {
    if log2_base == f32::NEG_INFINITY {
        return 0.0;
    }
    exp2(exponent * log2_base)
}

/// Sine and cosine together.
///
/// Reduced to a quadrant of `[-π/4, π/4]` — where a degree-seven series is
/// already at `f32` precision — with the quadrant restoring the signs. The two
/// halves of the reduction constant are split so the subtraction stays exact for
/// the argument sizes a blade produces.
#[inline]
pub fn sin_cos(x: f32) -> (f32, f32) {
    const FRAC_2_PI: f32 = std::f32::consts::FRAC_2_PI;
    // π/2 in three pieces. The first two have most of their mantissa cleared, so
    // that `k` times either of them is *exact* for every quadrant count this
    // could see; that is the whole trick, because the naive `x - k * (π/2)`
    // rounds the product itself and the error that leaves behind grows with `x`
    // — at ten turns out it is already thirty times the error of the series it
    // feeds. The third piece is a correction of order 1e-7 and its product is
    // not exact, which does not matter: rounding a term that small rounds it
    // below the precision of everything it is added to.
    const PI_2_A: f32 = 1.570_312_5; // seven mantissa bits
    const PI_2_B: f32 = 4.839_897_2e-4; // ten more
    const PI_2_C: f32 = -1.629_206_8e-7; // the remainder

    let k = (x * FRAC_2_PI).round_ties_even();
    let r = x - k * PI_2_A - k * PI_2_B - k * PI_2_C;

    let r2 = r * r;
    // sin r = r − r³/6 + r⁵/120 − r⁷/5040
    let sin = r * (1.0 + r2 * (-1.0 / 6.0 + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0))));
    // cos r = 1 − r²/2 + r⁴/24 − r⁶/720 + r⁸/40320
    let cos =
        1.0 + r2 * (-1.0 / 2.0 + r2 * (1.0 / 24.0 + r2 * (-1.0 / 720.0 + r2 * (1.0 / 40320.0))));

    // `k` is a whole number well inside i32 for any angle this crate produces;
    // the quadrant is its low two bits.
    match (k as i32) & 3 {
        0 => (sin, cos),
        1 => (cos, -sin),
        2 => (-sin, -cos),
        _ => (-cos, sin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sweeps rather than spot checks, because the failure these guard against
    /// is a polynomial that is fine in the middle of its range and drifts at one
    /// end — which is exactly where a blade's parameter spends its time.
    fn sweep(low: f32, high: f32, count: usize) -> impl Iterator<Item = f32> {
        (0..=count).map(move |i| low + (high - low) * i as f32 / count as f32)
    }

    #[test]
    fn log2_tracks_the_real_thing() {
        let mut worst = 0.0f32;
        for x in sweep(1.0e-6, 4096.0, 20_000) {
            worst = worst.max((log2(x) - x.log2()).abs());
        }
        assert!(worst < 2.0e-6, "worst absolute error {worst}");
    }

    #[test]
    fn log2_handles_the_edges() {
        assert_eq!(log2(0.0), f32::NEG_INFINITY);
        assert!(log2(-1.0).is_nan());
        assert!(log2(f32::INFINITY).is_infinite());
        assert!((log2(1.0)).abs() < 1.0e-7);
        // Subnormal: no implicit leading one, so the split needs its own path.
        assert!((log2(1.0e-40) - (1.0e-40f32).log2()).abs() < 1.0e-4);
    }

    #[test]
    fn exp2_tracks_the_real_thing() {
        // The whole normal range, not a comfortable middle. The exponent
        // assembly is the part most likely to be wrong, and it is only wrong at
        // the ends.
        let mut worst = 0.0f32;
        for x in sweep(-125.0, 127.0, 40_000) {
            let (ours, theirs) = (exp2(x), x.exp2());
            worst = worst.max(((ours - theirs) / theirs).abs());
        }
        // Two units in the last place of an `f32`. The series itself is far
        // better than that; what is left is the rounding of the final scaling
        // multiply, which no amount of polynomial buys back.
        assert!(worst < 3.0e-7, "worst relative error {worst}");
    }

    #[test]
    fn exp2_handles_the_edges() {
        assert_eq!(exp2(-200.0), 0.0);
        assert!(exp2(200.0).is_infinite());
        assert_eq!(exp2(0.0), 1.0);
        assert!((exp2(10.0) - 1024.0).abs() < 1.0e-3);
        // The ends of the format, both of which a single-piece exponent
        // assembly gets wrong: the smallest normal, a subnormal below it, and a
        // value whose rounded exponent is 128 while the value itself is not
        // infinite.
        assert_eq!(exp2(-126.0), f32::MIN_POSITIVE);
        assert_eq!(exp2(-140.0), (-140.0f32).exp2());
        assert!((exp2(127.5) / (127.5f32).exp2() - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn pow_covers_the_exponents_the_rasteriser_uses() {
        // Every fixed exponent in `stroke.rs`, over the whole `0..1` a blade's
        // arc parameter takes.
        let mut worst = 0.0f32;
        // The six fixed exponents in `stroke.rs`, and eleven from across the
        // range `field.rs` *draws* its mound sharpness from — a continuous
        // parameter, so a table of fixed values would not have covered it.
        let drawn: Vec<f32> = (0..=10).map(|i| 1.1 + i as f32 * 0.13).collect();
        for exponent in [0.55, 0.7, 1.2, 1.4, 1.55, 2.5].iter().chain(&drawn) {
            let exponent = *exponent;
            for base in sweep(0.0, 1.0, 20_000) {
                let theirs = base.powf(exponent);
                let ours = pow(base, exponent);
                worst = worst.max((ours - theirs).abs());
            }
        }
        assert!(worst < 5.0e-7, "worst absolute error {worst}");
    }

    #[test]
    fn a_shared_logarithm_gives_the_same_answer() {
        for base in sweep(0.001, 1.0, 1000) {
            let shared = log2(base);
            for exponent in [1.4f32, 1.55, 2.5] {
                assert_eq!(pow_from_log2(shared, exponent), pow(base, exponent));
            }
        }
        assert_eq!(pow_from_log2(log2(0.0), 1.55), 0.0);
    }

    #[test]
    fn sin_cos_tracks_the_real_thing() {
        // Wider than a blade's angles reach, so the quadrant restoration is
        // exercised in both signs and several turns out.
        let mut worst = 0.0f32;
        for x in sweep(-40.0, 40.0, 200_000) {
            let (sin, cos) = sin_cos(x);
            worst = worst.max((sin - x.sin()).abs()).max((cos - x.cos()).abs());
        }
        assert!(worst < 4.0e-7, "worst absolute error {worst}");
    }

    #[test]
    fn sin_cos_stays_on_the_unit_circle() {
        // The property the rasteriser depends on: these two become a direction
        // vector, and a length that drifted would lengthen the blade.
        for x in sweep(-20.0, 20.0, 50_000) {
            let (sin, cos) = sin_cos(x);
            assert!((sin * sin + cos * cos - 1.0).abs() < 1.0e-6, "at {x}");
        }
    }
}

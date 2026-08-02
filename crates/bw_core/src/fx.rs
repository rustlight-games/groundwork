//! Fixed-point arithmetic for simulation state.
//!
//! [`Real`] is the only numeric type simulation code should use for continuous
//! quantities. Converting to `f32` is a presentation concern and belongs at the
//! render boundary — see [`Vec2Fx::to_f32_array`].

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// The simulation's real-number type.
///
/// `I32F32` is backed by an `i64`: roughly ±2.1e9 of range with 2^-32 of
/// precision. That is far more headroom than a battlefield needs, and the
/// generous fractional part means accumulated per-tick integration does not
/// visibly drift.
///
/// This alias is deliberately the single place the precision is chosen. If the
/// range or precision ever needs to change, change it here.
pub type Real = fixed::types::I32F32;

/// `Real` constant from an integer, usable in `const` position.
///
/// `Real::from_num` is not a `const fn`, so integer constants go through
/// `from_bits` instead. Only valid for values inside the integral range.
pub const fn real_from_int(n: i32) -> Real {
    Real::from_bits((n as i64) << 32)
}

/// A ratio expressed exactly, e.g. `real_ratio(1, 60)` for a tick duration.
///
/// Prefer this over `Real::from_num(1.0 / 64.0)`: the float literal is rounded
/// once by the compiler and again by the conversion, whereas this divides once
/// in fixed point.
///
/// Exact when the denominator is a power of two, correctly rounded otherwise —
/// `real_ratio(1, 3)` is as close to a third as `Real` can represent, and no
/// closer. Reproducible either way.
pub fn real_ratio(numerator: i32, denominator: i32) -> Real {
    debug_assert!(denominator != 0, "real_ratio denominator must be non-zero");
    real_from_int(numerator) / real_from_int(denominator)
}

/// `floor(value / divisor)` as an integer.
///
/// Worth having in one place, because the obvious hand-rolled version is
/// wrong. `fixed`'s `to_num` already rounds toward negative infinity, unlike
/// Rust's `as` casts on floats, which truncate toward zero. Code that "corrects
/// for truncation" after calling `to_num` double-corrects and lands one cell
/// too low for every negative input — which shows up as an off-by-one-cell
/// misalignment only on the left and bottom of a map, and is thoroughly
/// unpleasant to track down.
pub fn floor_div_to_int(value: Real, divisor: Real) -> i32 {
    debug_assert!(
        divisor != Real::ZERO,
        "floor_div_to_int divisor must be non-zero"
    );
    (value / divisor).to_num::<i64>() as i32
}

/// Ceiling of `value / divisor`, clamped at zero.
pub fn ceil_div_to_int(value: Real, divisor: Real) -> i32 {
    debug_assert!(
        divisor != Real::ZERO,
        "ceil_div_to_int divisor must be non-zero"
    );
    let q = value / divisor;
    let floored = q.to_num::<i64>();
    let ceiled = if q > Real::from_num(floored) {
        floored + 1
    } else {
        floored
    };
    ceiled.max(0) as i32
}

/// Square root. Deterministic across platforms — CORDIC is pure integer work.
pub fn sqrt(v: Real) -> Real {
    if v <= Real::ZERO {
        return Real::ZERO;
    }
    cordic::sqrt(v)
}

/// Sine and cosine of an angle in radians.
pub fn sin_cos(radians: Real) -> (Real, Real) {
    cordic::sin_cos(radians)
}

/// Angle of the vector `(x, y)` in radians, in `(-pi, pi]`.
pub fn atan2(y: Real, x: Real) -> Real {
    cordic::atan2(y, x)
}

/// A 2D vector in fixed point.
///
/// Deliberately not `bevy_math::Vec2`: that is `f32`, and allowing it into
/// simulation code is exactly the mistake this type exists to prevent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Vec2Fx {
    pub x: Real,
    pub y: Real,
}

impl Vec2Fx {
    pub const ZERO: Self = Self {
        x: Real::ZERO,
        y: Real::ZERO,
    };

    pub const fn new(x: Real, y: Real) -> Self {
        Self { x, y }
    }

    pub const fn splat(v: Real) -> Self {
        Self { x: v, y: v }
    }

    /// Construct from integers, for literals in content and tests.
    pub const fn from_ints(x: i32, y: i32) -> Self {
        Self {
            x: real_from_int(x),
            y: real_from_int(y),
        }
    }

    pub fn dot(self, rhs: Self) -> Real {
        self.x * rhs.x + self.y * rhs.y
    }

    /// Rotated 90 degrees counter-clockwise.
    pub fn perp(self) -> Self {
        Self {
            x: -self.y,
            y: self.x,
        }
    }

    pub fn length_squared(self) -> Real {
        self.dot(self)
    }

    pub fn length(self) -> Real {
        sqrt(self.length_squared())
    }

    pub fn distance_squared(self, rhs: Self) -> Real {
        (self - rhs).length_squared()
    }

    pub fn distance(self, rhs: Self) -> Real {
        sqrt(self.distance_squared(rhs))
    }

    /// Unit vector, or zero when the input is too short to normalise safely.
    ///
    /// Returning zero rather than NaN keeps the failure mode boring: a unit
    /// with no direction stands still instead of teleporting to infinity.
    pub fn normalize_or_zero(self) -> Self {
        let len = self.length();
        if len == Real::ZERO {
            Self::ZERO
        } else {
            self / len
        }
    }

    /// Shorten to `max_len` if longer, otherwise unchanged.
    pub fn clamp_length(self, max_len: Real) -> Self {
        let len = self.length();
        if len > max_len && len != Real::ZERO {
            self / len * max_len
        } else {
            self
        }
    }

    pub fn lerp(self, rhs: Self, t: Real) -> Self {
        self + (rhs - self) * t
    }

    /// Angle in radians, in `(-pi, pi]`.
    pub fn angle(self) -> Real {
        atan2(self.y, self.x)
    }

    /// Unit vector at `radians`.
    pub fn from_angle(radians: Real) -> Self {
        let (s, c) = sin_cos(radians);
        Self { x: c, y: s }
    }

    /// Convert for rendering.
    ///
    /// This is the simulation/presentation boundary. Nothing that feeds back
    /// into simulation state may consume the result — once a value has been
    /// through `f32` it is no longer reproducible.
    pub fn to_f32_array(self) -> [f32; 2] {
        [self.x.to_num::<f32>(), self.y.to_num::<f32>()]
    }
}

impl Add for Vec2Fx {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Vec2Fx {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl Neg for Vec2Fx {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl Mul<Real> for Vec2Fx {
    type Output = Self;
    fn mul(self, rhs: Real) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl Div<Real> for Vec2Fx {
    type Output = Self;
    fn div(self, rhs: Real) -> Self {
        Self {
            x: self.x / rhs,
            y: self.y / rhs,
        }
    }
}

impl AddAssign for Vec2Fx {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Vec2Fx {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl MulAssign<Real> for Vec2Fx {
    fn mul_assign(&mut self, rhs: Real) {
        *self = *self * rhs;
    }
}

impl DivAssign<Real> for Vec2Fx {
    fn div_assign(&mut self, rhs: Real) {
        *self = *self / rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_from_int_matches_from_num() {
        for n in [-1000, -1, 0, 1, 7, 1000] {
            assert_eq!(real_from_int(n), Real::from_num(n));
        }
    }

    #[test]
    fn ratio_is_exact_for_representable_values() {
        assert_eq!(real_ratio(1, 2), Real::from_num(0.5));
        assert_eq!(real_ratio(3, 4) * real_from_int(4), real_from_int(3));
    }

    #[test]
    fn floor_div_rounds_toward_negative_infinity() {
        let two = real_from_int(2);
        assert_eq!(floor_div_to_int(real_from_int(5), two), 2);
        assert_eq!(floor_div_to_int(real_from_int(4), two), 2);
        assert_eq!(floor_div_to_int(real_from_int(3), two), 1);
        assert_eq!(floor_div_to_int(Real::ZERO, two), 0);
        // The cases the naive implementation gets wrong.
        assert_eq!(floor_div_to_int(real_from_int(-3), two), -2);
        assert_eq!(floor_div_to_int(real_from_int(-4), two), -2);
        assert_eq!(floor_div_to_int(real_from_int(-5), two), -3);
    }

    #[test]
    fn ceil_div_rounds_up_and_clamps_at_zero() {
        let two = real_from_int(2);
        assert_eq!(ceil_div_to_int(real_from_int(4), two), 2);
        assert_eq!(ceil_div_to_int(real_from_int(5), two), 3);
        assert_eq!(ceil_div_to_int(real_from_int(-4), two), 0);
    }

    #[test]
    fn sqrt_of_perfect_square_is_exact() {
        assert_eq!(sqrt(real_from_int(16)), real_from_int(4));
    }

    #[test]
    fn sqrt_of_non_positive_is_zero() {
        assert_eq!(sqrt(Real::ZERO), Real::ZERO);
        assert_eq!(sqrt(real_from_int(-4)), Real::ZERO);
    }

    #[test]
    fn normalising_a_zero_vector_yields_zero_not_nan() {
        assert_eq!(Vec2Fx::ZERO.normalize_or_zero(), Vec2Fx::ZERO);
    }

    #[test]
    fn three_four_five_triangle() {
        let v = Vec2Fx::from_ints(3, 4);
        assert_eq!(v.length(), real_from_int(5));
    }

    #[test]
    fn clamp_length_leaves_short_vectors_alone() {
        let v = Vec2Fx::from_ints(1, 0);
        assert_eq!(v.clamp_length(real_from_int(10)), v);
    }

    #[test]
    fn arithmetic_is_bit_reproducible() {
        // The property the whole simulation leans on: the same sequence of
        // operations produces bit-identical results every time.
        let run = || {
            let mut acc = Vec2Fx::ZERO;
            let step = Vec2Fx::new(real_ratio(1, 60), real_ratio(-1, 7));
            for _ in 0..10_000 {
                acc += step;
                acc = acc.clamp_length(real_from_int(100));
            }
            acc
        };
        assert_eq!(run(), run());
    }
}

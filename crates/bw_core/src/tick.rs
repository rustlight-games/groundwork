//! Simulation time.
//!
//! The simulation has no concept of wall-clock time or variable delta. It
//! advances in whole ticks of a fixed duration, and every duration in content
//! is expressed in ticks. A battle is therefore a pure function of its seed and
//! its tick count, which is what makes replays and headless training possible.

use serde::{Deserialize, Serialize};

use crate::fx::{Real, real_from_int, real_ratio};

/// Simulation rate. Physics and combat resolve at this frequency.
///
/// 64 rather than the more conventional 60, and the reason is arithmetic. In
/// binary fixed point a value is exact only when its denominator is a power of
/// two. `1/60` is not: it rounds, `tick_dt() * 60` comes out at 0.9999999963,
/// and integrating it over a long battle accumulates real error. `1/64` is
/// exactly `2^-6`, so `dt` is exact, sixty-four of them sum to exactly one
/// second, and integration drifts by nothing at all.
///
/// Fixed point on its own buys reproducibility — the same wrong answer on every
/// machine. Choosing a power-of-two rate additionally buys correctness, for
/// free. The four extra ticks per second cost nothing; the simulation is not
/// the bottleneck.
pub const TICKS_PER_SECOND: u32 = 64;

/// How often agents choose a new action, in ticks.
///
/// Decisions run at 8 Hz while the simulation runs at 64. A Q-network forward
/// pass per unit per tick is neither affordable nor useful — an auto-battler
/// unit re-evaluating its intent eight times a second already reacts faster
/// than a person can follow, and the eight-fold reduction in inference cost is
/// the difference between training overnight and training over a week.
pub const DECISION_INTERVAL_TICKS: u64 = 8;

/// Decisions per second. Exact, because both constants are powers of two.
pub const DECISIONS_PER_SECOND: u64 = TICKS_PER_SECOND as u64 / DECISION_INTERVAL_TICKS;

/// Monotonic tick counter for one battle.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Tick(pub u64);

impl Tick {
    pub const ZERO: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    pub fn advance(&mut self) {
        self.0 += 1;
    }

    /// Whether agents choose new actions on this tick.
    pub fn is_decision_tick(self) -> bool {
        self.0.is_multiple_of(DECISION_INTERVAL_TICKS)
    }

    /// Which decision step this tick belongs to.
    pub fn decision_index(self) -> u64 {
        self.0 / DECISION_INTERVAL_TICKS
    }

    /// Elapsed simulated seconds.
    ///
    /// Derived from the integer counter rather than accumulated, so it cannot
    /// drift however long a battle runs.
    pub fn elapsed_seconds(self) -> Real {
        real_ratio(self.0 as i32, TICKS_PER_SECOND as i32)
    }

    pub fn from_seconds(seconds: u32) -> Self {
        Self((seconds * TICKS_PER_SECOND) as u64)
    }
}

/// Duration of a single tick in seconds. Exactly `2^-6`.
pub fn tick_dt() -> Real {
    real_ratio(1, TICKS_PER_SECOND as i32)
}

/// Convert a per-second rate into a per-tick amount.
pub fn per_second(rate: Real) -> Real {
    rate / real_from_int(TICKS_PER_SECOND as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_dt_is_exactly_representable() {
        // 2^-6 with a 32-bit fraction is bit 26 of the fixed-point mantissa.
        assert_eq!(tick_dt().to_bits(), 1i64 << 26);
    }

    #[test]
    fn tick_dt_times_rate_is_exactly_one_second() {
        assert_eq!(
            tick_dt() * real_from_int(TICKS_PER_SECOND as i32),
            real_from_int(1)
        );
    }

    #[test]
    fn accumulating_dt_does_not_drift() {
        // The payoff from a power-of-two tick rate: this is exact, not merely
        // reproducible. With 60 ticks per second it lands on 0.9999999963 and
        // the error compounds for as long as the battle lasts.
        let mut acc = Real::ZERO;
        for _ in 0..TICKS_PER_SECOND * 600 {
            acc += tick_dt();
        }
        assert_eq!(acc, real_from_int(600));
    }

    #[test]
    fn decision_ticks_land_on_the_interval() {
        assert!(Tick(0).is_decision_tick());
        assert!(Tick(DECISION_INTERVAL_TICKS).is_decision_tick());
        assert!(!Tick(1).is_decision_tick());
        assert_eq!(Tick(DECISION_INTERVAL_TICKS * 3).decision_index(), 3);
    }

    #[test]
    fn decision_rate_divides_the_tick_rate_evenly() {
        assert_eq!(
            DECISIONS_PER_SECOND * DECISION_INTERVAL_TICKS,
            TICKS_PER_SECOND as u64
        );
    }

    #[test]
    fn seconds_round_trip() {
        assert_eq!(Tick::from_seconds(2).elapsed_seconds(), real_from_int(2));
    }
}

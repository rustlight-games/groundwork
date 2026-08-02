//! Primitives shared by every Backseat Warlord crate.
//!
//! Everything here exists to make the battle simulation reproducible. A battle
//! replayed from the same seed must produce the same result on any machine, in
//! the headless trainer and in the game, today and after a refactor. That is
//! what lets a policy learned during training be trusted in game, and it is
//! what makes a bug reproducible from a seed rather than a screen recording.
//!
//! Three things get you there, and all three live in this crate:
//!
//! - [`fx`] — fixed-point arithmetic. Floats are not reproducible across
//!   platforms once you involve fused multiply-add, x87 excess precision, or a
//!   vectoriser making different choices. Fixed-point is exact integer maths.
//! - [`rng`] — seed splitting rather than a shared mutable generator, so that
//!   parallel systems cannot reorder their way into different random draws.
//! - [`hash`] — a stable hash of world state, so a determinism regression is a
//!   failing test rather than something you notice three weeks later.
//!
//! See `docs/DETERMINISM.md` for the rules that go with these tools.

#![forbid(unsafe_code)]

pub mod fx;
pub mod grid;
pub mod hash;
pub mod ids;
pub mod prelude;
pub mod rng;
pub mod tick;

pub use fx::{
    Real, Vec2Fx, atan2, ceil_div_to_int, floor_div_to_int, real_from_int, real_ratio, sin_cos,
    sqrt,
};
pub use grid::{Grid, GridDims, GridPos};
pub use hash::{StableHash, StableHasher, hash_real};
pub use ids::{ContentId, Interner, TeamId, UnitId};
pub use rng::{RngStream, SimRng};
pub use tick::{
    DECISION_INTERVAL_TICKS, DECISIONS_PER_SECOND, TICKS_PER_SECOND, Tick, per_second, tick_dt,
};

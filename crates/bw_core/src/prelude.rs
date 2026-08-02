//! Common imports. `use bw_core::prelude::*;` in simulation code.

pub use crate::fx::{Real, Vec2Fx, atan2, real_from_int, real_ratio, sin_cos, sqrt};
pub use crate::grid::{Grid, GridDims, GridPos};
pub use crate::hash::{StableHash, StableHasher, hash_real};
pub use crate::ids::{ContentId, Interner, TeamId, UnitId};
pub use crate::rng::{RngStream, SimRng};
pub use crate::tick::{DECISION_INTERVAL_TICKS, TICKS_PER_SECOND, Tick, per_second, tick_dt};

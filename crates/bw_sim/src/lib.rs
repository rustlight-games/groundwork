//! The battle simulation.
//!
//! This crate is the authority on what happens in a fight, and it is designed
//! to be run far more often headless than on screen. A training run plays
//! millions of ticks with no window, no renderer and no GPU; the game plays the
//! same ticks with a camera pointed at them. Both get identical results from
//! identical seeds, which is the whole point.
//!
//! ## Reading this crate
//!
//! [`BattleSim`] is the entry point. It owns a `World` and a `Schedule` and
//! exposes four operations — step, observe, check for an outcome, and hash —
//! which between them are everything the trainer needs. There is deliberately
//! no `App`, no plugin, and no main loop here: those belong to whoever is
//! driving, and the trainer drives very differently from the game.
//!
//! ## Determinism rules
//!
//! Contributors should know these before touching a system:
//!
//! 1. Iterate in [`UnitId`](bw_core::UnitId) order, never raw `Query` order.
//!    Query iteration follows archetype layout, which changes when components
//!    are added or removed.
//! 2. Never mutate shared state from a parallel system without going through
//!    the [`EffectQueue`], which sorts before applying.
//! 3. Draw randomness through [`SimRng`](bw_core::SimRng) with an explicit
//!    salt, never from a shared generator.
//! 4. No floats. No wall-clock. No `HashMap`.
//!
//! `docs/DETERMINISM.md` explains the reasoning; `tests/determinism.rs`
//! enforces it.

#![forbid(unsafe_code)]

pub mod battle;
pub mod components;
pub mod effects;
pub mod resources;
pub mod schedule;
pub mod systems;

pub use battle::{BattleConfig, BattleSim, Outcome, SpawnStats, UnitSpawn};
pub use components::{
    AbilitySlots, Attack, Cooldowns, Dead, Health, Intent, Position, Stats, StatusStack, Target,
    Team, Unit, Velocity,
};
pub use effects::{EffectCtx, EffectHandler, EffectRegistry, PendingEffect};
pub use resources::{Battlefield, EffectQueue, SimClock, SimSeed, UnitIndex};
pub use schedule::{SimSchedule, SimSet};

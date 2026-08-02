//! Simulation components.
//!
//! Every quantity here is fixed point or an integer. If a component ever needs
//! an `f32`, it belongs in `bw_render` instead — it is presentation.

use bevy_ecs::prelude::*;
use bw_core::{ContentId, Real, StableHash, StableHasher, TeamId, Tick, UnitId, Vec2Fx, hash_real};
use serde::{Deserialize, Serialize};

/// Identity. Present on every unit, and the key everything else sorts by.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unit {
    pub id: UnitId,
    /// Which [`CharacterDef`](bw_content::CharacterDef) this was spawned from.
    pub character: ContentId,
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Team(pub TeamId);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Position(pub Vec2Fx);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Velocity(pub Vec2Fx);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub current: Real,
    pub max: Real,
}

impl Health {
    pub fn new(max: Real) -> Self {
        Self { current: max, max }
    }

    pub fn is_alive(&self) -> bool {
        self.current > Real::ZERO
    }

    /// Fraction remaining, 0..=1. Used in observations, so it is clamped rather
    /// than allowed to go negative on an overkill.
    pub fn fraction(&self) -> Real {
        if self.max <= Real::ZERO {
            return Real::ZERO;
        }
        (self.current / self.max).clamp(Real::ZERO, Real::from_num(1))
    }

    pub fn apply_damage(&mut self, amount: Real) {
        self.current -= amount.max(Real::ZERO);
    }

    pub fn heal(&mut self, amount: Real) {
        self.current = (self.current + amount.max(Real::ZERO)).min(self.max);
    }
}

/// Resolved combat statistics, copied from content at spawn.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub move_speed: Real,
    pub attack_damage: Real,
    pub attack_range: Real,
    pub attack_cooldown_ticks: u32,
    pub armor: Real,
    pub radius: Real,
}

/// Basic attack state.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Attack {
    /// Tick the unit may next attack on.
    pub ready_at: Tick,
}

/// The ability slots a unit has, in content order.
///
/// Slot order is part of the action space the network learns over, so it is
/// fixed at spawn and never reordered.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct AbilitySlots {
    pub abilities: Vec<ContentId>,
}

impl AbilitySlots {
    pub const MAX_SLOTS: usize = 4;

    pub fn get(&self, slot: usize) -> Option<ContentId> {
        self.abilities.get(slot).copied()
    }
}

/// Per-slot cooldowns, parallel to [`AbilitySlots`].
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct Cooldowns {
    pub ready_at: Vec<Tick>,
}

impl Cooldowns {
    pub fn for_slots(count: usize) -> Self {
        Self {
            ready_at: vec![Tick::ZERO; count],
        }
    }

    pub fn is_ready(&self, slot: usize, now: Tick) -> bool {
        self.ready_at.get(slot).is_some_and(|&t| now >= t)
    }

    pub fn start(&mut self, slot: usize, now: Tick, duration_ticks: u32) {
        if let Some(entry) = self.ready_at.get_mut(slot) {
            *entry = Tick(now.0 + duration_ticks as u64);
        }
    }
}

/// Statuses currently held, kept sorted by [`ContentId`].
///
/// Sorted rather than insertion-ordered because two units can gain the same
/// pair of statuses in either order, and the resulting stat modifiers must
/// combine identically regardless.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusStack {
    entries: Vec<StatusEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StatusEntry {
    pub status: ContentId,
    pub stacks: u32,
    pub expires_at: Tick,
}

impl StatusStack {
    pub fn entries(&self) -> &[StatusEntry] {
        &self.entries
    }

    pub fn has(&self, status: ContentId) -> bool {
        self.entries.iter().any(|e| e.status == status)
    }

    pub fn stacks_of(&self, status: ContentId) -> u32 {
        self.entries
            .iter()
            .find(|e| e.status == status)
            .map_or(0, |e| e.stacks)
    }

    /// Add or refresh a status.
    pub fn apply(&mut self, status: ContentId, expires_at: Tick, max_stacks: u32, refreshes: bool) {
        match self.entries.binary_search_by_key(&status, |e| e.status) {
            Ok(i) => {
                let entry = &mut self.entries[i];
                if refreshes {
                    entry.expires_at = expires_at.max(entry.expires_at);
                }
                entry.stacks = (entry.stacks + 1).min(max_stacks.max(1));
            }
            Err(i) => {
                self.entries.insert(
                    i,
                    StatusEntry {
                        status,
                        stacks: 1,
                        expires_at,
                    },
                );
            }
        }
    }

    pub fn remove(&mut self, status: ContentId) {
        self.entries.retain(|e| e.status != status);
    }

    /// Drop everything that has expired by `now`.
    pub fn expire(&mut self, now: Tick) {
        self.entries.retain(|e| e.expires_at > now);
    }
}

/// The unit this one is currently fighting.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target(pub Option<UnitId>);

/// What the unit decided to do this decision step.
///
/// Written by the policy at a decision tick and then held for the following
/// ticks, which is what decouples the 8 Hz decision rate from the 64 Hz
/// simulation rate.
///
/// Serialisable because a replay is a seed plus the sequence of intents: that
/// pair reproduces a battle exactly, and is far smaller than recording state.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    pub movement: MoveIntent,
    /// Ability slot to use, if any.
    pub ability: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveIntent {
    #[default]
    Hold,
    /// Advance along the flow field toward the current target.
    Engage,
    /// Back away from the current target.
    Retreat,
    /// One of eight compass directions.
    Direction(u8),
}

/// Marks a unit that has died this tick but not yet been cleaned up.
///
/// Death is deferred rather than immediate so that everything resolving on the
/// same tick sees a consistent world — two units that kill each other should
/// both land their blow.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dead {
    pub at: Tick,
    pub killer: Option<UnitId>,
}

impl StableHash for Health {
    fn stable_hash(&self, h: &mut StableHasher) {
        hash_real(h, self.current);
        hash_real(h, self.max);
    }
}

impl StableHash for StatusStack {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_u64(self.entries.len() as u64);
        for e in &self.entries {
            h.write_u32(e.status.0);
            h.write_u32(e.stacks);
            h.write_u64(e.expires_at.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i32) -> Real {
        bw_core::real_from_int(n)
    }

    #[test]
    fn health_fraction_is_clamped_on_overkill() {
        let mut h = Health::new(r(100));
        h.apply_damage(r(250));
        assert!(!h.is_alive());
        assert_eq!(h.fraction(), Real::ZERO);
    }

    #[test]
    fn healing_does_not_exceed_maximum() {
        let mut h = Health::new(r(100));
        h.apply_damage(r(30));
        h.heal(r(500));
        assert_eq!(h.current, r(100));
    }

    #[test]
    fn zero_maximum_health_does_not_divide_by_zero() {
        assert_eq!(
            Health {
                current: Real::ZERO,
                max: Real::ZERO
            }
            .fraction(),
            Real::ZERO
        );
    }

    #[test]
    fn statuses_stay_sorted_whatever_order_they_arrive_in() {
        // Two units gaining the same statuses in opposite orders must end up
        // with identical stacks, or their stat modifiers diverge.
        let mut a = StatusStack::default();
        a.apply(ContentId(5), Tick(10), 3, true);
        a.apply(ContentId(2), Tick(10), 3, true);
        let mut b = StatusStack::default();
        b.apply(ContentId(2), Tick(10), 3, true);
        b.apply(ContentId(5), Tick(10), 3, true);
        assert_eq!(a, b);
        assert_eq!(
            a.entries().iter().map(|e| e.status.0).collect::<Vec<_>>(),
            [2, 5]
        );
    }

    #[test]
    fn stacks_are_capped_and_refresh_extends() {
        let mut s = StatusStack::default();
        for _ in 0..10 {
            s.apply(ContentId(1), Tick(50), 3, true);
        }
        assert_eq!(s.stacks_of(ContentId(1)), 3);
        s.apply(ContentId(1), Tick(90), 3, true);
        assert_eq!(s.entries()[0].expires_at, Tick(90));
    }

    #[test]
    fn refresh_never_shortens_a_duration() {
        let mut s = StatusStack::default();
        s.apply(ContentId(1), Tick(100), 5, true);
        s.apply(ContentId(1), Tick(10), 5, true);
        assert_eq!(s.entries()[0].expires_at, Tick(100));
    }

    #[test]
    fn expiry_drops_only_elapsed_statuses() {
        let mut s = StatusStack::default();
        s.apply(ContentId(1), Tick(5), 1, true);
        s.apply(ContentId(2), Tick(50), 1, true);
        s.expire(Tick(10));
        assert!(!s.has(ContentId(1)));
        assert!(s.has(ContentId(2)));
    }

    #[test]
    fn cooldowns_gate_on_the_current_tick() {
        let mut c = Cooldowns::for_slots(2);
        assert!(c.is_ready(0, Tick(0)));
        c.start(0, Tick(10), 20);
        assert!(!c.is_ready(0, Tick(29)));
        assert!(c.is_ready(0, Tick(30)));
        // A slot that does not exist is never ready, rather than panicking.
        assert!(!c.is_ready(9, Tick(1000)));
    }
}

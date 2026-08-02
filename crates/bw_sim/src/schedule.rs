//! Tick ordering.

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::ScheduleLabel;

/// The schedule one call to [`BattleSim::step`](crate::BattleSim::step) runs.
#[derive(ScheduleLabel, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SimSchedule;

/// Phases of a tick, run strictly in this order.
///
/// Spelled out rather than inferred from data access. Bevy can order systems
/// automatically from their queries, but that ordering changes when a system's
/// parameters change, and a battle's outcome would change with it. An explicit
/// chain means a reordering is something a person did on purpose and a reviewer
/// can see in the diff.
///
/// The order encodes real rules:
///
/// - `Decision` precedes `Movement` so an intent takes effect on the tick it is
///   chosen, not the one after.
/// - `Effects` follows `Combat`, so a hit and the status it inflicts land
///   together.
/// - `Death` follows both, so two units that kill each other on the same tick
///   both connect. Resolving death immediately would give the earlier-iterated
///   unit an advantage that depends on entity layout.
/// - `Cleanup` is last, so everything above sees a consistent world.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Advance the clock and refresh the unit index.
    Begin,
    /// Refresh what each unit can see: targets, threat, nearby allies.
    Perception,
    /// Policies choose intents. Only runs on decision ticks.
    Decision,
    /// Turn intents into velocities, then integrate positions.
    Movement,
    /// Basic attacks and ability activation, both of which queue effects.
    Combat,
    /// Drain and apply the effect queue.
    Effects,
    /// Tick statuses down and expire them.
    Status,
    /// Mark units whose health has run out.
    Death,
    /// Despawn the dead and settle the outcome.
    Cleanup,
}

impl SimSet {
    /// Every phase in execution order.
    pub const ALL: [SimSet; 9] = [
        SimSet::Begin,
        SimSet::Perception,
        SimSet::Decision,
        SimSet::Movement,
        SimSet::Combat,
        SimSet::Effects,
        SimSet::Status,
        SimSet::Death,
        SimSet::Cleanup,
    ];

    /// Configure the phases into a strict chain on `schedule`.
    ///
    /// Spelled as a tuple rather than built from [`ALL`]: `chain` here is
    /// Bevy's ordering combinator, which is implemented for tuples of sets, not
    /// `Iterator::chain`. The test below keeps the two lists in step.
    ///
    /// [`ALL`]: SimSet::ALL
    pub fn configure(schedule: &mut Schedule) {
        schedule.configure_sets(
            (
                SimSet::Begin,
                SimSet::Perception,
                SimSet::Decision,
                SimSet::Movement,
                SimSet::Combat,
                SimSet::Effects,
                SimSet::Status,
                SimSet::Death,
                SimSet::Cleanup,
            )
                .chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_appears_exactly_once_in_all() {
        let mut seen = SimSet::ALL.to_vec();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "SimSet::ALL contains a duplicate");
        assert_eq!(before, 9);
    }

    #[test]
    fn configuring_sets_on_a_fresh_schedule_succeeds() {
        let mut schedule = Schedule::new(SimSchedule);
        SimSet::configure(&mut schedule);
        let mut world = World::new();
        schedule.run(&mut world);
    }
}

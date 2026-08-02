//! Tick systems.
//!
//! Every system that touches more than one unit iterates
//! [`UnitIndex::sorted_ids`] and looks entities up, rather than iterating a
//! `Query` directly. See the crate docs for why.

use bevy_ecs::prelude::*;
use bw_core::{Real, Tick, UnitId, Vec2Fx, real_from_int, tick_dt};
use bw_nav::avoidance::{Neighbor, SpatialHash, separation};

use crate::components::{
    Attack, Dead, Health, Intent, MoveIntent, Position, Stats, StatusStack, Target, Team, Unit,
    Velocity,
};
use crate::effects::PendingEffect;
use crate::resources::{Battlefield, EffectQueue, SimClock, UnitIndex};

/// Advance the clock and make the unit ordering current.
pub fn begin_tick(mut clock: ResMut<SimClock>, mut index: ResMut<UnitIndex>) {
    clock.0.advance();
    index.refresh();
}

/// Pick each unit's target: the nearest living enemy.
///
/// Ties break toward the lower [`UnitId`], which matters more than it looks —
/// two enemies at exactly equal distance is common when lines meet, and without
/// the rule the choice would depend on iteration order.
pub fn update_targets(
    index: Res<UnitIndex>,
    positions: Query<(&Position, &Team, &Health)>,
    mut targets: Query<&mut Target>,
) {
    let living: Vec<(UnitId, Vec2Fx, Team)> = index
        .sorted_ids()
        .iter()
        .filter_map(|&id| {
            let entity = index.entity(id)?;
            let (position, team, health) = positions.get(entity).ok()?;
            health.is_alive().then_some((id, position.0, *team))
        })
        .collect();

    for &(id, position, team) in &living {
        let mut best: Option<(Real, UnitId)> = None;
        for &(other_id, other_position, other_team) in &living {
            if !team.0.is_hostile_to(other_team.0) {
                continue;
            }
            let distance = position.distance_squared(other_position);
            let better = match best {
                None => true,
                // Strictly-less keeps the first (lowest id) on a tie.
                Some((best_distance, _)) => distance < best_distance,
            };
            if better {
                best = Some((distance, other_id));
            }
        }
        if let Some(entity) = index.entity(id)
            && let Ok(mut target) = targets.get_mut(entity)
        {
            target.0 = best.map(|(_, id)| id);
        }
    }
}

/// Turn intents into velocities and integrate positions.
///
/// Movement is steering, not physics: a desired velocity from the flow field,
/// plus a separation push, clamped to the unit's speed. There is no momentum,
/// because an auto-battler wants units that respond immediately to a new order
/// rather than sliding past it.
pub fn apply_movement(
    index: Res<UnitIndex>,
    mut battlefield: ResMut<Battlefield>,
    mut units: Query<(&Position, &mut Velocity, &Stats, &Intent, &Target, &Health)>,
) {
    let ids = index.sorted_ids().to_vec();

    let crowd: Vec<Neighbor> = ids
        .iter()
        .filter_map(|&id| {
            let entity = index.entity(id)?;
            let (position, _, stats, _, _, health) = units.get(entity).ok()?;
            health.is_alive().then_some(Neighbor {
                id,
                position: position.0,
                radius: stats.radius,
            })
        })
        .collect();

    let cell_size = crowd
        .iter()
        .map(|n| n.radius)
        .max()
        .map(|r| r * real_from_int(4))
        .unwrap_or(real_from_int(4));
    let hash = SpatialHash::build(cell_size, crowd.iter().copied());

    // Resolve every unit's desired direction first, so nothing reads a position
    // that has already been advanced this tick.
    let mut desired: Vec<(UnitId, Vec2Fx)> = Vec::with_capacity(ids.len());
    let mut neighbors = Vec::new();

    for &id in &ids {
        let Some(entity) = index.entity(id) else {
            continue;
        };
        let Ok((position, _, stats, intent, target, health)) = units.get(entity) else {
            continue;
        };
        if !health.is_alive() {
            continue;
        }

        let target_position = target
            .0
            .and_then(|t| index.entity(t))
            .and_then(|e| units.get(e).ok())
            .map(|(p, ..)| p.0);

        let steer = match intent.movement {
            MoveIntent::Hold => Vec2Fx::ZERO,
            MoveIntent::Direction(d) => compass_direction(d),
            MoveIntent::Retreat => target_position
                .map(|t| (position.0 - t).normalize_or_zero())
                .unwrap_or(Vec2Fx::ZERO),
            MoveIntent::Engage => match target_position {
                None => Vec2Fx::ZERO,
                Some(t) => {
                    let offset = t - position.0;
                    // Inside attack range there is nothing to gain from closing
                    // further, and pushing on would shove the target around.
                    if offset.length() <= stats.attack_range {
                        Vec2Fx::ZERO
                    } else {
                        flow_direction(&mut battlefield, position.0, t)
                            .unwrap_or_else(|| offset.normalize_or_zero())
                    }
                }
            },
        };

        let me = Neighbor {
            id,
            position: position.0,
            radius: stats.radius,
        };
        hash.query(
            position.0,
            stats.radius * real_from_int(4),
            Some(id),
            &mut neighbors,
        );
        let push = separation(me, &neighbors);

        let combined = (steer + push).clamp_length(real_from_int(1));
        desired.push((id, combined * stats.move_speed));
    }

    for (id, velocity) in desired {
        let Some(entity) = index.entity(id) else {
            continue;
        };
        if let Ok((_, mut current, ..)) = units.get_mut(entity) {
            current.0 = velocity;
        }
    }
}

/// Integrate positions from velocities. Split from steering so that every unit
/// steers against the same snapshot of the world.
pub fn integrate_positions(
    index: Res<UnitIndex>,
    mut units: Query<(&mut Position, &Velocity, &Health)>,
) {
    let dt = tick_dt();
    for &id in index.sorted_ids() {
        let Some(entity) = index.entity(id) else {
            continue;
        };
        let Ok((mut position, velocity, health)) = units.get_mut(entity) else {
            continue;
        };
        if health.is_alive() {
            position.0 += velocity.0 * dt;
        }
    }
}

/// Direction from the flow field toward `goal`, falling back to `None` when the
/// goal is off the map or unreachable.
fn flow_direction(battlefield: &mut Battlefield, from: Vec2Fx, goal: Vec2Fx) -> Option<Vec2Fx> {
    let grid = *battlefield.costs.grid();
    let goal_cell = grid.world_to_cell(goal);
    if !grid.contains(goal_cell) {
        return None;
    }
    let Battlefield { costs, flow, .. } = battlefield;
    let field = flow.get_or_build(costs, goal_cell);
    field.direction(grid.world_to_cell(from))
}

/// One of eight compass directions, as a unit vector.
fn compass_direction(index: u8) -> Vec2Fx {
    const DIRECTIONS: [(i32, i32); 8] = [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ];
    let (x, y) = DIRECTIONS[(index as usize) % DIRECTIONS.len()];
    Vec2Fx::from_ints(x, y).normalize_or_zero()
}

/// Basic attacks. Queues damage rather than applying it, so that everything
/// landing on this tick resolves together.
pub fn basic_attacks(
    clock: Res<SimClock>,
    index: Res<UnitIndex>,
    mut queue: ResMut<EffectQueue>,
    mut attackers: Query<(&Position, &Stats, &Target, &Health, &mut Attack)>,
    positions: Query<&Position>,
) {
    let now = clock.0;
    for &id in index.sorted_ids() {
        let Some(entity) = index.entity(id) else {
            continue;
        };
        let Ok((position, stats, target, health, attack)) = attackers.get(entity) else {
            continue;
        };
        if !health.is_alive() || now < attack.ready_at {
            continue;
        }
        let Some(target_id) = target.0 else { continue };
        let Some(target_position) = index.entity(target_id).and_then(|e| positions.get(e).ok())
        else {
            continue;
        };
        if position.0.distance(target_position.0) > stats.attack_range {
            continue;
        }

        let mut params = bw_content::Params::new();
        params.insert(
            "amount",
            bw_content::Value::Num(stats.attack_damage.to_num::<f64>()),
        );
        queue.push(PendingEffect::new("damage", id, vec![target_id]).with_params(params));

        if let Ok((_, stats, _, _, mut attack)) = attackers.get_mut(entity) {
            attack.ready_at = Tick(now.0 + stats.attack_cooldown_ticks as u64);
        }
    }
}

/// Expire statuses whose duration has run out.
pub fn tick_statuses(clock: Res<SimClock>, mut stacks: Query<&mut StatusStack>) {
    let now = clock.0;
    for mut stack in &mut stacks {
        // Only touch the component when something actually expires, so change
        // detection stays meaningful for the renderer.
        if stack.entries().iter().any(|e| e.expires_at <= now) {
            stack.expire(now);
        }
    }
}

/// Mark units whose health has run out.
pub fn mark_dead(
    clock: Res<SimClock>,
    index: Res<UnitIndex>,
    mut commands: Commands,
    units: Query<(&Health, Option<&Dead>)>,
) {
    let now = clock.0;
    for &id in index.sorted_ids() {
        let Some(entity) = index.entity(id) else {
            continue;
        };
        let Ok((health, already_dead)) = units.get(entity) else {
            continue;
        };
        if !health.is_alive() && already_dead.is_none() {
            commands.entity(entity).insert(Dead {
                at: now,
                killer: None,
            });
        }
    }
}

/// Remove the dead from the world and the index.
///
/// Refreshes the index before returning. This is the last system of the tick,
/// so leaving the ordering stale would make every between-tick read — the
/// outcome check, the state hash, the renderer's snapshot — either assert or
/// silently see a stale unit list.
pub fn cleanup_dead(
    mut commands: Commands,
    mut index: ResMut<UnitIndex>,
    dead: Query<(Entity, &Unit), With<Dead>>,
) {
    let mut removed: Vec<UnitId> = dead.iter().map(|(_, unit)| unit.id).collect();
    removed.sort_unstable();
    for (entity, _) in &dead {
        commands.entity(entity).despawn();
    }
    for id in removed {
        index.remove(id);
    }
    index.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compass_directions_are_unit_length_and_wrap() {
        for i in 0u8..16 {
            let d = compass_direction(i);
            assert!(d.length() <= real_from_int(1) + Real::from_num(0.001));
            assert!(d.length() >= Real::from_num(0.99));
        }
        assert_eq!(compass_direction(0), compass_direction(8));
    }
}

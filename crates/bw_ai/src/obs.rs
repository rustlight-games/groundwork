//! Observation encoding.
//!
//! Turning a battle into a fixed-size vector of numbers is the highest-leverage
//! design decision in the whole learning pipeline. Everything the network can
//! possibly learn has to be visible here, and everything visible here costs
//! training time — so the encoding is deliberately small, local, and relative.
//!
//! Relative in particular: positions are encoded as offsets from the observing
//! unit, not as absolute map coordinates. A unit that learns "close the gap to
//! the thing in front of me" generalises to any battlefield; one that learns
//! "walk toward (37, 12)" does not.

use bw_core::{Real, UnitId, Vec2Fx};
use bw_sim::BattleSim;
use bw_sim::components::{Attack, Cooldowns, Health, Position, Stats, Target, Team};
use bw_sim::resources::UnitIndex;

/// Bump whenever the encoding changes in any way.
///
/// A model trained on one encoding and run on another does not crash — it
/// produces confident nonsense, which is far worse. [`ModelManifest`] refuses
/// to load a mismatch.
///
/// [`ModelManifest`]: crate::ModelManifest
pub const OBS_VERSION: u32 = 1;

/// How many nearby units each observation describes.
///
/// Fixed, because the network's input layer is fixed. Six is enough to see the
/// local scrum without making the vector so wide that training slows down;
/// units beyond the nearest six are summarised in the global block instead.
pub const NEARBY_SLOTS: usize = 6;

/// Numbers describing the observing unit.
const SELF_FEATURES: usize = 8;
/// Numbers per nearby unit.
const NEARBY_FEATURES: usize = 6;
/// Numbers describing the battle as a whole.
const GLOBAL_FEATURES: usize = 4;

/// Total length of one observation.
pub const OBS_LEN: usize = SELF_FEATURES + NEARBY_SLOTS * NEARBY_FEATURES + GLOBAL_FEATURES;

/// Distance beyond which nearby units are not reported, in world units.
const PERCEPTION_RANGE: f32 = 20.0;

/// A batch of observations, laid out row-major for a single forward pass.
///
/// Batched on purpose. A forward pass per unit would mean dozens of tiny matrix
/// multiplications per decision tick, each dominated by call overhead; one pass
/// over an `n x OBS_LEN` matrix is the same arithmetic at a fraction of the
/// cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObsBatch {
    data: Vec<f32>,
    units: Vec<UnitId>,
}

impl ObsBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a batch directly. For the trainer, which assembles observations
    /// from stored transitions rather than from a live simulation, and for
    /// tests that need a batch of a given size.
    pub fn from_parts(data: Vec<f32>, units: Vec<UnitId>) -> Self {
        debug_assert_eq!(
            data.len(),
            units.len() * OBS_LEN,
            "batch data does not match unit count"
        );
        Self { data, units }
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.units.clear();
    }

    pub fn len(&self) -> usize {
        self.units.len()
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// Row-major `len() * OBS_LEN` values.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Which unit each row describes, in the same order.
    pub fn units(&self) -> &[UnitId] {
        &self.units
    }

    pub fn row(&self, index: usize) -> &[f32] {
        &self.data[index * OBS_LEN..(index + 1) * OBS_LEN]
    }
}

/// Builds [`ObsBatch`]es from a battle.
#[derive(Debug, Default)]
pub struct ObservationEncoder {
    scratch: Vec<(UnitId, Vec2Fx, Team, Health)>,
}

impl ObservationEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Encode every living unit's view of `sim` into `out`.
    pub fn encode(&mut self, sim: &BattleSim, out: &mut ObsBatch) {
        out.clear();
        let world = sim.world();
        let index = world.resource::<UnitIndex>();

        self.scratch.clear();
        for &id in index.sorted_ids() {
            let Some(entity) = index.entity(id) else {
                continue;
            };
            let (Some(position), Some(team), Some(health)) = (
                world.get::<Position>(entity),
                world.get::<Team>(entity),
                world.get::<Health>(entity),
            ) else {
                continue;
            };
            if health.is_alive() {
                self.scratch.push((id, position.0, *team, *health));
            }
        }

        let total = self.scratch.len().max(1) as f32;
        let allies_alive = |team: Team| {
            self.scratch
                .iter()
                .filter(|(_, _, t, _)| *t == team)
                .count() as f32
        };

        for slot in 0..self.scratch.len() {
            let (id, position, team, health) = self.scratch[slot];
            let Some(entity) = index.entity(id) else {
                continue;
            };

            // --- the observing unit -----------------------------------------
            let stats = world.get::<Stats>(entity);
            let attack = world.get::<Attack>(entity);
            let cooldowns = world.get::<Cooldowns>(entity);
            let target = world.get::<Target>(entity).and_then(|t| t.0);
            let now = sim.tick();

            out.data.push(health.fraction().to_num::<f32>());
            out.data
                .push(stats.map_or(0.0, |s| s.move_speed.to_num::<f32>() / 10.0));
            out.data
                .push(stats.map_or(0.0, |s| s.attack_range.to_num::<f32>() / 10.0));
            out.data
                .push(stats.map_or(0.0, |s| s.attack_damage.to_num::<f32>() / 50.0));
            out.data
                .push(attack.is_some_and(|a| now >= a.ready_at) as u8 as f32);
            // Ability readiness, as a fraction of slots available now.
            let ready = cooldowns.map_or(0.0, |c| {
                let n = c.ready_at.len().max(1) as f32;
                c.ready_at.iter().filter(|&&t| now >= t).count() as f32 / n
            });
            out.data.push(ready);
            out.data.push(target.is_some() as u8 as f32);
            out.data.push(team.0.0 as f32);

            // --- nearby units ------------------------------------------------
            // Sorted by distance, then unit id, so the same neighbours land in
            // the same slots every tick. Without the tie-break the network sees
            // its inputs shuffle whenever two units are equidistant.
            let mut neighbors: Vec<(Real, UnitId, Vec2Fx, Team, Health)> = self
                .scratch
                .iter()
                .filter(|(other, ..)| *other != id)
                .map(|&(other, other_position, other_team, other_health)| {
                    (
                        position.distance(other_position),
                        other,
                        other_position,
                        other_team,
                        other_health,
                    )
                })
                .filter(|(distance, ..)| distance.to_num::<f32>() <= PERCEPTION_RANGE)
                .collect();
            neighbors.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

            for nearby in 0..NEARBY_SLOTS {
                match neighbors.get(nearby) {
                    None => out.data.extend_from_slice(&[0.0; NEARBY_FEATURES]),
                    Some(&(distance, other_id, other_position, other_team, other_health)) => {
                        let offset = other_position - position;
                        out.data.push(offset.x.to_num::<f32>() / PERCEPTION_RANGE);
                        out.data.push(offset.y.to_num::<f32>() / PERCEPTION_RANGE);
                        out.data.push(distance.to_num::<f32>() / PERCEPTION_RANGE);
                        out.data.push(other_health.fraction().to_num::<f32>());
                        out.data
                            .push(team.0.is_hostile_to(other_team.0) as u8 as f32);
                        out.data.push((Some(other_id) == target) as u8 as f32);
                    }
                }
            }

            // --- the battle as a whole ---------------------------------------
            let friends = allies_alive(team);
            out.data.push(friends / total);
            out.data.push((total - friends) / total);
            out.data.push(neighbors.len() as f32 / total);
            // Elapsed fraction of the tick budget, so a unit can learn that
            // stalling runs out the clock.
            out.data.push((now.0 as f32 / (64.0 * 90.0)).min(1.0));

            out.units.push(id);
        }

        debug_assert_eq!(
            out.data.len(),
            out.units.len() * OBS_LEN,
            "observation length does not match OBS_LEN; bump OBS_VERSION"
        );
    }
}

#[cfg(test)]
mod tests {
    use bw_content::TerrainMap;
    use bw_core::{ContentId, Grid, GridDims, TeamId, real_from_int};
    use bw_sim::components::Stats;
    use bw_sim::effects::EffectRegistry;
    use bw_sim::{BattleConfig, SpawnStats, UnitSpawn};

    use super::*;

    fn sim_with(count: i32) -> BattleSim {
        let grid = Grid::centered(GridDims::new(32, 32), real_from_int(1));
        let mut sim = BattleSim::new(
            BattleConfig::default(),
            TerrainMap::new(grid),
            EffectRegistry::new(),
        );
        for i in 0..count {
            sim.spawn_unit(UnitSpawn {
                character: ContentId(0),
                team: if i % 2 == 0 {
                    TeamId::PLAYER
                } else {
                    TeamId::ENEMY
                },
                position: Vec2Fx::from_ints(i, 0),
                stats: SpawnStats {
                    max_health: real_from_int(50),
                    combat: Stats {
                        move_speed: real_from_int(3),
                        attack_damage: real_from_int(5),
                        attack_range: real_from_int(1),
                        attack_cooldown_ticks: 16,
                        armor: Real::ZERO,
                        radius: Real::from_num(0.5),
                    },
                },
                abilities: vec![ContentId(1)],
            });
        }
        sim
    }

    #[test]
    fn produces_one_fixed_length_row_per_unit() {
        let sim = sim_with(5);
        let mut batch = ObsBatch::new();
        ObservationEncoder::new().encode(&sim, &mut batch);
        assert_eq!(batch.len(), 5);
        assert_eq!(batch.data().len(), 5 * OBS_LEN);
        assert_eq!(batch.row(0).len(), OBS_LEN);
    }

    #[test]
    fn rows_are_ordered_by_unit_id() {
        let sim = sim_with(4);
        let mut batch = ObsBatch::new();
        ObservationEncoder::new().encode(&sim, &mut batch);
        let mut sorted = batch.units().to_vec();
        sorted.sort_unstable();
        assert_eq!(batch.units(), sorted.as_slice());
    }

    #[test]
    fn encoding_is_reproducible() {
        let sim = sim_with(6);
        let mut a = ObsBatch::new();
        let mut b = ObsBatch::new();
        ObservationEncoder::new().encode(&sim, &mut a);
        ObservationEncoder::new().encode(&sim, &mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn empty_slots_are_zeroed_not_omitted() {
        // A single unit has no neighbours, so every nearby slot must still be
        // present and zero — otherwise the row would be short and the whole
        // batch would misalign.
        let sim = sim_with(1);
        let mut batch = ObsBatch::new();
        ObservationEncoder::new().encode(&sim, &mut batch);
        assert_eq!(batch.row(0).len(), OBS_LEN);
        let nearby = &batch.row(0)[SELF_FEATURES..SELF_FEATURES + NEARBY_SLOTS * NEARBY_FEATURES];
        assert!(nearby.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn no_observation_value_is_nan_or_infinite() {
        // A single NaN propagates through the network and poisons every output.
        let sim = sim_with(8);
        let mut batch = ObsBatch::new();
        ObservationEncoder::new().encode(&sim, &mut batch);
        assert!(
            batch.data().iter().all(|v| v.is_finite()),
            "observation contains NaN or inf"
        );
    }

    #[test]
    fn encoding_an_empty_battle_yields_an_empty_batch() {
        let sim = sim_with(0);
        let mut batch = ObsBatch::new();
        ObservationEncoder::new().encode(&sim, &mut batch);
        assert!(batch.is_empty());
    }

    #[test]
    fn positions_are_relative_so_translation_does_not_change_the_view() {
        // The property that makes a learned policy generalise across maps.
        let build = |offset: i32| {
            let grid = Grid::centered(GridDims::new(64, 64), real_from_int(1));
            let mut sim = BattleSim::new(
                BattleConfig::default(),
                TerrainMap::new(grid),
                EffectRegistry::new(),
            );
            for i in 0..2 {
                sim.spawn_unit(UnitSpawn {
                    character: ContentId(0),
                    team: if i == 0 {
                        TeamId::PLAYER
                    } else {
                        TeamId::ENEMY
                    },
                    position: Vec2Fx::from_ints(i * 3 + offset, offset),
                    stats: SpawnStats {
                        max_health: real_from_int(50),
                        combat: Stats {
                            move_speed: real_from_int(3),
                            attack_damage: real_from_int(5),
                            attack_range: real_from_int(1),
                            attack_cooldown_ticks: 16,
                            armor: Real::ZERO,
                            radius: Real::from_num(0.5),
                        },
                    },
                    abilities: vec![],
                });
            }
            let mut batch = ObsBatch::new();
            ObservationEncoder::new().encode(&sim, &mut batch);
            batch.data().to_vec()
        };
        assert_eq!(build(0), build(10));
    }
}

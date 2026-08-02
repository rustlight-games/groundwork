//! The battle façade.
//!
//! [`BattleSim`] is what the trainer drives and what the game wraps. It owns a
//! `World` and a `Schedule` and nothing else — no `App`, no plugins, no main
//! loop — because the trainer wants to run a battle to completion as fast as
//! the CPU allows while the game wants one tick per frame, and neither should
//! have to accommodate the other.

use bevy_ecs::prelude::*;
use bw_content::TerrainMap;
use bw_core::{
    ContentId, Real, SimRng, StableHash, StableHasher, TeamId, Tick, UnitId, Vec2Fx, hash_real,
};

use crate::components::{
    AbilitySlots, Attack, Cooldowns, Health, Intent, Position, Stats, StatusStack, Target, Team,
    Unit, Velocity,
};
use crate::effects::{EffectRegistry, apply_effects};
use crate::resources::{Battlefield, EffectQueue, SimClock, SimSeed, UnitIndex};
use crate::schedule::{SimSchedule, SimSet};
use crate::systems;

/// How a battle ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// One team remains.
    Victory(TeamId),
    /// Everyone died on the same tick.
    Draw,
    /// The tick limit was reached.
    ///
    /// Worth distinguishing from a draw: during training a timeout usually
    /// means both policies learned to disengage, which wants a different reward
    /// than a mutual kill.
    Timeout,
}

/// Battle setup.
#[derive(Clone, Copy, Debug)]
pub struct BattleConfig {
    pub seed: u64,
    /// Hard stop, so a passive pair of policies cannot hang a training run.
    pub max_ticks: u64,
}

impl Default for BattleConfig {
    fn default() -> Self {
        // Ninety seconds at 64 Hz. Long enough for a real fight, short enough
        // that a stalled episode does not dominate a training batch.
        Self {
            seed: 0,
            max_ticks: 64 * 90,
        }
    }
}

/// One battle.
pub struct BattleSim {
    world: World,
    schedule: Schedule,
    config: BattleConfig,
}

impl BattleSim {
    /// Build an empty battle over `terrain`.
    ///
    /// Units are added with [`spawn_unit`]. The registry carries the effect
    /// handlers registered by the plugin crates.
    ///
    /// [`spawn_unit`]: BattleSim::spawn_unit
    pub fn new(config: BattleConfig, terrain: TerrainMap, registry: EffectRegistry) -> Self {
        let mut world = World::new();
        world.insert_resource(SimClock(Tick::ZERO));
        world.insert_resource(SimSeed(SimRng::new(config.seed)));
        world.insert_resource(Battlefield::new(terrain));
        world.insert_resource(EffectQueue::default());
        world.insert_resource(UnitIndex::new());
        world.insert_resource(registry);

        let mut schedule = Schedule::new(SimSchedule);
        SimSet::configure(&mut schedule);
        schedule.add_systems(systems::begin_tick.in_set(SimSet::Begin));
        schedule.add_systems(systems::update_targets.in_set(SimSet::Perception));
        schedule.add_systems(
            (systems::apply_movement, systems::integrate_positions)
                .chain()
                .in_set(SimSet::Movement),
        );
        schedule.add_systems(systems::basic_attacks.in_set(SimSet::Combat));
        schedule.add_systems(apply_effects.in_set(SimSet::Effects));
        schedule.add_systems(systems::tick_statuses.in_set(SimSet::Status));
        schedule.add_systems(systems::mark_dead.in_set(SimSet::Death));
        schedule.add_systems(systems::cleanup_dead.in_set(SimSet::Cleanup));

        Self {
            world,
            schedule,
            config,
        }
    }

    /// Add a unit and return its id.
    pub fn spawn_unit(&mut self, spawn: UnitSpawn) -> UnitId {
        let id = self.world.resource_mut::<UnitIndex>().allocate_id();
        let slots = spawn.abilities.len();
        let entity = self
            .world
            .spawn((
                Unit {
                    id,
                    character: spawn.character,
                },
                Team(spawn.team),
                Position(spawn.position),
                Velocity(Vec2Fx::ZERO),
                Health::new(spawn.stats.max_health),
                spawn.stats.combat,
                Attack::default(),
                AbilitySlots {
                    abilities: spawn.abilities,
                },
                Cooldowns::for_slots(slots),
                StatusStack::default(),
                Target::default(),
                Intent::default(),
            ))
            .id();
        self.world.resource_mut::<UnitIndex>().insert(id, entity);
        self.world.resource_mut::<UnitIndex>().refresh();
        id
    }

    /// Set one unit's intent.
    ///
    /// This is how a policy acts on the world. Intents persist until replaced,
    /// which is what lets decisions run at 8 Hz over a 64 Hz simulation.
    pub fn set_intent(&mut self, unit: UnitId, intent: Intent) {
        let Some(entity) = self.world.resource::<UnitIndex>().entity(unit) else {
            return;
        };
        if let Some(mut existing) = self.world.get_mut::<Intent>(entity) {
            *existing = intent;
        }
    }

    /// Apply a batch of intents, as produced by a policy at a decision tick.
    pub fn apply_intents(&mut self, intents: &[(UnitId, Intent)]) {
        for &(unit, intent) in intents {
            self.set_intent(unit, intent);
        }
    }

    /// Live unit ids, ascending. The canonical order for observations and
    /// actions — a policy's output vector is indexed by position in this list.
    pub fn unit_ids(&self) -> Vec<UnitId> {
        self.world.resource::<UnitIndex>().sorted_ids().to_vec()
    }

    /// Advance one tick.
    pub fn step(&mut self) {
        self.schedule.run(&mut self.world);
    }

    /// Advance until the battle ends or the tick limit is reached.
    pub fn run_to_completion(&mut self) -> Outcome {
        loop {
            if let Some(outcome) = self.outcome() {
                return outcome;
            }
            self.step();
        }
    }

    pub fn tick(&self) -> Tick {
        self.world.resource::<SimClock>().0
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn living_count(&self) -> usize {
        self.world.resource::<UnitIndex>().len()
    }

    /// Whether the battle has ended, and how.
    pub fn outcome(&self) -> Option<Outcome> {
        let index = self.world.resource::<UnitIndex>();
        if index.is_empty() {
            return Some(Outcome::Draw);
        }

        let mut teams: Vec<TeamId> = Vec::new();
        for &id in index.sorted_ids() {
            if let Some(entity) = index.entity(id)
                && let Some(team) = self.world.get::<Team>(entity)
                && !teams.contains(&team.0)
            {
                teams.push(team.0);
            }
        }

        match teams.as_slice() {
            [] => Some(Outcome::Draw),
            [only] => Some(Outcome::Victory(*only)),
            // Still contested — but stop if we have run out of patience.
            _ if self.tick().0 >= self.config.max_ticks => Some(Outcome::Timeout),
            _ => None,
        }
    }

    /// A stable hash of everything that affects the battle's future.
    ///
    /// This is the determinism instrument: two runs that agree here at every
    /// tick are the same battle. It deliberately covers positions, health, and
    /// statuses but not derived caches such as flow fields, which are a
    /// function of the terrain rather than independent state.
    pub fn state_hash(&self) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u64(self.tick().0);

        let index = self.world.resource::<UnitIndex>();
        hasher.write_u64(index.len() as u64);

        for &id in index.sorted_ids() {
            let Some(entity) = index.entity(id) else {
                continue;
            };
            hasher.write_u32(id.0);
            if let Some(team) = self.world.get::<Team>(entity) {
                hasher.write_u8(team.0.0);
            }
            if let Some(position) = self.world.get::<Position>(entity) {
                position.0.stable_hash(&mut hasher);
            }
            if let Some(velocity) = self.world.get::<Velocity>(entity) {
                velocity.0.stable_hash(&mut hasher);
            }
            if let Some(health) = self.world.get::<Health>(entity) {
                health.stable_hash(&mut hasher);
            }
            if let Some(statuses) = self.world.get::<StatusStack>(entity) {
                statuses.stable_hash(&mut hasher);
            }
            if let Some(attack) = self.world.get::<Attack>(entity) {
                hasher.write_u64(attack.ready_at.0);
            }
            if let Some(cooldowns) = self.world.get::<Cooldowns>(entity) {
                hasher.write_u64(cooldowns.ready_at.len() as u64);
                for ready in &cooldowns.ready_at {
                    hasher.write_u64(ready.0);
                }
            }
            if let Some(target) = self.world.get::<Target>(entity) {
                match target.0 {
                    None => hasher.write_u8(0),
                    Some(t) => {
                        hasher.write_u8(1);
                        hasher.write_u32(t.0);
                    }
                }
            }
        }
        hasher.finish()
    }
}

/// Everything needed to place a unit on the field.
#[derive(Clone, Debug)]
pub struct UnitSpawn {
    pub character: ContentId,
    pub team: TeamId,
    pub position: Vec2Fx,
    pub stats: SpawnStats,
    pub abilities: Vec<ContentId>,
}

/// Health plus the combat statistics, already in fixed point.
#[derive(Clone, Copy, Debug)]
pub struct SpawnStats {
    pub max_health: Real,
    pub combat: Stats,
}

impl SpawnStats {
    /// Build from a resolved content definition.
    pub fn from_content(resolved: &bw_content::schema::ResolvedStats) -> Self {
        Self {
            max_health: resolved.max_health,
            combat: Stats {
                move_speed: resolved.move_speed,
                attack_damage: resolved.attack_damage,
                attack_range: resolved.attack_range,
                attack_cooldown_ticks: resolved.attack_cooldown_ticks,
                armor: resolved.armor,
                radius: resolved.radius,
            },
        }
    }
}

/// Hash a `Real` into a battle hash. Re-exported for tools that build their own
/// snapshots.
pub fn hash_real_into(hasher: &mut StableHasher, value: Real) {
    hash_real(hasher, value);
}

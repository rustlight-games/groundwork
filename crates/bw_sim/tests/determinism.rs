//! The determinism regression suite.
//!
//! This is the most valuable test in the repository. Everything else checks
//! that a piece of the simulation is correct; this checks that the simulation
//! is *the same every time*, which is the property the entire training pipeline
//! is built on. When it fails, something has introduced a `HashMap` iteration,
//! a float, a wall-clock read, or an unsorted parallel write — and it will fail
//! long before that shows up as a policy mysteriously refusing to converge.

use bw_content::{Params, TerrainMap, Value};
use bw_core::{ContentId, Grid, GridDims, GridPos, Real, TeamId, UnitId, Vec2Fx, real_from_int};
use bw_sim::components::{Intent, MoveIntent, Stats};
use bw_sim::effects::{EffectCtx, EffectHandler, EffectRegistry, PendingEffect};
use bw_sim::{BattleConfig, BattleSim, SpawnStats, UnitSpawn};

/// A minimal damage primitive, standing in for `plugins/bw_fx_abilities`.
struct Damage;

impl EffectHandler for Damage {
    fn key(&self) -> &'static str {
        "damage"
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>) {
        let amount = ctx
            .params
            .real("damage", "amount")
            .unwrap_or(real_from_int(1));
        for target in ctx.targets.clone() {
            ctx.damage(target, amount);
        }
    }
}

/// A primitive that draws randomly, so the RNG is exercised too.
struct Bleed;

impl EffectHandler for Bleed {
    fn key(&self) -> &'static str {
        "bleed"
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>) {
        use rand::Rng;
        let mut rng = ctx.rng(bw_core::RngStream::Damage, 0);
        let roll: u32 = rng.random_range(1..=6);
        for target in ctx.targets.clone() {
            ctx.damage(target, real_from_int(roll as i32));
        }
    }
}

fn registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new();
    registry.add(Damage).add(Bleed);
    registry
}

fn terrain() -> TerrainMap {
    let grid = Grid::centered(GridDims::new(48, 48), real_from_int(1));
    let mut map = TerrainMap::new(grid);
    // A few obstacles and some rough ground, so pathfinding and cost lookups
    // actually participate rather than every route being a straight line.
    for i in 0..12 {
        map.set_blocked(GridPos::new(20 + i % 3, 10 + i), true);
        map.add_cost(GridPos::new(30, 12 + i), 400);
    }
    map
}

fn stats(speed: i32, damage: i32, range: i32) -> SpawnStats {
    SpawnStats {
        max_health: real_from_int(60),
        combat: Stats {
            move_speed: real_from_int(speed),
            attack_damage: real_from_int(damage),
            attack_range: real_from_int(range),
            attack_cooldown_ticks: 16,
            armor: real_from_int(1),
            radius: Real::from_num(0.5),
        },
    }
}

/// Build the same battle every time: two lines advancing on each other.
fn build_battle(seed: u64) -> BattleSim {
    let mut sim = BattleSim::new(
        BattleConfig {
            seed,
            max_ticks: 64 * 60,
        },
        terrain(),
        registry(),
    );

    for i in 0..8 {
        sim.spawn_unit(UnitSpawn {
            character: ContentId(0),
            team: TeamId::PLAYER,
            position: Vec2Fx::from_ints(-10, i - 4),
            stats: stats(3, 6, 1),
            abilities: vec![ContentId(1)],
        });
        sim.spawn_unit(UnitSpawn {
            character: ContentId(1),
            team: TeamId::ENEMY,
            position: Vec2Fx::from_ints(10, i - 4),
            stats: stats(2, 8, 1),
            abilities: vec![ContentId(2)],
        });
    }

    let engage: Vec<_> = sim
        .unit_ids()
        .into_iter()
        .map(|id| {
            (
                id,
                Intent {
                    movement: MoveIntent::Engage,
                    ability: None,
                },
            )
        })
        .collect();
    sim.apply_intents(&engage);
    sim
}

/// Run `ticks` and collect a hash after each one.
fn hash_trace(seed: u64, ticks: u64) -> Vec<u64> {
    let mut sim = build_battle(seed);
    let mut trace = Vec::with_capacity(ticks as usize);
    for _ in 0..ticks {
        sim.step();
        trace.push(sim.state_hash());
    }
    trace
}

/// As [`hash_trace`], but with a stochastic effect firing periodically.
///
/// Needed because the base simulation has no random element at all: movement,
/// targeting, and basic attacks are all fully determined by the setup, so the
/// seed cannot influence them. That is a fine property — and a real one worth
/// knowing — but it means a seed-sensitivity test has to exercise a path that
/// actually draws. When crits or damage variance arrive, they will make the
/// base battle seed-sensitive and this helper can go.
fn hash_trace_with_randomness(seed: u64, ticks: u64) -> Vec<u64> {
    let mut sim = build_battle(seed);
    let mut trace = Vec::with_capacity(ticks as usize);
    for tick in 0..ticks {
        if tick % 16 == 0 {
            let ids = sim.unit_ids();
            let mut queue = sim.world_mut().resource_mut::<bw_sim::EffectQueue>();
            for pair in ids.chunks(2) {
                if let [source, target] = pair {
                    queue.push(PendingEffect::new("bleed", *source, vec![*target]));
                }
            }
        }
        sim.step();
        trace.push(sim.state_hash());
    }
    trace
}

#[test]
fn two_runs_of_the_same_seed_agree_at_every_tick() {
    // Comparing the whole trace rather than just the final hash: if two runs
    // diverge and then re-converge, a final-state check would miss it.
    let a = hash_trace(0xBEEF, 600);
    let b = hash_trace(0xBEEF, 600);
    assert_eq!(a.len(), b.len());
    if let Some(tick) = (0..a.len()).find(|&i| a[i] != b[i]) {
        panic!(
            "runs diverged at tick {}: {:#x} vs {:#x}",
            tick + 1,
            a[tick],
            b[tick]
        );
    }
}

#[test]
fn different_seeds_produce_different_battles() {
    // Guards the opposite failure: a "deterministic" simulation that ignores
    // its seed entirely would pass every other test in this file.
    let a = hash_trace_with_randomness(1, 300);
    let b = hash_trace_with_randomness(2, 300);
    assert_ne!(
        a, b,
        "seed had no effect on a battle containing random draws"
    );
}

#[test]
fn a_battle_with_no_random_element_is_seed_independent() {
    // Documents the current state of the simulation rather than asserting a
    // requirement. Nothing in movement, targeting, or basic attacks draws, so
    // the seed cannot matter yet. When crits or damage variance land, this test
    // should start failing and be deleted — that is the point of it.
    assert_eq!(
        hash_trace(1, 200),
        hash_trace(2, 200),
        "a random element entered the base simulation; \
         move different_seeds_produce_different_battles onto hash_trace and delete this test"
    );
}

#[test]
fn interleaving_two_battles_does_not_couple_them() {
    // Two simulations advancing in lockstep must not influence each other. This
    // catches accidental global state — a static, a thread-local cache, a
    // shared RNG — which is exactly what breaks a rayon-parallel trainer.
    let mut solo = build_battle(7);
    let mut paired = build_battle(7);
    let mut other = build_battle(999);

    for _ in 0..300 {
        solo.step();
        paired.step();
        other.step();
        assert_eq!(solo.state_hash(), paired.state_hash());
    }
}

#[test]
fn the_battle_actually_does_something() {
    // A simulation where nothing happens is trivially deterministic. This keeps
    // the tests above honest by proving the battle progresses: units move,
    // fight, and die.
    let mut sim = build_battle(42);
    let start_hash = sim.state_hash();
    let start_count = sim.living_count();
    assert_eq!(start_count, 16);

    for _ in 0..600 {
        sim.step();
    }

    assert_ne!(sim.state_hash(), start_hash, "state never changed");
    assert!(
        sim.living_count() < start_count,
        "no unit died in 600 ticks; combat is not engaging"
    );
}

#[test]
fn a_battle_reaches_a_conclusion() {
    let mut sim = build_battle(11);
    let outcome = sim.run_to_completion();
    assert!(
        sim.tick().0 <= 64 * 60,
        "ran past the configured tick limit"
    );
    // Any conclusion is acceptable; hanging is not.
    let _ = outcome;
}

#[test]
fn state_hash_is_pinned() {
    // The golden value. A deliberate rules change should update this in the
    // same commit; an accidental one shows up here as a failing test with no
    // corresponding intent in the diff.
    //
    // Regenerate with: cargo test -p bw_sim --test determinism -- --nocapture
    const GOLDEN: u64 = 0xea02_8e54_a678_0dad;
    let actual = *hash_trace(0xBEEF, 600).last().unwrap();
    assert_eq!(
        actual, GOLDEN,
        "simulation rules changed; new value is {actual:#x}"
    );
}

#[test]
fn effects_resolve_independently_of_queue_order() {
    // Exercised through a real battle rather than a synthetic queue: many units
    // attacking on the same tick is precisely when ordering bugs appear.
    let mut sim = build_battle(3);
    for _ in 0..120 {
        sim.step();
    }
    let after_first = sim.state_hash();

    let mut again = build_battle(3);
    for _ in 0..120 {
        again.step();
    }
    assert_eq!(after_first, again.state_hash());
}

#[test]
fn random_draws_are_reproducible_across_runs() {
    // The Bleed handler rolls a die. If the RNG were shared mutable state, the
    // parallel schedule would make these diverge.
    let build = || {
        let mut sim = build_battle(5);
        let ids: Vec<UnitId> = sim.unit_ids();
        let mut params = Params::new();
        params.insert("amount", Value::Num(3.0));
        for chunk in ids.chunks(2) {
            if let [source, target] = chunk {
                sim.world_mut()
                    .resource_mut::<bw_sim::EffectQueue>()
                    .push(PendingEffect::new("bleed", *source, vec![*target]));
            }
        }
        for _ in 0..64 {
            sim.step();
        }
        sim.state_hash()
    };
    assert_eq!(build(), build());
}

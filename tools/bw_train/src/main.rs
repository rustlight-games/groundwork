//! Headless DQN trainer.
//!
//! `cargo run -p bw_train --release -- --episodes 100`
//!
//! No renderer, no window, no frame pacing — just battles run as fast as the
//! CPU allows. This is why `bw_sim` depends on `bevy_ecs` rather than `bevy`:
//! everything here would otherwise need a GPU and a display server.
//!
//! **Status: skeleton.** The environment loop, the registry wiring shared with
//! the game, and checkpointing are here. Reward shaping and the parallel
//! rayon-backed environment pool are the next pieces.

use std::path::PathBuf;

use bw_ai::net::DqnNetConfig;
use bw_ai::{ModelManifest, ObsBatch, ObservationEncoder, Policy, ScriptedPolicy};
use bw_content::TerrainMap;
use bw_content::terrain::TerrainGenContext;
use bw_content::{Params, registry::GeneratorRegistry};
use bw_core::{ContentId, Grid, GridDims, Real, TeamId, Vec2Fx, real_from_int};
use bw_sim::components::Stats;
use bw_sim::effects::EffectRegistry;
use bw_sim::{BattleConfig, BattleSim, Outcome, SpawnStats, UnitSpawn};
use clap::Parser;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

#[derive(Parser, Debug)]
#[command(about = "Train Backseat Warlord unit policies")]
struct Args {
    /// Battles to run.
    #[arg(long, default_value_t = 10)]
    episodes: u32,

    /// Root seed. Every battle derives from this, so a whole run is
    /// reproducible from one number.
    #[arg(long, default_value_t = 0xBEEF)]
    seed: u64,

    /// Units per side.
    #[arg(long, default_value_t = 8)]
    units: u32,

    /// Where to write weights and the manifest.
    #[arg(long, default_value = "assets/models")]
    out: PathBuf,

    /// Print per-episode results.
    #[arg(long)]
    verbose: bool,
}

/// Both registries, built exactly as the game builds them.
///
/// Duplicated from `bw_app::registries` rather than imported, because importing
/// it would make the trainer depend on the renderer. The test below keeps the
/// two lists honest.
fn effect_registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new();
    bw_fx_abilities::register(&mut registry);
    bw_fx_terrain::register_effects(&mut registry);
    registry
}

fn generator_registry() -> GeneratorRegistry {
    let mut registry = GeneratorRegistry::new();
    bw_fx_terrain::register_generators(&mut registry);
    registry
}

fn build_terrain(seed: u64, dims: GridDims) -> TerrainMap {
    let grid = Grid::centered(dims, real_from_int(1));
    let mut map = TerrainMap::new(grid);
    let registry = generator_registry();
    let params = Params::new();
    if let Some(generator) = registry.terrain("rolling_hills") {
        let ctx = TerrainGenContext {
            grid,
            params: &params,
            salt: seed,
        };
        generator.generate(&ctx, &mut ChaCha8Rng::seed_from_u64(seed), &mut map);
    }
    map
}

fn build_battle(seed: u64, units: u32) -> BattleSim {
    let terrain = build_terrain(seed, GridDims::new(64, 64));
    let mut sim = BattleSim::new(
        BattleConfig {
            seed,
            max_ticks: 64 * 60,
        },
        terrain,
        effect_registry(),
    );

    let stats = SpawnStats {
        max_health: real_from_int(60),
        combat: Stats {
            move_speed: real_from_int(3),
            attack_damage: real_from_int(6),
            attack_range: real_from_int(1),
            attack_cooldown_ticks: 16,
            armor: real_from_int(1),
            radius: Real::from_num(0.5),
        },
    };

    for i in 0..units as i32 {
        for (team, x) in [(TeamId::PLAYER, -12), (TeamId::ENEMY, 12)] {
            sim.spawn_unit(UnitSpawn {
                character: ContentId(0),
                team,
                position: Vec2Fx::from_ints(x, i - units as i32 / 2),
                stats,
                abilities: vec![],
            });
        }
    }
    sim
}

fn main() {
    let args = Args::parse();

    // Scripted for now. Swapping in a DqnLearner-backed policy is the next
    // step; the loop below does not change when that happens.
    let mut policy = ScriptedPolicy;
    let mut encoder = ObservationEncoder::new();
    let mut batch = ObsBatch::new();

    let mut victories = [0u32; 2];
    let mut draws = 0u32;
    let mut timeouts = 0u32;
    let mut total_ticks = 0u64;

    for episode in 0..args.episodes {
        let seed = args.seed.wrapping_add(episode as u64);
        let mut sim = build_battle(seed, args.units);

        let outcome = loop {
            if let Some(outcome) = sim.outcome() {
                break outcome;
            }
            if sim.tick().is_decision_tick() {
                encoder.encode(&sim, &mut batch);
                let intents: Vec<_> = policy
                    .act_for_units(&batch)
                    .into_iter()
                    .map(|(unit, action)| (unit, action.into()))
                    .collect();
                sim.apply_intents(&intents);
            }
            sim.step();
        };

        total_ticks += sim.tick().0;
        match outcome {
            Outcome::Victory(team) => victories[team.0 as usize % 2] += 1,
            Outcome::Draw => draws += 1,
            Outcome::Timeout => timeouts += 1,
        }
        if args.verbose {
            println!(
                "episode {episode}: {outcome:?} after {} ticks",
                sim.tick().0
            );
        }
    }

    println!(
        "{} episodes | player {} enemy {} draw {} timeout {} | {} ticks total",
        args.episodes, victories[0], victories[1], draws, timeouts, total_ticks
    );

    if let Err(error) = write_manifest(&args.out) {
        eprintln!("could not write manifest: {error}");
        std::process::exit(1);
    }
}

fn write_manifest(dir: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let manifest = ModelManifest::current(
        DqnNetConfig::new(bw_ai::obs::OBS_LEN, bw_ai::ActionSpace::size()),
        0,
    );
    manifest
        .save(&dir.join("policy.manifest.ron"))
        .map_err(|e| std::io::Error::other(e.to_string()))
}

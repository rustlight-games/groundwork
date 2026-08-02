//! Content pipeline CLI.
//!
//! `cargo run -p bw_forge -- validate`
//! `cargo run -p bw_forge -- score-rocks`
//!
//! The home for the character production pipeline. Two commands exist today:
//! `validate` loads every RON file and checks every cross-reference, and
//! `score-rocks` runs the rock generator across the standard seeds and reports
//! the aesthetic metrics from `bw_bench`.
//!
//! `score-rocks` is the pattern worth copying for other generators. Generated
//! content has no obvious pass/fail, so the useful thing a tool can do is print
//! the numbers and let a baseline comparison decide — see `docs/BENCHMARKS.md`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bw_bench::{SEEDS, compactness, convexity, luminance_spread, silhouette_variety};
use bw_content::registry::GeneratorRegistry;
use bw_content::{ContentDb, Params};
use bw_sim::effects::EffectRegistry;
use clap::{Parser, Subcommand};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

#[derive(Parser, Debug)]
#[command(about = "Backseat Warlord content tooling")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Load and validate every content file.
    Validate {
        #[arg(long, default_value = "assets/content")]
        dir: PathBuf,
    },
    /// Generate rocks across the standard seeds and report their metrics.
    ScoreRocks {
        #[arg(long, default_value_t = 14)]
        sides: i64,
    },
}

fn effect_registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new();
    bw_fx_abilities::register(&mut registry);
    bw_fx_terrain::register_effects(&mut registry);
    registry
}

fn generator_registry() -> GeneratorRegistry {
    let mut registry = GeneratorRegistry::new();
    bw_fx_terrain::register_generators(&mut registry);
    bw_fx_rocks::register_generators(&mut registry);
    registry
}

fn main() -> ExitCode {
    match Args::parse().command {
        Command::Validate { dir } => validate(&dir),
        Command::ScoreRocks { sides } => score_rocks(sides),
    }
}

fn validate(dir: &Path) -> ExitCode {
    let db = match ContentDb::load_dir(dir) {
        Ok(db) => db,
        Err(error) => {
            eprintln!("load failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    let effects = effect_registry();
    let generators = generator_registry();
    if let Err(error) = db.validate(&effects.key_set(), &generators) {
        eprintln!("validation failed: {error}");
        return ExitCode::FAILURE;
    }

    println!(
        "ok: {} characters, {} abilities, {} statuses, {} terrain, {} rocks, {} props, {} encounters",
        db.characters.len(),
        db.abilities.len(),
        db.statuses.len(),
        db.terrain.len(),
        db.rocks.len(),
        db.props.len(),
        db.encounters.len(),
    );
    ExitCode::SUCCESS
}

fn score_rocks(sides: i64) -> ExitCode {
    let registry = generator_registry();
    let Some(generator) = registry.rock("boulder") else {
        eprintln!("the 'boulder' generator is not registered");
        return ExitCode::FAILURE;
    };

    let mut params = Params::new();
    params.insert("sides", bw_content::Value::Int(sides));

    let mut areas = Vec::new();
    println!(
        "{:>18}  {:>11}  {:>9}  {:>9}",
        "seed", "compactness", "convexity", "contrast"
    );
    for seed in SEEDS {
        let rock = generator.generate(&params, &mut ChaCha8Rng::seed_from_u64(seed));
        let area = rock.signed_area().abs();
        areas.push(area);

        let palette = [rock.palette.shadow, rock.palette.base, rock.palette.light];
        println!(
            "{seed:#018x}  {:>11.3}  {:>9.3}  {:>9.3}",
            compactness(area, rock.perimeter()),
            convexity(&rock.outline),
            luminance_spread(&palette),
        );
    }

    println!("\nvariety across seeds: {:.3}", silhouette_variety(&areas));
    ExitCode::SUCCESS
}

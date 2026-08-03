//! Grow a page in Rust, trace it in Cycles.
//!
//! ```sh
//! cargo run --release -p bw_grass --example grass_cycles
//! cargo run --release -p bw_grass --example grass_cycles -- --size 768 --samples 512
//! cargo run --release -p bw_grass --example grass_cycles -- --px-per-metre 192 --keep
//! ```
//!
//! The eyeball loop for the path-traced tier, and the counterpart to
//! `grass_bake`. Placement happens here; light transport happens in Blender.
//! See [`bw_grass::cycles`] for why the line is drawn there.
//!
//! `BW_BLENDER` overrides where Blender is found.

use std::path::PathBuf;

use bevy::prelude::*;
use bw_grass::bake::{BakeParams, Page};
use bw_grass::cycles::{self, CyclesScene, RenderSettings};
use bw_grass::field::WorldField;
use bw_grass::scene::GrassScene;

fn main() {
    let options = Options::parse();
    let params = BakeParams {
        seed: options.seed,
        ..default()
    };

    let page = Page::at_detail(
        Vec2::ZERO,
        options.size,
        options.size,
        options.px_per_metre / bw_grass::iso::PX_PER_METRE,
    );
    let field = WorldField::lit_by(params.seed, params.light);

    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let grown = GrassScene::build(page, &field, &params);
    let grow = started.elapsed();

    let settings = RenderSettings {
        samples: options.samples,
        device: options.device.clone(),
        view_transform: options.view_transform.clone(),
        blade_width: options.blade_width,
        passes: options.passes,
        ..default()
    };
    let scene = CyclesScene::build(&grown, &field, settings);

    println!(
        "page {}x{} at {:.0} px/m — {} marks, {} curves, {}x{} ground",
        page.width,
        page.height,
        page.px_per_metre,
        grown.len(),
        scene.blades(),
        scene.ground_rows,
        scene.ground_columns,
    );
    println!(
        "camera: ortho {:.4} m, pixel aspect {:.5}, from {:?}",
        scene.camera.ortho_scale, scene.camera.pixel_aspect_y, scene.camera.basis[2],
    );
    println!("grown in {:.2} s", grow.as_secs_f64());

    let directory = PathBuf::from(&options.scene_dir);
    let header = match scene.write(&directory) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("cannot write scene to {}: {error}", directory.display());
            std::process::exit(1);
        }
    };
    println!("scene -> {}", header.display());

    let blender = cycles::blender_path();
    println!("tracing with {}", blender.display());
    #[allow(clippy::disallowed_types)]
    let traced = std::time::Instant::now();
    let output = match cycles::render(&header, &PathBuf::from(&options.out), &blender) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("cannot run {}: {error}", blender.display());
            eprintln!("set BW_BLENDER to the Blender executable");
            std::process::exit(1);
        }
    };

    // Blender is chatty and most of it is not interesting; the script's own
    // lines are, and so is anything on stderr.
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().filter(|l| l.contains("[bw_cycles]")) {
        println!("  {line}");
    }
    // Blender exits zero even when the script raised, so the status is not
    // enough on its own — the only reliable evidence a render happened is the
    // file. Without this check a traceback reports as a successful bake.
    let produced = std::fs::metadata(&options.out)
        .map(|m| m.len())
        .unwrap_or(0);
    if !output.status.success() || produced == 0 {
        eprintln!("--- blender stdout ---\n{stdout}");
        eprintln!(
            "--- blender stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("no image at {}", options.out);
        std::process::exit(1);
    }

    println!(
        "traced in {:.1} s -> {}",
        traced.elapsed().as_secs_f64(),
        options.out
    );
    if !options.keep {
        let _ = std::fs::remove_dir_all(&directory);
    }
}

struct Options {
    size: usize,
    px_per_metre: f32,
    seed: u64,
    samples: u32,
    device: String,
    view_transform: String,
    blade_width: f32,
    passes: bool,
    keep: bool,
    out: String,
    scene_dir: String,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            size: 512,
            // Higher than the 96 the art is authored at, and on purpose. Cycles
            // draws real geometry, and a blade a third of a pixel wide does not
            // become a thin blade — it becomes noise. The target art sits at
            // roughly this scale too.
            px_per_metre: 192.0,
            seed: 7,
            samples: 256,
            device: "GPU".to_string(),
            view_transform: "Standard".to_string(),
            blade_width: 0.35,
            passes: false,
            keep: false,
            out: "target/cycles.png".to_string(),
            scene_dir: "target/cycles-scene".to_string(),
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--size" => options.size = value().parse().unwrap_or(options.size),
                "--px-per-metre" => {
                    options.px_per_metre = value().parse().unwrap_or(options.px_per_metre);
                }
                "--seed" => options.seed = value().parse().unwrap_or(options.seed),
                "--samples" => options.samples = value().parse().unwrap_or(options.samples),
                "--blade-width" => {
                    options.blade_width = value().parse().unwrap_or(options.blade_width);
                }
                "--cpu" => options.device = "CPU".to_string(),
                "--agx" => options.view_transform = "AgX".to_string(),
                "--passes" => options.passes = true,
                "--keep" => options.keep = true,
                "--out" => options.out = value(),
                "--help" | "-h" => {
                    println!(
                        "grass_cycles [--size PX] [--px-per-metre N] [--seed N] [--samples N]\n\
                         \x20            [--blade-width N] [--cpu] [--agx] [--passes] [--keep]\n\
                         \x20            [--out PATH]"
                    );
                    std::process::exit(0);
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
        }
        options
    }
}

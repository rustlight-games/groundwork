//! Grow a page in Rust, trace it in Cycles.
//!
//! ```sh
//! cargo run --release -p terrain_cycles --example grass_cycles
//! cargo run --release -p terrain_cycles --example grass_cycles -- --size 768 --samples 512
//! cargo run --release -p terrain_cycles --example grass_cycles -- --px-per-metre 192 --keep
//! ```
//!
//! The eyeball loop for the path-traced tier, and the counterpart to
//! `grass_bake`. Placement happens here; light transport happens in Blender.
//! See [`terrain_cycles::cycles`] for why the line is drawn there.
//!
//! This is now an argument parser and nothing else. Everything that decides what
//! the picture looks like — the trace resolution, the supersample, the blade
//! width, the tiling and the guard band — lives in [`terrain_cycles::plate`], because
//! a second caller wanted it and none of it should have been reachable only by
//! running an example.
//!
//! `TERRAIN_BLENDER` overrides where Blender is found.

use std::path::PathBuf;

use glam::Vec2;
use terrain_cycles::cycles::RenderSettings;
use terrain_cycles::plate::{self, PlatePlan, PlateRequest, Progress};
use terrain_generators::field::WorldField;
use terrain_generators::style::GrassParams;

fn main() {
    let options = Options::parse();
    let params = plate::scaled_params(
        &GrassParams {
            seed: options.seed,
            ..Default::default()
        },
        options.density,
        options.length,
    );

    // The scale the picture is *shown* at, from the framing.
    let shown_px_per_metre = match options.view {
        Some(metres) => options.height as f32 / metres,
        None => options.px_per_metre,
    };

    let request = PlateRequest {
        width: options.width,
        height: options.height,
        origin: options.origin,
        px_per_metre: shown_px_per_metre,
        supersample: options.supersample,
        tiles: options.tiles,
        blade_width: options.blade_width,
        // A hand-framed laboratory plate: the whole rectangle is the picture,
        // so there is no silhouette to cut. `terrain render --layout nine` is
        // the path that asks for one.
        visible: None,
        settings: RenderSettings {
            samples: options.samples,
            device: options.device.clone(),
            view_transform: options.view_transform.clone(),
            passes: options.passes,
            ..Default::default()
        },
        scene_dir: PathBuf::from(&options.scene_dir),
        keep_scene: options.keep,
    };

    let plan = PlatePlan::resolve(&request, &params);
    println!(
        "{}x{} shown at {shown_px_per_metre:.0} px/m ({:.1}x{:.1} m of ground)",
        options.width,
        options.height,
        options.width as f32 / shown_px_per_metre,
        options.height as f32 / shown_px_per_metre,
    );
    println!(
        "  tracing at {:.0} px/m ({}x), {} ribs, {:.2} width, ~{:.1}M blades",
        plan.trace_px_per_metre,
        plan.supersample,
        plan.ribs,
        plan.blade_width,
        plan.estimated_blades / 1.0e6,
    );
    if plan.tiles_across > 1 {
        println!(
            "  {0}x{0} tiles of {1}x{2}, {3} px guard",
            plan.tiles_across, plan.tile_width, plan.tile_height, plan.guard
        );
    }

    let field = WorldField::lit_by(params.seed, params.light);
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let mut report = |progress: Progress| {
        if progress.tiles > 1 {
            println!(
                "  tile {}/{}  {:.0} s elapsed",
                progress.tile,
                progress.tiles,
                started.elapsed().as_secs_f64()
            );
        }
    };

    let plate = match plate::trace(&request, &params, &field, None, &mut report) {
        Ok(plate) => plate,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    println!(
        "  {} blades over {} tiles, traced in {:.0} s",
        plate.blades,
        plate.plan.tiles(),
        started.elapsed().as_secs_f64()
    );

    if let Err(error) = plate.save(std::path::Path::new(&options.out)) {
        eprintln!("cannot write {}: {error}", options.out);
        std::process::exit(1);
    }
    println!("wrote {}", options.out);
}

struct Options {
    width: usize,
    height: usize,
    view: Option<f32>,
    supersample: usize,
    tiles: usize,
    origin: Vec2,
    px_per_metre: f32,
    seed: u64,
    samples: u32,
    device: String,
    view_transform: String,
    blade_width: f32,
    density: f32,
    length: f32,
    passes: bool,
    keep: bool,
    out: String,
    scene_dir: String,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            width: 512,
            height: 512,
            view: None,
            // Zero means derive: supersample from the trace detail, tiles from
            // the vertex budget.
            supersample: 0,
            tiles: 0,
            origin: Vec2::ZERO,
            // Higher than the 96 the art is authored at, and on purpose. Cycles
            // draws real geometry, and a blade a third of a pixel wide does not
            // become a thin blade — it becomes noise. The target art sits at
            // roughly this scale too.
            px_per_metre: 192.0,
            seed: 7,
            samples: 256,
            device: "GPU".to_string(),
            view_transform: "Standard".to_string(),
            // Zero means derive from the framing — see `plate::blade_width_for`.
            blade_width: 0.0,
            density: plate::CYCLES_DENSITY,
            length: plate::CYCLES_LENGTH,
            passes: false,
            keep: false,
            out: "target/cycles.png".to_string(),
            scene_dir: "target/cycles-scene".to_string(),
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--size" => {
                    let side = value().parse().unwrap_or(options.width);
                    options.width = side;
                    options.height = side;
                }
                "--view" => options.view = value().parse().ok(),
                "--tiles" => options.tiles = value().parse().unwrap_or(options.tiles).clamp(0, 8),
                "--supersample" => {
                    options.supersample =
                        value().parse().unwrap_or(options.supersample).clamp(0, 10);
                }
                "--width" => options.width = value().parse().unwrap_or(options.width),
                "--height" => options.height = value().parse().unwrap_or(options.height),
                "--origin" => {
                    let text = value();
                    let mut parts = text.split(',').map(|p| p.trim().parse::<f32>());
                    if let (Some(Ok(x)), Some(Ok(y))) = (parts.next(), parts.next()) {
                        options.origin = Vec2::new(x, y);
                    }
                }
                "--px-per-metre" => {
                    options.px_per_metre = value().parse().unwrap_or(options.px_per_metre);
                }
                "--seed" => options.seed = value().parse().unwrap_or(options.seed),
                "--samples" => options.samples = value().parse().unwrap_or(options.samples),
                "--blade-width" => {
                    options.blade_width = value().parse().unwrap_or(options.blade_width);
                }
                "--density" => options.density = value().parse().unwrap_or(options.density),
                "--length" => options.length = value().parse().unwrap_or(options.length),
                "--cpu" => options.device = "CPU".to_string(),
                "--agx" => options.view_transform = "AgX".to_string(),
                "--passes" => options.passes = true,
                "--keep" => options.keep = true,
                "--out" => options.out = value(),
                "--help" | "-h" => {
                    println!(
                        "grass_cycles [--size PX] [--px-per-metre N] [--seed N] [--samples N]\n\
                         \x20            [--blade-width N] [--density N] [--length N]\n\
                         \x20            [--cpu] [--agx] [--passes] [--keep]\n\
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

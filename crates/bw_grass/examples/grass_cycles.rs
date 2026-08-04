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
    let mut params = BakeParams {
        seed: options.seed,
        ..default()
    };
    // Density and length are swept from the command line because the counts the
    // rasteriser was tuned to are counts of *strokes covering pixels*, and a
    // path tracer wants counts of *plants occupying space*. They are not the
    // same number and there is no way to derive one from the other.
    params.tufts *= options.density;
    params.fine *= options.density;
    params.thatch *= options.density;
    params.leaves *= options.density;
    params.blade_length.0 *= options.length;
    params.blade_length.1 *= options.length;

    // The scale the picture is *shown* at, from the framing.
    let shown_px_per_metre = match options.view {
        Some(metres) => options.height as f32 / metres,
        None => options.px_per_metre,
    };
    // ## Why a wide view cannot keep the same density
    //
    // Blade count grows with the ground on screen, and ground grows with the
    // square of how far out the camera pulls. The framing this look was tuned at
    // holds 2.2 million blades; the game's own framing covers twenty-five times
    // the ground, which would be fifty-five million — a billion vertices, and
    // Blender will not hold it.
    //
    // It also should not have to. At 39 pixels to the metre a blade is a fifth
    // of a pixel wide and cannot be resolved at all, so drawing every one of
    // them is not detail, it is noise being averaged away at great expense.
    // Fewer and slightly wider is what a mip level *is*, and the eye cannot tell
    // the difference at a scale where no single blade is visible.
    let detail_ratio = (shown_px_per_metre / TUNED_PX_PER_METRE).clamp(0.0, 1.0);
    let crowding = detail_ratio.max(MIN_CROWDING);
    let width_relief = 1.0 / crowding.sqrt();
    // The *tufts* thin. The short grass barely does, and the mat not at all.
    //
    // Thinning everything equally is what turned the wide view into soil with
    // tufts standing on it: the tall marks and the layer that closes the surface
    // between them went together, so the ground opened up exactly as the blades
    // stopped being resolvable. The fine layer is the cheapest geometry in the
    // scene and the only thing holding the canopy shut, so it is the last thing
    // that should be cut.
    params.tufts *= crowding;
    params.fine *= crowding.sqrt().max(0.62);
    params.thatch *= crowding.sqrt().max(0.70);
    params.leaves *= crowding;

    // Traced above the resolution it is stored at, then filtered down. Geometry
    // thinner than a pixel does not become a fine blade — it becomes a partly
    // covered pixel, and at canopy density that averages the field into a flat
    // wash. See `grass_prebake`, where the same mistake was made first.
    let supersample = options.supersample.max(1);
    let traced_width = options.width * supersample;
    let traced_height = options.height * supersample;
    let traced_px_per_metre = shown_px_per_metre * supersample as f32;

    let page = Page::at_detail(
        options.origin * supersample as f32,
        traced_width,
        traced_height,
        traced_px_per_metre / bw_grass::iso::PX_PER_METRE,
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
        // Thinned-out grass is widened to compensate, so the canopy keeps the
        // same coverage rather than opening up as the camera pulls back.
        blade_width: options.blade_width * width_relief,
        passes: options.passes,
        ..default()
    };
    let scene = CyclesScene::build(&grown, &field, settings);

    println!(
        "{}x{} shown at {:.0} px/m ({:.1}x{:.1} m of ground)",
        options.width,
        options.height,
        shown_px_per_metre,
        options.width as f32 / shown_px_per_metre,
        options.height as f32 / shown_px_per_metre,
    );
    println!(
        "  traced {}x{} at {:.0} px/m, {}x supersample, crowding {:.2}",
        traced_width, traced_height, traced_px_per_metre, supersample, crowding,
    );
    println!(
        "  {} marks, {} blades, {}x{} ground",
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

/// The framing the look was tuned at, in pixels per metre.
const TUNED_PX_PER_METRE: f32 = 192.0;

/// The least the canopy may be thinned however far the camera pulls back.
///
/// A third, and it started at a sixth. The mistake was treating the thinning as
/// purely a cost measure: individual blades really are invisible at a wide view,
/// so dropping five of every six looks free — and it is not, because what
/// survives at that distance is **coverage**, and coverage is what says the
/// ground is alive rather than mown. At a sixth the field went to bare soil with
/// tufts on it and the highlight share collapsed to a fifth of the reference's.
const MIN_CROWDING: f32 = 0.34;

struct Options {
    width: usize,
    height: usize,
    view: Option<f32>,
    supersample: usize,
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
            supersample: 1,
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
            blade_width: 0.35,
            // Eight times the rasteriser's counts and blades half again as
            // long. Not arbitrary: the old numbers are counts of *strokes
            // covering pixels*, tuned so a 2D mark vocabulary filled the frame,
            // and a path tracer wants counts of *plants occupying space*. Swept
            // against the target art, these are where the canopy closes and the
            // five gated bands all hold.
            // Six rather than eight, and blades a quarter shorter than they
            // were. At eight and 1.6 the canopy sealed completely: no substrate
            // showed anywhere, and grass with nothing between it reads as fur
            // rather than as plants standing in ground. The reference exposes
            // warm earth between its clumps, and that exposure is doing work —
            // it separates tufts, makes density legible and carries the only
            // warm colour in the picture.
            density: 7.0,
            length: 1.2,
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
                "--supersample" => {
                    options.supersample =
                        value().parse().unwrap_or(options.supersample).clamp(1, 6);
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

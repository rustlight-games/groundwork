//! The terrain framework's command line.
//!
//! ```sh
//! terrain render --out target/plate.png --size 768 --samples 512
//! terrain preview-export --out target/preview.png --size 1024
//! terrain dataset --out target/corpus --shards 8 --aovs
//! ```
//!
//! This is the headless entry point, and its first job is a structural one: it
//! must be possible to grow terrain, trace it through Cycles and export a corpus
//! **without linking the game**. Until this existed, every one of those paths ran
//! through an example inside the grass crate, and the grass crate sat in a
//! workspace whose root package pulled in the simulation, the trainer and the
//! renderer. Nothing was wrong with the code; the dependency graph simply said
//! "this is a game with a grass module in it", and that is the sentence the whole
//! migration is written to change.
//!
//! ## What is a stub, and why it says so loudly
//!
//! `benchmark` needs the terrain fixtures, which arrive with `terrain_bench`. It
//! exits non-zero with the reason rather than printing something reassuring: a
//! command that quietly succeeds while doing nothing is worse than one that is
//! missing, because the first thing built on top of it will be built on sand.
//!
//! ## Transitional dependencies
//!
//! Everything here reaches through `bw_grass`. That is expected at this point in
//! the migration and is the reason this crate exists now rather than later: the
//! entry points move first, the implementations follow, and the two are never
//! broken at the same time.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bevy::math::Vec2;
use clap::{Args, Parser, Subcommand};
use terrain_bake::bake::BakeParams;
use terrain_core::{SampleFootprint, SampleQuery, WorldPoint};
use terrain_cycles::cycles::RenderSettings;
use terrain_cycles::plate::{self, PlatePlan, PlateRequest, Progress};
use terrain_dataset::dataset::{self, CorpusRequest};
use terrain_generators::field::WorldField;
use terrain_generators::page::Page;
use terrain_generators::quality::GrassRenderQuality;
use terrain_generators::scene::GrassScene;
use terrain_generators::style::GrassParams;

#[derive(Parser, Debug)]
#[command(
    name = "terrain",
    about = "Compile, sample and render procedural terrain",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Load an authored terrain document and report every problem with it.
    Validate(DocumentArgs),
    /// Sample a prepared terrain at a point and print what is there.
    Inspect(InspectArgs),
    /// Render a plate through the cheap rasteriser.
    PreviewExport(PreviewArgs),
    /// Render a plate through Cycles.
    Render(RenderArgs),
    /// Generate a paired training corpus.
    Dataset(DatasetArgs),
    /// Run a measurement suite and report against its baseline.
    Benchmark(BenchmarkArgs),
}

#[derive(Args, Debug)]
struct DocumentArgs {
    /// The terrain document to read.
    document: PathBuf,
}

#[derive(Args, Debug)]
struct InspectArgs {
    document: PathBuf,
    /// World position to sample, as `U,V` in metres.
    #[arg(long, value_name = "U,V")]
    at: Option<String>,
    /// Report only this source's value.
    #[arg(long)]
    source: Option<String>,
    /// Sample over a disc of this radius rather than at a point.
    #[arg(long, value_name = "METRES")]
    footprint_m: Option<f64>,
}

/// The framing every raster path shares.
#[derive(Args, Debug, Clone)]
struct Framing {
    /// Output width in pixels.
    #[arg(long, default_value_t = 512)]
    width: usize,
    /// Output height in pixels. Defaults to the width.
    #[arg(long)]
    height: Option<usize>,
    /// Square output, setting both axes at once.
    #[arg(long)]
    size: Option<usize>,
    /// Cache-pixel corner of the plate, as `X,Y`.
    #[arg(long, value_name = "X,Y", default_value = "0,0")]
    origin: String,
    /// World metres visible vertically. Overrides `--px-per-metre`.
    #[arg(long)]
    view: Option<f32>,
    /// Pixels per world metre the plate is shown at.
    #[arg(long, default_value_t = 192.0)]
    px_per_metre: f32,
    #[arg(long, default_value_t = 7)]
    seed: u64,
}

impl Framing {
    fn size(&self) -> (usize, usize) {
        match self.size {
            Some(side) => (side, side),
            None => (self.width, self.height.unwrap_or(self.width)),
        }
    }

    fn origin(&self) -> Vec2 {
        let mut parts = self.origin.split(',').map(|p| p.trim().parse::<f32>());
        match (parts.next(), parts.next()) {
            (Some(Ok(x)), Some(Ok(y))) => Vec2::new(x, y),
            _ => Vec2::ZERO,
        }
    }

    /// Pixels per metre the picture is actually shown at.
    fn shown_px_per_metre(&self) -> f32 {
        match self.view {
            Some(metres) => self.size().1 as f32 / metres.max(0.01),
            None => self.px_per_metre,
        }
    }
}

#[derive(Args, Debug)]
struct PreviewArgs {
    #[command(flatten)]
    framing: Framing,
    #[arg(long, default_value = "target/preview.png")]
    out: PathBuf,
    /// How hard the rasteriser is allowed to work.
    #[arg(long, value_parser = parse_quality, default_value = "dataset")]
    quality: GrassRenderQuality,
}

#[derive(Args, Debug)]
struct RenderArgs {
    #[command(flatten)]
    framing: Framing,
    #[arg(long, default_value = "target/render.png")]
    out: PathBuf,
    /// Path-tracing samples per pixel.
    #[arg(long, default_value_t = 256)]
    samples: u32,
    /// Trace on the CPU rather than the GPU.
    #[arg(long)]
    cpu: bool,
    /// Apply Blender's filmic curve rather than staying linear-to-sRGB.
    #[arg(long)]
    agx: bool,
    /// Write the AOVs beside the beauty pass.
    #[arg(long)]
    passes: bool,
    /// Tiles on each axis. Zero derives it from the vertex budget.
    #[arg(long, default_value_t = 0)]
    tiles: usize,
    /// Zero derives it from the fixed trace resolution.
    #[arg(long, default_value_t = 0)]
    supersample: usize,
    /// Keep the exported scene package after a successful trace.
    #[arg(long)]
    keep_scene: bool,
    /// Where the scene package is staged.
    #[arg(long, default_value = "target/cycles-scene")]
    scene_dir: PathBuf,
    /// Resolve every derived number and print the plan without tracing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct DatasetArgs {
    #[arg(long, default_value = "target/grass-dataset")]
    out: PathBuf,
    #[arg(long, default_value_t = 8)]
    shards: usize,
    /// Side of the bake, in pixels. Larger than the crop, deliberately.
    #[arg(long, default_value_t = 448)]
    page: usize,
    /// Side of the crop actually kept.
    #[arg(long, default_value_t = 256)]
    crop: usize,
    #[arg(long, default_value_t = 0x9a55_0001)]
    seed: u64,
    #[arg(long, default_value_t = 192)]
    samples: u32,
    /// Write the structural channels beside the picture.
    #[arg(long)]
    aovs: bool,
    /// Pair against an expensive rasterisation rather than Cycles.
    #[arg(long)]
    raster: bool,
}

#[derive(Args, Debug)]
struct BenchmarkArgs {
    /// Which suite to run.
    suite: Option<String>,
}

fn parse_quality(text: &str) -> Result<GrassRenderQuality, String> {
    match text.to_ascii_lowercase().as_str() {
        "preview" => Ok(GrassRenderQuality::Preview),
        "dataset" => Ok(GrassRenderQuality::Dataset),
        "reference" => Ok(GrassRenderQuality::Reference),
        other => Err(format!(
            "unknown quality {other}; expected preview, dataset or reference"
        )),
    }
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate(args) => validate(&args),
        Command::Inspect(args) => inspect(&args),
        Command::PreviewExport(args) => preview_export(&args),
        Command::Render(args) => render(&args),
        Command::Dataset(args) => run_dataset(&args),
        Command::Benchmark(args) => not_yet(
            "benchmark",
            &args.suite.unwrap_or_else(|| "the default suite".into()),
            "the terrain fixtures and metrics arrive with `terrain_bench`",
        ),
    }
}

/// Load a document and report everything wrong with it.
///
/// Exits non-zero when the document has errors, so this is usable as a CI gate.
/// Warnings are printed and do not fail: a warning that stopped a build would be
/// silenced within a week, and then it would stop being read.
fn validate(args: &DocumentArgs) -> ExitCode {
    match terrain_format::load(&args.document) {
        Ok(loaded) => {
            let document = &loaded.document;
            // Recipe bindings, against what this binary can actually grow. The
            // loader cannot do this — it has no registry, deliberately, so a
            // document stays checkable from a CI job with no binary's recipe
            // list — so it happens here where the registry exists.
            let registry = terrain_generators::default_registry();
            let recipes = terrain_core::validate::validate_against(document, &registry.known());
            if recipes.has_errors() {
                eprintln!("{recipes}");
                return ExitCode::FAILURE;
            }
            if loaded.migration.migrated() {
                println!(
                    "migrated from format version {} to {}",
                    loaded.migration.from_version, loaded.migration.to_version
                );
                for step in &loaded.migration.steps {
                    println!("  {step}");
                }
            }
            if !loaded.report.is_empty() {
                print!("{}", loaded.report);
                println!();
            }
            println!(
                "ok: {} — {} material{}, {} channel{}, {} source{}, {} layer{}, \
                 {} population{}",
                args.document.display(),
                document.materials.len(),
                plural(document.materials.len()),
                document.modifier_channels.len(),
                plural(document.modifier_channels.len()),
                document.sources.len(),
                plural(document.sources.len()),
                document.layers.len(),
                plural(document.layers.len()),
                document.populations.len(),
                plural(document.populations.len()),
            );
            println!("  digest {}", document.digest());
            println!("  seed   {}", document.root_seed);
            println!(
                "  recipes {} registered: {}",
                registry.len(),
                registry.keys().collect::<Vec<_>>().join(", ")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Assets, resolved beside the document that names them.
///
/// Relative to the document's own directory rather than to the working
/// directory, so `terrain inspect` works from anywhere — and so a document is a
/// self-contained thing you can move.
struct BesideDocument {
    root: PathBuf,
}

impl terrain_core::AssetResolver for BesideDocument {
    fn read(&self, path: &str) -> Result<Vec<u8>, terrain_core::AssetError> {
        std::fs::read(self.root.join(path)).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => terrain_core::AssetError::NotFound,
            _ => terrain_core::AssetError::Unreadable(error.to_string()),
        })
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }
}

/// Where a document's assets live: beside it, one level up from `documents/`.
///
/// A document sits in `assets/terrain/documents/` and names
/// `features/main_path.spline.ron`, which is `assets/terrain/features/…`. So
/// the asset root is the document's parent's parent, and a document outside
/// that layout resolves against its own directory.
fn asset_root(document: &Path) -> PathBuf {
    let directory = document.parent().unwrap_or(Path::new("."));
    if directory.file_name().and_then(|n| n.to_str()) == Some("documents") {
        directory.parent().unwrap_or(directory).to_path_buf()
    } else {
        directory.to_path_buf()
    }
}

/// Compile a document and print what the terrain is at a point.
///
/// The instrument for the question "why is the ground like that here", and the
/// reason it prints the *source* values as well as the composed sample: a
/// material weight that is wrong is nearly always a mask that is wrong, and
/// seeing both at once is the difference between a guess and an answer.
fn inspect(args: &InspectArgs) -> ExitCode {
    let loaded = match terrain_format::load(&args.document) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let assets = BesideDocument {
        root: asset_root(&args.document),
    };
    let terrain = match terrain_core::prepare(
        &loaded.document,
        &assets,
        &terrain_core::SourceRegistry::new(),
        &terrain_core::PrepareOptions::default(),
    ) {
        Ok(terrain) => terrain,
        Err(report) => {
            eprintln!("{report}");
            return ExitCode::FAILURE;
        }
    };

    let position = match args.at.as_deref() {
        None => WorldPoint::ORIGIN,
        Some(text) => {
            let mut parts = text.split(',').map(|p| p.trim().parse::<f64>());
            match (parts.next(), parts.next()) {
                (Some(Ok(u)), Some(Ok(v))) => WorldPoint::new(u, v),
                _ => {
                    eprintln!("--at wants `U,V` in metres, not `{text}`");
                    return ExitCode::FAILURE;
                }
            }
        }
    };

    let footprint = match args.footprint_m {
        Some(radius) => SampleFootprint::circle(radius),
        None => SampleFootprint::Point,
    };
    let query = SampleQuery::at(position).with_footprint(footprint);

    // Source soloing, when asked for.
    if let Some(name) = &args.source {
        let key = match terrain_core::SourceKey::new(name.as_str()) {
            Ok(key) => key,
            Err(error) => {
                eprintln!("`{name}` is not a usable source key: {error}");
                return ExitCode::FAILURE;
            }
        };
        return match terrain.source_value(&key, &query) {
            Some(value) => {
                println!("{position} {name} = {value}");
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("no source named `{name}` in {}", args.document.display());
                ExitCode::FAILURE
            }
        };
    }

    let sample = terrain.sample(&query);
    println!("{position}");
    println!(
        "  elevation    {:.4} m   microrelief {:+.4} m   surface {:.4} m",
        sample.elevation_m,
        sample.microrelief.displacement_m,
        sample.surface_height_m(),
    );

    println!("  materials");
    if sample.material_weights.is_empty() {
        println!("    (none — this ground is made of nothing)");
    } else {
        for weight in sample.material_weights.iter() {
            let name = terrain
                .material_key(weight.material)
                .map(|k| k.as_str())
                .unwrap_or("?");
            println!("    {:>7.3}  {name}", weight.weight);
        }
        println!("    blend {:.3}", sample.material_weights.blend());
    }

    if !terrain.channels().is_empty() {
        println!("  modifiers");
        for (index, channel) in terrain.channels().iter().enumerate() {
            let value = sample
                .modifiers
                .get(terrain_core::ModifierIndex(index as u16))
                .unwrap_or(f32::NAN);
            println!("    {value:>7.3}  {}", channel.key);
        }
    }

    println!("  sources");
    for source in &loaded.document.sources {
        match terrain.source_value(&source.key, &query) {
            Some(value) => println!("    {value:>7.3}  {}", source.key),
            None => println!("    {:>7}  {}", "-", source.key),
        }
    }

    // The layers that actually ran, which is not the document's layer list:
    // disabled ones and ones whose references did not resolve are gone. An
    // author looking at terrain that is missing something wants this list.
    let ran: Vec<&str> = terrain.layer_keys().map(|k| k.as_str()).collect();
    println!("  layers in order: {}", ran.join(", "));
    let skipped = loaded.document.layers.len() - ran.len();
    if skipped > 0 {
        println!("    ({skipped} layer(s) disabled or unresolved)");
    }

    ExitCode::SUCCESS
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Report a command that exists in the design and not yet in the binary.
///
/// Non-zero, and that is the whole point of the function. A stub that exits zero
/// is indistinguishable from a command that worked, and the first script written
/// against it will be written against a lie.
fn not_yet(command: &str, subject: &str, reason: &str) -> ExitCode {
    eprintln!("terrain {command}: not implemented yet ({subject})");
    eprintln!("  {reason}");
    ExitCode::from(2)
}

/// Bake a plate through the cheap rasteriser.
fn preview_export(args: &PreviewArgs) -> ExitCode {
    let (width, height) = args.framing.size();
    let px_per_metre = args.framing.shown_px_per_metre();
    let params = BakeParams {
        seed: args.framing.seed,
        quality: args.quality,
        ..BakeParams::default()
    };
    let page = Page::at_detail(
        args.framing.origin(),
        width,
        height,
        px_per_metre / terrain_generators::iso::PX_PER_METRE,
    );

    println!(
        "{width}x{height} at {px_per_metre:.0} px/m ({:.1}x{:.1} m of ground), {} tier",
        width as f32 / px_per_metre,
        height as f32 / px_per_metre,
        args.quality.name(),
    );

    // Padded, so every neighbourhood-reading shading term sees the ground that
    // is actually there rather than whatever part of it fell inside the
    // rectangle. See `bake_padded`.
    let colours = terrain_bake::bake::bake_padded(page, &params);
    let bytes = terrain_bake::surface::to_rgb8(&colours);
    if let Err(error) = image::save_buffer(
        &args.out,
        &bytes,
        width as u32,
        height as u32,
        image::ColorType::Rgb8,
    ) {
        eprintln!("cannot write {}: {error}", args.out.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {}", args.out.display());
    ExitCode::SUCCESS
}

/// Trace a plate through Cycles.
fn render(args: &RenderArgs) -> ExitCode {
    let (width, height) = args.framing.size();
    let px_per_metre = args.framing.shown_px_per_metre();
    let params = plate::cycles_params(&GrassParams {
        seed: args.framing.seed,
        ..GrassParams::default()
    });

    let request = PlateRequest {
        width,
        height,
        origin: args.framing.origin(),
        px_per_metre,
        supersample: args.supersample,
        tiles: args.tiles,
        blade_width: 0.0,
        settings: RenderSettings {
            samples: args.samples,
            device: if args.cpu { "CPU" } else { "GPU" }.to_string(),
            view_transform: if args.agx { "AgX" } else { "Standard" }.to_string(),
            passes: args.passes,
            ..RenderSettings::default()
        },
        scene_dir: args.scene_dir.clone(),
        keep_scene: args.keep_scene,
    };

    let plan = PlatePlan::resolve(&request, &params);
    println!(
        "{width}x{height} shown at {px_per_metre:.0} px/m ({:.1}x{:.1} m of ground)",
        width as f32 / px_per_metre,
        height as f32 / px_per_metre,
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
    if args.dry_run {
        println!("dry run: nothing traced");
        return ExitCode::SUCCESS;
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

    let plate = match plate::trace(&request, &params, &field, &mut report) {
        Ok(plate) => plate,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "  {} blades over {} tiles, traced in {:.0} s",
        plate.blades,
        plate.plan.tiles(),
        started.elapsed().as_secs_f64()
    );

    if let Err(error) = plate.save(&args.out) {
        eprintln!("cannot write {}: {error}", args.out.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {}", args.out.display());
    ExitCode::SUCCESS
}

/// Generate a paired corpus.
fn run_dataset(args: &DatasetArgs) -> ExitCode {
    let request = CorpusRequest {
        shards: args.shards,
        page: args.page,
        crop: args.crop,
        seed: args.seed,
        samples: args.samples,
        aovs: args.aovs,
        raster: args.raster,
        out: args.out.clone(),
        ..CorpusRequest::default()
    };

    println!(
        "{} shards of {}² cropped from {}², margin {} px",
        request.shards,
        request.crop,
        request.page,
        request.margin(),
    );
    println!(
        "  input {} (raster) · target {}",
        request.input.name(),
        if request.raster {
            "raster".to_string()
        } else {
            format!("cycles {} spp", request.samples)
        },
    );

    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let mut progress = |shard: usize, images: usize| {
        println!("  shard {shard:05}: {images} images");
    };
    let report = match dataset::generate(&request, &mut progress) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("cannot write {}: {error}", request.out.display());
            return ExitCode::FAILURE;
        }
    };

    println!(
        "{} shards, {} images, {:.1} s → {}",
        report.shards,
        report.images,
        started.elapsed().as_secs_f64(),
        request.out.display()
    );
    if report.failed > 0 {
        eprintln!("{} shards produced nothing", report.failed);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Grow one page and report what it holds, without rendering anything.
///
/// Unused by the command surface today and kept compiled, because it is the
/// shape `terrain inspect` takes once there is a document to inspect: build the
/// scene, ask it questions, print numbers. Deleting it and rewriting it later
/// would lose the one thing worth keeping — that a scene can be interrogated
/// without a renderer anywhere in the call.
#[allow(dead_code)]
fn describe(page: Page, params: &BakeParams) -> String {
    let field = WorldField::lit_by(params.seed, params.light);
    let scene = GrassScene::build(page, &field, &params.grass());
    format!(
        "{} marks, canopy ceiling {:.3} m, fingerprint {}",
        scene.len(),
        scene.canopy_ceiling(),
        terrain_bench::fingerprint::fingerprint(&scene, params.seed, &field),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_parses() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_square_size_sets_both_axes() {
        let framing = Framing {
            width: 512,
            height: None,
            size: Some(768),
            origin: "0,0".into(),
            view: None,
            px_per_metre: 192.0,
            seed: 7,
        };
        assert_eq!(framing.size(), (768, 768));
    }

    #[test]
    fn a_framing_overrides_the_pixel_scale() {
        // The two ways of saying how much ground a plate covers, and the one
        // that has to win. `--view` is how a camera is set; `--px-per-metre` is
        // how a texture is. Asking for both and getting the texture answer is
        // how a render silently comes out at the wrong scale.
        let framing = Framing {
            width: 1920,
            height: Some(1080),
            size: None,
            origin: "0,0".into(),
            view: Some(27.0),
            px_per_metre: 192.0,
            seed: 7,
        };
        assert_eq!(framing.shown_px_per_metre(), 40.0);
    }

    #[test]
    fn an_origin_that_will_not_parse_falls_back_to_the_world_origin() {
        let framing = Framing {
            width: 64,
            height: None,
            size: None,
            origin: "not a point".into(),
            view: None,
            px_per_metre: 96.0,
            seed: 7,
        };
        assert_eq!(framing.origin(), Vec2::ZERO);
    }

    #[test]
    fn origins_parse_including_negative_ones() {
        let framing = |origin: &str| Framing {
            width: 64,
            height: None,
            size: None,
            origin: origin.into(),
            view: None,
            px_per_metre: 96.0,
            seed: 7,
        };
        assert_eq!(framing("-724, -543").origin(), Vec2::new(-724.0, -543.0));
        assert_eq!(framing("4800,2600").origin(), Vec2::new(4800.0, 2600.0));
    }

    #[test]
    fn every_quality_tier_has_a_name_that_parses_back() {
        for tier in [
            GrassRenderQuality::Preview,
            GrassRenderQuality::Dataset,
            GrassRenderQuality::Reference,
        ] {
            assert_eq!(parse_quality(tier.name()), Ok(tier));
        }
        assert!(parse_quality("cinematic").is_err());
    }

    #[test]
    fn describing_a_page_reports_a_meadow() {
        let params = BakeParams::default();
        let described = describe(Page::new(Vec2::ZERO, 64, 64), &params);
        assert!(described.contains("marks"));
        assert!(described.contains("fingerprint"));
    }
}

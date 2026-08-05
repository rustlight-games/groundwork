//! The terrain framework's command line.
//!
//! ```sh
//! terrain compile assets/terrain/documents/meadow_path.terrain.ron --out target/plate.png
//! terrain render --seed 5a17e33b0c9d2f14 --centre-tile=-713,284
//! ```
//!
//! ## A render is nine tiles
//!
//! `compile` and `render` frame a three-by-three isometric layout by default,
//! with the middle tile as the subject: a fresh world every run, a manifest
//! beside the picture, and the replay command printed at the end. `--manual` is
//! the hand-framed laboratory plate `render` still offers. See
//! `docs/ISOMETRIC_TILES.md`.
//!
//! This is the headless entry point, and its first job is a structural one: it
//! must be possible to grow terrain and trace it through Cycles **without
//! linking the game**. Until this existed, every one of those paths ran through
//! an example inside the grass crate, and the grass crate sat in a workspace
//! whose root package pulled in the simulation, the trainer and the renderer.
//! Nothing was wrong with the code; the dependency graph simply said "this is a
//! game with a grass module in it", and that is the sentence the whole
//! migration is written to change.
//!
//! ## Measuring the ground
//!
//! ```sh
//! terrain benchmark ground                       # every laboratory, as a table
//! terrain benchmark ground --scenario ground_band_coarse_only
//! terrain benchmark ground --out target/ground-bench --json
//! ```
//!
//! Exits non-zero when a gate *fails*. A gate that needs review does not fail
//! the run: those thresholds are still bootstrap guesses, and a suite that
//! failed on them would be silenced within a week and then stop being read.
//!
//! ## What it reaches for
//!
//! `terrain_format` to read a document, `terrain_core` to compile and sample it,
//! `terrain_generators` to grow content, and `terrain_cycles` to render it. Not
//! Bevy — nothing this binary does wants a window.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bevy::math::{Vec2, Vec3};
use clap::{Args, Parser, Subcommand};
use terrain_core::{SampleFootprint, SampleQuery, WorldPoint};
use terrain_cycles::cycles::RenderSettings;
use terrain_cycles::plate::{self, PlatePlan, PlateRequest, Progress};
use terrain_generators::field::WorldField;
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;
use terrain_generators::style::GrassParams;
use terrain_scene::frame::{
    IsoFrameOptions, RenderIdentity, RenderManifest, ResolvedRenderSample, resolve_render_sample,
};
use terrain_scene::layout::{TileLayoutPreset, WorldTileCoord};
use terrain_scene::projection::Projection;

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
    /// Render a plate through Cycles: a hand-framed laboratory plate.
    Render(RenderArgs),
    /// Run a measurement suite and report against its baseline.
    Benchmark(BenchmarkArgs),
    /// Compile a document into one scene and render the nine-tile plate.
    ///
    /// The production path: document, matrix, shared candidates, one scene, one
    /// picture, path-traced. There is no other renderer to fall back to — see
    /// root `CLAUDE.md`, "Cycles is the only renderer".
    Compile(CompileArgs),
}

#[derive(Args, Debug)]
struct CompileArgs {
    /// The terrain document to compile.
    document: PathBuf,
    /// Which world. Sixteen hex digits; omitted picks a fresh one.
    #[arg(long, value_parser = parse_seed)]
    seed: Option<u64>,
    /// Which tile sits in the middle, as `U,V`. Omitted derives one from the seed.
    #[arg(long, value_name = "U,V", allow_hyphen_values = true)]
    centre_tile: Option<String>,
    /// Output width in pixels.
    #[arg(long, default_value_t = 1920)]
    width: u32,
    /// Output height in pixels.
    #[arg(long, default_value_t = 1080)]
    height: u32,
    /// Metres along one tile edge.
    #[arg(long, default_value_t = 2.0)]
    tile_size_m: f64,
    /// Metres between field-stack samples. Omitted derives one from the framing.
    #[arg(long)]
    field_spacing_m: Option<f64>,
    /// How ragged a boundary between two substrates runs.
    #[arg(long, default_value_t = 0.30)]
    raggedness: f32,
    /// The size of the largest boundary lobes, in metres.
    #[arg(long, default_value_t = 0.18)]
    boundary_feature_m: f32,
    /// Where to write the plate.
    #[arg(long, default_value = "target/plate.png")]
    out: PathBuf,
    /// Samples per pixel, when tracing.
    #[arg(long, default_value_t = 128)]
    samples: u32,
    /// Split the plate into this many slices on each axis, for memory.
    #[arg(long, default_value_t = 1)]
    trace_tiles_across: usize,
    /// Trace on the CPU rather than the GPU.
    #[arg(long)]
    cpu: bool,
    /// Supersample factor for the trace.
    #[arg(long, default_value_t = 2)]
    supersample: usize,
    /// How high the sun sits, in degrees.
    ///
    /// Thirty-five is what the grass was tuned under. Bare ground wants less:
    /// high light hides shallow relief, and a clod two centimetres tall throws
    /// no shadow worth seeing from overhead. Fifteen or twenty is where a soil
    /// plate shows what its surface actually is.
    #[arg(long, default_value_t = 35.0)]
    sun_elevation_deg: f32,
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
///
/// Two modes, and they are kept apart on purpose. The **tile layout** mode is
/// what a render is: nine world tiles, a subject in the middle, fitted to the
/// frame. The **manual** mode is the laboratory plate this used to be — an
/// origin in cache pixels and a scale — and it survives because a diagnostic
/// sometimes needs to photograph one exact patch of ground at one exact scale.
///
/// Options from one mode are *refused* in the other rather than ignored.
/// Silent precedence is how a render comes out at a scale nobody asked for and
/// then cannot be reproduced, because the command line that produced it does
/// not say what actually happened.
#[derive(Args, Debug, Clone)]
struct Framing {
    /// Output width in pixels.
    #[arg(long, default_value_t = 1920)]
    width: usize,
    /// Output height in pixels.
    #[arg(long, default_value_t = 1080)]
    height: usize,
    /// Square output, setting both axes at once.
    #[arg(long)]
    size: Option<usize>,
    /// Which world. Sixteen hex digits; omitted picks a fresh one.
    #[arg(long, value_parser = parse_seed)]
    seed: Option<u64>,

    // --- tile layout ------------------------------------------------------
    /// The tile arrangement.
    #[arg(long, value_parser = parse_preset)]
    layout: Option<TileLayoutPreset>,
    /// Side of one world tile, in metres.
    #[arg(long, value_name = "METRES")]
    tile_size_m: Option<f64>,
    /// How much of the frame the layout fills, `0..1`.
    #[arg(long, value_name = "FRACTION")]
    layout_fill: Option<f64>,
    /// Which tile is the subject, as `U,V`. Omitted derives it from the seed.
    #[arg(long, value_name = "U,V", value_parser = parse_tile)]
    centre_tile: Option<WorldTileCoord>,

    // --- manual -----------------------------------------------------------
    /// Frame by hand rather than by layout: a laboratory plate.
    #[arg(long)]
    manual: bool,
    /// Cache-pixel corner of the plate, as `X,Y`. Manual framing only.
    #[arg(long, value_name = "X,Y")]
    origin: Option<String>,
    /// World metres visible vertically. Manual framing only.
    #[arg(long)]
    view: Option<f32>,
    /// Pixels per world metre the plate is shown at. Manual framing only.
    #[arg(long)]
    px_per_metre: Option<f32>,
}

/// The seed a manual plate uses when none is given.
///
/// Seven, which is what it has always been. Manual framing is for a diagnostic
/// that has to be the same twice, so it does **not** inherit the layout mode's
/// fresh-every-time behaviour.
const MANUAL_SEED: u64 = 7;

/// What a framing came to, whichever mode produced it.
///
/// Both renderers take this, so a Cycles plate and a raster plate framed from
/// the same command line are the same window on the same world.
struct ResolvedFraming {
    width: usize,
    height: usize,
    px_per_metre: f32,
    /// The plate's top-left corner, in cache pixels at `px_per_metre`.
    origin: Vec2,
    seed: u64,
    /// The layout this was framed from, or `None` for a manual plate.
    sample: Option<ResolvedRenderSample>,
    /// The preset and fill the layout was asked for, for the manifest.
    preset: TileLayoutPreset,
    fill: f64,
}

impl ResolvedFraming {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// The ground this render is *of*, for a renderer that draws a silhouette.
    ///
    /// `None` for a manual plate: the whole rectangle is the picture, which is
    /// what a laboratory plate has always meant.
    fn visible_ground(&self) -> Option<(Vec2, Vec2)> {
        let bounds = self.sample.as_ref()?.frame.visible_bounds();
        Some((
            Vec2::new(bounds.min.u_m as f32, bounds.min.v_m as f32),
            Vec2::new(bounds.max.u_m as f32, bounds.max.v_m as f32),
        ))
    }
}

impl Framing {
    fn size(&self) -> (usize, usize) {
        match self.size {
            Some(side) => (side, side),
            None => (self.width, self.height),
        }
    }

    /// Which mode this is, and whether the options agree with it.
    fn resolve(&self) -> Result<ResolvedFraming, String> {
        let manual_named =
            self.origin.is_some() || self.view.is_some() || self.px_per_metre.is_some();
        let layout_named = self.layout.is_some()
            || self.tile_size_m.is_some()
            || self.layout_fill.is_some()
            || self.centre_tile.is_some();

        if self.manual || (manual_named && !layout_named) {
            if layout_named {
                return Err("manual framing takes --origin, --view and --px-per-metre; \
                            --layout, --tile-size-m, --layout-fill and --centre-tile \
                            belong to the tile layout"
                    .into());
            }
            return Ok(self.manual_framing());
        }
        if manual_named {
            return Err(
                "tile-layout framing derives the origin and the scale from the \
                        layout, so --origin, --view and --px-per-metre are refused. \
                        Pass --manual for a hand-framed laboratory plate"
                    .into(),
            );
        }
        self.layout_framing()
    }

    fn manual_framing(&self) -> ResolvedFraming {
        let (width, height) = self.size();
        let px_per_metre = match self.view {
            Some(metres) => height as f32 / metres.max(0.01),
            None => self.px_per_metre.unwrap_or(192.0),
        };
        ResolvedFraming {
            width,
            height,
            px_per_metre,
            origin: parse_origin(self.origin.as_deref().unwrap_or("0,0")),
            seed: self.seed.unwrap_or(MANUAL_SEED),
            sample: None,
            preset: TileLayoutPreset::default(),
            fill: 1.0,
        }
    }

    fn layout_framing(&self) -> Result<ResolvedFraming, String> {
        let (width, height) = self.size();
        let preset = self.layout.unwrap_or_default();
        let fill = self.layout_fill.unwrap_or(0.90);
        let seed = self.seed.unwrap_or_else(fresh_seed);
        let identity = RenderIdentity::resolve(seed, self.centre_tile);
        let sample = resolve_render_sample(
            preset,
            self.tile_size_m.unwrap_or(DEFAULT_TILE_SIDE_M),
            identity,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions {
                output_size: [width as u32, height as u32],
                fill,
                subject_position: [0.5, 0.5],
                halo_m: terrain_scene::frame::DEFAULT_HALO_M,
            },
        )
        .map_err(|error| error.to_string())?;

        Ok(ResolvedFraming {
            width,
            height,
            px_per_metre: sample.frame.pixels_per_metre,
            origin: Vec2::new(sample.frame.cache_origin[0], sample.frame.cache_origin[1]),
            seed,
            sample: Some(sample),
            preset,
            fill,
        })
    }
}

/// The tile side a layout uses when none is given, in metres.
///
/// Two, and the number is chosen rather than round. Three things agree on it:
///
/// **Precedent.** Diablo II's floor tile is 160×80 pixels at 2:1 dimetric, cut
/// into 5×5 collision subtiles. Work back from a character at roughly 80 pixels
/// for 1.8 metres and a tile is a shade under two metres, with 0.4-metre pathing
/// cells. Dota 2 is coarser — Source units are inches, its terrain grid is 128
/// of them, so 3.25 metres — but its heroes are heroically oversized and its
/// camera sits much further out.
///
/// **Gameplay.** A tower occupies a tile, a keep occupies two by two, and a hero
/// with a 0.6-metre collision radius moves continuously across tiles rather than
/// snapping between them.
///
/// **The renderer**, and this is the one that settles it. The subject diamond is
/// 576×288 pixels at the default framing *whatever* the tile side, because the
/// layout always fills the same nine-ninths of the frame. What the tile side
/// actually changes is how many metres those pixels cover: two metres resolves
/// to 144 pixels per metre, four to 72.
///
/// That matters because a grass blade is about three millimetres across, so it
/// is one pixel wide at roughly 330 pixels to the metre, and below that it is a
/// *partially covered* pixel that averages to a flat wash. The path tracer
/// supersamples up to three times — see `terrain_cycles::plate` — so 144 px/m
/// traces at 432 and resolves a blade, while 72 px/m clamps at 216 and does not.
/// Two metres is the largest tile at which the expensive renderer can actually
/// see the grass it is rendering.
const DEFAULT_TILE_SIDE_M: f64 = 2.0;

/// A fresh seed, from the operating system.
///
/// `RandomState` is seeded from the system's own entropy and advances per call,
/// which is exactly what is wanted and costs no dependency. Deliberately *not*
/// the wall clock: two renders started in the same second would be the same
/// meadow, and a corpus generated by a loop would be one place repeated.
///
/// It is also deliberately here, in the command line, rather than in terrain
/// generation. Nothing below this binary may consult the world for a number —
/// everything down there is a pure function of a seed, and it stays that way.
fn fresh_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0x9e37_79b9_7f4a_7c15);
    terrain_core::seed::mix(hasher.finish())
}

fn parse_seed(text: &str) -> Result<u64, String> {
    text.parse::<terrain_core::seed::RootSeed>()
        .map(|seed| seed.bits())
        .map_err(|_| format!("a seed is up to sixteen hex digits, not `{text}`"))
}

fn parse_preset(text: &str) -> Result<TileLayoutPreset, String> {
    text.parse()
}

fn parse_tile(text: &str) -> Result<WorldTileCoord, String> {
    text.parse()
}

/// A cache-pixel origin, falling back to the world origin.
fn parse_origin(text: &str) -> Vec2 {
    let mut parts = text.split(',').map(|p| p.trim().parse::<f32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(x)), Some(Ok(y))) => Vec2::new(x, y),
        _ => Vec2::ZERO,
    }
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
    /// How many slices Blender traces the plate in, on each axis.
    ///
    /// **Not** the world-tile layout. This is a memory split: one plate, traced
    /// in pieces so the scene fits, with a guard band and a crop. Zero derives
    /// it from the vertex budget. See `terrain_cycles::plate`.
    #[arg(long, default_value_t = 0)]
    trace_tiles_across: usize,
    /// Zero derives it from the fixed trace resolution.
    #[arg(long, default_value_t = 0)]
    supersample: usize,
    /// Write only the picture: no tile grid, no subject mask, no manifest.
    #[arg(long)]
    no_sidecars: bool,
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
struct BenchmarkArgs {
    /// Which suite to run. `ground` is the soil benchmark.
    suite: Option<String>,
    /// Run one scenario rather than all of them.
    #[arg(long)]
    scenario: Option<String>,
    /// Write each report's JSON under this directory.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Print the JSON rather than the table.
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Compile(args) => compile(&args),
        Command::Validate(args) => validate(&args),
        Command::Inspect(args) => inspect(&args),
        Command::Render(args) => render(&args),
        Command::Benchmark(args) => benchmark(&args),
    }
}

/// Run a measurement suite.
///
/// Exits non-zero when a gate fails, so this is usable as a CI gate. A gate that
/// needs review does *not* fail the run: those are the thresholds that are still
/// bootstrap guesses, and a suite that fails on them would be silenced within a
/// week and then stop being read.
fn benchmark(args: &BenchmarkArgs) -> ExitCode {
    use terrain_bench::ground;

    let suite = args.suite.as_deref().unwrap_or("ground");
    if suite != "ground" {
        eprintln!("`{suite}` is not a suite this build knows. Try `ground`.");
        return ExitCode::FAILURE;
    }

    let scenarios: Vec<&ground::GroundScenario> = match args.scenario.as_deref() {
        None => ground::GROUND_SCENARIOS.iter().collect(),
        Some(name) => match ground::scenarios::scenario(name) {
            Some(scenario) => vec![scenario],
            None => {
                eprintln!(
                    "`{name}` is not a scenario. Known: {}",
                    ground::GROUND_SCENARIOS
                        .iter()
                        .map(|s| s.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return ExitCode::FAILURE;
            }
        },
    };

    if let Some(directory) = &args.out
        && let Err(error) = std::fs::create_dir_all(directory)
    {
        eprintln!("cannot create {}: {error}", directory.display());
        return ExitCode::FAILURE;
    }

    let mut failed = 0usize;
    let mut review = 0usize;
    for scenario in scenarios {
        let report = ground::run(scenario, ground::DEFAULT_SEED);
        match report.verdict.status {
            ground::GateStatus::Fail => failed += 1,
            ground::GateStatus::NeedsReview => review += 1,
            _ => {}
        }
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report.to_json()).unwrap_or_default()
            );
        } else {
            print!("{}", report.to_table());
            println!();
        }
        if let Some(directory) = &args.out {
            let path = directory.join(format!("{}.json", scenario.name));
            let text = serde_json::to_string_pretty(&report.to_json()).unwrap_or_default();
            if let Err(error) = std::fs::write(&path, text) {
                eprintln!("cannot write {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    if failed > 0 {
        eprintln!("{failed} scenario(s) failed a gate");
        return ExitCode::FAILURE;
    }
    if review > 0 {
        println!("{review} scenario(s) need review; none failed");
    }
    ExitCode::SUCCESS
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
            // Both registries. The shared-candidate families and the older
            // population recipes are two legitimate sets during the migration,
            // and a document naming either is a document this binary can grow.
            let legacy = terrain_generators::default_registry();
            let families = terrain_generators::family_registry();
            let mut names: Vec<String> = legacy
                .keys()
                .chain(families.keys())
                .map(|key| key.to_string())
                .collect();
            names.sort();
            names.dedup();
            let mut known = legacy.known();
            for key in families.keys() {
                known = known.with_population(
                    terrain_core::RecipeKey::new(key).expect("registered keys are valid"),
                );
            }
            let recipes = terrain_core::validate::validate_against(document, &known);
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
            println!("  recipes {} registered: {}", names.len(), names.join(", "));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Read every ground profile a document's materials name.
///
/// Loaded here rather than inside `prepare`, because parsing them means RON and
/// `terrain_core` depends on nothing but `serde` — the same reason documents
/// themselves arrive already parsed.
fn load_profiles(
    document: &terrain_core::document::TerrainDocument,
    assets: &dyn terrain_core::AssetResolver,
) -> Result<terrain_core::GroundProfileLibrary, String> {
    let named = document
        .materials
        .iter()
        .filter_map(|material| material.profile.clone());
    let (library, problems) = terrain_format::load_library(named, assets);
    if problems.is_empty() {
        return Ok(library);
    }
    // Every problem, in one pass. An author with three broken profiles should be
    // told about three.
    Err(problems
        .iter()
        .map(|problem| problem.to_string())
        .collect::<Vec<_>>()
        .join("\n"))
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
    let profiles = match load_profiles(&loaded.document, &assets) {
        Ok(profiles) => profiles,
        Err(problems) => {
            eprintln!("{problems}");
            return ExitCode::FAILURE;
        }
    };
    let terrain = match terrain_core::prepare(
        &loaded.document,
        &assets,
        &terrain_core::SourceRegistry::new(),
        &terrain_core::PrepareOptions {
            profiles,
            ..terrain_core::PrepareOptions::default()
        },
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

/// Say what a framing came to, in the terms a reader can check.
fn report_framing(framing: &ResolvedFraming) {
    let (width, height) = framing.size();
    match &framing.sample {
        Some(sample) => {
            let layout = sample.layout();
            println!(
                "{width}x{height} — {} tiles of {} m, subject {}, seed {}",
                layout.len(),
                layout.tile_side_m(),
                sample.identity.centre_tile,
                sample.identity.seed_hex(),
            );
            let subject = sample.frame.subject_polygon().corners_px;
            let span = |axis: usize| {
                let values = subject.map(|corner| corner[axis]);
                values.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                    - values.iter().cloned().fold(f32::INFINITY, f32::min)
            };
            println!(
                "  {:.0} px/m, subject diamond {:.0}x{:.0} px, layout fills {:.0}%",
                framing.px_per_metre,
                span(0),
                span(1),
                framing.fill * 100.0,
            );
        }
        None => println!(
            "{width}x{height} at {:.0} px/m ({:.1}x{:.1} m of ground), manual framing",
            framing.px_per_metre,
            width as f32 / framing.px_per_metre,
            height as f32 / framing.px_per_metre,
        ),
    }
}

/// Where a sidecar lands, given the picture's own path.
fn beside(out: &Path, suffix: &str, extension: &str) -> PathBuf {
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "render".into());
    out.with_file_name(format!("{stem}{suffix}.{extension}"))
}

/// Write the tile grid, the subject mask and the manifest beside a picture.
///
/// The beauty render has no visible tile boundaries — that is the point — so
/// nothing in it says whether the framing came out right. These three say.
fn write_sidecars(
    out: &Path,
    colours: &[Vec3],
    sample: &ResolvedRenderSample,
    manifest: &RenderManifest,
) -> std::io::Result<Vec<PathBuf>> {
    let frame = &sample.frame;
    let (width, height) = (frame.output_size[0] as usize, frame.output_size[1] as usize);
    let style = terrain_scene::overlay::GridStyle::default();
    let mut written = Vec::new();

    let mut annotated = colours.to_vec();
    let mut canvas = terrain_scene::overlay::Canvas::new(&mut annotated, width, height);
    terrain_scene::overlay::draw_tile_grid(&mut canvas, frame, &style);
    terrain_scene::overlay::draw_caption(
        &mut canvas,
        &[
            format!("SEED {}", sample.identity.seed_hex().to_uppercase()),
            format!("SUBJECT {}", sample.identity.centre_tile),
            format!(
                "{:.1} PX/M  TILE {} M",
                frame.pixels_per_metre,
                frame.layout.tile_side_m()
            ),
        ],
        &style,
    );
    let grid = beside(out, "-tiles", "png");
    save_rgb(&grid, &terrain_scene::to_rgb8(&annotated), width, height)?;
    written.push(grid);

    let mask = beside(out, "-subject-mask", "png");
    let bytes = terrain_scene::overlay::mask_to_gray8(&terrain_scene::overlay::subject_mask(frame));
    image::save_buffer(
        &mask,
        &bytes,
        width as u32,
        height as u32,
        image::ColorType::L8,
    )
    .map_err(std::io::Error::other)?;
    written.push(mask);

    let record = beside(out, "", "ron");
    std::fs::write(&record, manifest.to_ron())?;
    written.push(record);
    Ok(written)
}

fn save_rgb(path: &Path, bytes: &[u8], width: usize, height: usize) -> std::io::Result<()> {
    image::save_buffer(
        path,
        bytes,
        width as u32,
        height as u32,
        image::ColorType::Rgb8,
    )
    .map_err(std::io::Error::other)
}

/// Trace a plate through Cycles.
fn render(args: &RenderArgs) -> ExitCode {
    let framing = match args.framing.resolve() {
        Ok(framing) => framing,
        Err(problem) => {
            eprintln!("terrain render: {problem}");
            return ExitCode::FAILURE;
        }
    };
    let (width, height) = framing.size();
    let px_per_metre = framing.px_per_metre;
    let params = plate::cycles_params(&GrassParams {
        seed: framing.seed,
        ..GrassParams::default()
    });

    let request = PlateRequest {
        width,
        height,
        origin: framing.origin,
        px_per_metre,
        supersample: args.supersample,
        tiles: args.trace_tiles_across,
        blade_width: 0.0,
        visible: framing.visible_ground(),
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
    report_framing(&framing);
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
            "  traced in {0}x{0} slices of {1}x{2}, {3} px guard",
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
        "  {} blades over {} slices, traced in {:.0} s",
        plate.blades,
        plate.plan.tiles(),
        started.elapsed().as_secs_f64()
    );

    if let Err(error) = plate.save(&args.out) {
        eprintln!("cannot write {}: {error}", args.out.display());
        return ExitCode::FAILURE;
    }
    println!(
        "wrote {} ({:.0}% covered)",
        args.out.display(),
        plate.coverage() * 100.0
    );

    if let (Some(sample), false) = (&framing.sample, args.no_sidecars) {
        // Unpacked, so the overlay is drawn in the same space it composites
        // onto — see `terrain_scene::pixel`.
        let colours = terrain_scene::from_bytes_rgb(&plate.rgb());
        let mut manifest = sample.manifest("render", framing.preset, framing.fill);
        manifest.samples = Some(args.samples);
        manifest.marks = Some(plate.blades);
        match write_sidecars(&args.out, &colours, sample, &manifest) {
            Ok(paths) => {
                for path in paths {
                    println!("wrote {}", path.display());
                }
            }
            Err(error) => {
                eprintln!(
                    "cannot write a sidecar beside {}: {error}",
                    args.out.display()
                );
                return ExitCode::FAILURE;
            }
        }
        println!("\nreplay:\n  {}", manifest.replay);
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
fn describe(page: Page, params: &GrassParams) -> String {
    let field = WorldField::lit_by(params.seed, params.light);
    let scene = GrassScene::build(page, &field, params);
    format!(
        "{} marks, canopy ceiling {:.3} m, fingerprint {}",
        scene.len(),
        scene.canopy_ceiling(),
        terrain_bench::fingerprint::fingerprint(&scene, params.seed, &field),
    )
}

/// Compile a document into one scene and render the nine-tile plate.
///
/// The whole production path in one command, and the order is the point: the
/// document decides what the terrain *is*, the compiler decides what grows, and
/// the renderer only draws what it was handed. Nothing here scatters anything.
fn compile(args: &CompileArgs) -> ExitCode {
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
    let profiles = match load_profiles(&loaded.document, &assets) {
        Ok(profiles) => profiles,
        Err(problems) => {
            eprintln!("{problems}");
            return ExitCode::FAILURE;
        }
    };
    let terrain = match terrain_core::prepare(
        &loaded.document,
        &assets,
        &terrain_core::SourceRegistry::new(),
        &terrain_core::PrepareOptions {
            profiles,
            ..terrain_core::PrepareOptions::default()
        },
    ) {
        Ok(terrain) => terrain,
        Err(report) => {
            eprintln!("{report}");
            return ExitCode::FAILURE;
        }
    };

    // The world, and which tile is the middle of it. Derived from the seed
    // through named streams rather than drawn beside it, so one number
    // reproduces the whole frame.
    let seed = args.seed.unwrap_or_else(fresh_seed);
    let centre = match args.centre_tile.as_deref().map(parse_tile) {
        Some(Ok(tile)) => Some(tile),
        Some(Err(problem)) => {
            eprintln!("{problem}");
            return ExitCode::FAILURE;
        }
        None => None,
    };
    let identity = terrain_scene::RenderIdentity::resolve(seed, centre);

    let layout = match terrain_scene::IsoTileLayout::nine(identity.centre_tile, args.tile_size_m) {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let frame = terrain_scene::ResolvedIsoFrame::resolve(
        layout,
        terrain_scene::Projection::default(),
        terrain_scene::IsoFrameOptions {
            output_size: [args.width, args.height],
            ..terrain_scene::IsoFrameOptions::default()
        },
    );

    let transition = terrain_generators::TransitionProfile {
        raggedness: args.raggedness,
        feature_m: args.boundary_feature_m,
        octaves: 3,
    };
    let options = terrain_generators::SceneCompileOptions {
        field_spacing_m: args.field_spacing_m,
        derive: terrain_scene::derive::DerivedFieldRequest::ALL,
        transition,
        validate: true,
        ..terrain_generators::SceneCompileOptions::default()
    };

    // The halo is derived by the compiler from every reach that exists, so the
    // request asks for none and lets it decide.
    let request = frame.scene_request(0.0);
    let registry = terrain_generators::family_registry();

    // Progress reporting, which is the sanctioned use — see `clippy.toml`.
    // Nothing here reaches the generator or a digest.
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let compiled = match terrain_generators::compile_scene(&terrain, &request, &registry, &options)
    {
        Ok(compiled) => compiled,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let compile_time = started.elapsed();

    // One evaluator, shared by everything that asks about the ground: the mesh
    // that carries its relief, the shader that colours it, the overlay that
    // decides how much grass grows on it, and every secondary root registered
    // to it. See `terrain_generators::ground`.
    //
    // Taken from the compilation rather than rebuilt here, and that is the
    // whole of this line's significance. The CLI used to construct a second
    // evaluator from the same inputs — which agreed, until the day one of the
    // two construction sites was edited and the other was not. The lattice it
    // splits its relief bands at is still chosen by the soils in play, but the
    // choosing now happens once, in the compiler, before any content is placed
    // against it.
    let evaluator = std::sync::Arc::clone(&compiled.ground);

    // The relief ladder, resolved and printed. Every band gets exactly one
    // owner and the plan says which and why — so "the soil got smoother" can be
    // answered with "that band moved to the microfacet tier" rather than with a
    // guess.
    let traced_pixel_m = 1.0 / (frame.pixels_per_metre * args.supersample as f32).max(1.0);
    let plan = terrain_generators::relief::GroundReliefPlan::resolve(
        evaluator.profiles().iter().map(|p| p.as_ref()),
        evaluator.band_split().spacing_m,
        traced_pixel_m,
        frame.layout.visible_bounds().width_m() as f32,
    );
    print!(
        "  {}",
        plan.to_table().replace('\n', "\n  ").trim_end_matches("  ")
    );

    // The tuned generator, driven by the document.
    //
    // Deliberately *not* a fresh renderer over the generic scene. What the
    // document controls is `SemanticOverlay`: how much grows, and where the
    // earth shows. Every style field stays exactly as tuned.
    let overlay = std::sync::Arc::new(terrain_generators::SemanticOverlay {
        ground: std::sync::Arc::clone(&evaluator),
    });

    // Cycles, and only Cycles. See CLAUDE.md: this framework builds geometry and
    // a path tracer renders it. There is no second renderer to fall back to and
    // no cheap tier to compare against — the low-fidelity representation of this
    // terrain is the field stack, not a smaller picture of it.
    let params = plate::cycles_params(&GrassParams {
        seed,
        ..GrassParams::default()
    });
    let field =
        terrain_generators::WorldField::lit_by(params.seed, params.light).with_overlay(overlay);

    let visible = frame.layout.visible_bounds();
    let request = PlateRequest {
        width: args.width as usize,
        height: args.height as usize,
        origin: Vec2::new(frame.cache_origin[0], frame.cache_origin[1]),
        px_per_metre: frame.pixels_per_metre,
        supersample: args.supersample,
        tiles: args.trace_tiles_across,
        blade_width: 0.0,
        visible: Some((
            Vec2::new(visible.min.u_m as f32, visible.min.v_m as f32),
            Vec2::new(visible.max.u_m as f32, visible.max.v_m as f32),
        )),
        settings: RenderSettings {
            samples: args.samples,
            device: if args.cpu { "CPU" } else { "GPU" }.to_string(),
            view_transform: "AgX".to_string(),
            sun_elevation: args.sun_elevation_deg.to_radians(),
            ..RenderSettings::default()
        },
        scene_dir: std::env::temp_dir().join("terrain-compile-scene"),
        keep_scene: std::env::var_os("TERRAIN_KEEP_SCENE").is_some(),
    };

    let plan = PlatePlan::resolve(&request, &params);
    println!(
        "  tracing at {:.0} px/m ({}x), {} ribs, ~{:.1}M blades",
        plan.trace_px_per_metre,
        plan.supersample,
        plan.ribs,
        plan.estimated_blades / 1.0e6,
    );

    // Progress reporting, which is the sanctioned use — see `clippy.toml`.
    // Nothing here reaches the generator or a digest.
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let mut progress = |p: Progress| {
        if p.tiles > 1 {
            println!("  slice {}/{}", p.tile, p.tiles);
        }
    };
    let plate = match plate::trace(&request, &params, &field, &mut progress) {
        Ok(plate) => plate,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let render_time = started.elapsed();

    if let Some(parent) = args.out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = image::save_buffer(
        &args.out,
        &plate.pixels,
        plate.width as u32,
        plate.height as u32,
        image::ColorType::Rgba8,
    ) {
        eprintln!("cannot write {}: {error}", args.out.display());
        return ExitCode::FAILURE;
    }

    let report = &compiled.report;
    println!("{}", args.out.display());
    println!(
        "  document {} — seed {} — centre tile {},{}",
        loaded.document.metadata.name,
        identity.seed_hex(),
        identity.centre_tile.u,
        identity.centre_tile.v
    );
    println!(
        "  matrix   {} samples at {:.3} m, halo {:.2} m",
        report.field_samples, report.field_spacing_m, report.halo_m
    );
    println!(
        "  candidates {} generated, {} accepted, {} unowned",
        report.candidates_generated, report.candidates_accepted, report.candidates_unowned
    );
    println!(
        "  marks    {} from {} populations",
        report.marks_emitted,
        report.marks_by_population.len()
    );
    // Every population and who draws it, not only the ones that emitted. A
    // population showing nothing is the question an author actually has, and
    // "tuned(fine)" answers it where a missing row does not.
    for (population, class) in &report.render_classes {
        let marks = report
            .marks_by_population
            .get(population)
            .copied()
            .unwrap_or(0);
        println!("             {population}: {class}, {marks} marks");
    }
    if !report.deferred_populations.is_empty() {
        println!(
            "  deferred {} — declared and understood, not drawn yet",
            report.deferred_populations.join(", ")
        );
    }
    println!(
        "  scene    {} — {:.1} marks/m2",
        compiled.scene.fingerprint().short(),
        compiled.scene.mark_density()
    );
    println!(
        "  traced   {} blades in {:.0}s (compile {:.2}s)",
        plate.blades,
        render_time.as_secs_f64(),
        compile_time.as_secs_f64()
    );
    println!("\nreplay:");
    println!(
        "  terrain compile {} --seed {} --centre-tile={},{}",
        args.document.display(),
        identity.seed_hex(),
        identity.centre_tile.u,
        identity.centre_tile.v
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_parses() {
        Cli::command().debug_assert();
    }

    /// The default command line, which is a nine-tile layout at 1920×1080.
    fn framing() -> Framing {
        Framing {
            width: 1920,
            height: 1080,
            size: None,
            seed: None,
            layout: None,
            tile_size_m: None,
            layout_fill: None,
            centre_tile: None,
            manual: false,
            origin: None,
            view: None,
            px_per_metre: None,
        }
    }

    #[test]
    fn a_square_size_sets_both_axes() {
        let framing = Framing {
            size: Some(768),
            ..framing()
        };
        assert_eq!(framing.size(), (768, 768));
    }

    #[test]
    fn the_default_framing_is_the_nine_tile_layout() {
        // The change this whole migration is about: an ordinary invocation with
        // nothing on the command line is nine world tiles, not an arbitrary
        // rectangle at the world origin.
        let resolved = framing().resolve().expect("a default framing resolves");
        let sample = resolved.sample.expect("the default is layout-framed");
        assert_eq!(sample.layout().len(), 9);
        assert_eq!(sample.layout().tile_side_m(), DEFAULT_TILE_SIDE_M);
        // Two-metre tiles in a 1920×1080 frame at ninety percent: six metres of
        // ground across, twelve of projected screen, 144 pixels to the metre.
        assert!(
            (resolved.px_per_metre - 144.0).abs() < 1.0e-3,
            "{}",
            resolved.px_per_metre
        );
        // And the subject diamond is 576×288 pixels — which it would be at any
        // tile side, because the layout always fills the same fraction of the
        // frame. The tile side decides how many *metres* those pixels cover, and
        // that is the whole of what it decides.
        let subject = sample.frame.subject_polygon().corners_px;
        let span = |axis: usize| {
            subject
                .iter()
                .map(|c| c[axis])
                .fold(f32::NEG_INFINITY, f32::max)
                - subject
                    .iter()
                    .map(|c| c[axis])
                    .fold(f32::INFINITY, f32::min)
        };
        assert!((span(0) - 576.0).abs() < 0.01, "{}", span(0));
        assert!((span(1) - 288.0).abs() < 0.01, "{}", span(1));
    }

    #[test]
    fn the_default_tile_is_the_largest_the_path_tracer_can_resolve_a_blade_at() {
        // A blade is one pixel wide at about 330 px/m and a partially covered
        // pixel below that, which averages to a flat wash however many samples
        // it gets. The tracer may supersample by three, so the framing has to
        // arrive at 110 px/m or better — and the tile side is what decides that.
        // At four-metre tiles this framing traces at 216 px/m and the canopy
        // washes out; at two it traces at 432 and a blade is a blade.
        let resolved = framing().resolve().expect("resolves");
        let traced = resolved.px_per_metre * terrain_cycles::plate::MAX_SUPERSAMPLE as f32;
        assert!(
            traced >= terrain_cycles::plate::TRACE_PX_PER_METRE,
            "{DEFAULT_TILE_SIDE_M} m tiles trace at {traced:.0} px/m, under the {} a blade needs",
            terrain_cycles::plate::TRACE_PX_PER_METRE
        );
    }

    #[test]
    fn an_ordinary_invocation_is_somewhere_new_every_time() {
        // Random means every run produces something different. It must not mean
        // the result is unrecoverable, which is what the manifest is for.
        let first = framing().resolve().expect("resolves");
        let second = framing().resolve().expect("resolves");
        assert_ne!(first.seed, second.seed, "two runs drew the same seed");
    }

    #[test]
    fn a_named_seed_reproduces_the_frame_exactly() {
        let framed = || {
            Framing {
                seed: Some(0x5a17_e33b_0c9d_2f14),
                ..framing()
            }
            .resolve()
            .expect("resolves")
        };
        let (a, b) = (framed(), framed());
        assert_eq!(a.origin, b.origin);
        assert_eq!(a.px_per_metre, b.px_per_metre);
        assert_eq!(
            a.sample.unwrap().identity.centre_tile,
            b.sample.unwrap().identity.centre_tile
        );
    }

    #[test]
    fn a_named_centre_tile_frames_that_tile() {
        let wanted = WorldTileCoord::new(-713, 284);
        let resolved = Framing {
            seed: Some(1),
            centre_tile: Some(wanted),
            ..framing()
        }
        .resolve()
        .expect("resolves");
        let sample = resolved.sample.expect("layout-framed");
        assert_eq!(sample.layout().subject(), wanted);
        assert_eq!(sample.identity.centre_tile, wanted);
    }

    #[test]
    fn the_two_framing_modes_refuse_each_others_options() {
        // Silent precedence is how a render comes out at a scale nobody asked
        // for, and then cannot be reproduced because the command line that
        // produced it does not say what happened.
        let layout_with_manual = Framing {
            layout: Some(TileLayoutPreset::Nine),
            px_per_metre: Some(96.0),
            ..framing()
        };
        assert!(layout_with_manual.resolve().is_err());

        let manual_with_layout = Framing {
            manual: true,
            tile_size_m: Some(4.0),
            ..framing()
        };
        assert!(manual_with_layout.resolve().is_err());

        // And the plain forms of each are fine.
        assert!(
            Framing {
                manual: true,
                px_per_metre: Some(96.0),
                ..framing()
            }
            .resolve()
            .is_ok()
        );
        assert!(
            Framing {
                layout: Some(TileLayoutPreset::Nine),
                tile_size_m: Some(6.0),
                ..framing()
            }
            .resolve()
            .is_ok()
        );
    }

    #[test]
    fn a_manual_plate_is_the_laboratory_plate_it_always_was() {
        // Including the fixed seed: a diagnostic that changed every run would be
        // no use as a diagnostic.
        let resolved = Framing {
            manual: true,
            size: Some(1080),
            view: Some(27.0),
            origin: Some("-724, -543".into()),
            ..framing()
        }
        .resolve()
        .expect("resolves");
        assert!(resolved.sample.is_none());
        assert_eq!(resolved.seed, MANUAL_SEED);
        assert_eq!(resolved.px_per_metre, 40.0);
        assert_eq!(resolved.origin, Vec2::new(-724.0, -543.0));
    }

    #[test]
    fn an_origin_that_will_not_parse_falls_back_to_the_world_origin() {
        assert_eq!(parse_origin("not a point"), Vec2::ZERO);
        assert_eq!(parse_origin("-724, -543"), Vec2::new(-724.0, -543.0));
        assert_eq!(parse_origin("4800,2600"), Vec2::new(4800.0, 2600.0));
    }

    #[test]
    fn a_seed_is_read_as_hex_the_way_it_is_printed() {
        // The round trip a replay command depends on. A seed printed as hex and
        // read back as decimal would send every replay somewhere else.
        assert_eq!(parse_seed("5a17e33b0c9d2f14"), Ok(0x5a17_e33b_0c9d_2f14));
        assert_eq!(parse_seed("0x7"), Ok(7));
        assert_eq!(
            parse_seed(&RenderIdentity::from_seed(11).seed_hex()),
            Ok(11)
        );
        assert!(parse_seed("not a seed").is_err());
    }

    #[test]
    fn sidecars_land_beside_the_picture_they_describe() {
        let out = Path::new("target/corpus/plate.png");
        assert_eq!(
            beside(out, "-tiles", "png"),
            Path::new("target/corpus/plate-tiles.png")
        );
        assert_eq!(beside(out, "", "ron"), Path::new("target/corpus/plate.ron"));
    }

    #[test]
    fn describing_a_page_reports_a_meadow() {
        let params = GrassParams::default();
        let described = describe(Page::new(Vec2::ZERO, 64, 64), &params);
        assert!(described.contains("marks"));
        assert!(described.contains("fingerprint"));
    }
}

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
    // ## Tiling removes the need to thin at all
    //
    // The thinning above exists only because a single-pass wide render will not
    // fit in memory. Tiles bound memory directly and far better, so when the
    // render is tiled the canopy keeps its full density — which matters, because
    // thinning was never free. Holding *coverage* is not holding *structure*:
    // fewer, fatter blades cover the same ground and stop forming legible tufts,
    // and measured that took coherence from 0.46 to 0.22 and the highlight share
    // to a fifth of the reference's.
    //
    // So a wide view is bought with time rather than with the picture.
    let detail_ratio = (shown_px_per_metre / TUNED_PX_PER_METRE).clamp(0.0, 1.0);
    let crowding = if options.tiles > 1 {
        1.0
    } else {
        detail_ratio.max(MIN_CROWDING)
    };
    // ## The width compensation is 1/c, not 1/√c
    //
    // Coverage is `count × width × length`, so thinning the population by `c`
    // and widening by `1/c` holds it. The square root is the instinctive choice
    // — it is what you use for *spacing* — and it is wrong here by exactly the
    // amount that matters: at a fifth the count it widens by 2.1 where 4.5 is
    // needed, so the canopy loses more than half its coverage and the wide view
    // opens into patchy scrub over dirt however dense the close-up was.
    //
    // Capped, because a blade widened past a few times life size stops being a
    // blade. Past that cap the coverage genuinely cannot be held by width alone
    // and `MIN_CROWDING` is what has to give.
    let width_relief = (1.0 / crowding).min(4.5);
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
    let across = options.tiles.max(1);

    // ## Why a wide view has to be tiled
    //
    // At the close-up density this look was tuned to, the game's own framing
    // holds fifty-five million blades. There is no rib count that fits that in
    // memory, so a single-pass wide render can only be bought by thinning — and
    // thinning buys it at the price of the picture. Holding *coverage* is not
    // holding *structure*: fewer, fatter blades cover the same ground and no
    // longer form legible tufts, so the colony signal collapses and nothing
    // catches a highlight. Measured, coherence falls to 0.225 against 0.456.
    //
    // Tiles keep full density and pay in time instead. They are seamless for
    // free, because placement is a pure function of world position — the one
    // property this whole design has been protecting. Each tile is grown with a
    // guard band so blades just outside it still shadow and occlude inward, then
    // the guard is cropped away.
    let tile_width = options.width.div_ceil(across);
    let tile_height = options.height.div_ceil(across);
    let guard = (TILE_GUARD_METRES * shown_px_per_metre * supersample as f32).ceil() as usize;

    let field = WorldField::lit_by(params.seed, params.light);
    let mut canvas = vec![0u8; options.width * options.height * 3];
    #[allow(clippy::disallowed_types)]
    let started = std::time::Instant::now();
    let blender = cycles::blender_path();
    let directory = PathBuf::from(&options.scene_dir);
    let mut total_blades = 0usize;

    println!(
        "{}x{} shown at {:.0} px/m ({:.1}x{:.1} m of ground)",
        options.width,
        options.height,
        shown_px_per_metre,
        options.width as f32 / shown_px_per_metre,
        options.height as f32 / shown_px_per_metre,
    );
    if across > 1 {
        println!(
            "  {across}x{across} tiles of {tile_width}x{tile_height}, {guard} px guard, \
             {supersample}x supersample, crowding {crowding:.2}"
        );
    }

    for row in 0..across {
        for column in 0..across {
            // The tile's own window on the output, and the world it covers.
            let x0 = column * tile_width;
            let y0 = row * tile_height;
            let w = tile_width.min(options.width - x0);
            let h = tile_height.min(options.height - y0);

            let traced_w = w * supersample + guard * 2;
            let traced_h = h * supersample + guard * 2;
            let traced_px_per_metre = shown_px_per_metre * supersample as f32;
            // The page origin is in cache pixels at the page's own scale, so the
            // tile's offset scales with the supersample and the guard is
            // subtracted in the same units.
            let origin = options.origin * supersample as f32
                + Vec2::new(
                    (x0 * supersample) as f32 - guard as f32,
                    (y0 * supersample) as f32 - guard as f32,
                );

            let page = Page::at_detail(
                origin,
                traced_w,
                traced_h,
                traced_px_per_metre / bw_grass::iso::PX_PER_METRE,
            );
            let grown = GrassScene::build(page, &field, &params);
            let settings = RenderSettings {
                samples: options.samples,
                device: options.device.clone(),
                view_transform: options.view_transform.clone(),
                ribs: cycles::ribs_for(shown_px_per_metre),
                blade_width: options.blade_width * width_relief,
                passes: options.passes,
                ..default()
            };
            let scene = CyclesScene::build(&grown, &field, settings);
            total_blades += scene.blades();

            let vertices = scene.blades() * scene.ribs() * bw_grass::cycles::VERTICES_PER_RIB;
            if vertices > VERTEX_CEILING {
                eprintln!(
                    "\ntile {column},{row}: {:.0}M vertices is past the {:.0}M ceiling — \
                     Blender will run out of memory and take a segmentation fault rather \
                     than report anything.\nRaise --tiles, or lower --supersample.",
                    vertices as f64 / 1.0e6,
                    VERTEX_CEILING as f64 / 1.0e6,
                );
                std::process::exit(1);
            }

            let tile_png = directory.join(format!("tile-{row}-{column}.png"));
            let header = match scene.write(&directory) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("cannot write scene: {error}");
                    std::process::exit(1);
                }
            };
            let _ = std::fs::remove_file(&tile_png);
            match cycles::render(&header, &tile_png, &blender) {
                Ok(output) if tile_png.exists() && output.status.success() => {}
                Ok(output) => {
                    eprintln!(
                        "tile {column},{row} produced nothing:\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    std::process::exit(1);
                }
                Err(error) => {
                    eprintln!("cannot run blender: {error}");
                    std::process::exit(1);
                }
            }

            if !place_tile(
                &tile_png,
                &mut canvas,
                options.width,
                x0,
                y0,
                w,
                h,
                guard,
                supersample,
            ) {
                std::process::exit(1);
            }
            let _ = std::fs::remove_file(&tile_png);
            if across > 1 {
                println!(
                    "  tile {}/{}  {:.0} s elapsed",
                    row * across + column + 1,
                    across * across,
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }

    println!(
        "  {total_blades} blades over {} tiles, traced in {:.0} s",
        across * across,
        started.elapsed().as_secs_f64()
    );

    if let Err(error) = image::save_buffer(
        &options.out,
        &canvas,
        options.width as u32,
        options.height as u32,
        image::ColorType::Rgb8,
    ) {
        eprintln!("cannot write {}: {error}", options.out);
        std::process::exit(1);
    }
    println!("wrote {}", options.out);

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
const MIN_CROWDING: f32 = 0.22;

/// The most geometry one scene may ask Blender to hold.
///
/// A backstop, and it exists because the failure without it is not a slow render
/// — it is Blender taking a segmentation fault inside `Session::wait()`, several
/// minutes in, with a crash log instead of a picture. Twenty-three million
/// blades at seven ribs is half a billion vertices, and there is no message that
/// says so.
///
/// The number is measured rather than reasoned: a wide view at a hundred and
/// ninety million vertices renders in about two and a half minutes and one at
/// half a billion dies. This sits below the first with room to spare.
const VERTEX_CEILING: usize = 260_000_000;

/// How far a tile reaches beyond itself, in world metres.
///
/// Half a metre. A tile only holds the blades rooted inside it, so a blade just
/// outside its edge would cast no shadow into it and occlude nothing — and the
/// join would show as a bright seam, not a step. This is the tallest a blade
/// stands plus the ground it shades at the lowest sun the renderer supports.
///
/// The guard is rendered and then cropped away, which is the same discipline
/// `bake_padded` uses and for the same reason.
const TILE_GUARD_METRES: f32 = 0.5;

/// Crop a traced tile's guard band, filter it down, and put it on the canvas.
///
/// The guard is in *traced* pixels and the filter is a plain box average over
/// each output pixel's own footprint — see `grass_prebake` for why box is the
/// right filter here rather than a lazy one.
#[allow(clippy::too_many_arguments)]
fn place_tile(
    path: &std::path::Path,
    canvas: &mut [u8],
    canvas_width: usize,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    guard: usize,
    supersample: usize,
) -> bool {
    let image = match image::open(path) {
        Ok(image) => image.to_rgb8(),
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            return false;
        }
    };
    let stride = image.width() as usize;
    let area = (supersample * supersample) as u32;

    for y in 0..h {
        for x in 0..w {
            let mut sums = [0u32; 3];
            for dy in 0..supersample {
                for dx in 0..supersample {
                    let sx = guard + x * supersample + dx;
                    let sy = guard + y * supersample + dy;
                    if sx >= stride || sy >= image.height() as usize {
                        continue;
                    }
                    let pixel = image.get_pixel(sx as u32, sy as u32);
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += pixel[channel] as u32;
                    }
                }
            }
            let target = ((y0 + y) * canvas_width + x0 + x) * 3;
            for (channel, sum) in sums.iter().enumerate() {
                canvas[target + channel] = (sum / area) as u8;
            }
        }
    }
    true
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
            supersample: 1,
            tiles: 1,
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
                "--tiles" => options.tiles = value().parse().unwrap_or(options.tiles).clamp(1, 8),
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

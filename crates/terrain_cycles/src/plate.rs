//! One finished picture, traced in tiles and assembled.
//!
//! The vocabulary is worth fixing before the code, because three words were
//! being used for two things:
//!
//! - A **plate** is one logical raster output over a requested rectangle of
//!   world. It is what somebody asked for.
//! - A **tile** is a slice of a plate that fits in Blender's memory. It is an
//!   implementation detail of getting the plate traced, and nothing outside this
//!   module should have to know how many there were.
//! - A **page** is a unit of *storage* in the runtime cache, and is unrelated to
//!   either. See [`terrain_generators::page::Page`].
//!
//! This module owns the first two. It used to live inside the `grass_cycles`
//! example, which was the wrong place for it the moment a second caller wanted a
//! traced picture: the tiling arithmetic, the vertex ceiling and the guard-band
//! crop are the difference between a render and a segmentation fault twenty
//! minutes in, and none of that should be reachable only by running an example.
//!
//! ## The three derived numbers
//!
//! Everything hard here is a consequence of one fact: **a blade thinner than a
//! pixel does not minify into a thin blade, it minifies into nothing**, taking
//! its highlight and its silhouette with it. So three quantities scale with how
//! far out the camera is, and they have to move together:
//!
//! - The **trace resolution** is fixed at [`TRACE_PX_PER_METRE`] and the
//!   supersample is derived from it. A wide view is the same render over more
//!   ground, filtered down further — not a coarser one.
//! - The **blade width** grows as the view widens, because at the game's own
//!   framing a life-size blade is a fifth of a pixel. See [`blade_width_for`].
//! - The **tile count** comes from the vertex budget, because the scene's blade
//!   count is known before anything is traced and guessing it wrong is a crash
//!   rather than a slow render.

use std::path::{Path, PathBuf};

use glam::Vec2;

use crate::cycles::{self, CyclesScene, RenderSettings};
use terrain_generators::field::WorldField;
use terrain_generators::iso;
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;
use terrain_generators::style::GrassParams;

/// The framing blade width is authored against, in pixels per metre.
const WIDTH_REFERENCE_PX_PER_METRE: f32 = 108.0;

/// Blade half-width at the framing above, as a multiple of the authored value.
const WIDTH_AT_REFERENCE: f32 = 0.35;

/// The widest a blade may be drawn, however far the camera pulls back.
const WIDTH_CEILING: f32 = 0.95;

/// The detail every trace runs at, whatever scale the picture is shown at.
///
/// **The single number that decides whether a wide view works.**
///
/// A grass blade is about three millimetres across, so it is one pixel wide at
/// roughly 330 pixels to the metre. Below that it is a *partially covered*
/// pixel, and a canopy made of partially covered pixels averages to a flat wash
/// — no highlights, no silhouettes, no tufts, however many blades are in it.
/// Density does not argue with this and neither does sample count: the close-up
/// that measures well was traced at 324, and the wide view that failed was
/// traced at 155 with four times the samples.
pub const TRACE_PX_PER_METRE: f32 = 330.0;

/// The most the trace may be filtered down by.
///
/// Three, and the ceiling exists because supersampling cuts both ways. High
/// enough that a blade is at least a pixel wide *in the trace*; no higher,
/// because the filter that brings it back down averages away the very thing the
/// detail was for. Measured at the close-up framing, three gives an 8.5%
/// highlight share and five gives 2.6% — from the *same scene* at a *higher*
/// trace resolution. More detail, less picture.
pub const MAX_SUPERSAMPLE: usize = 3;

/// The most geometry one scene may ask Blender to hold.
///
/// A backstop, and it exists because the failure without it is not a slow render
/// — it is Blender taking a segmentation fault inside `Session::wait()`, several
/// minutes in, with a crash log instead of a picture. Measured rather than
/// reasoned: a wide view at a hundred and ninety million vertices renders in
/// about two and a half minutes and one at half a billion dies.
pub const VERTEX_CEILING: usize = 260_000_000;

/// The most vertices one tile may hold.
///
/// A quarter of [`VERTEX_CEILING`], so a tile has room for the guard band and
/// for Blender's own overhead without the estimate having to be exact.
const TILE_VERTEX_BUDGET: usize = VERTEX_CEILING / 4;

/// How far a tile reaches beyond itself, in world metres.
///
/// Half a metre. A tile only holds the blades rooted inside it, so a blade just
/// outside its edge would cast no shadow into it and occlude nothing — and the
/// join would show as a bright seam, not a step. This is the tallest a blade
/// stands plus the ground it shades at the lowest sun the renderer supports.
const TILE_GUARD_METRES: f32 = 0.5;

/// The most tiles a plate may be split into on each axis.
const MAX_TILES_ACROSS: usize = 8;

/// How wide to draw a blade that will be shown at `px_per_metre`.
///
/// **Blade width is a mip parameter**, which is not obvious and was the last
/// thing to go wrong. A blade drawn at life size is a fifth of a pixel at the
/// game camera. Measured at the overview, life-size blades gave a detail energy
/// of 15 against reference art's 22 and a highlight share of 0.4% against 3.3%.
///
/// Drawing them wider fixes that, and the same width ruins a close-up: at the
/// framing the look was tuned at it doubles the detail energy and the field
/// turns coarse and busy. There is no single number, because the question is not
/// how wide a blade is — it is how wide a blade has to be *drawn* to survive the
/// filtering between here and the screen.
pub fn blade_width_for(px_per_metre: f32) -> f32 {
    let ratio = WIDTH_REFERENCE_PX_PER_METRE / px_per_metre.max(1.0);
    (WIDTH_AT_REFERENCE * ratio).clamp(WIDTH_AT_REFERENCE, WIDTH_CEILING)
}

/// The population counts the path tracer wants, from the rasteriser's.
///
/// Seven times the density and blades a fifth longer. Not arbitrary, and not a
/// tuning knob that drifted: the rasteriser's numbers are counts of *strokes
/// covering pixels*, tuned so a 2D mark vocabulary filled the frame, and a path
/// tracer wants counts of *plants occupying space*. There is no way to derive
/// one from the other, so both are measured, and these are where the canopy
/// closes with warm ground still showing between the clumps.
pub fn cycles_params(params: &GrassParams) -> GrassParams {
    scaled_params(params, CYCLES_DENSITY, CYCLES_LENGTH)
}

/// Density multiplier from the rasteriser's counts to the tracer's.
pub const CYCLES_DENSITY: f32 = 7.0;

/// Blade-length multiplier from the rasteriser's to the tracer's.
pub const CYCLES_LENGTH: f32 = 1.2;

/// [`cycles_params`] with the two multipliers chosen by the caller.
pub fn scaled_params(params: &GrassParams, density: f32, length: f32) -> GrassParams {
    let mut scaled = *params;
    scaled.style.tufts *= density;
    scaled.style.fine *= density;
    scaled.style.thatch *= density;
    scaled.style.leaves *= density;
    scaled.style.blade_length.0 *= length;
    scaled.style.blade_length.1 *= length;
    scaled
}

/// What to trace.
#[derive(Clone, Debug)]
pub struct PlateRequest {
    /// Output size, in final pixels.
    pub width: usize,
    pub height: usize,
    /// Cache-pixel corner of the plate, at the scale it is *shown* at.
    pub origin: Vec2,
    /// Cache pixels per world metre the finished picture is shown at.
    pub px_per_metre: f32,
    /// Zero derives it from [`TRACE_PX_PER_METRE`].
    pub supersample: usize,
    /// Tiles on each axis. Zero derives it from the vertex budget.
    pub tiles: usize,
    /// Zero derives it from the framing — see [`blade_width_for`].
    pub blade_width: f32,
    /// The ground the render is *of*, in world metres. `None` is the whole page.
    ///
    /// Set it and the film goes transparent, the ground mesh ends at this
    /// rectangle, and blades rooted outside it shadow inward without appearing.
    /// See [`crate::cycles::RenderSettings::visible_ground`].
    pub visible: Option<(Vec2, Vec2)>,
    pub settings: RenderSettings,
    /// Where the exported scene package is staged.
    pub scene_dir: PathBuf,
    /// Keep the staged package after a successful trace.
    pub keep_scene: bool,
}

impl PlateRequest {
    /// A square plate at a chosen scale, with everything else derived.
    pub fn square(side: usize, px_per_metre: f32) -> Self {
        Self {
            width: side,
            height: side,
            origin: Vec2::ZERO,
            px_per_metre,
            supersample: 0,
            tiles: 0,
            blade_width: 0.0,
            visible: None,
            settings: RenderSettings::default(),
            scene_dir: PathBuf::from("target/cycles-scene"),
            keep_scene: false,
        }
    }

    /// Frame the plate by how many world metres it shows vertically, the way a
    /// camera is set rather than the way a texture is.
    pub fn framed(mut self, view_metres: f32) -> Self {
        self.px_per_metre = self.height as f32 / view_metres.max(0.01);
        self
    }
}

/// The plan a request resolves to, before anything is traced.
///
/// Separated out so it can be printed, tested and sanity-checked without
/// starting Blender. Every one of these numbers used to be derived inline in the
/// middle of the render loop, where the only way to find out what it came to was
/// to run a render.
#[derive(Clone, Copy, Debug)]
pub struct PlatePlan {
    pub supersample: usize,
    pub tiles_across: usize,
    pub tile_width: usize,
    pub tile_height: usize,
    /// Guard band, in *traced* pixels.
    pub guard: usize,
    pub ribs: usize,
    pub blade_width: f32,
    /// Blades the whole plate is expected to hold.
    pub estimated_blades: f32,
    pub trace_px_per_metre: f32,
}

impl PlatePlan {
    /// Resolve every derived number for `request` under `params`.
    pub fn resolve(request: &PlateRequest, params: &GrassParams) -> Self {
        let shown = request.px_per_metre;
        let supersample = match request.supersample {
            0 => ((TRACE_PX_PER_METRE / shown).ceil() as usize).clamp(1, MAX_SUPERSAMPLE),
            given => given,
        };
        let blade_width = if request.blade_width > 0.0 {
            request.blade_width
        } else {
            blade_width_for(shown)
        };

        let ribs = cycles::ribs_for(shown);
        let ground_metres = (request.width as f32 / shown) * (request.height as f32 / shown);
        let estimated_blades = (params.style.tufts * params.style.blades_per_tuft.1 as f32
            + params.style.fine
            + params.style.thatch
            + params.style.leaves)
            * ground_metres;
        let vertices = estimated_blades * (ribs * cycles::VERTICES_PER_RIB) as f32;
        let tiles_across = match request.tiles {
            0 => ((vertices / TILE_VERTEX_BUDGET as f32).sqrt().ceil() as usize)
                .clamp(1, MAX_TILES_ACROSS),
            given => given.max(1),
        };

        Self {
            supersample,
            tiles_across,
            tile_width: request.width.div_ceil(tiles_across),
            tile_height: request.height.div_ceil(tiles_across),
            guard: (TILE_GUARD_METRES * shown * supersample as f32).ceil() as usize,
            ribs,
            blade_width,
            estimated_blades,
            trace_px_per_metre: shown * supersample as f32,
        }
    }

    pub fn tiles(&self) -> usize {
        self.tiles_across * self.tiles_across
    }
}

/// A finished plate, in RGBA8.
///
/// Four channels whether or not the film was transparent, because the
/// alternative is two plate types and a caller that has to know which it got.
/// An opaque trace fills the alpha with 255 and nothing downstream has to care.
pub struct Plate {
    /// Interleaved RGBA, unpremultiplied.
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Blades actually traced, summed across tiles.
    pub blades: usize,
    /// Placement groups — plants, not primitives — that reached a slice.
    ///
    /// Summed across slices, so a plant straddling a join is counted in both.
    /// The alternative is a number that disagrees with the picture whenever the
    /// plate is sliced, which is worse than one that is honestly a per-slice
    /// total.
    pub secondary_groups: usize,
    /// Prototype instances lowered: heads, petals and stones.
    pub secondary_instances: usize,
    /// Tessellated ribbons: the broad ground leaves.
    ///
    /// Counted apart from the instances because they are a different kind of
    /// geometry with a different cost, and because a leaf that stopped being
    /// drawn would otherwise be invisible in a total dominated by petals.
    pub secondary_ribbons: usize,
    pub plan: PlatePlan,
}

/// Channels in a plate's buffer.
const CHANNELS: usize = 4;

/// The overlap of two world rectangles, or an empty one at the first's corner.
fn intersect(
    a: terrain_core::coords::WorldRect,
    b: terrain_core::coords::WorldRect,
) -> terrain_core::coords::WorldRect {
    let min =
        terrain_core::coords::WorldPoint::new(a.min.u_m.max(b.min.u_m), a.min.v_m.max(b.min.v_m));
    let max =
        terrain_core::coords::WorldPoint::new(a.max.u_m.min(b.max.u_m), a.max.v_m.min(b.max.v_m));
    if max.u_m <= min.u_m || max.v_m <= min.v_m {
        // No overlap: an empty rectangle, so every group falls to halo or is
        // omitted rather than all of them being classified camera-visible.
        return terrain_core::coords::WorldRect::new(a.min, a.min);
    }
    terrain_core::coords::WorldRect::new(min, max)
}

/// The world rectangle a traced page can show, as an axis-aligned box.
fn page_world_bounds(page: &Page) -> terrain_core::coords::WorldRect {
    let corners = [
        page.ground_at(Vec2::ZERO),
        page.ground_at(Vec2::new(page.width as f32, 0.0)),
        page.ground_at(Vec2::new(0.0, page.height as f32)),
        page.ground_at(Vec2::new(page.width as f32, page.height as f32)),
    ];
    let low = corners
        .iter()
        .fold(Vec2::splat(f32::INFINITY), |a, c| a.min(*c));
    let high = corners
        .iter()
        .fold(Vec2::splat(f32::NEG_INFINITY), |a, c| a.max(*c));
    terrain_core::coords::WorldRect::new(
        terrain_core::coords::WorldPoint::new(low.x as f64, low.y as f64),
        terrain_core::coords::WorldPoint::new(high.x as f64, high.y as f64),
    )
}

/// How far up-light a secondary object can be rooted and still shade a slice.
///
/// The same derivation the tuned blades use: an object of height `H` under a
/// sun at elevation `e` throws its shadow `H/tan(e)`. A flower stands about a
/// third of a metre, so at the thirty-five degrees the meadow is tuned under
/// this is about half a metre — and at fifteen, where bare ground shows its
/// relief, nearly one and a quarter.
///
/// Derived rather than written down, because a constant sized for one sun
/// elevation silently under-guards the other, and the symptom is a stripe of
/// missing shade at the edge of every slice.
fn secondary_shadow_reach_m(params: &GrassParams) -> f64 {
    /// The tallest thing the secondary vocabulary grows, world metres.
    const SECONDARY_CEILING_M: f32 = 0.45;
    let sun = terrain_generators::iso::image_to_world(params.light).normalize_or(glam::Vec3::Z);
    // A sixteenth over, matching the tuned guard's convention: the band costs
    // area and the area is worth less than the defect.
    (SECONDARY_CEILING_M * terrain_generators::geometry::reach_per_height(sun) * 1.0625) as f64
}

impl Plate {
    /// Write the plate as a PNG.
    pub fn save(&self, path: &Path) -> Result<(), image::ImageError> {
        image::save_buffer(
            path,
            &self.pixels,
            self.width as u32,
            self.height as u32,
            image::ColorType::Rgba8,
        )
    }

    /// The colour channels alone.
    pub fn rgb(&self) -> Vec<u8> {
        self.pixels
            .chunks_exact(CHANNELS)
            .flat_map(|pixel| pixel[..3].to_vec())
            .collect()
    }

    /// What fraction of the plate is picture rather than background.
    pub fn coverage(&self) -> f32 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        let total: u64 = self
            .pixels
            .chunks_exact(CHANNELS)
            .map(|pixel| pixel[3] as u64)
            .sum();
        total as f32 / (255.0 * (self.pixels.len() / CHANNELS) as f32)
    }
}

/// What can go wrong between a request and a picture.
#[derive(Debug)]
pub enum PlateError {
    /// The scene package could not be staged.
    Export(std::io::Error),
    /// Blender could not be started at all.
    Launch(std::io::Error),
    /// Blender ran and produced nothing, or exited non-zero.
    Render {
        tile: (usize, usize),
        stderr: String,
    },
    /// A traced tile could not be read back.
    Readback { path: PathBuf, error: String },
    /// The scene is past [`VERTEX_CEILING`] and Blender would take a
    /// segmentation fault rather than report anything.
    TooLarge {
        tile: (usize, usize),
        vertices: usize,
        ceiling: usize,
    },
}

impl std::fmt::Display for PlateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Export(error) => write!(f, "cannot write the scene: {error}"),
            Self::Launch(error) => write!(f, "cannot run blender: {error}"),
            Self::Render { tile, stderr } => {
                write!(f, "tile {},{} produced nothing:\n{stderr}", tile.0, tile.1)
            }
            Self::Readback { path, error } => {
                write!(f, "cannot read {}: {error}", path.display())
            }
            Self::TooLarge {
                tile,
                vertices,
                ceiling,
            } => write!(
                f,
                "tile {},{}: {:.0}M vertices is past the {:.0}M ceiling — Blender \
                 will run out of memory and take a segmentation fault rather than \
                 report anything. Raise the tile count, or lower the supersample.",
                tile.0,
                tile.1,
                *vertices as f64 / 1.0e6,
                *ceiling as f64 / 1.0e6,
            ),
        }
    }
}

impl std::error::Error for PlateError {}

/// How far along a trace is, for a caller that wants to say so.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub tile: usize,
    pub tiles: usize,
    pub blades: usize,
}

/// Trace a plate, tile by tile, and assemble it.
///
/// `params` is taken as given — apply [`cycles_params`] first if the caller
/// wants the path tracer's population counts rather than the rasteriser's.
pub fn trace(
    request: &PlateRequest,
    params: &GrassParams,
    field: &WorldField,
    secondary: Option<&terrain_scene::scene::TerrainScene>,
    progress: &mut dyn FnMut(Progress),
) -> Result<Plate, PlateError> {
    let plan = PlatePlan::resolve(request, params);
    let blender = cycles::blender_path();
    let mut canvas = vec![0u8; request.width * request.height * CHANNELS];
    let mut blades = 0usize;
    let mut secondary_groups = 0usize;
    let mut secondary_instances = 0usize;
    let mut secondary_ribbons = 0usize;

    for row in 0..plan.tiles_across {
        for column in 0..plan.tiles_across {
            // The tile's own window on the output, and the world it covers.
            let x0 = column * plan.tile_width;
            let y0 = row * plan.tile_height;
            let w = plan.tile_width.min(request.width - x0);
            let h = plan.tile_height.min(request.height - y0);

            let traced_w = w * plan.supersample + plan.guard * 2;
            let traced_h = h * plan.supersample + plan.guard * 2;
            // The page origin is in cache pixels at the page's own scale, so the
            // tile's offset scales with the supersample and the guard is
            // subtracted in the same units.
            let origin = request.origin * plan.supersample as f32
                + Vec2::new(
                    (x0 * plan.supersample) as f32 - plan.guard as f32,
                    (y0 * plan.supersample) as f32 - plan.guard as f32,
                );

            let page = Page::at_detail(
                origin,
                traced_w,
                traced_h,
                plan.trace_px_per_metre / iso::PX_PER_METRE,
            );
            let grown = GrassScene::build(page, field, params);
            let settings = RenderSettings {
                ribs: plan.ribs,
                blade_width: plan.blade_width,
                visible_ground: request.visible,
                trace_px_per_metre: plan.trace_px_per_metre,
                ..request.settings.clone()
            };
            let mut scene = CyclesScene::build(&grown, field, settings);
            blades += scene.blades();

            // The compiled scene, selected for this slice and lowered.
            //
            // Selected per slice and *never regenerated*: the compiler ran once
            // over the whole plate, so a flower on a slice boundary is the same
            // flower from both sides rather than two flowers that happen to
            // agree.
            if let Some(compiled) = secondary {
                // The ground the *plate* is of, intersected with what this page
                // can reach.
                //
                // Not the page's own world box, which was the first version and
                // was wrong in a way the picture made obvious: a traced page is
                // a rectangle of screen and its world footprint is the diamond's
                // bounding box, several times the ground actually being
                // rendered. Every flower in that surplus came through as
                // camera-visible and stood in the black void beyond the tiles.
                //
                // A plant outside the visible ground still belongs in the scene
                // — it shades inward — so it is halo rather than absent, and
                // `visible_ground` is what decides which.
                let page_box = page_world_bounds(&page);
                let visible = match request.visible {
                    None => page_box,
                    Some((low, high)) => intersect(
                        page_box,
                        terrain_core::coords::WorldRect::new(
                            terrain_core::coords::WorldPoint::new(low.x as f64, low.y as f64),
                            terrain_core::coords::WorldPoint::new(high.x as f64, high.y as f64),
                        ),
                    ),
                };
                let (geometry, report) =
                    crate::bridge::lower(compiled, visible, secondary_shadow_reach_m(params));
                if !report.unsupported.is_empty() {
                    // Printed rather than swallowed. A flower that silently did
                    // not render looks exactly like a flower that was never
                    // placed.
                    for (appearance, count) in &report.unsupported {
                        println!(
                            "  [terrain_cycles] {count} mark(s) of `{appearance}` have no lowering"
                        );
                    }
                }
                secondary_groups += report.total_groups();
                secondary_instances += report.instances;
                secondary_ribbons += report.ribbons;
                scene.secondary = geometry;
            }

            let vertices = scene.blades() * scene.ribs() * cycles::VERTICES_PER_RIB;
            if vertices > VERTEX_CEILING {
                return Err(PlateError::TooLarge {
                    tile: (column, row),
                    vertices,
                    ceiling: VERTEX_CEILING,
                });
            }

            let tile_png = request.scene_dir.join(format!("tile-{row}-{column}.png"));
            let header = scene
                .write(&request.scene_dir)
                .map_err(PlateError::Export)?;
            let _ = std::fs::remove_file(&tile_png);
            match cycles::render(&header, &tile_png, &blender) {
                Ok(output) if tile_png.exists() && output.status.success() => {}
                Ok(output) => {
                    return Err(PlateError::Render {
                        tile: (column, row),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    });
                }
                Err(error) => return Err(PlateError::Launch(error)),
            }

            place_tile(
                &tile_png,
                &mut canvas,
                request.width,
                (x0, y0),
                (w, h),
                plan.guard,
                plan.supersample,
            )?;
            let _ = std::fs::remove_file(&tile_png);
            progress(Progress {
                tile: row * plan.tiles_across + column + 1,
                tiles: plan.tiles(),
                blades,
            });
        }
    }

    if !request.keep_scene {
        let _ = std::fs::remove_dir_all(&request.scene_dir);
    }

    Ok(Plate {
        pixels: canvas,
        width: request.width,
        height: request.height,
        blades,
        secondary_groups,
        secondary_instances,
        secondary_ribbons,
        plan,
    })
}

/// Crop a traced tile's guard band, filter it down, and put it on the canvas.
///
/// The guard is in *traced* pixels and the filter is a plain box average over
/// each output pixel's own footprint. Box rather than anything cleverer because
/// the trace is already a supersampled estimate of the same integral — a
/// windowed filter here would be weighting samples that are already unweighted
/// draws from the pixel's own area, which is not sharpening, it is bias.
///
/// **Colour is weighted by coverage and alpha is not.** With a transparent film
/// the background samples come back black with zero alpha, and averaging them
/// into the colour unweighted would darken every edge of the diamond and every
/// blade against the sky — a black fringe on precisely the silhouette this whole
/// change exists to produce.
fn place_tile(
    path: &Path,
    canvas: &mut [u8],
    canvas_width: usize,
    (x0, y0): (usize, usize),
    (w, h): (usize, usize),
    guard: usize,
    supersample: usize,
) -> Result<(), PlateError> {
    let image = image::open(path)
        .map_err(|error| PlateError::Readback {
            path: path.to_path_buf(),
            error: error.to_string(),
        })?
        .to_rgba8();
    let stride = image.width() as usize;
    let rows = image.height() as usize;
    let area = (supersample * supersample) as f32;

    for y in 0..h {
        for x in 0..w {
            let mut colour = [0.0f32; 3];
            let mut coverage = 0.0f32;
            for dy in 0..supersample {
                for dx in 0..supersample {
                    let sx = guard + x * supersample + dx;
                    let sy = guard + y * supersample + dy;
                    if sx >= stride || sy >= rows {
                        continue;
                    }
                    let pixel = image.get_pixel(sx as u32, sy as u32);
                    let alpha = pixel[3] as f32 / 255.0;
                    for (channel, sum) in colour.iter_mut().enumerate() {
                        *sum += pixel[channel] as f32 * alpha;
                    }
                    coverage += alpha;
                }
            }
            let target = ((y0 + y) * canvas_width + x0 + x) * CHANNELS;
            for (channel, sum) in colour.iter().enumerate() {
                canvas[target + channel] = if coverage > 0.0 {
                    (sum / coverage).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                };
            }
            canvas[target + 3] = ((coverage / area) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_up_needs_no_tiling() {
        let request = PlateRequest::square(512, 192.0);
        let plan = PlatePlan::resolve(&request, &cycles_params(&GrassParams::default()));
        assert_eq!(plan.tiles_across, 1);
        assert!(plan.supersample >= 1);
    }

    #[test]
    fn a_wide_view_is_tiled_rather_than_thinned() {
        // The decision this module exists to make. Widening the framing must
        // add tiles, never quietly reduce the population — thinning holds
        // coverage and loses structure, which measured took coherence from 0.46
        // to 0.22.
        let params = cycles_params(&GrassParams::default());
        let close = PlatePlan::resolve(&PlateRequest::square(512, 192.0), &params);
        let wide = PlatePlan::resolve(&PlateRequest::square(1920, 192.0).framed(55.0), &params);
        assert!(
            wide.tiles_across > close.tiles_across,
            "a wide view came out at {} tiles, the same as a close-up",
            wide.tiles_across
        );
    }

    #[test]
    fn every_tile_stays_under_the_vertex_ceiling() {
        // The plan's whole job. A tile past the ceiling is a Blender
        // segmentation fault several minutes in, not an error message.
        let params = cycles_params(&GrassParams::default());
        for (side, px_per_metre) in [(512, 192.0), (1920, 96.0), (1920, 35.0), (3840, 40.0)] {
            let request = PlateRequest::square(side, px_per_metre);
            let plan = PlatePlan::resolve(&request, &params);
            let per_tile = plan.estimated_blades / plan.tiles() as f32;
            let vertices = per_tile * (plan.ribs * cycles::VERTICES_PER_RIB) as f32;
            assert!(
                vertices < VERTEX_CEILING as f32,
                "{side}px at {px_per_metre} px/m plans {:.0}M vertices a tile",
                vertices / 1.0e6
            );
        }
    }

    #[test]
    fn the_trace_resolution_holds_as_the_view_widens() {
        // A wide view is the same render over more ground, filtered down
        // further — not a coarser one. That is the whole reason the supersample
        // is derived rather than chosen.
        let params = cycles_params(&GrassParams::default());
        let mut previous = 0usize;
        for view in [10.0f32, 26.0, 55.0] {
            let plan = PlatePlan::resolve(&PlateRequest::square(1080, 96.0).framed(view), &params);
            assert!(
                plan.supersample >= previous,
                "the supersample fell as the view widened"
            );
            previous = plan.supersample;
        }
        assert_eq!(
            previous, MAX_SUPERSAMPLE,
            "the widest view did not reach the ceiling"
        );
    }

    #[test]
    fn blades_widen_as_the_camera_pulls_back() {
        // Blade width is a mip parameter. Life-size blades at the overview
        // measured a highlight share of 0.4% against reference art's 3.3%.
        let close = blade_width_for(330.0);
        let shipping = blade_width_for(20.0);
        assert!(shipping > close);
        assert_eq!(
            close, WIDTH_AT_REFERENCE,
            "the close-up left the authored width"
        );
        assert!(shipping <= WIDTH_CEILING);
    }

    #[test]
    fn the_tracer_gets_more_grass_than_the_rasteriser() {
        let base = GrassParams::default();
        let traced = cycles_params(&base);
        assert!(traced.style.tufts > base.style.tufts);
        assert!(traced.style.fine > base.style.fine);
        assert!(traced.style.blade_length.1 > base.style.blade_length.1);
    }

    #[test]
    fn tiles_cover_the_plate_exactly() {
        // An off-by-one here is a black stripe down the middle of a render that
        // took twenty minutes.
        let params = cycles_params(&GrassParams::default());
        for (width, height) in [(512, 512), (1920, 1080), (1000, 999), (7, 13)] {
            let request = PlateRequest {
                width,
                height,
                ..PlateRequest::square(width, 96.0)
            };
            let plan = PlatePlan::resolve(&request, &params);
            let mut covered = vec![0u8; width * height];
            for row in 0..plan.tiles_across {
                for column in 0..plan.tiles_across {
                    let x0 = column * plan.tile_width;
                    let y0 = row * plan.tile_height;
                    if x0 >= width || y0 >= height {
                        continue;
                    }
                    for y in y0..(y0 + plan.tile_height).min(height) {
                        for x in x0..(x0 + plan.tile_width).min(width) {
                            covered[y * width + x] += 1;
                        }
                    }
                }
            }
            assert!(
                covered.iter().all(|&n| n == 1),
                "{width}x{height} is not covered exactly once by its tiles"
            );
        }
    }

    #[test]
    fn a_traced_plate_frames_exactly_what_the_raster_plate_does() {
        // The registration check, and the reason both commands go through one
        // resolver. The cheap plate and the traced plate are the two halves of a
        // training pair; a camera half a pixel out makes every pair a small
        // translation, and a network trained on that learns to blur.
        use terrain_scene::frame::{IsoFrameOptions, ResolvedIsoFrame};
        use terrain_scene::layout::{IsoTileLayout, WorldTileCoord};
        use terrain_scene::projection::Projection;

        let layout = IsoTileLayout::nine(WorldTileCoord::new(-713, 284), 4.0).expect("well formed");
        let frame = ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::sized(960, 540),
        );

        // The page the rasteriser bakes, and the page the tracer's first slice
        // grows, are anchored at the same cache pixel at the same scale.
        let raster = Page::at_detail(
            Vec2::new(frame.cache_origin[0], frame.cache_origin[1]),
            960,
            540,
            frame.pixels_per_metre / iso::PX_PER_METRE,
        );

        // The camera that photographs that page has to look at the ground under
        // the middle of the frame, which is the subject tile's own centre.
        let camera = cycles::Camera::for_page(&raster, 0.5);
        let centre_pixel = Vec2::new(480.0, 270.0);
        let aimed = raster.ground_at(centre_pixel);
        let subject = frame.layout.subject_centre();
        assert!(
            (aimed.x as f64 - subject.u_m).abs() < 0.02
                && (aimed.y as f64 - subject.v_m).abs() < 0.02,
            "{aimed:?} against {subject}"
        );
        // And the camera is aimed at that same point, reflected.
        let target = camera.location - camera.basis[2] * (40.0 + 0.5 * 4.0);
        assert!((target.x - aimed.y).abs() < 0.02, "{target:?} {aimed:?}");
        assert!((target.y - aimed.x).abs() < 0.02, "{target:?} {aimed:?}");
    }

    #[test]
    fn the_memory_split_does_not_move_the_world_tiles() {
        // The two things this repository calls a tile, kept apart by a test.
        // `--trace-tiles-across` is how many pieces Blender traces the plate in;
        // it must not change the framing by so much as a pixel, or a render
        // would come out at a different place on a machine with less memory.
        use terrain_scene::frame::{IsoFrameOptions, ResolvedIsoFrame};
        use terrain_scene::layout::{IsoTileLayout, WorldTileCoord};
        use terrain_scene::projection::Projection;

        let layout = IsoTileLayout::nine(WorldTileCoord::new(-713, 284), 2.0).expect("well formed");
        let frame = ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::sized(960, 540),
        );
        let params = cycles_params(&GrassParams::default());

        let mut plans = Vec::new();
        for trace_tiles in [1usize, 2, 4] {
            let request = PlateRequest {
                width: 960,
                height: 540,
                origin: Vec2::new(frame.cache_origin[0], frame.cache_origin[1]),
                px_per_metre: frame.pixels_per_metre,
                tiles: trace_tiles,
                ..PlateRequest::square(960, frame.pixels_per_metre)
            };
            let plan = PlatePlan::resolve(&request, &params);
            assert_eq!(plan.tiles_across, trace_tiles);
            // The framing numbers are the same however the plate is sliced.
            assert_eq!(request.origin.x, frame.cache_origin[0]);
            assert_eq!(request.px_per_metre, frame.pixels_per_metre);
            plans.push((plan.supersample, plan.blade_width, plan.trace_px_per_metre));
        }
        assert!(
            plans.windows(2).all(|w| w[0] == w[1]),
            "the memory split changed the trace: {plans:?}"
        );
    }

    #[test]
    fn the_guard_band_reaches_past_the_tallest_blade() {
        // A guard shorter than a blade's shadow is a bright seam at every tile
        // join, and a seam is the one artefact this whole design exists to
        // avoid.
        let params = cycles_params(&GrassParams::default());
        let plan = PlatePlan::resolve(&PlateRequest::square(1024, 192.0), &params);
        let guard_metres = plan.guard as f32 / plan.trace_px_per_metre;
        assert!(
            guard_metres >= params.style.blade_length.1,
            "a {guard_metres:.2} m guard against blades up to {:.2} m",
            params.style.blade_length.1
        );
    }
}

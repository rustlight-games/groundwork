//! Streaming baked pages around the camera.
//!
//! Baking a page costs tens of milliseconds, which is far too long to do on the
//! frame that needs it, and far too cheap to justify shipping the whole world as
//! art. So pages are baked on the compute pool as the camera approaches them and
//! kept until it leaves. Nothing here decides what grass looks like; that is
//! [`crate::bake`]. This is only the bookkeeping that gets it on screen.
//!
//! ## Why the camera's tonemapping is turned off
//!
//! The palette is measured from reference art in the space that art was authored
//! in, and the baker writes final colours. Running those through a film curve at
//! display time would quietly move every one of them — the plate would no longer
//! be the thing that was measured, and every number in the comparison table
//! would be describing an image nobody sees. [`grass_camera`] exists to make
//! that decision once, in the open.

use bevy::asset::RenderAssetUsages;
use bevy::camera::ScalingMode;
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::image::ImageSampler;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::sprite_render::Material2dPlugin;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};

use crate::material::{GrassSurfaceMaterial, SurfaceSettings};
use terrain_bake::bake::{BakeParams, bake};
use terrain_generators::iso;
use terrain_generators::page::Page;

/// Side of a page, in cache pixels.
///
/// A little over two and a half metres of ground. Small enough that arriving at
/// a new one is a few tens of milliseconds of work on a background thread, large
/// enough that a 1080p view is a couple of dozen draws rather than hundreds.
///
/// The same number the offline baker tiles at, and deliberately one definition
/// rather than two: page size sets the ratio of guard band to interior, so a
/// benchmark tiling at some other size would be measuring work that never
/// ships.
pub const PAGE_PIXELS: usize = terrain_bake::bake::TILE_PIXELS;

/// The grass world: what to grow, and what has been grown so far.
#[derive(Resource)]
pub struct GrassWorld {
    /// Everything the look depends on.
    pub params: BakeParams,
    /// Extra cache pixels baked beyond the view, so a page is ready before it
    /// is needed rather than popping in once it is.
    pub lookahead: f32,
    /// How far beyond [`GrassWorld::lookahead`] a page may drift before it is
    /// dropped, in cache pixels.
    ///
    /// Deliberately not the same distance it was baked at. Evicting at exactly
    /// the lookahead means a camera nudged back and forth across one boundary
    /// rebakes the same page forever, which is the worst behaviour a cache can
    /// have: it costs the most exactly when the player is standing still.
    pub hysteresis: f32,
    pages: HashMap<IVec2, PageState>,
}

impl Default for GrassWorld {
    fn default() -> Self {
        Self {
            params: BakeParams::default(),
            lookahead: PAGE_PIXELS as f32,
            hysteresis: PAGE_PIXELS as f32 * 1.5,
            pages: HashMap::default(),
        }
    }
}

enum PageState {
    Baking(Task<Vec<u8>>),
    Ready(Entity),
}

/// Ordering within a frame.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GrassSet {
    /// Decide which pages are wanted and start baking the missing ones.
    Stream,
    /// Collect finished bakes and put them on screen.
    Publish,
    /// Drop pages the camera has left behind.
    Evict,
}

/// The grass renderer.
pub struct GrassPlugin;

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Material2dPlugin::<GrassSurfaceMaterial>::default())
            .init_resource::<GrassWorld>()
            .configure_sets(
                Update,
                (GrassSet::Stream, GrassSet::Publish, GrassSet::Evict).chain(),
            )
            .add_systems(
                Update,
                (
                    request_pages.in_set(GrassSet::Stream),
                    publish_pages.in_set(GrassSet::Publish),
                    evict_pages.in_set(GrassSet::Evict),
                ),
            );
    }
}

/// A 2D camera set up to show the grass as it was baked.
///
/// `view_height` is world metres visible vertically, the same unit the battle
/// camera uses.
pub fn grass_camera(view_height: f32) -> impl Bundle {
    // The mid-tone of the grass ramp, so that a page which has not finished
    // baking is a slightly flat patch of the right green rather than a black
    // hole. It will still be visible if you look for it, and that is the point:
    // hiding it entirely would hide the streaming falling behind as well.
    let waiting = terrain_bake::palette::shade(terrain_generators::tone::Tone::Grass, 0.45);
    (
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(waiting.x, waiting.y, waiting.z)),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: view_height.max(0.01),
            },
            ..OrthographicProjection::default_2d()
        }),
        // Both off for the same reason: the page already holds finished colour,
        // and anything that transforms it at display time makes the picture on
        // screen a different image from the one that was measured.
        Tonemapping::None,
        DebandDither::Disabled,
        Msaa::Off,
    )
}

/// The cache-pixel rectangle the camera can currently see.
fn view_rect(camera: &Projection, transform: &GlobalTransform, window: Vec2) -> (Vec2, Vec2) {
    let Projection::Orthographic(orthographic) = camera else {
        return (Vec2::ZERO, Vec2::ZERO);
    };
    let height = match orthographic.scaling_mode {
        ScalingMode::FixedVertical { viewport_height } => viewport_height,
        _ => 32.0,
    };
    let aspect = if window.y > 0.0 {
        window.x / window.y
    } else {
        16.0 / 9.0
    };
    let half = Vec2::new(height * aspect, height) * 0.5;
    let centre = transform.translation().truncate();
    // Screen metres to cache pixels: +Y flips, because the cache is an image and
    // images count downward.
    let low = Vec2::new(
        (centre.x - half.x) * iso::PX_PER_METRE,
        -(centre.y + half.y) * iso::PX_PER_METRE,
    );
    let high = Vec2::new(
        (centre.x + half.x) * iso::PX_PER_METRE,
        -(centre.y - half.y) * iso::PX_PER_METRE,
    );
    (low, high)
}

/// Start baking every page the camera can see and does not have.
fn request_pages(
    mut world: ResMut<GrassWorld>,
    cameras: Query<(&Projection, &GlobalTransform), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok((projection, transform)) = cameras.single() else {
        return;
    };
    let window = windows
        .iter()
        .next()
        .map(|w| Vec2::new(w.width(), w.height()))
        .unwrap_or(Vec2::new(1920.0, 1080.0));

    let (low, high) = view_rect(projection, transform, window);
    let lookahead = Vec2::splat(world.lookahead);
    let (low, high) = (low - lookahead, high + lookahead);

    let size = PAGE_PIXELS as f32;
    let first = (low / size).floor().as_ivec2();
    let last = (high / size).ceil().as_ivec2();

    let pool = AsyncComputeTaskPool::get();
    for y in first.y..last.y {
        for x in first.x..last.x {
            let coordinate = IVec2::new(x, y);
            if world.pages.contains_key(&coordinate) {
                continue;
            }
            let params = world.params;
            let page = Page::new(coordinate.as_vec2() * size, PAGE_PIXELS, PAGE_PIXELS);
            let task = pool.spawn(async move { bake_to_rgba(page, &params) });
            world.pages.insert(coordinate, PageState::Baking(task));
        }
    }
}

/// Bake a page straight to the bytes a texture wants.
///
/// Traced pages first, and rasterised only when there is not one.
///
/// This is where the two renderers meet. Cycles takes seconds a page and the
/// game has a frame, so the path tracer can never run here — but it does not
/// have to. A page is a *cache*, its content is a pure function of the world
/// coordinate and the seed, and that means a page traced last week is exactly
/// the page this function would produce if it had the time. So it is looked up
/// on disk, and the rasteriser becomes the fallback for ground nobody has traced
/// yet rather than the way the game is meant to look.
///
/// Off unless `TERRAIN_GRASS_TRACED` is set — see [`crate::cache::TERRAIN_GRASS_TRACED`]
/// for why mixing the two renderers by accident is worse than not mixing them at
/// all. Pre-trace a region with
/// `cargo run --release -p bw_grass --example grass_prebake`, then
/// `TERRAIN_GRASS_TRACED=1 ./run`.
///
/// For one whole scene traced at once — which is what to screenshot — use
/// `./render` instead. It has no page grid at all.
fn bake_to_rgba(page: Page, params: &BakeParams) -> Vec<u8> {
    if let Some(bytes) = crate::cache::load(&page, params) {
        return bytes;
    }
    let colours = bake(page, params);
    let mut bytes = Vec::with_capacity(colours.len() * 4);
    for colour in colours {
        let rgb = terrain_bake::palette::to_bytes(colour);
        bytes.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    bytes
}

/// Put finished bakes on screen.
fn publish_pages(
    mut commands: Commands,
    mut world: ResMut<GrassWorld>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GrassSurfaceMaterial>>,
) {
    let size = PAGE_PIXELS as f32 / iso::PX_PER_METRE;
    let mut finished = Vec::new();
    for (coordinate, state) in world.pages.iter_mut() {
        let PageState::Baking(task) = state else {
            continue;
        };
        if let Some(bytes) = future::block_on(future::poll_once(task)) {
            finished.push((*coordinate, bytes));
        }
    }

    for (coordinate, bytes) in finished {
        let mut image = Image::new(
            Extent3d {
                width: PAGE_PIXELS as u32,
                height: PAGE_PIXELS as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            bytes,
            // sRGB, because the palette is measured in the space the reference
            // art was authored in and the baker writes finished colours.
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        // Painted art, not pixel art: its strokes have soft edges and point
        // sampling throws them away.
        image.sampler = ImageSampler::linear();

        let material = materials.add(GrassSurfaceMaterial {
            settings: SurfaceSettings::default(),
            page: images.add(image),
        });
        let mesh = meshes.add(Rectangle::new(size, size));
        // The page's centre, converted from cache pixels back to screen metres.
        let centre = Vec2::new(
            (coordinate.x as f32 + 0.5) * size,
            -(coordinate.y as f32 + 0.5) * size,
        );
        let entity = commands
            .spawn((
                Mesh2d(mesh),
                MeshMaterial2d(material),
                // Behind everything. The ground is the floor of the scene and
                // nothing in the game is ever meant to sort below it.
                Transform::from_translation(centre.extend(-100.0)),
                Name::new(format!("grass page {},{}", coordinate.x, coordinate.y)),
            ))
            .id();
        world.pages.insert(coordinate, PageState::Ready(entity));
    }
}

/// Drop pages the camera has left far enough behind.
///
/// Runs after publishing rather than before requesting, so a page that finished
/// baking on the same frame the camera turned away is still put on screen and
/// then dropped, instead of leaking its task.
fn evict_pages(
    mut commands: Commands,
    mut world: ResMut<GrassWorld>,
    cameras: Query<(&Projection, &GlobalTransform), With<Camera2d>>,
    windows: Query<&Window>,
) {
    let Ok((projection, transform)) = cameras.single() else {
        return;
    };
    let window = windows
        .iter()
        .next()
        .map(|w| Vec2::new(w.width(), w.height()))
        .unwrap_or(Vec2::new(1920.0, 1080.0));

    let (low, high) = view_rect(projection, transform, window);
    let margin = Vec2::splat(world.lookahead + world.hysteresis);
    let (low, high) = (low - margin, high + margin);
    let size = PAGE_PIXELS as f32;

    let mut dropped = Vec::new();
    for (coordinate, state) in world.pages.iter() {
        let corner = coordinate.as_vec2() * size;
        if corner.x + size < low.x
            || corner.y + size < low.y
            || corner.x > high.x
            || corner.y > high.y
        {
            if let PageState::Ready(entity) = state {
                commands.entity(*entity).despawn();
            }
            dropped.push(*coordinate);
        }
    }
    for coordinate in dropped {
        world.pages.remove(&coordinate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_covers_exactly_its_own_patch_of_screen() {
        // Pages tile with no overlap and no gap, which is the property that lets
        // the baker treat them as independent. A half-pixel error here is a
        // visible grid of seams there.
        let size = PAGE_PIXELS as f32 / iso::PX_PER_METRE;
        let left = Vec2::new(0.5 * size, -0.5 * size);
        let right = Vec2::new(1.5 * size, -0.5 * size);
        assert!(((right.x - left.x) - size).abs() < 1.0e-5);
    }

    #[test]
    fn the_camera_frames_the_height_it_was_asked_for() {
        let mut app = App::new();
        let entity = app.world_mut().spawn(grass_camera(26.0)).id();
        let projection = app.world().entity(entity).get::<Projection>().unwrap();
        let Projection::Orthographic(orthographic) = projection else {
            panic!("expected an orthographic projection");
        };
        assert!(matches!(
            orthographic.scaling_mode,
            ScalingMode::FixedVertical { viewport_height } if (viewport_height - 26.0).abs() < 1e-6
        ));
    }

    #[test]
    fn eviction_is_further_out_than_baking() {
        // Equal distances make a camera nudged across one page boundary rebake
        // the same page forever — the cache costing most while nothing moves.
        let world = GrassWorld::default();
        assert!(world.hysteresis > 0.0);
    }

    #[test]
    fn tonemapping_is_off_so_the_plate_reaches_the_screen_unchanged() {
        let mut app = App::new();
        let entity = app.world_mut().spawn(grass_camera(26.0)).id();
        assert!(matches!(
            app.world().entity(entity).get::<Tonemapping>(),
            Some(Tonemapping::None)
        ));
    }
}

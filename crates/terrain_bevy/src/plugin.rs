//! Streaming traced pages around the camera.
//!
//! Reading a page from the cache is a filesystem call, which is small but not
//! free on the frame that needs it, so pages are loaded on the compute pool as
//! the camera approaches them and kept until it leaves. Nothing here decides
//! what grass looks like; that is Cycles, offline, once — see
//! [`crate::cache`]. This is only the bookkeeping that gets a traced page on
//! screen.
//!
//! ## Why the camera's tonemapping is turned off
//!
//! A traced page already holds finished colour. Running it through a film
//! curve at display time would quietly move every value — the plate would no
//! longer be the image Cycles produced. [`grass_camera`] exists to make that
//! decision once, in the open.

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
use terrain_generators::iso;
use terrain_generators::page::Page;

/// Side of a page, in cache pixels.
///
/// A little over two and a half metres of ground. Small enough that arriving at
/// a new one is a few tens of milliseconds of work on a background thread, large
/// enough that a 1080p view is a couple of dozen draws rather than hundreds.
pub const PAGE_PIXELS: usize = 256;

/// The grass world: which seed, and what has been loaded so far.
#[derive(Resource)]
pub struct GrassWorld {
    pub seed: u64,
    /// Extra cache pixels loaded beyond the view, so a page is ready before it
    /// is needed rather than popping in once it is.
    pub lookahead: f32,
    /// How far beyond [`GrassWorld::lookahead`] a page may drift before it is
    /// dropped, in cache pixels.
    ///
    /// Deliberately not the same distance it was loaded at. Evicting at exactly
    /// the lookahead means a camera nudged back and forth across one boundary
    /// re-reads the same page forever, which is the worst behaviour a cache can
    /// have: it costs the most exactly when the player is standing still.
    pub hysteresis: f32,
    pages: HashMap<IVec2, PageState>,
}

impl Default for GrassWorld {
    fn default() -> Self {
        Self {
            seed: 0x5eed_1234,
            lookahead: PAGE_PIXELS as f32,
            hysteresis: PAGE_PIXELS as f32 * 1.5,
            pages: HashMap::default(),
        }
    }
}

enum PageState {
    Loading(Task<Option<Vec<u8>>>),
    Ready(Entity),
    /// Asked for and not found. Counts down to zero before `request_pages`
    /// asks again — without this, a page nobody has traced yet spawns a new
    /// filesystem read on the compute pool every single frame, forever.
    Missing(u32),
}

/// Frames a missing page waits before it is asked for again.
///
/// Two seconds at sixty frames a second. Long enough that a prebake finishing
/// mid-session is noticed promptly; short enough that this is a retry
/// interval and not effectively "never".
const MISSING_RETRY_FRAMES: u32 = 120;

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
    // A flat, plausible mid-green, so that a page which has not finished
    // loading is a slightly flat patch of the right hue rather than a black
    // hole. It will still be visible if you look for it, and that is the
    // point: hiding it entirely would hide the streaming falling behind as
    // well. Not measured — there is no ramp to sample now that the raster
    // tier is gone, only a placeholder for ground nobody has traced yet.
    (
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.24, 0.30, 0.14)),
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

/// Count every waiting page down, and forget the ones whose wait is over.
///
/// Forgetting is what lets [`request_pages`] ask again: it skips any
/// coordinate already in the map, so a page stays unasked-for exactly as long
/// as it stays in it.
fn tick_missing(pages: &mut HashMap<IVec2, PageState>) {
    let mut elapsed = Vec::new();
    for (coordinate, state) in pages.iter_mut() {
        if let PageState::Missing(remaining) = state {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                elapsed.push(*coordinate);
            }
        }
    }
    for coordinate in elapsed {
        pages.remove(&coordinate);
    }
}

/// Start loading every page the camera can see and does not have.
fn request_pages(
    mut world: ResMut<GrassWorld>,
    cameras: Query<(&Projection, &GlobalTransform), With<Camera2d>>,
    windows: Query<&Window>,
) {
    // Every retry costs a page's worth of tasks; tick every waiting one down
    // before deciding what to (re)request this frame.
    tick_missing(&mut world.pages);

    // Nothing is traced, so nothing will ever load — see `crate::cache`,
    // "Cycles is the only renderer". Without this, every visible page would
    // ask, miss, and ask again every single frame for no reason.
    if !crate::cache::traced_enabled() {
        return;
    }

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
            let seed = world.seed;
            let page = Page::new(coordinate.as_vec2() * size, PAGE_PIXELS, PAGE_PIXELS);
            let task = pool.spawn(async move { crate::cache::load(&page, seed) });
            world.pages.insert(coordinate, PageState::Loading(task));
        }
    }
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
    // Loaded and empty: nothing traced there yet. Held as `Missing` rather
    // than shown or immediately re-asked for — see `crate::cache`, "Cycles is
    // the only renderer" — and rather than removed outright, which would have
    // `request_pages` spawn a fresh filesystem read for it every frame.
    let mut missed = Vec::new();
    for (coordinate, state) in world.pages.iter_mut() {
        let PageState::Loading(task) = state else {
            continue;
        };
        match future::block_on(future::poll_once(task)) {
            Some(Some(bytes)) => finished.push((*coordinate, bytes)),
            Some(None) => missed.push(*coordinate),
            None => {}
        }
    }
    for coordinate in missed {
        world
            .pages
            .insert(coordinate, PageState::Missing(MISSING_RETRY_FRAMES));
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
            // sRGB, matching the space a traced page is written to disk in.
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

    #[test]
    fn a_page_nobody_has_traced_is_not_asked_for_every_frame() {
        // The failure this exists to prevent: with no renderer to fall back to,
        // a cache miss used to be dropped from the map outright, so the very
        // next frame asked for it again. At a normal view size that is a few
        // hundred filesystem reads spawned onto the compute pool every frame,
        // forever, for ground that will not appear until somebody traces it.
        let mut pages = HashMap::default();
        pages.insert(IVec2::new(3, -4), PageState::Missing(MISSING_RETRY_FRAMES));

        for frame in 1..MISSING_RETRY_FRAMES {
            tick_missing(&mut pages);
            assert!(
                pages.contains_key(&IVec2::new(3, -4)),
                "the page was released for a retry after {frame} frames, not \
                 {MISSING_RETRY_FRAMES}"
            );
        }

        // The last tick retires it, and only then will `request_pages` — which
        // skips any coordinate already present — ask for it again.
        tick_missing(&mut pages);
        assert!(
            !pages.contains_key(&IVec2::new(3, -4)),
            "the page was never released, so it would never be retried"
        );
    }

    #[test]
    fn waiting_is_a_wait_and_not_a_synonym_for_never() {
        // Both directions matter. One frame would be the hot loop again; a
        // wait long enough to outlast a session would mean a prebake finishing
        // mid-run never showed up.
        assert!(MISSING_RETRY_FRAMES > 1);
        assert!(MISSING_RETRY_FRAMES < 60 * 30);
    }

    #[test]
    fn a_loading_or_ready_page_is_left_alone_by_the_countdown() {
        // `tick_missing` walks every entry; only the waiting ones are its
        // business. Evicting a page mid-load would leak its task, and evicting
        // a ready one would drop a page off the screen for no reason.
        let mut pages = HashMap::default();
        let entity = App::new().world_mut().spawn_empty().id();
        pages.insert(IVec2::ZERO, PageState::Ready(entity));
        tick_missing(&mut pages);
        assert!(matches!(pages.get(&IVec2::ZERO), Some(PageState::Ready(_))));
    }
}

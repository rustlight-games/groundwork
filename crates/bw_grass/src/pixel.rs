//! The pixel canvas.
//!
//! Grass does not draw to the window. It draws to a small image — around 270
//! rows — which is then blitted to the window at a whole-number scale with
//! nearest sampling. Everything about the pixel-art look follows from that one
//! decision, and none of it can be faked afterwards:
//!
//! - **Pixels are a grid, not a filter.** A blade either covers a pixel or it
//!   does not. Post-processing a full-resolution image into chunky blocks gives
//!   the blocks but not the *alignment*, and misaligned blocks are the thing
//!   that reads as a filter rather than as art.
//! - **The shader knows how big a pixel is.** [`PixelCanvas::pixels_per_unit`]
//!   goes into the material, which is what lets a blade guarantee itself a
//!   whole number of pixels of width instead of landing on a sliver.
//! - **Multisampling is off, deliberately.** Everywhere else in this crate the
//!   argument for opaque ribbons was that MSAA resolves their edges for free.
//!   Here that is exactly wrong: an antialiased edge invents colours that are
//!   not in the palette, and a soft edge on a 1px stroke is most of the stroke.
//! - **Tonemapping is off** for the same reason. A tonemapper is a curve
//!   applied to the whole image; run the palette through one and what reaches
//!   the screen is no longer the palette that was authored.
//!
//! ## Sizing
//!
//! The canvas is the window's own resolution — see [`PIXEL_SCALE`] for why it
//! does not upscale. Should that ever change, the scale stays a whole number
//! and the canvas overscans rather than letterboxing: a fractional scale draws
//! some pixels four screen pixels wide and their neighbours five, which is far
//! more visible than it sounds.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::palette;

/// Screen pixels per canvas pixel.
///
/// **One.** The canvas is the window's own resolution and nothing is upscaled.
///
/// That deserves explaining, because a pixel canvas that does not scale looks
/// pointless. It is not: chunky pixels are a *different* style from what this
/// game is aiming at. The reference is Warcraft III — hand-painted, stylised,
/// deliberate texel detail, viewed from far enough away that no individual
/// pixel is ever a visible square. Blowing pixels up to three screen pixels
/// each reads as a retro de-make instead, and no amount of tuning the art
/// underneath fixes that, because the chunkiness *is* what the eye sees first.
///
/// Everything else the canvas does still earns its place at a scale of one:
///
/// - It is where multisampling and tonemapping are turned off, which keeps
///   every pixel exactly on palette.
/// - It defines [`PixelCanvas::pixels_per_unit`], which the vertex shader needs
///   to give a blade a whole number of pixels of width. Without that a stroke
///   at this distance lands on fractional coverage and shimmers as it moves.
/// - It keeps the option open. Raising this to two or three is a one-line
///   change if the art direction ever wants the chunkier look.
pub const PIXEL_SCALE: u32 = 1;

/// Render layer the canvas blit lives on, so the grass camera cannot see it.
pub const CANVAS_LAYER: usize = 1;

/// Marks the camera that draws grass into the canvas.
#[derive(Component)]
pub struct GrassCamera;

/// Marks the camera that puts the canvas on the window.
#[derive(Component)]
pub struct CanvasCamera;

/// Marks the sprite the canvas is drawn with.
#[derive(Component)]
pub struct CanvasSprite;

/// The low-resolution image the grass is drawn into.
#[derive(Resource, Clone, Debug)]
pub struct PixelCanvas {
    /// The canvas image.
    pub image: Handle<Image>,
    /// Canvas size in canvas pixels.
    pub size: UVec2,
    /// Screen pixels per canvas pixel. Always a whole number.
    pub scale: u32,
    /// Canvas pixels per world unit, for the vertex shader's snapping.
    ///
    /// Stale for exactly one frame after a resize, which is harmless: the worst
    /// case is one frame of blades sized for the previous canvas.
    pub pixels_per_unit: f32,
}

impl PixelCanvas {
    /// Window position to canvas pixel.
    ///
    /// Needed because the grass camera's viewport is the canvas, not the
    /// window, so a raw cursor position picks the wrong place by the scale
    /// factor.
    pub fn window_to_canvas(&self, cursor: Vec2, window: Vec2) -> Vec2 {
        if window.x <= 0.0 || window.y <= 0.0 {
            return cursor;
        }
        cursor * Vec2::new(self.size.x as f32, self.size.y as f32) / window
    }
}

/// Sets up the canvas and keeps it matched to the window.
pub struct PixelCanvasPlugin;

impl Plugin for PixelCanvasPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_canvas).add_systems(
            Update,
            (resize_canvas, track_scale)
                .chain()
                .before(crate::GrassSet::Upload),
        );
    }
}

/// Canvas scale and size for a window of this many physical pixels.
pub fn canvas_geometry(window: UVec2) -> (u32, UVec2) {
    let scale = PIXEL_SCALE.max(1);
    let size = UVec2::new(
        window.x.div_ceil(scale).max(1),
        window.y.div_ceil(scale).max(1),
    );
    (scale, size)
}

fn canvas_image(size: UVec2) -> Image {
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    // The whole point. A linear sampler here would blur the canvas back into
    // the smooth image the canvas exists to avoid.
    image.sampler = ImageSampler::nearest();
    image
}

fn window_pixels(window: &Window) -> UVec2 {
    let size = window.resolution.physical_size();
    UVec2::new(size.x.max(1), size.y.max(1))
}

fn setup_canvas(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
) {
    let pixels = windows
        .iter()
        .next()
        .map_or(UVec2::new(1280, 720), window_pixels);
    let (scale, size) = canvas_geometry(pixels);
    let image = images.add(canvas_image(size));

    commands.insert_resource(PixelCanvas {
        image: image.clone(),
        size,
        scale,
        pixels_per_unit: 1.0,
    });

    commands.spawn((
        CanvasCamera,
        Camera2d,
        Camera {
            order: 0,
            clear_color: ClearColorConfig::Custom(palette::ground()),
            ..default()
        },
        // Physical pixels are the world units of this camera, so the blit is
        // one canvas pixel to exactly `scale` screen pixels whatever the
        // display's own scale factor happens to be.
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: pixels.x as f32,
                height: pixels.y as f32,
            },
            ..OrthographicProjection::default_2d()
        }),
        RenderLayers::layer(CANVAS_LAYER),
        Name::new("canvas camera"),
    ));

    commands.spawn((
        CanvasSprite,
        Sprite {
            image,
            custom_size: Some(Vec2::new((size.x * scale) as f32, (size.y * scale) as f32)),
            ..default()
        },
        RenderLayers::layer(CANVAS_LAYER),
        Name::new("canvas sprite"),
    ));
}

/// Keep the canvas sized to the window, and keep the grass camera pointed at it.
///
/// Also runs on the first frame, which is what actually attaches the target:
/// the grass camera is spawned by whoever is composing the scene, and there is
/// no ordering guarantee against `setup_canvas`.
fn resize_canvas(
    mut canvas: ResMut<PixelCanvas>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
    mut sprites: Query<&mut Sprite, With<CanvasSprite>>,
    mut canvas_cameras: Query<&mut Projection, With<CanvasCamera>>,
    mut grass_cameras: Query<&mut RenderTarget, With<GrassCamera>>,
) {
    let Some(pixels) = windows.iter().next().map(window_pixels) else {
        return;
    };
    let (scale, size) = canvas_geometry(pixels);

    if size != canvas.size || scale != canvas.scale {
        canvas.image = images.add(canvas_image(size));
        canvas.size = size;
        canvas.scale = scale;

        for mut sprite in &mut sprites {
            sprite.image = canvas.image.clone();
            sprite.custom_size = Some(Vec2::new((size.x * scale) as f32, (size.y * scale) as f32));
        }
        for mut projection in &mut canvas_cameras {
            *projection = Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed {
                    width: pixels.x as f32,
                    height: pixels.y as f32,
                },
                ..OrthographicProjection::default_2d()
            });
        }
    }

    for mut target in &mut grass_cameras {
        let matches = target
            .as_image()
            .is_some_and(|handle| *handle == canvas.image);
        if !matches {
            *target = RenderTarget::Image(ImageRenderTarget {
                handle: canvas.image.clone(),
                scale_factor: 1.0,
            });
        }
    }
}

/// Work out how big a pixel is in world units, and pin the camera to that grid.
///
/// Snapping the camera matters as much as snapping the blades. Blades are
/// snapped to the *world* pixel grid, so if the camera sits half a pixel off
/// that grid every blade lands half a pixel off too — and the whole field
/// shimmers as the camera drifts, which is the classic tell of pixel art
/// rendered by a 3D engine.
fn track_scale(
    mut canvas: ResMut<PixelCanvas>,
    mut cameras: Query<(&Projection, &mut Transform), With<GrassCamera>>,
) {
    let Some((projection, mut transform)) = cameras.iter_mut().next() else {
        return;
    };
    let Projection::Orthographic(orthographic) = projection else {
        return;
    };
    let height = orthographic.area.height();
    if height <= f32::EPSILON {
        return;
    }

    let pixels_per_unit = canvas.size.y as f32 / height;
    canvas.pixels_per_unit = pixels_per_unit;

    let snapped = (transform.translation.truncate() * pixels_per_unit).round() / pixels_per_unit;
    transform.translation.x = snapped.x;
    transform.translation.y = snapped.y;
}

/// A camera that draws grass into the canvas.
///
/// `view_height` is world units of visible height, so it reads against blade
/// length directly: at eighteen units a tall blade is a fourteen-pixel stroke.
pub fn grass_camera(view_height: f32) -> impl Bundle {
    (
        GrassCamera,
        Camera2d,
        Camera {
            // Behind the blit, which draws to the window itself.
            order: -1,
            clear_color: ClearColorConfig::Custom(palette::ground()),
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: view_height,
            },
            ..OrthographicProjection::default_2d()
        }),
        // See the module docs: both of these would smuggle colours into the
        // frame that are not in the palette.
        Msaa::Off,
        Tonemapping::None,
        Name::new("grass camera"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_common_window_gets_a_whole_scale_and_no_letterbox() {
        let (scale, size) = canvas_geometry(UVec2::new(1920, 1080));
        assert_eq!(scale, PIXEL_SCALE);
        assert_eq!(size * scale, UVec2::new(1920, 1080));
    }

    #[test]
    fn the_canvas_always_covers_the_window() {
        // Overscan is fine — a black bar is not. If the canvas ever came out
        // smaller than the window there would be an unpainted edge.
        for window in [
            UVec2::new(1920, 1080),
            UVec2::new(2560, 1440),
            UVec2::new(3840, 2160),
            UVec2::new(1366, 768),
            UVec2::new(1280, 800),
            UVec2::new(801, 457),
            UVec2::new(640, 269),
            UVec2::new(1, 1),
        ] {
            let (scale, size) = canvas_geometry(window);
            assert!(scale >= 1, "{window:?}");
            assert!(
                size.x * scale >= window.x,
                "{window:?} -> {size:?} x{scale}"
            );
            assert!(
                size.y * scale >= window.y,
                "{window:?} -> {size:?} x{scale}"
            );
            // And never by more than one canvas pixel, or the overscan starts
            // hiding real content.
            assert!(size.x * scale < window.x + scale, "{window:?}");
            assert!(size.y * scale < window.y + scale, "{window:?}");
        }
    }

    #[test]
    fn the_canvas_is_the_windows_own_resolution() {
        // The property that keeps this out of retro-de-make territory: no
        // pixel is ever drawn as a visible square. If someone raises
        // PIXEL_SCALE, this is the test that tells them what they changed.
        for height in [720, 800, 1080, 1440, 2160, 1600] {
            let window = UVec2::new(height * 16 / 9, height);
            let (scale, size) = canvas_geometry(window);
            assert_eq!(scale, 1, "the canvas is meant to render at native size");
            assert_eq!(size, window, "{height}");
        }
    }

    #[test]
    fn a_window_smaller_than_the_canvas_still_works() {
        // Never divide by zero into a scale of zero, which would panic on the
        // div_ceil below it.
        let (scale, size) = canvas_geometry(UVec2::new(320, 200));
        assert_eq!(scale, 1);
        assert_eq!(size, UVec2::new(320, 200));
    }

    #[test]
    fn the_canvas_is_a_render_target_the_gpu_can_draw_into() {
        use bevy::render::render_resource::TextureUsages;
        let image = canvas_image(UVec2::new(480, 270));
        let usage = image.texture_descriptor.usage;
        assert!(usage.contains(TextureUsages::RENDER_ATTACHMENT));
        assert!(usage.contains(TextureUsages::TEXTURE_BINDING));
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
    }

    #[test]
    fn the_canvas_is_sampled_without_filtering() {
        // A linear sampler here is the single change that would silently undo
        // the entire pixel-art pipeline while still looking "fine".
        use bevy::image::ImageFilterMode;
        let image = canvas_image(UVec2::new(64, 64));
        let ImageSampler::Descriptor(descriptor) = image.sampler else {
            panic!("the canvas must carry its own sampler, not the global default");
        };
        assert_eq!(descriptor.mag_filter, ImageFilterMode::Nearest);
        assert_eq!(descriptor.min_filter, ImageFilterMode::Nearest);
    }

    #[test]
    fn cursor_positions_map_into_canvas_pixels() {
        let canvas = PixelCanvas {
            image: Handle::default(),
            size: UVec2::new(480, 270),
            scale: 4,
            pixels_per_unit: 15.0,
        };
        let window = Vec2::new(1920.0, 1080.0);
        assert_eq!(canvas.window_to_canvas(Vec2::ZERO, window), Vec2::ZERO);
        assert_eq!(
            canvas.window_to_canvas(window, window),
            Vec2::new(480.0, 270.0)
        );
        assert_eq!(
            canvas.window_to_canvas(window * 0.5, window),
            Vec2::new(240.0, 135.0)
        );
    }

    #[test]
    fn a_degenerate_window_does_not_divide_by_zero() {
        let canvas = PixelCanvas {
            image: Handle::default(),
            size: UVec2::new(480, 270),
            scale: 4,
            pixels_per_unit: 15.0,
        };
        let cursor = Vec2::new(3.0, 4.0);
        assert_eq!(canvas.window_to_canvas(cursor, Vec2::ZERO), cursor);
    }
}

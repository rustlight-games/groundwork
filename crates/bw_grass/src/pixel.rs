//! The pixel canvas.
//!
//! Grass does not draw to the window. It draws to a small image — 960×540,
//! see [`CANVAS_HEIGHT`] — which is then blitted to the window at a whole-number
//! scale with nearest sampling. Everything about the pixel-art look follows from
//! that one decision, and none of it can be faked afterwards:
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
//! The canvas height is fixed and the *scale* varies with the display — the
//! opposite of the obvious arrangement, and [`CANVAS_HEIGHT`] explains why. The
//! scale stays a whole number and the canvas overscans rather than
//! letterboxing: a fractional scale draws some pixels two screen pixels wide
//! and their neighbours three, which is far more visible than it sounds.

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::palette;

/// Rows of canvas the grass is drawn into, whatever the display is.
///
/// **540.** With the 16:9 framing that is a 960×540 canvas, blitted up to fill
/// the window: two screen pixels per canvas pixel on a 1080p display, four on
/// the 2160-row backing store a retina window of the same size actually has.
///
/// Fixing the *canvas* rather than the *scale* is the whole trick, and it is
/// worth being explicit about why, because the obvious version of this constant
/// is a multiplier and the obvious version is wrong. A multiplier divides the
/// display, so the same "2×" gives 960×540 on one monitor and 1920×1080 on
/// another — the art is a different size on every machine, which for a pixel
/// style is the one thing that must never happen. A fixed canvas inverts it:
/// the picture is 960×540, and the *scale* is whatever whole number the display
/// can fit. 1080p and its retina equivalent both land on it exactly.
///
/// The promise is a bound rather than an equality, because integer scaling
/// cannot give more without letterboxing: a 1440-row display divides by two, not
/// by two and two thirds, so it draws 720 rows. The canvas is therefore always
/// in `[540, 1080)` — never finer than the design size, never as much as twice
/// it — which is the whole of what a whole-number scale can promise.
///
/// The canvas used to be the window's own resolution, on the argument that the
/// reference art is Warcraft III — hand-painted, viewed from far enough away
/// that no pixel is ever a visible square — and that chunky pixels would read
/// as a retro de-make. That was a claim about style made without measuring what
/// the style costs. At 540 rows:
///
/// - A clump is about 7–16 pixels tall rather than 15–32, so its baked interior
///   detail thins out and what carries the field is silhouette and tone — which
///   is what the tonal patches were built to do.
/// - It is a quarter of the fragments of a 1080p canvas. Nothing else available
///   to this renderer comes close; see `grass.overdraw` in the benchmark table.
///
/// 360 rows was tried first and is a third of the fragments again, but at that
/// size a clump is five pixels and the silhouette stops being readable as a
/// plant. 540 is where the chunkiness still reads as deliberate.
///
/// Everything the canvas already did still holds: multisampling and tonemapping
/// stay off, so every pixel is exactly on palette, and
/// [`PixelCanvas::pixels_per_unit`] still tells the vertex shader how big a
/// pixel is.
pub const CANVAS_HEIGHT: u32 = 540;

/// Runtime override for [`CANVAS_HEIGHT`].
///
/// Exists so 1080, 540, and 360 rows can be compared in one sitting. Insert it
/// before startup and the canvas is built at that height instead.
#[derive(Resource, Clone, Copy, Debug)]
pub struct PixelStyle {
    /// Rows of canvas. Clamped to at least one.
    pub canvas_height: u32,
}

impl Default for PixelStyle {
    fn default() -> Self {
        Self {
            canvas_height: CANVAS_HEIGHT,
        }
    }
}

impl PixelStyle {
    /// Read `BW_CANVAS_HEIGHT`, falling back to the compiled-in default.
    ///
    /// For the sandbox and for capture scripts. The game does not consult the
    /// environment — what ships is what [`CANVAS_HEIGHT`] says.
    pub fn from_env() -> Self {
        Self::parse(std::env::var("BW_CANVAS_HEIGHT").ok().as_deref())
    }

    /// The parsing half of [`from_env`](Self::from_env), without the world.
    ///
    /// Split out because the crate forbids `unsafe` and setting an environment
    /// variable now needs it — so the interesting behaviour is tested here and
    /// the read above is the one line that cannot be.
    pub fn parse(value: Option<&str>) -> Self {
        let canvas_height = value
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|height| *height >= 1)
            .unwrap_or(CANVAS_HEIGHT);
        Self { canvas_height }
    }
}

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
        app.init_resource::<PixelStyle>()
            .add_systems(Startup, setup_canvas)
            .add_systems(
                Update,
                (resize_canvas, track_scale)
                    .chain()
                    .before(crate::GrassSet::Upload),
            );
    }
}

/// Canvas scale and size for a window of this many physical pixels.
///
/// At the shipped [`CANVAS_HEIGHT`]. The benchmarks call this, so what they
/// measure is the resolution the game runs at.
pub fn canvas_geometry(window: UVec2) -> (u32, UVec2) {
    canvas_geometry_at(window, CANVAS_HEIGHT)
}

/// The same, for a canvas height chosen by the caller.
///
/// The scale is the largest whole number of screen pixels that still leaves at
/// least `canvas_height` rows to draw into. Whole, because a fractional scale
/// draws some pixels two screen pixels wide and their neighbours three, which
/// is far more visible than it sounds; and *at least*, because the canvas
/// overscans rather than letterboxing.
pub fn canvas_geometry_at(window: UVec2, canvas_height: u32) -> (u32, UVec2) {
    let scale = (window.y / canvas_height.max(1)).max(1);
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
    style: Res<PixelStyle>,
) {
    let pixels = windows
        .iter()
        .next()
        .map_or(UVec2::new(1280, 720), window_pixels);
    let (scale, size) = canvas_geometry_at(pixels, style.canvas_height);
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
    style: Res<PixelStyle>,
) {
    let Some(pixels) = windows.iter().next().map(window_pixels) else {
        return;
    };
    let (scale, size) = canvas_geometry_at(pixels, style.canvas_height);

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
        assert_eq!(scale, 2);
        assert_eq!(size * scale, UVec2::new(1920, 1080));
    }

    #[test]
    fn the_battle_display_is_960_by_540() {
        // The shipped configuration, spelled out: a 1080p window shows a
        // 960x540 canvas at a scale of two. Art direction rather than
        // arithmetic, so a change to it should have to come through here.
        let (scale, size) = canvas_geometry(UVec2::new(1920, 1080));
        assert_eq!(scale, 2);
        assert_eq!(size, UVec2::new(960, 540));
    }

    #[test]
    fn every_display_gets_a_canvas_within_one_doubling() {
        // The property a scale multiplier cannot give and the reason the
        // constant is a height. A retina window of the same size has four times
        // the pixels behind it; the art must not be four times smaller in it.
        //
        // Bounded rather than fixed, and the bound is the honest statement of
        // what integer scaling can promise. An exact multiple of 540 lands on
        // 540 exactly; 1440 does not, and rounding the scale *down* to two — the
        // only choice that still covers the window — gives 720 rows. So the
        // canvas is always in [540, 1080): never smaller than the design size,
        // never as much as twice it. Rounding up instead would letterbox.
        for window in [
            UVec2::new(1920, 1080),
            UVec2::new(3840, 2160),
            UVec2::new(2560, 1440),
            UVec2::new(5120, 2880),
            UVec2::new(1280, 720),
            UVec2::new(3440, 1440),
        ] {
            let (_, size) = canvas_geometry(window);
            assert!(
                size.y >= CANVAS_HEIGHT && size.y < CANVAS_HEIGHT * 2,
                "{window:?} gave {} rows",
                size.y
            );
        }
        // And the two that matter most land exactly on the design size.
        for window in [UVec2::new(1920, 1080), UVec2::new(3840, 2160)] {
            assert_eq!(canvas_geometry(window).1.y, CANVAS_HEIGHT, "{window:?}");
        }
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
    fn the_canvas_costs_a_quarter_of_the_fragments() {
        // The reason the canvas is 540 rows rather than a matter of taste,
        // stated as the ratio the overdraw benchmark moves by. It should not be
        // possible to change the height without this test naming the price.
        let (_, size) = canvas_geometry(UVec2::new(1920, 1080));
        let native = 1920.0 * 1080.0;
        let canvas = (size.x as f64) * (size.y as f64);
        assert!((canvas / native - 0.25).abs() < 1e-6, "{canvas} / {native}");
    }

    #[test]
    fn a_window_smaller_than_the_canvas_still_works() {
        // A window with fewer rows than the design canvas cannot be scaled at
        // all, so it draws at native size rather than letterboxing.
        let (scale, size) = canvas_geometry(UVec2::new(320, 200));
        assert_eq!(scale, 1);
        assert_eq!(size, UVec2::new(320, 200));
    }

    #[test]
    fn a_degenerate_canvas_height_does_not_divide_by_zero() {
        // Zero rows is nonsense, but it must be *harmless* nonsense: the height
        // clamps to one and the arithmetic below it stays finite.
        let (scale, size) = canvas_geometry_at(UVec2::new(320, 200), 0);
        assert!(scale >= 1);
        assert!(size.x >= 1 && size.y >= 1);
        assert!(size.x * scale >= 320 && size.y * scale >= 200);
    }

    #[test]
    fn the_height_can_be_overridden_for_comparison() {
        // 1080, 540 and 360 rows on a 1080p display: scales of one, two, three.
        for (height, expected) in [(1080, 1), (540, 2), (360, 3)] {
            let (scale, size) = canvas_geometry_at(UVec2::new(1920, 1080), height);
            assert_eq!(scale, expected, "{height}");
            assert_eq!(size.y, height, "{height}");
        }
    }

    #[test]
    fn an_unusable_override_falls_back_to_the_shipped_height() {
        // Reading the environment is a dev affordance, and a typo in it must
        // not silently produce a canvas nobody asked for.
        assert_eq!(PixelStyle::parse(None).canvas_height, CANVAS_HEIGHT);
        assert_eq!(
            PixelStyle::parse(Some("not a number")).canvas_height,
            CANVAS_HEIGHT
        );
        assert_eq!(PixelStyle::parse(Some("")).canvas_height, CANVAS_HEIGHT);
        assert_eq!(PixelStyle::parse(Some("0")).canvas_height, CANVAS_HEIGHT);
        assert_eq!(PixelStyle::parse(Some("-2")).canvas_height, CANVAS_HEIGHT);
        assert_eq!(PixelStyle::parse(Some("360")).canvas_height, 360);
        assert_eq!(PixelStyle::parse(Some(" 1080 ")).canvas_height, 1080);
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

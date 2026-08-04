//! Fitting a tile layout into a frame, and saying which world it was.
//!
//! One function, called by every renderer. That is the entire point of the
//! module: the cheap rasteriser and the path tracer have to photograph the same
//! nine tiles from the same camera at the same scale, and the reliable way to
//! arrange that is not to write the arithmetic down twice.
//!
//! ## Nothing here is a renderer's
//!
//! No pages, no plates, no Blender, no supersampling. A resolved frame is a
//! scale, a window, a raster origin and nine polygons — facts about geometry
//! that both renderers then express in their own units. `terrain_bake` turns the
//! origin and scale into a [`crate::scene::SceneRequest`] and a page;
//! `terrain_cycles` turns the same two numbers into a plate request and an
//! orthographic camera.
//!
//! ## The scale is fitted, not chosen
//!
//! A caller says how much of the frame the layout should fill, and the layout's
//! *projected* extent decides the rest. Fitting from the projected corners
//! rather than from a `6S × 3S` formula is what lets a future layout that is not
//! a filled square, or a projection that is not 2:1, frame correctly without
//! this function knowing about either.
//!
//! At the defaults — 1920×1080, four-metre tiles, ninety percent fill — that
//! comes to 72 pixels per metre, a 1728×864 outer diamond and a 576×288 subject.
//!
//! ## Random, and reproducible, are not opposites
//!
//! [`RenderIdentity`] is the two numbers that decide which meadow this is: a
//! seed, and which tile is the subject. Every ordinary invocation gets fresh
//! ones, and every render writes them down beside the picture. A render nobody
//! can reproduce is a render nobody can improve — a picture with a problem in it
//! is worth nothing if the next run is of somewhere else.
//!
//! The centre tile is *derived* from the seed rather than drawn separately, so a
//! single number reproduces the whole frame; a caller that wants a particular
//! tile names it and overrides the derivation.

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::seed::{key_hash, mix, unit_from_bits};

use crate::layout::{IsoTileLayout, LayoutError, TileLayoutPreset, TileRole, WorldTileCoord};
use crate::projection::{Projection, ScenePoint, ScreenPoint, ScreenRect};
use crate::scene::SceneRequest;

/// Which meadow, and which part of it.
///
/// Two numbers, and they are the whole reproduction contract. Written into every
/// manifest and printed as a replay command after every render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderIdentity {
    /// The world seed. Printed as sixteen hex digits, because a seed is copied
    /// between a terminal, a filename and a bug report, and nobody can check
    /// they typed a decimal `u64` correctly.
    pub seed: u64,
    /// Which tile the render is about.
    pub centre_tile: WorldTileCoord,
}

/// How far from the origin a derived centre tile may land, in tiles.
///
/// Deliberately moderate. The legacy grass path still carries a good many
/// positions as `f32`, and at four-metre tiles this is eight kilometres out —
/// far enough that two renders are never the same ground, near enough that a
/// single-precision cache pixel is still exact to well under a pixel.
pub const CENTRE_TILE_SPAN: i64 = 2048;

/// The named streams the centre tile is derived from.
const CENTRE_TILE_U: &str = "centre-tile-u";
const CENTRE_TILE_V: &str = "centre-tile-v";

impl RenderIdentity {
    /// A seed and a tile, both given.
    pub const fn new(seed: u64, centre_tile: WorldTileCoord) -> Self {
        Self { seed, centre_tile }
    }

    /// A seed, with the centre tile derived from it.
    ///
    /// Derived rather than drawn beside it so that one number reproduces the
    /// whole frame. Two named streams, mixed the same way every other addressed
    /// value in this repository is mixed — see `terrain_core::seed`.
    pub fn from_seed(seed: u64) -> Self {
        Self::new(
            seed,
            WorldTileCoord::new(
                derived_axis(seed, CENTRE_TILE_U),
                derived_axis(seed, CENTRE_TILE_V),
            ),
        )
    }

    /// A seed, with the centre tile overridden if the caller named one.
    pub fn resolve(seed: u64, centre_tile: Option<WorldTileCoord>) -> Self {
        match centre_tile {
            Some(coord) => Self::new(seed, coord),
            None => Self::from_seed(seed),
        }
    }

    /// The seed as sixteen hex digits.
    pub fn seed_hex(self) -> String {
        format!("{:016x}", self.seed)
    }
}

/// One axis of a derived centre tile, in `-CENTRE_TILE_SPAN..CENTRE_TILE_SPAN`.
fn derived_axis(seed: u64, stream: &str) -> i64 {
    let bits = mix(mix(seed) ^ key_hash(stream));
    let span = CENTRE_TILE_SPAN as f64;
    // `unit_from_bits` takes the *top* 53 bits, which is the whole reason it
    // exists: the low bits of most integer hashes are the least mixed, and a
    // centre tile drawn from them would cluster.
    (unit_from_bits(bits) * span * 2.0).floor() as i64 - CENTRE_TILE_SPAN
}

/// How to fit a layout into a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IsoFrameOptions {
    pub output_size: [u32; 2],
    /// How much of the frame the layout's projected extent fills, `0..1`.
    ///
    /// Ninety percent by default. Not one: grass rooted in the outer tiles leans
    /// past the diamond's edge, and a layout that filled the frame exactly would
    /// cut those blades off against the border — which reads as a crop rather
    /// than as an edge.
    pub fill: f64,
    /// Where the subject's centre sits in the frame, as a fraction of each axis
    /// from the top-left.
    pub subject_position: [f64; 2],
    /// How far past the visible tiles to generate.
    ///
    /// Shadows fall inward from outside, blades rooted outside lean in, and
    /// every neighbourhood-reading shading term wants ground beyond the edge it
    /// is shading. See [`crate::scene::SceneRequest::halo_m`].
    pub halo_m: f64,
}

impl Default for IsoFrameOptions {
    fn default() -> Self {
        Self {
            output_size: [1920, 1080],
            fill: 0.90,
            subject_position: [0.5, 0.5],
            halo_m: DEFAULT_HALO_M,
        }
    }
}

/// How far past the visible tiles a nine-tile render generates, in metres.
///
/// Half a metre, which is the same number the Cycles trace-tile guard uses and
/// for the same reason: it is the tallest a blade stands plus the ground it
/// shades at the lowest sun this renderer supports. It is a *semantic* halo, not
/// a page margin — the renderers add their own on top for filtering.
pub const DEFAULT_HALO_M: f64 = 0.5;

impl IsoFrameOptions {
    pub fn sized(width: u32, height: u32) -> Self {
        Self {
            output_size: [width, height],
            ..Self::default()
        }
    }

    pub fn with_fill(mut self, fill: f64) -> Self {
        self.fill = fill;
        self
    }
}

/// A tile's outline in the finished raster, in pixels.
///
/// Corners in projection order — top, right, bottom, left — so a consumer can
/// draw the diamond by walking them and closing the loop, without deciding for
/// itself which corner is which.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TilePolygon {
    pub coord: WorldTileCoord,
    pub role: TileRole,
    /// Pixel positions, `+Y` down, in the output raster's own frame.
    pub corners_px: [[f32; 2]; 4],
}

impl TilePolygon {
    /// Whether a pixel centre falls inside the diamond.
    ///
    /// A convex polygon, so the test is "on the same side of every edge". Used
    /// for the subject mask and for per-tile measurement, and shared so those
    /// two cannot disagree about which pixels belong to the middle tile.
    pub fn contains_px(&self, x: f32, y: f32) -> bool {
        let mut sign = 0.0f32;
        for index in 0..4 {
            let a = self.corners_px[index];
            let b = self.corners_px[(index + 1) % 4];
            let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
            if cross == 0.0 {
                continue;
            }
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
        true
    }
}

/// What a layout, a projection and a frame come to.
#[derive(Clone, Debug)]
pub struct ResolvedIsoFrame {
    pub layout: IsoTileLayout,
    pub projection: Projection,
    pub output_size: [u32; 2],
    /// Pixels per world metre the finished picture is shown at.
    pub pixels_per_metre: f32,
    /// The camera's window on the screen plane.
    pub viewport: ScreenRect,
    /// The output's top-left corner, in cache pixels at
    /// [`ResolvedIsoFrame::pixels_per_metre`].
    ///
    /// What the rasteriser's page and the path tracer's plate are both anchored
    /// at, which is what puts the two in register.
    pub cache_origin: [f32; 2],
    /// Every tile's outline in the finished raster, in layout order.
    pub tile_polygons_px: Vec<TilePolygon>,
}

impl ResolvedIsoFrame {
    /// Fit `layout` into `options`.
    pub fn resolve(
        layout: IsoTileLayout,
        projection: Projection,
        options: IsoFrameOptions,
    ) -> Self {
        let output = [options.output_size[0].max(1), options.output_size[1].max(1)];
        let fill = options.fill.clamp(0.01, 1.0);

        // Fitted from the projected corners rather than from a closed form, so a
        // layout that is not a filled square still frames correctly.
        let projected = projection.screen_bounds(layout.visible_bounds());
        let horizontal = output[0] as f64 * fill / projected.width_m().max(f64::MIN_POSITIVE);
        let vertical = output[1] as f64 * fill / projected.height_m().max(f64::MIN_POSITIVE);
        let pixels_per_metre = horizontal.min(vertical).max(f64::MIN_POSITIVE);

        // The subject's centre, placed where the caller asked for it.
        let subject = projection.project(ScenePoint::on_ground(layout.subject_centre()));
        let subject_cache = to_cache(subject, pixels_per_metre);
        let cache_origin = [
            subject_cache[0] - output[0] as f64 * options.subject_position[0],
            subject_cache[1] - output[1] as f64 * options.subject_position[1],
        ];

        let viewport = ScreenRect::new(
            from_cache(cache_origin, pixels_per_metre),
            from_cache(
                [
                    cache_origin[0] + output[0] as f64,
                    cache_origin[1] + output[1] as f64,
                ],
                pixels_per_metre,
            ),
        );

        let tile_polygons_px = layout
            .tiles()
            .iter()
            .map(|tile| {
                let ground = layout.tile_bounds(tile.coord);
                let corner = |u: f64, v: f64| {
                    let screen = projection.project(ScenePoint::new(u, v, 0.0));
                    let cache = to_cache(screen, pixels_per_metre);
                    [
                        (cache[0] - cache_origin[0]) as f32,
                        (cache[1] - cache_origin[1]) as f32,
                    ]
                };
                TilePolygon {
                    coord: tile.coord,
                    role: tile.role,
                    // Top, right, bottom, left — see `TilePolygon`.
                    corners_px: [
                        corner(ground.min.u_m, ground.min.v_m),
                        corner(ground.max.u_m, ground.min.v_m),
                        corner(ground.max.u_m, ground.max.v_m),
                        corner(ground.min.u_m, ground.max.v_m),
                    ],
                }
            })
            .collect();

        Self {
            layout,
            projection,
            output_size: output,
            pixels_per_metre: pixels_per_metre as f32,
            viewport,
            cache_origin: [cache_origin[0] as f32, cache_origin[1] as f32],
            tile_polygons_px,
        }
    }

    /// The subject tile's outline.
    pub fn subject_polygon(&self) -> &TilePolygon {
        self.tile_polygons_px
            .iter()
            .find(|tile| tile.role == TileRole::Subject)
            .expect("a layout has exactly one subject, enforced by its constructor")
    }

    /// The ground the render shows.
    pub fn visible_bounds(&self) -> WorldRect {
        self.layout.visible_bounds()
    }

    /// The scene request this frame asks for.
    pub fn scene_request(&self, halo_m: f64) -> SceneRequest {
        SceneRequest {
            bounds: self.visible_bounds(),
            viewport: self.viewport,
            projection: self.projection,
            output_size: self.output_size,
            pixels_per_metre: self.pixels_per_metre,
            lod: 0,
            halo_m,
        }
    }
}

/// A screen point in cache pixels: image space, `+Y` down.
fn to_cache(screen: ScreenPoint, pixels_per_metre: f64) -> [f64; 2] {
    [
        screen.x_m * pixels_per_metre,
        -screen.y_m * pixels_per_metre,
    ]
}

/// The inverse of [`to_cache`].
fn from_cache(cache: [f64; 2], pixels_per_metre: f64) -> ScreenPoint {
    ScreenPoint::new(cache[0] / pixels_per_metre, -cache[1] / pixels_per_metre)
}

/// Everything one render is: which world, which tiles, which frame.
///
/// Both renderers take this and nothing else, which is what makes "the cheap
/// plate and the traced plate are the same picture" a property of the types
/// rather than a thing to be careful about.
#[derive(Clone, Debug)]
pub struct ResolvedRenderSample {
    pub identity: RenderIdentity,
    pub frame: ResolvedIsoFrame,
    pub scene_request: SceneRequest,
}

impl ResolvedRenderSample {
    pub fn layout(&self) -> &IsoTileLayout {
        &self.frame.layout
    }

    /// The replay command that reproduces this render exactly.
    pub fn replay_command(&self, program: &str, preset: TileLayoutPreset) -> String {
        format!(
            "{program} --layout {} --tile-size-m {} --seed {} --centre-tile={}",
            preset.name(),
            self.frame.layout.tile_side_m(),
            self.identity.seed_hex(),
            self.identity.centre_tile,
        )
    }
}

/// Resolve a whole render: identity, layout, frame and scene request.
///
/// The one function both `terrain preview-export` and `terrain render` call.
/// Anything either of them derives for itself is a way for the two halves of a
/// training pair to stop being the same meadow.
pub fn resolve_render_sample(
    preset: TileLayoutPreset,
    tile_side_m: f64,
    identity: RenderIdentity,
    projection: Projection,
    options: IsoFrameOptions,
) -> Result<ResolvedRenderSample, LayoutError> {
    let layout = preset.layout(identity.centre_tile, tile_side_m)?;
    let frame = ResolvedIsoFrame::resolve(layout, projection, options);
    let scene_request = frame.scene_request(options.halo_m);
    Ok(ResolvedRenderSample {
        identity,
        frame,
        scene_request,
    })
}

/// The ground under a pixel centre, for a resolved frame.
///
/// Shared so the floor pass, the mask and the debug overlay cannot disagree
/// about which patch of world a pixel is looking at.
pub fn ground_under_pixel(frame: &ResolvedIsoFrame, x: f64, y: f64) -> WorldPoint {
    let cache = [
        frame.cache_origin[0] as f64 + x,
        frame.cache_origin[1] as f64 + y,
    ];
    frame
        .projection
        .unproject_ground(from_cache(cache, frame.pixels_per_metre as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> ResolvedIsoFrame {
        let layout = IsoTileLayout::nine(WorldTileCoord::new(-713, 284), 4.0).expect("well formed");
        ResolvedIsoFrame::resolve(layout, Projection::DIMETRIC_2_1, IsoFrameOptions::default())
    }

    fn diamond_size(polygon: &TilePolygon) -> (f32, f32) {
        let xs = polygon.corners_px.map(|c| c[0]);
        let ys = polygon.corners_px.map(|c| c[1]);
        (
            xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - xs.iter().cloned().fold(f32::INFINITY, f32::min),
            ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - ys.iter().cloned().fold(f32::INFINITY, f32::min),
        )
    }

    #[test]
    fn the_default_framing_is_the_one_that_was_worked_out() {
        // 1920x1080, four-metre tiles, ninety percent fill. Every number here
        // was derived by hand before any of it was written, and pinning them is
        // what turns "the framing looks about right" into a check.
        let frame = frame();
        assert!(
            (frame.pixels_per_metre - 72.0).abs() < 1.0e-3,
            "{}",
            frame.pixels_per_metre
        );

        let mut low = [f32::INFINITY; 2];
        let mut high = [f32::NEG_INFINITY; 2];
        for tile in &frame.tile_polygons_px {
            for corner in tile.corners_px {
                low[0] = low[0].min(corner[0]);
                low[1] = low[1].min(corner[1]);
                high[0] = high[0].max(corner[0]);
                high[1] = high[1].max(corner[1]);
            }
        }
        assert!((high[0] - low[0] - 1728.0).abs() < 0.01, "{low:?} {high:?}");
        assert!((high[1] - low[1] - 864.0).abs() < 0.01, "{low:?} {high:?}");
        // Margins, which is the whole reason the fill is not one.
        assert!((low[0] - 96.0).abs() < 0.01, "{low:?}");
        assert!((low[1] - 108.0).abs() < 0.01, "{low:?}");

        let (width, height) = diamond_size(frame.subject_polygon());
        assert!((width - 576.0).abs() < 0.01, "{width}");
        assert!((height - 288.0).abs() < 0.01, "{height}");
    }

    #[test]
    fn the_subject_lands_where_it_was_asked_to() {
        let frame = frame();
        let corners = frame.subject_polygon().corners_px;
        let centre = [
            corners.iter().map(|c| c[0]).sum::<f32>() / 4.0,
            corners.iter().map(|c| c[1]).sum::<f32>() / 4.0,
        ];
        assert!((centre[0] - 960.0).abs() < 0.01, "{centre:?}");
        assert!((centre[1] - 540.0).abs() < 0.01, "{centre:?}");
    }

    #[test]
    fn moving_the_subject_moves_the_picture_and_not_the_scale() {
        // The framing knob that is not a zoom. Composition and scale are
        // separate requests, and a caller that shifted the subject and silently
        // got a different pixel scale could not compare two renders.
        let layout = IsoTileLayout::nine(WorldTileCoord::ORIGIN, 4.0).expect("well formed");
        let centred = ResolvedIsoFrame::resolve(
            layout.clone(),
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::default(),
        );
        let low = ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions {
                subject_position: [0.5, 0.75],
                ..IsoFrameOptions::default()
            },
        );
        assert_eq!(centred.pixels_per_metre, low.pixels_per_metre);
        // Asking for the subject three-quarters of the way down puts it 270
        // pixels lower in the frame, which moves the window *up* over the world
        // by the same amount.
        let shift = low.cache_origin[1] - centred.cache_origin[1];
        assert!((shift + 1080.0 * 0.25).abs() < 0.01, "{shift}");
        let subject_y = low
            .subject_polygon()
            .corners_px
            .iter()
            .map(|c| c[1])
            .sum::<f32>()
            / 4.0;
        assert!((subject_y - 1080.0 * 0.75).abs() < 0.01, "{subject_y}");
    }

    #[test]
    fn the_fitted_scale_is_whichever_axis_runs_out_first() {
        // Twenty-four by twelve metres of screen in a 16:9 frame is limited
        // horizontally. A tall frame is limited the other way, and the fit has
        // to notice rather than assume.
        let layout = IsoTileLayout::nine(WorldTileCoord::ORIGIN, 4.0).expect("well formed");
        let wide = ResolvedIsoFrame::resolve(
            layout.clone(),
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::sized(1920, 1080),
        );
        assert!((wide.pixels_per_metre - 1920.0 * 0.9 / 24.0).abs() < 1.0e-3);

        let tall = ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::sized(1080, 200),
        );
        assert!((tall.pixels_per_metre - 200.0 * 0.9 / 12.0).abs() < 1.0e-3);
    }

    #[test]
    fn a_fuller_frame_is_a_larger_picture_of_the_same_ground() {
        let layout = IsoTileLayout::nine(WorldTileCoord::ORIGIN, 4.0).expect("well formed");
        let loose = ResolvedIsoFrame::resolve(
            layout.clone(),
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::default().with_fill(0.5),
        );
        let tight = ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::default().with_fill(1.0),
        );
        assert!(tight.pixels_per_metre > loose.pixels_per_metre);
        assert_eq!(tight.visible_bounds(), loose.visible_bounds());
    }

    #[test]
    fn the_corners_are_top_right_bottom_left() {
        // Consumers walk them and close the loop rather than deciding which
        // corner is which, so the order is part of the contract.
        let frame = frame();
        let corners = frame.subject_polygon().corners_px;
        let [top, right, bottom, left] = corners;
        assert!(top[1] < left[1] && top[1] < right[1], "{corners:?}");
        assert!(bottom[1] > left[1] && bottom[1] > right[1], "{corners:?}");
        assert!(right[0] > top[0] && right[0] > bottom[0], "{corners:?}");
        assert!(left[0] < top[0] && left[0] < bottom[0], "{corners:?}");
    }

    #[test]
    fn the_nine_diamonds_tile_the_outer_one_without_gaps() {
        // The property the beauty render's silhouette rests on: every pixel
        // inside the outer diamond belongs to exactly one tile.
        let frame = frame();
        let mut checked = 0usize;
        for y in (0..1080).step_by(7) {
            for x in (0..1920).step_by(7) {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let owners = frame
                    .tile_polygons_px
                    .iter()
                    .filter(|tile| tile.contains_px(px, py))
                    .count();
                // Off the diamond entirely is zero; inside is exactly one. Two
                // would mean the tiles overlap, which double-counts a line of
                // ground down every internal edge.
                assert!(owners <= 1, "{owners} tiles claim ({px}, {py})");
                checked += owners;
            }
        }
        assert!(checked > 1000, "the sweep found almost no covered pixels");
    }

    #[test]
    fn the_ground_under_the_subjects_centre_pixel_is_the_subjects_centre() {
        // The registration check. If this drifts, the debug overlay annotates
        // ground it is not over and the subject mask crops the wrong tile.
        let frame = frame();
        let ground = ground_under_pixel(&frame, 960.0, 540.0);
        let wanted = frame.layout.subject_centre();
        assert!(
            (ground.u_m - wanted.u_m).abs() < 1.0e-3,
            "{ground} {wanted}"
        );
        assert!(
            (ground.v_m - wanted.v_m).abs() < 1.0e-3,
            "{ground} {wanted}"
        );
        assert_eq!(frame.layout.tile_at(ground), frame.layout.subject());
    }

    #[test]
    fn the_viewport_and_the_scale_agree_with_the_output_size() {
        let frame = frame();
        let request = frame.scene_request(DEFAULT_HALO_M);
        assert!(
            (request.viewport_pixels_per_metre() - request.pixels_per_metre).abs() < 1.0e-2,
            "{} against {}",
            request.viewport_pixels_per_metre(),
            request.pixels_per_metre
        );
        assert_eq!(request.bounds, frame.visible_bounds());
        assert_eq!(request.halo_m, DEFAULT_HALO_M);
    }

    #[test]
    fn a_seed_derives_a_centre_tile_and_the_same_seed_derives_the_same_one() {
        for seed in [0u64, 1, 0x5a17_e33b_0c9d_2f14, u64::MAX] {
            let identity = RenderIdentity::from_seed(seed);
            assert_eq!(identity, RenderIdentity::from_seed(seed));
            assert!(
                (-CENTRE_TILE_SPAN..CENTRE_TILE_SPAN).contains(&identity.centre_tile.u),
                "{identity:?}"
            );
            assert!(
                (-CENTRE_TILE_SPAN..CENTRE_TILE_SPAN).contains(&identity.centre_tile.v),
                "{identity:?}"
            );
        }
    }

    #[test]
    fn neighbouring_seeds_derive_unrelated_tiles() {
        // The mixer's job, and worth checking here rather than trusting: a
        // derivation that walked the grid as the seed incremented would make
        // consecutive renders neighbours, and a corpus of them would cover a
        // strip of world rather than the world.
        let tiles: Vec<_> = (0..64u64)
            .map(|seed| RenderIdentity::from_seed(seed).centre_tile)
            .collect();
        let mut unique = tiles.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), tiles.len(), "two seeds shared a tile");
        // And no run of three walks in a straight line by one tile.
        for window in tiles.windows(3) {
            let step = |a: WorldTileCoord, b: WorldTileCoord| (b.u - a.u, b.v - a.v);
            assert_ne!(
                step(window[0], window[1]),
                step(window[1], window[2]),
                "{window:?} walks"
            );
        }
    }

    #[test]
    fn the_two_axes_are_drawn_from_different_streams() {
        // Sharing a stream would put every centre tile on the diagonal.
        let diagonal = (0..64u64)
            .map(RenderIdentity::from_seed)
            .filter(|identity| identity.centre_tile.u == identity.centre_tile.v)
            .count();
        assert!(diagonal <= 1, "{diagonal} of 64 centre tiles are on u = v");
    }

    #[test]
    fn a_named_centre_tile_overrides_the_derivation() {
        let named = WorldTileCoord::new(-713, 284);
        assert_eq!(
            RenderIdentity::resolve(7, Some(named)).centre_tile,
            named,
            "the derivation won over an explicit tile"
        );
        assert_eq!(
            RenderIdentity::resolve(7, None),
            RenderIdentity::from_seed(7)
        );
    }

    #[test]
    fn a_replay_command_names_everything_a_repeat_needs() {
        let sample = resolve_render_sample(
            TileLayoutPreset::Nine,
            4.0,
            RenderIdentity::from_seed(0x5a17_e33b_0c9d_2f14),
            Projection::DIMETRIC_2_1,
            IsoFrameOptions::default(),
        )
        .expect("well formed");
        let command = sample.replay_command("terrain render", TileLayoutPreset::Nine);
        assert!(command.contains("--seed 5a17e33b0c9d2f14"), "{command}");
        assert!(command.contains("--layout nine"), "{command}");
        assert!(command.contains("--tile-size-m 4"), "{command}");
        assert!(
            command.contains(&format!("--centre-tile={}", sample.identity.centre_tile)),
            "{command}"
        );
    }

    #[test]
    fn resolving_the_same_sample_twice_gives_the_same_frame() {
        let resolve = || {
            resolve_render_sample(
                TileLayoutPreset::Nine,
                4.0,
                RenderIdentity::from_seed(11),
                Projection::DIMETRIC_2_1,
                IsoFrameOptions::default(),
            )
            .expect("well formed")
        };
        let (a, b) = (resolve(), resolve());
        assert_eq!(a.frame.cache_origin, b.frame.cache_origin);
        assert_eq!(a.frame.pixels_per_metre, b.frame.pixels_per_metre);
        assert_eq!(a.scene_request, b.scene_request);
    }

    #[test]
    fn a_different_seed_frames_different_ground() {
        let sample = |seed| {
            resolve_render_sample(
                TileLayoutPreset::Nine,
                4.0,
                RenderIdentity::from_seed(seed),
                Projection::DIMETRIC_2_1,
                IsoFrameOptions::default(),
            )
            .expect("well formed")
        };
        assert_ne!(
            sample(1).scene_request.bounds,
            sample(2).scene_request.bounds
        );
    }

    #[test]
    fn a_degenerate_frame_does_not_divide_by_nothing() {
        let layout = IsoTileLayout::nine(WorldTileCoord::ORIGIN, 4.0).expect("well formed");
        let frame = ResolvedIsoFrame::resolve(
            layout,
            Projection::DIMETRIC_2_1,
            IsoFrameOptions {
                output_size: [0, 0],
                fill: 0.0,
                ..IsoFrameOptions::default()
            },
        );
        assert!(frame.pixels_per_metre.is_finite() && frame.pixels_per_metre > 0.0);
        assert_eq!(frame.output_size, [1, 1]);
    }
}

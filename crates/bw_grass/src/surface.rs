//! The buffers a page is composited into, and how they resolve to pixels.
//!
//! ## Why this is a depth buffer and not alpha-over
//!
//! Compositing thousands of grass stamps with ordinary alpha-over produces a
//! collage: every stroke sits flatly on top of the one before it, and the field
//! reads as a stack of decals rather than as something with an inside. So each
//! pixel remembers the isometric depth of whatever is currently on top of it,
//! and a stroke arriving later only takes the pixel if it is genuinely in front.
//! Losing fragments carry no visible payload. Canopy shadowing comes from the
//! winning height field, so recording every failed depth test only added a hot
//! read-modify-write that resolve discarded.
//!
//! Depth, not height, is the test. [`crate::iso::depth`] folds "how far down the
//! screen is this rooted" together with "how high does it stand", which is the
//! only ordering that lets a short blade in front cover a tall blade behind
//! while a tall blade still covers its own roots.
//!
//! ## Supersampling
//!
//! The reference art is painted, not pixel art: its strokes have soft two- and
//! three-pixel edges, and hard alpha would throw that away. Compositing at a
//! multiple of the final resolution and box-filtering down gives every stroke
//! edge many levels of coverage, which is enough to read as a brush mark rather
//! than as a polygon.
//!
//! The factor is a property of the surface rather than a constant, because it
//! is now the main thing a [`crate::quality::GrassRenderQuality`] buys. It used
//! to be three everywhere; three is still what the streaming tier uses, and the
//! offline tiers spend four.

use bevy::prelude::*;

use crate::palette::{self, Tone};

/// The supersampling factor the look was tuned at, and what
/// [`crate::quality::GrassRenderQuality::Preview`] still uses.
///
/// Kept as a named constant rather than an inline three because two things have
/// to agree about it — the quality tier and every test that builds a bare
/// [`Surface`] — and a test that quietly used a different factor from the
/// renderer would be measuring a picture nobody bakes.
pub const SUPERSAMPLE: usize = 3;

/// Everything one supersampled pixel remembers, in one place.
///
/// Six parallel arrays would be the natural shape for this and it was the shape
/// it had. The rasteriser is what argues against it: a stroke plots along a rib
/// perpendicular to itself, so it walks one short run of pixels and then jumps a
/// row, and with six arrays that is six cache lines fetched for every run
/// instead of one. Interleaving them made the stroke pass measurably faster for
/// no change to a single pixel of output.
///
/// ## It became a G-buffer
///
/// It used to hold a light index and nothing else about the surface, because
/// shading happened at the rib and only its answer survived. That works for one
/// lighting model and stops working the moment there are several: a cast shadow
/// has to attenuate the *direct* term without touching the ambient one,
/// occlusion has to darken the interior without flattening the form, and
/// transmission has to key on which way the surface faces. None of those are
/// expressible against a number that has already had the form baked into it.
///
/// So the rib now records what the surface *is* and the resolve decides what it
/// looks like. Twenty bytes rather than twelve, which at four-times
/// supersampling on a 256-pixel page is 21 MB — against a scene that is one and
/// a shadow map that is ten.
#[derive(Clone, Copy)]
struct Cell {
    /// Isometric depth of whatever currently owns this pixel.
    depth: f32,
    /// The owning mark's own light index, **before** any lighting.
    ///
    /// Albedo, in the ramp's own units: how bright this mark is as a thing,
    /// not how bright it happens to be lit. The form, the shadow and the
    /// occlusion are all applied at resolve against the normal below.
    light: f32,
    /// World-space surface normal, as signed bytes.
    ///
    /// A byte per axis is about half a degree of angular precision, which is
    /// far finer than anything downstream reads it at — the diffuse term is
    /// wrapped and the glint is a broad lobe. Three bytes rather than an
    /// octahedral pair because the decode is a multiply and the encode is a
    /// round, and this is written far more often than it is read.
    normal: [i8; 3],
    /// Height of the owning mark above the soil, in eighths of a reference
    /// pixel.
    ///
    /// Eighths, and that is a fix rather than a flourish. It was whole pixels in
    /// a byte, which is a sixth of a millimetre of world — invisible at the
    /// scale the game shows and clearly visible as horizontal banding on a
    /// laboratory plate rendered at six times the authoring scale, which is
    /// exactly where this renderer now does its judging.
    top: u16,
    /// Which ramp the owning mark shades through.
    tone: u8,
    /// How far the floor at this pixel has turned to bare earth, `0..255`.
    ///
    /// A blend rather than a choice, and that is the whole point of it being a
    /// separate channel. Switching from the thatch ramp to the soil ramp at a
    /// threshold puts a hard edge around every bare patch — the two ramps differ
    /// in hue, so no light index makes them meet — and a hard-edged patch reads
    /// as a stone lying on the grass rather than as ground showing through it.
    soil: u8,
    /// Root-to-tip position along the owning mark, `0..255`.
    along: u8,
    /// How mature the owning mark is, `0..255`.
    maturity: u8,
    /// How much geometry has passed through this pixel, winner or loser.
    ///
    /// The channel that was deleted and is back for a different reason. It used
    /// to be a count of buried fragments with no consumer, and deleting it was
    /// right — it cost a read-modify-write on the hottest loop in the crate and
    /// resolve threw it away.
    ///
    /// It earns its place now because occlusion needs it. How dark the inside of
    /// a tuft should be is not a question about the one blade that won the pixel;
    /// it is a question about how much leaf is stacked behind that blade, and
    /// this is the only place that is known. Saturating, because past a couple of
    /// dozen layers the answer is "opaque" and the difference stops mattering.
    optical: u8,
    /// Bit 0: the camera is looking at this surface's underside.
    flags: u8,
}

/// The owning mark faces away from the camera — this is its back.
const FLAG_UNDERSIDE: u8 = 1;

/// Sub-divisions of a reference pixel that [`Cell::top`] counts in.
///
/// Eight, which puts the quantisation at an eighth of a reference pixel — under
/// a tenth of a millimetre of world, and finer than the finest plate this
/// renderer produces. It was one whole pixel, and the banding that caused was
/// visible on any laboratory plate rendered above the authoring scale.
const TOP_STEPS: u16 = 8;

/// Pack a world normal into three signed bytes.
#[inline]
fn encode_normal(normal: Vec3) -> [i8; 3] {
    let n = normal.normalize_or(Vec3::Z) * 127.0;
    [
        n.x.round().clamp(-127.0, 127.0) as i8,
        n.y.round().clamp(-127.0, 127.0) as i8,
        n.z.round().clamp(-127.0, 127.0) as i8,
    ]
}

/// One pixel of floor.
#[inline]
fn floor_cell(light: f32, soil: f32, normal: Vec3) -> Cell {
    Cell {
        depth: f32::NEG_INFINITY,
        light,
        normal: encode_normal(normal),
        top: 0,
        tone: Tone::Thatch as u8,
        soil: (soil.clamp(0.0, 1.0) * 255.0) as u8,
        along: 0,
        maturity: 0,
        // Deliberately not reset. The floor is laid before anything grows, and
        // every blade that later passes over this pixel adds to it — so what the
        // counter holds by the end is how much canopy stands between this patch
        // of ground and the camera, which is exactly what the floor wants to know
        // about how dark it should be.
        optical: 0,
        flags: 0,
    }
}

impl Cell {
    /// The world-space normal, decoded.
    #[inline]
    fn normal(&self) -> Vec3 {
        Vec3::new(
            self.normal[0] as f32,
            self.normal[1] as f32,
            self.normal[2] as f32,
        ) * (1.0 / 127.0)
    }
}

/// One rasterised surface point, on its way into the buffer.
///
/// A struct rather than nine arguments, because the rib fills most of it once
/// and varies two fields across the width.
#[derive(Clone, Copy)]
pub struct Fragment {
    pub depth: f32,
    pub light: f32,
    pub normal: Vec3,
    pub tone: Tone,
    /// Height above the soil, in reference pixels.
    pub top: f32,
    /// Root-to-tip position, `0..1`.
    pub along: f32,
    /// How mature the owning mark is, `0..1`.
    pub maturity: f32,
    /// Whether the camera sees this surface's back.
    pub underside: bool,
}

/// The composited state of one page, at supersampled resolution.
pub struct Surface {
    /// Supersampled width in pixels.
    pub width: usize,
    /// Supersampled height in pixels.
    pub height: usize,
    /// Supersampled pixels per final pixel, on each axis.
    supersample: usize,
    cells: Vec<Cell>,
}

impl Surface {
    /// An empty page at the tuned supersampling factor, filled with soil at the
    /// ground plane.
    pub fn new(final_width: usize, final_height: usize) -> Self {
        Self::at_supersample(final_width, final_height, SUPERSAMPLE)
    }

    /// An empty page composited at a chosen supersampling factor.
    pub fn at_supersample(final_width: usize, final_height: usize, supersample: usize) -> Self {
        let supersample = supersample.max(1);
        let width = final_width * supersample;
        let height = final_height * supersample;
        Self {
            width,
            height,
            supersample,
            cells: vec![
                Cell {
                    depth: f32::NEG_INFINITY,
                    light: 0.0,
                    normal: encode_normal(Vec3::Z),
                    top: 0,
                    tone: Tone::Soil as u8,
                    soil: 0,
                    along: 0,
                    maturity: 0,
                    optical: 0,
                    flags: 0,
                };
                width * height
            ],
        }
    }

    /// Supersampled pixels per final pixel.
    #[inline]
    pub fn supersample(&self) -> usize {
        self.supersample
    }

    /// Offer a pixel to the surface, taking it only if it is in front.
    ///
    /// The optical-depth counter rises **whether or not the fragment wins**, and
    /// that asymmetry is the point of it: a blade hidden behind three others
    /// contributes nothing to what the pixel looks like and everything to how
    /// dark the inside of that tuft should be.
    ///
    /// The index is not bounds-checked against a slice a second time — the
    /// caller has already clamped it — but it is still a safe indexing
    /// operation, so a mistake panics rather than corrupting the page.
    #[inline]
    pub fn write(&mut self, index: usize, fragment: Fragment) {
        let cell = &mut self.cells[index];
        cell.optical = cell.optical.saturating_add(1);
        if fragment.depth >= cell.depth {
            cell.depth = fragment.depth;
            cell.light = fragment.light;
            cell.normal = encode_normal(fragment.normal);
            cell.tone = fragment.tone as u8;
            cell.top = (fragment.top.clamp(0.0, 8_000.0) * TOP_STEPS as f32) as u16;
            cell.along = (fragment.along.clamp(0.0, 1.0) * 255.0) as u8;
            cell.maturity = (fragment.maturity.clamp(0.0, 1.0) * 255.0) as u8;
            cell.flags = if fragment.underside {
                FLAG_UNDERSIDE
            } else {
                0
            };
            // A blade covering bare earth is a blade, not earth.
            cell.soil = 0;
        }
    }

    /// Fill every pixel unconditionally — the floor pass, and nothing else.
    ///
    /// `soil` is how far this patch of floor has turned to bare earth: nought is
    /// the dark mat under a thick canopy, one is exposed ground. `normal` is the
    /// ground's own, so the floor is lit by the terrain it belongs to rather
    /// than being a flat plane under everything.
    #[inline]
    pub fn lay(&mut self, index: usize, light: f32, soil: f32, normal: Vec3) {
        self.cells[index] = floor_cell(light, soil, normal);
    }

    /// Lay a whole run of floor pixels that share a colour.
    ///
    /// The floor pass fills a supersampled block per final pixel, so its inner
    /// loop is several identical writes to consecutive addresses. Handing the run
    /// over whole lets it be one bounds check and one straight-line store instead
    /// of one of each per pixel.
    #[inline]
    pub fn lay_run(&mut self, index: usize, count: usize, light: f32, soil: f32, normal: Vec3) {
        self.cells[index..index + count].fill(floor_cell(light, soil, normal));
    }

    /// How far toward bare earth this pixel's floor has gone, `0..1`.
    #[inline]
    pub fn soil_at(&self, index: usize) -> f32 {
        self.cells[index].soil as f32 / 255.0
    }

    /// The world-space surface normal at a supersampled pixel.
    #[inline]
    pub fn normal_at(&self, index: usize) -> Vec3 {
        self.cells[index].normal()
    }

    /// Root-to-tip position of the owning mark, `0..1`.
    #[inline]
    pub fn along_at(&self, index: usize) -> f32 {
        self.cells[index].along as f32 / 255.0
    }

    /// How mature the owning mark is, `0..1`.
    #[inline]
    pub fn maturity_at(&self, index: usize) -> f32 {
        self.cells[index].maturity as f32 / 255.0
    }

    /// Whether the camera is looking at the owning surface's back.
    #[inline]
    pub fn underside_at(&self, index: usize) -> bool {
        self.cells[index].flags & FLAG_UNDERSIDE != 0
    }

    /// How much geometry passed through this pixel, winner or loser.
    ///
    /// Saturating at 255 in the buffer, handed back as a count.
    #[inline]
    pub fn optical_at(&self, index: usize) -> f32 {
        self.cells[index].optical as f32
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// Canopy height, box-filtered to final resolution.
    pub fn height_map(&self, final_width: usize, final_height: usize) -> Vec<f32> {
        let mut heights = vec![0.0f32; final_width * final_height];
        let step = self.supersample;
        let inverse = 1.0 / (step * step) as f32;
        for y in 0..final_height {
            for x in 0..final_width {
                let mut height = 0u32;
                for sy in 0..step {
                    let row = (y * step + sy) * self.width + x * step;
                    for cell in &self.cells[row..row + step] {
                        height += cell.top as u32;
                    }
                }
                heights[y * final_width + x] = height as f32 * inverse / TOP_STEPS as f32;
            }
        }
        heights
    }

    /// Coverage helper for shape-bound tests. Production pages lay a floor
    /// before strokes, so this intentionally remains test-only.
    #[cfg(test)]
    pub(crate) fn painted_map(&self, final_width: usize, final_height: usize) -> Vec<f32> {
        let mut painted = vec![0.0; final_width * final_height];
        let step = self.supersample;
        let inverse = 1.0 / (step * step) as f32;
        for y in 0..final_height {
            for x in 0..final_width {
                let mut count = 0usize;
                for sy in 0..step {
                    let row = (y * step + sy) * self.width + x * step;
                    count += self.cells[row..row + step]
                        .iter()
                        .filter(|cell| cell.depth.is_finite())
                        .count();
                }
                painted[y * final_width + x] = count as f32 * inverse;
            }
        }
        painted
    }

    /// The stroke light index and tone at a supersampled pixel.
    #[inline]
    pub fn pixel(&self, index: usize) -> (f32, Tone) {
        let cell = &self.cells[index];
        let tone = match cell.tone {
            0 => Tone::Soil,
            1 => Tone::Thatch,
            2 => Tone::Grass,
            3 => Tone::Leaf,
            _ => Tone::Dry,
        };
        (cell.light, tone)
    }

    /// Height above the soil at a supersampled pixel, in reference pixels.
    #[inline]
    pub fn top_at(&self, index: usize) -> f32 {
        self.cells[index].top as f32 / TOP_STEPS as f32
    }

    /// Average colour of the supersampled block behind one final pixel.
    ///
    /// `shade` turns one supersampled pixel into a colour; averaging afterwards
    /// rather than averaging the light index first matters, because two pixels
    /// on different ramps have no meaningful average index — soil at 0.5 and
    /// grass at 0.5 are not the same colour, and blending the indices would
    /// invent a third material that exists nowhere in the palette.
    pub fn resolve_pixel(&self, x: usize, y: usize, mut shade: impl FnMut(usize) -> Vec3) -> Vec3 {
        let step = self.supersample;
        let mut total = Vec3::ZERO;
        for sy in 0..step {
            for sx in 0..step {
                total += shade(self.index(x * step + sx, y * step + sy));
            }
        }
        total / (step * step) as f32
    }
}

/// A separable box blur, run twice, which is close enough to a Gaussian for
/// shading terms and a great deal cheaper.
pub fn blur(source: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 {
        return source.to_vec();
    }
    let mut a = source.to_vec();
    let mut b = vec![0.0; source.len()];
    for _ in 0..2 {
        // Horizontal.
        for y in 0..height {
            let row = y * width;
            let mut sum = 0.0;
            for x in 0..=radius.min(width - 1) {
                sum += a[row + x];
            }
            let mut count = radius.min(width - 1) + 1;
            for x in 0..width {
                b[row + x] = sum / count as f32;
                if x >= radius {
                    sum -= a[row + x - radius];
                    count -= 1;
                }
                if x + radius + 1 < width {
                    sum += a[row + x + radius + 1];
                    count += 1;
                }
            }
        }
        // Vertical.
        for x in 0..width {
            let mut sum = 0.0;
            for y in 0..=radius.min(height - 1) {
                sum += b[y * width + x];
            }
            let mut count = radius.min(height - 1) + 1;
            for y in 0..height {
                a[y * width + x] = sum / count as f32;
                if y >= radius {
                    sum -= b[(y - radius) * width + x];
                    count -= 1;
                }
                if y + radius + 1 < height {
                    sum += b[(y + radius + 1) * width + x];
                    count += 1;
                }
            }
        }
    }
    a
}

/// Area-average a plate down to a target size.
///
/// A box filter over each output pixel's exact footprint, fractional edges
/// included, which is what a correct mip chain converges to. So this shows the
/// **best case** for how the surface minifies — worth stating plainly, because
/// baked pages currently have no mip chain at all, and a GPU point-sampling a
/// page at a third of its size will alias considerably worse than this.
///
/// It matters to the snapshot suite for one reason beyond correctness: it is
/// deterministic and independent of the machine. A snapshot taken through a GPU
/// would differ between two computers by more than most optimisations do, and
/// the comparison would be measuring the driver.
pub fn resample(
    source: &[Vec3],
    width: usize,
    height: usize,
    target_w: usize,
    target_h: usize,
) -> Vec<Vec3> {
    if width == 0 || height == 0 || target_w == 0 || target_h == 0 {
        return vec![Vec3::ZERO; target_w * target_h];
    }
    let sx = width as f32 / target_w as f32;
    let sy = height as f32 / target_h as f32;
    let mut out = vec![Vec3::ZERO; target_w * target_h];

    for y in 0..target_h {
        let (top, bottom) = (y as f32 * sy, (y as f32 + 1.0) * sy);
        for x in 0..target_w {
            let (left, right) = (x as f32 * sx, (x as f32 + 1.0) * sx);
            let mut total = Vec3::ZERO;
            let mut weight = 0.0f32;
            for py in top.floor() as usize..(bottom.ceil() as usize).min(height) {
                // Vertical overlap of this source row with the output pixel.
                let cover_y = (bottom.min(py as f32 + 1.0) - top.max(py as f32)).max(0.0);
                if cover_y <= 0.0 {
                    continue;
                }
                for px in left.floor() as usize..(right.ceil() as usize).min(width) {
                    let cover_x = (right.min(px as f32 + 1.0) - left.max(px as f32)).max(0.0);
                    if cover_x <= 0.0 {
                        continue;
                    }
                    let w = cover_x * cover_y;
                    total += source[py * width + px] * w;
                    weight += w;
                }
            }
            out[y * target_w + x] = if weight > 0.0 {
                total / weight
            } else {
                Vec3::ZERO
            };
        }
    }
    out
}

/// Pack linear colours into sRGB bytes.
pub fn to_rgb8(colours: &[Vec3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colours.len() * 3);
    for colour in colours {
        bytes.extend_from_slice(&palette::to_bytes(*colour));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fragment(depth: f32, light: f32, tone: Tone, top: f32) -> Fragment {
        Fragment {
            depth,
            light,
            normal: Vec3::Z,
            tone,
            top,
            along: 0.5,
            maturity: 0.5,
            underside: false,
        }
    }

    #[test]
    fn a_nearer_stroke_takes_the_pixel() {
        let mut surface = Surface::new(2, 2);
        surface.write(0, fragment(1.0, 0.4, Tone::Grass, 10.0));
        surface.write(0, fragment(2.0, 0.9, Tone::Leaf, 20.0));
        let (light, tone) = surface.pixel(0);
        assert_eq!(tone, Tone::Leaf);
        assert!((light - 0.9).abs() < 1e-6);
        assert_eq!(surface.top_at(0), 20.0);
    }

    #[test]
    fn a_further_stroke_does_not_steal_the_pixel() {
        let mut surface = Surface::new(2, 2);
        surface.write(0, fragment(5.0, 0.9, Tone::Grass, 20.0));
        surface.write(0, fragment(1.0, 0.1, Tone::Thatch, 4.0));
        let (light, tone) = surface.pixel(0);
        assert_eq!(tone, Tone::Grass, "the buried stroke stole the pixel");
        assert!((light - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_buried_fragment_still_counts_toward_optical_depth() {
        // The whole reason the counter came back. A blade hidden behind three
        // others contributes nothing to what the pixel looks like and everything
        // to how dark the inside of that tuft should be.
        let mut surface = Surface::new(2, 2);
        surface.write(0, fragment(5.0, 0.9, Tone::Grass, 20.0));
        for _ in 0..4 {
            surface.write(0, fragment(1.0, 0.1, Tone::Thatch, 4.0));
        }
        assert_eq!(surface.optical_at(0), 5.0);
        // And the visible surface is still the near one.
        assert_eq!(surface.pixel(0).1, Tone::Grass);
    }

    #[test]
    fn a_normal_survives_the_round_trip() {
        let mut surface = Surface::new(2, 2);
        for normal in [
            Vec3::Z,
            Vec3::new(0.6, -0.3, 0.74).normalize(),
            Vec3::new(-0.9, 0.1, 0.42).normalize(),
            -Vec3::Z,
        ] {
            surface.write(
                0,
                Fragment {
                    depth: 10.0,
                    normal,
                    ..fragment(10.0, 0.5, Tone::Grass, 1.0)
                },
            );
            let back = surface.normal_at(0);
            assert!(
                back.normalize().distance(normal) < 0.02,
                "{normal:?} came back as {back:?}"
            );
        }
    }

    #[test]
    fn canopy_height_keeps_more_than_whole_pixels() {
        // The banding fix. Height used to be a byte of whole reference pixels,
        // which is visible as horizontal steps on any plate rendered above the
        // authoring scale — and that is where this renderer now does its judging.
        let mut surface = Surface::new(1, 1);
        let mut seen = Vec::new();
        for step in 0..8 {
            let top = 10.0 + step as f32 * 0.125;
            surface.write(0, fragment(10.0 + step as f32, 0.5, Tone::Grass, top));
            seen.push(surface.top_at(0));
        }
        seen.dedup();
        assert!(
            seen.len() >= 8,
            "eighth-pixel height steps collapsed to {} levels",
            seen.len()
        );
    }

    #[test]
    fn blurring_preserves_the_mean() {
        let source: Vec<f32> = (0..64 * 64)
            .map(|i| ((i * 37) % 101) as f32 / 100.0)
            .collect();
        let mean = source.iter().sum::<f32>() / source.len() as f32;
        let blurred = blur(&source, 64, 64, 3);
        let after = blurred.iter().sum::<f32>() / blurred.len() as f32;
        assert!((mean - after).abs() < 0.02, "{mean} vs {after}");
        // And it actually smoothed: variance must fall.
        let spread = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32
        };
        assert!(spread(&blurred) < spread(&source) * 0.5);
    }

    #[test]
    fn a_zero_radius_blur_is_the_identity() {
        let source = vec![0.25, 0.5, 0.75, 1.0];
        assert_eq!(blur(&source, 2, 2, 0), source);
    }

    #[test]
    fn resampling_preserves_the_mean() {
        // The property the snapshot comparison depends on. A downsample that
        // shifted the average would make every view look like a tone regression
        // at the zoom levels it was worst at.
        let source: Vec<Vec3> = (0..120 * 90)
            .map(|i| Vec3::splat(((i * 41) % 97) as f32 / 96.0))
            .collect();
        let mean = |v: &[Vec3]| v.iter().fold(Vec3::ZERO, |a, b| a + *b) / v.len() as f32;
        let shrunk = resample(&source, 120, 90, 40, 30);
        assert_eq!(shrunk.len(), 40 * 30);
        assert!((mean(&shrunk) - mean(&source)).length() < 0.01);
    }

    #[test]
    fn resampling_to_the_same_size_changes_nothing() {
        let source: Vec<Vec3> = (0..8 * 8).map(|i| Vec3::splat(i as f32 / 64.0)).collect();
        let same = resample(&source, 8, 8, 8, 8);
        for (a, b) in same.iter().zip(&source) {
            assert!((*a - *b).length() < 1.0e-5);
        }
    }

    #[test]
    fn resampling_a_degenerate_plate_does_not_panic() {
        assert_eq!(resample(&[], 0, 0, 4, 4).len(), 16);
        assert!(resample(&[Vec3::ONE], 1, 1, 0, 0).is_empty());
    }
}

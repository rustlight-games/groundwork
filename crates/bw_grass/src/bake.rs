//! Baking one page of ground.
//!
//! A page is a rectangle in *screen* space that has already been through the
//! isometric projection, and the patch of world under it is the parallelogram
//! that maps onto it. Baking in screen space rather than in world diamonds is
//! what makes pages tile the display exactly, with no wasted texels in the
//! corners and no diamond seams to hide.
//!
//! Nothing here consults a neighbour. Every placement decision is a hash of a
//! world coordinate, so two pages that share an edge grow the same grass along
//! it whether or not they were ever baked in the same process — which is the
//! only reason a streamed, generated world can look continuous.
//!
//! ## Order of operations
//!
//! 1. Sample the composition fields on a coarse grid over the page.
//! 2. Lay the floor: soil where the ground is bare, thatch where it is not.
//! 3. Draw the dark mat, then the body, then leaves, then the tall accents.
//! 4. Derive occlusion and the fixed directional shadow from the height the
//!    canopy actually reached.
//! 5. Resolve: assemble one light index per pixel and look it up in a ramp.
//!
//! Steps two to four are the part that cannot be reordered. Occlusion measured
//! before the canopy exists is occlusion of nothing.

use bevy::prelude::*;
use rayon::prelude::*;

use crate::field::{Ground, WorldField};
use crate::iso;
use crate::palette::{self, Tone};
use crate::rng::{Draw, Stream};
use crate::stroke::{Painter, Profile, Stroke};
use crate::surface::{SUPERSAMPLE, Surface, blur};

/// Hermite ramp between two edges.
#[inline]
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A soft shoulder on the light index, above which highlights compress.
///
/// The ramp assumes a light index uniform on `[0, 1]`, and the baker's is not
/// quite: enough terms add near the top that the upper fifth is over-populated,
/// which shows up as a field with too many bright tips rather than as a field
/// that is too bright overall. Compressing above the knee fixes the count
/// without touching the mid-tones or capping the genuine glints, which still
/// arrive with enough index to reach the end of the ramp.
#[inline]
fn shoulder(q: f32) -> f32 {
    const KNEE: f32 = 0.736;
    if q <= KNEE {
        q
    } else {
        KNEE + (q - KNEE) * 0.60
    }
}

/// A rectangle of already-projected screen, in cache pixels.
#[derive(Clone, Copy, Debug)]
pub struct Page {
    /// Cache-pixel position of the top-left corner.
    pub origin: Vec2,
    pub width: usize,
    pub height: usize,
}

impl Page {
    pub const fn new(origin: Vec2, width: usize, height: usize) -> Self {
        Self {
            origin,
            width,
            height,
        }
    }
}

/// Every number the look depends on, in one place.
///
/// Deliberately a plain struct of tunables rather than constants scattered
/// through the baker. Matching a piece of reference art is an iterative,
/// numerical exercise, and it goes very badly when the knobs are spread across
/// six files.
#[derive(Clone, Copy, Debug)]
pub struct BakeParams {
    pub seed: u64,
    /// Direction toward the key light in image space: +X right, +Y **down**,
    /// +Z toward the viewer.
    pub light: Vec3,

    /// Tufts per square metre of ground at full density.
    ///
    /// Blades grow in tufts rather than independently, which is the difference
    /// between grass and fur. A tuft shares a lean, a length and a brightness
    /// with its neighbours, and that shared-ness is most of what the eye reads
    /// as vegetation; scatter the same number of blades uniformly and the field
    /// turns into a doormat.
    pub tufts: f32,
    /// Blades in one tuft.
    pub blades_per_tuft: (usize, usize),
    /// Short dark strokes per square metre, under everything.
    pub thatch: f32,
    /// Broadleaf clusters per square metre.
    pub leaves: f32,

    /// Blade arc length, metres.
    pub blade_length: (f32, f32),
    /// Blade half-width at the root, cache pixels.
    pub blade_width: (f32, f32),
    /// Bend from vertical at the tip, radians.
    pub blade_bend: (f32, f32),

    /// Base light index a blade starts from.
    pub base_light: f32,
    /// A gentle lift toward the tip, spread over the whole blade.
    pub tip_light: f32,
    /// The sharp catch of light on the third of marks that get one.
    pub glint: f32,
    /// Strength of the one-sided lateral shading.
    pub side_light: f32,
    /// Width of the dark under-stroke, cache pixels.
    pub under: f32,

    /// Weight of the mound's lit-face-to-dark-back separation.
    ///
    /// The dominant macro term, and it has to be. A mound is only a mound
    /// because one side of it faces the light and the other falls away; brighten
    /// it by *height* instead and every mound is a uniformly pale blob with no
    /// inside, which reads as a stain on the field rather than as a shape on it.
    /// That is why this is many times [`BakeParams::elevation_light`] rather
    /// than the other way round.
    ///
    /// Restrained all the same. Ground relief is a *rhythm* in this art, not
    /// its subject, and a directional term strong enough to fully describe
    /// every swell turns the field into a bed of cushions — each one crowned,
    /// each one ringed, none of them part of anything larger. Below about a
    /// third the relief stops being announced and starts being felt, which is
    /// where it belongs.
    pub mound_light: f32,
    /// Weight of raw canopy height: taller ground catches a little more light.
    ///
    /// Deliberately small. It exists to keep the hollows from reading as level
    /// with the tops, and nothing more.
    pub elevation_light: f32,
    /// Extra light on mound crowns, where the bright tips gather.
    pub crown_light: f32,
    /// Small-radius occlusion between overlapping blades.
    pub micro_occlusion: f32,
    /// How much a bunch standing above its neighbours catches, and a gap loses.
    ///
    /// Signed, and that is the whole of the design. The obvious form of this
    /// term measures only the *shortfall* — how far below the surrounding canopy
    /// a pixel sits — and subtracts it as occlusion. Every hollow then darkens
    /// and no crest ever brightens, which draws a dark ring at the foot of each
    /// bright mass and leaves the mass itself flat. A field of bright centres in
    /// dark rings reads as cushions whatever shape the underlying forms are.
    ///
    /// Measured against a blur a third of a metre wide, which is the scale a
    /// bunch of grass is. So it says the one true thing about a bunch: the tips
    /// on top of it are in the light and the ground between it and the next one
    /// is not. Zero mean, so it costs no exposure — it only redistributes.
    ///
    /// And measured *off to one side*, toward the key — see [`RELIEF_REACH`].
    /// Comparing a pixel with the canopy centred on it says only "is this high",
    /// which is a symmetric statement and lights a bunch like a halo. Comparing
    /// it with the canopy a few centimetres sunward says "is this the edge that
    /// faces the light", which puts a bright rim on one side of every bunch and
    /// a dark foot on the other. That pairing is what actually reads as volume:
    /// a diffuse bright patch says a region is pale, a lit edge over a dark base
    /// says a thing is standing up.
    pub canopy_relief: f32,
    /// The fixed directional self-shadow.
    pub shadow: f32,
    /// Light that has passed *through* the canopy rather than reflected off it.
    ///
    /// The single term that separates lit grass from cut-out grass. Without it
    /// the shaded side of every mound is a flat dark shape with a boundary; with
    /// it, the side facing away glows, because a few centimetres of grass is not
    /// opaque and the sun is behind it.
    pub transmission: f32,
    /// Radius, in pixels, that the assembled macro lighting is blurred by.
    ///
    /// Guarantees no hard transition survives into the plate regardless of what
    /// the lighting terms do. Cheap insurance against a whole class of artefact.
    pub light_blur: usize,
    /// Broad regional light drift, at several metres per cycle.
    ///
    /// The reference keeps a third of its luminance variance after a
    /// sixty-four-pixel blur. Mounds alone do not produce that; something has to
    /// vary at a scale larger than any single mound, and this is it.
    ///
    /// Cut hard when the regional fields doubled in size, and the two moves
    /// belong together. Regions have to be large to read as places, but a large
    /// region that varies in *brightness* is a large pale or dim area, and ten
    /// worlds full of those sit much further apart in mean luminance than ten
    /// worlds whose regions vary in hue, density and how tall the grass is. The
    /// suite caught it immediately — the sweep's worst seed nearly doubled while
    /// the plate this was being tuned against improved — and the repair is the
    /// same thing the eye wants: regions that differ in character rather than in
    /// exposure. Brightness is the one axis of regional variation that costs
    /// something and says least.
    pub region: f32,
    /// Per-tuft brightness scatter.
    pub scatter: f32,
    /// How much of the canopy is glazed back into its own local colour.
    ///
    /// The painterly half of the look, and the one thing no amount of stroke
    /// tuning substitutes for. A painter lays down dense grass, glazes most of
    /// it back into a mass, and then puts a handful of crisp accents on top; a
    /// generator that only ever *adds* marks ends up with every blade legible,
    /// which is what makes procedural vegetation read as fur. This pulls the
    /// lower canopy back toward its neighbourhood colour and leaves the highest
    /// marks alone.
    pub glaze: f32,
    /// How far the shadowed regions drift toward emerald.
    ///
    /// The reference does not shade one green up and down. Its shadows are
    /// cooler as well as darker and its lights warmer as well as brighter, and a
    /// field that varies only in value reads as one plastic colour under a lamp.
    pub cool: f32,
    /// How much chroma the *shadows* give up, tapering to none at the tips.
    ///
    /// A deliberate, measured departure from the reference rather than a closer
    /// match to it. The art this ramp was sampled from is a saturated painting
    /// meant to be looked at; this is a battlefield floor that has to sit one
    /// visual level below twenty units in faction colours, and a ground plane
    /// carrying full chroma everywhere competes with them. Draining the low and
    /// mid range and none of the top buys that headroom in the one place it is
    /// free — the difference between a highlight and its surround gets *wider*,
    /// not narrower, because only the surround moved.
    ///
    /// It shows up in the suite as a lower `saturation` row against the
    /// reference, and that row is the one number here that is expected to read
    /// worse. Everything it buys is either invisible to the descriptors or
    /// visible only once there is something standing on the grass.
    pub temper: f32,
    /// How far a region's own hue may wander: olive one way, blue-green the other.
    ///
    /// The regional counterpart of [`BakeParams::cool`], and the difference
    /// between them is what each is keyed to. `cool` keys on depth in the canopy,
    /// so it says "shadow is a different green from light". This keys on nothing
    /// but position, so it says "the grass over there is a different green from
    /// the grass here" — older, drier, damper, whatever the reason. One palette
    /// stretched over an entire field is the most reliable way to make generated
    /// ground look printed, and no amount of value variation substitutes for it,
    /// because value variation is exactly what a single palette already has.
    ///
    /// Both directions matter. Drifting only toward olive drains the field;
    /// drifting only toward blue chills it. Drifting both ways from the measured
    /// ramp leaves the mean where the reference put it.
    pub drift: f32,
    /// How much of a one-pixel tent blur to mix into the finished page.
    ///
    /// The reference is painted, not rendered: even at full resolution its
    /// strokes have soft two-pixel edges, and a plate composited from clean
    /// geometry is measurably sharper than it at every radius. This is the
    /// difference, applied once at the end rather than smeared through the
    /// stroke rasteriser where it would also cost fill rate.
    pub soften: f32,
}

impl Default for BakeParams {
    fn default() -> Self {
        Self {
            seed: 0x5eed_1234,
            // Up and to the left on screen, and well in front of the ground
            // plane. Image space, so +Y is *down*: negative X is leftward and
            // negative Y is up the screen. Every mound in the field is therefore
            // lit on its upper-left face and falls away toward the lower-right,
            // and every mark's under-stroke sits on its lower-right side. One
            // direction, stated once, obeyed everywhere — a field where the
            // macro light and the marks disagree about where the sun is reads as
            // wrong long before anyone can say why.
            light: Vec3::new(-0.42, -0.40, 0.81).normalize(),

            // Half as many tufts as there were, of roughly twice the reach.
            //
            // The pair moves together: ink laid per square metre goes as count
            // times length, so halving one while doubling the other keeps the
            // canopy as closed as it was. What changes is the *scale* the field
            // is organised at. Short marks at high density are a mat — every
            // square inch gets its share, nothing is a plant, and the only
            // structure above a centimetre is whatever the lighting invents.
            // Longer marks in fewer bunches leave gaps between the bunches, and
            // those gaps are where a fifth of a metre of structure comes from:
            // the scale the reference has most of its variance at, and the one
            // this field was flattest at.
            //
            // Down again by a quarter, with the blade count up by a third and
            // the tuft radius up by nearly half, and the arithmetic is the same
            // arithmetic: total ink held while the unit it is organised into
            // grows. Fewer, larger, fuller bunches is the whole of the change,
            // and it is the one the eye asks for as "broad masses of grass
            // breaking into blades, rather than thousands of small tufts
            // assembling into a surface". The two readings differ only in the
            // size of the repeating unit, which is why this is a count and not a
            // shape.
            tufts: 50.0,
            // Blades per tuft rises faster than the tuft count falls, and it has
            // to. Ink per square metre is count times blades and would be held
            // by matching them; *closure* is not ink, it is overlap, and overlap
            // falls with the square of the radius the blades are spread over. A
            // bunch half again as wide with the same blades in it is a looser
            // bunch, and the floor comes through — measured, the exposed-earth
            // share doubled on the first attempt at this while every other
            // number improved. So the count carries the extra.
            blades_per_tuft: (10, 30),
            // And the mat carries the rest. It is the layer that actually roofs
            // a canopy — three hundred and eighty short strokes to a square
            // metre, nearly all of them buried — and it is far cheaper per unit
            // of closure than a blade, because it is drawn to be lost.
            thatch: 395.0,
            leaves: 4.0,

            blade_length: (0.05, 0.40),
            blade_width: (0.42, 1.95),
            // Well off vertical even at the low end. Grass drawn standing up is
            // grass drawn as objects; this art draws it as strokes lying along
            // the ground, and the difference survives being shrunk to gameplay
            // size when almost nothing else does. Pulled back a little from
            // where it was, because a mark twice as long at the same bend lies
            // over twice as far and the bunch stops having a top.
            blade_bend: (0.35, 1.40),

            base_light: 0.556,
            tip_light: 0.42,
            // Up, while the *number* of marks carrying one came down by a third.
            // The two moves are the same instruction: a highlight should be a
            // reward on a chosen tip rather than a property of the surface. Peak
            // brightness was never what made this field read as lime — the
            // reference reaches the same peak — it was how much of the plate was
            // up there with it, and taking area out and putting amplitude back
            // widens the gap between a lit tip and its surround in both
            // directions at once.
            glint: 0.85,
            side_light: 0.118,
            // Barely pulled back, and the restraint is the lesson. The obvious
            // reading of "too visually active" is to take contrast out of the
            // marks, and it is wrong: measured against the art, the contrast at
            // two and four pixels was already right, and cutting it produced a
            // plate that was flatter everywhere and grouped no better. Activity
            // is not the same quantity as contrast. What makes a surface read as
            // busy is contrast that is *evenly distributed*, and the repair for
            // that is at the bunch scale, not this one.
            //
            // Then raised, once the highlight population came down by a third.
            // Local contrast is not one quantity, and the critique that asks for
            // fewer glossy strands and the one that asks for deeper cavities are
            // asking for opposite halves of the same number. Spending on the
            // dark side is the better half here: the plate carries barely two
            // thirds of the reference's share of genuinely dark pixels, the dark
            // it is missing is exactly the narrow separation between one blade
            // and the next, and narrow dark is the kind [`BROAD_DARK`] has no
            // quarrel with.
            under: 0.68,

            // Down by a seventh rather than the third the eye asked for, and the
            // difference is what the structure ladder costs. This term is the
            // main thing organising the plate between a fifth of a metre and a
            // metre; taking a third of it out drops the variance at those radii
            // by a quarter, and a field with no mid-scale organisation reads as
            // carpet, which is a worse failure than reading as cushions. The
            // ridges and the shared flow already did most of the work the
            // critique was asking this number to do.
            // Back up a little, and the reason it is safe to raise now is that
            // what it lights has changed. When this was cut, every mound was a
            // rounded cushion and the term's only possible statement was "bright
            // in the middle, dark round the edge" — the more of it, the more the
            // field read as a bed of them. The mounds are elongated ridges along
            // a shared flow now, shaded from the geometry of the domes rather
            // than from a gradient, and modulated by `statement` so some are
            // described and some merely suggested. Under those conditions the
            // term says the thing the eye has been asking for instead: one
            // light-facing crown, one mid body, one shadow flank, per mass.
            //
            // It is also the only lighting term whose scale is larger than half
            // a metre, which is where the plate is still measurably flat.
            mound_light: 0.42,
            elevation_light: 0.035,
            crown_light: 0.038,
            micro_occlusion: 0.125,
            // Up by half, and now directional. This is where the volume that
            // came out of `mound_light` goes, and it is a better place for it:
            // it describes the bunches the eye actually groups by rather than
            // the metre-scale swells underneath them.
            //
            // Up again, and this is the term the structure ladder was asking
            // for. Its blur radius sets its scale — half a metre — so it is the
            // only lighting term that lands squarely in the band the plate
            // measures thirty percent short at. Every other candidate for that
            // shortfall varies at the scale of a *plate* and would be paid for
            // in world-to-world spread; this one varies at the scale of a bunch
            // and is paid for in nothing.
            canopy_relief: 0.38,
            // The contact under the near side of a bunch, and worth more than
            // its size suggests. It is derived from the canopy heights, so it
            // falls where a mass actually stands above what is in front of it,
            // and it falls *down-screen* because the key is up and to the left.
            // In a fixed isometric view that one-sided darkness at the foot of a
            // clump is most of what separates a canopy seen from above and in
            // front from a pattern seen from directly overhead — and unlike the
            // lighting terms it is narrow, so [`BROAD_DARK`] has no quarrel with
            // it.
            shadow: 0.14,
            transmission: 0.205,
            light_blur: 4,
            region: 0.32,
            // The one term that raises mid-scale organisation without touching
            // a single pixel of high-frequency contrast, because it varies from
            // tuft to tuft and a tuft is a fifth of a metre — exactly the radius
            // the plate measures flattest at. Variation *between* bunches groups
            // the field; variation *within* one only makes it noisy. They cost
            // the same and this is the one worth having.
            scatter: 0.50,
            glaze: 0.128,
            cool: 0.155,
            temper: 0.165,
            drift: 0.52,
            soften: 0.048,
        }
    }
}

/// The composition fields, sampled on a lattice over the page.
///
/// Cheap to build and cheap to read, which is the point: a full [`WorldField`]
/// sample costs twenty-odd mound kernels, and doing that per supersampled pixel
/// would cost more than every blade in the page put together. The fields it
/// holds have nothing above the mound frequency, so a lattice this coarse loses
/// nothing.
pub struct Macro {
    stride: usize,
    width: usize,
    height: usize,
    height_field: Vec<f32>,
    lit: Vec<f32>,
    crown: Vec<f32>,
    tint: Vec<f32>,
    hue: Vec<f32>,
    bare: Vec<f32>,
    resolution: Vec<f32>,
    statement: Vec<f32>,
}

/// How many final pixels between lattice samples.
const MACRO_STRIDE: usize = 6;

impl Macro {
    pub fn build(page: &Page, field: &WorldField) -> Self {
        let width = page.width.div_ceil(MACRO_STRIDE) + 2;
        let height = page.height.div_ceil(MACRO_STRIDE) + 2;
        let mut height_field = vec![0.0; width * height];
        let mut lit = vec![0.0; width * height];
        let mut crown = vec![0.0; width * height];
        let mut tint = vec![0.0; width * height];
        let mut hue = vec![0.0; width * height];
        let mut bare = vec![0.0; width * height];
        let mut resolution = vec![0.0; width * height];
        let mut statement = vec![0.0; width * height];

        for y in 0..height {
            for x in 0..width {
                let cache =
                    page.origin + Vec2::new(x as f32 - 0.5, y as f32 - 0.5) * MACRO_STRIDE as f32;
                let ground = field.sample(iso::from_cache_ground(cache));
                let i = y * width + x;
                height_field[i] = ground.height;
                lit[i] = ground.lit;
                crown[i] = ground.crown;
                tint[i] = ground.tint;
                hue[i] = ground.hue;
                bare[i] = ground.bare;
                resolution[i] = ground.resolution;
                statement[i] = ground.statement;
            }
        }

        Self {
            stride: MACRO_STRIDE,
            width,
            height,
            height_field,
            lit,
            crown,
            tint,
            hue,
            bare,
            resolution,
            statement,
        }
    }

    /// Bilinear read at a final-resolution page pixel.
    fn at(&self, source: &[f32], x: f32, y: f32) -> f32 {
        let u = (x / self.stride as f32 + 0.5).clamp(0.0, (self.width - 1) as f32);
        let v = (y / self.stride as f32 + 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (u as usize, v as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (u - x0 as f32, v - y0 as f32);
        let top = source[y0 * self.width + x0] * (1.0 - fx) + source[y0 * self.width + x1] * fx;
        let bottom = source[y1 * self.width + x0] * (1.0 - fx) + source[y1 * self.width + x1] * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

/// Bake one page and return its final-resolution linear colour.
///
/// Five stages, and they are public individually because that is the only way
/// to find out where a page's time goes. A single opaque number for "a page
/// costs 100 ms" tells an optimiser nothing about which of these to attack, and
/// the answer is not guessable from reading the code — the stroke pass looks
/// like the expensive one and the shading pass is nine times the pixels.
/// `benches/bake.rs` times each of them separately for exactly that reason.
pub fn bake(page: Page, params: &BakeParams) -> Vec<Vec3> {
    let field = WorldField::lit_by(params.seed, params.light);
    let lattice = Macro::build(&page, &field);
    let mut surface = Surface::new(page.width, page.height);

    lay_floor(&mut surface, &page, &field, &lattice);
    plant_strokes(&mut surface, &page, &field, params);
    resolve(&surface, &page, &lattice, params)
}

/// Grow every mark the page holds onto an already-floored surface.
///
/// The stroke pass, wrapped so it can be run — and timed — on its own. The
/// [`Painter`] borrows the surface for the duration and has to be dropped before
/// anything reads it back, which is the only reason this is a function rather
/// than three lines inside [`bake`].
pub fn plant_strokes(surface: &mut Surface, page: &Page, field: &WorldField, params: &BakeParams) {
    let mut painter = Painter::new(surface, page.origin, params.light);
    plant(
        &mut painter,
        &Bed {
            page,
            field,
            params,
        },
    );
}

/// Side of the page the tiled baker splits a region into.
///
/// The same size the streaming renderer uses, deliberately. A benchmark that
/// tiled at some other size would be measuring a page shape that never ships,
/// and page size is not neutral — it sets the ratio of guard band to interior,
/// which is real work that scales with the perimeter rather than the area.
pub const TILE_PIXELS: usize = 256;

/// Bake a region larger than a page as independent pages, in parallel, and
/// stitch them.
///
/// The point is not only speed, though a twenty-megapixel screenful is thirteen
/// seconds on one core and about a second on all of them. It is that a stitched
/// plate shows page seams if there are any, and seams are the one failure of
/// this design that a single-page bake cannot possibly reveal. Every placement
/// decision is a pure function of world coordinates, so the tiles agree along
/// their edges or the design is broken — and this is what asks.
/// A screenful at the widest camera is thirty-seven megapixels, so the tiles are
/// written straight into the finished plate a band at a time rather than
/// collected and stitched afterwards. Collecting them first holds the whole
/// region twice — nearly a gigabyte for that view — for no gain, since the
/// stitch is a memcpy either way.
pub fn bake_grid(region: Page, params: &BakeParams) -> Vec<Vec3> {
    if region.width == 0 || region.height == 0 {
        return Vec::new();
    }
    let across = region.width.div_ceil(TILE_PIXELS);
    let mut plate = vec![Vec3::ZERO; region.width * region.height];

    plate
        .par_chunks_mut(TILE_PIXELS * region.width)
        .enumerate()
        .for_each(|(band, rows)| {
            // The last band is short whenever the region is not a whole number
            // of pages tall, which is the usual case.
            let height = rows.len() / region.width;
            for tx in 0..across {
                let width = TILE_PIXELS.min(region.width - tx * TILE_PIXELS);
                let origin = region.origin
                    + Vec2::new((tx * TILE_PIXELS) as f32, (band * TILE_PIXELS) as f32);
                let tile = bake(Page::new(origin, width, height), params);
                for y in 0..height {
                    let source = y * width;
                    let target = y * region.width + tx * TILE_PIXELS;
                    rows[target..target + width].copy_from_slice(&tile[source..source + width]);
                }
            }
        });
    plate
}

/// The floor under everything: soil where the ground is bare, dark mat where it
/// is not.
///
/// Filling the floor with thatch rather than growing enough short strokes to
/// hide the soil is worth a great deal of time. The gaps between bright blades
/// have to be dark green, not brown, or the field reads as grass scattered on
/// dirt; but they do not have to be *textured* dark green, because almost none
/// of it survives the canopy.
pub fn lay_floor(surface: &mut Surface, page: &Page, field: &WorldField, lattice: &Macro) {
    for y in 0..page.height {
        for x in 0..page.width {
            let cache = page.origin + Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let ground = iso::from_cache_ground(cache);
            let bare = lattice.at(&lattice.bare, x as f32, y as f32);
            let mottle = field.soil_mottle(ground);

            // Two scales of variation on the earth, and a floor that is never
            // flat: bare ground painted one colour reads as a hole in the
            // texture rather than as ground.
            let grain = field.jitter(Stream::Soil, ground * 3.1, 9.0);
            // Opens sooner and saturates earlier than it did. The field grows
            // twice as much bare ground as the finished plate shows, and almost
            // all of the loss is here: a patch that needs four fifths of the
            // field's peak before it is fully earth spends most of its area as
            // slightly-brown thatch, which reads as a stain rather than as an
            // opening. Widening the *placement* to compensate is the wrong
            // repair and was tried — it makes more stains.
            let soil = smoothstep(0.05, 0.64, bare);
            // Kept dark, and kept grainy. Bare ground that is much paler than
            // the canopy turns every blade lying across it into a dark comma on
            // a light field, which is the single loudest way to make a clearing
            // read as a hole with things planted in it.
            let loose = 1.0 - lattice.at(&lattice.resolution, x as f32, y as f32);
            // A dark contact where the grass meets the earth, and only there.
            //
            // Peaks halfway through the transition and is zero at both ends, so
            // it darkens neither the open soil nor the closed canopy — it draws
            // a soft line along the boundary between them. Without it a patch of
            // earth and the grass around it are two tones meeting at a feathered
            // edge, and a feathered edge between two flat tones reads as one
            // painted *over* the other. The dark line is what puts the soil
            // underneath: it is the shadow of the fringe standing on it, which
            // is the only reason ground ever looks like ground rather than like
            // a hole of a different colour.
            let rim = soil * (1.0 - soil) * 4.0;
            // Its mean is lifted well off the bottom of the thatch ramp and its
            // *range* reaches all the way down to it, and the difference between
            // those two statements is the whole of this line. The floor shows
            // between blades everywhere, so a floor that is uniformly dark turns
            // every clump into a shaded volume sitting in a shadowed pit — a
            // perfectly good way to draw a plant and the wrong way to draw this
            // field. But a floor that never goes dark anywhere has no deepest
            // point either, and the reference has one: half a percent of it sits
            // under 0.20 luminance, all of it the gap between one bunch and the
            // next seen edge-on. This field had a seventh of that, because the
            // shallowest thing in the plate was also its darkest.
            //
            // Same mean, twice the spread. Almost nothing reaches the bottom —
            // `mottle` is fractal noise and piles up around its middle — which
            // is exactly the population the measurement is asking for.
            // The grain fades out toward the middle of an opening. Earth that is
            // as agitated at its centre as at its rim reads as dead moss rather
            // than as soil — what makes a surface look like packed ground is
            // that it is *smoother* than the vegetation around it, and the
            // texture belongs at the disturbed edge where the thatch broke up.
            // The middle of an opening lifts clear of its own rim, and the pair
            // is what makes a scuff read as recessed. Three zones, from the
            // outside in: a dark compressed boundary where the thatch broke up,
            // the grain of disturbed earth, then a slightly clearer and warmer
            // centre where nothing has been growing. Only the first two were
            // here, and two of the three zones is a feathered edge between green
            // and brown — which reads as a stain rather than as ground, however
            // irregular the outline.
            let light = 0.228
                + mottle * 0.54
                + grain * 0.36 * soil * (1.0 - soil * 0.55)
                + bare * 0.09
                + loose * 0.10
                - rim * 0.14;

            for sy in 0..SUPERSAMPLE {
                for sx in 0..SUPERSAMPLE {
                    let index = surface.index(x * SUPERSAMPLE + sx, y * SUPERSAMPLE + sy);
                    surface.lay(index, light, soil);
                }
            }
        }
    }
}

/// The world rectangle whose grass can reach this page.
///
/// Wider than the page in every direction, and much wider below it: grass grows
/// up the screen, so a blade rooted off the bottom edge still leans into view,
/// and one rooted off the top edge never does.
fn footprint(page: &Page) -> (Vec2, Vec2) {
    // Generous, and sized against the *arc length* of the longest mark rather
    // than against its height. Most marks in this field lie well over toward the
    // ground, so a stroke rooted off the left edge reaches sideways almost its
    // whole length — far further than the height that a guard band sized for
    // upright grass would allow for. Too small a band here does not look like a
    // clipped blade; it looks like a straight line down the page where one side
    // grew something the other did not.
    // Each of these must independently exceed the reach of the longest mark in
    // that direction, because this rectangle decides which cells are *visited*
    // at all and [`reaches_page`] only narrows it afterwards.
    //
    // Measured the way [`MARGIN`] is, by
    // [`tests::the_placement_rectangle_covers_every_direction_a_mark_reaches`],
    // which sweeps the vocabulary rather than reasoning about it. Each of these
    // sits comfortably above its requirement, which is worth keeping an eye on
    // in the other direction too: widening this rectangle costs bake time on
    // every page in proportion to its area, and an extra thirty pixels all round
    // is fourteen percent of a page.
    //
    // `ABOVE` is a third of `BELOW` because grass grows up the screen, and that
    // asymmetry is an assumption about the mark vocabulary rather than about
    // geometry. It looked like it had just been broken: a minority of each
    // tuft's blades are now deliberately turned down-screen and laid over to
    // make a near-side skirt (see [`DOWN_SCREEN`]), and a tuft rooted above a
    // page and leaning into it whose cell is never *visited* is the nastiest
    // failure this design has — not a shading difference but a stroke that is
    // simply absent, present on one side of a join and missing on the other.
    // The page-join test cannot see it either, since it splits left from right
    // and gives both halves the same top edge.
    //
    // Measured, the assumption survived, for a reason worth keeping: a blade
    // laid over travels down-screen along the ground and *loses height* doing
    // it, and in a dimetric projection those two nearly cancel. Even at the
    // skirt's full extra bend the furthest any mark descends from its own root
    // is about seven pixels. Almost all of what `ABOVE` guards is the tuft
    // radius, not the mark.
    const SIDE: f32 = 122.0;
    const BELOW: f32 = 156.0;
    const ABOVE: f32 = 46.0;
    let corners = [
        Vec2::new(-SIDE, -ABOVE),
        Vec2::new(page.width as f32 + SIDE, -ABOVE),
        Vec2::new(-SIDE, page.height as f32 + BELOW),
        Vec2::new(page.width as f32 + SIDE, page.height as f32 + BELOW),
    ];
    let mut low = Vec2::splat(f32::INFINITY);
    let mut high = Vec2::splat(f32::NEG_INFINITY);
    for corner in corners {
        let ground = iso::from_cache_ground(page.origin + corner);
        low = low.min(ground);
        high = high.max(ground);
    }
    (low, high)
}

/// Grow everything that stands up.
///
/// The mat goes down first. Not for correctness — the depth test would sort it
/// out either way — but because the mat's job is to be *buried*, and a buried
/// stroke contributes occlusion where one that wins its pixel does not.
fn plant(painter: &mut Painter, bed: &Bed) {
    // The mat thickens exactly where the tufts thin out. Loosely described
    // ground is not *empty* ground — it is ground described as a mass instead of
    // as blades — and taking the tufts away without putting the mass in leaves
    // bald floor, which is worse than the carpet it was meant to fix.
    scatter(
        painter,
        bed,
        Stream::Thatch,
        bed.params.thatch,
        // Thinned hard over bare ground, on top of the coverage every pass
        // gets. The mat is the layer that actually closes a clearing: it is
        // short, there are three hundred of them to a square metre, and the
        // tuft pass thinning itself does nothing about them. An opening with a
        // full mat over it is an opening you cannot see.
        |ground| (1.20 - ground.resolution * 0.20) * (1.0 - ground.bare * 0.62),
        |painter, page, draw, root, ground, params| {
            let stroke = mat_stroke(draw, root, ground, params);
            paint(painter, page, stroke);
        },
    );
    scatter(
        painter,
        bed,
        Stream::Blade,
        bed.params.tufts,
        // Wider than it was, now that this field runs mostly at the broad scale
        // rather than the mound scale. Thinning the tufts inside a single mound
        // does read as that patch being out of focus; thinning them across a
        // quarter of the view reads as a quieter passage of the same meadow,
        // and quiet passages are what the detailed ones are measured against.
        |ground| 0.60 + ground.resolution * 0.52,
        grow_tuft,
    );
    scatter(
        painter,
        bed,
        Stream::Leaf,
        bed.params.leaves,
        |ground| (0.35 + ground.resolution * 0.35) * ground.colony,
        leaf_cluster,
    );
}

/// The three things every planting pass needs: where it is, what grows there,
/// and how it looks.
struct Bed<'a> {
    page: &'a Page,
    field: &'a WorldField,
    params: &'a BakeParams,
}

/// Walk a jittered grid over the page's world footprint, placing one thing per
/// cell.
///
/// One per cell, jittered across the whole cell, rather than several per cell.
/// The distinction matters more than it sounds: several points in one cell
/// cluster at the cell's scale, and since the cell grid is axis-aligned in world
/// space, that clustering projects to a diagonal lattice on screen. It is
/// exactly regular enough for the eye to find, and once seen it cannot be
/// unseen. Spacing the grid to the requested density instead gives an even
/// scatter with no rhythm of its own.
fn scatter(
    painter: &mut Painter,
    bed: &Bed,
    stream: Stream,
    per_square_metre: f32,
    weight: impl Fn(&Ground) -> f32,
    mut place: impl FnMut(&mut Painter, &Page, &mut Draw, Vec2, &Ground, &BakeParams),
) {
    let Bed {
        page,
        field,
        params,
    } = *bed;
    let spacing = (1.0 / per_square_metre.max(0.01)).sqrt();
    let (low, high) = footprint(page);
    let (x0, y0) = (
        (low.x / spacing).floor() as i32,
        (low.y / spacing).floor() as i32,
    );
    let (x1, y1) = (
        (high.x / spacing).ceil() as i32,
        (high.y / spacing).ceil() as i32,
    );

    for cell_y in y0..=y1 {
        for cell_x in x0..=x1 {
            let mut draw = Draw::at(params.seed, stream, cell_x, cell_y);
            let root =
                Vec2::new(cell_x as f32 + draw.unit(), cell_y as f32 + draw.unit()) * spacing;
            // Reject before sampling, not after. The placement rectangle is the
            // world bounding box of a projected parallelogram plus a guard band
            // wide enough for the longest mark, so well over half of these cells
            // can never touch the page — and a [`WorldField`] sample costs a
            // hundred mound kernels. Testing the cheap thing first is most of
            // this crate's bake time.
            if !reaches_page(painter, page, root) {
                continue;
            }
            let ground = field.sample(root);
            // Bare ground grows a fringe, not nothing. The fringe is what makes
            // a patch read as a depression rather than as a hole — and it has to
            // be a broad fringe. A patch that goes from full grass to none over
            // a few centimetres has an edge, and edges are what make procedural
            // ground look stamped.
            // Never all the way to nothing, and a fifth rather than a tenth.
            // The reference has no patch of ground with no green on it at all;
            // even its barest scuffs carry shoots and root marks, and that is
            // most of what keeps them reading as ground rather than as bald
            // spots.
            //
            // The distinction that makes this affordable is between *coverage*
            // and *closure*. Doubling the number of marks in an opening does not
            // halve the visible earth, because what is added is short, flat and
            // scattered — it speckles the soil rather than roofing it. A patch
            // with nothing growing in it is a hole in the texture; a patch with
            // shoots coming through it is somewhere the grass is thin, and the
            // second one is what ground looks like.
            let coverage = 1.0 - smoothstep(0.04, 0.88, ground.bare) * 0.80;
            if !draw.chance((ground.density * coverage * weight(&ground)).min(1.0)) {
                continue;
            }
            place(painter, page, &mut draw, root, &ground, params);
        }
    }
}

/// Reject a stroke that cannot possibly touch the page before rasterising it.
///
/// The placement rectangle is the world AABB of a projected parallelogram, so it
/// is a bit over twice the area actually on screen. Testing here rather than
/// per-pixel inside the rasteriser is the difference between wasting half the
/// bake and wasting none of it.
#[inline]
fn paint(painter: &mut Painter, page: &Page, stroke: Stroke) {
    if !reaches_page(painter, page, stroke.root.truncate()) {
        return;
    }
    painter.draw(&stroke);
}

/// Could something rooted here mark this page at all?
///
/// The margin is the longest reach any mark has, and it is generous on purpose:
/// rejecting a stroke that would have touched the page puts a straight line down
/// the join between two pages, which costs far more than drawing a few marks
/// that turn out to be invisible.
#[inline]
fn reaches_page(painter: &Painter, page: &Page, root: Vec2) -> bool {
    let at = painter.to_page(root.extend(0.0)) / SUPERSAMPLE as f32;
    at.x >= -MARGIN
        && at.y >= -MARGIN
        && at.x <= page.width as f32 + MARGIN
        && at.y <= page.height as f32 + MARGIN
}

/// How far outside a page a tuft may sit and still mark it, in cache pixels.
///
/// The longest mark in the field is a `Tangle` at the top of the length range,
/// grown by a vigorous mound and a tall-accent draw: 0.40 m of arc times 1.25
/// times 1.35 times 1.35. Arc length is not reach, though — the mark bends as it
/// grows and its tip curls back — so the honest bound is the furthest a
/// *rasterised* stroke gets from its own root, which
/// [`tests::the_guard_band_covers_the_longest_mark_the_field_can_grow`] measures
/// rather than assumes. Add the tuft radius the mark may be rooted at, and the
/// half-width plus under-stroke of the rib itself.
///
/// Measured, that comes to about 125 pixels. This is 140, and the extra eighth
/// is not slack — it is the room for the next person to lengthen a blade. Too
/// small a value here does not look like a clipped mark; it looks like a
/// straight line down the join between two pages, appears only once two pages
/// are on screen at once, and is invisible in every still that does not happen
/// to contain a join.
///
/// Costs nothing to raise, unlike the rectangle in [`footprint`]: this test only
/// decides how many already-enumerated cells are discarded, so a generous value
/// discards a few less rather than walking any more.
const MARGIN: f32 = 140.0;

/// Per-plant brightness, gathering the terms that vary plant to plant rather
/// than pixel to pixel.
fn plant_light(draw: &mut Draw, ground: &Ground, params: &BakeParams) -> f32 {
    params.base_light
        + draw.normal() * params.scatter
        + ground.crown * 0.05
        // Roots that overhang bare ground darken. Placement alone does not make
        // a patch read as a depression; this does.
        // Lifted, not lowered, over bare ground. A stroke lying across pale
        // earth at canopy brightness reads as a dark comma stuck to the soil;
        // the reference's clearings carry pale shoots, not dark ones.
        + ground.bare * 0.07
}

/// The dark mat: short, hooked, and almost entirely buried.
fn mat_stroke(draw: &mut Draw, root: Vec2, ground: &Ground, params: &BakeParams) -> Stroke {
    Stroke {
        root: root.extend(0.0),
        // Loosely along the flow, and much more loosely than a blade: the mat is
        // tangle, and tangle that all points one way is thatch on a roof. But it
        // is also the layer that shows through everywhere, so leaving it fully
        // isotropic under a directional canopy puts a fine random weave behind
        // every sweep and takes half the direction back out.
        azimuth: ground.flow + draw.signed() * 1.5,
        length: draw.range(0.09, 0.22),
        // Laid over hard. The mat is meant to read as tangle, not as short
        // grass standing to attention.
        bend: draw.range(0.9, 1.9),
        curl: draw.range(0.0, 1.4),
        sway: draw.signed() * 0.8,
        width: draw.range(0.8, 1.6),
        tip_width: 0.28,
        profile: Profile::Tapered,
        // Over bare earth the mat stops being a mat. Thatch is a dark tone for
        // the floor of a thick canopy; laid across pale soil in a clearing it is
        // a scatter of dark commas, which is exactly what a clearing must not
        // look like.
        tone: if ground.resolution < 0.4 || ground.bare > 0.3 {
            Tone::Grass
        } else {
            Tone::Thatch
        },
        base_light: (0.532
            + draw.normal() * 0.16
            + ground.crown * 0.06
            + (1.0 - ground.resolution) * 0.06
            + ground.bare * 0.18)
            .clamp(0.1, 0.9),
        tip_light: 0.14,
        side_light: params.side_light * 0.6,
        under: params.under * 0.5 * (1.0 - ground.bare * 0.85),
        ..default()
    }
}

/// Straight down the screen, as a world azimuth.
///
/// A world step of `(dx, dy)` moves `(dx - dy)` across the screen and `(dx + dy)`
/// halved down it, so the direction that runs straight down the screen with no
/// sideways component at all is the one where `dx == dy` — a quarter turn. It is
/// the only direction in this projection that means anything to the viewer
/// rather than to the world, which is what makes it the right one to lay a skirt
/// along.
const DOWN_SCREEN: f32 = std::f32::consts::FRAC_PI_4;

/// How far from its centre a tuft may root a blade, metres.
///
/// Named rather than written into the draw because both guard-band tests have to
/// add it to the reach they measure, and a copy of it in a test is a copy that
/// goes stale silently — the test then certifies a band as sufficient for a
/// narrower tuft than the one the baker actually grows.
const TUFT_RADIUS: f32 = 0.185;

/// Extra bend a skirt blade is laid over by, at most, radians.
///
/// Only here so the guard-band test can sweep to the same limit the baker
/// reaches. See the skirt in [`grow_tuft`].
const SKIRT_BEND: f32 = 0.75;

/// One tuft: a handful of blades that agree with each other.
///
/// The agreement is the point. Blades in a tuft share a lean, a length scale and
/// a brightness, and differ only within those; that is what makes a clump read
/// as one plant rather than as a coincidence. It is also where the field's
/// middle scale comes from — twenty pixels of structure that neither a single
/// blade nor the mound field can produce.
fn grow_tuft(
    painter: &mut Painter,
    page: &Page,
    draw: &mut Draw,
    centre: Vec2,
    ground: &Ground,
    params: &BakeParams,
) {
    // Height follows the mound. A mound whose blades are the same length as the
    // hollow beside it is not a mound, it is a stain.
    // Grass on the edge of a bare patch is shorter as well as sparser. Count
    // alone leaves full-height blades standing in a thinning fringe, which reads
    // as grass that has been pulled out rather than grass that never grew.
    // Weighted away from the crown and toward the clump fields, for the same
    // reason the density is: blade length that tracks relief closely makes every
    // raised place taller *and* thicker *and* brighter, and three fields saying
    // one thing is how a surface starts reading as its own height map.
    // Widened against the clump field and narrowed against the mound, at the
    // same mean. How tall the grass is in one bunch against the next is the
    // other half of what groups the field at a fifth of a metre — the first half
    // being how bright it is — and unlike brightness it survives being squinted
    // at, because a taller bunch occludes what is behind it.
    // The bare-ground penalty is steeper than the coverage thinning, and the
    // pair is what lets an opening carry twice the marks and still read as open.
    // Shortening a shoot takes area out of the picture as the square; thinning
    // the count takes it out linearly. So the marks in a clearing get more
    // numerous and much smaller at the same time, which is speckle, and the ink
    // on the soil goes *down* rather than up.
    // And the last factor is how loudly this passage speaks at all — see
    // [`Ground::resolution`]. Quiet ground grows *shorter* grass, not merely
    // rounder-shaped grass, and that distinction is the difference between a
    // hierarchy and a change of vocabulary. Two passages drawn with different
    // mark families at the same length and the same contrast carry the same
    // activity per square inch, and activity per square inch is what the eye
    // measures. Length is the cheapest way to move it, because a shorter mark
    // takes area out of the picture as the square.
    //
    // Centred, so the mean length is unchanged and only its spread grows. A
    // multiplier that ran from one downward would quietly shave the whole
    // canopy and would show up in the comparison as an exposure fault rather
    // than as the organisation it is.
    let vigour = ((0.16 + ground.crown * 0.30 + ground.density * 0.80)
        * (1.0 - ground.bare * 0.62)
        * (0.76 + ground.resolution * 0.44))
        .clamp(0.24, 1.45);
    // One tuft in eight stands well clear of its neighbours. Sparse tall accents
    // are what stop the canopy reading as a mown line.
    let mut reach = if draw.chance(0.12) {
        draw.range(1.1, 1.35)
    } else {
        1.0
    };

    // Sparks: a handful of tufts per square metre that are brighter than their
    // surroundings whatever their surroundings say, aimed at the places that
    // would otherwise have nothing.
    //
    // Three separate terms conspire on dim, loosely described ground — fewer
    // marks glint, the glints that happen are weaker, and the glaze is at its
    // strongest — so those regions come out as a soft dark mass with no
    // incident in them at all. Each of the three is right on its own; together
    // they overshoot, and the result reads as a stain on the texture rather than
    // as a shaded part of a meadow. A broad dark area is only wrong while it is
    // *featureless*: put a few lit tufts in it and the same darkness becomes
    // depth, because now there is something at the front for it to be behind.
    //
    // So the rate leans deliberately the other way from everything else here —
    // up where the ground is dim and loosely described, down where the ordinary
    // glint population is already doing the job.
    let spark = draw
        .chance((0.04 + (1.0 - ground.resolution) * 0.045 - ground.tint * 0.03).clamp(0.0, 1.0));
    if spark {
        // Standing a little proud matters as much as being brighter: the glaze
        // is keyed on canopy height, so a mark that does not clear the mass
        // around it gets averaged straight back into the mass it was meant to
        // break up.
        reach = reach.max(draw.range(1.12, 1.3));
    }

    // Along the local flow, loosely. A uniform heading over the whole circle is
    // isotropic, and isotropic grass has no direction for the eye to travel
    // along — so the only structure left at the middle scale is the outline of
    // each clump, which is precisely the round-blob reading. Two thirds of a
    // radian of scatter on top of the tuft's own fan is enough to bias the field
    // without combing it, and one tuft in six ignores the flow entirely.
    let heading = if draw.chance(0.17) {
        draw.range(0.0, std::f32::consts::TAU)
    } else {
        ground.flow + draw.signed() * 0.7
    };
    // How far the blades fan out from the shared lean. A tight tuft reads as a
    // spike, a loose one as a rosette; the reference has both.
    let fan = draw.range(0.25, 2.1);
    // Half again as wide, in step with the tuft count coming down. A bunch is
    // now a fifth of a metre across at the top of the range rather than a
    // seventh, which at the size the ground is displayed is the difference
    // between a mass with an outline and a dot with blades on it.
    let radius = draw.range(0.045, TUFT_RADIUS);
    let shade = plant_light(draw, ground, params) - params.base_light;
    let (fewest, most) = params.blades_per_tuft;
    let blades = fewest + draw.index(most - fewest + 1);
    let leaning = draw.chance(0.35);

    for _ in 0..blades {
        // Square root of a uniform: fills the disc evenly instead of piling up
        // at the centre.
        let angle = draw.range(0.0, std::f32::consts::TAU);
        let offset = Vec2::from_angle(angle) * radius * draw.unit().sqrt();
        // Bare ground grows sideways, and mostly in dabs. Upright sprouts evenly
        // spaced across a clearing are the giveaway that the clearing was cut
        // out of the grass rather than found in it.
        //
        // Weighted toward `Fleck` rather than evenly with `Broad`, because the
        // two do different jobs here. A broad stroke is a mass of colour and a
        // few of them across an opening start roofing it; a fleck is a dab the
        // width of a shoot, and a scatter of them speckles the earth without
        // hiding any of it. Speckle is what an opening in real ground has —
        // seedlings, root crowns, the odd blade coming through — and it is the
        // difference between soil the grass has worn thin and a shape cut out of
        // the canopy.
        let mut stroke = if ground.bare > 0.3 && draw.chance(0.78) {
            let flat = if draw.chance(0.72) {
                Mark::Fleck
            } else {
                Mark::Broad
            };
            flat.shape(draw, params, ground)
        } else {
            Mark::pick(draw, ground.resolution).shape(draw, params, ground)
        };
        stroke.root = (centre + offset).extend(0.0);
        stroke.azimuth = heading + draw.signed() * fan;
        stroke.length *= vigour * reach;
        if leaning {
            stroke.bend += draw.range(0.15, 0.4);
        }
        // A skirt on the near side, and it is the cheapest isometric cue there
        // is.
        //
        // A fixed three-quarter camera has a front and a back, and a tuft whose
        // blades radiate evenly has neither — it is a rosette seen from directly
        // above, which is what makes a field of them read as a top-down carpet
        // however much volume the lighting gives it. What says "in front of"
        // rather than "beside" is one thing lying over another, so a minority of
        // each bunch's blades are turned down-screen and laid well over, where
        // they overhang the ground the eye reads as nearer.
        //
        // A minority, and *within* a tuft rather than across the field. Every
        // bunch keeps its own heading and its own fan; this only decides which
        // few of its blades fall toward the viewer. Applied globally the same
        // idea is a comb, and a combed field is a worse failure than a flat one.
        if draw.chance(0.17) {
            stroke.azimuth = DOWN_SCREEN + draw.signed() * 0.6;
            stroke.bend += draw.range(0.3, SKIRT_BEND);
        }
        // Blades within a tuft differ as much as tufts differ from each other.
        // The reference has bright single blades standing in dim clumps and dim
        // ones in bright clumps, and a tuft whose blades all agree exactly reads
        // as a moulded plastic plant.
        //
        // Narrowed, though, and the tuft-to-tuft scatter left alone. Variation
        // *within* a bunch is the frequency that competes with a unit standing
        // on the grass; variation *between* bunches is the frequency that groups
        // them into something the eye can read at a glance. They cost the same
        // and they are not worth the same.
        // And narrowed further where the ground is quiet. This is the other half
        // of the intensity classes — the first being length — and it is the half
        // that decides whether a passage reads as a *canopy* or as a collection
        // of blades. Blades that differ from each other are individually
        // legible; blades that agree merge into a mass of one colour, which is
        // exactly what "still lush but smoother, lower in local contrast" asks
        // for. Nothing is taken away to get it: the same marks are drawn, they
        // simply stop arguing with their neighbours.
        stroke.base_light =
            (stroke.base_light + shade + draw.normal() * 0.085 * (0.62 + ground.resolution * 0.76))
                .clamp(0.05, 0.95);
        if spark {
            // Applied after the clamp's inputs are gathered rather than folded
            // into `shade`, because a spark has to survive a dim neighbourhood
            // rather than be averaged with it — and it has to catch the light
            // whether or not this particular mark drew a glint.
            stroke.base_light = (stroke.base_light + 0.10).min(0.95);
            stroke.glint = stroke.glint.max(params.glint * draw.range(0.75, 1.15));
            stroke.tip_light *= 1.3;
        }
        paint(painter, page, stroke);
    }
}

/// The shapes a tuft draws from.
///
/// Six families rather than one parameterised arc, because a single curve
/// function is the loudest possible signature. However much a smooth arch is
/// jittered, a field of them resolves into nested arcs and parallel combs, and
/// what the eye then reads is the generator rather than the grass. Weighted so
/// that no family accounts for more than a quarter of the marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    /// Short, near-straight, barely bent. The commonest mark in the reference
    /// and the one that reads least like a drawn curve.
    Dash,
    /// Changes direction abruptly partway along.
    Kink,
    /// A shallow S. Two curvatures, so it never resolves into an arc.
    Sway,
    /// The arch that used to be the whole vocabulary. Now a minority.
    Hook,
    /// A tiny blunt dab. Not a blade at all — the flecks between blades.
    Fleck,
    /// A wide, soft, low-contrast stroke. Reads as a mass rather than as a
    /// blade, and is most of what stops the field looking like bristles.
    Broad,
    /// A long stroke laid almost flat, curling back on itself. Reads sideways
    /// where everything else reads upright, which is the one difference that
    /// survives being shrunk to gameplay size.
    Tangle,
    /// An ordinary blade drawn behind its neighbours, so only pieces survive.
    Buried,
}

impl Mark {
    /// Choose a family, weighted by how finely this ground is being described.
    ///
    /// In a well-described passage the mix leans toward marks that stay legible.
    /// In a loosely described one it leans toward broad strokes, flecks and
    /// buried fragments — the marks that melt together. That shift is the whole
    /// mechanism behind "some passages are blades and some are paint", and it
    /// costs one extra parameter.
    fn pick(draw: &mut Draw, resolution: f32) -> Self {
        let loose = 1.0 - resolution;
        let u = draw.unit();
        // Cumulative, so the weights read in the same order they are declared.
        //
        // Weighted toward the straight and the massed, and away from the curled.
        // A tangle of thin hooked shapes is what moss, clover and low fern cover
        // all look like; grass is a narrow tapered blade with one simple curve
        // in it, and the reading depends far more on the *proportion* of curled
        // marks than on any individual shape being wrong. `Dash` is the mark
        // that reads unmistakably as a blade, `Broad` and `Buried` are the marks
        // that stop being individually legible and become mass, and between them
        // they are now over half the field.
        // Cut again toward the straight and the massed. The curled families —
        // `Sway`, `Hook`, `Tangle` — are now under a sixth of the field between
        // them, down from over a third two rounds ago, because a tangle of thin
        // hooked shapes is what every *other* kind of ground cover looks like.
        // Moss, clover, low fern and seaweed are all read from the same cue, and
        // it is not the individual shape that carries it, it is what fraction of
        // the marks curl. Grass is a straight tapered blade with at most one
        // bend in it, and the reading flips somewhere around a fifth.
        // Cut a third time, and this is where it stops. Between them `Sway`,
        // `Hook` and `Tangle` are now about a ninth of the field, which is the
        // proportion a botanist's eye reads as "grass with some character in it"
        // rather than as fern, moss or clover. Below a twentieth the field
        // starts reading as printed strokes; above a fifth it stops reading as
        // grass. There is no more to win here — the remaining complaint about
        // the shape language is answered by how *long* the marks are and how
        // they group, not by which curve family they belong to.
        let weights = [
            0.335 - loose * 0.115, // Dash
            0.145 - loose * 0.05,  // Kink
            0.055 - loose * 0.02,  // Sway
            0.032 - loose * 0.012, // Hook
            0.10 + loose * 0.05,   // Fleck
            0.165 + loose * 0.085, // Broad
            0.025 + loose * 0.012, // Tangle
            0.143 + loose * 0.06,  // Buried
        ];
        let total: f32 = weights.iter().sum();
        let mut cursor = 0.0;
        for (index, weight) in weights.iter().enumerate() {
            cursor += weight / total;
            if u < cursor {
                return [
                    Mark::Dash,
                    Mark::Kink,
                    Mark::Sway,
                    Mark::Hook,
                    Mark::Fleck,
                    Mark::Broad,
                    Mark::Tangle,
                    Mark::Buried,
                ][index];
            }
        }
        Mark::Buried
    }

    fn shape(self, draw: &mut Draw, params: &BakeParams, ground: &Ground) -> Stroke {
        let (short, tall) = params.blade_length;
        let (thin, thick) = params.blade_width;
        let (low, high) = params.blade_bend;
        // Roughly a seventh of marks catch the light sharply, and fewer where the
        // ground is loosely described. Give every blade a glint and the field
        // turns to wet plastic; give none and it is felt.
        //
        // The *count* came down by a fifth and the strength did not move at all,
        // which is the whole of the adjustment. Peak brightness is not what
        // makes a field read as lime — the reference reaches the same peak this
        // does — it is how much of the surface is up there with it. Bright
        // pixels spread across a third of the plate stop being highlights and
        // become the base colour, and then the actual base has to read as shadow
        // to get any separation at all.
        // Down by another thirty percent, and the strength again did not move.
        //
        // This is the third time the same lever has been pulled and the reason
        // is measured rather than aesthetic: the plate carries a quarter more
        // pixels above the reference's own bright threshold than the reference
        // does, while reaching an almost identical peak. Too much of the surface
        // is up at the top of the ramp, and a highlight population that broad
        // stops being highlights — it becomes the base colour, and then the
        // actual base has to read as shade to get any separation at all. What
        // makes a field look luminous is the *ratio* of lit to unlit, not the
        // brightness of the lit part.
        let lit = 0.046 + ground.resolution * 0.058;
        let glint = if draw.chance(lit) {
            // Well-described ground gets brighter accents as well as more of
            // them, which is what keeps its local contrast up while the loosely
            // described ground beside it is being glazed flat.
            params.glint * draw.range(0.7, 1.4) * (0.65 + ground.resolution * 0.7)
        } else {
            0.0
        };
        // Grass in a clearing lies over rather than standing up. Upright sprouts
        // in a bare patch read as something planted in a hole.
        let lodged = ground.bare * 0.22;
        // Marks in a clearing lose their dark under-stroke. Against dense grass
        // the under-stroke is what separates one blade from the next; against
        // bare earth it is an outline, and an outlined blade lying on soil reads
        // as a decal stuck to the ground.
        let outlined = 1.0 - ground.bare * 0.85;

        // A fifth of the marks recede: dimmer than their neighbours, and with no
        // catch of light at all. The reference is full of grass that is present
        // without being described — subdued stems, dark blades that read as mass
        // rather than as objects — and a field where every mark competes equally
        // for attention is a field with no depth of reading, however varied its
        // shapes are.
        let recessive = if draw.chance(0.20) {
            draw.range(0.09, 0.19)
        } else {
            0.0
        };
        let base = Stroke {
            length: draw.range(short, tall),
            bend: draw.range(low, high) + lodged,
            width: draw.range(thin, thick),
            tip_width: 0.30,
            profile: if draw.chance(0.08) {
                Profile::Stem
            } else {
                Profile::Tapered
            },
            // Straw belongs where the ground is already drifting olive, not
            // sprinkled at a fixed rate across the whole field. A uniform
            // scatter of pale stems says "some blades are dry"; a scatter that
            // thickens through the drier regions says the region is.
            // Straw belongs where the ground is already drifting olive, and in a
            // ring around every opening.
            //
            // The fringe term is the middle stage of the three an edge of bare
            // earth needs: dense grass, then sparse *dry* blades, then open
            // soil. Without it an opening has two stages and they meet at a
            // feathered boundary between green and brown, which reads as one
            // material painted over the other however irregular the outline is.
            // Grass at the lip of a scuff is drier than grass a hand's width
            // back — less root, more sun, whatever wore the patch also wore it —
            // and half a dozen straw-coloured blades around a rim say that
            // faster than any amount of shaping.
            //
            // Keyed to a band rather than to `bare` itself, so it peaks on the
            // rim and falls away to nothing in both directions. Straw in the
            // middle of a clearing is dead grass; straw at its edge is the edge.
            tone: if draw.chance(
                0.004
                    + ground.hue.max(0.0) * 0.055
                    + smoothstep(0.05, 0.30, ground.bare)
                        * (1.0 - smoothstep(0.42, 0.80, ground.bare))
                        * 0.24,
            ) {
                Tone::Dry
            } else {
                Tone::Grass
            },
            base_light: params.base_light - recessive,
            tip_light: params.tip_light * draw.range(0.7, 1.3),
            glint: if recessive > 0.0 { 0.0 } else { glint },
            side_light: params.side_light,
            // The third thing the intensity classes move, after length and
            // blade-to-blade scatter. The under-stroke is what separates one
            // blade from the next, so draining it is precisely "let these merge
            // into a softer canopy" — and it is the term that carries the most
            // local contrast per pixel of anything in the field, which makes it
            // the most effective one to spend on the distinction.
            under: params.under * outlined * (0.77 + ground.resolution * 0.46),
            ..default()
        };

        // The largest marks give silhouette and mass, not attention. Leaving the
        // long wide tail of the distribution as bright and as curved as everything
        // else turns it into a recognisable class of hero object — a sickle that
        // the eye picks out of every clump — which is a different way of being
        // uniform.
        let bulk = ((base.length - short) / (tall - short).max(1.0e-4)
            + (base.width - thin) / (thick - thin).max(1.0e-4))
            * 0.5;
        // A glint belongs on a blade, not on a thread.
        //
        // The brightest marks in this field are also its thinnest, because the
        // two reductions above both key on size and nothing keyed on legibility.
        // A half-pixel stroke at the top of the ramp is not a catch of light on
        // a leaf; it is a specular pinprick, and a scatter of them is what gives
        // a surface the wet, varnished, faintly neon reading that no amount of
        // colour work removes. They are also the pixels most certain to crawl
        // when the camera moves: a feature narrower than a screen pixel cannot
        // be resampled, only sampled, so it flickers on and off as the ground
        // slides under the grid.
        //
        // So the two gates now pull in opposite directions and the highlights
        // land between them — off the threads by this one, off the big soft
        // masses by `bulk`. Same count, better placed.
        // Sized to catch only the marks that are genuinely narrower than a
        // screen pixel, and no wider. The first attempt at this band ran from
        // half a pixel to one and a quarter, which sounds modest and removed
        // nearly a third of the remaining highlight area on top of the thirty
        // percent already taken out by the count — the two gates multiplied, the
        // plate lost a tenth of its contrast at every small radius, and the
        // repair was measured rather than seen. Two reductions aimed at the same
        // population have to be budgeted together.
        let legible = smoothstep(0.45, 0.95, base.width);
        let base = Stroke {
            base_light: base.base_light - bulk * 0.11,
            glint: base.glint * (1.0 - bulk * 0.55) * legible,
            ..base
        };

        match self {
            Mark::Dash => Stroke {
                length: base.length * draw.range(0.55, 0.85),
                bend: draw.range(0.3, 0.9),
                curl: draw.range(0.0, 0.35),
                sway: draw.normal() * 0.2,
                ..base
            },
            Mark::Kink => Stroke {
                bend: draw.range(0.45, 1.05),
                kink: draw.signed() * draw.range(0.5, 1.3),
                kink_at: draw.range(0.35, 0.7),
                kink_turn: draw.signed() * draw.range(0.3, 1.1),
                ..base
            },
            Mark::Sway => Stroke {
                bend: draw.range(0.55, 1.25),
                sway: draw.signed() * draw.range(0.9, 2.0),
                curl: draw.range(0.0, 0.4),
                ..base
            },
            Mark::Hook => Stroke {
                bend: draw.range(0.75, 1.5),
                curl: draw.range(0.9, 2.3),
                ..base
            },
            Mark::Broad => Stroke {
                length: base.length * draw.range(0.42, 0.78),
                bend: draw.range(0.7, 1.6) + lodged,
                curl: draw.range(0.0, 0.7),
                sway: draw.signed() * 0.5,
                // Nearly three times a blade. A "wide" stroke that is only half
                // again as wide vanishes into the blade population the moment
                // the page is viewed at gameplay size, which is the only size
                // that matters.
                width: base.width * draw.range(1.45, 2.1),
                tip_width: 0.6,
                // No glint and only a whisper of tip lift: this family exists to
                // be a mass of colour, and a highlight would make it a leaf.
                glint: 0.0,
                tip_light: base.tip_light * 0.55,
                side_light: params.side_light * 0.5,
                under: params.under * 0.6,
                ..base
            },
            Mark::Fleck => Stroke {
                // Scaled against the blade range rather than fixed, so it stays
                // a fleck when the blades grow. A "small dab" that is a fifth of
                // a metre long is a leaf.
                length: base.length * draw.range(0.18, 0.34),
                bend: draw.range(0.7, 1.7),
                width: base.width * draw.range(1.0, 1.4),
                tip_width: 0.45,
                profile: Profile::Oval,
                glint: glint * 0.4,
                ..base
            },
            Mark::Tangle => Stroke {
                // The longest mark in the field, and the one the page guard band
                // is sized against — see [`reaches_page`]. Raising this without
                // raising that puts a straight line down every page join.
                length: base.length * draw.range(0.9, 1.25),
                // Past a right angle the tip descends, so the stroke lies along
                // the ground and doubles back.
                bend: draw.range(1.3, 2.0),
                curl: draw.range(0.4, 1.4),
                sway: draw.signed() * draw.range(1.2, 2.4),
                width: base.width * draw.range(0.9, 1.35),
                glint: glint * 0.35,
                tip_light: base.tip_light * 0.6,
                ..base
            },
            Mark::Buried => Stroke {
                bend: draw.range(0.3, 1.4),
                curl: draw.range(0.0, 1.6),
                sway: draw.signed() * 0.6,
                // Far enough behind to lose most overlaps, near enough that the
                // fragments still show.
                depth_bias: draw.range(0.02, 0.09),
                glint: 0.0,
                tip_light: base.tip_light * 0.5,
                ..base
            },
        }
    }
}

/// A rosette of small round leaves.
///
/// Sparse on purpose — under a tenth of the marks in the reference. They are
/// punctuation: at the right frequency they say "this is a real meadow", and at
/// twice that they say "this is a clover lawn".
fn leaf_cluster(
    painter: &mut Painter,
    page: &Page,
    draw: &mut Draw,
    root: Vec2,
    ground: &Ground,
    params: &BakeParams,
) {
    let leaves = 3 + draw.index(5);
    let spread = draw.range(0.65, 1.15);
    let start = draw.range(0.0, std::f32::consts::TAU);
    // One cluster in five is a pale rosette. They read at a distance where an
    // ordinary leaf does not, and the reference is dotted with them.
    let pale = if draw.chance(0.22) { 0.18 } else { 0.0 };
    let light = (plant_light(draw, ground, params) + 0.12 + pale).clamp(0.05, 0.95);
    let stem = draw.range(0.02, 0.07);

    for leaf in 0..leaves {
        let angle =
            start + leaf as f32 / leaves as f32 * std::f32::consts::TAU + draw.signed() * 0.35;
        paint(
            painter,
            page,
            Stroke {
                root: (root + Vec2::from_angle(angle) * stem).extend(0.0),
                azimuth: angle,
                length: draw.range(0.065, 0.125) * spread,
                // Leaves lie over much further than blades; that near-horizontal
                // pose is what makes them read as flat rather than as stubs.
                bend: draw.range(1.1, 1.9),
                curl: draw.range(0.0, 0.6),
                sway: draw.signed() * 0.3,
                width: draw.range(1.5, 2.6),
                tip_width: 0.5,
                profile: Profile::Oval,
                tone: Tone::Leaf,
                base_light: light + draw.normal() * 0.05,
                tip_light: 0.10,
                glint: if draw.chance(0.3) {
                    params.glint * 0.5
                } else {
                    0.0
                },
                side_light: params.side_light * 1.4,
                under: params.under * 0.8,
                ..default()
            },
        );
    }
}

/// How far toward the key the canopy-relief comparison is taken, in pixels.
///
/// About a sixth of a metre: small against the half-metre blur it reads from,
/// so the term still says "this bunch against the ground around it" rather than
/// "this bunch against the next one along", and large enough that the sunward
/// and shaded sides of one bunch get visibly different answers. Push it much
/// past a third of the blur radius and the two stop being the same measurement —
/// a crest starts comparing itself to a neighbouring crest, and the whole field
/// picks up a directional smear instead of individually lit bunches.
///
/// Sitting at exactly that third now rather than a comfortable quarter of it,
/// because the asymmetry is the whole product. At a quarter the comparison is
/// three parts "is this high" to one part "does this face the sun", and a term
/// that is mostly the first draws a halo round every bunch — a bright centre in
/// a dark ring, which is the radial cushion reading the field keeps being
/// accused of. Pushing it to the limit of what stays one measurement is what
/// turns the halo into a crown on one flank and a shadow on the other.
const RELIEF_REACH: f32 = 17.0;

/// How much of a broad lighting term's *downward* half survives.
///
/// Light may be broad; dark may not. This is the one rule that decides whether a
/// generated field reads as ground under a sun or as ground that is patchily
/// underexposed, and it is not symmetric — which is why it needs stating rather
/// than falling out of the arithmetic.
///
/// A broad bright area is a place the sun is reaching, and the eye accepts one
/// at any size. A broad *dark* area is not a shadow: shadows have a caster, and
/// nothing in a flat meadow casts one metres across. Read at gameplay size, a
/// soft dark region two or three metres wide reads as a patch of grass that has
/// simply been dimmed — a stain on the texture rather than anything happening in
/// the world — and it does this most obviously where the canopy is *open*, since
/// there is not even any thickness to explain it.
///
/// So the terms that vary slowly get their negative half compressed hard, and
/// the terms that vary fast — micro-occlusion at three pixels, the under-stroke
/// on each mark, the mat below the canopy — keep theirs in full. Dark then only
/// ever appears as a narrow thing between two lit things, which is the only
/// place it is legible as depth. It costs a little exposure in the upward
/// direction, which the field wanted anyway.
const BROAD_DARK: f32 = 0.58;

/// Keep a signed term's positive half and compress its negative half.
///
/// Deliberately linear on each side rather than a smooth curve through zero: a
/// curve would also flatten the small values, which are most of the field, and
/// the point is to change what *large* negative excursions do without touching
/// the gentle modulation everywhere else. Continuous at zero, so nothing here
/// can print an edge.
#[inline]
fn squashed(value: f32, below: f32) -> f32 {
    if value >= 0.0 { value } else { value * below }
}

/// Rebalance a colour shift so it moves hue without also moving exposure.
///
/// Three separate terms in [`resolve`] push a resolved colour toward a different
/// green — the canopy-depth cooling, the chroma calming, and the regional drift
/// — and every one of them is meant to answer "*which* green is this" rather
/// than "how much light is on it". Written by hand they do not: dropping red by
/// a fifth takes real luminance out with it, and the three of them together were
/// quietly costing the plate about a percent of its exposure.
///
/// That is worse than it sounds, because `drift` keys on a *regional* field. A
/// hue shift that also darkens turns "this part of the meadow is a different
/// green" into "this part of the meadow is dimmer", so whole regions lose light,
/// the ten seeded worlds spread apart in mean luminance, and the suite reports it
/// as a generator that cannot hold its exposure. The fix belongs here rather
/// than in a compensating lift somewhere else, because a lift restores the mean
/// and leaves the region-to-region spread exactly where it was.
///
/// Normalising the whole vector rather than solving for green alone: it is exact
/// at every input colour instead of at the one the constant was derived from,
/// and it survives someone changing a multiplier. It pulls a little of the shift
/// back out — the ratio scales red and blue too — which is why the multipliers
/// at the call sites are stated stronger than the effect they are meant to have.
#[inline]
fn hue_only(from: Vec3, to: Vec3) -> Vec3 {
    const WEIGHTS: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);
    let after = to.dot(WEIGHTS);
    if after > 1.0e-6 {
        to * (from.dot(WEIGHTS) / after)
    } else {
        to
    }
}

/// Assemble one light index per pixel and look it up in a ramp.
pub fn resolve(surface: &Surface, page: &Page, lattice: &Macro, params: &BakeParams) -> Vec<Vec3> {
    let (width, height) = (page.width, page.height);
    let (heights, _buried) = surface.height_maps(width, height);
    // A fixed ceiling rather than this page's own tallest blade. Normalising by
    // a per-page maximum makes every derived term — the glaze, the cool drift —
    // depend on what happened to grow inside that particular rectangle, so two
    // neighbouring pages shade the same pixel differently and the join between
    // them becomes visible. Constants tile; page statistics do not.
    const CANOPY_CEILING: f32 = 48.0;

    // Two radii of the same measurement, and they are half a metre apart because
    // they answer different questions. Three pixels separates one blade from the
    // one behind it. Fifty-two — about half a metre — is the distance from the
    // middle of a bunch of grass to the open ground beside it.
    //
    // That second number is set by measurement rather than by taste, and it is
    // worth saying how. Decomposing the structure ladder into energy per octave
    // — `structure.r32² − structure.r64²` and its neighbours — says where the
    // variance actually is, which no single rung of the ladder can. Read that
    // way this field had *half again* the reference's energy between four and
    // sixteen pixels and less than half of it between thirty-two and sixty-four:
    // not a flat plate, a plate whose organisation was all at the wrong radius.
    // This term is the one that decides which radius, because it is the only
    // lighting term whose scale is a free parameter rather than a consequence of
    // the geometry, and moving it from a third of a metre to a half moved the
    // energy with it.
    let near = blur(&heights, width, height, 3);
    let far = blur(&heights, width, height, 52);
    // Which way to look for the canopy a bunch is standing against — see
    // [`BakeParams::canopy_relief`]. Toward the key, so that a pixel on the
    // sunward flank of a bunch is compared with the open ground in front of it
    // and a pixel at its shaded foot is compared with the bunch itself.
    let toward = Vec2::new(params.light.x, params.light.y).normalize_or(Vec2::NEG_Y);

    let shadow = directional_shadow(&heights, width, height, params.light);
    // Five pixels, not two. Sunlight through a canopy has no sharp edge to it;
    // the shadow this term describes is cast by grass onto grass a few
    // centimetres away, and the penumbra of that is wider than the shadow.
    let shadow = blur(&shadow, width, height, 5);

    let mut colours = vec![Vec3::ZERO; width * height];
    // How much of each pixel gets glazed back into its neighbourhood, filled in
    // as the colours are resolved.
    let mut glaze_mask = vec![0.0f32; width * height];
    let field = WorldField::lit_by(params.seed, params.light);

    // The macro lighting is assembled first and then blurred, rather than being
    // applied where it is computed.
    //
    // Every term in it is derived from a lattice sampled every six pixels and
    // read back bilinearly, so its gradient is piecewise constant: the value is
    // continuous but its slope jumps at every lattice line. On a flat green
    // field that shows as faint creases — hard transitions in something that is
    // meant to be light. Blurring the assembled term costs one pass and makes a
    // hard transition impossible by construction, whatever the terms do.
    let mut macro_light = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            // Shaded by the domes themselves rather than by differencing their
            // sum — see [`crate::field::WorldField::mounds`]. Smooth by
            // construction, with no lattice creases and no terminator.
            let facing = lattice.at(&lattice.lit, fx, fy).clamp(-1.0, 1.0);
            // The shaded side falls away at roughly half the rate the lit side
            // climbs.
            //
            // A surface lit by `N·L` alone darkens as fast as it brightens, and
            // the back of every mound runs to the bottom of the ramp together —
            // which is what makes a form read as cut out rather than as lit.
            // Grass does not do that: it is a thin translucent canopy over more
            // of itself, so the far side of a mound is dimmer than the near side
            // by much less than the geometry says. Compressing the negative half
            // is the cheapest honest model of that, and it leaves flat ground
            // exactly neutral, which a wrap of the usual `(x+k)/(1+k)` form does
            // not.
            //
            // Pulled down to meet [`BROAD_DARK`], which it is the oldest special
            // case of: a mound is metres across, so its shaded back is a broad
            // dark area and falls under the same rule.
            const SHADED_SIDE: f32 = 0.40;
            let wrapped = if facing >= 0.0 {
                facing
            } else {
                facing * SHADED_SIDE
            };
            // Scaled to the field's actual range, not to a tenth of it. At the
            // old scale this saturated across most of the ground and the term
            // degenerated into a constant — present in the arithmetic, absent
            // from the picture.
            let rise = (lattice.at(&lattice.height_field, fx, fy) * 5.0).clamp(0.0, 1.0);

            let crown = lattice.at(&lattice.crown, fx, fy);
            let tint = lattice.at(&lattice.tint, fx, fy);

            let index = y * width + x;
            let canopy = heights[index];
            // Light that has come through the canopy rather than off it.
            //
            // Thin grass on the shaded side of a mound is not dark, it glows:
            // the blades are a few cells thick and the sun is behind them. So
            // the transmitted term is strongest exactly where the reflected one
            // is weakest, which is what keeps the far side of every mound a
            // luminous green instead of a flat one.
            let thinness = (1.0 - canopy / CANOPY_CEILING).clamp(0.0, 1.0);
            let through = (-facing).max(0.0) * (0.35 + thinness * 0.65);
            // Occlusion is measured as "lower than the canopy around me", which
            // is exactly true inside a clump and exactly backwards inside a
            // clearing: a bare patch is lower than everything near it *and* open
            // to the sky. Left uncorrected it paints a ring of shadow round every
            // scuff of earth, which is the one lighting mistake that makes bare
            // ground read as a hole punched through the field.
            let open = 1.0 - lattice.at(&lattice.bare, fx, fy) * 0.85;
            let micro = ((near[index] - canopy) * 0.09).clamp(0.0, 1.0) * open;
            // Signed, at the bunch scale, and read off toward the key — see
            // [`BakeParams::canopy_relief`]. Clamped into the page rather than
            // wrapped or mirrored: within `RELIEF_REACH` of an edge the offset
            // collapses back to the symmetric comparison, which is a gradual
            // softening of one term across ten pixels of a page that has already
            // been blurred by `light_blur`, and not a discontinuity.
            let sample = Vec2::new(fx, fy) + toward * RELIEF_REACH;
            let sx = sample.x.clamp(0.0, (width - 1) as f32) as usize;
            let sy = sample.y.clamp(0.0, (height - 1) as f32) as usize;
            let relief = ((canopy - far[sy * width + sx]) * 0.040).clamp(-1.0, 1.0) * open;

            // How strongly this area states its mound at all. Without it the
            // macro lighting describes every form equally and reads as a map of
            // the height field rather than as light falling on ground.
            let stated = lattice.at(&lattice.statement, fx, fy).clamp(0.0, 1.4);
            macro_light[index] = params.mound_light * wrapped * stated
                + params.transmission * through
                + params.elevation_light * (rise - 0.45)
                + params.crown_light * (crown - 0.4)
                - params.micro_occlusion * micro
                + params.canopy_relief * squashed(relief, BROAD_DARK)
                - params.shadow * shadow[index]
                + params.region * squashed(tint, BROAD_DARK);
        }
    }

    let macro_light = blur(&macro_light, width, height, params.light_blur);

    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            let index = y * width + x;
            let canopy = heights[index];
            let world = macro_light[index];

            let resolved = surface.resolve_pixel(x, y, |i| {
                let (light, tone) = surface.pixel(i);
                let q = shoulder(light + world);
                let colour = palette::shade(tone, q);
                let soil = surface.soil_at(i);
                if soil <= 0.0 {
                    colour
                } else {
                    colour.lerp(palette::shade(Tone::Soil, q), soil)
                }
            });

            // Cool the shadows toward emerald and leave the lights alone. The
            // reference's darks are not its mid-greens turned down; they are a
            // different, bluer green, and a plate that only varies in value
            // gives itself away as one colour under a lamp.
            //
            // Deeper than it was, in both senses: more of it, and further. Warm
            // light against cool shadow is the oldest way there is of making a
            // surface read as lit rather than as patterned, and it is the one
            // thing a value-only shader cannot fake — no amount of contrast
            // between two samples of the same hue says which direction the sun
            // is in.
            let ground_at = iso::from_cache_ground(page.origin + Vec2::new(fx, fy));
            let dampness = field.jitter(Stream::Tint, ground_at, 0.55);
            let shade_depth = (1.0 - (canopy / CANOPY_CEILING)).clamp(0.0, 1.0);
            let cool = params.cool * shade_depth * (0.4 + dampness * 0.8);
            // Most of the cooling is red given up rather than blue picked up,
            // and the ratio matters more than the amount. Adding blue to a green
            // this saturated turns it toward teal and the hue rows say so within
            // a couple of degrees; taking red out slides it toward emerald,
            // which is the same perceptual move and costs nothing measurable.
            let cooled = hue_only(
                resolved,
                Vec3::new(
                    resolved.x * 0.78,
                    resolved.y,
                    resolved.z + resolved.y * 0.042,
                ),
            );
            let resolved = resolved.lerp(cooled, cool.clamp(0.0, 1.0));

            // Take a little chroma out of the body of the field and none at all
            // out of the tips.
            //
            // The ramp is measured from the reference and the reference is a
            // saturated painting, so a plate that matches it row for row is
            // correct and still lands as one continuous acid green — because
            // *every* part of it is carrying full chroma, and a colour that is
            // everywhere is not a colour, it is a cast. Draining the mid and low
            // range slightly while leaving the highlights alone widens the gap
            // between the two, and a highlight is only as bright as what it sits
            // against. Selective saturation reads richer than universal
            // saturation; this is the whole of that idea in three lines.
            //
            // Red given up, not blue picked up, and this was worth getting
            // wrong once to learn. The obvious way to desaturate is to lerp
            // toward the pixel's own luminance, and in a palette whose blue
            // channel sits at 0.04 that is almost entirely a blue *injection* —
            // a five percent lerp moved the plate's mean blue by forty. The
            // measured result was a field that had gone teal while the number
            // that was supposed to move, saturation, had barely shifted.
            //
            // What "too lime" actually describes is red sitting too close to
            // green, so that is the channel to move. Taking a little red out
            // walks the hue toward emerald, reads as calmer at a glance, and
            // costs almost nothing on any row of the comparison — the same
            // perceptual result for a twentieth of the measured drift.
            // A real chroma reduction, weighted by how deep in shade the pixel
            // is: hardest in the shadows, half as hard through the body, none at
            // all on the tips.
            //
            // The first attempt at this only took red out, because pulling
            // toward grey raised the plate's mean blue by forty percent and that
            // looked like a disaster. It is not one — `channel.blue` is not a
            // term in `metrics::distance`, and it could not sensibly be, because
            // this palette's blue sits at 0.04 where a one-part-in-fifty change
            // is a forty percent change. The only thing a desaturation actually
            // costs is the `saturation` row, and that row is being deliberately
            // spent: the reference is a saturated painting to be looked at, and
            // this is a floor that has to sit under an army.
            //
            // Toward the pixel's own luminance, so it is exactly a chroma move
            // and cannot touch exposure or the tone percentiles. Shadows that are
            // cooler *and* less saturated than the light is most of what
            // separates a painted surface from one green under a dimmer lamp —
            // the cooling alone was only ever half the statement.
            // Toward a cool green of the same luminance, never toward grey.
            //
            // Grey is the obvious desaturation target and it is subtly the wrong
            // one: it drains the hue as well as the chroma, and a shaded passage
            // that has lost its hue reads as haze lying over the field rather
            // than as shadow in it. The measured `saturation` row cannot tell the
            // two apart — both land on the same number — which is exactly why
            // this has to be decided by eye and then written down.
            //
            // So the target keeps a clear green bias and gives up most of its
            // chroma rather than all of it. The blend then does two things at
            // once and they are the two things the shade wanted: less saturated,
            // and cooler, because the target's red is well below its green.
            let luma = resolved.dot(Vec3::new(0.2126, 0.7152, 0.0722));
            let calm = params.temper * (1.0 - smoothstep(0.28, 0.62, luma));
            let muted = hue_only(resolved, Vec3::splat(luma) * Vec3::new(0.80, 1.14, 0.84));
            let resolved = resolved.lerp(muted, calm.clamp(0.0, 1.0));

            // Then the region's own hue, which is keyed to nowhere near the same
            // thing — see [`BakeParams::drift`]. Both ends are gentle multiples
            // of the colour already resolved rather than blends toward a named
            // paint, so the ramp's measured relationship between its channels
            // survives the drift and only its balance moves.
            let drift = lattice.at(&lattice.hue, fx, fy).clamp(-1.0, 1.0) * params.drift;
            // Both branches are luminance-preserving — see [`hue_only`]. This is
            // the one that mattered most: `hue` is a *regional* field, so a
            // drift that also darkened made whole regions dim, and whole dim
            // regions are what pushed the ten worlds apart in mean luminance.
            // A plate that lands in a strongly drifted region was measurably
            // darker than one that did not, for no reason anybody asked for.
            let shifted = hue_only(
                resolved,
                if drift >= 0.0 {
                    // Olive: drier, older grass. Red gains on green and the blue
                    // that was barely there gives up more of it.
                    Vec3::new(resolved.x * 1.14, resolved.y, resolved.z * 0.82)
                } else {
                    // Blue-green: shaded, damp, or simply a different species.
                    Vec3::new(resolved.x * 0.84, resolved.y, resolved.z * 1.06)
                },
            );
            colours[index] = resolved.lerp(shifted, drift.abs());

            // Glaze the low canopy back into its neighbourhood, and leave the
            // marks that stand proud of it crisp. Height is the right selector:
            // it is exactly "did this stroke win its pixel by standing above the
            // mass", which is the same question a painter answers when deciding
            // which strokes survive the glaze.
            let exposure = (canopy / CANOPY_CEILING).clamp(0.0, 1.0);
            // Loosely described ground glazes far harder than well described
            // ground. This is where "some passages are paint" actually happens:
            // the marks are still drawn, they simply stop being individually
            // legible.
            let loose = 1.0 - lattice.at(&lattice.resolution, fx, fy);
            glaze_mask[index] = params.glaze * (0.15 + loose * 0.85) * (1.0 - exposure).powf(1.2);
        }
    }

    glaze(&mut colours, width, height, &glaze_mask);
    soften(&mut colours, width, height, params.soften);
    colours
}

/// Blend each pixel toward the average colour of its neighbourhood.
///
/// A five-tap cross at two pixels, rather than a proper blur: the aim is to melt
/// adjacent strokes into one another, not to smear the page. Anything wider
/// starts eating the marks that were meant to survive.
fn glaze(colours: &mut [Vec3], width: usize, height: usize, mask: &[f32]) {
    const REACH: usize = 2;
    let source = colours.to_vec();
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let amount = mask[index];
            if amount <= 0.01 {
                continue;
            }
            let left = x.saturating_sub(REACH);
            let right = (x + REACH).min(width - 1);
            let up = y.saturating_sub(REACH);
            let down = (y + REACH).min(height - 1);
            let local = (source[index]
                + source[y * width + left]
                + source[y * width + right]
                + source[up * width + x]
                + source[down * width + x])
                / 5.0;
            colours[index] = source[index].lerp(local, amount.min(1.0));
        }
    }
}

/// Mix a one-pixel tent blur into the finished page.
///
/// A tent rather than a wider Gaussian, and mixed rather than applied: the goal
/// is to land on the reference's edge softness, not to lose the strokes. Too
/// much and the plate turns to felt; none at all and it is visibly crisper than
/// the painting it is meant to match at every measured radius.
fn soften(colours: &mut [Vec3], width: usize, height: usize, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let source = colours.to_vec();
    let at = |x: usize, y: usize| source[y * width + x];
    for y in 0..height {
        for x in 0..width {
            let (left, right) = (x.saturating_sub(1), (x + 1).min(width - 1));
            let (up, down) = (y.saturating_sub(1), (y + 1).min(height - 1));
            let centre = at(x, y);
            let sides = at(left, y) + at(right, y) + at(x, up) + at(x, down);
            let corners = at(left, up) + at(right, up) + at(left, down) + at(right, down);
            let blurred = (centre * 4.0 + sides * 2.0 + corners) / 16.0;
            colours[y * width + x] = centre.lerp(blurred, amount.clamp(0.0, 1.0));
        }
    }
}

/// March the canopy height toward the light and see what blocks it.
///
/// Fixed camera and fixed key mean this can be baked once, and it is the term
/// that gives each mound a lit face and a dark back. Kept deliberately short —
/// eight pixels of soft separation rather than a long cast shadow — because the
/// reference has no hard shadows anywhere in it.
fn directional_shadow(heights: &[f32], width: usize, height: usize, light: Vec3) -> Vec<f32> {
    let plane = Vec2::new(light.x, light.y);
    let toward = plane.normalize_or(Vec2::NEG_Y);
    // Height a blocker must gain per pixel travelled to shade this point.
    let rise = (light.z / plane.length().max(1.0e-3)).clamp(0.3, 4.0);

    const STEPS: usize = 9;
    const STEP: f32 = 1.4;
    let mut shadow = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let base = heights[y * width + x];
            let mut most = 0.0f32;
            for step in 1..=STEPS {
                let distance = step as f32 * STEP;
                let sample = Vec2::new(x as f32, y as f32) + toward * distance;
                if sample.x < 0.0 || sample.y < 0.0 {
                    break;
                }
                let (sx, sy) = (sample.x as usize, sample.y as usize);
                if sx >= width || sy >= height {
                    break;
                }
                let over = heights[sy * width + sx] - base - distance * rise * 0.5;
                most = most.max((over * 0.10).clamp(0.0, 1.0));
            }
            shadow[y * width + x] = most;
        }
    }
    shadow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_page() -> Page {
        Page::new(Vec2::new(-64.0, -64.0), 96, 96)
    }

    #[test]
    fn pages_meet_without_a_seam() {
        // The claim page independence rests on, measured rather than asserted:
        // two pages baked with no knowledge of each other have to join.
        //
        // Not by being identical to one big page — they are not, and cannot be.
        // The macro lattice is laid out from each page's own origin at a
        // six-pixel stride, and 256 is not a multiple of six, so neighbours
        // interpolate the composition fields from sample points up to four
        // pixels apart. A whole-region bake and a tiled one therefore differ
        // across a fifth of their pixels. That is a property of the lattice, not
        // a defect, and it is invisible for the reason this test checks: the
        // fields are smooth at six pixels, so reading them from slightly
        // different places moves the answer by far less than the grass on top
        // of it does.
        //
        // What would be a defect is a *step* at the join — a column of ground
        // systematically brighter than the column beside it. Column means are
        // what expose that: they average the stroke noise away and leave the
        // slowly-varying part, which is exactly the part a lattice mismatch
        // would disturb.
        const WIDTH: usize = 512;
        const HEIGHT: usize = 256;
        let region = Page::new(Vec2::new(-256.0, -128.0), WIDTH, HEIGHT);
        let plate = bake_grid(region, &BakeParams::default());

        let column = |x: usize| -> f32 {
            (0..HEIGHT)
                .map(|y| {
                    let c = plate[y * WIDTH + x];
                    c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
                })
                .sum::<f32>()
                / HEIGHT as f32
        };
        let step = |x: usize| (column(x) - column(x - 1)).abs();

        let seam = step(WIDTH / 2);
        let interior: Vec<f32> = (1..WIDTH).filter(|x| *x != WIDTH / 2).map(step).collect();
        let typical = interior.iter().sum::<f32>() / interior.len() as f32;
        let worst = interior.iter().copied().fold(0.0f32, f32::max);

        assert!(
            seam <= worst,
            "the page join steps by {seam:.5}, more than any ordinary column pair \
             (typical {typical:.5}, worst {worst:.5})"
        );
    }

    #[test]
    fn a_page_bakes_to_grass_rather_than_to_soil() {
        let colours = bake(small_page(), &BakeParams::default());
        assert_eq!(colours.len(), 96 * 96);
        let green = colours
            .iter()
            .filter(|c| c.y > c.x * 1.25 && c.z < 0.25)
            .count();
        let fraction = green as f32 / colours.len() as f32;
        // Not 95%. The brightest tips are nearly as red as they are green and
        // fall outside this test on purpose, and a few percent of any honest
        // page is exposed earth. What it catches is the failure that has
        // actually happened — the canopy not covering, and the plate coming back
        // mostly soil.
        assert!(
            fraction > 0.82,
            "only {:.1}% of the page reads as grass",
            fraction * 100.0
        );
    }

    #[test]
    fn splitting_a_page_in_two_grows_the_same_grass() {
        // The property the whole design exists to guarantee, and the one that
        // fails silently: a seam is invisible until the world is big enough to
        // have two pages on screen at once.
        //
        // Comparing the two pixels either side of the join would not test it.
        // Those are different pixels of a deliberately high-frequency texture,
        // and a bright tip beside a dark gap is a large difference in a plate
        // that is behaving perfectly. What has to hold is that the *same* world
        // pixel comes out the same however the page grid was laid over it.
        let params = BakeParams::default();
        let whole = bake(Page::new(Vec2::ZERO, 128, 64), &params);
        let left = bake(Page::new(Vec2::ZERO, 64, 64), &params);
        let right = bake(Page::new(Vec2::new(64.0, 0.0), 64, 64), &params);

        let (mut worst, mut total) = (0.0f32, 0.0f32);
        for y in 0..64 {
            for x in 0..128 {
                let split = if x < 64 {
                    left[y * 64 + x]
                } else {
                    right[y * 64 + (x - 64)]
                };
                let difference = (whole[y * 128 + x] - split).length();
                worst = worst.max(difference);
                total += difference;
            }
        }
        let mean = total / (128.0 * 64.0);

        // Not bit-identical, and it cannot be: occlusion, the directional
        // shadow and the glaze all read a neighbourhood, and near a page edge
        // that neighbourhood is cropped. What matters is that the disagreement
        // stays at the level of shading rather than of content — a stroke
        // present on one side and missing on the other would be far larger.
        assert!(
            worst < 0.30,
            "a page edge changed what grows there: worst pixel differs by {worst}"
        );
        assert!(mean < 0.01, "the join is visible on average: {mean}");
    }

    /// The key light is declared twice — here, and in [`crate::field`] where the
    /// mound domes shade themselves against it. A doc comment claiming the two
    /// agree is worth nothing; a mismatch would light the mounds from one
    /// direction and their blades' under-strokes from another, which reads as
    /// wrong long before anyone works out why.
    #[test]
    fn the_mound_field_is_lit_from_the_same_place_as_the_marks() {
        let light = BakeParams::default().light;
        let plane = Vec2::new(light.x, light.y).normalize();
        assert!(
            plane.distance(crate::field::LIGHT_PLANE) < 1.0e-3,
            "bake says {plane:?}, the mound field says {:?}",
            crate::field::LIGHT_PLANE
        );
        // And it is the upper left, in image space where +Y points down.
        assert!(plane.x < 0.0 && plane.y < 0.0, "the sun moved: {plane:?}");
    }

    /// The guard band has to be wider than the longest mark can reach, and the
    /// arithmetic for that is easy to get wrong in the safe-looking direction.
    ///
    /// Arc length is not reach: every mark here bends as it grows, so a stroke
    /// of 0.91 metres gets nowhere near 0.91 metres from its root. Bounding the
    /// band by arc length alone would be conservative and fine; the failure mode
    /// is the opposite one, where someone lengthens a blade or widens the vigour
    /// clamp and the band silently stops covering it. So this rasterises the
    /// worst mark the parameters allow and measures where the paint actually
    /// lands, which is the only version of the question that stays true when the
    /// parameters move.
    #[test]
    fn the_guard_band_covers_the_longest_mark_the_field_can_grow() {
        let params = BakeParams::default();
        // Every multiplier on the path from `blade_length` to a rasterised
        // stroke, at its maximum: the `Tangle` family's own factor, the vigour
        // clamp in `grow_tuft`, and the tall-accent reach draw.
        let longest = params.blade_length.1 * 1.25 * 1.35 * 1.35;
        // The tuft's blades are rooted up to this far from the centre that
        // `scatter` tests, so the centre has to cover the offset as well. Must
        // track the radius drawn in [`grow_tuft`]; a stale value here is a test
        // that reports a guard band as sufficient for a field that no longer
        // exists.
        let tuft_radius = TUFT_RADIUS;

        let mut worst: f32 = 0.0;
        // Sweep the azimuth, because the projection is anisotropic: a mark laid
        // along the world diagonal that maps to the screen's horizontal covers
        // 1.41 times the cache pixels one laid along the other diagonal does.
        // And sweep the bend, because a *straighter* mark reaches further — the
        // longest arc is not the furthest-reaching one.
        for step in 0..64 {
            let azimuth = step as f32 / 64.0 * std::f32::consts::TAU;
            for bend in [1.3, 1.65, 2.0, 2.62] {
                let mut surface = Surface::new(512, 512);
                let origin = Vec2::new(-256.0, -256.0);
                let mut painter = Painter::new(&mut surface, origin, params.light);
                let root = painter.to_ground(Vec2::new(
                    256.0 * SUPERSAMPLE as f32,
                    256.0 * SUPERSAMPLE as f32,
                ));
                painter.draw(&Stroke {
                    root: root.extend(0.0),
                    azimuth,
                    length: longest,
                    bend,
                    curl: 1.4,
                    sway: 2.4,
                    width: params.blade_width.1,
                    under: params.under,
                    ..default()
                });
                let (heights, _) = surface.height_maps(512, 512);
                for y in 0..512 {
                    for x in 0..512 {
                        if heights[y * 512 + x] > 0.0 {
                            let dx = x as f32 - 256.0;
                            let dy = y as f32 - 256.0;
                            worst = worst.max((dx * dx + dy * dy).sqrt());
                        }
                    }
                }
            }
        }

        let needed = worst + tuft_radius * std::f32::consts::SQRT_2 * iso::PX_PER_METRE;
        assert!(
            MARGIN > needed,
            "a mark can reach {worst:.1} px from its root and be rooted \
             {:.1} px from the tuft centre the guard tests, so the band needs \
             {needed:.1} px and it is {MARGIN}",
            tuft_radius * std::f32::consts::SQRT_2 * iso::PX_PER_METRE,
        );
        // And the band must not be so far past the requirement that it is
        // costing bake time for nothing: every extra pixel widens the world
        // rectangle every scatter pass walks.
        assert!(
            MARGIN < needed * 1.6,
            "the guard band is {MARGIN} px for a {needed:.1} px reach, which is \
             paid for on every page"
        );
    }

    /// The furthest a mark reaches from its own root, per screen direction.
    ///
    /// Returns `(left, right, up, down)` in cache pixels, all positive. Sweeps
    /// the azimuth because the projection is anisotropic, and sweeps the bend
    /// because a straighter mark reaches further than a longer one — the longest
    /// arc is not the furthest-reaching stroke.
    fn reach_by_direction(
        params: &BakeParams,
        longest: f32,
        bends: &[f32],
    ) -> (f32, f32, f32, f32) {
        let (mut left, mut right, mut up, mut down) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for step in 0..64 {
            let azimuth = step as f32 / 64.0 * std::f32::consts::TAU;
            for bend in bends {
                let mut surface = Surface::new(512, 512);
                let mut painter =
                    Painter::new(&mut surface, Vec2::new(-256.0, -256.0), params.light);
                let root = painter.to_ground(Vec2::new(
                    256.0 * SUPERSAMPLE as f32,
                    256.0 * SUPERSAMPLE as f32,
                ));
                painter.draw(&Stroke {
                    root: root.extend(0.0),
                    azimuth,
                    length: longest,
                    bend: *bend,
                    curl: 1.4,
                    sway: 2.4,
                    width: params.blade_width.1,
                    under: params.under,
                    ..default()
                });
                let (heights, _) = surface.height_maps(512, 512);
                for y in 0..512 {
                    for x in 0..512 {
                        if heights[y * 512 + x] > 0.0 {
                            let (dx, dy) = (x as f32 - 256.0, y as f32 - 256.0);
                            left = left.max(-dx);
                            right = right.max(dx);
                            up = up.max(-dy);
                            down = down.max(dy);
                        }
                    }
                }
            }
        }
        (left, right, up, down)
    }

    /// The placement rectangle is asymmetric, so it needs a test that knows
    /// which way is up.
    ///
    /// [`footprint`] decides which cells are *visited* at all, and its four
    /// margins are different sizes because grass grows up the screen. That
    /// asymmetry is an assumption about the mark vocabulary, and the vocabulary
    /// changes: when a minority of blades were turned down-screen to lay a
    /// near-side skirt, `ABOVE` went from comfortably sufficient to a quarter of
    /// what it needed to be, and nothing failed.
    ///
    /// Nothing *could* fail. The page-join test splits left from right, so both
    /// halves share a top edge and a shortfall above is invisible to it. The
    /// symptom would have been a tuft rooted off the top of a page, leaning into
    /// it, simply absent — a straight line along the join where one page grew
    /// something its neighbour did not, showing up only once two pages are on
    /// screen together.
    #[test]
    fn the_placement_rectangle_covers_every_direction_a_mark_reaches() {
        let params = BakeParams::default();
        // Every multiplier from `blade_length` to a rasterised stroke, at its
        // maximum: the `Tangle` factor, the vigour clamp, the tall-accent reach.
        let longest = params.blade_length.1 * 1.25 * 1.45 * 1.35;
        // Up to `Tangle`'s ceiling, plus what the skirt adds on top of it.
        let bends = [0.9, 1.4, params.blade_bend.1, 2.0, 2.0 + SKIRT_BEND];
        let (left, right, up, down) = reach_by_direction(&params, longest, &bends);
        // A blade is rooted anywhere within its tuft, so the guard has to cover
        // the offset as well as the reach. Projected, a world radius `r` spans
        // `2r` across the screen and `r` down it — the anisotropy of the
        // dimetric projection, which is why the sideways band is the tight one.
        let across = TUFT_RADIUS * 2.0 * iso::PX_PER_METRE;
        let downward = TUFT_RADIUS * iso::PX_PER_METRE;

        // Mirrors the constants in `footprint`, which cannot be read from here.
        for (name, band, needed) in [
            ("SIDE", 122.0f32, left.max(right) + across),
            ("BELOW", 156.0, up + downward),
            ("ABOVE", 46.0, down + downward),
        ] {
            assert!(
                band > needed,
                "a mark reaches {needed:.1} px in the direction {name} guards \
                 and the band is {band}"
            );
            assert!(
                band < needed * 2.2,
                "{name} is {band} px for a {needed:.1} px reach, and every pixel \
                 of it widens the rectangle each scatter pass walks"
            );
        }
    }

    #[test]
    fn baking_is_deterministic() {
        let params = BakeParams::default();
        assert_eq!(bake(small_page(), &params), bake(small_page(), &params));
    }

    #[test]
    fn a_different_seed_grows_different_grass() {
        let mut params = BakeParams::default();
        let first = bake(small_page(), &params);
        params.seed ^= 0xabcd;
        let second = bake(small_page(), &params);
        let differing = first
            .iter()
            .zip(&second)
            .filter(|(a, b)| (**a - **b).length() > 0.02)
            .count();
        assert!(
            differing > first.len() / 2,
            "the seed barely changed anything"
        );
    }

    #[test]
    fn the_page_has_no_dead_black_and_no_blown_white() {
        // Two failure modes with the same cause — a shading term escaping its
        // range — and both are instantly visible.
        let colours = bake(small_page(), &BakeParams::default());
        for colour in &colours {
            let luma = colour.x * 0.2126 + colour.y * 0.7152 + colour.z * 0.0722;
            assert!(luma > 0.04, "a pixel went black: {colour:?}");
            assert!(luma < 0.95, "a pixel blew out: {colour:?}");
        }
    }
}

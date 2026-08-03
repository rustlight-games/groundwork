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
    const KNEE: f32 = 0.740;
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
    /// Measured against a blur a seventh of a metre wide, which is the scale a
    /// bunch of grass is. So it says the one true thing about a bunch: the tips
    /// on top of it are in the light and the ground between it and the next one
    /// is not. Zero mean, so it costs no exposure — it only redistributes.
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
            tufts: 65.0,
            blades_per_tuft: (6, 20),
            thatch: 340.0,
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

            base_light: 0.577,
            tip_light: 0.42,
            glint: 0.775,
            side_light: 0.075,
            under: 0.55,

            mound_light: 0.33,
            elevation_light: 0.035,
            crown_light: 0.038,
            micro_occlusion: 0.105,
            canopy_relief: 0.20,
            shadow: 0.10,
            transmission: 0.17,
            light_blur: 4,
            region: 0.44,
            scatter: 0.505,
            glaze: 0.11,
            cool: 0.15,
            drift: 0.52,
            soften: 0.10,
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
struct Macro {
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
    fn build(page: &Page, field: &WorldField) -> Self {
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
pub fn bake(page: Page, params: &BakeParams) -> Vec<Vec3> {
    let field = WorldField::lit_by(params.seed, params.light);
    let lattice = Macro::build(&page, &field);
    let mut surface = Surface::new(page.width, page.height);

    lay_floor(&mut surface, &page, &field, &lattice);
    {
        let mut painter = Painter::new(&mut surface, page.origin, params.light);
        plant(
            &mut painter,
            &Bed {
                page: &page,
                field: &field,
                params,
            },
        );
    }
    resolve(&surface, &page, &lattice, params)
}

/// The floor under everything: soil where the ground is bare, dark mat where it
/// is not.
///
/// Filling the floor with thatch rather than growing enough short strokes to
/// hide the soil is worth a great deal of time. The gaps between bright blades
/// have to be dark green, not brown, or the field reads as grass scattered on
/// dirt; but they do not have to be *textured* dark green, because almost none
/// of it survives the canopy.
fn lay_floor(surface: &mut Surface, page: &Page, field: &WorldField, lattice: &Macro) {
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
            let soil = smoothstep(0.07, 0.82, bare);
            // Kept dark, and kept grainy. Bare ground that is much paler than
            // the canopy turns every blade lying across it into a dark comma on
            // a light field, which is the single loudest way to make a clearing
            // read as a hole with things planted in it.
            let loose = 1.0 - lattice.at(&lattice.resolution, x as f32, y as f32);
            // Lifted well off the bottom of the thatch ramp. The floor shows
            // between blades everywhere, and a floor that is genuinely dark
            // turns every clump into a shaded volume sitting in a shadowed pit —
            // which is a perfectly good way to draw a plant and the wrong way to
            // draw this field.
            let light = 0.35 + mottle * 0.28 + grain * 0.34 * soil + bare * 0.05 + loose * 0.10;

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
    // Measured the way [`MARGIN`] is, the longest mark reaches about 100 pixels
    // sideways, 125 upward, and barely 30 down — a mark climbs as it grows, so
    // only its curled tip ever descends, and `ABOVE` is guarding against very
    // little. Each of these sits a fifth to a half above its requirement, which
    // is worth keeping an eye on in the other direction too: widening this
    // rectangle costs bake time on every page in proportion to its area, and an
    // extra thirty pixels all round is fourteen percent of a page.
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
        |ground| (1.20 - ground.resolution * 0.20) * (1.0 - ground.bare * 0.55),
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
        |ground| 0.72 + ground.resolution * 0.34,
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
            // Never all the way to nothing. The reference has no patch of
            // ground with no green on it at all; even its barest scuffs carry
            // shoots and root marks, and that is most of what keeps them
            // reading as ground rather than as bald spots.
            let coverage = 1.0 - smoothstep(0.04, 0.88, ground.bare) * 0.90;
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
        + ground.bare * 0.12
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
        base_light: (0.47
            + draw.normal() * 0.10
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
    let vigour = ((0.48 + ground.crown * 0.32 + ground.density * 0.52) * (1.0 - ground.bare * 0.5))
        .clamp(0.32, 1.35);
    // One tuft in eight stands well clear of its neighbours. Sparse tall accents
    // are what stop the canopy reading as a mown line.
    let reach = if draw.chance(0.12) {
        draw.range(1.1, 1.35)
    } else {
        1.0
    };

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
    let radius = draw.range(0.035, 0.15);
    let shade = plant_light(draw, ground, params) - params.base_light;
    let (fewest, most) = params.blades_per_tuft;
    let blades = fewest + draw.index(most - fewest + 1);
    let leaning = draw.chance(0.35);

    for _ in 0..blades {
        // Square root of a uniform: fills the disc evenly instead of piling up
        // at the centre.
        let angle = draw.range(0.0, std::f32::consts::TAU);
        let offset = Vec2::from_angle(angle) * radius * draw.unit().sqrt();
        // Bare ground grows sideways. Upright sprouts evenly spaced across a
        // clearing are the giveaway that the clearing was cut out of the grass
        // rather than found in it.
        let mut stroke = if ground.bare > 0.3 && draw.chance(0.55) {
            let flat = if draw.chance(0.5) {
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
        // Blades within a tuft differ as much as tufts differ from each other.
        // The reference has bright single blades standing in dim clumps and dim
        // ones in bright clumps, and a tuft whose blades all agree exactly reads
        // as a moulded plastic plant.
        stroke.base_light = (stroke.base_light + shade + draw.normal() * 0.09).clamp(0.05, 0.95);
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
        let weights = [
            0.21 - loose * 0.08, // Dash
            0.16 - loose * 0.06, // Kink
            0.14 - loose * 0.05, // Sway
            0.11 - loose * 0.04, // Hook
            0.11 + loose * 0.06, // Fleck
            0.10 + loose * 0.06, // Broad
            0.06 + loose * 0.03, // Tangle
            0.11 + loose * 0.05, // Buried
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
        // Roughly a quarter of marks catch the light sharply, and fewer where the
        // ground is loosely described. Give every blade a glint and the field
        // turns to wet plastic; give none and it is felt.
        let lit = 0.11 + ground.resolution * 0.13;
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
            tone: if draw.chance(0.004 + ground.hue.max(0.0) * 0.055) {
                Tone::Dry
            } else {
                Tone::Grass
            },
            base_light: params.base_light - recessive,
            tip_light: params.tip_light * draw.range(0.7, 1.3),
            glint: if recessive > 0.0 { 0.0 } else { glint },
            side_light: params.side_light,
            under: params.under * outlined,
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
        let base = Stroke {
            base_light: base.base_light - bulk * 0.11,
            glint: base.glint * (1.0 - bulk * 0.7),
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

/// Assemble one light index per pixel and look it up in a ramp.
fn resolve(surface: &Surface, page: &Page, lattice: &Macro, params: &BakeParams) -> Vec<Vec3> {
    let (width, height) = (page.width, page.height);
    let (heights, _buried) = surface.height_maps(width, height);
    // A fixed ceiling rather than this page's own tallest blade. Normalising by
    // a per-page maximum makes every derived term — the glaze, the cool drift —
    // depend on what happened to grow inside that particular rectangle, so two
    // neighbouring pages shade the same pixel differently and the join between
    // them becomes visible. Constants tile; page statistics do not.
    const CANOPY_CEILING: f32 = 48.0;

    // Two radii of the same measurement, and they are a third of a metre apart
    // because they answer different questions. Three pixels separates one blade
    // from the one behind it. Thirty-four — about a third of a metre — is the
    // distance from the middle of a bunch of grass to the open ground beside it,
    // and that is the scale this field was measurably flattest at: the reference
    // keeps half again as much variance through a thirty-two-pixel blur as an
    // earlier version of this baker did, and none of the stroke work closes that
    // gap, because a stroke is four pixels wide.
    let near = blur(&heights, width, height, 3);
    let far = blur(&heights, width, height, 34);

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
            const SHADED_SIDE: f32 = 0.42;
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
            // Signed at the bunch scale — see [`BakeParams::canopy_relief`].
            let relief = ((canopy - far[index]) * 0.035).clamp(-1.0, 1.0) * open;

            // How strongly this area states its mound at all. Without it the
            // macro lighting describes every form equally and reads as a map of
            // the height field rather than as light falling on ground.
            let stated = lattice.at(&lattice.statement, fx, fy).clamp(0.0, 1.4);
            macro_light[index] = params.mound_light * wrapped * stated
                + params.transmission * through
                + params.elevation_light * (rise - 0.45)
                + params.crown_light * (crown - 0.4)
                - params.micro_occlusion * micro
                + params.canopy_relief * relief
                - params.shadow * shadow[index]
                + params.region * tint;
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
            let ground_at = iso::from_cache_ground(page.origin + Vec2::new(fx, fy));
            let dampness = field.jitter(Stream::Tint, ground_at, 0.55);
            let shade_depth = (1.0 - (canopy / CANOPY_CEILING)).clamp(0.0, 1.0);
            let cool = params.cool * shade_depth * (0.4 + dampness * 0.8);
            let cooled = Vec3::new(
                resolved.x * 0.86,
                resolved.y,
                resolved.z + resolved.y * 0.035,
            );
            let resolved = resolved.lerp(cooled, cool.clamp(0.0, 1.0));

            // Then the region's own hue, which is keyed to nowhere near the same
            // thing — see [`BakeParams::drift`]. Both ends are gentle multiples
            // of the colour already resolved rather than blends toward a named
            // paint, so the ramp's measured relationship between its channels
            // survives the drift and only its balance moves.
            let drift = lattice.at(&lattice.hue, fx, fy).clamp(-1.0, 1.0) * params.drift;
            let shifted = if drift >= 0.0 {
                // Olive: drier, older grass. Red gains on green and the blue
                // that was barely there gives up more of it.
                Vec3::new(resolved.x * 1.11, resolved.y * 0.955, resolved.z * 0.82)
            } else {
                // Blue-green: shaded, damp, or simply a different species.
                Vec3::new(resolved.x * 0.86, resolved.y * 1.01, resolved.z * 1.06)
            };
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
        // `scatter` tests, so the centre has to cover the offset as well.
        let tuft_radius = 0.17;

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

//! Where every mark goes, and what shape it is.
//!
//! Placement used to live inside the baker and to run *through* the rasteriser:
//! `scatter` took a `Painter`, decided a blade should exist, and drew it in the
//! same breath. That works for one camera pass and becomes impossible the moment
//! the same geometry has to be rendered twice — once from the camera and once
//! from the sun — because the second pass would have to regenerate the scene and
//! trust it to come out the same.
//!
//! So nothing here draws. Every function in this module answers "what is here",
//! pushes it onto a list, and stops. [`crate::scene::GrassScene`] is that list,
//! and rasterising it is somebody else's job.
//!
//! ## What survives from the old arrangement
//!
//! The rules that made this look like grass rather than fur, all of them
//! unchanged:
//!
//! - **Place in world space.** Every decision is a pure function of a world
//!   coordinate and the seed, which is what lets two pages that have never met
//!   agree along a shared edge.
//! - **One thing per jittered cell**, spaced to the requested density, rather
//!   than several per cell — several in one cell clusters at the cell's scale,
//!   and a world-axis-aligned cell grid projects that clustering onto the screen
//!   diagonals where the eye finds it immediately.
//! - **Reject before sampling.** A [`crate::field::WorldField`] read costs a
//!   hundred mound kernels and over half the enumerated cells can never touch
//!   the page. Testing the cheap thing first is most of this crate's bake time.
//! - **Tufts, not blades.** Blades that share a lean, a length and a brightness
//!   read as a plant; the same blades scattered independently read as a doormat.

use glam::{Vec2, Vec3};

use crate::field::{Ground, GroundCache, WorldField};
use crate::geometry::TipProfile;
use crate::page::Page;
use crate::rng::{Draw, Stream};
use crate::stroke::{Profile, Stroke};
use crate::style::GrassParams;
use crate::tone::Tone;

/// Hermite ramp between two edges.
#[inline]
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The world rectangle whose grass can reach this page.
///
/// Wider than the page in every direction, and much wider below it: grass grows
/// up the screen, so a blade rooted off the bottom edge still leans into view,
/// and one rooted off the top edge never does.
pub fn footprint(page: &Page, caster_reach: f32) -> (Vec2, Vec2) {
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
    //
    // In *reference* pixels, and scaled to the page: these bound how far a mark
    // reaches, marks are measured in metres, and a page baked at a quarter scale
    // holds a mark in a quarter of the pixels. A band that did not scale would
    // guard four times the ground it needed to and walk four times the cells.
    const SIDE: f32 = 122.0;
    const BELOW: f32 = 156.0;
    const ABOVE: f32 = 46.0;
    let (side, below, above) = (page.px(SIDE), page.px(BELOW), page.px(ABOVE));
    let corners = [
        Vec2::new(-side, -above),
        Vec2::new(page.width as f32 + side, -above),
        Vec2::new(-side, page.height as f32 + below),
        Vec2::new(page.width as f32 + side, page.height as f32 + below),
    ];
    let mut low = Vec2::splat(f32::INFINITY);
    let mut high = Vec2::splat(f32::NEG_INFINITY);
    for corner in corners {
        let ground = page.ground_at(corner);
        low = low.min(ground);
        high = high.max(ground);
    }

    // And then again, by however far a *shadow* reaches — in **world metres**,
    // after the AABB rather than before it.
    //
    // That ordering is the whole of this paragraph. A shadow's reach is a world
    // distance, and widening the *page rectangle* by the equivalent number of
    // cache pixels does not produce it: the projection is anisotropic, so a
    // pixel across the screen is 0.71 of a pixel of ground while a pixel down it
    // is 1.41, and the world margin that comes out the far side of the AABB is a
    // fifth short in the worst direction. Measured, not reasoned about — the
    // first version of this asked for 1.357 m and delivered 1.097.
    //
    // This is also the half of the guard band that geometry alone never asks
    // for, and it fails in the nastiest way there is. A mark rooted up-light of
    // the page and outside the band is not clipped, it is never generated — so
    // its shadow is simply absent, on the pages whose casters happened to fall
    // outside. Nothing about the page looks wrong; a stripe of the world is just
    // missing its shade.
    //
    // Widened in every direction rather than only toward the sun. Doing it
    // one-sided would save a little area and would mean a sign that has to be
    // right, which is exactly the class of mistake that produces a defect nobody
    // notices for a month.
    let shade = Vec2::splat(caster_reach.max(0.0));
    (low - shade, high + shade)
}

/// Grow everything that stands up.
///
/// The mat goes down first. Not for correctness — the depth test would sort it
/// out either way — but because the mat's job is to be *buried*, and a buried
/// stroke contributes occlusion where one that wins its pixel does not.
pub fn plant(marks: &mut Vec<Stroke>, bed: &Bed) {
    // One cache for all three passes, so the mat's reads warm it for the tufts
    // standing in the same ground. See [`GroundCache`] for why a lattice is the
    // right resolution to make placement decisions at.
    let mut ground = GroundCache::new(bed.field, bed.page.px_per_metre);
    // The mat thickens exactly where the tufts thin out. Loosely described
    // ground is not *empty* ground — it is ground described as a mass instead of
    // as blades — and taking the tufts away without putting the mass in leaves
    // bald floor, which is worse than the carpet it was meant to fix.
    scatter(
        marks,
        bed,
        &mut ground,
        Stream::Thatch,
        bed.params.style.thatch,
        // Thinned hard over bare ground, on top of the coverage every pass
        // gets. The mat is the layer that actually closes a clearing: it is
        // short, there are three hundred of them to a square metre, and the
        // tuft pass thinning itself does nothing about them. An opening with a
        // full mat over it is an opening you cannot see.
        |ground| (1.20 - ground.resolution * 0.20) * (1.0 - bareness(ground.bare) * 0.55),
        |marks, page, draw, root, ground, params| {
            let stroke = mat_stroke(draw, root, ground, params);
            emit(marks, page, stroke);
        },
    );
    // The fine layer: the closed canopy the statement tufts stand *in*.
    //
    // This is the layer the renderer did not have, and its absence is most of
    // why the field read as strokes scattered on a floor rather than as grass.
    // The mat above it is drawn to be buried and shaded through the thatch ramp,
    // so it reads as floor however much of it there is; the tufts are sparse by
    // construction, because a tuft is a plant and plants have gaps between them.
    // Nothing was doing the job the reference art gives most of its area to —
    // thousands of short, fine, strongly combed blades forming a surface.
    //
    // Combed much harder than anything else in the field, and that is the point.
    // A tuft scatters widely around the flow because a plant does; this layer is
    // the *grain* of the meadow, and grain that wanders is not grain. It is also
    // what carries the flow field at a distance, where individual tufts have
    // stopped being resolvable.
    scatter(
        marks,
        bed,
        &mut ground,
        Stream::Fine,
        bed.params.style.fine,
        // Thickest where the tufts are thinnest, so a quiet passage is a
        // *smoother* canopy rather than a balder one.
        //
        // And it survives bare ground far better than the tufts do — 0.42 rather
        // than the 0.75 it was. This layer is the **transition**: a clearing that
        // loses its tufts and its short grass together has a hard edge, and the
        // soil inside it becomes a shape rather than a gap. Worse, a bare patch
        // sitting on a terrain mound with nothing growing on it stops reading as
        // ground at all and reads as a rock lying on the field, which is exactly
        // what it did.
        //
        // Keeping the short grass thins toward the middle of a clearing instead
        // of stopping at its edge, so the soil is glimpsed *through* something.
        |ground| (1.15 - ground.resolution * 0.30) * (1.0 - bareness(ground.bare) * 0.42),
        |marks, page, draw, root, ground, params| {
            let stroke = fine_stroke(draw, root, ground, params, params.seed);
            emit(marks, page, stroke);
        },
    );
    scatter(
        marks,
        bed,
        &mut ground,
        Stream::Blade,
        bed.params.style.tufts,
        // Wider than it was, now that this field runs mostly at the broad scale
        // rather than the mound scale. Thinning the tufts inside a single mound
        // does read as that patch being out of focus; thinning them across a
        // quarter of the view reads as a quieter passage of the same meadow,
        // and quiet passages are what the detailed ones are measured against.
        |ground| 0.60 + ground.resolution * 0.52,
        grow_tuft,
    );
    scatter(
        marks,
        bed,
        &mut ground,
        Stream::Leaf,
        bed.params.style.leaves,
        |ground| (0.35 + ground.resolution * 0.35) * ground.colony,
        leaf_cluster,
    );
}

/// The three things every planting pass needs: where it is, what grows there,
/// and how it looks.
pub struct Bed<'a> {
    pub page: &'a Page,
    pub field: &'a WorldField,
    pub params: &'a GrassParams,
}

impl Bed<'_> {
    /// How far up-light a mark can be rooted and still shade this page, metres.
    ///
    /// Derived from the sun rather than written down, because it is a function
    /// of the elevation — one over its tangent — and the elevation is a
    /// parameter. At the 35° this renderer is built for it is one and a half
    /// times the canopy's height; at 20° it would be nearly three times, and a
    /// constant sized for one would silently under-guard the other.
    pub fn caster_reach(&self) -> f32 {
        if self.params.quality.shadow_density() <= 0.0 {
            return 0.0;
        }
        let sun = crate::iso::image_to_world(self.params.light).normalize_or(Vec3::Z);
        // A sixteenth over, so that a later nudge to the sun or the canopy does
        // not need this recalculated on the same day. The band costs area and
        // the area is worth less than the defect.
        CANOPY_METRES * crate::geometry::reach_per_height(sun) * 1.0625
    }
}

/// How high anything in the field can stand, world metres.
///
/// The tallest mark the vocabulary can grow: the longest arc a `Tangle` reaches
/// at full vigour, at full tall-accent reach, on the leading flank of a piled
/// crown, standing near upright so almost all of that length becomes height. A
/// bound rather than a measurement, because the guard band has to be sized
/// before the field exists.
///
/// It costs real work to raise. The shadow guard is this times one over the
/// tangent of the sun's elevation, and the guard is area every scatter pass
/// walks on every page — so a metre added here is paid for a few hundred
/// thousand times.
///
/// [`tests::the_canopy_bound_is_never_beaten`] sweeps the vocabulary against
/// it rather than trusting this paragraph.
pub const CANOPY_METRES: f32 = 1.20;

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
    marks: &mut Vec<Stroke>,
    bed: &Bed,
    cache: &mut GroundCache,
    stream: Stream,
    per_square_metre: f32,
    weight: impl Fn(&Ground) -> f32,
    mut place: impl FnMut(&mut Vec<Stroke>, &Page, &mut Draw, Vec2, &Ground, &GrassParams),
) {
    let Bed { page, params, .. } = *bed;
    let spacing = (1.0 / per_square_metre.max(0.01)).sqrt();
    let (low, high) = footprint(page, bed.caster_reach());
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
            if !reaches_page(page, root) {
                continue;
            }
            let ground = cache.sample(root);
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
            let coverage = 1.0 - smoothstep(0.04, 0.88, bareness(ground.bare)) * 0.80;
            if !draw.chance((ground.density * coverage * weight(&ground)).min(1.0)) {
                continue;
            }
            place(marks, page, &mut draw, root, &ground, params);
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
fn emit(marks: &mut Vec<Stroke>, page: &Page, stroke: Stroke) {
    // Against this mark's own reach, not against [`MARGIN`]. The two differ by a
    // great deal and the difference is most of a page: `MARGIN` is sized for the
    // longest mark the vocabulary can produce, rooted at the far edge of the
    // widest tuft, and the ordinary mark is a fifth of that. Testing every
    // stroke against the worst case admits about three marks for every one that
    // can touch the page, and each of the other two walks its whole centreline
    // and every rib before the rasteriser discovers there was nothing to write.
    //
    // `MARGIN` still guards the *cell*, in [`scatter`], because a tuft's blades
    // are not drawn yet when that test runs.
    let reach = stroke.reach(page.px_per_metre);
    let at = page.to_pixel(stroke.root);
    if at.x < -reach
        || at.y < -reach
        || at.x > page.width as f32 + reach
        || at.y > page.height as f32 + reach
    {
        return;
    }
    marks.push(stroke);
}

/// Could something rooted here mark this page at all?
///
/// The margin is the longest reach any mark has, and it is generous on purpose:
/// rejecting a stroke that would have touched the page puts a straight line down
/// the join between two pages, which costs far more than drawing a few marks
/// that turn out to be invisible.
#[inline]
fn reaches_page(page: &Page, root: Vec2) -> bool {
    let margin = page.px(MARGIN);
    let at = page.to_pixel(root.extend(0.0));
    at.x >= -margin
        && at.y >= -margin
        && at.x <= page.width as f32 + margin
        && at.y <= page.height as f32 + margin
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
pub const MARGIN: f32 = 140.0;

/// Per-plant brightness, gathering the terms that vary plant to plant rather
/// than pixel to pixel.
fn plant_light(draw: &mut Draw, ground: &Ground, params: &GrassParams) -> f32 {
    params.style.base_light
        + draw.normal() * params.style.scatter
        + ground.crown * 0.02
        // Roots that overhang bare ground darken. Placement alone does not make
        // a patch read as a depression; this does.
        // Lifted, not lowered, over bare ground. A stroke lying across pale
        // earth at canopy brightness reads as a dark comma stuck to the soil;
        // the reference's clearings carry pale shoots, not dark ones.
        + ground.bare * 0.07
}

/// The dark mat: short, hooked, and almost entirely buried.
fn mat_stroke(draw: &mut Draw, root: Vec2, ground: &Ground, params: &GrassParams) -> Stroke {
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
        side_light: params.style.side_light * 0.6,
        under: params.style.under * 0.5 * (1.0 - ground.bare * 0.85),
        ..Default::default()
    }
}

/// The fine canopy: short, narrow, strongly combed, and meant to be seen.
///
/// Distinguished from the mat by what it is *for*. The mat is thatch — dark,
/// tangled, isotropic, shaded through a ramp that reads as floor, and drawn to
/// be buried. This stands up, takes the grass ramp, and closes the surface.
/// Between them they are the two things under a tuft that the old renderer
/// collapsed into one.
fn fine_stroke(
    draw: &mut Draw,
    root: Vec2,
    ground: &Ground,
    params: &GrassParams,
    seed: u64,
) -> Stroke {
    // Around the colony's heading, not the world's flow. This is the largest
    // population in the field by an order of magnitude, so it is the layer that
    // decides what the middle scale looks like from any distance — grain that
    // ignores its colony averages the colonies away.
    let azimuth = colony_of(seed, root, ground).heading + draw.normal() * 0.34;
    let width = draw.range(0.30, 0.72);
    Stroke {
        root: root.extend(0.0),
        azimuth,
        // Short. This layer is a surface, and a surface made of long marks is
        // a surface made of objects.
        length: draw.range(0.048, 0.105) * (0.80 + ground.density * 0.40),
        bend: draw.range(0.55, 1.25),
        curl: draw.range(0.0, 0.5),
        sway: draw.signed() * 0.35,
        width,
        tip_width: 0.22,
        profile: Profile::Leaf,
        // Enough twist to break the comb without loosening it. Fine blades are
        // nearly round in section, so this is at the bottom of the range.
        twist: draw.signed() * 0.5,
        tip: TipProfile::Pointed,
        maturity: draw.range(0.15, 0.55),
        tone: Tone::Grass,
        base_light: (params.style.base_light - 0.05
            + draw.normal() * 0.07
            + ground.crown * 0.02
            + ground.bare * 0.10)
            .clamp(0.05, 0.95),
        tip_light: params.style.tip_light * draw.range(0.5, 0.9),
        // A tenth of the rate the statement blades get. This layer is area, and
        // a highlight that appears on a tenth of the area is a texture rather
        // than an accent.
        glint: if draw.chance(0.012 * ground.resolution) {
            params.style.glint * draw.range(0.5, 0.9)
        } else {
            0.0
        },
        side_light: params.style.side_light,
        // Halved. The under-stroke separates one blade from the next, and at
        // this density full-strength separation turns the layer into a woven
        // mesh — every blade outlined, which is exactly the fur reading.
        under: params.style.under * 0.45 * (1.0 - ground.bare * 0.85),
        ..Default::default()
    }
}

/// How bare a patch of ground is allowed to actually get.
///
/// The field's `bare` runs to one, meaning nothing grows. Nothing growing turns
/// out to be a shape rather than an absence: a broad smooth expanse of lit soil
/// sitting on a terrain mound stops reading as ground glimpsed between plants
/// and starts reading as an *object* lying on the field — a rock, most often,
/// which is a thing this world does not have yet.
///
/// So the top of the range is compressed. A clearing still reads as a clearing;
/// it simply always has something growing in it, and the soil is always seen
/// *through* grass rather than instead of it.
#[inline]
fn bareness(bare: f32) -> f32 {
    // Nothing above this, however bare the field says the ground is.
    const CEILING: f32 = 0.72;
    (bare * CEILING).clamp(0.0, CEILING)
}

/// How far one tuft may stray from its colony's heading, radians.
///
/// About seventeen degrees, and the number has been wrong in both directions.
///
/// It started at forty — each tuft scattering around the world flow on its own —
/// and no colony could form, because agreement never survived one plant. Pulling
/// it to seven built the colonies but overshot: the field turned into broad
/// directional waves, read as fur or combed reeds rather than as tufted grass,
/// and lost the sense that each clump is a plant with its own mind.
///
/// The band that works is narrow. A colony has to hold its direction across
/// enough plants for the eye to group them, while each plant still visibly
/// decides for itself.
const COLONY_SPREAD: f32 = 0.30;

/// How wide a colony is, in world metres.
///
/// Two and a quarter metres, which at the scale the art is judged at puts a
/// colony somewhere between a hundred and three hundred pixels across — the band
/// the reference organises itself in, and the one this renderer had nothing at.
const COLONY_METRES: f32 = 2.25;

/// How much longer a vigour mass is along the flow than across it.
///
/// Three. An elongated mass carries its direction in its *area*, which is what
/// survives being minified to a gameplay camera; a round one carries direction
/// only in the blades inside it, which does not. Applied to noise rather than to
/// a cell grid — see the note in [`colony_of`] for why that distinction is the
/// difference between streaks and visible chunk boundaries.
const COLONY_ELONGATION: f32 = 3.0;

/// A group of tufts that grow as one mass.
#[derive(Clone, Copy)]
struct Colony {
    /// The direction every tuft in it leans, world radians.
    heading: f32,
    /// How well this colony is doing, about `0.72..1.30`.
    ///
    /// Multiplies blade length and, through it, how much light the colony
    /// catches. **This is the variation that survives being looked at from far
    /// away**, and that is why it exists.
    ///
    /// An overview shows about forty pixels to the metre, where a blade is a
    /// fifth of a pixel. Everything happening at blade scale — the lit facet,
    /// the tip highlight, the fold — is averaged out long before it reaches the
    /// screen: measured, a tenfold minification takes the highlight share from
    /// eight percent to three tenths of one. Contrast at *colony* scale is
    /// spread over hundreds of pixels and survives intact.
    ///
    /// So the surface reads at distance because whole masses differ from each
    /// other, not because individual blades do. That is also how the genre draws
    /// grass, and it is not a cheat — a meadow really does have lush stretches
    /// and tired ones, and this is the scale at which anyone standing back sees
    /// them.
    vigour: f32,
}

/// Which colony a point belongs to, and which way it runs.
///
/// A pure function of world position, like everything else in this module, so
/// two pages that have never met put the same tuft in the same colony and agree
/// about its heading. Deriving this from anything page-local would put a visible
/// join wherever two pages met — and it would be a *change of texture* rather
/// than a step, which is the kind that survives every seam test.
///
/// ## The cell edge has to be soft, and the first version's was not
///
/// The heading is interpolated between the four nearest cell centres rather than
/// read from whichever cell the point falls in. A hard lookup was tried first,
/// on the reasoning that only a *direction* comes from the cell so a boundary
/// could not show. That reasoning was wrong in a way worth recording.
///
/// Two adjacent colonies can differ by most of a radian. With a hard edge, the
/// tufts on either side of the line lean away from each other, and grass that
/// leans apart does not interleave — it opens. The result was a network of
/// near-black fissures following the cell grid, several blade-lengths deep,
/// which reads as holes burned in the canopy rather than as shadow. It was the
/// single worst-looking thing in the field and it came from a boundary that was
/// supposed to be invisible.
///
/// Smoothstep weights rather than linear, so the blend has no derivative
/// discontinuity at the cell centres either — a linear blend of directions turns
/// fastest exactly where it crosses a centre, which reads as a crease.
fn colony_of(seed: u64, root: Vec2, ground: &Ground) -> Colony {
    // ## The grid is not warped, and that was tried
    //
    // Stretching the cells along the flow to make elongated colonies is the
    // obvious way to carry direction at a distance, and it fails twice.
    //
    // It does not work: four-to-one elongation took overview coherence *down*
    // from 0.183 to 0.153, because a longer cell means fewer boundaries in
    // frame and it is the boundaries between differing masses that a gradient
    // actually sees.
    //
    // And it introduces a defect. The warp reads `ground.flow`, which varies
    // with position, so two neighbouring points can land in genuinely different
    // cells — the blend cannot smooth over a discontinuity in *which cells it is
    // blending*. What that draws is short straight or L-shaped breaks in density
    // and direction, which read as chunk boundaries rather than as anything a
    // meadow does, and are exactly the kind of artefact that gets hunted for in
    // the compositing.
    //
    // So the grid stays axis-aligned and the blend does its job.
    let grid = root / COLONY_METRES;
    let base = grid.floor();
    let fraction = grid - base;
    // Smoothstep, so the weights meet flat at both ends.
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let (fx, fy) = (ease(fraction.x), ease(fraction.y));

    // Headings are angles, so they are blended as unit vectors. Averaging
    // radians directly puts the mean of 350° and 10° at 180°, which would turn
    // the very boundary this is smoothing into a hard reversal.
    let mut sum = Vec2::ZERO;
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let cell = base + Vec2::new(dx as f32, dy as f32);
        let mut draw = Draw::at(seed, Stream::Colony, cell.x as i32, cell.y as i32);
        let heading = ground.flow + draw.signed() * 0.75;
        let weight = (if dx == 0 { 1.0 - fx } else { fx }) * (if dy == 0 { 1.0 - fy } else { fy });
        sum += Vec2::from_angle(heading) * weight;
    }

    // ## Vigour comes from continuous noise, not from cells
    //
    // The heading can live on a cell grid because a four-cell blend genuinely
    // smooths it. Vigour cannot, and the difference is worth understanding: to
    // make the masses *directional* — which is the only thing that carries
    // direction to an overview, where every blade is a fifth of a pixel — the
    // sampling frame has to be stretched along the flow. Stretch a *cell grid*
    // that way and the cell coordinate stops being monotonic in position
    // wherever the flow turns quickly, so neighbouring points land in cells that
    // are not neighbours. A blend cannot smooth a discontinuity in *which cells
    // it is blending*, and what it draws instead is short straight and L-shaped
    // breaks in density that read as chunk boundaries.
    //
    // Noise has no cell identity to fold, so the same stretch applied to it is
    // simply an elongated field: continuous everywhere, directional, and with no
    // grid to leak through. Two octaves, because a third puts detail back at the
    // blade scale where it cannot survive the minification anyway.
    let flow = Vec2::from_angle(ground.flow);
    let along = root.dot(flow) / (COLONY_METRES * COLONY_ELONGATION);
    let across = root.dot(Vec2::new(-flow.y, flow.x)) / COLONY_METRES;
    let drift = crate::rng::fbm(seed, Stream::Colony, along, across, 2);
    let vigour = COLONY_VIGOUR.0 + drift * (COLONY_VIGOUR.1 - COLONY_VIGOUR.0);

    Colony {
        heading: sum.normalize_or(Vec2::from_angle(ground.flow)).to_angle(),
        vigour,
    }
}

/// How far a colony's vigour may run from the mean.
///
/// Nearly a factor of two between the best and worst stretches. That is a large
/// range and it is chosen against the *overview* rather than against botany:
/// blade-scale contrast does not survive minification and colony-scale contrast
/// does, so this is where the surface has to get its legibility from. Narrow it
/// and a field seen from the game camera goes uniform.
const COLONY_VIGOUR: (f32, f32) = (0.62, 1.38);

/// Straight down the screen, as a world azimuth.
///
/// A world step of `(dx, dy)` moves `(dx - dy)` across the screen and `(dx + dy)`
/// halved down it, so the direction that runs straight down the screen with no
/// sideways component at all is the one where `dx == dy` — a quarter turn. It is
/// the only direction in this projection that means anything to the viewer
/// rather than to the world, which is what makes it the right one to lay a skirt
/// along.
pub const DOWN_SCREEN: f32 = std::f32::consts::FRAC_PI_4;

/// How far from its centre a tuft may root a blade, metres.
///
/// Named rather than written into the draw because both guard-band tests have to
/// add it to the reach they measure, and a copy of it in a test is a copy that
/// goes stale silently — the test then certifies a band as sufficient for a
/// narrower tuft than the one the baker actually grows.
pub const TUFT_RADIUS: f32 = 0.185;

/// The most a vigorous mound can lengthen the grass standing on it.
///
/// Named because three guard-band tests have to reach the same number, and a
/// copy of it in a test is a copy that goes stale silently. One of them had:
/// the clamp read 1.45 and the test read 1.35, so the band was certified against
/// a mark seven percent shorter than the field can actually grow, and the
/// symptom of that being wrong is a stroke present on one side of a page join
/// and missing on the other.
pub const VIGOUR_CEILING: f32 = 1.45;

/// Extra bend a skirt blade is laid over by, at most, radians.
///
/// Only here so the guard-band test can sweep to the same limit the baker
/// reaches. See the skirt in [`grow_tuft`].
pub const SKIRT_BEND: f32 = 0.75;

/// The most a tiller's structural role can add to a mark's bend, radians.
///
/// [`Role::Perimeter`] is the outlier: it exists to lay the skirt over, and the
/// skirt is what the tuft sits on. Named for the same reason [`SKIRT_BEND`] is —
/// three guard-band tests have to sweep to the limit the baker actually reaches,
/// and a copy of the number in a test is a copy that goes stale silently.
///
/// The two stack. A perimeter blade that is *also* turned down-screen takes both,
/// so the vocabulary's true bend ceiling is a family's own maximum plus
/// `ROLE_LEAN + SKIRT_BEND`, and that is what
/// [`the baker's tests::the_placement_rectangle_covers_every_direction_a_mark_reaches`]
/// has to sweep to. Getting this wrong does not clip a blade — it puts a
/// straight line down every page join.
pub const ROLE_LEAN: f32 = 0.85;

/// The largest bend any mark in the vocabulary can end up with, radians.
///
/// The `Tangle` family's own ceiling, plus the lodging a clearing adds, plus
/// both structural adjustments. Well past a right angle, which is the point:
/// those marks lie along the ground and double back, and how far they reach is
/// not something arc length alone can answer.
pub const BEND_CEILING: f32 = 2.0 + 0.22 + ROLE_LEAN + SKIRT_BEND;

/// One tuft: a crown of shoot bundles that agree with each other.
///
/// ## Why there is a layer between the tuft and the blade
///
/// A tuft used to be a handful of blades scattered in a disc around a point.
/// That is one level of organisation, and it is one fewer than grass has. Real
/// grass grows in **tillers** — small shoot bundles, each a fan of three to six
/// related leaves sharing a root, a direction and an age — and a tuft is a
/// crowd of tillers rather than a crowd of leaves.
///
/// The difference is visible and it is not subtle. Blades scattered
/// independently in a disc give a rosette: every blade equidistant from its
/// neighbours, no internal grouping, an outline and nothing inside it. Blades
/// grouped into fans give the reference's reading — dense knots of parallel
/// leaves, gaps between the knots, and a silhouette made of overlapping small
/// masses rather than one smooth arc.
///
/// ```text
///   tuft            an irregular multi-lobed crown, 0.1–0.2 m across
///     └── tiller    a shoot bundle sharing a root and a heading
///           └── blade
/// ```
///
/// ## Why the footprint is lobed rather than elliptical
///
/// An ellipse has one centre, so density falls off monotonically from it and the
/// tuft reads as a hedgehog — thickest in the middle, thinning evenly outward,
/// with a smooth outline. Real clumps are lumpy: they have two or three centres
/// of vigour, the gaps between them show floor, and the outline has bays in it.
/// Two to four overlapping lobes combined with a p-norm gives that for almost
/// nothing, and the combination matters — summing the lobes averages them into
/// one blob, while a smooth maximum keeps each one's own shoulder.
///
/// ## The four structural roles
///
/// Where a tiller sits in the crown decides what grows there, and the shares are
/// what stop a tuft reading as hair on a disc:
///
/// | Role | Share | Behaviour |
/// | --- | ---: | --- |
/// | Core | 15% | Tall, near upright, narrow |
/// | Body | 50% | Long, curved, most of the volume |
/// | Perimeter | 25% | Shorter, strongly outward — the skirt |
/// | Accent | 10% | Broad, twisted, forked, brighter |
///
/// The perimeter is the load-bearing one. Without a skirt a tuft is hair growing
/// vertically out of a flat disc, and no amount of lighting makes it look
/// planted; the low outward blades hide the root mass and give the crown
/// something to sit on. A minority of them are turned down-screen as well,
/// which in a fixed isometric view is the cheapest depth cue there is.
fn grow_tuft(
    marks: &mut Vec<Stroke>,
    page: &Page,
    draw: &mut Draw,
    centre: Vec2,
    ground: &Ground,
    params: &GrassParams,
) {
    // Height follows the mound. A mound whose blades are the same length as the
    // hollow beside it is not a mound, it is a stain. Weighted away from the
    // crown and toward the clump fields, because blade length that tracks relief
    // closely makes every raised place taller *and* thicker *and* brighter, and
    // three fields saying one thing is how a surface starts reading as its own
    // height map.
    //
    // Centred, so the mean length is unchanged and only its spread grows. A
    // multiplier that ran from one downward would quietly shave the whole canopy
    // and would show up in the comparison as an exposure fault rather than as
    // the organisation it is.
    let vigour = ((0.16 + ground.crown * 0.30 + ground.density * 0.80)
        * (1.0 - ground.bare * 0.62)
        * (0.76 + ground.resolution * 0.44))
        .clamp(0.24, VIGOUR_CEILING);
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
    // strongest — so those regions come out as a soft dark mass with no incident
    // in them at all. A broad dark area is only wrong while it is *featureless*:
    // put a few lit tufts in it and the same darkness becomes depth, because now
    // there is something at the front for it to be behind.
    let spark = draw
        .chance((0.04 + (1.0 - ground.resolution) * 0.045 - ground.tint * 0.03).clamp(0.0, 1.0));
    if spark {
        // Standing a little proud matters as much as being brighter: the glaze
        // is keyed on canopy height, so a mark that does not clear the mass
        // around it gets averaged straight back into the mass it was meant to
        // break up.
        reach = reach.max(draw.range(1.12, 1.3));
    }

    // Along the colony's heading, tightly — and the colony along the flow,
    // loosely. That two-step is the whole of the middle scale.
    //
    // A uniform heading over the whole circle is isotropic, and isotropic grass
    // has no direction for the eye to travel along. But scattering every tuft
    // *independently* around the flow is barely better: at ±0.7 radians two
    // neighbours can disagree by eighty degrees, so agreement never survives
    // more than one plant and the middle scale has nothing to say but the
    // outline of each clump. Measured, that is a directional coherence of 0.44
    // against reference art's 0.51, and it is what reads as mottle.
    //
    // So the wide scatter moves up a level, to the colony, and what is left at
    // the tuft is a tenth of it. Tufts sharing a colony now agree closely enough
    // for the eye to group them into one mass with a direction; colonies still
    // differ from each other as much as tufts used to.
    let colony = colony_of(params.seed, centre, ground);
    // Three kinds of plant, and the two minorities are what stop a colony
    // reading as brushed fibre.
    //
    // A field where every tuft obeys its colony is a comb however good the
    // colony structure is — the flow becomes one continuous bend and the eye
    // reads fur, carpet or seaweed rather than plants. What breaks it is not
    // more noise; noise averages back into the mean. It is a *minority that
    // disagrees structurally*: some tufts standing across the flow, and some
    // standing up out of it entirely.
    let dissent = draw.unit();
    let heading = if dissent < 0.04 {
        // Across the colony, near a right angle. Few, and deliberately not
        // random — a tuft at ninety degrees to its neighbours interrupts the
        // band, where one at a random angle just blurs it.
        colony.heading + std::f32::consts::FRAC_PI_2 * draw.signed().signum() + draw.signed() * 0.35
    } else if dissent < 0.09 {
        colony.heading + draw.signed() * 1.2
    } else {
        colony.heading + draw.signed() * COLONY_SPREAD
    };
    // And a minority that leans hardly at all. A flow is only legible against
    // something upright; with nothing standing straight the whole field looks
    // combed rather than blown.
    let upright = draw.chance(0.08);
    // The colony's own vigour, on top of everything the mound field already
    // said. This is the term that makes one stretch of meadow read as lusher
    // than the next from across the map.
    reach *= colony.vigour;
    let flow = Vec2::from_angle(heading);
    let shade = plant_light(draw, ground, params) - params.style.base_light;
    let maturity = (ground.resolution * 0.6 + ground.density * 0.3 + draw.unit() * 0.35).min(1.0);

    // Everything below draws from its own sequence rather than continuing the
    // tuft's. The tuft's draws decide where the crown is and how vigorous it is,
    // and those answers must not move every time the *internal* structure gains
    // a parameter — otherwise every refinement to a tiller reshuffles the whole
    // world. Seeded from the tuft, so it is still a pure function of position.
    let mut inner = Draw::from_seed(draw.seed() ^ ((Stream::Tiller as u64) << 48));

    let crown = Crown::grow(&mut inner, ground, flow);

    // The dark under-canopy goes down first, and it goes down *before* anything
    // that will stand over it. Not for correctness — the depth test sorts it out
    // either way — but because its job is to be buried, and a mark that loses its
    // pixel still darkens the interior it lost it in.
    //
    // This is what makes a tuft read as dense rather than as painted a darker
    // green. The interior is genuinely occluded because there is genuinely
    // something in it.
    let understorey = 5 + inner.index(7);
    for _ in 0..understorey {
        let local = crown.sample(&mut inner);
        let outward = local.normalize_or(flow);
        emit(
            marks,
            page,
            Stroke {
                root: (centre + local).extend(0.0),
                azimuth: outward.to_angle() + inner.signed() * 0.9,
                length: crown.height * vigour * inner.range(0.18, 0.42),
                // Laid over hard, so it hugs the floor and roofs the root mass
                // rather than standing among the blades above it.
                bend: inner.range(1.15, 1.85),
                curl: inner.range(0.0, 0.9),
                width: inner.range(0.55, 1.05),
                tip_width: 0.24,
                profile: Profile::Tapered,
                tone: Tone::Thatch,
                base_light: (params.style.base_light - inner.range(0.10, 0.22)).max(0.05),
                tip_light: 0.06,
                glint: 0.0,
                side_light: params.style.side_light * 0.5,
                under: params.style.under * 0.4,
                twist: inner.signed() * 0.4,
                ..Default::default()
            },
        );
    }

    // Tiller roots, placed by best candidate rather than by pure jitter. Even
    // spacing is wrong and pure randomness is wrong: real shoots crowd where the
    // clump is vigorous and leave gaps elsewhere, and a fifth of them come in
    // pairs a few millimetres apart. Scoring density against separation gets all
    // three from one loop.
    let tillers = 6 + inner.index(9);
    let mut roots = [Vec2::ZERO; MAX_TILLERS];
    let mut placed = 0usize;
    for _ in 0..tillers.min(MAX_TILLERS) {
        let mut best = Vec2::ZERO;
        let mut best_score = f32::NEG_INFINITY;
        for _ in 0..TILLER_CANDIDATES {
            let candidate = crown.sample(&mut inner);
            let density = crown.envelope(candidate);
            // Distance to the nearest root already accepted, in units of the
            // spacing the clump wants.
            let separation = roots[..placed]
                .iter()
                .map(|root| root.distance(candidate))
                .fold(f32::INFINITY, f32::min)
                / crown.spacing;
            // Weighted so density leads and separation corrects. The other way
            // round gives an evenly spaced ring, which is the failure this is
            // here to avoid.
            let score = density * 1.5 + separation.min(2.0) + inner.unit() * 0.35;
            if score > best_score {
                best_score = score;
                best = candidate;
            }
        }
        roots[placed] = best;
        placed += 1;
    }

    for root in &roots[..placed] {
        let radius = crown.radius_of(*root);
        let role = Role::at(radius, &mut inner);
        // Where this tiller points, blended from four things that each say
        // something different. The centre of a clump follows the shared flow;
        // its edge fans outward; the nearest lobe pulls a little; and a swirl
        // stops the whole tuft combing.
        //
        // Blended rather than chosen, because each on its own is a recognisable
        // failure — pure flow is a comb, pure radial is a rosette, pure swirl is
        // a whirlpool.
        let outward = crown.outward(*root);
        let swirl = Vec2::new(-outward.y, outward.x) * crown.swirl;
        let direction = (flow * (0.65 - radius * 0.30)
            + outward * (0.10 + radius * 0.30)
            + swirl * 0.10
            + Vec2::new(inner.signed(), inner.signed()) * 0.08)
            .normalize_or(flow);
        let tiller_heading = direction.to_angle();

        // A fan of related leaves rather than one blade. The dominant leaf is
        // full length and the rest are graded down, which is what makes a bundle
        // read as one plant of a certain age instead of as several plants that
        // happen to be touching.
        let blades = 3 + inner.index(4);
        let fan = inner.range(0.21, 0.70);
        for blade in 0..blades {
            let across = if blades > 1 {
                blade as f32 / (blades - 1) as f32 * 2.0 - 1.0
            } else {
                0.0
            };
            // Graded lengths within the bundle. Not random: a bundle has one
            // mature leaf, a couple of half-grown ones and a short new shoot,
            // and stratifying by index rather than drawing independently is what
            // keeps that reading from tuft to tuft.
            let age = 1.0 - (blade as f32 / blades as f32) * inner.range(0.35, 0.62);

            // How high this side of the crown stands. The leaning flank grows
            // taller and stays more upright; the trailing one is shorter and lies
            // further over, which is what turns a dome into something with a front
            // and a back.
            let pile = crown.pile(*root);

            let mut stroke = pick_mark(&mut inner, ground, role).shape(&mut inner, params, ground);
            // A tiny offset within the bundle, so the leaves share a root
            // without being coincident.
            let jitter = Vec2::new(inner.signed(), inner.signed()) * crown.spacing * 0.18;
            stroke.root = (centre + *root + jitter).extend(0.0);
            stroke.azimuth = tiller_heading + across * fan + inner.signed() * 0.09;
            // The pile moves length at half strength and bend at full. Height
            // is the expensive half — every metre of canopy widens the shadow
            // guard band on every page — and lying over is the half that
            // actually reads, because a trailing skirt is a silhouette rather
            // than a measurement.
            stroke.length *=
                vigour * reach * role.length(&mut inner) * age * (1.0 + (pile - 1.0) * 0.5);
            stroke.bend += role.lean(&mut inner) + (1.0 - pile) * 1.1;
            if upright {
                // A tuft that stands. Bend is what turns a plant into a stroke
                // lying along the flow, so taking most of it back is what makes
                // this one read as standing *in* the field rather than being
                // swept through it — and a flow is only legible against
                // something that is not obeying it.
                stroke.bend *= 0.38;
            }
            stroke.width *= role.width(&mut inner) * (0.72 + age * 0.38);
            stroke.twist *= role.twist();

            if role == Role::Accent {
                stroke.tip_light *= 1.25;
            } else if role != Role::Core && inner.chance(0.13) {
                // A minority of the skirt is turned toward the viewer and laid
                // well over. A fixed three-quarter camera has a front and a
                // back, and a tuft whose blades radiate evenly has neither — what
                // says "in front of" rather than "beside" is one thing lying over
                // another.
                //
                // A minority, and *within* a tuft rather than across the field.
                // Applied globally the same idea is a comb, and a combed field is
                // a worse failure than a flat one.
                stroke.azimuth = DOWN_SCREEN + inner.signed() * 0.6;
                stroke.bend += inner.range(0.3, SKIRT_BEND);
            }

            // Blades within a tuft differ as much as tufts differ from each
            // other, but *less* where the ground is quiet — that is the half of
            // the intensity classes that decides whether a passage reads as a
            // canopy or as a collection of blades. Nothing is taken away to get
            // it: the same marks are drawn, they simply stop arguing with their
            // neighbours.
            stroke.base_light = (stroke.base_light
                + shade
                + inner.normal() * 0.085 * (0.62 + ground.resolution * 0.76))
                .clamp(0.05, 0.95);
            stroke.maturity = maturity * age;
            if spark {
                // Applied after the clamp's inputs are gathered rather than
                // folded into `shade`, because a spark has to survive a dim
                // neighbourhood rather than be averaged with it — and it has to
                // catch the light whether or not this particular mark drew a
                // glint.
                stroke.base_light = (stroke.base_light + 0.03).min(0.95);
                stroke.glint = stroke
                    .glint
                    .max(params.style.glint * inner.range(0.75, 1.15));
                stroke.tip_light *= 1.45;
            }
            emit(marks, page, stroke);
        }
    }
}

/// The most tillers one tuft may hold.
///
/// A fixed array rather than a `Vec`, because this runs a few hundred times per
/// page and the allocation showed up. Sized to the draw's own ceiling.
const MAX_TILLERS: usize = 15;

/// How many places a tiller root is offered before one is accepted.
///
/// Eight. Best-candidate sampling converges on blue noise as this rises and gets
/// no better past about a dozen; the cost is linear in it and the clump is small
/// enough that the difference between eight and twelve is invisible.
const TILLER_CANDIDATES: usize = 8;

/// The lumpy footprint a tuft's shoots are distributed inside.
struct Crown {
    /// Two to four overlapping lobes, in tuft-local metres.
    lobes: [Lobe; MAX_LOBES],
    count: usize,
    /// The bounding ellipse, tuft-local metres.
    radius: Vec2,
    /// Which way the crown piles up, and how hard.
    ///
    /// A clump is not a dome. It grows into its own flow, so one flank stands
    /// high and the opposite one trails away — and *that* asymmetry is what
    /// gives a tuft a lit side and a dark side under a fixed sun. A symmetric
    /// crown has neither: every flank of it faces the light equally, so the
    /// whole thing reads as uniformly bright and the field turns into a carpet
    /// with brighter patches rather than a field of plants.
    ///
    /// Derived from the tuft's own flow, never from the sun. Which tufts catch
    /// the light and which do not is then a property of where they grew, and
    /// turning the key relights the field instead of regrowing it.
    lean: Vec2,
    /// How tall the blades on it want to be, metres.
    height: f32,
    /// How far apart the shoots want to sit, metres.
    spacing: f32,
    /// How much the tillers turn about the crown's centre.
    swirl: f32,
}

const MAX_LOBES: usize = 4;

#[derive(Clone, Copy, Default)]
struct Lobe {
    centre: Vec2,
    radius: Vec2,
    density: f32,
}

impl Crown {
    fn grow(draw: &mut Draw, ground: &Ground, flow: Vec2) -> Self {
        // The footprint, well inside the guard band's `TUFT_RADIUS`. The lobes
        // are offset within it and every one of them has to stay inside, or a
        // tuft could root a blade further out than the band allows for — which
        // is a mark present on one side of a page join and missing on the other.
        let span = draw.range(0.55, 1.0);
        let aspect = draw.range(0.60, 1.0);
        let radius = Vec2::new(TUFT_RADIUS * span, TUFT_RADIUS * span * aspect);

        // Which way this crown piles up. Along its own flow, with enough spread
        // that neighbouring tufts do not all lean the same way — a field of
        // identically leaning clumps is a comb at a larger scale.
        let lean = Vec2::from_angle(flow.to_angle() + draw.signed() * 0.8);

        let count = 2 + draw.index(MAX_LOBES - 1);
        let mut lobes = [Lobe::default(); MAX_LOBES];
        for (index, lobe) in lobes.iter_mut().enumerate().take(count) {
            // The first lobe sits near the middle and carries the tuft; the rest
            // are offset and weaker. A clump with several equal centres reads as
            // several clumps.
            let (offset, size, density) = if index == 0 {
                (0.12, 0.80, 1.0)
            } else {
                (LOBE_OFFSET, LOBE_SIZE, draw.range(0.55, 0.95))
            };
            let angle = draw.range(0.0, std::f32::consts::TAU);
            // Pulled toward the lean, so the satellite mass gathers on one
            // flank instead of ringing the middle.
            let direction = (Vec2::from_angle(angle) + lean * LOBE_LEAN).normalize_or(Vec2::X);
            *lobe = Lobe {
                centre: direction * radius * offset * draw.unit().sqrt(),
                radius: radius * size * draw.range(0.75, 1.0),
                density,
            };
        }

        let height = ground.density.mul_add(0.06, 0.16);
        Self {
            lobes,
            count,
            radius,
            lean,
            height,
            // Denser in a vigorous clump. Around a centimetre and a half, which
            // is what a shoot bundle actually occupies.
            spacing: (0.024 - ground.density * 0.008).max(0.010),
            swirl: draw.signed() * 0.22,
        }
    }

    /// How much crown there is at a tuft-local point, `0..1`.
    ///
    /// A p-norm rather than a sum. Summing the lobes averages them into one
    /// smooth blob and throws away the whole reason there is more than one;
    /// a smooth maximum keeps each lobe's own shoulder, so the outline has bays
    /// in it and the interior has two or three centres of vigour.
    fn envelope(&self, local: Vec2) -> f32 {
        let mut total = 0.0f32;
        for lobe in &self.lobes[..self.count] {
            let offset = (local - lobe.centre) / lobe.radius.max(Vec2::splat(1.0e-4));
            let inside = (1.0 - offset.length_squared()).max(0.0);
            let value = inside * inside.sqrt() * lobe.density;
            total += value * value * value * value;
        }
        total.sqrt().sqrt().min(1.0)
    }

    /// A point drawn from the crown, biased toward where the crown is.
    fn sample(&self, draw: &mut Draw) -> Vec2 {
        // Rejection against the envelope, with a hard cap so a thin crown cannot
        // spin. Four tries lands inside the lobes the overwhelming majority of
        // the time, and the fallback — the last candidate, wherever it fell — is
        // still inside the bounding ellipse and so still inside the guard band.
        let mut candidate = Vec2::ZERO;
        for _ in 0..4 {
            let angle = draw.range(0.0, std::f32::consts::TAU);
            // Square root of a uniform fills the disc evenly instead of piling
            // up at the centre.
            candidate = Vec2::from_angle(angle) * self.radius * draw.unit().sqrt();
            if draw.unit() < self.envelope(candidate) {
                return candidate;
            }
        }
        candidate
    }

    /// How far out a local point sits, `0..1` across the bounding ellipse.
    fn radius_of(&self, local: Vec2) -> f32 {
        (local / self.radius.max(Vec2::splat(1.0e-4)))
            .length()
            .min(1.0)
    }

    /// How much taller a blade at this local point should stand, as a multiple.
    ///
    /// The high flank and the trailing skirt, in one number. Nothing else in the
    /// tuft needs to know which way the crown leans; every blade just asks how
    /// tall it is where it stands, which is what keeps the asymmetry a property
    /// of the shape rather than a special case in the blade loop.
    fn pile(&self, local: Vec2) -> f32 {
        let along = local.normalize_or_zero().dot(self.lean);
        let out = self.radius_of(local);
        // Only felt away from the middle — a clump's centre is its centre
        // whichever way it leans, and scaling there would just make the whole
        // tuft taller or shorter.
        1.0 + along * out * CROWN_PILE
    }

    /// Which way is outward from the nearest lobe's centre.
    ///
    /// From the *lobe* rather than from the tuft, so shoots on a satellite lobe
    /// fan away from their own mass instead of all pointing away from a centre
    /// they are nowhere near.
    fn outward(&self, local: Vec2) -> Vec2 {
        let mut best = Vec2::ZERO;
        let mut nearest = f32::INFINITY;
        for lobe in &self.lobes[..self.count] {
            let distance = local.distance_squared(lobe.centre);
            if distance < nearest {
                nearest = distance;
                best = lobe.centre;
            }
        }
        (local - best).normalize_or(Vec2::X)
    }
}

/// How far a satellite lobe's centre may sit from the tuft's, as a fraction of
/// the bounding radius.
const LOBE_OFFSET: f32 = 0.30;
/// How hard a satellite lobe is pulled onto the crown's leaning flank.
const LOBE_LEAN: f32 = 0.55;

/// How much taller the leaning flank of a crown stands than its trailing one.
///
/// Nearly half again at the rim, which sounds like a great deal and is what the
/// difference between a plant and a patch costs. A crown that varies by a tenth
/// reads as a slightly uneven dome; one that varies by a half has a *top* and a
/// *back*, and only the second one catches the light on one side.
const CROWN_PILE: f32 = 0.45;

/// How large a satellite lobe may be, likewise.
///
/// `LOBE_OFFSET + LOBE_SIZE` must not exceed one, or a lobe reaches outside the
/// bounding ellipse that [`TUFT_RADIUS`] — and therefore the page guard band —
/// is sized against. `tests::a_crown_stays_inside_the_guard_band` measures it
/// rather than trusting this sentence.
const LOBE_SIZE: f32 = 0.70;

/// What a shoot bundle is for, decided by where in the crown it sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// Tall, near upright, narrow. The spine of the clump.
    Core,
    /// Long and curved. Most of the volume.
    Body,
    /// Short, strongly outward. The skirt that hides the roots.
    Perimeter,
    /// Broad, twisted, brighter. Punctuation.
    Accent,
}

impl Role {
    /// Chosen mostly by radius, with enough overlap that the roles do not print
    /// as concentric rings.
    fn at(radius: f32, draw: &mut Draw) -> Self {
        // Drawn independently of radius, so accents appear anywhere in the crown
        // — an accent that only ever occurred at one depth would read as a ring
        // of hero blades.
        if draw.chance(0.10) {
            return Role::Accent;
        }
        // The blur is what keeps the boundaries from printing. A tiller at 0.5
        // could be body or perimeter, and which one is a coin weighted by how
        // far out it actually is.
        let blurred = radius + draw.signed() * 0.18;
        if blurred < 0.36 {
            Role::Core
        } else if blurred < 0.74 {
            Role::Body
        } else {
            Role::Perimeter
        }
    }

    /// Length multiplier.
    fn length(self, draw: &mut Draw) -> f32 {
        match self {
            Role::Core => draw.range(0.90, 1.15),
            Role::Body => draw.range(0.78, 1.10),
            Role::Perimeter => draw.range(0.46, 0.80),
            Role::Accent => draw.range(0.92, 1.22),
        }
    }

    /// Extra lean from vertical, radians.
    fn lean(self, draw: &mut Draw) -> f32 {
        match self {
            Role::Core => draw.range(-0.20, 0.10),
            Role::Body => draw.range(0.05, 0.42),
            Role::Perimeter => draw.range(0.35, ROLE_LEAN),
            Role::Accent => draw.range(0.10, 0.50),
        }
    }

    /// Width multiplier.
    fn width(self, draw: &mut Draw) -> f32 {
        match self {
            Role::Core => draw.range(0.70, 0.95),
            Role::Body => draw.range(0.85, 1.15),
            Role::Perimeter => draw.range(0.70, 1.05),
            Role::Accent => draw.range(1.25, 1.75),
        }
    }

    /// Twist multiplier. Broad accents turn most; the narrow core barely does,
    /// because a blade with no face has nothing to turn.
    fn twist(self) -> f32 {
        match self {
            Role::Core => 0.55,
            Role::Body => 1.0,
            Role::Perimeter => 0.85,
            Role::Accent => 1.5,
        }
    }
}

/// Choose a centreline family, weighted by the role as well as the ground.
///
/// The roles want different characters — a core blade is a spear, a skirt blade
/// lies over, an accent curls — and mapping them onto the existing mark
/// vocabulary is what keeps one shape language across the whole field instead of
/// growing a second one for tufts.
fn pick_mark(draw: &mut Draw, ground: &Ground, role: Role) -> Mark {
    match role {
        // Straight and upright: the spine of the clump has no business curling.
        Role::Core if draw.chance(0.72) => Mark::Dash,
        // The skirt is the layer that lies along the ground.
        Role::Perimeter if draw.chance(0.34) => Mark::Tangle,
        Role::Perimeter if draw.chance(0.30) => Mark::Broad,
        // Accents are the marks that get to have character.
        Role::Accent if draw.chance(0.40) => Mark::Sway,
        Role::Accent if draw.chance(0.30) => Mark::Hook,
        _ => Mark::pick(draw, ground.resolution),
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

    fn shape(self, draw: &mut Draw, params: &GrassParams, ground: &Ground) -> Stroke {
        let (short, tall) = params.style.blade_length;
        let (thin, thick) = params.style.blade_width;
        let (low, high) = params.style.blade_bend;
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
            params.style.glint * draw.range(0.7, 1.4) * (0.65 + ground.resolution * 0.7)
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

        // How broad this mark is within the vocabulary's own range. Twist and
        // forking both key on it, because both are properties of a mature blade
        // with some material in it — a thread does neither.
        let width = draw.range(thin, thick);
        let broad = smoothstep(thin, thick, width);

        // Twist, which is the cheapest thing in the vocabulary and close to the
        // most valuable: without it every blade in a tuft shows the sun the same
        // face and the tuft reads as a comb.
        //
        // Signed, so blades turn both ways. Scaled by breadth, because a fine
        // blade is nearly round in section and has no face to turn — the range
        // runs from about a quarter turn on the thinnest to a half turn on the
        // broadest, which is what real grass does.
        let twist = draw.signed() * (0.44 + broad * 1.13);

        // And the tip. Forks belong to broad, mature, well-described blades and
        // nowhere else: fork everything and the field becomes antlers, which is
        // a louder failure than the plain tips it replaced.
        //
        // The notch is the quiet majority partner. It costs one segment, it
        // reads at any distance, and it is what a fork becomes when the page is
        // too coarse to draw one — so having it in the vocabulary in its own
        // right means the two are the same shape rather than a shape and its
        // apology.
        let mature = broad * ground.resolution * (1.0 - ground.bare * 0.8);
        let tip = if draw.chance(0.10 * mature) {
            let split_at = draw.range(0.76, 0.90);
            // One branch longer than the other, always. A symmetric fork is a
            // tuning fork and the eye finds the mirror line immediately.
            let remaining = 1.0 - split_at;
            TipProfile::Forked {
                split_at,
                opening: draw.range(0.10, 0.32),
                long: remaining + draw.range(0.0, 0.06),
                short: remaining * draw.range(0.40, 0.72),
            }
        } else if draw.chance(0.11 + mature * 0.06) {
            TipProfile::Notched {
                depth: draw.range(0.03, 0.11),
            }
        } else {
            TipProfile::Pointed
        };

        let base = Stroke {
            length: draw.range(short, tall),
            bend: draw.range(low, high) + lodged,
            width,
            tip_width: 0.30,
            profile: if draw.chance(0.08) {
                Profile::Stem
            } else {
                Profile::Leaf
            },
            twist,
            tip,
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
            base_light: params.style.base_light - recessive,
            tip_light: params.style.tip_light * draw.range(0.7, 1.3),
            glint: if recessive > 0.0 { 0.0 } else { glint },
            side_light: params.style.side_light,
            // The third thing the intensity classes move, after length and
            // blade-to-blade scatter. The under-stroke is what separates one
            // blade from the next, so draining it is precisely "let these merge
            // into a softer canopy" — and it is the term that carries the most
            // local contrast per pixel of anything in the field, which makes it
            // the most effective one to spend on the distinction.
            under: params.style.under * outlined * (0.77 + ground.resolution * 0.46),
            ..Default::default()
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
                side_light: params.style.side_light * 0.5,
                under: params.style.under * 0.6,
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
    marks: &mut Vec<Stroke>,
    page: &Page,
    draw: &mut Draw,
    root: Vec2,
    ground: &Ground,
    params: &GrassParams,
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
        emit(
            marks,
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
                    params.style.glint * 0.5
                } else {
                    0.0
                },
                side_light: params.style.side_light * 1.4,
                under: params.style.under * 0.8,
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::GrassRenderQuality;
    use crate::scene::GrassScene;

    #[test]
    fn the_canopy_bound_is_never_beaten() {
        // The shadow guard band is sized from `CANOPY_METRES`, so a mark that
        // stands taller than it can cast onto a page from outside the band —
        // and the symptom is not a clipped shadow, it is a missing one, on the
        // pages whose casters happened to fall outside.
        //
        // Swept over real pages rather than reasoned about, because the bound is
        // the product of four independent multipliers and any one of them can be
        // raised without the others being looked at.
        let mut tallest = 0.0f32;
        for (index, origin) in crate::fixtures::PLACES.iter().enumerate() {
            let params = GrassParams {
                seed: 0x5eed_1234u64.wrapping_add(index as u64 * 0x9e37_79b9),
                quality: GrassRenderQuality::Reference,
                ..GrassParams::default()
            };
            let page = Page::new(*origin, 192, 192);
            let field = WorldField::lit_by(params.seed, params.light);
            let scene = GrassScene::build(page, &field, &params);
            tallest = tallest.max(scene.canopy_ceiling());
        }
        assert!(
            tallest <= CANOPY_METRES,
            "the field grows {tallest:.3} m of canopy against a {CANOPY_METRES} m \
             bound the shadow guard band is sized from"
        );
        // And not so far over that the band is costing area for nothing: every
        // extra metre of reach widens the rectangle every scatter pass walks.
        assert!(
            tallest > CANOPY_METRES * 0.55,
            "the canopy bound is {CANOPY_METRES} m for a field that reaches \
             {tallest:.3} m, which is guard band nobody needs"
        );
    }

    #[test]
    fn the_shadow_guard_covers_every_caster_that_can_reach_a_page() {
        // Measured against the sun rather than against a constant, and swept
        // down to the lowest elevation the renderer claims to support. Getting
        // this wrong at 35° and right at 55° is exactly the shape of the bug
        // this exists to prevent.
        let field = WorldField::lit_by(1, GrassParams::default().light);
        for degrees in [35.0f32, 45.0, 55.0] {
            let elevation = degrees.to_radians();
            let params = GrassParams {
                quality: GrassRenderQuality::Reference,
                light: crate::sun::Key {
                    azimuth: 0.0,
                    elevation,
                }
                .direction(),
                ..GrassParams::default()
            };
            for detail in [1.0f32, 0.5, 0.25] {
                let page = Page::at_detail(Vec2::new(-64.0, -64.0), 128, 128, detail);
                let bed = Bed {
                    page: &page,
                    field: &field,
                    params: &params,
                };
                let (low, high) = footprint(&page, bed.caster_reach());
                // Where the page itself is, without any band at all.
                let (bare_low, bare_high) = footprint(&page, -1.0e6);
                let needed = CANOPY_METRES / elevation.tan();
                let margin = (bare_low - low).min(high - bare_high);
                assert!(
                    margin.x >= needed && margin.y >= needed,
                    "at {degrees}° detail {detail} the band gives {margin:?} m \
                     where a caster reaches {needed:.3} m"
                );
            }
        }
    }

    #[test]
    fn a_crown_stays_inside_the_guard_band() {
        // The page guard band is sized against `TUFT_RADIUS`, so a crown that
        // rooted a shoot outside its own bounding ellipse would put a blade
        // beyond what the band allows for — and the symptom of that is not a
        // clipped blade, it is a mark present on one side of a page join and
        // missing on the other.
        //
        // Swept rather than reasoned about, because the bound is the sum of two
        // constants (`LOBE_OFFSET` and `LOBE_SIZE`) that are easy to raise one
        // at a time.
        let ground = Ground {
            height: 0.1,
            crown: 0.5,
            lit: 0.0,
            flow: 0.0,
            hue: 0.0,
            density: 1.3,
            tint: 0.0,
            bare: 0.0,
            colony: 0.5,
            statement: 0.5,
            resolution: 1.0,
        };
        const _: () = assert!(
            LOBE_OFFSET + LOBE_SIZE <= 1.0,
            "a satellite lobe reaches outside the bounding ellipse"
        );

        let mut worst = 0.0f32;
        for seed in 0..400u64 {
            let mut draw = Draw::from_seed(seed);
            let crown = Crown::grow(&mut draw, &ground, Vec2::X);
            // The bounding ellipse itself must fit.
            worst = worst.max(crown.radius.x).max(crown.radius.y);
            // And every lobe inside it.
            for lobe in &crown.lobes[..crown.count] {
                worst = worst
                    .max(lobe.centre.x.abs() + lobe.radius.x)
                    .max(lobe.centre.y.abs() + lobe.radius.y);
            }
            // And every point the sampler can actually return.
            for _ in 0..64 {
                let local = crown.sample(&mut draw);
                worst = worst.max(local.length());
            }
        }
        assert!(
            worst <= TUFT_RADIUS + 1.0e-4,
            "a crown reaches {worst:.4} m from its centre, past the \
             {TUFT_RADIUS} m the guard band is sized for"
        );
    }

    #[test]
    fn a_crown_is_lumpy_rather_than_elliptical() {
        // The property that stops a tuft reading as a hedgehog. A single-centred
        // envelope falls off monotonically from the middle; a multi-lobed one
        // has interior minima, and those are the gaps the reference art shows
        // floor through.
        let ground = Ground {
            height: 0.1,
            crown: 0.5,
            lit: 0.0,
            flow: 0.0,
            hue: 0.0,
            density: 1.0,
            tint: 0.0,
            bare: 0.0,
            colony: 0.5,
            statement: 0.5,
            resolution: 1.0,
        };
        // Walk a ring at a fixed radius and count how many times the envelope
        // turns around. A pure ellipse turns twice; a lobed crown turns more.
        let mut lumpy = 0;
        for seed in 0..64u64 {
            let mut draw = Draw::from_seed(seed ^ 0xc0ffee);
            let crown = Crown::grow(&mut draw, &ground, Vec2::X);
            let ring: Vec<f32> = (0..48)
                .map(|step| {
                    let angle = step as f32 / 48.0 * std::f32::consts::TAU;
                    crown.envelope(Vec2::from_angle(angle) * crown.radius * 0.45)
                })
                .collect();
            let turns = (0..48)
                .filter(|i| {
                    let (a, b, c) = (ring[(i + 47) % 48], ring[*i], ring[(i + 1) % 48]);
                    (b - a).signum() != (c - b).signum()
                })
                .count();
            if turns > 2 {
                lumpy += 1;
            }
        }
        assert!(
            lumpy > 32,
            "only {lumpy} of 64 crowns had more than one centre of vigour"
        );
    }

    #[test]
    fn every_role_appears_and_the_skirt_is_a_quarter_of_the_crown() {
        // The shares matter more than the roles. A crown with no perimeter is
        // hair growing out of a flat disc, and no lighting makes it look planted.
        let mut counts = [0usize; 4];
        let mut draw = Draw::from_seed(0x5eed);
        for step in 0..20_000 {
            // Radius distributed as a disc, which is how the sampler produces
            // them.
            let radius = ((step % 100) as f32 / 100.0).sqrt();
            let role = Role::at(radius, &mut draw);
            counts[match role {
                Role::Core => 0,
                Role::Body => 1,
                Role::Perimeter => 2,
                Role::Accent => 3,
            }] += 1;
        }
        let total: usize = counts.iter().sum();
        let share = |index: usize| counts[index] as f32 / total as f32;
        assert!(share(0) > 0.05, "no core: {:.3}", share(0));
        assert!(share(1) > 0.25, "no body: {:.3}", share(1));
        assert!(
            (0.15..0.45).contains(&share(2)),
            "the skirt is {:.3} of the crown",
            share(2)
        );
        assert!(
            (0.05..0.16).contains(&share(3)),
            "accents are {:.3} of the crown",
            share(3)
        );
    }
}

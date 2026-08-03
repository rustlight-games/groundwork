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

use bevy::prelude::*;

use crate::bake::{BakeParams, Page};
use crate::field::{Ground, GroundCache, WorldField};
use crate::geometry::TipProfile;
use crate::palette::Tone;
use crate::rng::{Draw, Stream};
use crate::stroke::{Profile, Stroke};

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
pub fn footprint(page: &Page) -> (Vec2, Vec2) {
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
    (low, high)
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
        bed.params.thatch,
        // Thinned hard over bare ground, on top of the coverage every pass
        // gets. The mat is the layer that actually closes a clearing: it is
        // short, there are three hundred of them to a square metre, and the
        // tuft pass thinning itself does nothing about them. An opening with a
        // full mat over it is an opening you cannot see.
        |ground| (1.20 - ground.resolution * 0.20) * (1.0 - ground.bare * 0.62),
        |marks, page, draw, root, ground, params| {
            let stroke = mat_stroke(draw, root, ground, params);
            emit(marks, page, stroke);
        },
    );
    scatter(
        marks,
        bed,
        &mut ground,
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
        marks,
        bed,
        &mut ground,
        Stream::Leaf,
        bed.params.leaves,
        |ground| (0.35 + ground.resolution * 0.35) * ground.colony,
        leaf_cluster,
    );
}

/// The three things every planting pass needs: where it is, what grows there,
/// and how it looks.
pub struct Bed<'a> {
    pub page: &'a Page,
    pub field: &'a WorldField,
    pub params: &'a BakeParams,
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
    marks: &mut Vec<Stroke>,
    bed: &Bed,
    cache: &mut GroundCache,
    stream: Stream,
    per_square_metre: f32,
    weight: impl Fn(&Ground) -> f32,
    mut place: impl FnMut(&mut Vec<Stroke>, &Page, &mut Draw, Vec2, &Ground, &BakeParams),
) {
    let Bed { page, params, .. } = *bed;
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
            let coverage = 1.0 - smoothstep(0.04, 0.88, ground.bare) * 0.80;
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

/// One tuft: a handful of blades that agree with each other.
///
/// The agreement is the point. Blades in a tuft share a lean, a length scale and
/// a brightness, and differ only within those; that is what makes a clump read
/// as one plant rather than as a coincidence. It is also where the field's
/// middle scale comes from — twenty pixels of structure that neither a single
/// blade nor the mound field can produce.
fn grow_tuft(
    marks: &mut Vec<Stroke>,
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
        emit(marks, page, stroke);
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
    marks: &mut Vec<Stroke>,
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

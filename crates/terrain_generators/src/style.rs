//! What the generator is told, and nothing about how it is drawn.
//!
//! [`GrassParams`] is the *entire* determinant of a meadow: which world, how
//! hard to work, where the sun is, and what the grass is made of. Nothing else
//! reaches placement. So a scene is a pure function of one small value, and two
//! renderers handed the same one are looking at the same field — the same
//! guarantee that used to be a crate boundary against the rasteriser, and is
//! now a boundary against Cycles alone.
//!
//! [`GrassStyle`] is the fourth of those — the population counts and the
//! morphology.

use glam::Vec3;

use crate::quality::GrassRenderQuality;

/// Everything the generator reads, and nothing else.
///
/// The input to placement, and therefore the *entire* determinant of a meadow.
/// Four things: which world, how hard to work, where the sun is, and what the
/// meadow is made of.
///
/// It exists so that a crate boundary can. Placement used to take a whole
/// `BakeParams`, so every module that decided where a blade goes depended on
/// the rasteriser — the generator could not be separated from it without
/// separating a struct first. The rasteriser is gone now, but the boundary
/// stayed: `terrain_generators` still knows nothing about how a renderer
/// draws.
///
/// The sun is here, and it is the one field worth justifying. A meadow should
/// not depend on the light, and almost none of it does — but the mound field
/// shades its own domes analytically and needs to know which way the sun is,
/// so the light reaches placement through this. That coupling is real and
/// deserves its own measured change rather than being unpicked during a
/// migration.
#[derive(Clone, Copy, Debug)]
pub struct GrassParams {
    pub seed: u64,
    /// How hard the generator is allowed to work.
    ///
    /// A tier may decide how *finely* something is measured and never whether it
    /// exists — see [`crate::quality`]. That rule is what lets a cheap render and
    /// an expensive one be two photographs of one meadow.
    pub quality: GrassRenderQuality,
    /// Direction toward the key light in image space: +X right, +Y **down**,
    /// +Z toward the viewer.
    pub light: Vec3,
    pub style: GrassStyle,
}

impl Default for GrassParams {
    fn default() -> Self {
        Self {
            seed: 0x5eed_1234,
            quality: GrassRenderQuality::Preview,
            // Up and to the left on screen, and well in front of the ground
            // plane. Image space, so +Y is *down*: negative X is leftward and
            // negative Y is up the screen. Every mound in the field is therefore
            // lit on its upper-left face and falls away toward the lower-right,
            // and every mark's under-stroke sits on its lower-right side. One
            // direction, stated once, obeyed everywhere — a field where the
            // macro light and the marks disagree about where the sun is reads as
            // wrong long before anyone can say why.
            //
            // Fetched from the laboratory's key rather than written out, and
            // that is a dependency worth naming: `lab::Key` is a *sun*, authored
            // as an azimuth and an elevation, and a sun belongs on this side of
            // the boundary rather than inside a measurement tool. Moving it is
            // its own change. Inlining the vector instead is not an option — the
            // default key lies almost in the ground plane, which is not the
            // number anyone would guess from the comment above.
            light: crate::sun::Key::default().direction(),
            style: GrassStyle::default(),
        }
    }
}

/// What the meadow is made of.
///
/// Everything the *generator* reads: how many of each kind of mark grow per
/// square metre, how long and wide and bent they are, and the intrinsic colour
/// family each carries. Change any of it and the meadow is a different meadow,
/// so every scene fingerprint moves.
///
/// The four dimensions the rasteriser also reads — blade length, width, bend and
/// the under-stroke — are here rather than in [`PreviewRasterStyle`] because the
/// generator *decides* them and writes them onto each mark. A renderer reading a
/// style is fine; a renderer that could change one would not be.
#[derive(Clone, Copy, Debug)]
pub struct GrassStyle {
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
    /// Fine blades per square metre — the closed canopy the tufts stand in.
    ///
    /// The largest single count in the field, and it should be. The reference
    /// art gives most of its *area* to short combed grass and its accents to
    /// everything else; a renderer that grows only accents produces marks
    /// scattered on a floor.
    pub fine: f32,
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
    /// Strength of the one-sided lateral shading, applied at the rib.
    pub side_light: f32,
    /// Width of the dark under-stroke, cache pixels.
    ///
    /// **Cut to a third.** It used to be the field's only shadow, and it was a
    /// good one — a dark band offset away from the light, which is what a shadow
    /// looks like from a distance. Now that blades cast real shadows onto each
    /// other it is double-counting, and two shadows on one blade read as an
    /// outline rather than as depth.
    ///
    /// It keeps the job the geometry shadows cannot do at this resolution:
    /// separating two overlapping blades of nearly the same colour, at a width
    /// of about a pixel, where a cast shadow has no room to form.
    pub under: f32,
    /// Per-tuft brightness scatter.
    pub scatter: f32,
}

impl Default for GrassStyle {
    fn default() -> Self {
        Self {
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
            // About one every sixteen millimetres. Dense enough that the layer
            // is a surface rather than a scatter, which is the whole distinction
            // it exists to draw — coverage is not closure, and closure is what
            // makes the floor stop showing between the marks.
            fine: 3800.0,
            leaves: 4.0,

            // The short end is lifted rather than the whole range scaled, and
            // that distinction is what keeps the guard band affordable. A tuft
            // has to stand clear of the fine layer to read as a plant at all —
            // there is seventy times as much fine grass as there is tuft, so a
            // tuft whose blades are the same height simply joins it and the
            // plate reads as a carpet with denser patches. But clearance is a
            // statement about the *shortest* blade in a tuft, and multiplying
            // the whole distribution to get it stacks onto four other
            // multipliers and puts metre-and-a-half blades in a meadow.
            blade_length: (0.14, 0.38),
            blade_width: (0.42, 1.95),
            // Well off vertical even at the low end. Grass drawn standing up is
            // grass drawn as objects; this art draws it as strokes lying along
            // the ground, and the difference survives being shrunk to gameplay
            // size when almost nothing else does. Pulled back a little from
            // where it was, because a mark twice as long at the same bend lies
            // over twice as far and the bunch stops having a top.
            blade_bend: (0.35, 1.40),

            base_light: 0.556,
            // Down by a fifth while `TIP_CURVE` went up by two thirds, which is
            // one instruction rather than two: the same light, gathered onto the
            // last few pixels of a mark instead of spread along its upper half.
            tip_light: 0.34,
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
            under: 0.24,
            // The one term that raises mid-scale organisation without touching
            // a single pixel of high-frequency contrast, because it varies from
            // tuft to tuft and a tuft is a fifth of a metre — exactly the radius
            // the plate measures flattest at. Variation *between* bunches groups
            // the field; variation *within* one only makes it noisy. They cost
            // the same and this is the one worth having.
            scatter: 0.50,
        }
    }
}

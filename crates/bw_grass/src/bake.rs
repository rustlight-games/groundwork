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

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use crate::field::WorldField;
use crate::iso;
use crate::lighting::{self, FormWeights};
use crate::page::Page;
use crate::painter::Painter;
use crate::palette;
use crate::quality::GrassRenderQuality;
use crate::rng::Stream;
use crate::scene::GrassScene;
use crate::shadow::{self, ShadowMap};
use crate::style::{GrassParams, GrassStyle};
use crate::surface::{Surface, blur};
use crate::tone::Tone;

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

/// Every number the look depends on, in one place.
///
/// Deliberately a plain struct of tunables rather than constants scattered
/// through the baker. Matching a piece of reference art is an iterative,
/// numerical exercise, and it goes very badly when the knobs are spread across
/// six files.
#[derive(Clone, Copy, Debug)]
pub struct BakeParams {
    pub seed: u64,
    /// How hard the renderer is allowed to work. See [`GrassRenderQuality`].
    pub quality: GrassRenderQuality,
    /// Direction toward the key light in image space: +X right, +Y **down**,
    /// +Z toward the viewer.
    pub light: Vec3,

    /// What the meadow is made of. See [`GrassStyle`].
    pub style: GrassStyle,

    /// How the cheap rasteriser draws it. See [`PreviewRasterStyle`].
    pub raster: PreviewRasterStyle,
}

impl BakeParams {
    /// The half of these parameters the generator reads.
    pub fn grass(&self) -> GrassParams {
        GrassParams {
            seed: self.seed,
            quality: self.quality,
            light: self.light,
            style: self.style,
        }
    }
}

/// How the cheap rasteriser draws a meadow.
///
/// Twenty-three parameters, and **not one of them changes where a blade goes**.
/// They are the fake occlusion, the macro lighting, the under-strokes and the
/// colour grade — everything that decides the *picture* rather than the meadow.
///
/// That is the whole reason this is a separate struct. A scene built from a
/// [`GrassStyle`] survives any change to these: the marks are the same marks,
/// so the fingerprint holds, and a plate can be re-shaded without regenerating a
/// thing. When the two were one struct that property was true and invisible, and
/// an invisible property is one somebody breaks.
///
/// Nothing here reaches the Cycles path either. The path tracer integrates a
/// hemisphere and has no use for a term that approximates one — see
/// `RenderSettings`, which is this tier's counterpart on that side.
#[derive(Clone, Copy, Debug)]
pub struct PreviewRasterStyle {
    /// Weight of the three-scale form term — how much a surface's own facing
    /// moves its light index.
    ///
    /// The term that answers the user-visible complaint "one side should receive
    /// more light and the other should be darker", and the one that could not
    /// exist before there were normals. It is generous compared with everything
    /// around it because it is the only term in the field that is a *statement
    /// about direction*: the macro lighting says where the mounds are, the
    /// occlusion says where the cavities are, and neither of them says which way
    /// the sun is.
    pub form_light: f32,
    /// Weight of light that has passed through a leaf rather than off it.
    ///
    /// Distinct from [`BakeParams::transmission`], which is the same idea at the
    /// scale of a whole mound; this one is per leaf and keys on the leaf's own
    /// normal. Both are wanted — a canopy glows at two scales — and they are not
    /// substitutes.
    pub leaf_transmission: f32,
    /// Weight of the broad waxy sheen along a leaf.
    ///
    /// Small, and it should stay small. Grass is not wet. This exists to give
    /// mature broad blades a lustre that says "surface" rather than "paint", and
    /// past about a twentieth it starts reading as plastic.
    pub gloss: f32,

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
    ///
    /// **Retired to a whisper.** It measured "am I lower than the canopy three
    /// pixels away", which was the best available answer before the surface knew
    /// how much geometry was stacked at each pixel and before the canopy could
    /// be scanned by direction. Both of those now exist, and both answer the same
    /// question better — see [`BakeParams::interior`] and
    /// [`BakeParams::ambient_occlusion`]. What is left of it is a one-pixel
    /// separation between touching marks, which is a job neither of the others
    /// does at that radius.
    pub micro_occlusion: f32,
    /// How much of the sky the canopy's own shape takes away.
    ///
    /// Horizon-scanned rather than differenced — see
    /// [`crate::lighting::horizon_occlusion`]. This is the term that makes the
    /// gap between two crowns dark without also darkening the open ground beside
    /// them, which every previous attempt in this crate did.
    pub ambient_occlusion: f32,
    /// How dark a stack of overlapping leaves gets.
    ///
    /// The other half of occlusion, and the half no height field can supply. A
    /// canopy surface says how high the grass is; it says nothing about whether
    /// there is one blade there or fifteen, and the inside of a tuft is dark
    /// because it is *full*. Comes from the fragment counter the rasteriser
    /// keeps — see [`crate::surface::Surface::optical_at`].
    pub interior: f32,
    /// How fast the interior term saturates with stacked leaves.
    pub interior_density: f32,
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
    /// The fixed directional self-shadow, marched over the canopy height.
    ///
    /// Kept, demoted, and now doing a different job. It describes the canopy as
    /// a *surface* — one crown against the ground behind it — which is a real
    /// cue and one the geometry shadows below do not supply, because they see
    /// blades rather than masses. What it can no longer claim to be is the
    /// shadow of anything in particular.
    pub shadow: f32,
    /// How far down the ramp a surface falls when no light reaches it at all.
    ///
    /// ## Why this is a subtraction and not a multiply
    ///
    /// A physical renderer multiplies albedo by light. This one cannot: its
    /// "albedo" is a position in a hand-authored ramp, and half that ramp's
    /// value is *where the hue goes* as the value falls. Multiplying an index by
    /// 0.3 does not darken a colour, it picks a different one — usually the
    /// wrong one.
    ///
    /// So light moves a surface **along** the ramp instead, and this is how far
    /// it can move. It is sized to carry a fully unlit surface from the middle
    /// of the measured range down past its floor and into the shadow extension
    /// below — which is exactly the journey a shadow has to make, and the one
    /// the old additive terms could not: they took a fixed amount off whatever
    /// the surface already was, so a bright blade in deep shade stayed brighter
    /// than a dim one in full sun.
    pub shade_depth: f32,
    /// How much light still reaches a surface the sun cannot see.
    ///
    /// Sky, and green bounce off the canopy underneath. Without it a shadow
    /// under a dense tuft goes to the floor of the extension and reads as a
    /// hole; grass in shade is dim and *saturated*, never black, and the fix for
    /// an over-dark shadow is always more fill and never a weaker sun.
    pub sky_fill: f32,
    /// Angular radius of the sun, radians. Softens every cast shadow.
    pub sun_radius: f32,
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
        let grass = GrassParams::default();
        Self {
            seed: grass.seed,
            quality: grass.quality,
            light: grass.light,
            style: grass.style,
            raster: PreviewRasterStyle::default(),
        }
    }
}

impl Default for PreviewRasterStyle {
    fn default() -> Self {
        Self {
            // Large, and the largest single lighting term in the field. Nothing
            // else here says which way the sun is.
            form_light: 0.46,
            leaf_transmission: 0.20,
            gloss: 0.045,

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
            // Down by a quarter, now that `BROAD_LIGHT` compresses its upward
            // half as well. The two moves are one instruction: the mound is a
            // rhythm and not a source, and a rhythm that brightens a whole
            // region is announcing itself.
            mound_light: 0.32,
            elevation_light: 0.035,
            crown_light: 0.038,
            micro_occlusion: 0.030,
            ambient_occlusion: 0.30,
            interior: 0.34,
            interior_density: 0.115,
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
            shadow: 0.075,
            // Enough to carry a mid-ramp surface past the measured floor and
            // well into the extension. A cast shadow is the darkest thing in the
            // field by a wide margin and it should be — the reference art's
            // deepest values are all either a shadow or the inside of a tuft.
            shade_depth: 0.86,
            // A third of the direct light, which is roughly what an open sky
            // gives against a midday sun once the green bounce off the canopy is
            // counted with it.
            sky_fill: 0.34,
            sun_radius: crate::shadow::SUN_RADIUS,
            transmission: 0.205,
            light_blur: 4,
            region: 0.20,
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
    /// Cache pixels from the page's corner back to the last lattice line before
    /// it, per axis. See [`Macro::build`].
    offset: Vec2,
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
    /// Sample the composition fields on a lattice anchored in the **world**.
    ///
    /// The anchoring is the whole of what changed here, and it is worth stating
    /// why it is not cosmetic.
    ///
    /// The lattice used to be laid out from each page's own top-left corner at a
    /// six-pixel stride, and 256 is not a multiple of six. So two neighbouring
    /// pages read the composition fields from points up to four pixels apart,
    /// and a region baked whole differed from the same region baked as pages
    /// across about a fifth of its pixels. That was tolerable while it was only
    /// a shading difference — the fields are smooth at six pixels, so reading
    /// them from slightly different places moves the answer by far less than the
    /// grass on top does.
    ///
    /// It stops being tolerable the moment a **shadow map is shared across a
    /// region**, because then the two paths are no longer two renderings of the
    /// same thing that happen to differ slightly: one of them is the caster and
    /// the other is the receiver, and they have to agree exactly about where the
    /// ground is. It also matters for training, where a crop taken from a
    /// region bake and a crop taken from a page bake have to be interchangeable
    /// samples of one distribution rather than two.
    ///
    /// So the lattice lines now fall at fixed multiples of the stride in cache
    /// coordinates, which are a linear function of world position. A page finds
    /// the last line before its own corner, records how far past it the corner
    /// sits, and indexes from there. Every page in the world therefore samples
    /// the same points, whatever grid it was cut on.
    pub fn build(page: &Page, field: &WorldField) -> Self {
        // The stride is a statement about the world, not about the page: six
        // reference pixels is a sixteenth of a metre, comfortably finer than
        // anything the composition fields hold. Keeping the *pixel* stride while
        // the page scale drops would let it slide to a quarter of a metre and
        // start aliasing the mound field, so it scales down with the page and
        // the lattice keeps sampling the same ground just as finely.
        let stride = page.radius(MACRO_STRIDE);
        let step = stride as f32;
        // How far the page's corner sits past the last lattice line before it,
        // per axis, in `0..stride`.
        let offset = page.origin - (page.origin / step).floor() * step;
        let width = ((page.width as f32 + offset.x) / step).ceil() as usize + 2;
        let height = ((page.height as f32 + offset.y) / step).ceil() as usize + 2;
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
                // Lattice point `(x, y)` sits at cache position
                // `anchor + (x - 0.5) * stride`, which is a function of the world
                // alone. Subtracting the offset is what turns the page-relative
                // index back into that world-anchored place.
                let pixel = Vec2::new(x as f32 - 0.5, y as f32 - 0.5) * step - offset;
                let ground = field.sample(page.ground_at(pixel));
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
            stride,
            offset,
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

    /// Where a final-resolution page pixel falls in lattice coordinates.
    #[inline]
    fn coordinate(&self, x: f32, y: f32) -> (f32, f32) {
        let step = self.stride as f32;
        (
            ((x + self.offset.x) / step + 0.5).clamp(0.0, (self.width - 1) as f32),
            ((y + self.offset.y) / step + 0.5).clamp(0.0, (self.height - 1) as f32),
        )
    }

    /// The ground's own world normal at a page pixel.
    ///
    /// Differenced off the lattice rather than sampled from the field, because
    /// the lattice is already built and the mound field's finest feature is
    /// several lattice cells wide — so a central difference across one stride is
    /// reading the shape at the resolution it actually has, not approximating it.
    ///
    /// The height is in *metres* here and the slope wants reference pixels per
    /// page pixel, which is what the conversion is for.
    fn ground_normal(&self, x: f32, y: f32, detail: f32) -> Vec3 {
        let step = self.stride as f32;
        let slope = Vec2::new(
            self.at(&self.height_field, x + step, y) - self.at(&self.height_field, x - step, y),
            self.at(&self.height_field, x, y + step) - self.at(&self.height_field, x, y - step),
        ) * (iso::PX_PER_METRE / (2.0 * step));
        lighting::height_field_normal(slope, detail)
    }

    /// Bilinear read at a final-resolution page pixel.
    fn at(&self, source: &[f32], x: f32, y: f32) -> f32 {
        let (u, v) = self.coordinate(x, y);
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
    // One scene, rendered twice: once from the sun into a depth buffer and once
    // from the camera into the surface. Building it here rather than inside each
    // pass is the whole reason `GrassScene` exists — regenerating the blades for
    // the shadow pass would nearly work, and "nearly" is what produces shadows
    // that do not quite belong to the blades casting them.
    let scene = GrassScene::build(page, &field, &params.grass());
    let mut surface =
        Surface::at_supersample(page.width, page.height, params.quality.supersample());

    lay_floor(&mut surface, &page, &field, &lattice);
    {
        let mut painter =
            Painter::at_scale(&mut surface, page.origin, params.light, page.px_per_metre)
                .with_ribs_per_pixel(params.quality.ribs_per_pixel());
        painter.draw_scene(&scene);
    }
    let shadows = cast_shadows(&scene, params);
    resolve_lit(&surface, &page, &lattice, params, shadows.as_deref())
}

/// Render the scene from the sun, once per sample over its disc.
///
/// Several maps rather than one, and averaged at the receiver rather than
/// blurred afterwards. Blurring a hard shadow gives every edge the same
/// penumbra whatever cast it; averaging several sun directions gives a narrow
/// penumbra close to the caster and a wide one far from it, which is what a
/// shadow actually does and is most of what stops a field of them reading as
/// stencils.
///
/// Returns nothing at [`GrassRenderQuality::Preview`], which is the streaming
/// tier and cannot afford a second pass over the geometry.
pub fn cast_shadows(scene: &GrassScene, params: &BakeParams) -> Option<Vec<ShadowMap>> {
    if params.quality.shadow_density() <= 0.0 {
        return None;
    }
    let sun = iso::image_to_world(params.light).normalize_or(Vec3::Z);
    // A genuine bound, not an estimate. A caster clipped out of the volume is a
    // shadow that simply is not there, and only on the pages whose casters
    // happened to fall outside.
    let ceiling = scene.canopy_ceiling().max(0.05);
    let maps: Vec<ShadowMap> = shadow::sun_samples(params.quality.sun_samples())
        .into_iter()
        .filter_map(|offset| {
            ShadowMap::cast(
                scene,
                shadow::nudge(sun, offset, params.raster.sun_radius),
                ceiling,
                params.quality,
                // Half a texel of stagger between the maps, so the several
                // grids do not agree about where their texel boundaries are.
                // Without it the averaged penumbra keeps the grid of whichever
                // map happened to dominate.
                offset * 0.5,
            )
        })
        .collect();
    (!maps.is_empty()).then_some(maps)
}

/// Grow every mark the page holds onto an already-floored surface.
///
/// The stroke pass, wrapped so it can be run — and timed — on its own. The
/// [`Painter`] borrows the surface for the duration and has to be dropped before
/// anything reads it back, which is the only reason this is a function rather
/// than three lines inside [`bake`].
pub fn plant_strokes(surface: &mut Surface, page: &Page, field: &WorldField, params: &BakeParams) {
    let scene = GrassScene::build(*page, field, &params.grass());
    let mut painter = Painter::at_scale(surface, page.origin, params.light, page.px_per_metre);
    painter.draw_scene(&scene);
}

/// How far outside itself a page's shading terms read, in reference pixels.
///
/// The number a padded bake has to grow by, and it is **derived rather than
/// chosen**, because the terms it covers chain: the relief comparison samples a
/// blur at an offset, and the result is then blurred again, and the painterly
/// passes read that. Each stage's reach adds to the last, and a pad short by a
/// single pixel leaves a visible step at every page join — which is exactly what
/// a hand-picked 128 turned out to be.
///
/// Two of these fail differently and both matter. A blur whose support is
/// cropped at a page edge is a smooth bias, which shows up as a gentle gradient.
/// The *directional* relief offset being clamped is a **step**: a pixel just
/// inside a page's left edge compares itself against ground it cannot see and
/// falls back to the symmetric comparison, while the pixel one column to its
/// left — on the neighbouring page — does not.
///
/// Note the doubling on every blur. [`crate::surface::blur`] runs its box pass
/// twice to approximate a Gaussian, so a stated radius of 52 actually reaches
/// 104. That is the single easiest thing here to get wrong by half.
fn shading_reach(params: &BakeParams) -> usize {
    /// The canopy-relief blur's stated radius. Half a metre — see [`resolve`].
    const FAR_BLUR: usize = 52;
    /// Two box passes per [`crate::surface::blur`] call.
    const PASSES: usize = 2;

    let far = FAR_BLUR * PASSES;
    let relief = RELIEF_REACH.ceil() as usize;
    let macro_blur = params.raster.light_blur * PASSES;
    let painterly = GLAZE_REACH + 1;
    // An eighth over, so that adding a term does not silently need this
    // recalculated on the same day.
    (far + relief + macro_blur + painterly) * 9 / 8
}

/// Bake a page with its surroundings rasterised too, then crop.
///
/// The correct way to bake, and the expensive one. Every shading term in
/// [`resolve`] that reads a neighbourhood — the occlusion radii, the directional
/// relief, the shadow march, the glaze — is computed with the ground that is
/// actually there rather than with whatever half of it fell inside the page, so
/// the result does not depend on where the page grid was laid.
///
/// It costs the padding's area, and that is why [`BakeRegion`] exists. Padding
/// one 256-pixel page by [`SHADING_REACH`] on every side more than triples it;
/// padding a two-by-two region doubles it; padding a four-by-four adds half
/// again. The pad is a perimeter cost and the pages are an area, so the bigger
/// the piece of ground the cheaper the correctness.
///
/// [`bake`] remains for the streaming tier, which cannot afford this and does
/// not need it — a page popping in with a slightly different relief term at its
/// left edge is not what anybody notices about grass appearing.
///
/// [`BakeRegion`]: BakeRegion
pub fn bake_padded(page: Page, params: &BakeParams) -> Vec<Vec3> {
    let pad = page.radius(shading_reach(params));
    let grown = Page {
        origin: page.origin - Vec2::splat(pad as f32),
        width: page.width + pad * 2,
        height: page.height + pad * 2,
        px_per_metre: page.px_per_metre,
    };
    let plate = bake(grown, params);

    let mut cropped = Vec::with_capacity(page.width * page.height);
    for row in 0..page.height {
        let start = (row + pad) * grown.width + pad;
        cropped.extend_from_slice(&plate[start..start + page.width]);
    }
    cropped
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
/// written straight into the finished plate rather than collected and stitched
/// afterwards. Collecting them first holds the whole region twice — nearly a
/// gigabyte for that view — for no gain, since the stitch is a memcpy either way.
///
/// ## Every page is its own task, and that is not how it started
///
/// The obvious parallel split is one task per **band** of rows, because rows are
/// contiguous and `par_chunks_mut` hands out disjoint row ranges for free. It is
/// also badly wrong at the size that matters. A band is one page tall, so a
/// 1080-pixel view is five tasks — on a machine with sixteen cores, three
/// quarters of which then sit idle while five threads each bake eight pages in
/// sequence. The narrower the view, the worse it gets, and a page baked for a
/// distant camera makes views narrower.
///
/// So the rows are handed out band by band as before, and then each band is
/// **split again into per-page column strips** before anything is baked. The
/// strips of one band are disjoint slices of the same rows, which is exactly what
/// lets every page in the region be its own task without the region ever existing
/// twice in memory. Tasks now number pages rather than bands, and the tail is set
/// by the slowest single page rather than by the slowest row of them — which
/// matters more than it sounds, because page cost varies by a factor of two and a
/// half from one patch of world to another.
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

            // Regroup the band's rows into one strip per page. Each row is cut
            // at the page boundaries and the pieces are dealt out by column, so
            // strip `tx` ends up owning that page's rows and nothing else.
            let mut strips: Vec<Vec<&mut [Vec3]>> =
                (0..across).map(|_| Vec::with_capacity(height)).collect();
            for row in rows.chunks_mut(region.width) {
                let mut rest = row;
                for strip in strips.iter_mut() {
                    let (head, tail) = rest.split_at_mut(TILE_PIXELS.min(rest.len()));
                    strip.push(head);
                    rest = tail;
                }
            }

            strips
                .into_par_iter()
                .enumerate()
                .for_each(|(tx, mut strip)| {
                    let width = TILE_PIXELS.min(region.width - tx * TILE_PIXELS);
                    let origin = region.origin
                        + Vec2::new((tx * TILE_PIXELS) as f32, (band * TILE_PIXELS) as f32);
                    // The tiles inherit the region's scale. A region baked for a
                    // distant camera is tiled into pages baked for the same one,
                    // and the page size stays in pixels rather than metres
                    // because it is a streaming unit and a draw call, not a piece
                    // of world.
                    let tile = bake(
                        Page {
                            origin,
                            width,
                            height,
                            px_per_metre: region.px_per_metre,
                        },
                        params,
                    );
                    for (y, row) in strip.iter_mut().enumerate() {
                        let source = y * width;
                        row.copy_from_slice(&tile[source..source + width]);
                    }
                });
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
            let ground = page.ground_at(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
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

            // The floor takes the terrain's own normal rather than a flat up.
            // Bare earth between the blades is the one part of the field where
            // the ground's shape is directly visible, so a flat floor there is
            // exactly where "this is a texture, not a place" gets given away.
            let normal = lattice.ground_normal(x as f32, y as f32, page.detail());
            let step = surface.supersample();
            for sy in 0..step {
                let index = surface.index(x * step, y * step + sy);
                surface.lay_run(index, step, light, soil, normal);
            }
        }
    }
}

/// Everything the renderer knew, beside the picture it produced.
///
/// ## Why a neural renderer needs more than the colour
///
/// The expensive path decides a great many things from hashes the cheap input
/// cannot see. Whether *this* broad blade forked, which way its face turned,
/// how much canopy is stacked behind it — none of that is recoverable from a
/// low-resolution plate, and a network trained on pixels alone has no choice but
/// to average over the possibilities. Averaged forks are soft tips, and averaged
/// occlusion is a flat interior; both are exactly the failures the whole of this
/// work was meant to remove.
///
/// So the structure travels with the target. Not all of it will be needed at
/// inference — most of these are here to find out *which* are — but a channel
/// that was never exported cannot be tested for.
#[derive(Default)]
pub struct Passes {
    /// The mark's own light index, before any lighting.
    pub albedo: Vec<f32>,
    /// World-space surface normal of whatever won each pixel.
    pub normal: Vec<Vec3>,
    /// The terrain's normal, at metres.
    pub ground_normal: Vec<Vec3>,
    /// The crown's normal, at centimetres.
    pub canopy_normal: Vec<Vec3>,
    /// Canopy height above the soil, in reference pixels.
    pub height: Vec<f32>,
    /// How much sun reaches the canopy, `0..1`.
    pub sunlight: Vec<f32>,
    /// How much sky the canopy's own shape takes away, `0..1`.
    pub occlusion: Vec<f32>,
    /// How much geometry stacked up at each pixel.
    pub optical: Vec<f32>,
    /// Root-to-tip position of the winning mark, `0..1`.
    pub along: Vec<f32>,
    /// How mature the winning mark is, `0..1`.
    pub maturity: Vec<f32>,
}

impl Passes {
    fn resize(&mut self, pixels: usize) {
        self.albedo = vec![0.0; pixels];
        self.normal = vec![Vec3::Z; pixels];
        self.ground_normal = vec![Vec3::Z; pixels];
        self.canopy_normal = vec![Vec3::Z; pixels];
        self.height = vec![0.0; pixels];
        self.sunlight = vec![1.0; pixels];
        self.occlusion = vec![0.0; pixels];
        self.optical = vec![0.0; pixels];
        self.along = vec![0.0; pixels];
        self.maturity = vec![0.0; pixels];
    }

    /// Every channel, as a name and a single-component view.
    ///
    /// For writing them out. Vector channels are split rather than packed,
    /// because a normal written as an image has to be encoded to be looked at
    /// and the encoding is the caller's business.
    pub fn scalars(&self) -> Vec<(&'static str, &[f32])> {
        vec![
            ("albedo", &self.albedo),
            ("height", &self.height),
            ("sunlight", &self.sunlight),
            ("occlusion", &self.occlusion),
            ("optical", &self.optical),
            ("along", &self.along),
            ("maturity", &self.maturity),
        ]
    }

    /// The vector channels, likewise.
    pub fn vectors(&self) -> Vec<(&'static str, &[Vec3])> {
        vec![
            ("normal", &self.normal),
            ("ground-normal", &self.ground_normal),
            ("canopy-normal", &self.canopy_normal),
        ]
    }
}

/// Where the sum of the macro lighting terms starts to compress.
///
/// Six broad terms that each behave perfectly can still *agree*, and where they
/// do the light index runs off the top of the ramp. The symptom is a flat pale
/// region — the brightest passage in the plate carrying none of its own detail,
/// because everything in it has clipped to the same stop.
const MACRO_KNEE: f32 = 0.16;

/// The radii the horizon scan samples along each direction, reference pixels.
///
/// Geometric rather than uniform, which is what lets five taps cover two orders
/// of magnitude: a blade is three pixels wide, a tuft is twenty across, and the
/// gap between two crowns is fifty. A uniform spread over the same range would
/// spend four of its five samples describing the largest scale and none at all
/// on the one the eye reads first.
const AO_RADII: [usize; 5] = [3, 7, 15, 30, 56];

/// Radius the canopy is blurred by to get the crown surface, reference pixels.
///
/// A seventh of a metre. Wide enough that individual blades are gone and only
/// the bunch's own shape is left; narrow enough that neighbouring bunches have
/// *not* merged into one dune — which is the failure that matters, because a
/// merged crown surface gives every tuft the same normal and every tuft the same
/// light, and a field of equally lit tufts is a carpet.
const CROWN_BLUR: usize = 13;

/// How tightly the waxy sheen gathers.
///
/// Broad. Grass has a soft lustre rather than a specular pinprick, and a narrow
/// lobe on geometry this fine produces exactly the sub-pixel highlights that
/// cannot be filtered and therefore crawl whenever the ground moves under the
/// sampling grid.
const GLOSS_POWER: f32 = 12.0;

/// How far clear of the mass a mark stands, `0..1`, from its height.
///
/// Shared by the transmission gate and the glaze, because they are asking the
/// same question from opposite directions: did this mark win its pixel by
/// standing above the canopy, or by being the only thing there.
#[inline]
fn exposure_of(top: f32) -> f32 {
    (top / CANOPY_CEILING).clamp(0.0, 1.0)
}

/// A fixed ceiling rather than any page's own tallest blade.
///
/// Normalising by a per-page maximum makes every derived term depend on what
/// happened to grow inside that particular rectangle, so two neighbouring pages
/// shade the same pixel differently and the join between them becomes visible.
/// Constants tile; page statistics do not.
const CANOPY_CEILING: f32 = 48.0;

/// The slope of a height field at a page pixel, per pixel, by central
/// difference.
#[inline]
fn sample_slope(
    field: &[f32],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    across: bool,
) -> f32 {
    // Two pixels apart rather than one. A one-pixel difference on a field this
    // finely sampled is mostly reading the blur's own residual noise, and a
    // normal built from noise scintillates.
    const REACH: usize = 2;
    let (low, high) = if across {
        (
            field[y * width + x.saturating_sub(REACH)],
            field[y * width + (x + REACH).min(width - 1)],
        )
    } else {
        (
            field[y.saturating_sub(REACH) * width + x],
            field[(y + REACH).min(height - 1) * width + x],
        )
    };
    (high - low) / (2.0 * REACH as f32)
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

/// How much of a broad lighting term's *upward* half survives.
///
/// The mirror of [`BROAD_DARK`], and it took real shadows arriving before the
/// need for it was visible.
///
/// That rule says light may be broad and dark may not, because a broad dark area
/// has no caster and reads as a stain. The reverse is just as true and was
/// hidden while the field had no shadows to compare against: **a broad bright
/// area has no source.** The sun does not get stronger over there. A metre-wide
/// pale patch is the field inventing exposure, and once there are genuine
/// shadows beside it the patch stops reading as "sunlit" and starts reading as
/// blown out — a passage with no incident in it, which is the same complaint as
/// a featureless dark one and for the same reason.
///
/// So the slowly-varying terms now give up part of both halves, and the *fast*
/// ones — the tip lift, the glint, the blade's own form — keep theirs entirely.
/// Bright then only ever appears as a small thing among larger duller things,
/// which is where it is legible as a highlight.
///
/// Less severe than `BROAD_DARK` on purpose. A broad bright area is a milder
/// fault than a broad dark one: the eye forgives a hazy sunlit passage far more
/// readily than a shadow with nothing casting it.
const BROAD_LIGHT: f32 = 0.66;

/// Compress both halves of a slowly-varying term.
///
/// One function where there were two. It began as `squashed`, which kept a
/// term's positive half whole and compressed only its negative half, because at
/// the time the only broad artefact the field could produce was a dark stain —
/// see [`BROAD_DARK`]. Real shadows made the other half visible: a broad *bright*
/// area has no source either, and beside a genuine shadow it reads as blown out
/// rather than as sunlit. So both halves are compressed, by different amounts,
/// and the asymmetry between the two constants is the whole of what is left of
/// the original rule.
///
/// Deliberately linear on each side rather than a smooth curve through zero: a
/// curve would also flatten the small values, which are most of the field, and
/// the point is to change what *large* excursions do without touching the gentle
/// modulation everywhere else. Continuous at zero, so nothing here can print an
/// edge.
#[inline]
fn broad(value: f32) -> f32 {
    if value >= 0.0 {
        value * BROAD_LIGHT
    } else {
        value * BROAD_DARK
    }
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
///
/// Kept as the shadowless entry point, because `benches/bake.rs` times the
/// resolve stage on its own and a stage that silently included a shadow pass
/// would be timing something the name does not say.
pub fn resolve(surface: &Surface, page: &Page, lattice: &Macro, params: &BakeParams) -> Vec<Vec3> {
    resolve_lit(surface, page, lattice, params, None)
}

/// [`resolve`], with the sun's own view of the scene.
pub fn resolve_lit(
    surface: &Surface,
    page: &Page,
    lattice: &Macro,
    params: &BakeParams,
    shadows: Option<&[ShadowMap]>,
) -> Vec<Vec3> {
    resolve_passes(surface, page, lattice, params, shadows, None)
}

/// [`resolve_lit`], optionally recording what it saw.
///
/// The auxiliary passes exist for the neural renderer rather than for the
/// picture, and they are gathered *here* rather than reconstructed afterwards
/// for one reason: several of them — how much sun reached a point, how much sky
/// the canopy left it — are computed in this loop and nowhere else. Rebuilding
/// them from the finished plate would be guessing at the renderer's own working,
/// which is exactly what a training target must not require.
pub fn resolve_passes(
    surface: &Surface,
    page: &Page,
    lattice: &Macro,
    params: &BakeParams,
    shadows: Option<&[ShadowMap]>,
    mut passes: Option<&mut Passes>,
) -> Vec<Vec3> {
    if let Some(passes) = passes.as_deref_mut() {
        passes.resize(page.width * page.height);
    }
    let (width, height) = (page.width, page.height);
    let heights = surface.height_map(width, height);
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
    // Both radii are authored in reference pixels and scaled to this page, so
    // they keep asking about the same distance of *ground* however coarsely the
    // page is baked. A radius that did not scale would ask about half a metre on
    // one page and two metres on its neighbour.
    let near = blur(&heights, width, height, page.radius(3));
    let far = blur(&heights, width, height, page.radius(52));
    // Which way to look for the canopy a bunch is standing against — see
    // [`BakeParams::canopy_relief`]. Toward the key, so that a pixel on the
    // sunward flank of a bunch is compared with the open ground in front of it
    // and a pixel at its shaded foot is compared with the bunch itself.
    let toward = Vec2::new(params.light.x, params.light.y).normalize_or(Vec2::NEG_Y);

    // The crown surface: the canopy blurred at the scale of a tuft, so its
    // gradient is the shape of the *bunch* rather than of any blade in it. This
    // is the middle of the three normals — the one that makes a clump read as a
    // lit mass instead of as a collection of individually correct leaves.
    //
    // A tuft is a fifth of a metre; blurring at a third of that keeps the crown's
    // own shoulder while removing the blades, which is exactly the separation the
    // term needs.
    let crown_surface = blur(&heights, width, height, page.radius(CROWN_BLUR));

    // How much sky the canopy's own shape lets through, scanned by direction.
    //
    // The radii span two orders of magnitude on purpose: the near ones are one
    // blade against the blade behind it, the middle ones the inside of a tuft,
    // and the far ones a crown against the valley beside it. All three are
    // occlusion and they are not the same shape, which is why a single blur
    // radius could never stand in for the set.
    let horizon = if params.quality.ao_directions() > 0 {
        let radii: Vec<f32> = AO_RADII.iter().map(|r| page.radius(*r) as f32).collect();
        lighting::horizon_occlusion(
            &heights,
            width,
            height,
            params.quality.ao_directions(),
            &radii,
        )
    } else {
        // The streaming tier gets the cheap answer it always had: how far below
        // its own neighbourhood a pixel sits. Wrong about direction, right about
        // magnitude, and a hundred times faster.
        let coarse = blur(
            &heights,
            width,
            height,
            page.radius(AO_RADII[AO_RADII.len() - 1]),
        );
        (0..width * height)
            .map(|i| ((coarse[i] - heights[i]) * 0.03).clamp(0.0, 1.0))
            .collect()
    };

    let shadow = directional_shadow(&heights, width, height, params.light, page.detail());
    // Five pixels, not two. Sunlight through a canopy has no sharp edge to it;
    // the shadow this term describes is cast by grass onto grass a few
    // centimetres away, and the penumbra of that is wider than the shadow.
    let shadow = blur(&shadow, width, height, page.radius(5));

    // The sun, in the only space a surface normal lives in.
    let sun = iso::image_to_world(params.light).normalize_or(Vec3::Z);
    let half = lighting::half_vector(sun);
    let weights = FormWeights::default();
    let detail = page.detail();

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
            let _ = &far;
            // Signed, at the bunch scale, and read off toward the key — see
            // [`BakeParams::canopy_relief`]. Clamped into the page rather than
            // wrapped or mirrored: within `RELIEF_REACH` of an edge the offset
            // collapses back to the symmetric comparison, which is a gradual
            // softening of one term across ten pixels of a page that has already
            // been blurred by `light_blur`, and not a discontinuity.
            let sample = Vec2::new(fx, fy) + toward * page.px(RELIEF_REACH);
            let sx = sample.x.clamp(0.0, (width - 1) as f32) as usize;
            let sy = sample.y.clamp(0.0, (height - 1) as f32) as usize;
            let relief = ((canopy - far[sy * width + sx]) * 0.040).clamp(-1.0, 1.0) * open;

            // How strongly this area states its mound at all. Without it the
            // macro lighting describes every form equally and reads as a map of
            // the height field rather than as light falling on ground.
            let stated = lattice.at(&lattice.statement, fx, fy).clamp(0.0, 1.4);
            // Every term here varies over metres, so every one of them is
            // subject to [`BROAD_LIGHT`] as well as [`BROAD_DARK`]. The fast
            // terms — the tip lift, the glint, the blade's own form — are
            // applied per supersample below and keep both halves in full.
            let slow = params.raster.mound_light * broad(wrapped) * stated
                + params.raster.transmission * through
                + params.raster.elevation_light * broad(rise - 0.45)
                + params.raster.crown_light * broad(crown - 0.4)
                - params.raster.micro_occlusion * micro
                + params.raster.canopy_relief * broad(relief)
                - params.raster.shadow * shadow[index]
                + params.raster.region * broad(tint);
            // And a soft ceiling on their sum, which is a different failure from
            // any one of them being too strong. Six terms that each behave
            // perfectly can still agree, and where they do the light index runs
            // off the top of the ramp and a whole region goes flat pale — the
            // brightest thing in the picture carrying none of its detail.
            macro_light[index] = if slow > MACRO_KNEE {
                MACRO_KNEE + (slow - MACRO_KNEE) * 0.35
            } else {
                slow
            };
        }
    }

    let macro_light = blur(
        &macro_light,
        width,
        height,
        page.radius(params.raster.light_blur),
    );

    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            let index = y * width + x;
            let canopy = heights[index];
            let world = macro_light[index];

            // The two coarse normals are constant across the supersampled block
            // — they vary at the scale of a tuft and of the terrain, not of a
            // pixel — so they are read once per final pixel rather than sixteen
            // times.
            let ground_normal = lattice.ground_normal(fx, fy, detail);
            let canopy_normal = lighting::height_field_normal(
                Vec2::new(
                    sample_slope(&crown_surface, width, height, x, y, true),
                    sample_slope(&crown_surface, width, height, x, y, false),
                ),
                detail,
            );

            // Where this pixel's ground is, which is where a shadow lookup has
            // to happen. The canopy height lifts it: a blade three centimetres
            // up is shadowed by what is above *it*, not by what is above the
            // soil beneath it.
            let ground_here = page.ground_at(Vec2::new(fx + 0.5, fy + 0.5));

            let resolved = surface.resolve_pixel(x, y, |i| {
                let (albedo, tone) = surface.pixel(i);
                let blade_normal = surface.normal_at(i);

                // Form, at three scales. See [`crate::lighting`] for why all
                // three are blended rather than one being chosen.
                let form = lighting::form(weights, ground_normal, canopy_normal, blade_normal, sun);
                // Light that came *through* the leaf. Strongest at the tip,
                // where there is least material to get through, and gated on the
                // mark standing clear of the mass — a leaf buried in a tuft has
                // several centimetres of canopy behind it, not one blade.
                let thinness = surface.along_at(i) * (0.35 + exposure_of(surface.top_at(i)) * 0.65);
                let through = lighting::transmission(blade_normal, sun, thinness);
                // A broad waxy sheen, on the marks that have a face to catch it.
                let gloss = lighting::sheen(blade_normal, half, GLOSS_POWER)
                    * (0.35 + surface.maturity_at(i) * 0.65);

                // How much sun reaches this surface, averaged over the sun's
                // disc. One map gives a hard edge; several sampled directions
                // give a penumbra that widens with distance from the caster,
                // which is what a shadow does and what a blur cannot imitate.
                let sunlight = match shadows {
                    Some(maps) if !maps.is_empty() => {
                        let at = ground_here.extend(surface.top_at(i) * iso::METRES_PER_PX_UP);
                        let total: f32 = maps
                            .iter()
                            .map(|map| map.visibility(at, blade_normal))
                            .sum();
                        total / maps.len() as f32
                    }
                    _ => 1.0,
                };
                // Sky fill first, then whatever direct light survives the
                // shadow. Splitting them is the whole reason the G-buffer holds
                // a normal rather than a shaded value: a shadow has to take out
                // the *direct* term and leave the ambient one, or a shaded blade
                // loses its form as well as its light.
                // Occlusion, from the two places it comes from. The canopy's
                // shape decides how much sky arrives; the stack of leaves at
                // this pixel decides how much of what arrives gets through.
                //
                // They multiply rather than add, because they are independent
                // attenuations of the same light — a point deep inside a tuft in
                // a hollow is darker than either would make it, which is what
                // the reference art's cavities look like.
                let stacked = lighting::optical_occlusion(
                    surface.optical_at(i),
                    params.raster.interior_density,
                );
                let sky = (1.0 - params.raster.ambient_occlusion * horizon[index])
                    * (1.0 - params.raster.interior * stacked);
                // How much light of any kind reaches this surface, `0..1`.
                //
                // Sky fill and direct sun, separately, because a shadow takes
                // out one and leaves the other. Splitting them is the whole
                // reason the G-buffer holds a normal rather than a shaded value.
                //
                // The fill is also what keeps a cavity green. The fix for a
                // shadow that has gone too dark is always more of this and never
                // a weaker sun — grass in shade is dim and saturated, and a
                // shadow with no fill in it reads as a hole in the plate.
                let light =
                    params.raster.sky_fill * sky + (1.0 - params.raster.sky_fill) * sunlight;
                let lit = albedo
                    + world
                    + params.raster.form_light * form * light
                    + params.raster.leaf_transmission * through
                    + params.raster.gloss * gloss * sunlight
                    + lighting::underside_fill(surface.underside_at(i));
                // And then light carries the surface along the ramp, rather than
                // a fixed amount being taken off it — see
                // [`BakeParams::shade_depth`]. This is what lets a bright blade
                // in deep shade end up darker than a dim one in full sun, which
                // no additive term can do.
                let q = shoulder(lit) - (1.0 - light) * params.raster.shade_depth;
                // Through the material axes rather than the bare ramp — see
                // [`palette::Material`]. One index cannot say *which* kind of
                // bright a bright pixel is, and the field's whole remaining
                // complaint was that bright meant lime everywhere.
                let material = palette::Material {
                    exposure: light,
                    along: surface.along_at(i),
                    maturity: surface.maturity_at(i),
                };
                let colour = palette::shade_material(tone, q, material);
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
            let ground_at = page.ground_at(Vec2::new(fx, fy));
            let dampness = field.jitter(Stream::Tint, ground_at, 0.55);
            let shade_depth = (1.0 - (canopy / CANOPY_CEILING)).clamp(0.0, 1.0);
            let cool = params.raster.cool * shade_depth * (0.4 + dampness * 0.8);
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
            let calm = params.raster.temper * (1.0 - smoothstep(0.28, 0.62, luma));
            let muted = hue_only(resolved, Vec3::splat(luma) * Vec3::new(0.80, 1.14, 0.84));
            let resolved = resolved.lerp(muted, calm.clamp(0.0, 1.0));

            // Then the region's own hue, which is keyed to nowhere near the same
            // thing — see [`BakeParams::drift`]. Both ends are gentle multiples
            // of the colour already resolved rather than blends toward a named
            // paint, so the ramp's measured relationship between its channels
            // survives the drift and only its balance moves.
            let drift = lattice.at(&lattice.hue, fx, fy).clamp(-1.0, 1.0) * params.raster.drift;
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

            if let Some(passes) = passes.as_deref_mut() {
                // Averaged over the supersampled block, exactly as the colour
                // is, so every channel describes the same pixel.
                let step = surface.supersample();
                let inverse = 1.0 / (step * step) as f32;
                let (mut albedo, mut normal) = (0.0f32, Vec3::ZERO);
                let (mut along, mut optical, mut mature) = (0.0f32, 0.0f32, 0.0f32);
                for sy in 0..step {
                    for sx in 0..step {
                        let i = surface.index(x * step + sx, y * step + sy);
                        albedo += surface.pixel(i).0;
                        normal += surface.normal_at(i);
                        along += surface.along_at(i);
                        optical += surface.optical_at(i);
                        mature += surface.maturity_at(i);
                    }
                }
                passes.albedo[index] = albedo * inverse;
                passes.normal[index] = normal.normalize_or(Vec3::Z);
                passes.along[index] = along * inverse;
                passes.optical[index] = optical * inverse;
                passes.maturity[index] = mature * inverse;
                passes.height[index] = canopy;
                passes.occlusion[index] = horizon[index];
                passes.ground_normal[index] = ground_normal;
                passes.canopy_normal[index] = canopy_normal;
                // One lookup at the canopy's own height rather than sixteen —
                // this pass is a summary, and the sun does not vary inside a
                // final pixel by anything a network could use.
                passes.sunlight[index] = match shadows {
                    Some(maps) if !maps.is_empty() => {
                        let at = ground_here.extend(canopy * iso::METRES_PER_PX_UP);
                        maps.iter()
                            .map(|map| map.visibility(at, canopy_normal))
                            .sum::<f32>()
                            / maps.len() as f32
                    }
                    _ => 1.0,
                };
            }

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
            glaze_mask[index] =
                params.raster.glaze * (0.15 + loose * 0.85) * (1.0 - exposure).powf(1.2);
        }
    }

    glaze(
        &mut colours,
        width,
        height,
        &glaze_mask,
        page.radius(GLAZE_REACH),
    );
    soften(&mut colours, width, height, params.raster.soften);
    colours
}

/// Blend each pixel toward the average colour of its neighbourhood.
///
/// How far the glaze reaches, in reference cache pixels.
const GLAZE_REACH: usize = 2;

/// A five-tap cross at two pixels, rather than a proper blur: the aim is to melt
/// adjacent strokes into one another, not to smear the page. Anything wider
/// starts eating the marks that were meant to survive.
fn glaze(colours: &mut [Vec3], width: usize, height: usize, mask: &[f32], reach: usize) {
    let source = colours.to_vec();
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            let amount = mask[index];
            if amount <= 0.01 {
                continue;
            }
            let left = x.saturating_sub(reach);
            let right = (x + reach).min(width - 1);
            let up = y.saturating_sub(reach);
            let down = (y + reach).min(height - 1);
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
fn directional_shadow(
    heights: &[f32],
    width: usize,
    height: usize,
    light: Vec3,
    detail: f32,
) -> Vec<f32> {
    let plane = Vec2::new(light.x, light.y);
    let toward = plane.normalize_or(Vec2::NEG_Y);
    // Height a blocker must gain per pixel travelled to shade this point.
    let rise = (light.z / plane.length().max(1.0e-3)).clamp(0.3, 4.0);

    const STEPS: usize = 9;
    const STEP: f32 = 1.4;
    // Never below a page pixel, and the count cut to match so the ray still
    // covers the same ground. A coarse page reaches the same distance in fewer,
    // longer strides, which is the most a page of that resolution can say.
    let step_page = (STEP * detail).max(1.0);
    let steps = (((STEPS as f32 * STEP * detail) / step_page).round() as usize).max(1);
    let mut shadow = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let base = heights[y * width + x];
            let mut most = 0.0f32;
            for step in 1..=steps {
                // Two distances for one march, and the page's is the one that
                // has to be honest. The height field is sampled by whole page
                // pixels — there is nothing between them — so the step taken is
                // rounded up to one, and the *reference* distance the rise term
                // compares against is then derived from the step actually taken
                // rather than from the one that was asked for. Getting that
                // backwards is what a first attempt did: at an eighth scale it
                // asked for a step of 0.175 page pixels, truncation turned every
                // one of them into a whole pixel — eight reference pixels of
                // ground — and the rise threshold went on being computed for
                // 1.4. The shadows came out several times too strong, and which
                // way they leaned depended on the sign of the light.
                let along = step as f32 * step_page;
                let distance = along / detail.max(1.0e-3);
                let sample = Vec2::new(x as f32, y as f32) + toward * along;
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

    #[test]
    fn a_region_tiles_its_own_pages_exactly() {
        let region = BakeRegion::square(Vec2::new(-256.0, -256.0), 2, 128);
        let whole = region.whole();
        assert_eq!(whole.width, 256);
        assert_eq!(whole.height, 256);
        assert_eq!(region.count(), 4);
        // Every tile's corner has to land on the region grid, or a crop takes
        // the wrong pixels.
        for y in 0..region.pages.1 {
            for x in 0..region.pages.0 {
                let tile = region.tile(x, y);
                let offset = tile.origin - region.origin;
                assert_eq!(offset.x, (x * region.page_pixels) as f32);
                assert_eq!(offset.y, (y * region.page_pixels) as f32);
            }
        }
    }

    #[test]
    fn cropping_a_region_takes_the_right_pixels() {
        // A plate whose every pixel encodes its own coordinate, so a crop that
        // is off by a row or a column cannot pass.
        let region = BakeRegion::square(Vec2::ZERO, 2, 4);
        let whole = region.whole();
        let plate: Vec<Vec3> = (0..whole.width * whole.height)
            .map(|i| {
                let (x, y) = (i % whole.width, i / whole.width);
                Vec3::new(x as f32, y as f32, 0.0)
            })
            .collect();
        for ty in 0..2 {
            for tx in 0..2 {
                let page = region.crop(&plate, tx, ty);
                assert_eq!(page.len(), 16);
                for row in 0..4 {
                    for column in 0..4 {
                        let pixel = page[row * 4 + column];
                        assert_eq!(pixel.x, (tx * 4 + column) as f32);
                        assert_eq!(pixel.y, (ty * 4 + row) as f32);
                    }
                }
            }
        }
    }
    use super::*;
    // The guard-band tests live here rather than beside `MARGIN` because they
    // measure the whole path: placement decides which cells are visited, and
    // only the rasteriser knows where the paint actually lands. A test that
    // reasoned about the band without drawing anything would certify arithmetic
    // rather than pixels, which is exactly the failure they exist to prevent.
    use crate::placement::{BEND_CEILING, MARGIN, TUFT_RADIUS, VIGOUR_CEILING};
    use crate::stroke::Stroke;

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
        //
        // ## Measured against its own neighbourhood, not against the whole plate
        //
        // The obvious test — "the seam must step less than any other column pair
        // on the plate" — compares one sample against the maximum of five
        // hundred, which is an extreme order statistic and therefore a coin
        // toss whenever the seam sits anywhere near the top of the distribution.
        // It passed for a long time and then failed by under two percent when
        // the canopy got denser, which is not a signal about seams.
        //
        // So the comparison is local. A join is invisible when it steps like the
        // ground either side of it steps; whether some column four hundred
        // pixels away happens to step more is not evidence about anything. The
        // window controls for the field's own variation, which is the quantity
        // that made the global test unstable.
        const WIDTH: usize = 512;
        const HEIGHT: usize = 256;
        let region = BakeRegion {
            origin: Vec2::new(-256.0, -128.0),
            pages: (2, 1),
            page_pixels: 256,
            px_per_metre: iso::PX_PER_METRE,
        };
        // Two pages, each baked *padded* and cropped, then laid side by side.
        // That is the offline path, and it is the one the claim is about: a page
        // whose shading terms saw the ground beyond its own edge has nothing left
        // to disagree with its neighbour about.
        let mut plate = vec![Vec3::ZERO; WIDTH * HEIGHT];
        for x in 0..2 {
            let tile = bake_padded(region.tile(x, 0), &BakeParams::default());
            for row in 0..HEIGHT {
                let from = row * 256;
                let to = row * WIDTH + x * 256;
                plate[to..to + 256].copy_from_slice(&tile[from..from + 256]);
            }
        }

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

        // Against the same ground baked in one piece, which is the version with
        // no join in it at all.
        //
        // This is the formulation that finally asked the right question. Earlier
        // ones compared the join's column step against the *rest of the plate's*
        // column steps, and that conflates two things: how much a join disturbs
        // the picture, and how much the meadow itself varies from column to
        // column. The meadow varies a great deal — a bright crown moves a column
        // mean by several times the typical step — so the comparison was mostly
        // measuring the field's own tail, and it moved whenever the canopy did.
        //
        // Baking the identical rectangle whole removes the field from the
        // question entirely. Whatever the ground does at that column, both
        // versions do it; the only thing that differs is whether a page boundary
        // ran through it.
        let unbroken = bake_padded(region.whole(), &BakeParams::default());
        let unbroken_column = |x: usize| -> f32 {
            (0..HEIGHT)
                .map(|y| {
                    let c = unbroken[y * WIDTH + x];
                    c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
                })
                .sum::<f32>()
                / HEIGHT as f32
        };

        let join = WIDTH / 2;
        // How far each version's column mean sits from the other's, near the
        // join and away from it.
        let drift = |x: usize| (column(x) - unbroken_column(x)).abs();
        let at_join = drift(join).max(drift(join - 1));
        let away: Vec<f32> = (8..WIDTH - 8)
            .filter(|x| x.abs_diff(join) > 24)
            .map(drift)
            .collect();
        let typical_drift = away.iter().sum::<f32>() / away.len() as f32;

        assert!(
            at_join < typical_drift * 4.0 + 1.0e-3,
            "the columns either side of the join sit {at_join:.5} from where the \
             same ground lands when it is baked in one piece, against \
             {typical_drift:.5} elsewhere — cutting the page grid through this \
             column changed what is drawn there"
        );
        // And the join must not step in a way the unbroken bake does not: a real
        // seam is a difference between the two versions, not a feature of the
        // ground that both share.
        let seam = step(join);
        let unbroken_seam = (unbroken_column(join) - unbroken_column(join - 1)).abs();
        assert!(
            seam < unbroken_seam + typical_drift * 4.0 + 2.0e-3,
            "the join steps by {seam:.5} where the unbroken bake steps by \
             {unbroken_seam:.5} at the same column"
        );
    }

    #[test]
    fn the_macro_lattice_samples_the_same_world_points_from_any_page() {
        // The property the shared shadow pass and the training crops both need,
        // and the one the lattice did not have: two pages cut on different grids
        // have to read the composition fields from the *same* places wherever
        // they overlap.
        //
        // Measured through `Macro::at` rather than by inspecting the arrays,
        // because the arrays are indexed differently by construction — a page
        // whose corner falls mid-stride holds a different number of lattice
        // lines. What has to agree is the value handed back for a given patch of
        // world, which is what every consumer actually asks for.
        let field = WorldField::lit_by(0x5eed_1234, BakeParams::default().light);

        // Two pages overlapping the same ground, cut on grids offset by an
        // amount that is deliberately not a multiple of the six-pixel stride.
        let left = Page::new(Vec2::new(0.0, 0.0), 128, 128);
        let right = Page::new(Vec2::new(-58.0, -22.0), 128, 128);
        let a = Macro::build(&left, &field);
        let b = Macro::build(&right, &field);

        let mut worst = 0.0f32;
        for step in 0..40 {
            for other in 0..40 {
                // A world point inside both pages, addressed in each page's own
                // pixel coordinates.
                let cache = left.origin + Vec2::new(step as f32 * 1.7, other as f32 * 1.7);
                let (ax, ay) = (cache - left.origin).into();
                let (bx, by) = (cache - right.origin).into();
                for source in [&a.bare, &a.hue, &a.lit]
                    .into_iter()
                    .zip([&b.bare, &b.hue, &b.lit])
                {
                    let difference = (a.at(source.0, ax, ay) - b.at(source.1, bx, by)).abs();
                    worst = worst.max(difference);
                }
            }
        }
        assert!(
            worst < 1.0e-5,
            "two pages disagree about the composition fields by {worst} — the \
             lattice is anchored to the page rather than to the world"
        );
    }

    #[test]
    fn where_the_page_grid_falls_is_a_boundary_effect_and_nothing_more() {
        // What the world-anchored lattice actually bought, stated as the claim
        // it supports rather than as a single number.
        //
        // A tiled bake and a whole-region bake can never be identical: the
        // occlusion terms, the directional shadow and the glaze all read a
        // neighbourhood, and near a page edge that neighbourhood is cropped
        // where the region has it complete. The widest of those reads half a
        // metre, so a band that wide either side of every join is expected to
        // differ and there is no arrangement of the code that avoids it.
        //
        // What *was* avoidable is the composition differing as well. The lattice
        // used to be laid from each page's own corner at a six-pixel stride, so
        // two pages read the fields from different points everywhere, not just
        // near the join — and the damage was spread across the whole plate
        // rather than confined to its edges.
        //
        // So this measures the interior and the join band separately. The
        // interior is the claim; the join band is reported for scale.
        let params = BakeParams::default();
        let region = BakeRegion {
            origin: Vec2::new(-256.0, -128.0),
            pages: (2, 1),
            page_pixels: 256,
            px_per_metre: iso::PX_PER_METRE,
        };
        let whole = bake(region.whole(), &params);
        let (width, height) = (region.whole().width, region.whole().height);

        // As wide as the widest neighbourhood any shading term reads. That is
        // the half-metre `far` blur in `resolve` — but at *twice* its stated
        // radius, because [`crate::surface::blur`] runs its box pass twice to
        // approximate a Gaussian, and two boxes of radius 52 reach about 104
        // pixels. Worth knowing on its own: the canopy-relief term's reach is
        // very nearly a whole page at the size the renderer streams, which is
        // most of why a tiled bake and a region bake can differ at all.
        const JOIN_BAND: usize = 112;
        let near_a_join = |x: usize| {
            let seam = width / 2;
            x < JOIN_BAND || x >= width - JOIN_BAND || x.abs_diff(seam) < JOIN_BAND
        };

        let (mut interior, mut interior_count) = (0.0f64, 0usize);
        let (mut band, mut band_count) = (0.0f64, 0usize);
        for x in 0..region.pages.0 {
            let tile = bake(region.tile(x, 0), &params);
            for row in 0..height {
                for column in 0..region.page_pixels {
                    let difference = (tile[row * region.page_pixels + column]
                        - whole[row * width + x * region.page_pixels + column])
                        .length() as f64;
                    if near_a_join(x * region.page_pixels + column) {
                        band += difference;
                        band_count += 1;
                    } else {
                        interior += difference;
                        interior_count += 1;
                    }
                }
            }
        }
        let interior = interior / interior_count.max(1) as f64;
        let band = band / band_count.max(1) as f64;

        // A ratio, not just a threshold, and the ratio is the actual claim. An
        // absolute bound alone would pass just as well if every part of the
        // plate were equally slightly wrong, which is precisely the failure the
        // world-anchored lattice was built to remove.
        assert!(
            band > interior * 4.0,
            "the join band ({band:.6}) is no worse than the interior \
             ({interior:.6}), so the disagreement is spread over the plate \
             rather than confined to its edges — the composition lattice is not \
             anchored in the world"
        );
        // And the interior has to be invisible in absolute terms too: a
        // twentieth of one 8-bit step.
        assert!(
            interior < 2.0 / 255.0 / 20.0,
            "the plate's interior moved by {interior:.6} depending on where the \
             page grid was laid"
        );
    }

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
        use crate::placement::CANOPY_METRES;
        let mut tallest = 0.0f32;
        for (index, origin) in crate::fixtures::PLACES.iter().enumerate() {
            let params = BakeParams {
                seed: bw_seed(index),
                quality: GrassRenderQuality::Reference,
                ..Default::default()
            };
            let page = Page::new(*origin, 192, 192);
            let field = WorldField::lit_by(params.seed, params.light);
            let scene = GrassScene::build(page, &field, &params.grass());
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

    /// A stable per-place seed, so the sweep above covers different worlds.
    fn bw_seed(index: usize) -> u64 {
        0x5eed_1234u64.wrapping_add(index as u64 * 0x9e37_79b9)
    }

    #[test]
    fn the_shadow_guard_covers_every_caster_that_can_reach_a_page() {
        // Measured against the sun rather than against a constant, and swept
        // down to the lowest elevation the renderer claims to support. Getting
        // this wrong at 35° and right at 55° is exactly the shape of the bug
        // this exists to prevent.
        use crate::placement::{Bed, CANOPY_METRES, footprint};
        let field = WorldField::lit_by(1, BakeParams::default().light);
        for degrees in [35.0f32, 45.0, 55.0] {
            let elevation = degrees.to_radians();
            let params = BakeParams {
                quality: GrassRenderQuality::Reference,
                light: crate::sun::Key {
                    azimuth: 0.0,
                    elevation,
                }
                .direction(),
                ..Default::default()
            };
            for detail in [1.0f32, 0.5, 0.25] {
                let page = Page::at_detail(Vec2::new(-64.0, -64.0), 128, 128, detail);
                let bed = Bed {
                    page: &page,
                    field: &field,
                    params: &params.grass(),
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
    fn turning_the_sun_relights_the_field_without_regrowing_it() {
        // The gate for the whole lighting phase, and it has two halves that a
        // single "did the picture change" would not separate.
        //
        // The picture has to change, obviously — that is the complaint the
        // normals were built to answer, and before them four bearings ninety
        // degrees apart gave four indistinguishable plates.
        //
        // But the *geometry* must not. Rotating a key light is a shading
        // operation; if it also moved a blade then the field would not be
        // relightable, the training pairs would be two different meadows, and no
        // amount of good lighting would make up for it.
        let page = Page::new(Vec2::new(-64.0, -64.0), 96, 96);
        let elevation: f32 = 0.7;
        let bake_at = |bearing: f32| {
            let a: f32 = bearing;
            let world = Vec3::new(
                a.cos() * elevation.cos(),
                a.sin() * elevation.cos(),
                elevation.sin(),
            );
            let params = BakeParams {
                light: iso::world_to_image(world).normalize(),
                ..Default::default()
            };
            let field = WorldField::lit_by(params.seed, params.light);
            let scene = GrassScene::build(page, &field, &params.grass());
            let colours = bake(page, &params);
            (scene, colours)
        };

        let (first_scene, first) = bake_at(0.0);
        let (turned_scene, turned) = bake_at(std::f32::consts::FRAC_PI_2);

        // Same meadow.
        assert_eq!(
            first_scene.len(),
            turned_scene.len(),
            "the sun regrew the field"
        );
        for (a, b) in first_scene.marks.iter().zip(&turned_scene.marks) {
            assert_eq!(a.root.to_array(), b.root.to_array());
            assert_eq!(a.length.to_bits(), b.length.to_bits());
            assert_eq!(a.twist.to_bits(), b.twist.to_bits());
        }

        // Different picture, and by a margin that reads rather than one that
        // only registers. A quarter of an 8-bit step averaged over the plate
        // would be arithmetic noise; this is a visible relight.
        let mean = first
            .iter()
            .zip(&turned)
            .map(|(a, b)| (*a - *b).length() as f64)
            .sum::<f64>()
            / first.len() as f64;
        assert!(
            mean > 0.02,
            "turning the sun a quarter turn moved the plate by {mean:.4}"
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
        let longest = params.style.blade_length.1 * 1.25 * VIGOUR_CEILING * 1.35;
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
            for bend in [1.3, 1.65, 2.0, 2.62, BEND_CEILING] {
                let mut surface = Surface::new(512, 512);
                let origin = Vec2::new(-256.0, -256.0);
                let mut painter = Painter::new(&mut surface, origin, params.light);
                let root = painter.to_ground(Vec2::splat(256.0 * painter.supersample()));
                painter.draw(&Stroke {
                    root: root.extend(0.0),
                    azimuth,
                    length: longest,
                    bend,
                    curl: 1.4,
                    sway: 2.4,
                    width: params.style.blade_width.1,
                    under: params.style.under,
                    ..Default::default()
                });
                // Density, not height. `top` is a canopy height in whole
                // pixels, so every rib at ground level — a root, an
                // under-stroke, anything a laid-over mark drags below its own
                // origin — paints the page and reports zero. Measuring reach by
                // height therefore measures the reach of the *tall* part of a
                // mark and calls it the reach of the mark. The density channel
                // counts writes, so it sees all of it.
                let painted = surface.painted_map(512, 512);
                for y in 0..512 {
                    for x in 0..512 {
                        if painted[y * 512 + x] > 0.0 {
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
                let root = painter.to_ground(Vec2::splat(256.0 * painter.supersample()));
                painter.draw(&Stroke {
                    root: root.extend(0.0),
                    azimuth,
                    length: longest,
                    bend: *bend,
                    curl: 1.4,
                    sway: 2.4,
                    width: params.style.blade_width.1,
                    under: params.style.under,
                    ..Default::default()
                });
                // Density, not height. `top` is a canopy height in whole
                // pixels, so every rib at ground level — a root, an
                // under-stroke, anything a laid-over mark drags below its own
                // origin — paints the page and reports zero. Measuring reach by
                // height therefore measures the reach of the *tall* part of a
                // mark and calls it the reach of the mark. The density channel
                // counts writes, so it sees all of it.
                let painted = surface.painted_map(512, 512);
                for y in 0..512 {
                    for x in 0..512 {
                        if painted[y * 512 + x] > 0.0 {
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
        let longest = params.style.blade_length.1 * 1.25 * VIGOUR_CEILING * 1.35;
        // Up to `Tangle`'s ceiling, plus what the skirt adds on top of it.
        let bends = [0.9, 1.4, params.style.blade_bend.1, 2.0, BEND_CEILING];
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

    /// The per-stroke cull is only sound while its bound is a real bound.
    ///
    /// [`Painter::reach`] is what lets [`paint`] throw away two marks in three
    /// before rasterising them, and it is derived rather than measured — from
    /// the fact that an arc cannot displace its tip further than its own length,
    /// and from the largest that displacement can project to. A derivation can
    /// be wrong, and the symptom of a bound that is too tight is the worst one
    /// this design has: a mark drawn on one side of a page join and missing on
    /// the other, invisible in any still that does not contain a join.
    ///
    /// So it is checked against the rasteriser itself, across the vocabulary's
    /// full range of length, bend and azimuth. `reach_by_direction` returns how
    /// far a drawn mark actually got from its root in each of the four
    /// directions; every one of them has to fit inside the bound.
    #[test]
    fn the_stroke_reach_bound_is_never_beaten() {
        let params = BakeParams::default();
        let longest = params.style.blade_length.1 * 1.25 * VIGOUR_CEILING * 1.35;
        let bends = [0.0, 0.9, 1.4, params.style.blade_bend.1, 2.0, BEND_CEILING];
        let (left, right, up, down) = reach_by_direction(&params, longest, &bends);

        let stroke = Stroke {
            length: longest,
            width: params.style.blade_width.1,
            under: params.style.under,
            ..Default::default()
        };
        let mut surface = Surface::new(8, 8);
        let bound = Painter::new(&mut surface, Vec2::ZERO, params.light).reach(&stroke);
        let worst = left.max(right).max(up).max(down);
        assert!(
            bound > worst,
            "a mark reached {worst:.1} px from its root and the cull bound is \
             {bound:.1} px, so the cull can drop a mark that would have drawn"
        );
        // The other half of the property: a bound that is merely enormous is
        // sound and useless. This one should be close to the truth, because
        // every pixel of slack is strokes rasterised for nothing.
        assert!(
            bound < worst * 2.0,
            "the cull bound is {bound:.1} px for a {worst:.1} px reach, which \
             buys back much less of the stroke pass than it could"
        );
    }

    /// The cull bound has to hold at every scale a page can be baked at.
    ///
    /// The sweep above checks it against the rasteriser at the authoring scale
    /// only, and the bound is not scale-free: it multiplies a world length by
    /// the page's own pixels-per-metre and adds three widths that are authored
    /// in reference pixels and scaled separately. Two quantities that scale by
    /// different factors is exactly the shape of arithmetic that comes out right
    /// at one scale and wrong at another — and wrong here means a mark culled on
    /// a coarse page that a fine page would have drawn.
    #[test]
    fn the_reach_bound_holds_at_every_page_scale() {
        let params = BakeParams::default();
        let stroke = Stroke {
            length: params.style.blade_length.1 * 1.25 * VIGOUR_CEILING * 1.35,
            width: params.style.blade_width.1,
            under: params.style.under,
            ..Default::default()
        };
        let mut surface = Surface::new(8, 8);
        let full = Painter::new(&mut surface, Vec2::ZERO, params.light).reach(&stroke);

        for detail in [1.0f32, 0.5, 0.25, 0.125] {
            let mut surface = Surface::new(8, 8);
            let bound = Painter::at_scale(
                &mut surface,
                Vec2::ZERO,
                params.light,
                iso::PX_PER_METRE * detail,
            )
            .reach(&stroke);
            // Every term but the one-pixel rasterisation guard is a length on
            // the page, so all of them scale with it and none of them may be
            // left behind.
            let expected = (full - 1.0) * detail + 1.0;
            assert!(
                (bound - expected).abs() < 1.0e-3,
                "at detail {detail} the bound is {bound:.3} px where the same \
                 mark's reach scales to {expected:.3} px"
            );
        }
    }

    use crate::fixtures::PLACES;

    /// A page baked coarsely has to be the page baked finely and then shrunk.
    ///
    /// This is the whole justification for [`Page::at_detail`]. The camera shows
    /// the ground at about a fifth, so the cache the player actually sees is
    /// twenty-four times smaller than the one being baked, and the difference is
    /// thrown away by the sampler. Baking at the scale the page is *shown* at is
    /// only allowed if it lands somewhere the minification filter would have
    /// landed anyway — otherwise it is not a level of detail, it is different
    /// grass.
    ///
    /// So: the same ground, twice. Once at the authoring scale and area-averaged
    /// down, once baked coarse to begin with.
    #[test]
    fn a_coarse_page_agrees_with_a_minified_fine_one() {
        const FINE: usize = 256;
        let params = BakeParams::default();
        let fine = bake(Page::new(PLACES[0], FINE, FINE), &params);

        // Every level the ladder offers, including the one the shipping camera
        // lands on. A single level is a spot check, and the way this fails is by
        // drifting further at each step down.
        for detail in [0.5f32, 0.25, 0.2, 0.125] {
            let side = (FINE as f32 * detail) as usize;
            let shrunk = crate::surface::resample(&fine, FINE, FINE, side, side);
            let coarse = bake(
                Page::at_detail(PLACES[0] * detail, side, side, detail),
                &params,
            );
            let similarity = crate::compare::compare(&coarse, &shrunk, side, side);

            assert!(
                similarity.luma_drift.abs() < 0.03,
                "at detail {detail} the coarse page is {:.4} off in tone: {similarity:?}",
                similarity.luma_drift
            );
            // Not a luminance test, and that is the point. `luma_drift` and
            // `detail_ratio` are both blind to a page that is the right
            // brightness and the right busy-ness in the wrong *places*, and
            // blind to hue entirely — which is exactly how a world lookup left
            // at the authoring scale hid here for a while. It moved the
            // cool-shadow field across the page without touching a single
            // luminance statistic. SSIM sees the arrangement; RMSE is over all
            // three channels.
            assert!(
                similarity.ssim > 0.55,
                "at detail {detail} the coarse page is arranged differently \
                 (ssim {:.4}): {similarity:?}",
                similarity.ssim
            );
            assert!(
                similarity.rmse < 0.075,
                "at detail {detail} the coarse page differs by {:.4} rmse over \
                 all three channels: {similarity:?}",
                similarity.rmse
            );
            // And a cheap page that is cheap because it is blurrier passes every
            // test of tone and of arrangement. This is the one that catches it.
            assert!(
                similarity.detail_ratio > 0.72 && similarity.detail_ratio < 1.4,
                "at detail {detail} the coarse page holds {:.2} of the fine \
                 one's local contrast: {similarity:?}",
                similarity.detail_ratio
            );
        }
    }

    /// Page independence has to survive the detail levels too.
    ///
    /// Every placement decision is a pure function of a world coordinate, and
    /// [`crate::field::GroundCache`] quantises that coordinate onto a lattice
    /// whose spacing comes from the page's *scale*. If it ever came from the
    /// page's origin instead, two neighbouring pages would quantise the same
    /// point differently and the join between them would open up.
    #[test]
    fn coarse_pages_meet_without_a_seam() {
        const DETAIL: f32 = 0.25;
        const SIDE: usize = 64;
        let params = BakeParams::default();
        let origin = PLACES[0] * DETAIL;

        let left = bake(Page::at_detail(origin, SIDE, SIDE, DETAIL), &params);
        let right = bake(
            Page::at_detail(origin + Vec2::new(SIDE as f32, 0.0), SIDE, SIDE, DETAIL),
            &params,
        );

        // Column means, the way `pages_meet_without_a_seam` reads it, and for
        // the same reason: a whole-plate average is blind to a one-column step,
        // which is precisely what a broken quantisation would produce. Averaging
        // down a column removes the stroke noise and leaves the slowly-varying
        // part — the part a lattice mismatch disturbs.
        let column = |plate: &[Vec3], x: usize| -> f32 {
            (0..SIDE)
                .map(|y| {
                    let c = plate[y * SIDE + x];
                    c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
                })
                .sum::<f32>()
                / SIDE as f32
        };
        // The join itself: the last column of the left page against the first of
        // the right one.
        let seam = (column(&right, 0) - column(&left, SIDE - 1)).abs();
        let interior: Vec<f32> = (1..SIDE)
            .map(|x| (column(&left, x) - column(&left, x - 1)).abs())
            .collect();
        let worst = interior.iter().copied().fold(0.0f32, f32::max);
        let typical = interior.iter().sum::<f32>() / interior.len() as f32;

        assert!(
            seam <= worst,
            "the coarse page join steps by {seam:.5}, more than any ordinary \
             column pair inside a page (typical {typical:.5}, worst {worst:.5})"
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

/// Several output pages baked as one piece of ground.
///
/// The runtime still consumes 256-pixel pages, and nothing about that changes.
/// What changes is that the offline renderer stops baking them one at a time.
///
/// Three things want a region rather than a page. A cast shadow crosses page
/// boundaries, and a shadow map built per page has to guard for casters it will
/// then throw away — build it once over four pages and the guard is paid for
/// once instead of four times. Patch and tuft structure spans page edges, so a
/// training crop taken from a region carries genuine neighbourhood context
/// rather than context that stops at a border. And the fixed per-page costs —
/// the field, the lattice, the guard band's own area — amortise.
///
/// The region is baked as one large [`Page`] and cut afterwards, which is what
/// makes the output identical to a page bake wherever it can be: the world is
/// sampled on the same world-anchored lattice either way, and the only remaining
/// difference is that a neighbourhood read near a page edge is cropped in one
/// path and complete in the other.
#[derive(Clone, Copy, Debug)]
pub struct BakeRegion {
    /// Cache-pixel corner of the region's first page.
    pub origin: Vec2,
    /// Output pages across and down.
    pub pages: (usize, usize),
    /// Side of one output page, in cache pixels.
    pub page_pixels: usize,
    /// Cache pixels per world metre.
    pub px_per_metre: f32,
}

impl BakeRegion {
    /// A square region of `side × side` pages at the authoring scale.
    pub fn square(origin: Vec2, side: usize, page_pixels: usize) -> Self {
        Self {
            origin,
            pages: (side.max(1), side.max(1)),
            page_pixels,
            px_per_metre: crate::iso::PX_PER_METRE,
        }
    }

    /// The whole region as one page.
    pub fn whole(&self) -> Page {
        Page {
            origin: self.origin,
            width: self.pages.0 * self.page_pixels,
            height: self.pages.1 * self.page_pixels,
            px_per_metre: self.px_per_metre,
        }
    }

    /// One output page of the region.
    pub fn tile(&self, x: usize, y: usize) -> Page {
        Page {
            origin: self.origin
                + Vec2::new((x * self.page_pixels) as f32, (y * self.page_pixels) as f32),
            width: self.page_pixels,
            height: self.page_pixels,
            px_per_metre: self.px_per_metre,
        }
    }

    /// How many output pages the region holds.
    pub fn count(&self) -> usize {
        self.pages.0 * self.pages.1
    }

    /// Bake the whole region, correctly, and hand back the finished plate.
    ///
    /// Goes through [`crate::bake::bake_padded`], so every neighbourhood-reading
    /// shading term sees the ground that is actually there rather than whatever
    /// part of it fell inside the rectangle. The pad is a perimeter cost against
    /// an area of pages, which is the whole reason to bake a region rather than
    /// a page: one 256-pixel page padded for correctness costs three and a half
    /// times itself, a four-by-four region costs half again.
    ///
    /// Crop the result with [`BakeRegion::crop`] to get pages the runtime cache
    /// can hold.
    pub fn bake(&self, params: &crate::bake::BakeParams) -> Vec<Vec3> {
        crate::bake::bake_padded(self.whole(), params)
    }

    /// Cut one output page out of a finished region plate.
    pub fn crop(&self, plate: &[Vec3], x: usize, y: usize) -> Vec<Vec3> {
        let whole = self.whole();
        let side = self.page_pixels;
        let mut page = Vec::with_capacity(side * side);
        for row in 0..side {
            let start = (y * side + row) * whole.width + x * side;
            page.extend_from_slice(&plate[start..start + side]);
        }
        page
    }
}

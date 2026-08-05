//! The world fields: mounds, density, colour drift and bare ground.
//!
//! These are the composition layer. They decide *where* things go; the baker
//! decides what they look like once they are there. Keeping the two apart is
//! what lets a page be baked in isolation and still line up with its neighbours,
//! because every function here is a pure function of a world coordinate.
//!
//! ## Mounds are placed, not noised
//!
//! Fractal noise makes clouds. The reference art is made of identifiable
//! overlapping grass masses with tops and backs, and noise cannot produce that
//! because it has no notion of an object. So mounds come from a jittered point
//! process — one mound per grid cell, displaced within it — and are combined
//! with a smooth maximum rather than a sum, which keeps each mass rounded
//! instead of averaging neighbours into a plateau.
//!
//! The amplitude is deliberately small. Measured against the reference, the
//! luminance still varying after a 64-pixel blur has a standard deviation of
//! about 0.036 on a mean of 0.39 — under a tenth. Mounds in this art are a soft
//! organising rhythm, not terrain.

use std::cell::Cell;

use glam::{Vec2, Vec3};

use crate::rng::{Draw, Stream, fbm, value_noise};

/// Hermite ramp between two edges. The usual one.
#[inline]
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rotation applied to every field lattice, radians.
///
/// Every grid in this module is axis-aligned in *world* space, and the world
/// axes are exactly the two diagonals of an isometric screen. So an unrotated
/// lattice — mound cells, noise cells, patch cells — lays its faint regularity
/// down the screen diagonals, which is the one direction an isometric image is
/// already full of straight lines in. The eye finds it immediately.
///
/// A quarter of a right angle points the lattice at neither the world axes nor
/// the screen axes. The value is arbitrary; that it is not a multiple of a right
/// angle is not.
///
/// World point into the rotated frame every field is evaluated in.
pub const SKEW: f32 = 0.42;

#[inline]
fn skewed(p: Vec2) -> Vec2 {
    // cos and sin of `SKEW`.
    const COS: f32 = 0.913_089_3;
    const SIN: f32 = 0.407_760_3;
    Vec2::new(p.x * COS - p.y * SIN, p.x * SIN + p.y * COS)
}

/// Metres between mound centres.
///
/// About 154 cache pixels, which puts the resulting light-and-dark rhythm in the
/// 150–270 pixel band the reference's macro structure occupies.
pub const MOUND_SPACING: f32 = 1.6;

/// The range a mound's nominal size is drawn from, metres.
const MOUND_EXTENT: (f32, f32) = (0.46, 1.55);

/// The range a *stretched* mound's aspect ratio is drawn from.
///
/// The round minority is drawn from a narrower range inside this one, so the
/// upper end here bounds both.
const MOUND_ASPECT: (f32, f32) = (1.7, 3.5);

/// The furthest any mound's ellipse can reach from its own centre, metres.
///
/// The stretch is area-preserving, so a mound's semi-major axis is its extent
/// times the square root of its aspect, and both are bounded above:
/// `1.55 × √3.5 = 2.8998`. Rounded up once.
///
/// Two things depend on it and neither may exceed it. The five-by-five window in
/// [`WorldField::mounds`] has to be wide enough that no mound reaching this far
/// is missed — two cells is 3.2 metres, so it is — and the early rejection
/// inside that window has to admit every mound that could contribute.
/// `mound_reach_bounds_every_ellipse` checks the bound against the draws rather
/// than against this paragraph.
const MOUND_REACH: f32 = 2.9;

/// How far past its nominal size a bare patch can still be felt, as a multiple.
///
/// [`WorldField::blobs`] wobbles its boundary by up to five harmonics — a
/// combined 1.638 at their limits — and then erodes the rim by a factor of up to
/// 1.28, so the furthest a patch can contribute is 2.097 times its drawn size.
/// This is the bound rounded up, and both the per-blob early-out and the window
/// width are derived from it, so the two can never disagree.
const REJECT: f32 = 2.2;

/// The narrowest a bare patch's short axis gets, as a fraction of its round one.
///
/// Its reciprocal is how far the *long* axis reaches, which is what decides how
/// many cells of window the field has to scan.
const MIN_STRETCH: f32 = 0.648_074_1;

/// [`WorldField::sample`], read on a lattice and remembered.
///
/// ## Why the placement passes cannot afford the field
///
/// The composition fields are the expensive part of this crate after the
/// rasteriser: sixteen fractal lookups and a five-by-five window of mound cells,
/// something over a microsecond, and the baker reads one for **every blade of
/// grass it considers planting**. At the mat's three hundred and ninety-five
/// marks to the square metre that is twelve thousand reads a page, and they do
/// not get cheaper when the page does — which is why a page baked at an eighth
/// scale was still spending three quarters of its time here.
///
/// ## Why a lattice is allowed
///
/// The thing being decided is where a mark goes and how vigorous it is, and the
/// field answering it has nothing in it above the mound frequency — its finest
/// feature is a bare patch a good many centimetres across. Reading it every
/// two *page pixels* rather than every mark is therefore not an approximation of
/// the composition, it is sampling it at the resolution the page can hold. At
/// the authoring scale that lattice is a fiftieth of a metre, finer than the
/// spacing of the marks reading it, so almost nothing is shared and almost
/// nothing changes. At an eighth scale it is a sixth of a metre and a dozen
/// marks share one read, which is the entire point.
///
/// ## Why it stays page-independent
///
/// The lattice is anchored in the **world**, and its spacing comes from the
/// page's scale rather than from its position, so two pages baked at the same
/// detail quantise every point identically and agree along their join exactly as
/// they did before. The memo below is only a memo: hit or miss, the value
/// returned is [`WorldField::sample`] at the cell's centre, so a collision costs
/// a recomputation and never a different answer.
pub struct GroundCache<'a> {
    field: &'a WorldField,
    /// Metres between lattice points.
    step: f32,
    inverse_step: f32,
    /// Packed cell coordinate per slot, or [`i64::MIN`] for an empty one.
    keys: Vec<i64>,
    values: Vec<Ground>,
}

/// Slots in a [`GroundCache`]. A power of two, so the index is a mask.
///
/// Sized against the working set rather than against the whole page: a coarse
/// page's footprint holds a couple of thousand lattice cells, and a fine one
/// holds far more than would ever fit — but a fine page shares almost nothing
/// anyway, so its miss rate is the cost it was always paying.
const GROUND_SLOTS: usize = 8192;

/// How hard the curl bends the prevailing flow.
///
/// The curl arrives as a raw gradient difference, so its magnitude is arbitrary;
/// this is what turns it into a fraction of a unit vector. At this strength the
/// flow keeps a recognisable prevailing direction over a few metres while
/// developing arcs and eddies inside that — push it much past one and the
/// prevailing direction disappears into swirl, which reads as turbulence rather
/// than as a meadow.
const CURL_STRENGTH: f32 = 0.85;

/// How wide a lattice cell is, in the page's own pixels.
const LATTICE_PIXELS: f32 = 2.0;

/// The page scale at which reading the field on a lattice starts paying.
///
/// A lattice is worth having exactly when one of its cells is coarser than the
/// marks reading it — otherwise every read is a miss, and the cache is a hash
/// and a probe added to a call that was going to happen anyway. The mat is the
/// densest pass at three hundred and ninety-five marks to the square metre,
/// which puts its marks about a twentieth of a metre apart, and
/// [`LATTICE_PIXELS`] of a page at this scale is exactly that.
///
/// So above it there is no lattice at all: a page baked near the authoring scale
/// reads the field per mark, exactly as it always did, and its pixels are
/// unchanged. Below it a dozen marks share a read. Measured, a one-pixel lattice
/// applied at every scale made a full-detail page *slower* — all probe, no hit —
/// which is what this threshold exists to avoid.
const LATTICE_ABOVE_PX_PER_METRE: f32 = 48.0;

impl<'a> GroundCache<'a> {
    /// A cache over `field`, on a lattice sized to this page's pixels.
    ///
    /// Above [`LATTICE_ABOVE_PX_PER_METRE`] there is no lattice and this is a
    /// pass-through to [`WorldField::sample`].
    pub fn new(field: &'a WorldField, px_per_metre: f32) -> Self {
        let step = if px_per_metre < LATTICE_ABOVE_PX_PER_METRE {
            LATTICE_PIXELS / px_per_metre.max(1.0e-3)
        } else {
            0.0
        };
        let slots = if step > 0.0 { GROUND_SLOTS } else { 0 };
        Self {
            field,
            step,
            inverse_step: if step > 0.0 { 1.0 / step } else { 0.0 },
            keys: vec![i64::MIN; slots],
            values: vec![Ground::EMPTY; slots],
        }
    }

    /// The ground at `world`, to the nearest lattice cell.
    pub fn sample(&mut self, world: Vec2) -> Ground {
        if self.step <= 0.0 {
            return self.field.sample(world);
        }
        let cx = (world.x * self.inverse_step).floor() as i32;
        let cy = (world.y * self.inverse_step).floor() as i32;
        let packed = ((cx as i64) << 32) | (cy as i64 as u32 as i64);
        // One packed coordinate collides with the empty sentinel, and a slot that
        // reads as empty when it holds data would hand back a blank `Ground`.
        // Fold that cell onto its neighbour rather than widening the key: it is
        // the cell two billion lattice steps out on both axes, further from the
        // origin than any world this game will hold, and folding it costs that
        // cell a shared sample where the alternative costs every lookup a branch
        // or a second array.
        let key = if packed == i64::MIN {
            i64::MIN + 1
        } else {
            packed
        };
        let slot = (crate::rng::scramble(key as u64) as usize) & (GROUND_SLOTS - 1);
        if self.keys[slot] == key {
            return self.values[slot];
        }
        // The cell's centre, not the point asked about: the value has to depend
        // on the cell alone or the memo would be returning one point's answer
        // for another's.
        let centre = Vec2::new(cx as f32 + 0.5, cy as f32 + 0.5) * self.step;
        let ground = self.field.sample(centre);
        self.keys[slot] = key;
        self.values[slot] = ground;
        ground
    }
}

/// Everything the composition layer knows about one point of ground.
#[derive(Clone, Copy, Debug)]
pub struct Ground {
    /// Mound relief above the local ground, metres.
    ///
    /// Ground, not canopy. Nothing adds this to a blade's own height; it decides
    /// which way the ground faces, how vigorous the grass on it is, and where
    /// water and bare earth collect. So it is free to exceed the length of the
    /// grass standing on it, and it does: the mean is around 0.06 and the top
    /// percentile reaches 0.25, which over a mound a metre across is a slope of
    /// about twelve degrees. A meadow's worth of swell.
    pub height: f32,
    /// How close this point is to the top of its mound, `0..1`.
    ///
    /// Relative, not absolute: a mound in a low corner of the world still gets a
    /// crown. Absolute height would leave whole regions with no bright tips at
    /// all, which reads as a lighting bug rather than as terrain.
    pub crown: f32,
    /// How much this point faces the key light, about `-1..1`.
    ///
    /// Analytic, from the geometry of the domes themselves rather than from a
    /// derivative of their sum — see [`WorldField::mounds`]. Zero on flat
    /// ground, positive on a slope turned toward the light, negative on one
    /// turned away, and smooth everywhere including where two mounds meet.
    pub lit: f32,
    /// Which way the field runs here: a world-space azimuth, radians.
    ///
    /// The same field that orients the ridges orients the grass growing on
    /// them, and that agreement is most of what separates a meadow from a
    /// carpet. Grass drawn with a uniformly random heading is isotropic — it
    /// looks the same in every direction, which nothing wind has ever touched
    /// does — and isotropy at the blade scale is what leaves the middle scale
    /// with nothing but round clumps to say.
    ///
    /// A bias, never a rule: everything that reads this scatters widely around
    /// it, and a minority ignores it entirely. Combed grass is a worse failure
    /// than isotropic grass.
    pub flow: f32,
    /// Regional hue drift, `-1..1`: cool blue-green below, warm olive above.
    ///
    /// Deliberately its own field rather than a second reading of [`tint`].
    /// When a generator varies only in value, every region of the field is the
    /// same green under a brighter or dimmer lamp — which is exactly how a
    /// single sampled palette gives itself away. Real ground varies in *what
    /// green it is*: older and drier one place, shaded and damp another.
    ///
    /// [`tint`]: Ground::tint
    pub hue: f32,
    /// Blade-count multiplier, `0..1.3`.
    pub density: f32,
    /// Broad colour drift, `-1..1`. Independent of everything else here.
    pub tint: f32,
    /// How bare this point is: 0 fully grown, 1 exposed soil.
    pub bare: f32,
    /// How much this ground favours the leafier vocabulary, `0..1.6`.
    ///
    /// Species do not scatter evenly. In the reference the broadleaf forms come
    /// in local colonies and then stop, and distributing them uniformly is one
    /// of the quieter ways a generated field announces itself — every square
    /// metre ends up with its fair share of everything.
    pub colony: f32,
    /// How strongly this area states the mound it sits on, `0..1`.
    ///
    /// Independent of the mound field itself, and that is the point. When macro
    /// lighting describes every mound equally, the lighting stops being light
    /// and becomes a diagram of the height field — every form explained, none
    /// merely suggested. In the reference, illumination sometimes crosses a
    /// vegetation mass and sometimes dies before reaching its edge.
    pub statement: f32,
    /// How loudly this patch of ground speaks, `0..1`.
    ///
    /// The most painterly field here, and the one with no physical meaning at
    /// all. A painter does not describe every square inch of a meadow to the
    /// same degree: some passages are individual blades, some are leafy flecks,
    /// and some are barely more than a shaped mass of colour. A generator that
    /// resolves everything equally produces a uniform bristle carpet — which is
    /// legible as grass and unmistakable as machinery. Low values here mean
    /// "let this area collapse into paint".
    ///
    /// ## It carries amplitude as well as description
    ///
    /// It began as a purely descriptive axis — which *families* of mark grow
    /// here, how hard the glaze pulls them back together — and that turned out
    /// to be half a mechanism. A quiet passage drawn with soft marks at full
    /// length, full contrast and the ordinary rate of tip glints is not quiet.
    /// It has exactly as much light and dark in it as the passage beside it; it
    /// merely spends it on rounder shapes. The field then reads as uniformly
    /// busy however carefully the vocabulary is varied, because the eye measures
    /// activity as contrast per square inch and nothing about the contrast
    /// moved.
    ///
    /// So this now also shortens the grass, drains the under-strokes, narrows
    /// the blade-to-blade scatter and thins the glints. Low ground is shorter,
    /// smoother and flatter as well as looser — a softer mid-green canopy rather
    /// than bare or blurred — and high ground gets the longest blades, the
    /// deepest separations and nearly all of the highlights.
    ///
    /// Which makes it the one field in this module that a *distant* viewer can
    /// see. Everything else here organises the plate at a metre or less, and a
    /// metre is five screen pixels once the ground is displayed at the size the
    /// camera presents it. This runs from a metre and a half out to ten, so it
    /// is what supplies "broad regions of darker, lighter, denser and shorter
    /// grass" — the reading the plate measured furthest from the reference at.
    pub resolution: f32,
}

impl Ground {
    /// A blank one, for filling a cache that has not been read yet.
    ///
    /// Never returned: [`GroundCache`] only hands back a slot whose key matches,
    /// and no key matches until the slot has been written.
    const EMPTY: Self = Self {
        height: 0.0,
        crown: 0.0,
        lit: 0.0,
        flow: 0.0,
        hue: 0.0,
        density: 0.0,
        tint: 0.0,
        bare: 0.0,
        colony: 0.0,
        statement: 0.0,
        resolution: 0.0,
    };
}

/// Where the key light is, projected onto the screen plane and normalised.
///
/// Up and to the left, in image space where +Y is down. Kept here as well as in
/// [`crate::style::GrassParams`] because the mound field shades its own domes and
/// needs to know which way the sun is; the two are checked against each other by
/// a test rather than by hope.
pub const LIGHT_PLANE: Vec2 = Vec2::new(-0.724_1, -0.689_7);

/// What the authored document says about ground the procedural fields are
/// describing.
///
/// ## Why this is a modulation and not a replacement
///
/// The fields in [`Ground`] are two different kinds of thing wearing one type.
/// `flow`, `colony`, `statement`, `crown`, `hue` and `tint` are **style** —
/// how a meadow is painted, tuned against reference art, and no business of a
/// terrain document. `density` and `bare` are **terrain semantics** — how much
/// grows here and whether the ground is exposed — and those are exactly what an
/// author writing a path through a meadow is trying to control.
///
/// So the overlay touches those two and leaves the rest alone. That boundary is
/// what lets the document decide where the track runs while the grass beside it
/// keeps every bit of the tuning that makes it look like grass: the colonies
/// still comb together, the statement field still lets passages collapse into
/// paint, and the mounds still swell. Replacing the whole field with document
/// values would produce semantically correct ground that looks like a carpet.
///
/// ## The substrate is realised, not read
///
/// Bareness comes through the transition solver rather than from the substrate
/// planes directly, so the edge of the track is ragged at tuft scale and — more
/// importantly — is ragged in *exactly the same places* the compiler decided
/// ownership. One function, called by both.
pub struct SemanticOverlay {
    /// The one evaluator. See [`crate::ground`].
    ///
    /// Not a copy of its inputs. An overlay that realised the substrate itself
    /// would be the second caller doing so, and the whole reason the evaluator
    /// exists is that two callers realising the same ragged boundary separately
    /// is how a track's colour ends up a centimetre from where its grass
    /// thinned.
    pub ground: std::sync::Arc<crate::ground::GroundEvaluator>,
}

impl std::fmt::Debug for SemanticOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticOverlay")
            .field("ground", &self.ground)
            .finish()
    }
}

/// The composition fields for one world.
#[derive(Debug)]
pub struct WorldField {
    seed: u64,
    light: Vec2,
    mound_cache: MoundCache,
    blob_cache: Box<BlobCache>,
    /// What the document says, when there is one.
    ///
    /// `None` is the laboratory meadow the reference art was tuned against, and
    /// it stays the default so that every existing measurement keeps meaning
    /// what it meant.
    overlay: Option<std::sync::Arc<SemanticOverlay>>,
}

const MOUND_CACHE_SLOTS: usize = 256;

#[derive(Clone, Copy, Debug)]
struct MoundRecord {
    present: bool,
    centre: Vec2,
    minor: f32,
    major: f32,
    sin: f32,
    cos: f32,
    amplitude: f32,
    sharpness: f32,
}

impl MoundRecord {
    const EMPTY: Self = Self {
        present: false,
        centre: Vec2::ZERO,
        minor: 0.0,
        major: 0.0,
        sin: 0.0,
        cos: 1.0,
        amplitude: 0.0,
        sharpness: 1.0,
    };
}

#[derive(Debug)]
struct MoundCache {
    valid: [Cell<bool>; MOUND_CACHE_SLOTS],
    keys: [Cell<i64>; MOUND_CACHE_SLOTS],
    records: [Cell<MoundRecord>; MOUND_CACHE_SLOTS],
}

impl MoundCache {
    fn new() -> Self {
        Self {
            valid: std::array::from_fn(|_| Cell::new(false)),
            keys: std::array::from_fn(|_| Cell::new(0)),
            records: std::array::from_fn(|_| Cell::new(MoundRecord::EMPTY)),
        }
    }

    #[inline]
    fn get(&self, key: i64) -> Option<MoundRecord> {
        let slot = (crate::rng::scramble(key as u64) as usize) & (MOUND_CACHE_SLOTS - 1);
        (self.valid[slot].get() && self.keys[slot].get() == key).then_some(self.records[slot].get())
    }

    #[inline]
    fn insert(&self, key: i64, record: MoundRecord) {
        let slot = (crate::rng::scramble(key as u64) as usize) & (MOUND_CACHE_SLOTS - 1);
        self.keys[slot].set(key);
        self.records[slot].set(record);
        self.valid[slot].set(true);
    }
}

const BLOB_CACHE_SLOTS: usize = 256;

#[derive(Clone, Copy, Debug)]
struct BlobRecord {
    present: bool,
    centre: Vec2,
    base: f32,
    stretch: f32,
    sin: f32,
    cos: f32,
    phases: [f32; 5],
    weights: [f32; 5],
}

impl BlobRecord {
    const EMPTY: Self = Self {
        present: false,
        centre: Vec2::ZERO,
        base: 0.0,
        stretch: 1.0,
        sin: 0.0,
        cos: 1.0,
        phases: [0.0; 5],
        weights: [0.0; 5],
    };
}

#[derive(Debug)]
struct BlobRecordCache {
    valid: [Cell<bool>; BLOB_CACHE_SLOTS],
    keys: [Cell<i64>; BLOB_CACHE_SLOTS],
    records: [Cell<BlobRecord>; BLOB_CACHE_SLOTS],
}

impl BlobRecordCache {
    fn new() -> Self {
        Self {
            valid: std::array::from_fn(|_| Cell::new(false)),
            keys: std::array::from_fn(|_| Cell::new(0)),
            records: std::array::from_fn(|_| Cell::new(BlobRecord::EMPTY)),
        }
    }

    #[inline]
    fn slot(key: i64) -> usize {
        (crate::rng::scramble(key as u64) as usize) & (BLOB_CACHE_SLOTS - 1)
    }

    #[inline]
    fn get(&self, key: i64) -> Option<BlobRecord> {
        let slot = Self::slot(key);
        (self.valid[slot].get() && self.keys[slot].get() == key).then_some(self.records[slot].get())
    }

    #[inline]
    fn insert(&self, key: i64, record: BlobRecord) {
        let slot = Self::slot(key);
        self.keys[slot].set(key);
        self.records[slot].set(record);
        self.valid[slot].set(true);
    }
}

#[derive(Debug)]
struct BlobCache {
    tiers: [BlobRecordCache; 3],
}

impl BlobCache {
    fn new() -> Self {
        Self {
            tiers: std::array::from_fn(|_| BlobRecordCache::new()),
        }
    }
}

impl WorldField {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            light: LIGHT_PLANE,
            mound_cache: MoundCache::new(),
            blob_cache: Box::new(BlobCache::new()),
            overlay: None,
        }
    }

    /// The same world, told what the document says about it.
    pub fn with_overlay(mut self, overlay: std::sync::Arc<SemanticOverlay>) -> Self {
        self.overlay = Some(overlay);
        self
    }

    /// The same world under a different key light.
    pub fn lit_by(seed: u64, light: Vec3) -> Self {
        Self {
            seed,
            light: Vec2::new(light.x, light.y).normalize_or(LIGHT_PLANE),
            mound_cache: MoundCache::new(),
            blob_cache: Box::new(BlobCache::new()),
            overlay: None,
        }
    }

    fn mound_record(&self, cell_x: i32, cell_y: i32) -> MoundRecord {
        let key = ((cell_x as i64) << 32) | (cell_y as u32 as i64);
        if let Some(record) = self.mound_cache.get(key) {
            return record;
        }
        let mut draw = Draw::at(self.seed, Stream::Mound, cell_x, cell_y);
        let record = if !draw.chance(0.74) {
            MoundRecord::EMPTY
        } else {
            let centre = Vec2::new(cell_x as f32, cell_y as f32) * MOUND_SPACING
                + Vec2::new(draw.unit(), draw.unit()) * MOUND_SPACING;
            let extent = draw.range(MOUND_EXTENT.0, MOUND_EXTENT.1);
            let aspect = if draw.chance(0.26) {
                draw.range(0.85, 1.25)
            } else {
                draw.range(MOUND_ASPECT.0, MOUND_ASPECT.1)
            };
            let wander = draw.signed();
            let adrift = draw.chance(0.20);
            let spin = draw.range(0.0, std::f32::consts::TAU);
            let strength = draw.unit();
            let amplitude = 0.035 + strength * strength * 0.30;
            let sharpness = draw.range(1.1, 2.4);
            let root = aspect.sqrt();
            let angle = if adrift {
                spin
            } else {
                self.flow_at(centre) + wander * 0.5
            };
            let (sin, cos) = crate::fastmath::sin_cos(angle);
            MoundRecord {
                present: true,
                centre,
                minor: extent / root,
                major: extent * root,
                sin,
                cos,
                amplitude,
                sharpness,
            }
        };
        self.mound_cache.insert(key, record);
        record
    }

    fn blob_record(
        &self,
        draw_x: i32,
        draw_y: i32,
        spacing: f32,
        chance: f32,
        size: (f32, f32),
        salt: i32,
    ) -> BlobRecord {
        let tier = match salt {
            0x00 => 0,
            0x40 => 1,
            _ => 2,
        };
        let key = ((draw_x as i64) << 32) | (draw_y as u32 as i64);
        if let Some(record) = self.blob_cache.tiers[tier].get(key) {
            return record;
        }
        let mut draw = Draw::at(self.seed, Stream::Dirt, draw_x, draw_y);
        let record = if !draw.chance(chance) {
            BlobRecord::EMPTY
        } else {
            let base_cell = Vec2::new((draw_x - salt) as f32, (draw_y + salt) as f32);
            let centre = (base_cell + Vec2::new(draw.unit(), draw.unit())) * spacing;
            let base = draw.range(size.0, size.1);
            let stretch = draw.range(MIN_STRETCH * MIN_STRETCH, 1.0).sqrt();
            let (sin, cos) = (self.flow_at(centre) + draw.signed() * 0.8).sin_cos();
            let mut phases = [0.0; 5];
            let mut weights = [0.0; 5];
            for (index, harmonic) in (2..=6).enumerate() {
                phases[index] = draw.range(0.0, std::f32::consts::TAU);
                weights[index] = draw.range(0.08, 0.44) / harmonic as f32;
            }
            BlobRecord {
                present: true,
                centre,
                base,
                stretch,
                sin,
                cos,
                phases,
                weights,
            }
        };
        self.blob_cache.tiers[tier].insert(key, record);
        record
    }

    /// Height and shading together, because the second is analytic in the first.
    ///
    /// Reads the twenty-five mound cells around `p`, and the count is not
    /// arbitrary. A mound is displaced up to a full cell from its own corner and
    /// its ellipse reaches up to `extent * √aspect` — 2.9 metres against a
    /// 1.6-metre grid — so a mound two cells away can still cover this point. A
    /// three-by-three window silently clips those, and a clipped kernel is a
    /// step discontinuity in the height field: an invisible straight line where
    /// the lighting changes, which is exactly the chunk-seam artefact this whole
    /// design exists to avoid.
    ///
    /// **This is the most expensive thing in the crate that is not a pixel**, and
    /// [`WorldField::sample`] used to call it five times. Four of those were
    /// central differences taken to fill a `slope` field that nothing in the
    /// workspace ever read — a gradient computed, stored and discarded at every
    /// point the baker considered planting something, which at the mat's density
    /// is some twelve thousand times a page. If a gradient is wanted, take it
    /// here by finite-differencing this function, and read the section below
    /// first: the reason `lit` is analytic rather than differenced is that a
    /// differenced composite creases, and a gradient taken the same way would
    /// crease in the same places.
    ///
    /// ## Shading a dome instead of differencing a field
    ///
    /// The obvious way to light this is to finite-difference the composited
    /// height field and treat the gradient as a surface normal. It works, and it
    /// is wrong in a specific way: the composite is sampled on a lattice and read
    /// back bilinearly, so its *slope* is piecewise constant and jumps at every
    /// lattice line. Those jumps are faint creases in the finished plate — hard
    /// transitions in the one thing that must not have any.
    ///
    /// But the shapes are known. Each of these is a dome, and a dome's normal at
    /// normalised radius `u` from its centre leans outward by `u` and upward by
    /// `sqrt(1 - u²)`. So the directional term is simply how far out this point
    /// sits, times how much its outward direction agrees with the light — an
    /// estimate, evaluated per dome, with no derivatives anywhere and nothing to
    /// crease. Where domes overlap they are averaged by the same weights the
    /// height uses, so the shading follows the shape that is actually winning.
    ///
    /// ## Ridges, not cushions
    ///
    /// Most of these are drawn strongly elongated and oriented along
    /// [`WorldField::flow_at`], because a field of *round* masses is the single
    /// loudest thing a placed point process can say. Every mound then has a
    /// bright crown, a dark surround and roughly the silhouette of its
    /// neighbours, and the surface reads as hundreds of small cushions however
    /// carefully the sizes are varied. Stretched along a shared local direction
    /// they read instead as the soft bands and shallow dips ground actually has,
    /// and — because neighbouring ridges share an orientation — they run into
    /// one another rather than each closing its own outline.
    ///
    /// The stretch is area-preserving. Elongating without it would quietly make
    /// every ridge a *larger* mound as well as a longer one, and the mound
    /// spacing would stop meaning anything.
    fn mounds(&self, world: Vec2) -> (f32, f32) {
        let p = skewed(world);
        let cell = (p / MOUND_SPACING).floor();
        let (cx, cy) = (cell.x as i32, cell.y as i32);

        // Smooth maximum as a p-norm, and the choice is load-bearing.
        //
        // The obvious form — `ln(sum of exp(k*h)) / k` — has a baseline: a cell
        // contributing nothing still adds `exp(0) = 1` to the sum. Skipping
        // those cells to avoid the offset is what the first version of this did,
        // and it puts a step discontinuity in the field exactly where a kernel
        // switches on, because the sum jumps by a whole unit for a mound of zero
        // height. That reads on screen as a straight line where the lighting
        // changes, and it is invisible in every still until you happen to look
        // at the right one.
        //
        // A p-norm has no baseline. Every cell contributes `h^4`, a cell with no
        // mound contributes nothing at all, one mound alone returns its own
        // height exactly, and two overlapping ones blend by about a fifth.
        let mut accumulated = 0.0f32;
        let mut shading = 0.0f32;

        for dy in -2..=2 {
            for dx in -2..=2 {
                let mound = self.mound_record(cx + dx, cy + dy);
                // A quarter of the cells grow nothing. This is the single most
                // effective thing in the function: one mound per cell, however
                // hard it is jittered, tessellates — the field becomes bright
                // islands separated by a connected network of dark troughs that
                // traces the grid, and once seen it cannot be unseen. Leaving
                // gaps breaks the network, and the gaps are also where the broad
                // calm ground comes from.
                if !mound.present {
                    continue;
                }
                // Every draw for this cell happens here, before anything that
                // depends on where we are standing. A cell's mound has to be the
                // same mound from every point that can see it, so no early-out
                // may sit in the middle of the sequence.
                // Reject on the widest mound the vocabulary can grow, before
                // drawing the eight values that say how wide *this* one is.
                //
                // The rejection further down is the exact one and it stays; this
                // is the cheap conservative one, and the order is the whole
                // point. Twenty-five cells are read for every sample of this
                // field and about seven of them can reach the point being
                // sampled, so putting a bound here means eighteen cells cost
                // three draws instead of eleven. `field.sample` is read once per
                // blade of grass, so that is a large fraction of a page.
                //
                // Sound because [`MOUND_REACH`] is an upper bound on `major`
                // below, which `mound_reach_bounds_every_ellipse` checks against
                // the draws themselves rather than against this comment.
                if (p - mound.centre).length_squared() > MOUND_REACH * MOUND_REACH {
                    continue;
                }
                // Smaller than the grid they sit on, so neighbours touch rather
                // than merge, and *widely* varied in every dimension. Mounds of
                // similar size and strength read as a pattern even when their
                // placement is random, because the eye finds the repeated unit
                // rather than the arrangement.
                // Three ridges to every cushion. See the note above the function.
                //
                // The long axis is capped where it is because the window above is
                // five cells wide: a mound reaching more than two cell widths —
                // 3.2 metres — could be missed by a sample that should have seen
                // it, and a *missed* mound is a step in the field, which is the
                // one artefact this whole design is built to avoid.
                // Squared, so most mounds are faint and a few are pronounced.
                // A uniform draw makes every mound roughly as assertive as its
                // neighbours, which is most of what "bubble terrain" is.
                // Unchanged even though the *lighting* on these was halved, and
                // the split is the point. Amplitude does not decide how mounded
                // the plate looks — `lit` is normalised, so it is scale-free —
                // it decides how much thicker and longer the grass on a swell
                // grows than the grass beside it. That thickness is where the
                // structure at a fifth of a metre and up comes from, and taking
                // it out along with the directional shading flattens the plate
                // at every radius rather than only at the one that was shouting.
                // Falloff exponent: low is a dome, high is a plateau with a
                // sharp shoulder. Mixing both is what stops every mound reading
                // as the same shape at a glance.
                let offset = p - mound.centre;
                // Reject on the semi-major axis before orienting anything. This
                // is what keeps a per-mound flow lookup affordable: nothing
                // outside the bounding circle can be inside the ellipse, and
                // twenty of the twenty-five cells leave here.
                if offset.length_squared() > mound.major * mound.major {
                    continue;
                }
                // Ridges run along the local flow; a fifth strike out on their
                // own. Without that minority the field acquires a *grain*, and a
                // grain is only a subtler kind of pattern.
                let along = Vec2::new(
                    offset.x * mound.cos + offset.y * mound.sin,
                    offset.y * mound.cos - offset.x * mound.sin,
                );
                let local = Vec2::new(along.x / mound.minor, along.y / mound.major);
                let falloff = 1.0 - local.length_squared();
                if falloff <= 0.0 {
                    continue;
                }
                let height = mound.amplitude * crate::fastmath::pow(falloff, mound.sharpness);
                let squared = height * height;
                let weight = squared * squared;
                accumulated += weight;

                // Which way this point on the ridge faces, projected.
                //
                // Not the radial direction. On a circle the two agree, and on a
                // ridge they do not agree at all: the radial direction runs off
                // the *end* of a long mound, so a ridge shaded radially would be
                // bright at one tip and dark at the other rather than along one
                // flank. The outward direction is the gradient of the ellipse,
                // which in the ridge's own frame is the local offset divided by
                // the square of each semi-axis — then rotated back into the
                // world, and projected the way everything else is: a world step
                // of `(dx, dy)` moves `(dx - dy)` across the screen and
                // `(dx + dy)` halved down it.
                let gradient = Vec2::new(local.x / mound.minor, local.y / mound.major);
                let outward = Vec2::new(
                    gradient.x * mound.cos - gradient.y * mound.sin,
                    gradient.x * mound.sin + gradient.y * mound.cos,
                )
                .normalize_or_zero();
                let screen = Vec2::new(outward.x - outward.y, (outward.x + outward.y) * 0.5)
                    .normalize_or_zero();
                // `u` is how far out we are; a dome leans hardest at its rim and
                // not at all at its top, which is what spreads the light across
                // the whole shape instead of putting a terminator on it.
                let u = local.length().min(1.0);
                shading += weight * u * screen.dot(self.light);
            }
        }

        // Fourth power rather than sixth. The exponent is how hard the maximum
        // is: low melts overlapping mounds into one mass, high keeps each one
        // its own shape and lets the join between two of them stay a join. Softer
        // than it was, so a ridge runs into its neighbour and the pair reads as
        // one long swell instead of two forms with a seam.
        // A fourth root is two square roots, and a square root is a hardware
        // instruction where `powf` is a call with a logarithm inside it. Same
        // number, a great deal less of it.
        let height = accumulated.sqrt().sqrt();
        let lit = if accumulated > 1.0e-12 {
            shading / accumulated
        } else {
            0.0
        };
        (height, lit)
    }

    /// Which way the ground runs at a point: a world azimuth, radians.
    ///
    /// Two terms, and they do different jobs.
    ///
    /// The first is one octave of value noise at about five and a half metres
    /// per cycle — roughly a third of the width of a 1080p view. That scale is
    /// chosen against the eye rather than against anything physical: a flow that
    /// turns faster than the eye can follow is just another kind of noise, and
    /// one that turns slower is a comb.
    ///
    /// The second is the **curl of a scalar potential**, and it is what makes
    /// the flow read as growth rather than as drift. A direction field sampled
    /// straight from noise wanders; it has no arcs, no eddies and no centres,
    /// because nothing constrains it to conserve anything. The curl of a
    /// potential is divergence-free by construction, so it *must* circulate —
    /// which is where fans, curls and the occasional vortex come from, and those
    /// are precisely the shapes the reference art organises its colonies around.
    ///
    /// ```text
    ///   F = ( ∂ψ/∂y , −∂ψ/∂x )
    /// ```
    ///
    /// Cheap on purpose. It is read once per contributing mound rather than once
    /// per sample, so it sits inside the hottest loop in the crate.
    #[inline]
    fn flow_at(&self, p: Vec2) -> f32 {
        let drift = value_noise(self.seed, Stream::Flow, p.x * 0.18, p.y * 0.18);
        // A finite difference wide enough to stay smooth and narrow enough that
        // the eddies are colony-sized rather than regional.
        const STEP: f32 = 0.35;
        const FREQUENCY: f32 = 0.11;
        let potential = |q: Vec2| fbm(self.seed, Stream::Curl, q.x * FREQUENCY, q.y * FREQUENCY, 3);
        let dy = potential(p + Vec2::Y * STEP) - potential(p - Vec2::Y * STEP);
        let dx = potential(p + Vec2::X * STEP) - potential(p - Vec2::X * STEP);
        let curl = Vec2::new(dy, -dx);
        // The drift sets the prevailing direction and the curl bends it. Adding
        // the curl as a *vector* to the drift's own unit vector rather than as
        // an angle keeps the turn rate bounded — an angular sum can spin a full
        // turn inside a metre and read as a defect.
        let base = Vec2::from_angle(drift * std::f32::consts::TAU);
        (base + curl * CURL_STRENGTH).normalize_or(base).to_angle()
    }

    /// Everything at once, which is how the baker wants it.
    /// The ground here: the tuned style fields, then what the document says.
    ///
    /// Two steps kept apart on purpose. See [`SemanticOverlay`]: the procedural
    /// half is the meadow the reference art was tuned against, and the document
    /// half only ever decides *how much* grows and *whether the earth shows*.
    pub fn sample(&self, world: Vec2) -> Ground {
        let mut ground = self.sample_style(world);
        if let Some(overlay) = &self.overlay {
            let vegetated = overlay.ground.vegetation_support(world);
            let abundance = overlay.ground.abundance(world);
            // Multiplied into the tuned density rather than replacing it, so a
            // meadow the document leaves alone keeps every clump and channel the
            // procedural field gave it.
            ground.density *= vegetated * abundance.max(0.0);
            // Bareness takes whichever is barer. A document that says "this is a
            // track" must be able to expose ground the mound field thought was
            // lush, and ground the procedural field already thinned must not be
            // covered up by a document that said nothing about it.
            //
            // ## Abundance belongs in here, and used not to be
            //
            // Bareness came from vegetation *support* alone — from what the
            // ground allows — so lowering an authored abundance grew a tenth as
            // much grass and reported no bare earth at all. What came out was
            // holes punched in an otherwise closed canopy: the tuft pass thinned
            // and every bare-ground response stayed switched off, so the soil
            // between the clumps went on being shaded as though a canopy were
            // still over it.
            //
            // A genuinely sparse stand is not a lush one with pieces missing.
            // It is short, dry, pale at the root, with the earth reading through
            // — and all of that is what `bare` turns on.
            ground.bare = ground.bare.max(Self::semantic_bare(vegetated, abundance));
        }
        ground
    }

    /// How sharply bareness follows a falling cover.
    ///
    /// One: linear, and pinned in a constant so that changing it is a
    /// deliberate look change with a version behind it rather than a number
    /// somebody nudged. A measured response curve may replace it; it must not
    /// arrive by accident.
    pub const BARE_RESPONSE_GAMMA: f32 = 1.0;

    /// How much earth an authored cover ought to expose, `0..1`.
    ///
    /// One function, called by both [`WorldField::sample`] and
    /// [`WorldField::exposed_share`]. They used to compute it separately and
    /// disagreed: `sample` folded in abundance and `exposed_share` returned the
    /// complement of support, so the ground mesh and the grass on it had
    /// different ideas about where the earth showed.
    ///
    /// Linear in version one, and deliberately so. A physically exact uncovered
    /// fraction would need calibrated projected footprints and overlap
    /// statistics for every tuned pass; the tuned generator's `bare` is an
    /// artistic control rather than a measured Boolean union of blade shadows,
    /// and pretending otherwise would add false precision to a number that is
    /// already doing an honest job.
    pub fn semantic_bare(support: f32, abundance: f32) -> f32 {
        // Above one may thicken the canopy; it may not create negative earth.
        let cover = support * abundance.clamp(0.0, 1.0);
        (1.0 - cover.powf(Self::BARE_RESPONSE_GAMMA)).clamp(0.0, 1.0)
    }

    /// How much of this point is *not* ground that plants grow on, `0..1`.
    ///
    /// Zero without an overlay, so the laboratory meadow's bare patches keep the
    /// olive ramp they were tuned against and every existing baseline holds.
    ///
    /// Note what this is not: a material identity. It is the complement of the
    /// weighted vegetation affinity, which is the right question for *the grass*
    /// — how much of this point can a blade root in — and the wrong question for
    /// the ground itself. What the ground is made of is
    /// [`GroundEvaluator::substrates`](crate::ground::GroundEvaluator::substrates),
    /// which names each soil rather than lumping them into "not grass".
    pub fn exposed_share(&self, world: Vec2) -> f32 {
        match &self.overlay {
            None => 0.0,
            // The same function `sample` uses, not a second formula that agrees
            // with it today. They disagreed before this: `sample` folded in
            // abundance and this returned the complement of support, so the
            // ground mesh and the grass standing on it had different ideas
            // about where the earth showed.
            Some(overlay) => Self::semantic_bare(
                overlay.ground.vegetation_support(world),
                overlay.ground.abundance(world),
            ),
        }
    }

    /// The ground evaluator behind this field, if a document supplied one.
    pub fn ground(&self) -> Option<&std::sync::Arc<crate::ground::GroundEvaluator>> {
        self.overlay.as_ref().map(|overlay| &overlay.ground)
    }

    fn sample_style(&self, world: Vec2) -> Ground {
        let (height, lit) = self.mounds(world);
        // Every noise lookup below shares this frame; the mound grid rotates
        // itself internally so that finite differences still come back in world
        // space, where the lighting wants them.
        let p = skewed(world);

        // Crown is height against a broad local average rather than against
        // zero. `fbm` at a third of the mound frequency stands in for the
        // Gaussian blur a baked field would use — same job, no second pass.
        let broad = fbm(self.seed, Stream::Mound, p.x * 0.22, p.y * 0.22, 3);
        let crown = ((height - 0.026 - broad * 0.050) * 16.0).clamp(0.0, 1.0);

        // Three separate fields, deliberately. Density that followed the mound
        // field exactly would make every mound identically shaggy, and the eye
        // finds that rule almost immediately.
        //
        // The contrast curve is the load-bearing part. Raw noise gives a field
        // that is almost everywhere near its mean, so the grass ends up the same
        // thickness everywhere and reads as turf. The reference is built from
        // distinct bunches with thinner channels running between them, and a
        // smoothstep is what turns "slightly more grass here" into "a clump,
        // then a gap".
        // Three clump scales rather than one, and a gentler curve than the first
        // attempt used. A single scale with a hard curve carves connected dark
        // rivers between the clumps; the reference's thin ground is patchy
        // pockets, not channels.
        //
        // The broadest of the three is the field that answers "where are the
        // large calm regions". Density varying only at the clump scale gives a
        // plate that is uniformly busy once you step back from it — every square
        // foot has had its fair share of thick and thin — and the eye reads that
        // uniformity as machinery long before it can name what is wrong.
        //
        // Nearly eight metres per cycle, which is most of a 1080p view and about
        // twice what it used to be. The scale is set by what the field has to
        // read as at *gameplay* size rather than by what looks varied in a
        // plate: a region the size of a couple of unit footprints is a mottle
        // the player scrolls past, and one the size of half the screen is a part
        // of the map. Regions have to be big enough that the eye meets the broad
        // pattern first and the clumps second, which is the order this had
        // backwards.
        let sweep = smoothstep(
            0.32,
            0.76,
            fbm(self.seed, Stream::Family, p.x * 0.13 + 77.0, p.y * 0.13, 4),
        );
        // Moved down to fill the hole the broader regions opened, not left where
        // it was. The three scales have to *ladder*: pushing the regional field
        // out to eight metres while this one stayed at one and a half left
        // nothing describing the band between them, and the mounds alone are not
        // enough to hold it.
        //
        // Then moved *out* again by nearly half, to a metre and a quarter, and
        // this is the rung that decides how big a parent clump is. The number is
        // set by the camera rather than by the plate: the ground displays at
        // about a fifth of the size it is baked at, so a clump of this period is
        // twenty-five screen pixels and a clump of the previous one was
        // seventeen. Below roughly ten a shape stops being a shape and becomes
        // grain, and everything the eye is meant to group by has to clear that
        // with room to spare.
        let coarse = smoothstep(
            0.33,
            0.74,
            fbm(self.seed, Stream::Family, p.x * 0.80, p.y * 0.80, 4),
        );
        // The bunch scale — the individual clump inside a parent mass, and now
        // the *second* voice rather than the loudest. Its curve is the steepest
        // of the three on purpose: a gentle curve here gives ground that is
        // *slightly* thicker in some places, which reads as one continuous sward
        // with a mottle on it. A steep one gives a bunch, then a gap, then the
        // next bunch, which is the thing the reference has.
        //
        // Out by half along with the rung above it, and the pair has to move
        // together or the ladder loses a rung. Weight moved the other way at the
        // same time: the plate's measured shortfall is at large radius and grows
        // monotonically with it — thirty percent down at a third of a metre,
        // nearly forty at two thirds — which is not a flat field but a field
        // whose organisation is all at the wrong scale.
        let fine = smoothstep(
            0.36,
            0.70,
            fbm(self.seed, Stream::Family, p.x * 1.65 + 40.0, p.y * 1.65, 3),
        );
        let bunched = sweep * 0.26 + coarse * 0.38 + fine * 0.36;
        // Weighted toward the clump fields and away from the mounds. Density
        // that follows relief closely makes thickness a second statement of
        // height, and then every raised place is also a busy place and every
        // bright place is raised — three fields collapsed into one, which is
        // most of what makes a generated surface read as a diagram of its own
        // height map. The mound terms are left in because grass genuinely is
        // more vigorous on a swell; they are simply no longer the loudest voice.
        // Averaging three fields narrows the spread — variance adds in squares —
        // so the multiplier climbs with the count and the constant falls to keep
        // the mean where it was. Getting that wrong is easy and quiet: the field
        // simply stops having thin ground anywhere.
        let density = (bunched * 1.62 + crown * 0.22 + height * 1.1).clamp(0.05, 1.45);

        // Six metres per cycle, deliberately larger than a mound, and
        // deliberately a single scale.
        //
        // This is the only field with structure above the mound scale, and
        // without it the plate loses a third of its large-radius variance.
        // Splitting it across two scales was tried and is worse: averaging two
        // noises narrows the spread of both, so the broad end — the one nothing
        // else in the plate can supply — pays for a mid scale that the clump
        // fields already cover.
        //
        // Halved in frequency along with every other regional field here, and
        // the cost is known and accepted: a plate that spans fewer cycles of
        // this averages less of it away, so the ten worlds sit further apart in
        // mean luminance and `match.distance` pays for it. What is bought is the
        // thing the distance cannot see — that a region is large enough to be a
        // *place* rather than a mottle. See the note on the tension in
        // docs/BENCHMARKS.md.
        let tint = fbm(self.seed, Stream::Tint, p.x * 0.16, p.y * 0.16, 4) * 2.0 - 1.0;

        Ground {
            height,
            lit,
            crown,
            density,
            tint,
            // Turned back into a *world* azimuth, which is the frame everything
            // that reads it works in — a stroke's `azimuth` steps its position
            // through world x and y.
            //
            // Every field in this module is evaluated on `skewed` coordinates,
            // so an angle that comes out of one is an angle in skewed space, and
            // handing it straight to the baker rotates the grass away from the
            // ridges by the skew. The quarter turn on top is because `flow_at`
            // names the ridge's *short* axis: a ridge runs across the direction
            // its ellipse is narrowest in, and grass on a ridge runs along the
            // ridge.
            flow: self.flow_at(p) + std::f32::consts::FRAC_PI_2 - SKEW,
            // Its own stream and its own scale, and neither shared with `tint`.
            // Hue that tracked brightness would only be a longer way of saying
            // the same thing: the pale regions warm, the dim ones cool, and the
            // field still reads as one colour under a moving lamp.
            hue: fbm(self.seed, Stream::Hue, p.x * 0.085 + 29.0, p.y * 0.085, 3) * 2.0 - 1.0,
            bare: self.bare(world, height, density),
            colony: smoothstep(
                0.42,
                0.78,
                fbm(self.seed, Stream::Leaf, p.x * 0.85 + 13.0, p.y * 0.85, 3),
            ) * 1.6,
            // Broader than a mound and independent of it, so a strongly stated
            // form and a barely stated one can sit side by side.
            statement: (0.52
                + fbm(self.seed, Stream::Shade, p.x * 0.46, p.y * 0.46, 3) * 0.96)
                // Broken by a second, finer field. One smooth low-frequency
                // field scaling the macro light is visible *as a field* — broad
                // sweeps of pale grass that read as a mask laid over the ground
                // rather than as light falling on it. Multiplying by something
                // three times finer keeps the "some forms stated, some not"
                // behaviour and takes the sweep away.
                * (0.55 + fbm(self.seed, Stream::Shade, p.x * 1.35 + 60.0, p.y * 1.35, 3) * 0.9),
            // Deliberately independent of everything else. Tie descriptive
            // resolution to density or to the mounds and it stops being a
            // painter's choice and becomes another way of saying the same thing.
            //
            // Three scales, weighted toward the broad one, and the ladder is the
            // point. A single field at the mound frequency gives calm and busy
            // passages that are themselves mound-sized, so the variation lands
            // *inside* the texture instead of organising it, and the plate ends
            // up uniformly busy at every radius the eye checks. The broadest
            // term runs at about ten metres per cycle — most of a 1080p view —
            // and it is the one that produces the quiet ground a detailed
            // passage needs in order to read as detailed at all.
            //
            // A middle rung at three metres was added when this field gained
            // amplitude. Two scales an octave and a half apart leave a gap, and
            // the gap is exactly where a plate is measured: at ten metres a
            // single plate spans one cycle, so that rung moves the *whole
            // plate's* brightness and lands as world-to-world spread rather than
            // as structure within any one of them. At three metres a plate spans
            // five cycles, so the same amplitude buys large-radius organisation
            // and costs nothing in the comparison. Where a term sits relative to
            // the size of the thing being measured decides which of the two it
            // turns into, and it is easy to spend a lot of effort on the first
            // while asking for the second.
            //
            // The band is deliberately off centre. `fbm` returns something
            // roughly symmetric about a half, so a band centred there splits the
            // field evenly between described and loose; pushing it up spends
            // most of the ground on the quiet side and keeps the articulated
            // passages for the minority that earns them. Grass that rewards
            // close inspection everywhere rewards it nowhere, because there is
            // nothing left for it to be more detailed *than*.
            //
            // It also narrowed when the third octave arrived, and had to.
            // Independent noises sum in quadrature, so three of them spread less
            // than two however the weights are set; a band left at its old width
            // over a narrower distribution quietly collapses the three intensity
            // classes into one middling one. `grass_bake` prints the resulting
            // split — quiet, ordinary, hero — for exactly this reason.
            resolution: smoothstep(
                0.315,
                0.700,
                fbm(
                    self.seed,
                    Stream::Detail,
                    p.x * 0.095 + 71.0,
                    p.y * 0.095,
                    3,
                ) * 0.375
                    + fbm(self.seed, Stream::Detail, p.x * 0.30 + 13.0, p.y * 0.30, 3) * 0.345
                    + fbm(self.seed, Stream::Detail, p.x * 0.72, p.y * 0.72, 3) * 0.28,
            ),
        }
    }

    /// How much soil shows through at a point.
    ///
    /// Bare ground is placed in the valleys — low mound height, low density —
    /// because that is where it reads as a depression rather than as a hole
    /// punched in a green sheet. Placement is the larger half of the effect; the
    /// baker does the rest by darkening the roots that overhang the edge.
    fn bare(&self, world: Vec2, height: f32, density: f32) -> f32 {
        let p = skewed(world);
        // Warp first. An unwarped blob field gives lobed but recognisably
        // radial patches; warping the lookup is what makes their outlines read
        // as eroded.
        let warp = Vec2::new(
            fbm(self.seed, Stream::Dirt, p.x * 1.1 + 11.0, p.y * 1.1, 3) - 0.5,
            fbm(self.seed, Stream::Dirt, p.x * 1.1, p.y * 1.1 + 23.0, 3) - 0.5,
        ) * 0.55;
        let q = p + warp;

        // Two scales, because the reference has both: a handful of broad scuffs
        // most of a metre across, and many small nicks between clumps. One
        // spacing produces patches that are all the same size, and a field of
        // same-size patches reads as a pattern however irregular each one is.
        //
        // Fewer of both than there were, and half again as large — the same
        // quantity of earth arranged into a smaller number of bigger openings.
        // Patch *count* and patch *area* pull in opposite directions on the same
        // failure: many similar openings scattered evenly read as a texture
        // applied to the grass, because nothing about where any one of them
        // landed has a reason. A few large irregular ones read as places, and
        // the small nicks near them read as their edges rather than as more
        // texture. Total exposed earth is meant to come out slightly *up*, not
        // down; it is the arrangement that changed.
        // Fewer again — a third off every scale — and kept exactly as large.
        //
        // The count is the lever here and the gain is not, which is worth
        // knowing before reaching for the obvious knob. `best` falls linearly
        // from a patch's centre and the result is clamped, so most of a patch
        // sits at the top of the clamp; cutting the gain by a third only moves
        // the radius where the clamp starts, and the exposed *area* falls by
        // under a tenth. Measured: gain 13 to 8.8 took the rendered soil share
        // from 4.5 percent to 3.8. Halving the count halves the area.
        let broad = self.blobs(q, 4.1, 0.33, (0.46, 1.18), 0x00);
        let fine = self.blobs(q, 1.7, 0.30, (0.17, 0.46), 0x40);
        let flecks = self.blobs(q, 0.52, 0.13, (0.025, 0.075), 0x77);
        // Where the field is worn at all: a broad, soft region about a metre
        // across, inside which openings gather and outside which they mostly do
        // not. Everything below is multiplied by it.
        //
        // Without it the three scales scatter independently and the result is
        // isolated specks — a small round brown dot in the middle of thick grass
        // reads as a blemish on the texture, not as ground, because ground that
        // shows through has a reason and reasons are local. Gathering them into
        // worn zones is what turns the same quantity of soil into openings with
        // debris around their edges.
        let worn = smoothstep(
            0.38,
            0.70,
            fbm(self.seed, Stream::Dirt, p.x * 0.9 + 55.0, p.y * 0.9, 3),
        );
        // Grass bridges. A patch of earth with an unbroken rounded outline reads
        // as a bald hole however irregular that outline is; the reference's
        // openings are crossed by tongues of grass and shed detached flecks at
        // their edges, so they read as somewhere the field has worn thin.
        let bridges = smoothstep(
            0.26,
            0.70,
            fbm(
                self.seed,
                Stream::Dirt,
                p.x * 2.6 + 91.0,
                p.y * 2.6 - 17.0,
                3,
            ),
        );
        let best = broad.max(fine * 0.9) * (0.30 + 0.70 * bridges);
        // Flecks are debris around an opening, never openings of their own.
        let best = best.max(flecks * 0.85 * worn);
        let best = best * (0.42 + 0.58 * worn);

        // Valley bias, so a patch that strays onto a mound flank fades out
        // rather than clipping off. Gentle: bias it hard and the patches vanish
        // altogether, because almost all of this ground is mound flank.
        let valley = 1.0 - (height * 5.5).min(1.0);
        // Thin grass is where earth shows. Tied to density rather than placed
        // independently, so a patch never opens in the middle of a thick clump —
        // which is what makes one read as a hole rather than as ground.
        let sparse = (1.55 - density).clamp(0.12, 1.0);
        // Raised alongside the patch sizing, then cut by a third when the clump
        // fields grew, and the second move is the one worth explaining.
        //
        // Nothing here changed. What changed is `sparse`, which is a function of
        // density — and enlarging the clump ladder by half enlarged the *gaps*
        // between clumps by half along with it. An opening needs thin ground and
        // a blob in the same place; when the thin ground came in pockets a third
        // of a metre across, most blobs straddled a boundary and only their
        // cores reached full exposure. At two thirds of a metre a whole blob
        // fits inside one pocket and saturates, so the same placement field
        // yields nearly twice the earth. Measured, the rendered soil share went
        // from 2.4 percent to 4.5 against a reference that has 2.3.
        //
        // Worth writing down because the failure is entirely non-local: nothing
        // in this function moved, and the number this function controls doubled.
        // Any change to the density ladder has to be checked here.
        (best * 8.8 * valley * sparse).clamp(0.0, 1.0)
    }

    /// An irregular blob field: jittered centres, wobbly boundaries.
    ///
    /// `salt` separates one scale's cell hashes from another's, so the fine
    /// patches are not simply small copies of the broad ones sitting in the same
    /// places.
    ///
    /// ## The window is derived, not assumed
    ///
    /// A blob is jittered a full cell from its own corner and stretched up to
    /// `1/sqrt(0.42)` along one axis, so how many cells away one can still be
    /// felt is a function of `spacing` and `size` rather than a constant. Getting
    /// that wrong does not look like a clipped patch: a blob that a sample should
    /// have seen and did not is a *step* in the bare field, which prints as a
    /// straight line of dirt edge running across the world along a lattice
    /// direction — page-independent, so every page agrees on it, and therefore
    /// indistinguishable from something that was meant to be there.
    ///
    /// The three call sites are all sized so this comes out at one, and the
    /// arithmetic is here rather than in a comment beside them so that raising a
    /// patch size widens the window instead of silently truncating it.
    fn blobs(&self, q: Vec2, spacing: f32, chance: f32, size: (f32, f32), salt: i32) -> f32 {
        let cell = (q / spacing).floor();
        let (cx, cy) = (cell.x as i32 + salt, cell.y as i32 - salt);
        let mut best = 0.0f32;

        // `REJECT` below bounds the reach in the blob's own stretched frame;
        // dividing by the tightest stretch turns that into a world distance.
        let span = REJECT * size.1 / MIN_STRETCH;
        let reach = (span / spacing).ceil() as i32;
        // The eroded rim is a property of the point being sampled, not of the
        // blob contributing to it. Computing this inside the contributing-cell
        // loop repeated the same three-octave field several times.
        let bite = 0.72 + fbm(self.seed, Stream::Dirt, q.x * 9.0 + 5.0, q.y * 9.0, 3) * 0.56;

        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let blob = self.blob_record(cx + dx, cy + dy, spacing, chance, size, salt);
                // Most cells grow nothing. A patch per cell would read as a
                // polka dot however irregular its outline.
                if !blob.present {
                    continue;
                }
                // Elliptical, and freely oriented. Bare ground opens along the
                // channels between clumps rather than as a disc, and a field of
                // circles reads as damage rather than as ground however wobbly
                // each outline is.
                // Area-preserving: one axis stretches by as much as the other
                // shrinks. Dividing one axis alone would make every elongated
                // patch a smaller patch too, and the total bare ground would
                // quietly follow the aspect ratio around.
                // Lying along the same flow the ridges and the blades follow,
                // loosely. A worn opening runs *with* the ground rather than
                // across it, and openings that share a direction with the grass
                // around them read as places the field wore through; openings
                // scattered at every angle read as damage.
                let world_offset = q - blob.centre;
                let offset = Vec2::new(
                    (world_offset.x * blob.cos + world_offset.y * blob.sin) * blob.stretch,
                    (world_offset.y * blob.cos - world_offset.x * blob.sin) / blob.stretch,
                );
                let distance = offset.length();
                if distance > blob.base * REJECT {
                    continue;
                }
                // Four harmonics of boundary wobble. Two would read as an
                // ellipse and six as noise; four gives lobes and indentations.
                let angle = offset.y.atan2(offset.x);
                let mut radius = 1.0;
                for (index, harmonic) in (2..=6).enumerate() {
                    radius +=
                        blob.weights[index] * (harmonic as f32 * angle + blob.phases[index]).cos();
                }
                // Erode the rim at a much finer scale than the harmonics do.
                // Six harmonics give an organic *outline*; they do not give a
                // broken one, and an opening whose contour reads as a single
                // smooth curve is a soft mask over the grass rather than ground
                // the thatch has been worn off.
                let edge = blob.base * radius.max(0.25) * bite;
                best = best.max(1.0 - (distance / edge).clamp(0.0, 1.0));
            }
        }
        best
    }

    /// Fine mottling for the soil layer, `0..1`.
    pub fn soil_mottle(&self, world: Vec2) -> f32 {
        let p = skewed(world);
        fbm(self.seed, Stream::Soil, p.x * 7.0, p.y * 7.0, 3)
    }

    /// A cheap per-point wobble for anything that wants to avoid looking ruled.
    pub fn jitter(&self, stream: Stream, p: Vec2, frequency: f32) -> f32 {
        value_noise(self.seed, stream, p.x * frequency, p.y * frequency)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn lowering_abundance_exposes_earth_rather_than_punching_holes() {
        // The defect this fixed. Bareness came from vegetation *support* alone,
        // so a document that halved its abundance grew half as much grass and
        // reported no bare earth at all: what came out was holes in an
        // otherwise closed canopy, with every bare-ground response still
        // switched off and the soil between the clumps shaded as though a
        // canopy were over it.
        let full = WorldField::semantic_bare(1.0, 1.0);
        let half = WorldField::semantic_bare(1.0, 0.5);
        let none = WorldField::semantic_bare(1.0, 0.0);
        assert_eq!(full, 0.0, "a full stand exposes nothing");
        assert!(half > 0.0, "halving the abundance exposed no earth at all");
        assert_eq!(none, 1.0, "no vegetation at all is entirely bare");
        assert!(half < none);
    }

    #[test]
    fn ground_that_supports_nothing_is_bare_however_much_was_asked_for() {
        // A track is a track. An author raising the abundance over compacted
        // dirt is asking for grass the ground cannot grow, and the answer is
        // still bare earth.
        assert_eq!(WorldField::semantic_bare(0.0, 1.0), 1.0);
        assert_eq!(WorldField::semantic_bare(0.0, 4.0), 1.0);
    }

    #[test]
    fn abundance_above_one_cannot_make_negative_earth() {
        // A thicker canopy than the style's is a legitimate request; a surface
        // that is less than zero percent bare is not, and a negative `bare`
        // reaching the tuned generator drives its responses backwards.
        for abundance in [1.0f32, 1.5, 4.0, 1.0e6] {
            let bare = WorldField::semantic_bare(1.0, abundance);
            assert!((0.0..=1.0).contains(&bare), "{abundance} gave {bare}");
        }
    }

    #[test]
    fn semantic_bareness_is_monotone_in_abundance() {
        // Monotone, so an author turning a density down never gets *less*
        // visible earth. Not obvious once a gamma is in the expression, and the
        // thing that would break silently if one were introduced.
        let mut previous = 1.0;
        for step in 0..=20 {
            let bare = WorldField::semantic_bare(1.0, step as f32 / 20.0);
            assert!(bare <= previous + 1.0e-6, "bareness rose at {step}");
            previous = bare;
        }
    }
    use super::*;

    /// The early rejection in [`WorldField::mounds`] is only sound while
    /// [`MOUND_REACH`] really does bound every ellipse.
    ///
    /// It is a derived constant, and a derived constant is a claim about code
    /// somewhere else. If the draws it is derived from are ever widened, this
    /// fails; without it, the mounds simply get quietly clipped at the rejection
    /// radius, which is a step discontinuity in the height field — an invisible
    /// straight line where the lighting changes, and the exact artefact the
    /// whole placed-point design exists to avoid.
    ///
    /// So this replays the draw sequence rather than trusting the arithmetic in
    /// the comment: same streams, same order, across enough cells that every
    /// branch of the aspect draw is taken many times.
    #[test]
    fn mound_reach_bounds_every_ellipse() {
        let mut worst = 0.0f32;
        for x in -40..40 {
            for y in -40..40 {
                let mut draw = Draw::at(0x5eed_1234, Stream::Mound, x, y);
                if !draw.chance(0.74) {
                    continue;
                }
                let (_, _) = (draw.unit(), draw.unit());
                let extent = draw.range(MOUND_EXTENT.0, MOUND_EXTENT.1);
                let aspect = if draw.chance(0.26) {
                    draw.range(0.85, 1.25)
                } else {
                    draw.range(MOUND_ASPECT.0, MOUND_ASPECT.1)
                };
                worst = worst.max(extent * aspect.sqrt());
            }
        }
        // The sampled tail, and then the analytic endpoint. A sweep of cells
        // finds what the draws happen to produce; the endpoint is what they are
        // *allowed* to produce, and a bound that only clears the sample can be
        // beaten by a cell the sweep did not reach.
        let limit = MOUND_EXTENT.1 * MOUND_ASPECT.1.sqrt();
        assert!(
            MOUND_REACH >= limit,
            "a mound's semi-major axis may legally reach {limit} and the \
             rejection radius is {MOUND_REACH}, so mounds can be clipped"
        );
        assert!(
            MOUND_REACH >= worst,
            "a mound's semi-major axis reached {worst} and the rejection radius \
             is {MOUND_REACH}, so mounds are being clipped"
        );
        // And it must not be loose either: every extra metre admits cells that
        // then pay for eight draws and contribute nothing.
        assert!(
            MOUND_REACH < limit * 1.05,
            "the rejection radius is {MOUND_REACH} for a legal limit of {limit}"
        );
        // The window has to be wide enough for the bound as well as the bound
        // wide enough for the mounds. Two cells out is what `-2..=2` gives.
        // Written against the constants rather than as a literal comparison, so
        // that clippy sees two names rather than two numbers it can fold — the
        // point of the assertion is that it fails if either one moves.
        let window = MOUND_SPACING * 2.0;
        let reach = MOUND_REACH;
        assert!(
            window >= reach,
            "a mound can reach {MOUND_REACH} m and the window sees \
             {window} m, so one can be missed entirely"
        );
    }

    #[test]
    fn the_field_is_a_pure_function_of_position() {
        // The property that makes page-independent baking possible at all.
        let field = WorldField::new(0x5eed);
        let p = Vec2::new(3.7, -12.25);
        let a = field.sample(p);
        let b = field.sample(p);
        assert_eq!(a.height, b.height);
        assert_eq!(a.bare, b.bare);
        assert_eq!(a.density, b.density);
    }

    #[test]
    fn mounds_are_a_soft_rhythm_rather_than_terrain() {
        let field = WorldField::new(1);
        let mut heights = Vec::with_capacity(200 * 200);
        for i in 0..200 {
            for j in 0..200 {
                heights.push(
                    field
                        .sample(Vec2::new(i as f32 * 0.07, j as f32 * 0.07))
                        .height,
                );
            }
        }
        heights.sort_by(f32::total_cmp);
        let mean = heights.iter().sum::<f32>() / heights.len() as f32;
        let typical = heights[heights.len() * 99 / 100];
        let peak = *heights.last().unwrap();

        // The ninety-ninth percentile, not the maximum. Mound strength is drawn
        // squared on purpose — most mounds are faint and a few are pronounced,
        // which is what stops the field reading as a quilt of equally assertive
        // hummocks — so the single tallest mound in forty thousand samples says
        // nothing about whether the rhythm is soft. What matters is that the
        // ground the eye spends its time on is gently modulated.
        //
        // The band is drawn against ground relief rather than canopy height; see
        // [`Ground::height`]. A quarter of a metre over a mound a metre across is
        // a gentle swell, and the failure this guards against is the field
        // becoming hills — which the mean catches, because hills raise the whole
        // field rather than one percentile of it.
        assert!(typical < 0.30, "mounds became terrain: p99 {typical}");
        assert!(mean < 0.08, "the whole field rose: mean {mean}");
        assert!(mean > 0.01, "the mound field is flat: mean {mean}");
        // A loose absolute cap, so a runaway amplitude still fails loudly.
        assert!(peak < 0.45, "a single mound became a hill: peak {peak}");
    }

    #[test]
    fn the_field_is_continuous() {
        // A discontinuity here is a visible seam there, and it would look
        // exactly like a chunk boundary — the bug this design avoids by
        // construction, so it is worth a test rather than an assumption.
        let field = WorldField::new(7);
        for i in 0..400 {
            let p = Vec2::new(i as f32 * 0.031 - 6.0, i as f32 * 0.017 + 2.0);
            let a = field.sample(p).height;
            let b = field.sample(p + Vec2::splat(0.004)).height;
            assert!((a - b).abs() < 0.01, "jump of {} at {p:?}", (a - b).abs());
        }
    }

    #[test]
    fn bare_ground_is_rare_and_sits_in_the_valleys() {
        let field = WorldField::new(3);
        let (mut bare, mut count) = (0, 0);
        let (mut bare_height, mut grown_height) = (0.0f32, 0.0f32);
        for i in 0..300 {
            for j in 0..300 {
                let g = field.sample(Vec2::new(i as f32 * 0.05, j as f32 * 0.05));
                count += 1;
                if g.bare > 0.5 {
                    bare += 1;
                    bare_height += g.height;
                } else {
                    grown_height += g.height;
                }
            }
        }
        let fraction = bare as f32 / count as f32;
        // A twelfth, not a sixteenth, and the band moved for a reason worth
        // recording: this field stopped meaning "here is a hole in the grass"
        // and started meaning "here the grass is thinning".
        //
        // `bare` now drives three things in the baker that all fade in
        // gradually — how far the tuft coverage is thinned, how much the mat
        // below it is thinned, and how short and flat the marks that do grow
        // become — so a point at `bare = 0.6` still carries shoots, and the
        // openings read as worn ground rather than as bald spots. What actually
        // shows as earth in a finished plate is a good deal less than what this
        // counts — it measured 0.019 on the render while this field read eight
        // percent, back when the plate was still being scored against the
        // reference art.
        //
        // So this bound is a runaway guard, not the specification. It is also
        // the last thing left watching bare ground: the descriptor that used to
        // carry its own band went out with the aesthetic suite, because the look
        // is settled and `grass_snapshot` now answers the only remaining
        // question — whether a change moved the picture — far more strictly than
        // a share ever could.
        assert!(
            fraction < 0.12,
            "too much bare ground: {:.1}%",
            fraction * 100.0
        );
        assert!(
            fraction > 0.001,
            "no bare ground at all: {:.3}%",
            fraction * 100.0
        );
        let bare_mean = bare_height / bare.max(1) as f32;
        let grown_mean = grown_height / (count - bare).max(1) as f32;
        assert!(bare_mean < grown_mean, "bare ground climbed the mounds");
    }

    #[test]
    fn every_mound_gets_a_crown_somewhere() {
        // Crown measured against absolute height would leave whole regions with
        // no bright tips, which reads as a broken light rather than as terrain.
        let field = WorldField::new(11);
        let mut crowned = 0;
        for i in 0..160 {
            for j in 0..160 {
                if field
                    .sample(Vec2::new(i as f32 * 0.06, j as f32 * 0.06))
                    .crown
                    > 0.5
                {
                    crowned += 1;
                }
            }
        }
        let fraction = crowned as f32 / (160.0 * 160.0);
        assert!(
            (0.03..0.6).contains(&fraction),
            "crown coverage {fraction:.3}"
        );
    }
}

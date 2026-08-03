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

use bevy::prelude::*;

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
    /// Ground-plane gradient of [`Ground::height`], for anything that needs it.
    pub slope: Vec2,
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
    /// How finely this patch of ground is described, `0..1`.
    ///
    /// The most painterly field here, and the one with no physical meaning at
    /// all. A painter does not describe every square inch of a meadow to the
    /// same degree: some passages are individual blades, some are leafy flecks,
    /// and some are barely more than a shaped mass of colour. A generator that
    /// resolves everything equally produces a uniform bristle carpet — which is
    /// legible as grass and unmistakable as machinery. Low values here mean
    /// "let this area collapse into paint".
    pub resolution: f32,
}

/// Where the key light is, projected onto the screen plane and normalised.
///
/// Up and to the left, in image space where +Y is down. Kept here as well as in
/// [`crate::bake::BakeParams`] because the mound field shades its own domes and
/// needs to know which way the sun is; the two are checked against each other by
/// a test rather than by hope.
pub const LIGHT_PLANE: Vec2 = Vec2::new(-0.724_1, -0.689_7);

/// The composition fields for one world.
#[derive(Clone, Copy, Debug)]
pub struct WorldField {
    seed: u64,
    light: Vec2,
}

impl WorldField {
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            light: LIGHT_PLANE,
        }
    }

    /// The same world under a different key light.
    pub fn lit_by(seed: u64, light: Vec3) -> Self {
        Self {
            seed,
            light: Vec2::new(light.x, light.y).normalize_or(LIGHT_PLANE),
        }
    }

    /// Smooth-maximum mound height at a point, in metres.
    ///
    /// Reads the twenty-five mound cells around `p`, and the count is not
    /// arbitrary. A mound is displaced up to a full cell from its own corner and
    /// its ellipse reaches up to `radius * aspect` — 2.3 metres against a
    /// 1.6-metre grid — so a mound two cells away can still cover this point. A
    /// three-by-three window silently clips those, and a clipped kernel is a
    /// step discontinuity in the height field: an invisible straight line where
    /// the lighting changes, which is exactly the chunk-seam artefact this whole
    /// design exists to avoid.
    fn mound_height(&self, world: Vec2) -> f32 {
        self.mounds(world).0
    }

    /// Height and shading together, because the second is analytic in the first.
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
                let mut draw = Draw::at(self.seed, Stream::Mound, cx + dx, cy + dy);
                // A quarter of the cells grow nothing. This is the single most
                // effective thing in the function: one mound per cell, however
                // hard it is jittered, tessellates — the field becomes bright
                // islands separated by a connected network of dark troughs that
                // traces the grid, and once seen it cannot be unseen. Leaving
                // gaps breaks the network, and the gaps are also where the broad
                // calm ground comes from.
                if !draw.chance(0.74) {
                    continue;
                }
                // Every draw for this cell happens here, before anything that
                // depends on where we are standing. A cell's mound has to be the
                // same mound from every point that can see it, so no early-out
                // may sit in the middle of the sequence.
                let centre = Vec2::new(cx as f32 + dx as f32, cy as f32 + dy as f32)
                    * MOUND_SPACING
                    + Vec2::new(draw.unit(), draw.unit()) * MOUND_SPACING;
                // Smaller than the grid they sit on, so neighbours touch rather
                // than merge, and *widely* varied in every dimension. Mounds of
                // similar size and strength read as a pattern even when their
                // placement is random, because the eye finds the repeated unit
                // rather than the arrangement.
                let extent = draw.range(0.46, 1.55);
                // Three ridges to every cushion. See the note above the function.
                //
                // The long axis is capped where it is because the window above is
                // five cells wide: a mound reaching more than two cell widths —
                // 3.2 metres — could be missed by a sample that should have seen
                // it, and a *missed* mound is a step in the field, which is the
                // one artefact this whole design is built to avoid.
                let aspect = if draw.chance(0.26) {
                    draw.range(0.85, 1.25)
                } else {
                    draw.range(1.7, 3.5)
                };
                let wander = draw.signed();
                let adrift = draw.chance(0.20);
                let spin = draw.range(0.0, std::f32::consts::TAU);
                // Squared, so most mounds are faint and a few are pronounced.
                // A uniform draw makes every mound roughly as assertive as its
                // neighbours, which is most of what "bubble terrain" is.
                let strength = draw.unit();
                // Unchanged even though the *lighting* on these was halved, and
                // the split is the point. Amplitude does not decide how mounded
                // the plate looks — `lit` is normalised, so it is scale-free —
                // it decides how much thicker and longer the grass on a swell
                // grows than the grass beside it. That thickness is where the
                // structure at a fifth of a metre and up comes from, and taking
                // it out along with the directional shading flattens the plate
                // at every radius rather than only at the one that was shouting.
                let amplitude = 0.035 + strength * strength * 0.30;
                // Falloff exponent: low is a dome, high is a plateau with a
                // sharp shoulder. Mixing both is what stops every mound reading
                // as the same shape at a glance.
                let sharpness = draw.range(1.1, 2.4);

                // Area-preserving: the long axis grows by exactly as much as the
                // short one shrinks, so stretching a mound never also enlarges it.
                let root = aspect.sqrt();
                let (minor, major) = (extent / root, extent * root);
                let offset = p - centre;
                // Reject on the semi-major axis before orienting anything. This
                // is what keeps a per-mound flow lookup affordable: nothing
                // outside the bounding circle can be inside the ellipse, and
                // twenty of the twenty-five cells leave here.
                if offset.length_squared() > major * major {
                    continue;
                }
                // Ridges run along the local flow; a fifth strike out on their
                // own. Without that minority the field acquires a *grain*, and a
                // grain is only a subtler kind of pattern.
                let angle = if adrift {
                    spin
                } else {
                    self.flow_at(centre) + wander * 0.5
                };

                let (sin, cos) = angle.sin_cos();
                let along = Vec2::new(
                    offset.x * cos + offset.y * sin,
                    offset.y * cos - offset.x * sin,
                );
                let local = Vec2::new(along.x / minor, along.y / major);
                let falloff = 1.0 - local.length_squared();
                if falloff <= 0.0 {
                    continue;
                }
                let height = amplitude * falloff.powf(sharpness);
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
                let gradient = Vec2::new(local.x / minor, local.y / major);
                let outward = Vec2::new(
                    gradient.x * cos - gradient.y * sin,
                    gradient.x * sin + gradient.y * cos,
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
        let height = accumulated.powf(0.25);
        let lit = if accumulated > 1.0e-12 {
            shading / accumulated
        } else {
            0.0
        };
        (height, lit)
    }

    /// Which way the ground runs at a point: a world azimuth, radians.
    ///
    /// One octave, not four, and at a very low frequency: about five and a half
    /// metres per cycle, which is roughly a third of the width of a 1080p view.
    /// That scale is chosen against the eye rather than against anything
    /// physical — a flow that turns faster than the eye can follow is just
    /// another kind of noise, and one that turns slower is a comb.
    ///
    /// Cheap on purpose. It is read once per contributing mound rather than once
    /// per sample, so it sits inside the hottest loop in the crate.
    #[inline]
    fn flow_at(&self, p: Vec2) -> f32 {
        value_noise(self.seed, Stream::Flow, p.x * 0.18, p.y * 0.18) * std::f32::consts::TAU
    }

    /// Everything at once, which is how the baker wants it.
    pub fn sample(&self, world: Vec2) -> Ground {
        let (height, lit) = self.mounds(world);
        // Every noise lookup below shares this frame; the mound grid rotates
        // itself internally so that finite differences still come back in world
        // space, where the lighting wants them.
        let p = skewed(world);

        // Central differences at a tenth of a mound: fine enough to catch a
        // mound's flank, coarse enough not to pick up the kernel's own wobble.
        const STEP: f32 = 0.16;
        let dx =
            self.mound_height(world + Vec2::X * STEP) - self.mound_height(world - Vec2::X * STEP);
        let dy =
            self.mound_height(world + Vec2::Y * STEP) - self.mound_height(world - Vec2::Y * STEP);
        let slope = Vec2::new(dx, dy) / (2.0 * STEP);

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
        // uniformity as machinery long before it can name what is wrong. Four
        // metres per cycle is roughly a quarter of a 1080p view, which is the
        // scale a painter would change their mind at.
        let sweep = smoothstep(
            0.32,
            0.76,
            fbm(self.seed, Stream::Family, p.x * 0.24 + 77.0, p.y * 0.24, 4),
        );
        let coarse = smoothstep(
            0.30,
            0.80,
            fbm(self.seed, Stream::Family, p.x * 0.72, p.y * 0.72, 4),
        );
        let fine = smoothstep(
            0.34,
            0.74,
            fbm(self.seed, Stream::Family, p.x * 1.9 + 40.0, p.y * 1.9, 3),
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
        let density = (0.05 + bunched * 1.50 + crown * 0.22 + height * 1.1).clamp(0.05, 1.45);

        // Several metres per cycle, deliberately larger than a mound, and
        // deliberately a single scale.
        //
        // This is the only field with structure above the mound scale, and
        // without it the plate loses a third of its large-radius variance.
        // Splitting it across two scales was tried and is worse: averaging two
        // noises narrows the spread of both, so the broad end — the one nothing
        // else in the plate can supply — pays for a mid scale that the clump
        // fields already cover.
        let tint = fbm(self.seed, Stream::Tint, p.x * 0.30, p.y * 0.30, 4) * 2.0 - 1.0;

        Ground {
            height,
            lit,
            crown,
            slope,
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
            hue: fbm(self.seed, Stream::Hue, p.x * 0.21 + 29.0, p.y * 0.21, 3) * 2.0 - 1.0,
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
            // Two scales, weighted toward the broad one. A single field at the
            // mound frequency gives calm and busy passages that are themselves
            // mound-sized, so the variation lands *inside* the texture instead
            // of organising it, and the plate ends up uniformly busy at every
            // radius the eye checks. The broad term runs at about six metres per
            // cycle — a third of a 1080p view — and it is the one that produces
            // the quiet ground a detailed passage needs in order to read as
            // detailed at all.
            resolution: smoothstep(
                0.34,
                0.66,
                fbm(self.seed, Stream::Detail, p.x * 0.17 + 71.0, p.y * 0.17, 3) * 0.60
                    + fbm(self.seed, Stream::Detail, p.x * 0.62, p.y * 0.62, 3) * 0.40,
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
        let broad = self.blobs(q, 3.1, 0.72, (0.30, 0.82), 0x00);
        let fine = self.blobs(q, 1.15, 0.5, (0.12, 0.34), 0x40);
        let flecks = self.blobs(q, 0.52, 0.20, (0.025, 0.075), 0x77);
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
        let sparse = (1.45 - density).clamp(0.12, 1.0);
        (best * 6.6 * valley * sparse).clamp(0.0, 1.0)
    }

    /// An irregular blob field: jittered centres, wobbly boundaries.
    ///
    /// `salt` separates one scale's cell hashes from another's, so the fine
    /// patches are not simply small copies of the broad ones sitting in the same
    /// places.
    fn blobs(&self, q: Vec2, spacing: f32, chance: f32, size: (f32, f32), salt: i32) -> f32 {
        let cell = (q / spacing).floor();
        let (cx, cy) = (cell.x as i32 + salt, cell.y as i32 - salt);
        let mut best = 0.0f32;

        for dy in -1..=1 {
            for dx in -1..=1 {
                let mut draw = Draw::at(self.seed, Stream::Dirt, cx + dx, cy + dy);
                // Most cells grow nothing. A patch per cell would read as a
                // polka dot however irregular its outline.
                if !draw.chance(chance) {
                    continue;
                }
                let centre = (Vec2::new(cell.x + dx as f32, cell.y + dy as f32)
                    + Vec2::new(draw.unit(), draw.unit()))
                    * spacing;
                let base = draw.range(size.0, size.1);
                // Elliptical, and freely oriented. Bare ground opens along the
                // channels between clumps rather than as a disc, and a field of
                // circles reads as damage rather than as ground however wobbly
                // each outline is.
                // Area-preserving: one axis stretches by as much as the other
                // shrinks. Dividing one axis alone would make every elongated
                // patch a smaller patch too, and the total bare ground would
                // quietly follow the aspect ratio around.
                let stretch = draw.range(0.42, 1.0).sqrt();
                // Lying along the same flow the ridges and the blades follow,
                // loosely. A worn opening runs *with* the ground rather than
                // across it, and openings that share a direction with the grass
                // around them read as places the field wore through; openings
                // scattered at every angle read as damage.
                let (sin, cos) = (self.flow_at(centre) + draw.signed() * 0.8).sin_cos();
                let world_offset = q - centre;
                let offset = Vec2::new(
                    (world_offset.x * cos + world_offset.y * sin) * stretch,
                    (world_offset.y * cos - world_offset.x * sin) / stretch,
                );
                let distance = offset.length();
                if distance > base * 2.2 {
                    continue;
                }
                // Four harmonics of boundary wobble. Two would read as an
                // ellipse and six as noise; four gives lobes and indentations.
                let angle = offset.y.atan2(offset.x);
                let mut radius = 1.0;
                for harmonic in 2..=6 {
                    let phase = draw.range(0.0, std::f32::consts::TAU);
                    let weight = draw.range(0.08, 0.44) / harmonic as f32;
                    radius += weight * (harmonic as f32 * angle + phase).cos();
                }
                // Erode the rim at a much finer scale than the harmonics do.
                // Six harmonics give an organic *outline*; they do not give a
                // broken one, and an opening whose contour reads as a single
                // smooth curve is a soft mask over the grass rather than ground
                // the thatch has been worn off.
                let bite =
                    0.72 + fbm(self.seed, Stream::Dirt, q.x * 9.0 + 5.0, q.y * 9.0, 3) * 0.56;
                let edge = base * radius.max(0.25) * bite;
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
    use super::*;

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
        assert!(
            fraction < 0.06,
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

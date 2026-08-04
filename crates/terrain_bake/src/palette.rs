//! The colour language, as a set of one-dimensional ramps.
//!
//! Shading here is a *lookup*, never a multiply. Multiplying an albedo by a
//! lambert term is the single fastest way to get dead grey-green shadows, which
//! is exactly what the reference art does not have: its darkest pixels are still
//! saturated green, and its brightest are yellow-green paint rather than white
//! light. A ramp encodes that directly — where the hue goes as the value falls
//! is authored, not derived.
//!
//! ## The light index is a percentile
//!
//! [`GRASS`] is measured, not invented, and it is measured in a particular way
//! that changes what `q` *means*. The reference plate's pixels were sorted by
//! luminance and averaged into thirty-two equal-population buckets, so stop *i*
//! is "the colour of the reference at its `i/31` percentile". Feed this ramp a
//! light index uniform on `[0, 1]` and the histogram that comes back out is the
//! reference's histogram.
//!
//! That turns tone matching from guesswork into arithmetic. The baker's job is
//! no longer "make it look about that bright"; it is "make `q` uniform", and the
//! percentile rows of the comparison table say directly which way to push. A
//! candidate whose `p95` is low is not too dark overall — its light index does
//! not reach far enough at the top, which is a different repair.
//!
//! Two properties of the measured ramp are easy to destroy by hand:
//!
//! - **Blue stays almost absent.** 0.034 at the bottom, 0.13 at the top. Grass
//!   shadow in this art is not "green plus black"; it is a deeper, slightly
//!   cooler green whose blue channel never wakes up.
//! - **Almost nothing is dark.** The reference's first percentile is 0.215
//!   luminance: there are no shadows in this painting, only less light, which is
//!   why the ramp's bottom stop is a mid green. The "almost" is worth half a
//!   percent of the image, and [`THATCH`] reaches below the ramp to supply it —
//!   see the note there for why that half percent is not optional.

use glam::Vec3;

pub use terrain_generators::tone::Tone;

/// The ramp a tone shades through.
///
/// A free function rather than a method, and the change is not cosmetic: a tone
/// is an intrinsic property of a plant and lives with the generator, while the
/// measured colours it shades through are this module's. A method would have put
/// the paint table inside the vocabulary that names families of plant.
pub fn ramp(tone: Tone) -> &'static [[f32; 3]] {
    match tone {
        Tone::Soil => &SOIL,
        Tone::Thatch => &THATCH,
        Tone::Grass => &GRASS,
        Tone::Leaf => &LEAF,
        Tone::Dry => &DRY,
    }
}

/// Grass, at thirty-two equal-population stops of the reference plate plus one
/// glint above its top.
///
/// Two of the measured channel values land within a whisker of a mathematical
/// constant by pure coincidence, which the lint below would otherwise report as
/// a mistyped `PI/8`. They are colours, sampled from a painting.
///
/// Deliberately not analytic. An analytic ramp through the same endpoints misses
/// the slight desaturation through the mid-tones and the late acceleration at
/// the top, and the field it produces reads as plastic.
///
/// The final stop is the one invented entry: the reference's brightest tenth of
/// a percent, extrapolated, so that a genuine tip glint has somewhere to go.
#[allow(clippy::approx_constant)]
pub const GRASS: [[f32; 3]; 33] = [
    [0.1419, 0.2554, 0.0343],
    [0.1668, 0.2902, 0.0336],
    [0.1806, 0.3079, 0.0341],
    [0.1914, 0.3215, 0.0344],
    [0.2012, 0.3327, 0.0351],
    [0.2098, 0.3428, 0.0355],
    [0.2178, 0.3521, 0.0359],
    [0.2253, 0.3608, 0.0363],
    [0.2327, 0.3690, 0.0367],
    [0.2397, 0.3771, 0.0372],
    [0.2466, 0.3849, 0.0377],
    [0.2535, 0.3926, 0.0382],
    [0.2603, 0.4002, 0.0387],
    [0.2676, 0.4078, 0.0393],
    [0.2744, 0.4155, 0.0398],
    [0.2815, 0.4234, 0.0401],
    [0.2889, 0.4315, 0.0408],
    [0.2967, 0.4399, 0.0416],
    [0.3050, 0.4486, 0.0424],
    [0.3136, 0.4578, 0.0431],
    [0.3228, 0.4677, 0.0440],
    [0.3332, 0.4780, 0.0450],
    [0.3443, 0.4892, 0.0465],
    [0.3564, 0.5017, 0.0478],
    [0.3705, 0.5153, 0.0497],
    [0.3863, 0.5309, 0.0519],
    [0.4046, 0.5489, 0.0548],
    [0.4264, 0.5703, 0.0582],
    [0.4542, 0.5967, 0.0635],
    [0.4916, 0.6309, 0.0713],
    [0.5465, 0.6804, 0.0855],
    [0.6698, 0.7837, 0.1346],
    [0.7350, 0.8350, 0.1550], // tip glint
];

/// The mat below the canopy: the same family, cooled and compressed into the
/// bottom third of the grass ramp.
///
/// Not simply "grass, darker". Thatch that only differs in value disappears into
/// shadow; shifting it a few degrees toward blue-green is what keeps a cavity
/// reading as depth rather than as a smudge.
///
/// The bottom of this ramp is the only place in the palette that goes below the
/// reference's first percentile, and it has to. The reference *does* have darks
/// — half a percent of it sits under 0.20 luminance — and they are all the same
/// thing: the gap between one bunch of grass and the next, seen edge-on with
/// nothing lighting it. A floor that stops at the reference's p01 cannot produce
/// them at all, so the deepest gaps come out the same value as ordinary shaded
/// grass, the bunches lose their separation, and the field reads as one
/// continuous sward. Only the lowest two stops are down here, and almost nothing
/// reaches them.
///
/// The bottom of it also swings toward emerald rather than simply down, and the
/// swing fades out by the top where this ramp has to meet the grass. Deep
/// vegetation that is only a darker version of the vegetation above it says
/// nothing about depth — it says the lamp is dimmer there. A canopy interior is
/// lit by light that has already been through several centimetres of leaf, and
/// what comes out the other side is cooler as well as weaker. Two stops of red
/// given up and a little blue picked up is the whole of it, and it is the
/// difference between a cavity and a smudge.
pub const THATCH: [[f32; 3]; 8] = [
    [0.0838, 0.1938, 0.0318],
    [0.1058, 0.2298, 0.0330],
    [0.1292, 0.2625, 0.0362],
    [0.1530, 0.2934, 0.0368],
    [0.1770, 0.3232, 0.0372],
    [0.1980, 0.3492, 0.0376],
    [0.2130, 0.3700, 0.0384],
    [0.2239, 0.3842, 0.0397],
];

/// Bare earth. Olive-brown — the reference's soil sits near hue 50°, which is
/// closer to its own grass than to anything a painter would call brown.
///
/// Warmed a few percent off that measurement, deliberately, and it is the only
/// ramp here that is allowed to disagree with the art. Soil is the one material
/// in the field that is not green, so it is the only place a *complementary*
/// contrast is available at all — and a complementary contrast is worth far more
/// to a green field than another green is. Pushed to a true brown it would read
/// as a different game; held a few degrees warm of the grass it gives the eye
/// somewhere to rest, and makes the green around it read as green rather than as
/// the colour everything happens to be.
pub const SOIL: [[f32; 3]; 8] = [
    [0.2255, 0.2274, 0.0680],
    [0.2794, 0.2754, 0.0800],
    [0.3333, 0.3234, 0.0930],
    [0.3872, 0.3724, 0.1060],
    [0.4477, 0.4263, 0.1210],
    [0.5148, 0.4851, 0.1390],
    [0.5885, 0.5488, 0.1580],
    [0.6710, 0.6194, 0.1820],
];

/// Broadleaf: flatter shading and a touch bluer, so a leaf cluster separates
/// from the blades it sits among without being a different colour.
pub const LEAF: [[f32; 3]; 8] = [
    [0.2047, 0.3521, 0.0442],
    [0.2318, 0.3849, 0.0464],
    [0.2579, 0.4155, 0.0490],
    [0.2867, 0.4486, 0.0522],
    [0.3236, 0.4892, 0.0573],
    [0.3803, 0.5489, 0.0675],
    [0.4621, 0.6309, 0.0878],
    [0.6296, 0.7837, 0.1658],
];

/// Dry stems: the same value range, drained toward straw.
#[allow(clippy::approx_constant)]
pub const DRY: [[f32; 3]; 8] = [
    [0.2400, 0.2560, 0.0570],
    [0.2900, 0.3010, 0.0670],
    [0.3400, 0.3460, 0.0760],
    [0.3950, 0.3960, 0.0875],
    [0.4550, 0.4490, 0.1000],
    [0.5250, 0.5110, 0.1180],
    [0.6100, 0.5840, 0.1410],
    [0.7200, 0.6780, 0.1790],
];

/// How far below a ramp's own bottom the shadow extension reaches.
///
/// ## Why the measured ramp had to be extended downward
///
/// The ramps above are percentile maps of a painting, and that painting has no
/// shadows in it — its first percentile is 0.215 luminance. For a long time that
/// was exactly right, because the renderer had no shadows either: everything
/// dark in the field was a *narrow* dark, the gap between one blade and the next,
/// and [`THATCH`] reaching a little below the measurement covered it.
///
/// Real cast shadows change the requirement completely. A shadow is a broad dark
/// area with a caster, it is the thing the eye reads a light direction from, and
/// a shadow looked up in a ramp whose floor is a mid green comes back as a
/// slightly duller mid green. The lighting would have been correct and invisible.
///
/// So `q` is now allowed to go **negative**, down to `-SHADOW_DEPTH`, and the
/// region below zero is a generated extension rather than more measured stops.
/// Positive `q` still means exactly what it always did — the reference's own
/// percentile — so every constant tuned against the measured range keeps its
/// meaning, and only the terms that genuinely describe occlusion reach past it.
pub const SHADOW_DEPTH: f32 = 0.30;

/// The colour a ramp's bottom stop becomes at the full depth of shadow.
///
/// Not a multiply toward black, and not a lerp toward grey. Both of those are
/// what a shader does when nobody has looked at vegetation in shade: the first
/// gives a dead dark green that reads as underexposure, the second drains the
/// hue and reads as haze lying over the field rather than as shadow in it.
///
/// Light reaching the inside of a canopy has already been through several
/// centimetres of leaf, and leaf is a filter — it takes out far more red than
/// green and barely touches what little blue there is. So the three channels are
/// scaled by very different amounts, and the result is a deep, *cool*, still
/// clearly green colour whose blue channel has risen relative to its green even
/// though it has fallen in absolute terms.
#[inline]
fn shadow_floor(bottom: Vec3) -> Vec3 {
    Vec3::new(bottom.x * 0.21, bottom.y * 0.29, bottom.z * 0.64)
}

/// Look `q` up in a ramp, interpolating between stops.
///
/// `q` is a light index, not a luminance and not an albedo. It is everything the
/// shader knows about how lit a pixel is, collapsed to one number; the ramp turns
/// that back into colour.
///
/// `0..1` is the measured range — stop *i* is the reference art at its `i/n`
/// percentile. Below zero is the shadow extension, down to `-`[`SHADOW_DEPTH`];
/// above one clamps, because the top stop is already an extrapolation.
#[inline]
pub fn shade(tone: Tone, q: f32) -> Vec3 {
    let ramp = ramp(tone);
    if q < 0.0 {
        let bottom = Vec3::new(ramp[0][0], ramp[0][1], ramp[0][2]);
        // One at the ramp's own floor, zero at the full depth of shadow.
        let t = (1.0 + q / SHADOW_DEPTH).clamp(0.0, 1.0);
        // Eased rather than linear, so the extension spends most of its length
        // near the ramp it joins and only the last of it reaches the deepest
        // colour. A linear descent puts too much of the field's area at the
        // bottom, which is the same mistake as a shadow with no penumbra.
        let eased = t * t * (3.0 - 2.0 * t);
        return shadow_floor(bottom).lerp(bottom, eased);
    }
    let last = ramp.len() - 1;
    let position = q.min(1.0) * last as f32;
    let low = position as usize;
    let high = (low + 1).min(last);
    let t = position - low as f32;
    let a = ramp[low];
    let b = ramp[high];
    Vec3::new(
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    )
}

/// What a surface is, beyond how lit it is.
///
/// ## Why one light index is not enough any more
///
/// A ramp lookup answers "how bright is this" and the ramps answer it well —
/// they carry the hue shift that a multiply cannot, which is why shading here
/// has always been a lookup. What a single index cannot carry is *which* of
/// several things is bright.
///
/// The reference art's brightest paint is a sunlit tip: warm, yellow-green,
/// and only on the exposed upper third of a mature blade. Its darkest is a tuft
/// interior: cool emerald, saturated, nowhere near grey. Both are the same
/// green underneath and a one-dimensional ramp has to choose — either its top
/// is yellow, in which case every bright thing in the field goes yellow
/// including the ones that should not, or its top is not, in which case the
/// tips never catch.
///
/// The plate showed exactly that. Bright meant lime everywhere, and a field
/// where a colour is *everywhere* has no colour at all — it has a cast.
///
/// So the axes that decide which kind of bright this is travel with the pixel.
#[derive(Clone, Copy, Debug)]
pub struct Material {
    /// How much light of any kind reaches it, `0..1`.
    pub exposure: f32,
    /// Root-to-tip position on its own mark, `0..1`.
    pub along: f32,
    /// How old and established the mark is, `0..1`.
    pub maturity: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            along: 0.5,
            maturity: 0.5,
        }
    }
}

/// How far the warm and cool ends may travel from the measured ramp.
///
/// Small numbers doing a large amount of work, because they are *hue* moves at
/// constant luminance and the eye reads a few degrees of hue far more readily
/// than it reads a few percent of value. Push either much past this and the
/// field stops being one meadow with light on it and becomes two materials.
const WARM: f32 = 0.30;
const COOL: f32 = 0.26;

/// Shade a surface through its ramp and then along the material axes.
///
/// The lookup first, so the measured relationship between the channels survives
/// intact and only its balance moves afterwards. Both shifts are
/// luminance-preserving — see [`crate::bake`]'s `hue_only` for why that matters
/// so much more than it sounds: a hue move that also darkens turns "this is a
/// different green" into "this is dimmer", and whole regions of the field lose
/// light for a reason nobody asked for.
pub fn shade_material(tone: Tone, q: f32, material: Material) -> Vec3 {
    let base = shade(tone, q);
    // Soil and dry straw are not leaves and have no upper surface to catch the
    // sun on; shifting them by leaf rules turns bare earth green at its edges.
    if matches!(tone, Tone::Soil | Tone::Dry) {
        return base;
    }

    // Warm: sunlit, high on the blade, and mature. All three, multiplied — a
    // highlight is a coincidence of conditions rather than a property of
    // brightness, and requiring all three is what keeps the population of warm
    // pixels small enough to read as accents.
    let exposed = smoothstep(0.55, 1.0, material.exposure);
    let upper = smoothstep(0.45, 0.95, material.along);
    let warm = WARM * exposed * upper * (0.45 + material.maturity * 0.55);

    // Cool: whatever the light did not reach. Not gated on anything else,
    // because shade is not a coincidence — everything the sun misses is cooler,
    // and a field whose shadows are merely darker gives itself away as one
    // colour under a dimmer lamp.
    let cool = COOL * (1.0 - material.exposure) * (1.0 - upper * 0.35);

    let luma = base.dot(LUMA);
    // Toward straw at one end and emerald at the other, both at the pixel's own
    // luminance. Red is the channel that moves: in a palette whose blue sits
    // near 0.04, adding blue is a forty-percent change to a channel nobody
    // sees, while red sitting close to green is exactly what "too lime" means.
    let warmed = Vec3::new(base.x * 1.26, base.y, base.z * 0.72);
    let cooled = Vec3::new(base.x * 0.70, base.y, base.z + base.y * 0.085);
    let shifted = base.lerp(warmed, warm.clamp(0.0, 1.0));
    let shifted = shifted.lerp(cooled, cool.clamp(0.0, 1.0));

    // Renormalised to the luminance it started with, so neither shift costs
    // exposure. Exact at every input colour rather than at the one a constant
    // was derived from, which is what lets the multipliers above be stated as
    // the effect they are meant to have.
    let after = shifted.dot(LUMA);
    if after > 1.0e-6 {
        shifted * (luma / after)
    } else {
        shifted
    }
}

/// Rec. 709 luminance weights.
const LUMA: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

#[inline]
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Convert a linear colour to the sRGB bytes a PNG or a texture wants.
///
/// The ramps are stored as the reference's own sRGB values, so this is only the
/// clamp-and-quantise half; there is no gamma conversion, deliberately. Doing
/// the arithmetic in the space the art was authored in is what keeps a mid-tone
/// blend looking like the mid-tone a painter would have picked.
#[inline]
pub fn to_bytes(colour: Vec3) -> [u8; 3] {
    [
        (colour.x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (colour.y.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (colour.z.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

/// The inverse of [`to_bytes`], over a whole packed plate.
///
/// Only a rescale, because [`to_bytes`] is only a quantise — the ramps are
/// stored in the reference's own sRGB values and there is no gamma conversion in
/// either direction. What this is for is a plate that arrived as bytes from
/// somewhere else, usually Blender: an overlay drawn on it has to be drawn in
/// the same space as one drawn on a raster plate, or the two annotations are
/// different colours and read as meaning different things.
pub fn from_bytes_rgb(bytes: &[u8]) -> Vec<Vec3> {
    bytes
        .chunks_exact(3)
        .map(|pixel| {
            Vec3::new(
                pixel[0] as f32 / 255.0,
                pixel[1] as f32 / 255.0,
                pixel[2] as f32 / 255.0,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(c: Vec3) -> f32 {
        c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
    }

    #[test]
    fn packing_a_plate_and_unpacking_it_returns_the_plate() {
        let colours: Vec<Vec3> = (0..64)
            .map(|i| Vec3::splat(i as f32 / 63.0) * Vec3::new(1.0, 0.5, 0.25))
            .collect();
        let bytes: Vec<u8> = colours.iter().flat_map(|c| to_bytes(*c)).collect();
        let back = from_bytes_rgb(&bytes);
        assert_eq!(back.len(), colours.len());
        for (a, b) in back.iter().zip(&colours) {
            assert!(a.distance(*b) < 1.0 / 255.0, "{a:?} against {b:?}");
        }
        // A trailing partial pixel is dropped rather than half-decoded.
        assert_eq!(from_bytes_rgb(&[0, 0, 0, 9]).len(), 1);
    }

    #[test]
    fn every_ramp_climbs() {
        for tone in [Tone::Soil, Tone::Thatch, Tone::Grass, Tone::Leaf, Tone::Dry] {
            let ramp = ramp(tone);
            for pair in ramp.windows(2) {
                let (a, b) = (Vec3::from(pair[0]), Vec3::from(pair[1]));
                assert!(luma(b) > luma(a), "{tone:?} ramp goes backwards");
            }
        }
    }

    #[test]
    fn grass_shadow_stays_green_rather_than_going_grey() {
        // The failure this ramp exists to prevent. A multiply-based shader
        // reaches grey here; the reference never does.
        let dark = shade(Tone::Grass, 0.0);
        assert!(dark.y > dark.x * 1.5, "shadow lost its hue: {dark:?}");
        assert!(dark.z < dark.y * 0.35, "shadow went blue: {dark:?}");
    }

    #[test]
    fn the_brightest_grass_is_yellow_green_not_white() {
        let bright = shade(Tone::Grass, 1.0);
        assert!(
            bright.z < 0.30,
            "tip glint is washing out to white: {bright:?}"
        );
        assert!(
            bright.x < bright.y,
            "tip glint turned yellow-orange: {bright:?}"
        );
    }

    #[test]
    fn soil_is_olive_rather_than_red() {
        for stop in SOIL {
            let c = Vec3::from(stop);
            assert!(c.y > c.x * 0.85, "soil went red: {c:?}");
            assert!(c.z < c.x * 0.45, "soil went grey: {c:?}");
        }
    }

    #[test]
    fn lookups_clamp_rather_than_wrap() {
        assert_eq!(shade(Tone::Grass, -5.0), shade(Tone::Grass, -SHADOW_DEPTH));
        assert_eq!(shade(Tone::Grass, 5.0), shade(Tone::Grass, 1.0));
    }

    #[test]
    fn the_material_axes_move_hue_and_not_exposure() {
        // The property both shifts are built around. A hue move that also
        // darkened would turn "this part of the field is a different green" into
        // "this part of the field is dimmer", and whole regions would lose light
        // for a reason nobody asked for.
        for tone in [Tone::Grass, Tone::Thatch, Tone::Leaf] {
            for step in 0..=10 {
                let q = step as f32 / 10.0;
                let plain = luma(shade(tone, q));
                for exposure in [0.0f32, 0.5, 1.0] {
                    for along in [0.0f32, 0.6, 1.0] {
                        let shifted = luma(shade_material(
                            tone,
                            q,
                            Material {
                                exposure,
                                along,
                                maturity: 1.0,
                            },
                        ));
                        assert!(
                            (shifted - plain).abs() < 1.0e-4,
                            "{tone:?} at q {q} exposure {exposure} along {along} \
                             moved luminance {plain:.4} → {shifted:.4}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_sunlit_tip_is_warmer_than_a_shaded_root() {
        // The whole point of the axes. Both are the same green underneath and a
        // one-dimensional ramp has to choose between them.
        let warmth = |c: Vec3| c.x / c.y.max(1.0e-6);
        let tip = shade_material(
            Tone::Grass,
            0.75,
            Material {
                exposure: 1.0,
                along: 1.0,
                maturity: 1.0,
            },
        );
        let root = shade_material(
            Tone::Grass,
            0.75,
            Material {
                exposure: 0.1,
                along: 0.05,
                maturity: 1.0,
            },
        );
        // A sixth, which sounds small and is not: this is a hue separation at
        // constant luminance, and the eye reads a few degrees of hue far more
        // readily than it reads a few percent of value. Bigger than this and the
        // two stop being one meadow with light on it.
        assert!(
            warmth(tip) > warmth(root) * 1.12,
            "a sunlit tip {tip:?} is no warmer than a shaded root {root:?}"
        );
        // And both are still green.
        assert!(tip.y > tip.x, "the tip went orange: {tip:?}");
        // Both compared at the same `q` so the shift is the only difference,
        // which puts the "shaded root" higher up the ramp than a real one ever
        // sits — the measured ramp is already warm at three quarters. A root
        // this bright staying this green is the stronger claim anyway.
        assert!(root.y > root.x * 1.4, "the root lost its hue: {root:?}");
    }

    #[test]
    fn a_shaded_tip_does_not_get_the_warm_treatment() {
        // Warmth is a coincidence of three conditions rather than a property of
        // height on the blade. A tip in shade is just a tip in shade.
        let warmth = |c: Vec3| c.x / c.y.max(1.0e-6);
        let lit = shade_material(
            Tone::Grass,
            0.7,
            Material {
                exposure: 1.0,
                along: 1.0,
                maturity: 1.0,
            },
        );
        let shaded = shade_material(
            Tone::Grass,
            0.7,
            Material {
                exposure: 0.2,
                along: 1.0,
                maturity: 1.0,
            },
        );
        assert!(warmth(shaded) < warmth(lit));
    }

    #[test]
    fn earth_and_straw_are_left_alone() {
        // They are not leaves. Shifting bare soil by leaf rules turns the edge
        // of every opening green, which is the one thing an opening must not do.
        for tone in [Tone::Soil, Tone::Dry] {
            for exposure in [0.0f32, 1.0] {
                let shifted = shade_material(
                    tone,
                    0.6,
                    Material {
                        exposure,
                        along: 1.0,
                        maturity: 1.0,
                    },
                );
                assert_eq!(shifted, shade(tone, 0.6));
            }
        }
    }

    #[test]
    fn the_shadow_extension_joins_the_measured_ramp() {
        // A step where the generated part meets the measured part would print as
        // a contour line wherever the light index crosses zero, which is
        // everywhere the shadows are softest.
        for tone in [Tone::Soil, Tone::Thatch, Tone::Grass, Tone::Leaf, Tone::Dry] {
            let below = shade(tone, -1.0e-4);
            let at = shade(tone, 0.0);
            assert!(
                (below - at).length() < 1.0e-3,
                "{tone:?} steps at the join: {below:?} vs {at:?}"
            );
        }
    }

    #[test]
    fn the_shadow_extension_is_a_real_dark() {
        // The whole reason it exists. The measured ramp's floor is a mid green —
        // the painting it came from has no shadows in it — so a cast shadow
        // looked up there would come back as a slightly duller mid green.
        let floor = luma(shade(Tone::Grass, 0.0));
        let deep = luma(shade(Tone::Grass, -SHADOW_DEPTH));
        assert!(
            deep < floor * 0.45,
            "the deepest shadow is {deep:.3} against a ramp floor of {floor:.3}"
        );
        // And it descends the whole way rather than bottoming out early.
        let mut previous = deep;
        for step in 1..=20 {
            let q = -SHADOW_DEPTH + step as f32 / 20.0 * SHADOW_DEPTH;
            let value = luma(shade(Tone::Grass, q));
            assert!(value >= previous - 1.0e-6, "the extension goes backwards");
            previous = value;
        }
    }

    #[test]
    fn shadow_is_cooler_than_the_light_it_replaces() {
        // Not "green times a number". Light reaching the inside of a canopy has
        // been through several centimetres of leaf, which takes out far more red
        // than green — so the deepest shade is a different, cooler green rather
        // than a dimmer version of the same one. A shadow that is only darker
        // reads as underexposure.
        let lit = shade(Tone::Grass, 0.0);
        let shade_colour = shade(Tone::Grass, -SHADOW_DEPTH);
        let warmth = |c: Vec3| c.x / c.y.max(1.0e-6);
        assert!(
            warmth(shade_colour) < warmth(lit) * 0.85,
            "shadow {shade_colour:?} is no cooler than light {lit:?}"
        );
        // Still unmistakably green, never grey and never black.
        assert!(shade_colour.y > shade_colour.x * 1.5, "{shade_colour:?}");
        assert!(shade_colour.y > 0.03, "the shadow bottomed out at black");
    }

    #[test]
    fn the_ramp_is_continuous() {
        let mut previous = shade(Tone::Grass, 0.0);
        for step in 1..=200 {
            let next = shade(Tone::Grass, step as f32 / 200.0);
            assert!((next - previous).length() < 0.06, "banding at {step}");
            previous = next;
        }
    }
}

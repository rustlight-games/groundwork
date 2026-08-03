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

use bevy::prelude::*;

/// Which ramp a pixel is shaded through.
///
/// A small closed set on purpose: every material in the field is one of these,
/// and a pixel that cannot say which one it is has no business being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Tone {
    /// Bare earth: olive-brown, never reddish.
    Soil = 0,
    /// The dark mat under the canopy.
    Thatch = 1,
    /// Ordinary blades — most of the field.
    Grass = 2,
    /// Broadleaf clusters, a shade cooler and flatter than blades.
    Leaf = 3,
    /// Dry stems and the odd bleached tuft.
    Dry = 4,
}

impl Tone {
    /// The ramp this tone shades through.
    #[inline]
    pub fn ramp(self) -> &'static [[f32; 3]] {
        match self {
            Tone::Soil => &SOIL,
            Tone::Thatch => &THATCH,
            Tone::Grass => &GRASS,
            Tone::Leaf => &LEAF,
            Tone::Dry => &DRY,
        }
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

/// Look `q` up in a ramp, interpolating between stops.
///
/// `q` is a light index in `[0, 1]`, not a luminance and not an albedo. It is
/// everything the shader knows about how lit a pixel is, collapsed to one
/// number; the ramp turns that back into colour.
#[inline]
pub fn shade(tone: Tone, q: f32) -> Vec3 {
    let ramp = tone.ramp();
    let last = ramp.len() - 1;
    let position = q.clamp(0.0, 1.0) * last as f32;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(c: Vec3) -> f32 {
        c.x * 0.2126 + c.y * 0.7152 + c.z * 0.0722
    }

    #[test]
    fn every_ramp_climbs() {
        for tone in [Tone::Soil, Tone::Thatch, Tone::Grass, Tone::Leaf, Tone::Dry] {
            let ramp = tone.ramp();
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
        assert_eq!(shade(Tone::Grass, -5.0), shade(Tone::Grass, 0.0));
        assert_eq!(shade(Tone::Grass, 5.0), shade(Tone::Grass, 1.0));
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

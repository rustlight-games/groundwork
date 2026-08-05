//! What colour the ground is, and how it changes when it gets wet.
//!
//! ## Measured off the profile, not off the picture
//!
//! Every number here comes from the `GroundMaterialProfile` and the state fields
//! that drive it, not from beauty pixels. A beauty pixel has been through the
//! sun angle, the sky, the canopy occlusion and a filmic view transform, and
//! comparing two of them tells you the lighting changed. The question this
//! module answers is narrower and more useful: *did the material respond the way
//! the profile says it should?*
//!
//! ## The failure this exists to reject
//!
//! A linear grey dimmer masquerading as wet soil. Measured soil reflectance
//! changes nonlinearly with moisture and does not multiply every channel by the
//! same number — red survives relative to blue, by an amount the soil's own
//! composition decides. A wet response implemented as `albedo * 0.6` passes every
//! visual check ("it got darker") and is wrong in a way that shows the moment two
//! soils are put side by side.
//!
//! So: the endpoints are checked against the *declared* dry and wet mid tones,
//! the response is checked for monotonicity through the range, and the channel
//! ratios are reported separately so a grey multiplier is visible as a flat line.

use terrain_core::ground_material::{GroundMaterialProfile, LinearRgb};

/// One colour, described the ways the report needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourMetric {
    pub linear_rgb: LinearRgb,
    pub luminance: f64,
    /// Green over red. Together with `b_over_r` this is the hue, expressed the
    /// way a soil reference photograph can be measured without a colour chart.
    pub g_over_r: f64,
    pub b_over_r: f64,
    /// CIE Lab, for a perceptual distance between two variants.
    pub lab: [f64; 3],
}

impl ColourMetric {
    pub fn of(colour: LinearRgb) -> Self {
        let [r, g, b] = colour.map(|c| c as f64);
        Self {
            linear_rgb: colour,
            luminance: 0.2126 * r + 0.7152 * g + 0.0722 * b,
            g_over_r: if r > 0.0 { g / r } else { 0.0 },
            b_over_r: if r > 0.0 { b / r } else { 0.0 },
            lab: linear_rgb_to_lab(colour),
        }
    }
}

/// sRGB primaries to CIE XYZ under D65, then to Lab.
///
/// The transform is declared rather than assumed because a Lab value means
/// nothing without one: the same linear triple is a different Lab under D50, and
/// two reports using different white points would be comparing different
/// quantities while reporting the same field name.
pub fn linear_rgb_to_lab(colour: LinearRgb) -> [f64; 3] {
    let [r, g, b] = colour.map(|c| c as f64);
    let x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = 0.0193339 * r + 0.1191920 * g + 0.9503041 * b;
    // D65 white.
    let (xn, yn, zn) = (0.95047, 1.0, 1.08883);
    let f = |t: f64| {
        const DELTA: f64 = 6.0 / 29.0;
        if t > DELTA * DELTA * DELTA {
            t.cbrt()
        } else {
            t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
        }
    };
    let (fx, fy, fz) = (f(x / xn), f(y / yn), f(z / zn));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 between two Lab colours.
///
/// The perceptual distance the render comparison uses. Implemented rather than
/// approximated with a Euclidean Lab distance because the whole point of using
/// it is that Lab is *not* perceptually uniform in the blue-yellow region, which
/// is exactly where wet soil moves.
pub fn delta_e_2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (l1, a1, b1) = (a[0], a[1], a[2]);
    let (l2, a2, b2) = (b[0], b[1], b[2]);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let c_bar = (c1 + c2) * 0.5;
    let c7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c7 / (c7 + 25.0f64.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();
    let h1p = if b1 == 0.0 && a1p == 0.0 {
        0.0
    } else {
        b1.atan2(a1p).to_degrees().rem_euclid(360.0)
    };
    let h2p = if b2 == 0.0 && a2p == 0.0 {
        0.0
    } else {
        b2.atan2(a2p).to_degrees().rem_euclid(360.0)
    };

    let delta_l = l2 - l1;
    let delta_c = c2p - c1p;
    let delta_h = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let delta_hp = 2.0 * (c1p * c2p).sqrt() * (delta_h.to_radians() * 0.5).sin();

    let l_bar = (l1 + l2) * 0.5;
    let c_bar_p = (c1p + c2p) * 0.5;
    let h_bar_p = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) * 0.5
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) * 0.5
    } else {
        (h1p + h2p - 360.0) * 0.5
    };

    let t = 1.0 - 0.17 * (h_bar_p - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_bar_p).to_radians().cos()
        + 0.32 * (3.0 * h_bar_p + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_bar_p - 63.0).to_radians().cos();
    let delta_theta = 30.0 * (-(((h_bar_p - 275.0) / 25.0).powi(2))).exp();
    let c_bar_p7 = c_bar_p.powi(7);
    let rc = 2.0 * (c_bar_p7 / (c_bar_p7 + 25.0f64.powi(7))).sqrt();
    let sl = 1.0 + (0.015 * (l_bar - 50.0).powi(2)) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;
    let rt = -rc * (2.0 * delta_theta.to_radians()).sin();

    ((delta_l / sl).powi(2)
        + (delta_c / sc).powi(2)
        + (delta_hp / sh).powi(2)
        + rt * (delta_c / sc) * (delta_hp / sh))
        .sqrt()
}

/// One point of a moisture sweep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoistureSample {
    pub moisture: f32,
    pub colour: ColourMetric,
}

/// What a profile's optics do across their declared range.
#[derive(Clone, Debug, PartialEq)]
pub struct OpticsMetrics {
    pub profile: String,
    pub dry: ColourMetric,
    pub wet: ColourMetric,
    pub sweep: Vec<MoistureSample>,
    /// Whether reflectance falls monotonically as the ground wets.
    pub moisture_albedo_monotone: bool,
    /// Whether every channel stays finite and non-negative throughout.
    pub finite_and_non_negative: bool,
    /// Whether the sweep hits the declared dry mid at zero and wet mid at one.
    pub endpoints_match_declaration: bool,
    /// How much the hue moves across the sweep, as a CIEDE2000 distance.
    ///
    /// Reported so a grey dimmer is visible. Multiplying every channel by one
    /// number moves lightness and leaves the chroma ratios untouched, which
    /// shows here as a large distance and a flat `g_over_r`.
    pub delta_e_dry_to_wet: f64,
    /// The span of `g_over_r` across the sweep.
    ///
    /// Near zero means a single grey multiplier, which is the failure this
    /// module exists to reject.
    pub hue_ratio_span: f64,
}

/// Sweep one profile's wet response and report what it did.
pub fn measure(profile: &GroundMaterialProfile, steps: usize) -> OpticsMetrics {
    let steps = steps.max(2);
    let dry_mid = profile.optics.dry_palette.mid;
    let wet_mid = profile.optics.wet.wet_mid;

    let mut sweep = Vec::with_capacity(steps);
    let mut finite = true;
    for step in 0..steps {
        let moisture = step as f32 / (steps - 1) as f32;
        // Tone 0.5 is the palette's mid stop exactly, which is the tone the
        // profile's declared wet mid was measured at. Sweeping any other tone
        // would compare the response against a colour nobody measured.
        let colour = profile.albedo(0.5, moisture);
        if !colour.iter().all(|c| c.is_finite() && *c >= 0.0) {
            finite = false;
        }
        sweep.push(MoistureSample {
            moisture,
            colour: ColourMetric::of(colour),
        });
    }

    // Monotone in luminance, with a tolerance for the last bit of an `f32`
    // multiply. Strict monotonicity would fail on a profile whose wet mid
    // happens to equal its dry mid, which is a legitimate thing for a material
    // that does not darken.
    let mut monotone = true;
    for pair in sweep.windows(2) {
        if pair[1].colour.luminance > pair[0].colour.luminance + 1.0e-9 {
            monotone = false;
        }
    }

    let dry = ColourMetric::of(dry_mid);
    let wet = ColourMetric::of(sweep[steps - 1].colour.linear_rgb);
    let close = |a: LinearRgb, b: LinearRgb| {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| (x - y).abs() <= 1.0e-4 + y.abs() * 1.0e-3)
    };
    let ratios: Vec<f64> = sweep.iter().map(|s| s.colour.g_over_r).collect();
    let span = ratios.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        - ratios.iter().cloned().fold(f64::INFINITY, f64::min);

    OpticsMetrics {
        profile: profile.key.as_str().to_string(),
        dry,
        wet,
        moisture_albedo_monotone: monotone,
        finite_and_non_negative: finite,
        endpoints_match_declaration: close(sweep[0].colour.linear_rgb, dry_mid)
            && close(sweep[steps - 1].colour.linear_rgb, wet_mid),
        delta_e_dry_to_wet: delta_e_2000(dry.lab, wet.lab),
        hue_ratio_span: span,
        sweep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_and_a_black_are_the_same_hue_and_different_lightness() {
        let grey = ColourMetric::of([0.5, 0.5, 0.5]);
        let dark = ColourMetric::of([0.1, 0.1, 0.1]);
        assert!((grey.g_over_r - 1.0).abs() < 1.0e-9);
        assert!((dark.g_over_r - 1.0).abs() < 1.0e-9);
        assert!(grey.lab[0] > dark.lab[0]);
        // And their chroma is zero, so the only distance between them is
        // lightness. Not *exactly* zero: the published sRGB matrix rows sum to
        // the D65 white point only to seven digits, and Lab's `a` and `b` scale
        // that residue by five hundred. A tolerance rather than a correction,
        // because adjusting the matrix to close the gap would move every colour
        // away from the standard to make one test read better.
        assert!(grey.lab[1].abs() < 1.0e-4 && grey.lab[2].abs() < 1.0e-4);
    }

    #[test]
    fn delta_e_is_zero_for_a_colour_against_itself() {
        let colour = linear_rgb_to_lab([0.0545, 0.0343, 0.0222]);
        assert!(delta_e_2000(colour, colour) < 1.0e-9);
    }

    #[test]
    fn delta_e_grows_with_a_visible_difference() {
        // Not a calibration test — a sanity check that the formula is oriented
        // the right way and does not return a constant.
        let a = linear_rgb_to_lab([0.0545, 0.0343, 0.0222]);
        let b = linear_rgb_to_lab([0.0210, 0.0116, 0.0058]);
        let far = delta_e_2000(a, b);
        let near = delta_e_2000(a, linear_rgb_to_lab([0.0550, 0.0345, 0.0224]));
        assert!(far > near * 10.0, "far {far}, near {near}");
    }

    #[test]
    fn a_soil_darkens_monotonically_and_keeps_its_hue_moving() {
        // The two halves of the wet response, together. Darkening alone is what
        // a grey dimmer does; the hue ratio moving as well is what says the
        // response came from the profile's own measured wet mid rather than from
        // a multiplier.
        let profile = crate::ground::scenarios::loam_profile();
        let metrics = measure(&profile, 9);
        assert!(metrics.moisture_albedo_monotone, "{:?}", metrics.sweep);
        assert!(metrics.finite_and_non_negative);
        assert!(
            metrics.endpoints_match_declaration,
            "dry {:?} wet {:?}",
            metrics.sweep[0].colour.linear_rgb,
            metrics.sweep.last().expect("a sweep").colour.linear_rgb
        );
        assert!(
            metrics.delta_e_dry_to_wet > 1.0,
            "a wet soil that moved by only {} is not visibly wet",
            metrics.delta_e_dry_to_wet
        );
    }

    #[test]
    fn a_grey_multiplier_is_visible_as_a_flat_hue_ratio() {
        // The failure this module exists to reject, constructed deliberately so
        // the detector is shown to fire. A profile whose wet mid is its dry mid
        // scaled by one number has a hue-ratio span of exactly zero.
        let mut profile = crate::ground::scenarios::loam_profile();
        let mid = profile.optics.dry_palette.mid;
        profile.optics.wet.wet_mid = [mid[0] * 0.4, mid[1] * 0.4, mid[2] * 0.4];
        let metrics = measure(&profile, 9);
        assert!(
            metrics.hue_ratio_span < 1.0e-6,
            "a grey multiplier moved the hue ratio by {}",
            metrics.hue_ratio_span
        );

        // Where the real profile, whose wet mid was measured rather than
        // computed, does move it.
        let real = measure(&crate::ground::scenarios::loam_profile(), 9);
        assert!(
            real.hue_ratio_span > 1.0e-4,
            "the shipped loam behaves like a grey multiplier: span {}",
            real.hue_ratio_span
        );
    }
}

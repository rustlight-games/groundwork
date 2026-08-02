//! The colours grass is allowed to be.
//!
//! Pixel art is defined by its palette more than by its resolution. A low-res
//! buffer with continuous shading in it is just a small photograph; what makes
//! an image read as *drawn* is that every pixel is one of a handful of colours
//! someone chose. So shading never produces a colour. It produces a brightness,
//! and that brightness selects an entry from this table.
//!
//! ## The palette is baked from the lighting rig
//!
//! The entries are not hand-picked greens. They are [`crate::light`]'s three
//! suns evaluated against a grass albedo and then quantised — which is exactly
//! how a pixel artist de-makes a 3D render: light the thing, then pick the
//! palette off the result.
//!
//! Doing it that way rather than by eye buys the one property that matters for
//! a game whose characters are baked sprites out of the same rig: the grass and
//! the units cannot drift apart. Change the key's colour and both move
//! together. Hand-authored greens would look right beside today's sprites and
//! wrong beside tomorrow's, and nothing would report it.
//!
//! ## Ramps rather than one gradient
//!
//! A single dark-to-light ramp collapses a meadow into one material. Real grass
//! reads as many plants because neighbouring blades differ in *hue* as well as
//! brightness — some in the blue fill, some catching the golden key.
//!
//! Each ramp is therefore the rig evaluated at a different strand orientation,
//! and each of the sixteen steps within it is a different depth in the canopy:
//!
//! | Ramp | Strand orientation | Reads as |
//! |---|---|---|
//! | [`SHADOW`] | along the key, catching almost none of it | Cool, dark. Deep canopy |
//! | [`BODY`] | oblique to the key | The bulk of the field |
//! | [`HIGHLIGHT`] | across the key, catching all of it | Warm. Blades in the sun |
//! | [`DRY`] | oblique, straw albedo | Trampled grass |
//!
//! A blade picks a ramp from how much key it is actually catching and stays on
//! it; shading only moves it up and down that ramp. That is what lets the rig
//! show up as *colour* — a gust turning a patch of blades toward the key walks
//! them onto the warm ramp — while every pixel stays exactly on palette.
//!
//! Grass does not fade smoothly into [`DRY`] as it is crushed; it cannot, with
//! a fixed palette. Instead each blade decides independently whether it has
//! been crushed, weighted by how flattened its cell is, so a trail appears as a
//! rising *proportion* of dry blades. That is how a pixel artist would draw it
//! and it costs nothing.
//!
//! ## Baked in linear, stored as sRGB
//!
//! The bake works in linear light, then quantises to eight-bit sRGB — the same
//! precision a palette would have been authored at. The shader is handed those
//! entries converted back to linear and the render target converts once more on
//! write, so what reaches the screen is exactly the quantised entry. Verified
//! by [`tests::a_round_trip_through_linear_returns_the_stored_entry`].
//!
//! `RAMPS` and `RAMP_STEPS` are duplicated in `assets/shaders/grass.wgsl`;
//! `shader_palette_matches_this_module` fails if they drift.

use std::sync::LazyLock;

use bevy::prelude::*;

use crate::light;

/// Hue families a blade can belong to.
pub const RAMPS: usize = 4;

/// Brightness steps within a ramp.
///
/// Sixteen, for sixty-four colours in total. This is a *high*-colour palette on
/// purpose — the target is modern hi-bit pixel art, not an eight-bit console.
///
/// Six steps posterises: a canopy has a continuous gradient from its floor to
/// its tips, and forcing that through six tones puts visible bands across the
/// field and throws away most of the lighting rig's work. Sixteen blends
/// smoothly enough to read as painted while still being a fixed, deliberate set
/// of colours rather than whatever the shader happened to compute.
///
/// What makes the result pixel art is not the palette size. It is the canvas
/// grid, the snapping and the pose quantisation in [`crate::pixel`] and the
/// vertex shader — those survive any palette depth, and they are what a
/// resolution-independent renderer cannot fake.
pub const RAMP_STEPS: usize = 16;

/// Total entries uploaded to the shader.
pub const PALETTE_SIZE: usize = RAMPS * RAMP_STEPS;

/// Index of the cool, dark ramp.
pub const SHADOW: usize = 0;
/// Index of the main ramp.
pub const BODY: usize = 1;
/// Index of the warm, sunlit ramp.
pub const HIGHLIGHT: usize = 2;
/// Index of the trampled ramp.
pub const DRY: usize = 3;

/// Grass albedo per ramp, in linear light.
///
/// Dark and strongly green-dominant. Foliage albedo really is this low — a
/// leaf reflects around a tenth of the red and blue landing on it — and using a
/// bright albedo is the classic way to end up with pale sage grass no amount of
/// palette tuning can rescue, because the ambient's red and blue get multiplied
/// up along with the green.
///
/// The three living ramps vary slightly in hue as well as in lighting, so that
/// shaded and sunlit grass are the same plant rather than two different ones.
///
/// Red is not incidental either. The art target runs a red-to-green ratio of
/// 0.65 — a distinctly yellow-green, not a pure one — a warm green, not a pure one — and pushing saturation up without
/// raising red alongside it drove ours to 0.32 and the grass toward emerald.
/// Saturation and hue have to be tuned together or one simply eats the other.
///
/// Blue is almost absent, and measurably so: the art target averages a blue
/// channel of 10 against a green of 93, and matching that is most of what moves
/// `grass.match.chroma`. An earlier revision carried three times this much blue
/// and scored zero on that metric — the greens were plausible on their own and
/// obviously wrong beside the reference, because the sky and fill multiply
/// whatever blue the albedo offers them straight back up.
const ALBEDO: [Vec3; RAMPS] = [
    Vec3::new(0.050, 0.130, 0.022), // shadow: cooler, bluer leaf
    Vec3::new(0.078, 0.172, 0.018), // body
    Vec3::new(0.128, 0.224, 0.015), // highlight: yellower leaf
    Vec3::new(0.150, 0.128, 0.052), // dry: straw
];

/// Exposure applied to the whole bake.
///
/// Scaling the rig rather than the entries keeps every ratio the character
/// sprites were lit at. This is the one number to turn if the field as a whole
/// is too bright or too dark.
///
/// It is set from the art target rather than by eye. The reference plate spans
/// a luminance of 0.18 at its fifth percentile to 0.49 at its ninety-fifth; an
/// earlier value of 0.185 reached only 0.34 at the top, and the measured result
/// was a field with the right *average* brightness and a third of the target's
/// range — which is exactly what "flat" looks like when you take it apart.
/// Brightness is not the problem a flat image has. Reach is.
const EXPOSURE: f32 = 0.37;

/// Sky reaching the deepest step of a ramp. Shared with the shader.
const OCCLUSION_FLOOR: f32 = light::CANOPY_FLOOR;

/// How far a rim highlight washes toward the light's own colour.
///
/// Foliage catching a hard backlight goes pale, not saturated green — but only
/// a little. The rim is the coolest light in the rig, so every unit of wash
/// toward its own colour is blue arriving in the palette by the back door. With
/// the albedo's blue already down at 0.013 this was the largest remaining
/// source: the art target runs a blue-to-green ratio of 0.10 and a fifth of a
/// wash put ours at 0.21.
const RIM_WASH: f32 = 0.07;

/// Extra saturation applied after the bake.
///
/// Backed off from where the resemblance metric wanted it. Chasing the plate's
/// channel ratios drove this high enough that the greens went acidic — a
/// perfectly defensible number producing a colour nobody wants to look at for
/// an hour. The metric measures a still frame; a game is played in motion and
/// at length, and vivid reads as tiring long before it reads as wrong.
///
/// The rig's fill and sky are broad-spectrum, so a physically summed result
/// drifts toward grey — correct, and not what stylised game art looks like.
/// Pulling saturation back up is the same call as rendering the character
/// sprites through Blender's Standard view transform instead of AgX, and for
/// the same reason: AgX desaturates highlights in a way that flatters
/// photographs and washes out costume golds and foliage greens.
const SATURATION: f32 = 1.12;

/// The baked palette, `[ramp][step]` with the darkest step first.
static PALETTE: LazyLock<[[[u8; 3]; RAMP_STEPS]; RAMPS]> = LazyLock::new(bake);

/// Strand orientation each ramp is baked at.
///
/// Expressed against the key so the ramps stay meaningful if the rig moves.
fn ramp_tangent(ramp: usize) -> Vec3 {
    let key = light::key().direction;
    // A ground-plane vector square on to the key, which a blade leaning across
    // the sun presents.
    let across = Vec3::new(-key.y, key.x, 0.0).normalize();
    match ramp {
        SHADOW => key,                         // along it: catches none
        HIGHLIGHT => across,                   // across it: catches all
        _ => (key + across * 1.6).normalize(), // oblique
    }
}

fn bake() -> [[[u8; 3]; RAMP_STEPS]; RAMPS] {
    let mut out = [[[0u8; 3]; RAMP_STEPS]; RAMPS];
    let key = light::key();
    let fill = light::fill();
    let rim = light::rim();

    for ramp in 0..RAMPS {
        let tangent = ramp_tangent(ramp);
        let albedo = ALBEDO[ramp];
        let rim_albedo = albedo.lerp(Vec3::ONE, RIM_WASH);

        for (step, entry) in out[ramp].iter_mut().enumerate() {
            let t = step as f32 / (RAMP_STEPS - 1) as f32;
            // Swept evenly in *occlusion*, not in height. The ramp is indexed
            // by exposure, and the shader is what maps a blade's height onto
            // occlusion; sweeping height here instead would inherit that curve
            // twice and flatten both ends of the ramp — the top two steps come
            // out identical, because a smoothstep saturates.
            let occlusion = OCCLUSION_FLOOR + (1.0 - OCCLUSION_FLOOR) * t;
            let response = light::respond(tangent, occlusion);

            // A blade's tip points up, its base is buried, so what the sky
            // gives it goes up with the step alongside everything else.
            let ambient = light::sky(-0.35 + 1.25 * t) * occlusion;

            let linear = albedo
                * (key.radiance() * response.key + fill.radiance() * response.fill + ambient)
                + rim_albedo * rim.radiance() * response.rim;

            *entry = quantise(saturate(linear * EXPOSURE, SATURATION));
        }
    }
    out
}

/// Push a linear colour away from its own grey, without crushing a channel.
///
/// The floor matters. Saturation drives the weak channels down, and once one
/// reaches zero it stops carrying information: further steps up the ramp change
/// only the other two, so the ramp flattens in hue exactly where it should be
/// richest. Clamping to a fraction of the grey instead keeps every channel
/// alive while still letting blue fall to the near-nothing the art target
/// actually has.
fn saturate(colour: Vec3, amount: f32) -> Vec3 {
    let grey = 0.2126 * colour.x + 0.7152 * colour.y + 0.0722 * colour.z;
    let pushed = Vec3::splat(grey) + (colour - Vec3::splat(grey)) * amount;
    pushed.max(Vec3::splat(grey * CHANNEL_FLOOR))
}

/// Smallest a channel may fall to, as a fraction of the colour's own grey.
///
/// Low, because on this palette the floor binds on exactly one channel — blue —
/// and blue is the channel furthest from the target. It exists to stop a
/// channel reaching zero and flattening the ramp's hue, not to keep it
/// comfortable, so it sits just above where that happens.
const CHANNEL_FLOOR: f32 = 0.025;

fn quantise(linear: Vec3) -> [u8; 3] {
    [
        (linear_to_srgb(linear.x).clamp(0.0, 1.0) * 255.0).round() as u8,
        (linear_to_srgb(linear.y).clamp(0.0, 1.0) * 255.0).round() as u8,
        (linear_to_srgb(linear.z).clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

/// The colour behind the grass.
///
/// Deep shade from the shadow ramp rather than the palette's darkest entry.
/// Gaps in the canopy are roughly a sixth of the frame even when it is dense,
/// and at the very bottom of the ramp they stop reading as shade *between*
/// blades and start reading as holes punched through to nothing — which is
/// exactly the difference between the reference art and a sprite on a black
/// background.
pub fn ground() -> Color {
    let [r, g, b] = channels(SHADOW, 1);
    Color::srgb_u8(r, g, b)
}

/// The darkest entry anywhere in the palette.
///
/// Not what [`ground`] uses — see the note there — but the floor the ramps are
/// checked against.
pub fn darkest() -> [u8; 3] {
    let mut best = PALETTE[SHADOW][0];
    let mut best_luma = f32::MAX;
    for ramp in 0..RAMPS {
        for step in 0..RAMP_STEPS {
            let luma = luminance(ramp, step);
            if luma < best_luma {
                best_luma = luma;
                best = PALETTE[ramp][step];
            }
        }
    }
    best
}

/// One entry, as the sRGB bytes it was quantised to.
pub fn channels(ramp: usize, step: usize) -> [u8; 3] {
    PALETTE[ramp][step]
}

/// One entry as hex, the way a palette is normally written down.
pub fn hex(ramp: usize, step: usize) -> u32 {
    let [r, g, b] = channels(ramp, step);
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// One entry in linear space, ready for the shader.
pub fn entry(ramp: usize, step: usize) -> Vec4 {
    let [r, g, b] = channels(ramp, step);
    Vec4::new(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), 1.0)
}

/// The whole palette flattened the way the shader indexes it:
/// `ramp * RAMP_STEPS + step`.
pub fn flattened() -> [Vec4; PALETTE_SIZE] {
    let mut out = [Vec4::ZERO; PALETTE_SIZE];
    for ramp in 0..RAMPS {
        for step in 0..RAMP_STEPS {
            out[ramp * RAMP_STEPS + step] = entry(ramp, step);
        }
    }
    out
}

/// Perceived brightness of an entry, on the stored sRGB values.
///
/// sRGB rather than linear on purpose: this checks that a ramp looks evenly
/// spaced to a person, and people see roughly in sRGB.
pub fn luminance(ramp: usize, step: usize) -> f32 {
    let [r, g, b] = channels(ramp, step);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

/// Darkest to lightest across the whole palette, in 0..1.
///
/// A palette that drifts narrow makes the grass read as one flat material no
/// matter how much geometry is in it.
pub fn luminance_spread() -> f32 {
    let mut low = f32::MAX;
    let mut high = f32::MIN;
    for ramp in 0..RAMPS {
        for step in 0..RAMP_STEPS {
            let luma = luminance(ramp, step);
            low = low.min(luma);
            high = high.max(luma);
        }
    }
    high - low
}

/// Fraction of ramp steps brighter than the step below them.
///
/// Exactly 1.0 or the palette has a kink in it, which shows up as a blade going
/// *darker* toward its tip in one band and is remarkably hard to spot by eye.
pub fn ramp_monotonicity() -> f32 {
    let mut good = 0;
    let mut total = 0;
    for ramp in 0..RAMPS {
        for step in 1..RAMP_STEPS {
            total += 1;
            if luminance(ramp, step) > luminance(ramp, step - 1) {
                good += 1;
            }
        }
    }
    good as f32 / total as f32
}

/// Mean gap between neighbouring steps over the largest gap, worst ramp.
///
/// Near 1.0 means evenly spaced. A ramp with one huge jump in it posterises:
/// everything lands either side of the jump and the steps in between never get
/// used.
pub fn ramp_evenness() -> f32 {
    let mut worst = 1.0f32;
    for ramp in 0..RAMPS {
        let gaps: Vec<f32> = (1..RAMP_STEPS)
            .map(|step| luminance(ramp, step) - luminance(ramp, step - 1))
            .collect();
        let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
        let largest = gaps.iter().cloned().fold(f32::MIN, f32::max);
        if largest > 0.0 {
            worst = worst.min(mean / largest);
        }
    }
    worst
}

/// Mean saturation across the palette, in 0..1.
///
/// Guards the call [`SATURATION`] exists to make. A physically summed rig
/// drifts grey, and grey grass is the failure this whole module is arranged to
/// avoid.
pub fn saturation() -> f32 {
    let mut total = 0.0;
    for ramp in 0..RAMPS {
        for step in 0..RAMP_STEPS {
            let [r, g, b] = channels(ramp, step);
            let high = r.max(g).max(b) as f32;
            let low = r.min(g).min(b) as f32;
            if high > 0.0 {
                total += (high - low) / high;
            }
        }
    }
    total / PALETTE_SIZE as f32
}

/// Warmth of the sunlit ramp over the shaded one, in 0..1.
///
/// The rig's whole purpose, reduced to one number: if a golden key and a blue
/// fill are doing their job, blades in the sun are measurably warmer than
/// blades in shade. If this collapses toward zero the lighting has gone flat,
/// whatever the sun directions still say.
pub fn key_warmth() -> f32 {
    let warmth = |ramp: usize| {
        let [r, _, b] = channels(ramp, RAMP_STEPS - 1);
        (r as f32 - b as f32) / 255.0
    };
    warmth(HIGHLIGHT) - warmth(SHADOW)
}

/// The sRGB transfer function, exactly as the GPU applies it.
fn srgb_to_linear(value: u8) -> f32 {
    let v = value as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p bw_grass -- --ignored --nocapture show_the_palette`
    ///
    /// Not an assertion. The palette is computed, so this is how a person looks
    /// at what the rig actually produced before deciding whether to turn
    /// [`EXPOSURE`] or [`SATURATION`].
    #[test]
    #[ignore = "prints the baked palette for inspection"]
    fn show_the_palette() {
        for (index, name) in ["shadow", "body", "highlight", "dry"].iter().enumerate() {
            let entries: Vec<String> = (0..RAMP_STEPS)
                .map(|step| format!("#{:06x}", hex(index, step)))
                .collect();
            println!("{name:>9}  {}", entries.join(" "));
        }
        println!();
        println!("spread     {:.3}", luminance_spread());
        println!("saturation {:.3}", saturation());
        println!("evenness   {:.3}", ramp_evenness());
        println!("key warmth {:.3}", key_warmth());
    }

    #[test]
    fn every_ramp_gets_brighter() {
        assert_eq!(ramp_monotonicity(), 1.0, "{:?}", *PALETTE);
    }

    #[test]
    fn the_palette_spans_a_usable_range() {
        // Too narrow and the grass is one flat tone; too wide and the darkest
        // entry is pure black, which reads as a hole rather than as shade.
        let spread = luminance_spread();
        assert!((0.30..0.85).contains(&spread), "spread {spread}");
        let [r, g, b] = darkest();
        assert!(
            r as u32 + g as u32 + b as u32 > 12,
            "the darkest entry is basically black: {r},{g},{b}"
        );
    }

    #[test]
    fn ramps_are_evenly_spaced() {
        // The threshold depends on the step count and is not a universal
        // constant: this metric is the mean gap over the largest gap, and with
        // more steps there is more opportunity for one gap to run ahead of the
        // mean. It was 0.45 when a ramp had six steps and cannot be met at
        // sixteen. What it still catches is a ramp with a *cliff* in it, where
        // one jump swallows the range and the steps either side go unused.
        let evenness = ramp_evenness();
        assert!(evenness > 0.30, "posterised ramp: evenness {evenness}");
    }

    #[test]
    fn the_grass_ramps_are_green() {
        // DRY is allowed to be straw; the other three must not drift grey.
        for ramp in [SHADOW, BODY, HIGHLIGHT] {
            for step in 0..RAMP_STEPS {
                let [r, g, b] = channels(ramp, step);
                assert!(
                    g > r && g > b,
                    "ramp {ramp} step {step} is not green: {r},{g},{b}"
                );
            }
        }
    }

    #[test]
    fn the_living_ramps_have_not_drifted_grey() {
        // Measured on the three living ramps only. DRY is *meant* to be dull —
        // including it here would let the greens wash out as long as the straw
        // stayed straw.
        for ramp in [SHADOW, BODY, HIGHLIGHT] {
            let mut total = 0.0;
            for step in 0..RAMP_STEPS {
                let [r, g, b] = channels(ramp, step);
                let high = r.max(g).max(b) as f32;
                let low = r.min(g).min(b) as f32;
                if high > 0.0 {
                    total += (high - low) / high;
                }
            }
            let mean = total / RAMP_STEPS as f32;
            assert!(mean > 0.45, "ramp {ramp} washed out: saturation {mean}");
        }
    }

    #[test]
    fn the_lit_end_of_every_ramp_keeps_all_three_channels() {
        // A channel pinned at zero can no longer carry information, so a ramp
        // that crushes one flattens at that end without the spread metric
        // noticing.
        //
        // Checked on the lit half only. Down in the dark the blue channel
        // genuinely does reach zero, and that is correct rather than a defect:
        // the art target averages a blue of 10 against a green of 93, and a
        // shaded leaf reflects almost none. Requiring blue everywhere is what
        // produced the washed-out sage green this palette used to be.
        for ramp in 0..RAMPS {
            for step in RAMP_STEPS / 2..RAMP_STEPS {
                let [r, g, b] = channels(ramp, step);
                assert!(
                    r > 0 && g > 0 && b > 0,
                    "ramp {ramp} step {step} has a crushed channel: {r},{g},{b}"
                );
            }
        }
    }

    #[test]
    fn sunlit_grass_is_warmer_than_shaded_grass() {
        // The rig, visible. A golden key and a blue fill that did not produce
        // this would be a rig in name only.
        let warmth = key_warmth();
        assert!(warmth > 0.02, "the lighting has gone flat: warmth {warmth}");
    }

    #[test]
    fn the_shadow_ramp_is_darker_than_the_highlight_ramp() {
        for step in 0..RAMP_STEPS {
            assert!(
                luminance(SHADOW, step) < luminance(HIGHLIGHT, step),
                "step {step}"
            );
        }
    }

    #[test]
    fn dry_grass_is_duller_than_living_grass() {
        let saturation = |ramp: usize| {
            let [r, g, b] = channels(ramp, RAMP_STEPS - 1);
            g as i32 - r.min(b) as i32
        };
        assert!(saturation(DRY) < saturation(BODY));
    }

    #[test]
    fn the_bake_is_deterministic() {
        // It feeds a committed benchmark baseline, so it has to be the same
        // palette on every machine and every run.
        assert_eq!(bake(), bake());
        assert_eq!(bake(), *PALETTE);
    }

    #[test]
    fn a_round_trip_through_linear_returns_the_stored_entry() {
        // The shader is handed linear values and the render target converts
        // back on write. If that round trip were lossy the palette on screen
        // would not be the palette that was baked, and every measurement taken
        // against it would be measuring the wrong colours.
        for ramp in 0..RAMPS {
            for step in 0..RAMP_STEPS {
                let stored = channels(ramp, step);
                let linear = entry(ramp, step);
                let back = [
                    (linear_to_srgb(linear.x) * 255.0).round() as u8,
                    (linear_to_srgb(linear.y) * 255.0).round() as u8,
                    (linear_to_srgb(linear.z) * 255.0).round() as u8,
                ];
                assert_eq!(stored, back, "ramp {ramp} step {step}");
            }
        }
    }

    #[test]
    fn flattening_matches_the_shaders_indexing() {
        let flat = flattened();
        assert_eq!(flat.len(), PALETTE_SIZE);
        for ramp in 0..RAMPS {
            for step in 0..RAMP_STEPS {
                assert_eq!(flat[ramp * RAMP_STEPS + step], entry(ramp, step));
            }
        }
    }

    #[test]
    fn no_entry_clips() {
        // A clipped channel means the exposure is too high and the top of the
        // ramp has stopped carrying information — two steps that look
        // different in the bake come out identical on screen.
        for ramp in 0..RAMPS {
            for step in 0..RAMP_STEPS {
                let [r, g, b] = channels(ramp, step);
                assert!(
                    r < 255 && g < 255 && b < 255,
                    "ramp {ramp} step {step} clips: {r},{g},{b}"
                );
            }
        }
    }

    /// *Both* shaders index this palette by hand, so both have to agree on its
    /// shape.
    ///
    /// A mismatch does not fail to compile. It reads the wrong ramp, and the
    /// grass comes out a plausible but wrong colour.
    ///
    /// The ground shader is checked here too, and that is the whole reason this
    /// test takes a list of files rather than one path. When the ground was
    /// added it copied these six constants and nothing guarded them — changing
    /// `RAMP_STEPS` would have moved every ramp under the blades while leaving
    /// the ground reading from the old offsets, which is exactly the silent,
    /// plausible wrongness this test exists to prevent. Any future shader that
    /// touches the palette belongs in this list.
    #[test]
    fn every_shader_agrees_on_the_palette_shape() {
        let shaders = [
            (
                "grass.wgsl",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../assets/shaders/grass.wgsl"
                ),
            ),
            (
                "ground.wgsl",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../assets/shaders/ground.wgsl"
                ),
            ),
        ];

        for (name, path) in shaders {
            let source =
                std::fs::read_to_string(path).unwrap_or_else(|_| panic!("{name} must exist"));
            for (constant, value) in [
                ("RAMPS", RAMPS),
                ("RAMP_STEPS", RAMP_STEPS),
                ("PALETTE_SIZE", PALETTE_SIZE),
                ("RAMP_SHADOW", SHADOW),
                ("RAMP_BODY", BODY),
                ("RAMP_DRY", DRY),
            ] {
                let needle = format!("const {constant}: i32 = {value};");
                assert!(
                    source.contains(&needle),
                    "{name} must declare `{needle}` to stay in step with palette.rs"
                );
            }
        }

        // Only the blade shader picks the highlight ramp; the ground never does.
        let grass = std::fs::read_to_string(shaders[0].1).expect("the grass shader must exist");
        let needle = format!("const RAMP_HIGHLIGHT: i32 = {HIGHLIGHT};");
        assert!(
            grass.contains(&needle),
            "grass.wgsl must declare `{needle}`"
        );
    }

    /// The palette is uploaded as a fixed-size WGSL array, and the size is a
    /// literal in each shader because WGSL cannot size an array from an
    /// imported constant.
    ///
    /// So `PALETTE_SIZE` matching is necessary but not sufficient — the array
    /// declaration itself has to match too, and it is the one place a stale
    /// number silently truncates or over-reads the uniform.
    #[test]
    fn every_shader_sizes_the_palette_array_correctly() {
        for path in [
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/shaders/grass.wgsl"
            ),
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/shaders/ground.wgsl"
            ),
        ] {
            let source = std::fs::read_to_string(path).expect("shader must exist");
            let needle = format!("palette: array<vec4<f32>, {PALETTE_SIZE}>,");
            assert!(source.contains(&needle), "{path} must declare `{needle}`");
        }
    }
}

//! The colours grass is allowed to be.
//!
//! Pixel art is defined by its palette more than by its resolution. A low-res
//! buffer with continuous shading in it is just a small photograph; what makes
//! an image read as *drawn* is that every pixel is one of a handful of colours
//! someone chose. So shading never produces a colour. It produces a brightness,
//! and that brightness selects an entry from this table.
//!
//! ## The palette is baked from the lighting rig, and fitted to the art target
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
//! What the rig does not settle is *which* greens. A physically summed grass
//! palette is a defensible object and not the one this game wants — it comes out
//! desaturated and blue-shifted, because two of the three suns and the entire
//! sky are blue. So the free numbers in the bake — albedo, exposure, saturation
//! and where each ramp's strand points — are **fitted** to [`TARGET`], the
//! palette pulled off the reference artwork, by `fit_to_the_target`. The rig
//! decides the shape of the ramps; the target decides where they land.
//!
//! That fit is measured, not asserted: [`chroma_error`] scores every living
//! entry against the target's hue at its own brightness, [`living_range`]
//! against [`target_range`] checks the palette reaches both ends, and
//! [`blue_to_green`] watches the one axis the rig pushes hardest the wrong way.
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
//! | [`SHADOW`] | nearly along the key, catching little of it | Cool, dark. Deep canopy |
//! | [`BODY`] | oblique to the key | The bulk of the field |
//! | [`HIGHLIGHT`] | close to across the key, catching most of it | Warm. Blades in the sun |
//! | [`DRY`] | oblique, straw albedo | Trampled grass |
//!
//! Only [`DRY`] is a different material. The other three are the same leaf at
//! three orientations, which is why they read as one plant — and it is also why
//! they can all sit on the target's single ramp while still meaning different
//! things to the shader. What separates them is where they sit *along* it:
//! [`SHADOW`] holds the target's darkest greens, [`HIGHLIGHT`] its lightest.
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

/// The art target: the palette this bake is fitted to, darkest first.
///
/// Ten colours pulled off the reference artwork by perceptual clustering, each
/// with the share of the image it covers. Both halves matter and they are used
/// in different places:
///
/// - **The colours** are what [`bake`] is fitted to. They constrain hue and
///   chroma at every brightness — see [`chroma_error`].
/// - **The shares** are what the *renderer* is fitted to. A palette can be
///   perfect and the image still wrong, because how much of each tone appears
///   is decided by the clump bake's shading and by the ground's tonal wash, not
///   by the palette at all. [`crate::clump::Atlas::tone_shares`] measures the
///   baked sprites against this column.
///
/// Three things about the target are worth stating, because each one contradicts
/// what a physically summed lighting rig produces on its own:
///
/// - **It is one hue family, not four.** Every colour is a yellow-green between
///   72° and 91°. There is no separate straw or blue-green material in it.
/// - **Hue warms as it brightens.** Red-to-green climbs steadily from 0.54 at
///   the darkest to 0.82 at the lightest, so the ramp travels from a cool
///   grass-green to a warm lime rather than just getting paler.
/// - **Blue is almost gone and almost constant.** Every entry sits between 7 and
///   11, against greens from 102 to 222 — a blue-to-green ratio of 0.05 in the
///   middle of the ramp. The rig's fill and sky are both blue, so reaching this
///   is most of what the fit has to fight.
///
/// The distribution is the other half of the style. It peaks low — the three
/// darkest greens are 39% of the image between them — and thins steadily
/// upward, with the brightest highlight at 2%. That is what makes the reference
/// read as grass in even light with sun catching a few tips, rather than as a
/// field lit from directly overhead.
pub const TARGET_TONES: usize = 10;

/// See [`TARGET_TONES`] — the art target, as colour and share.
pub const TARGET: [([u8; 3], f32); TARGET_TONES] = [
    ([55, 102, 11], 0.092),  // deep shadow
    ([64, 114, 10], 0.150),  // forest green
    ([74, 124, 9], 0.147),   // dark grass
    ([84, 133, 8], 0.132),   // mid-dark green
    ([93, 143, 8], 0.123),   // base green
    ([104, 153, 8], 0.118),  // fresh green
    ([117, 166, 7], 0.098),  // bright grass
    ([132, 180, 8], 0.074),  // light green
    ([152, 197, 9], 0.046),  // lime highlight
    ([181, 222, 11], 0.021), // brightest highlight
];

/// The numbers the bake is fitted with.
///
/// Gathered into a struct rather than left as loose constants because they are
/// **fitted, not chosen** — `fit_to_the_target` searches them against [`TARGET`]
/// and prints the result, and a search needs somewhere to put a candidate. The
/// committed values are the output of that search, and re-running it is how a
/// new art target gets turned into a palette.
#[derive(Clone, Copy, Debug)]
struct BakeParams {
    /// Grass albedo per ramp, in linear light.
    ///
    /// Dark and strongly green-dominant. Foliage albedo really is this low — a
    /// leaf reflects around a tenth of the red and blue landing on it — and a
    /// bright albedo is the classic way to end up with pale sage grass no amount
    /// of palette tuning can rescue, because the ambient's red and blue get
    /// multiplied up along with the green.
    ///
    /// Blue is the channel that matters most and it is nearly zero. The target
    /// runs a blue-to-green ratio of 0.05, and since the rig's fill and sky are
    /// both blue, anything the albedo offers them comes straight back.
    albedo: [Vec3; RAMPS],
    /// Where each ramp's strand points, from along the key (0) to across it (1).
    ///
    /// This is what separates the ramps: a strand along the key catches none of
    /// it and comes out cool and dark, one across it catches all of it and comes
    /// out warm and bright. Sweeping this is how the target's *hue warms as it
    /// brightens* is reproduced without hand-authoring three different greens.
    ///
    /// None of the living ramps sits at zero. A strand exactly along the key is
    /// lit by fill and sky alone — both blue, and together far dimmer than the
    /// target's floor of 102 green. The old bake did put `SHADOW` there and its
    /// darkest entry came out `#142411`: a third of the target's darkest, and
    /// blue-to-green of 0.47 against the target's 0.11.
    blend: [f32; RAMPS],
    /// Exposure applied to the whole bake.
    ///
    /// Scaling the rig rather than the entries keeps every ratio the character
    /// sprites were lit at. This is the one number to turn if the field as a
    /// whole is too bright or too dark.
    exposure: f32,
    /// Saturation applied after the bake.
    ///
    /// Above one, which is a deliberate departure from physical summation. The
    /// rig's fill and sky are broad-spectrum, so a summed result drifts toward
    /// grey — correct, and not what stylised game art looks like. The target is
    /// a strongly saturated chartreuse; reaching it is the same call as
    /// rendering the character sprites through Blender's Standard view transform
    /// instead of AgX, and for the same reason.
    saturation: f32,
    /// Smallest a channel may fall to, as a fraction of the colour's own grey.
    ///
    /// It binds on exactly one channel — blue — and its job here is to let blue
    /// fall to the near-nothing the target has without letting it reach zero,
    /// which would flatten the ramp's hue at whichever end it happened first.
    channel_floor: f32,
    /// How far a rim highlight washes toward the light's own colour.
    ///
    /// Foliage catching a hard backlight goes pale, not saturated green — but
    /// only a little. The rim is the coolest light in the rig, so every unit of
    /// wash is blue arriving in the palette by the back door.
    rim_wash: f32,
}

/// The committed fit. See [`BakeParams`].
const FITTED: BakeParams = BakeParams {
    albedo: [
        Vec3::new(0.119, 0.223, 0.001), // shadow: the cool, green end of the target
        Vec3::new(0.095, 0.169, 0.021), // body
        Vec3::new(0.157, 0.259, 0.002), // highlight: the warm, lime end
        Vec3::new(0.197, 0.170, 0.078), // dry: straw, and not fitted — see below
    ],
    blend: [0.48, 0.75, 0.85, 0.75],
    exposure: 0.380,
    saturation: 1.235,
    channel_floor: 0.007,
    rim_wash: 0.060,
};

/// Sky reaching the deepest step of a ramp. Shared with the shader.
const OCCLUSION_FLOOR: f32 = light::CANOPY_FLOOR;

/// The baked palette, `[ramp][step]` with the darkest step first.
static PALETTE: LazyLock<[[[u8; 3]; RAMP_STEPS]; RAMPS]> = LazyLock::new(bake);

/// Strand orientation a ramp is baked at.
///
/// Expressed against the key so the ramps stay meaningful if the rig moves.
fn ramp_tangent(ramp: usize, params: &BakeParams) -> Vec3 {
    let key = light::key().direction;
    // A ground-plane vector square on to the key, which a blade leaning across
    // the sun presents.
    let across = Vec3::new(-key.y, key.x, 0.0).normalize();
    key.lerp(across, params.blend[ramp]).normalize()
}

fn bake() -> [[[u8; 3]; RAMP_STEPS]; RAMPS] {
    bake_with(&FITTED)
}

fn bake_with(params: &BakeParams) -> [[[u8; 3]; RAMP_STEPS]; RAMPS] {
    let mut out = [[[0u8; 3]; RAMP_STEPS]; RAMPS];
    let key = light::key();
    let fill = light::fill();
    let rim = light::rim();

    for (ramp, steps) in out.iter_mut().enumerate() {
        let tangent = ramp_tangent(ramp, params);
        let albedo = params.albedo[ramp];
        let rim_albedo = albedo.lerp(Vec3::ONE, params.rim_wash);

        for (step, entry) in steps.iter_mut().enumerate() {
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

            *entry = quantise(saturate(
                linear * params.exposure,
                params.saturation,
                params.channel_floor,
            ));
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
fn saturate(colour: Vec3, amount: f32, floor: f32) -> Vec3 {
    let grey = 0.2126 * colour.x + 0.7152 * colour.y + 0.0722 * colour.z;
    let pushed = Vec3::splat(grey) + (colour - Vec3::splat(grey)) * amount;
    pushed.max(Vec3::splat(grey * floor))
}

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

/// Perceived brightness of a [`TARGET`] entry, on the same scale as
/// [`luminance`].
pub fn target_luminance(index: usize) -> f32 {
    let [r, g, b] = TARGET[index].0;
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

/// The art target's own luminance range, darkest to lightest.
pub fn target_range() -> (f32, f32) {
    (target_luminance(0), target_luminance(TARGET.len() - 1))
}

/// The colour the art target holds at a given brightness, in sRGB 0..1.
///
/// The target is ten samples of a continuous ramp, so this walks it as a curve
/// rather than snapping to the nearest of ten. Outside its range it holds the
/// end colour: what is being asked is "what hue does the target use at this
/// brightness", and beyond the ends there is no answer but the nearest one.
pub fn target_at(luma: f32) -> Vec3 {
    let colour = |index: usize| {
        let [r, g, b] = TARGET[index].0;
        Vec3::new(r as f32, g as f32, b as f32) / 255.0
    };
    if luma <= target_luminance(0) {
        return colour(0);
    }
    for index in 1..TARGET.len() {
        let high = target_luminance(index);
        if luma <= high {
            let low = target_luminance(index - 1);
            let t = (luma - low) / (high - low).max(1e-6);
            return colour(index - 1).lerp(colour(index), t);
        }
    }
    colour(TARGET.len() - 1)
}

/// How far the living ramps sit from the art target's hue, in sRGB 0..1.
///
/// Compared **at matched brightness**, which is the whole reason this is not a
/// nearest-colour distance. Brightness is not the palette's to get right: the
/// shader picks a step per fragment, so which entries appear and how often is
/// decided by [`crate::clump`] and the ground wash. What the palette owns is the
/// hue and chroma at each brightness, and that is what this measures.
///
/// [`DRY`] is excluded. The target has no straw in it at all — it is one hue
/// family from end to end — so scoring the straw ramp against it would only
/// report that straw is not grass.
pub fn chroma_error() -> f32 {
    let mut total = 0.0;
    let mut count = 0;
    for ramp in [SHADOW, BODY, HIGHLIGHT] {
        for step in 0..RAMP_STEPS {
            let [r, g, b] = channels(ramp, step);
            let ours = Vec3::new(r as f32, g as f32, b as f32) / 255.0;
            total += (ours - target_at(luminance(ramp, step))).length();
            count += 1;
        }
    }
    total / count.max(1) as f32
}

/// Darkest and lightest of the living ramps.
///
/// Read against [`target_range`]. A palette narrower than the target cannot
/// reach its darks or its highlights however the renderer distributes them; one
/// much wider spends steps on tones the reference never uses.
pub fn living_range() -> (f32, f32) {
    let mut low = f32::MAX;
    let mut high = f32::MIN;
    for ramp in [SHADOW, BODY, HIGHLIGHT] {
        for step in 0..RAMP_STEPS {
            let luma = luminance(ramp, step);
            low = low.min(luma);
            high = high.max(luma);
        }
    }
    (low, high)
}

/// Which of the art target's ten tones a brightness belongs to.
///
/// Split at the midpoints between neighbouring target luminances, so a tone's
/// bucket is everything closer to it than to either neighbour. This is what lets
/// a rendered image be scored against the target's *share* column rather than
/// only its colours — see [`TARGET`] and
/// [`crate::clump::Atlas::tone_shares`].
pub fn target_tone(luma: f32) -> usize {
    for index in 1..TARGET_TONES {
        let boundary = (target_luminance(index - 1) + target_luminance(index)) * 0.5;
        if luma < boundary {
            return index - 1;
        }
    }
    TARGET_TONES - 1
}

/// How far a measured tone distribution sits from the art target's, in 0..1.
///
/// Total variation: half the sum of the absolute differences, so zero is an
/// exact match and one is no overlap at all. Half the sum rather than the whole
/// of it because every unit of share that leaves one bucket arrives in another,
/// and counting both ends doubles the same disagreement.
pub fn tone_divergence(shares: &[f32; TARGET_TONES]) -> f32 {
    let mut total = 0.0;
    for (index, share) in shares.iter().enumerate() {
        total += (share - TARGET[index].1).abs();
    }
    total * 0.5
}

/// Mean blue-to-green ratio of the living ramps, on the stored sRGB values.
///
/// Called out on its own because it is the single number the rig fights hardest.
/// Two of the three suns and the whole sky are blue, so a physically summed
/// grass palette lands around 0.4 here; the art target runs 0.05.
pub fn blue_to_green() -> f32 {
    let mut total = 0.0;
    let mut count = 0;
    for ramp in [SHADOW, BODY, HIGHLIGHT] {
        for step in 0..RAMP_STEPS {
            let [_, g, b] = channels(ramp, step);
            if g > 0 {
                total += b as f32 / g as f32;
                count += 1;
            }
        }
    }
    total / count.max(1) as f32
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
/// Guards the call `BakeParams::saturation` exists to make. A physically summed
/// rig drifts grey, and grey grass is the failure this whole module is arranged
/// to avoid.
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
    /// at what the rig actually produced before deciding whether to re-run
    /// [`fit_to_the_target`].
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
        let target: Vec<String> = TARGET
            .iter()
            .map(|([r, g, b], _)| format!("#{r:02x}{g:02x}{b:02x}"))
            .collect();
        println!("   target  {}", target.join(" "));
        println!();
        let (low, high) = living_range();
        let (target_low, target_high) = target_range();
        println!("spread       {:.3}", luminance_spread());
        println!("saturation   {:.3}", saturation());
        println!("evenness     {:.3}", ramp_evenness());
        println!("key warmth   {:.3}", key_warmth());
        println!("chroma error {:.4}", chroma_error());
        println!("blue/green   {:.3}  (target 0.052)", blue_to_green());
        println!("living range {low:.3}..{high:.3}  (target {target_low:.3}..{target_high:.3})");
    }

    /// `cargo test -p bw_grass -- --ignored --nocapture score_the_capture`
    ///
    /// Scores a rendered frame against the art target's share column, beside the
    /// reference plate scored the same way. Not an assertion — it needs a GPU
    /// and a capture that may not be there.
    ///
    /// This is the measurement that actually matters, and it is not the one
    /// `clump::Atlas::tone_shares` makes. A sprite's own tone distribution is
    /// not what reaches the screen: the depth test hands each pixel to whichever
    /// clump is *highest* there, so a dense field shows its upper envelope —
    /// tips and outer leaves — and hides the shaded interiors that make up most
    /// of the atlas. The two numbers can disagree by a wide margin and both be
    /// right about what they measure.
    #[test]
    #[ignore = "scores a rendered frame against the art target"]
    fn score_the_capture() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let shares_of = |path: &str| {
            let image = image::open(path).ok()?.to_rgb8();
            let mut counts = [0.0f32; TARGET_TONES];
            let mut total = 0.0f32;
            for pixel in image.pixels() {
                let luma = (0.2126 * pixel[0] as f32
                    + 0.7152 * pixel[1] as f32
                    + 0.0722 * pixel[2] as f32)
                    / 255.0;
                counts[target_tone(luma)] += 1.0;
                total += 1.0;
            }
            for count in &mut counts {
                *count /= total.max(1.0);
            }
            Some(counts)
        };

        let ours = shares_of(&format!("{root}/benchmarks/capture/grass.png"));
        let plate = shares_of(&format!(
            "{root}/benchmarks/reference/pixel_grass_target.png"
        ));
        let Some(ours) = ours else {
            println!("no capture at benchmarks/capture/grass.png");
            return;
        };

        println!("      tone     ours    plate   target");
        for (index, ([r, g, b], target)) in TARGET.iter().enumerate() {
            let plate = plate.map_or(f32::NAN, |p| p[index] * 100.0);
            println!(
                "  #{r:02x}{g:02x}{b:02x}   {:6.1}% {plate:6.1}% {:6.1}%",
                ours[index] * 100.0,
                target * 100.0
            );
        }
        println!("  divergence {:.3}", tone_divergence(&ours));
        if let Some(plate) = plate {
            println!("  plate      {:.3}", tone_divergence(&plate));
        }
    }

    /// `cargo test -p bw_grass -- --ignored --nocapture fit_to_the_target`
    ///
    /// Searches [`BakeParams`] against [`TARGET`] and prints the result, which
    /// is then pasted into [`FITTED`]. This is how the committed numbers were
    /// arrived at, and re-running it is how a new art target becomes a palette.
    ///
    /// Coordinate descent rather than anything cleverer, because the surface is
    /// smooth in every knob and the search takes under a second. It starts from
    /// the committed fit, so running it after an art change refines rather than
    /// restarts — which matters, since the loss has a shallow basin in `blend`
    /// and a cold start can settle in a different one.
    #[test]
    #[ignore = "searches the bake parameters against the art target"]
    fn fit_to_the_target() {
        // Albedo for the three living ramps, their blends, then the four global
        // knobs. `DRY` is not fitted: the target has no straw in it.
        let read = |p: &BakeParams| {
            let mut v = Vec::new();
            for ramp in [SHADOW, BODY, HIGHLIGHT] {
                v.extend_from_slice(&p.albedo[ramp].to_array());
            }
            v.extend_from_slice(&[p.blend[SHADOW], p.blend[BODY], p.blend[HIGHLIGHT]]);
            v.extend_from_slice(&[p.exposure, p.saturation, p.channel_floor, p.rim_wash]);
            v
        };
        let write = |v: &[f32]| {
            let mut p = FITTED;
            for (slot, ramp) in [SHADOW, BODY, HIGHLIGHT].into_iter().enumerate() {
                p.albedo[ramp] = Vec3::new(v[slot * 3], v[slot * 3 + 1], v[slot * 3 + 2]);
                p.blend[ramp] = v[9 + slot];
            }
            // The straw ramp follows the body's lighting so it stays the same
            // plant in the same sun, and only its albedo makes it straw.
            p.blend[DRY] = p.blend[BODY];
            p.exposure = v[12];
            p.saturation = v[13];
            p.channel_floor = v[14];
            p.rim_wash = v[15];
            p
        };
        let bounds = [
            (0.0005f32, 0.40f32), // albedo, nine of them
            (0.0, 1.0),           // blend, three
            (0.05, 1.50),         // exposure
            (0.50, 2.00),         // saturation
            (0.001, 0.20),        // channel floor
            (0.00, 0.40),         // rim wash
        ];
        let bound = |index: usize| match index {
            0..=8 => bounds[0],
            9..=11 => bounds[1],
            12 => bounds[2],
            13 => bounds[3],
            14 => bounds[4],
            _ => bounds[5],
        };
        let scale = |index: usize| match index {
            0..=8 => 0.02,
            9..=11 => 0.06,
            12 => 0.02,
            13 => 0.05,
            14 => 0.004,
            _ => 0.02,
        };

        let loss = |v: &[f32]| {
            let table = bake_with(&write(v));
            let luma = |c: [u8; 3]| {
                (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
            };
            let (mut chroma, mut count) = (0.0f32, 0usize);
            let mut mean = [0.0f32; RAMPS];
            for ramp in [SHADOW, BODY, HIGHLIGHT] {
                for step in 0..RAMP_STEPS {
                    let c = table[ramp][step];
                    let l = luma(c);
                    mean[ramp] += l / RAMP_STEPS as f32;
                    let ours = Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) / 255.0;
                    chroma += (ours - target_at(l)).length();
                    count += 1;
                }
            }
            // A kink is a ramp going darker as it climbs. Not a preference the
            // fit trades against hue — a blade that darkens toward its tip is
            // simply broken — so it is priced out of reach.
            let mut kinks = 0.0;
            for ramp in 0..RAMPS {
                for step in 1..RAMP_STEPS {
                    if luma(table[ramp][step]) <= luma(table[ramp][step - 1]) {
                        kinks += 1.0;
                    }
                }
            }

            // The ramps have to stay in the order their names promise. Left to
            // itself the fit does not keep them there: the target is a single
            // curve, so laying all three ramps on top of each other scores
            // perfectly while making the ramp choice carry no information at
            // all. `BODY` came out brighter than `HIGHLIGHT` on the first run —
            // a good number and an inverted image, since the shader picks
            // `HIGHLIGHT` for exactly the blades that are catching the sun.
            let ordering = (mean[SHADOW] - mean[BODY] + 0.06).max(0.0)
                + (mean[BODY] - mean[HIGHLIGHT] + 0.06).max(0.0);

            // Coverage is measured at the ends that mean something rather than
            // over the union: the deep canopy has to reach the target's darkest
            // green and sunlit tips its brightest, and it is no use if some
            // other ramp got there instead.
            let (target_low, target_high) = target_range();
            let reach = (luma(table[SHADOW][0]) - target_low).abs()
                + (luma(table[HIGHLIGHT][RAMP_STEPS - 1]) - target_high).abs();

            chroma / count as f32 + 0.5 * reach + 2.0 * ordering + kinks
        };

        let mut best = read(&FITTED);
        let mut best_loss = loss(&best);
        let start = best_loss;
        let mut step = 1.0f32;
        while step > 0.02 {
            let mut improved = false;
            for index in 0..best.len() {
                for direction in [1.0f32, -1.0] {
                    let (low, high) = bound(index);
                    let mut candidate = best.clone();
                    candidate[index] =
                        (candidate[index] + direction * step * scale(index)).clamp(low, high);
                    let value = loss(&candidate);
                    if value < best_loss - 1e-6 {
                        best = candidate;
                        best_loss = value;
                        improved = true;
                    }
                }
            }
            if !improved {
                step *= 0.5;
            }
        }

        let fitted = write(&best);
        println!("loss {start:.4} -> {best_loss:.4}");
        println!("const FITTED: BakeParams = BakeParams {{");
        println!("    albedo: [");
        for ramp in 0..RAMPS {
            let a = fitted.albedo[ramp];
            println!("        Vec3::new({:.3}, {:.3}, {:.3}),", a.x, a.y, a.z);
        }
        println!("    ],");
        println!(
            "    blend: [{:.2}, {:.2}, {:.2}, {:.2}],",
            fitted.blend[0], fitted.blend[1], fitted.blend[2], fitted.blend[3]
        );
        println!("    exposure: {:.3},", fitted.exposure);
        println!("    saturation: {:.3},", fitted.saturation);
        println!("    channel_floor: {:.3},", fitted.channel_floor);
        println!("    rim_wash: {:.3},", fitted.rim_wash);
        println!("}};");
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
            // The art target is a strongly saturated chartreuse — every one of
            // its ten colours sits between 0.89 and 0.95 — so there is a lot of
            // headroom under this and anything much below it is not the same
            // palette any more.
            assert!(mean > 0.88, "ramp {ramp} washed out: saturation {mean}");
        }
    }

    #[test]
    fn the_palette_still_matches_the_art_target() {
        // The whole point of fitting rather than eyeballing: the fit is only
        // worth having if something notices when it stops holding. Anything in
        // the rig moves this — a sun's energy, the canopy floor, the rim's
        // strand correction — and all of them are legitimate changes that
        // simply need the fit re-run afterwards.
        //
        // The tolerance is loose against the fitted 0.028, because this is a
        // guard against drift rather than a re-statement of the fit. At 0.06 the
        // mean entry is fifteen levels from the target hue at its own
        // brightness, which is where a person starts to see it.
        let error = chroma_error();
        assert!(error < 0.06, "the palette has drifted off target: {error}");
    }

    #[test]
    fn the_palette_reaches_both_ends_of_the_art_target() {
        // Hue can be right at every step and the image still wrong, because a
        // palette that stops short of the target's darks has no deep canopy to
        // put between the clumps and one that stops short of its lights has no
        // sunlit tips. Neither shows up in `chroma_error`, which only ever asks
        // about the entries that do exist.
        let (low, high) = living_range();
        let (target_low, target_high) = target_range();
        assert!(
            (low - target_low).abs() < 0.04,
            "the darks stop at {low}, the target reaches {target_low}"
        );
        assert!(
            (high - target_high).abs() < 0.04,
            "the lights stop at {high}, the target reaches {target_high}"
        );
    }

    #[test]
    fn blue_is_all_but_absent() {
        // Called out separately from `chroma_error` because it is the one axis
        // the lighting rig actively pushes the wrong way, and because it is
        // recoverable-looking when it goes wrong: too much blue reads as
        // plausible sage-green grass rather than as a bug.
        let ratio = blue_to_green();
        assert!(ratio < 0.10, "the palette has gone blue-green: {ratio}");
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

//! The lighting rig.
//!
//! One rig lights everything in this game: the grass here, and the character
//! and prop renders that are baked to sprites out of Blender. Two rigs would be
//! immediately visible — a unit standing in grass lit from a different sun does
//! not read as being *in* the grass, it reads as a cut-out placed on top of it.
//! So the rig lives here as data, and both ends refer to it.
//!
//! Three suns, no area lamps. A sun is directional and infinitely far away, so
//! a knee-high blade and a four-metre siege engine take the same energy with no
//! per-object tuning. Area lamps were the first attempt on the character rig
//! and read flat: at working distance their falloff compressed key-to-fill
//! toward 1:1, and a 1:1 key-to-fill is the definition of flat.
//!
//! | Sun | Elevation | Colour | Energy | Role |
//! |---|---|---|---|---|
//! | [`key`] | 38° | golden `1.00, 0.91, 0.72` | 5.6 | Carves the form. The only shadow caster |
//! | [`fill`] | 14° | saturated blue `0.42, 0.62, 1.00` | 0.73 | Keeps the shadow side coloured, not muddy |
//! | [`rim`] | 55° | cool `0.80, 0.89, 1.00` | 4.5 | Lifts the silhouette off the background |
//!
//! A key:fill ratio of about 7.7:1. The fill is kept dim deliberately — the sky
//! already does most of that job, and every extra unit of fill lands on the
//! unlit side and erases the contrast the key just made. It is saturated rather
//! than neutral grey for the same reason: a grey fill makes shadows grey, and
//! grey shadows are what make stylised art look like an untextured render.
//!
//! Behind the three suns is a stylised vertical gradient sky rather than bare
//! ambient — saturated blue at the zenith, pale cool haze at the horizon, warm
//! dirt bounce from below. Three suns over a black world leave every shadow
//! grey; this is the single biggest difference between "3D render" and "game
//! art".
//!
//! ## Why azimuths are expressed against the camera
//!
//! The character rig's azimuths are numbers in a Blender scene whose camera can
//! be anywhere. This game's camera cannot: it is the fixed isometric view in
//! [`crate::iso`]. So what is preserved here is the rig's *geometry* — the key
//! over the viewer's left shoulder well round from the camera axis, the fill
//! roughly opposite it and low, the rim high and behind — along with its
//! elevations, colours and energy ratios exactly. Copying the raw azimuths
//! instead would put the key somewhere arbitrary in this view and lose the one
//! property that makes the rig work.
//!
//! Azimuth here is measured in the ground plane from screen-right, turning
//! toward the viewer: 0° is screen-right, 90° is directly over the camera, 180°
//! is screen-left, 270° is directly behind the subject.
//!
//! ## What the shader does with this
//!
//! Not what you would expect. The shader does **not** compute a colour from the
//! rig — colour comes from [`crate::palette`], which is baked from these same
//! numbers. What the shader computes per blade is *where on the palette* the
//! rig puts it: how much light it is catching, and how much of that light is
//! the key rather than the fill. That keeps every pixel exactly on the palette
//! while still letting a blade genuinely respond to which way it is leaning.

use bevy::prelude::*;

/// A directional light.
#[derive(Clone, Copy, Debug)]
pub struct Sun {
    /// Unit vector pointing from the scene toward the sun.
    pub direction: Vec3,
    /// Linear colour.
    pub colour: Vec3,
    /// Irradiance, in the same arbitrary units as the character rig.
    pub energy: f32,
}

impl Sun {
    /// Colour scaled by energy.
    pub fn radiance(&self) -> Vec3 {
        self.colour * self.energy
    }
}

/// Direction from the scene toward the camera.
///
/// A true isometric camera sits equally along all three axes. Mirrored in the
/// shader as `VIEW_DIRECTION`.
pub const VIEW: Vec3 = Vec3::new(0.577_350_3, 0.577_350_3, 0.577_350_3);

/// Energy of the key, which everything else is quoted against.
pub const KEY_ENERGY: f32 = 5.6;
/// Energy of the fill.
pub const FILL_ENERGY: f32 = 0.73;
/// Energy of the rim.
pub const RIM_ENERGY: f32 = 4.5;
/// Strength of the gradient sky.
pub const SKY_ENERGY: f32 = 1.85;

/// Zenith of the stylised sky.
pub const SKY_ZENITH: Vec3 = Vec3::new(0.34, 0.55, 1.00);
/// Horizon haze.
pub const SKY_HORIZON: Vec3 = Vec3::new(0.72, 0.82, 0.95);
/// Warm bounce off the dirt below.
pub const SKY_GROUND: Vec3 = Vec3::new(0.42, 0.34, 0.22);

/// How much sky reaches the very bottom of the canopy.
///
/// Blades are packed together and their lower halves sit in each other's
/// shadow. Not zero — the bottom of a canopy is dim, not black, and a palette
/// whose darkest entry is black reads as holes punched in the field.
///
/// Shared between the palette bake and the shader so that a blade's base lands
/// on the step the palette was baked for.
pub const CANOPY_FLOOR: f32 = 0.26;

/// Height, in metres, at which a blade is clear of the canopy around it.
///
/// Occlusion is a function of *absolute* height, not of how far up a given
/// blade a point is. That distinction is the whole reason a two-layer canopy
/// reads as one: a mat blade's tip at thirty centimetres is still buried among
/// its neighbours and should be lit like it, while a tuft blade's tip at a
/// metre is out in the open. Scaling by each blade's own length instead lights
/// every tip identically, and the two layers collapse into one flat surface.
pub const CANOPY_HEIGHT: f32 = 0.46;

/// How much sky reaches a point this many metres above the ground.
pub fn canopy_occlusion(height: f32) -> f32 {
    let t = (height / CANOPY_HEIGHT).clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);
    CANOPY_FLOOR + (1.0 - CANOPY_FLOOR) * eased
}

/// The golden afternoon key, over the viewer's left shoulder.
pub fn key() -> Sun {
    Sun {
        // Raked well round from the camera axis rather than sitting just off
        // it. The rig came with the measurement that makes this concrete: a key
        // 45° off-axis produced a left-to-right luminance ratio of only 1.41,
        // which is what "flat" means. This lands at about 55°.
        direction: direction(160.0, 38.0),
        colour: Vec3::new(1.00, 0.91, 0.72),
        energy: KEY_ENERGY,
    }
}

/// The low blue fill, roughly opposite the key.
pub fn fill() -> Sun {
    Sun {
        direction: direction(340.0, 14.0),
        colour: Vec3::new(0.42, 0.62, 1.00),
        energy: FILL_ENERGY,
    }
}

/// The high cool rim, from behind the subject.
pub fn rim() -> Sun {
    Sun {
        direction: direction(290.0, 55.0),
        colour: Vec3::new(0.80, 0.89, 1.00),
        energy: RIM_ENERGY,
    }
}

/// A ground-plane azimuth and an elevation, both in degrees, as a world vector.
///
/// Azimuth turns from screen-right toward the viewer. The two basis vectors are
/// the ground directions the isometric projection sends to screen-right and to
/// screen-down, which is what makes "over the viewer's left shoulder" mean the
/// same thing here as it does in front of the monitor.
pub fn direction(azimuth_degrees: f32, elevation_degrees: f32) -> Vec3 {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let right = Vec3::new(INV_SQRT2, -INV_SQRT2, 0.0);
    let toward_viewer = Vec3::new(INV_SQRT2, INV_SQRT2, 0.0);

    let azimuth = azimuth_degrees.to_radians();
    let elevation = elevation_degrees.to_radians();
    let horizontal = right * azimuth.cos() + toward_viewer * azimuth.sin();
    (horizontal * elevation.cos() + Vec3::Z * elevation.sin()).normalize()
}

/// The sky's contribution to a surface tilted `upness` (`-1`..`1`) from down to up.
pub fn sky(upness: f32) -> Vec3 {
    let t = (upness.clamp(-1.0, 1.0) + 1.0) * 0.5;
    if t >= 0.5 {
        SKY_HORIZON.lerp(SKY_ZENITH, (t - 0.5) * 2.0) * SKY_ENERGY
    } else {
        SKY_GROUND.lerp(SKY_HORIZON, t * 2.0) * SKY_ENERGY
    }
}

/// How much of a sun a strand catches, and how much it glints.
///
/// A blade is effectively a cylinder with no single normal, so lighting is
/// computed from its tangent instead: brightest across the light, darkest along
/// it. Because the tangent turns as the blade bends, grass changes tone as it
/// moves — which is most of why a gust reads as a wave of light travelling
/// across a field rather than as geometry wiggling.
pub fn strand(tangent: Vec3, sun: &Sun) -> (f32, f32) {
    let along_light = tangent.dot(sun.direction);
    let along_view = tangent.dot(VIEW);
    let across_light = (1.0 - along_light * along_light).max(0.0).sqrt();
    let across_view = (1.0 - along_view * along_view).max(0.0).sqrt();

    let diffuse = across_light;
    let glint = (across_light * across_view - along_light * along_view)
        .max(0.0)
        .powi(20);
    (diffuse, glint)
}

/// What each sun contributes to a strand.
#[derive(Clone, Copy, Debug, Default)]
pub struct Response {
    pub key: f32,
    pub fill: f32,
    pub rim: f32,
    /// How much of the gradient sky this strand can see.
    ///
    /// Carried rather than assumed, because the sky is the single largest term
    /// in the rig at 1.85 against a key of 5.6. Treating it as an unoccluded
    /// ambient constant puts a fifth of full exposure on *every* blade whatever
    /// its depth, which collapses the whole canopy onto one or two palette
    /// steps — a flat green rectangle, and one that looks deliberate enough to
    /// go unquestioned.
    pub sky: f32,
}

/// Evaluate the rig for a strand.
///
/// `occlusion` is how much of the sky reaches this point — near zero deep in
/// the canopy, one at an exposed tip.
///
/// The rim comes in almost entirely through its *glint* rather than its
/// diffuse term, which is the difference between a rim light and a third fill.
/// A strand's diffuse response to a light is near one for every orientation
/// except parallel to it, so at this rig's energies a diffuse rim would land
/// roughly four units of cool light on every blade in the field — enough to
/// swamp the albedo entirely and turn the dark end of every ramp grey. The
/// glint is a twentieth-power term and is genuinely sparse: it fires only where
/// a blade is edge-on to the light *and* to the camera at once, which is
/// exactly the edge a rim is supposed to draw.
pub fn respond(tangent: Vec3, occlusion: f32) -> Response {
    let occlusion = occlusion.clamp(0.0, 1.0);
    let (key_diffuse, key_glint) = strand(tangent, &key());
    let (fill_diffuse, _) = strand(tangent, &fill());
    let (rim_diffuse, rim_glint) = strand(tangent, &rim());

    Response {
        key: (key_diffuse + key_glint * 0.6) * occlusion,
        fill: fill_diffuse * occlusion,
        rim: (rim_glint + rim_diffuse * RIM_DIFFUSE) * occlusion * RIM_STRAND,
        sky: occlusion,
    }
}

/// How much of the rim arrives as plain diffuse rather than as a glint.
///
/// Small on purpose — see [`respond`].
pub const RIM_DIFFUSE: f32 = 0.12;

/// Correction from the rig's surface rim energy to a strand's.
///
/// [`RIM_ENERGY`] is the character rig's number and stays that way, because it
/// is what the sprites were rendered at. But it was calibrated against
/// *surfaces*, which reflect a rim into a narrow lobe around the mirror
/// direction. A strand has no single normal and scatters into a whole cone, so
/// the same irradiance comes back over a far wider range of orientations —
/// measurably, the strand glint stays above a half for most of the sphere where
/// a surface's would have fallen off long before.
///
/// Applied unscaled, the rim therefore lands several units of cool light on
/// nearly every blade at once. That does not read as a rim; it reads as a third
/// fill, and it greys the palette out from the dark end up. This is the ratio
/// of the two lobe widths, and it is the difference between grass with a bright
/// edge on it and grass the colour of dishwater.
pub const RIM_STRAND: f32 = 0.22;

/// Total light on a strand, normalised so 1.0 is a fully exposed blade square
/// on to the key.
///
/// This is the number the palette ramps are baked against, so a blade's
/// exposure picks its step directly.
pub fn exposure(response: &Response) -> f32 {
    let total = response.key * KEY_ENERGY
        + response.fill * FILL_ENERGY
        + response.rim * RIM_ENERGY
        + response.sky * SKY_ENERGY;
    (total / FULL_EXPOSURE).clamp(0.0, 1.0)
}

/// Fraction of the light on a strand that is the key rather than the fill.
///
/// Picks which ramp a blade sits on: key-lit blades run golden, fill-lit blades
/// run blue. This is the term that makes the rig visible as *colour* rather
/// than only as brightness.
pub fn key_share(response: &Response) -> f32 {
    let warm = response.key * KEY_ENERGY;
    let cool =
        response.fill * FILL_ENERGY + response.rim * RIM_ENERGY + response.sky * SKY_ENERGY * 0.5;
    if warm + cool <= 1e-6 {
        return 0.0;
    }
    (warm / (warm + cool)).clamp(0.0, 1.0)
}

/// Normalising constant for [`exposure`]: the rig's response to an ideal
/// fully-exposed strand.
const FULL_EXPOSURE: f32 = KEY_ENERGY + FILL_ENERGY * 0.5 + RIM_ENERGY * 0.25 + SKY_ENERGY;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    /// `cargo test -p bw_grass --lib -- --ignored --nocapture show_the_key_share`
    ///
    /// Not an assertion. The palette's ramp boundaries are two numbers in
    /// `GrassSettings`, and they are only meaningful against the distribution
    /// of key share the canopy actually produces. Guessing them puts the whole
    /// field on one ramp, which looks deliberate and is very hard to spot as a
    /// bug — the grass is a perfectly plausible green, just all of it.
    #[test]
    #[ignore = "prints the key-share distribution for calibration"]
    fn show_the_key_share() {
        // A rough stand-in for a canopy: blades leaning every way, weighted
        // toward upright, sampled through the range of depths they sit at.
        let mut samples = Vec::new();
        for a in 0..72 {
            let azimuth = a as f32 / 72.0 * std::f32::consts::TAU;
            for lean_step in 0..7 {
                let lean = lean_step as f32 / 6.0 * 1.2;
                let tangent = Vec3::new(
                    azimuth.cos() * lean.sin(),
                    azimuth.sin() * lean.sin(),
                    lean.cos(),
                );
                for depth in 0..6 {
                    let occlusion = canopy_occlusion(depth as f32 / 5.0 * CANOPY_HEIGHT);
                    let response = respond(tangent.normalize(), occlusion);
                    samples.push((key_share(&response), exposure(&response)));
                }
            }
        }
        let mut shares: Vec<f32> = samples.iter().map(|s| s.0).collect();
        let mut exposures: Vec<f32> = samples.iter().map(|s| s.1).collect();
        shares.sort_by(f32::total_cmp);
        exposures.sort_by(f32::total_cmp);

        let at = |v: &[f32], p: f32| v[((v.len() - 1) as f32 * p) as usize];
        println!("            p05    p25    p50    p75    p95");
        println!(
            "key share  {:.3}  {:.3}  {:.3}  {:.3}  {:.3}",
            at(&shares, 0.05),
            at(&shares, 0.25),
            at(&shares, 0.50),
            at(&shares, 0.75),
            at(&shares, 0.95)
        );
        println!(
            "exposure   {:.3}  {:.3}  {:.3}  {:.3}  {:.3}",
            at(&exposures, 0.05),
            at(&exposures, 0.25),
            at(&exposures, 0.50),
            at(&exposures, 0.75),
            at(&exposures, 0.95)
        );
    }

    #[test]
    fn every_sun_is_above_the_horizon() {
        for sun in [key(), fill(), rim()] {
            assert!(sun.direction.z > 0.0, "{sun:?} is underground");
            assert!(close(sun.direction.length(), 1.0, 1e-5));
        }
    }

    #[test]
    fn elevations_match_the_character_rig() {
        for (sun, elevation) in [(key(), 38.0), (fill(), 14.0), (rim(), 55.0)] {
            let measured = sun.direction.z.asin().to_degrees();
            assert!(
                close(measured, elevation, 0.05),
                "{measured} vs {elevation}"
            );
        }
    }

    #[test]
    fn the_key_comes_over_the_viewers_left_shoulder() {
        // Two things have to hold. It has to be on the viewer's side, or it is
        // a backlight; and it has to be to screen-left, or the form is carved
        // the wrong way round from every character sprite.
        const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let toward_viewer = Vec3::new(INV_SQRT2, INV_SQRT2, 0.0);
        let screen_left = Vec3::new(-INV_SQRT2, INV_SQRT2, 0.0);
        let key = key().direction;
        assert!(
            key.dot(toward_viewer) > 0.0,
            "the key is behind the subject"
        );
        assert!(key.dot(screen_left) > 0.0, "the key is on the wrong side");
    }

    #[test]
    fn the_key_is_well_off_the_camera_axis() {
        // The note that came with the rig: a key only 45° off-axis measured a
        // left/right luminance ratio of 1.41, which is what flat means. Raking
        // it well round is what carves form.
        let angle = key()
            .direction
            .dot(VIEW)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        assert!(
            angle > 50.0,
            "the key is only {angle:.0}° off the camera axis"
        );
    }

    #[test]
    fn the_fill_opposes_the_key() {
        // Their ground-plane directions should be roughly opposite; a fill next
        // to the key adds nothing and only lifts the shadows.
        let flat = |v: Vec3| Vec2::new(v.x, v.y).normalize();
        let separation = flat(key().direction).dot(flat(fill().direction));
        assert!(
            separation < -0.9,
            "fill is {separation} from opposing the key"
        );
    }

    #[test]
    fn the_rim_comes_from_behind() {
        const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let toward_viewer = Vec3::new(INV_SQRT2, INV_SQRT2, 0.0);
        assert!(rim().direction.dot(toward_viewer) < 0.0);
    }

    #[test]
    fn the_key_to_fill_ratio_is_preserved() {
        let ratio = KEY_ENERGY / FILL_ENERGY;
        assert!(close(ratio, 7.7, 0.1), "key:fill is {ratio}");
    }

    #[test]
    fn the_key_is_warm_and_the_fill_is_cool() {
        // The property that keeps shadows coloured rather than muddy. Losing it
        // is easy to do while tuning and turns the whole palette grey.
        let key = key().colour;
        let fill = fill().colour;
        assert!(key.x > key.z, "the key is not warm");
        assert!(fill.z > fill.x, "the fill is not cool");
        assert!(rim().colour.z > rim().colour.x, "the rim is not cool");
    }

    #[test]
    fn the_sky_is_blue_above_and_warm_below() {
        let above = sky(1.0);
        let below = sky(-1.0);
        assert!(above.z > above.x, "the zenith is not blue");
        assert!(below.x > below.z, "the ground bounce is not warm");
    }

    #[test]
    fn an_upright_blade_catches_more_key_than_a_flattened_one() {
        // A blade lying along the key direction presents no cross-section to
        // it. If this inverted, gusts would darken the grass instead of
        // lighting it up.
        let upright = respond(Vec3::Z, 1.0);
        let flat = respond(key().direction, 1.0);
        assert!(upright.key > flat.key, "{upright:?} vs {flat:?}");
    }

    #[test]
    fn exposure_rises_with_occlusion() {
        let mut previous = -1.0;
        for step in 0..=10 {
            let occlusion = step as f32 / 10.0;
            let value = exposure(&respond(Vec3::Z, occlusion));
            assert!(value > previous, "exposure fell at occlusion {occlusion}");
            previous = value;
        }
        assert!(previous <= 1.0);
    }

    #[test]
    fn exposure_stays_inside_its_range() {
        // Sampled over a sphere of tangents, because the ramp lookup indexes
        // straight off this and an out-of-range value would read the wrong ramp.
        for i in 0..64 {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            for tilt in [0.0, 0.3, 0.6, 0.9, 1.0] {
                let tangent = Vec3::new(
                    a.cos() * tilt,
                    a.sin() * tilt,
                    (1.0 - tilt * tilt).max(0.0).sqrt(),
                );
                for occlusion in [0.0, 0.5, 1.0] {
                    let value = exposure(&respond(tangent.normalize(), occlusion));
                    assert!((0.0..=1.0).contains(&value), "{value}");
                    let share = key_share(&respond(tangent.normalize(), occlusion));
                    assert!((0.0..=1.0).contains(&share), "{share}");
                }
            }
        }
    }

    #[test]
    fn a_blade_facing_the_key_reads_warmer_than_one_facing_away() {
        // What makes the ramp choice mean something. A strand across the key
        // catches it; one along the key does not.
        let across = Vec3::new(-key().direction.y, key().direction.x, 0.0).normalize();
        let along = key().direction;
        assert!(key_share(&respond(across, 1.0)) > key_share(&respond(along, 1.0)));
    }

    /// The shader evaluates this rig per blade, so it has to agree on where the
    /// suns are. A mismatch does not fail to compile — the grass just gets lit
    /// from somewhere the characters are not.
    #[test]
    fn shader_directions_match_this_module() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/grass.wgsl"
        );
        let source = std::fs::read_to_string(path).expect("the grass shader must exist");

        for (name, expected) in [
            ("KEY_DIRECTION", key().direction),
            ("FILL_DIRECTION", fill().direction),
            ("RIM_DIRECTION", rim().direction),
            ("VIEW_DIRECTION", VIEW),
        ] {
            let found = shader_vec3(&source, name)
                .unwrap_or_else(|| panic!("grass.wgsl must declare `{name}`"));
            assert!(
                (found - expected).length() < 1e-3,
                "{name}: shader has {found:?}, this module says {expected:?}"
            );
        }

        for (name, expected) in [
            ("KEY_ENERGY", KEY_ENERGY),
            ("FILL_ENERGY", FILL_ENERGY),
            ("RIM_ENERGY", RIM_ENERGY),
            ("SKY_ENERGY", SKY_ENERGY),
            ("FULL_EXPOSURE", FULL_EXPOSURE),
            ("CANOPY_FLOOR", CANOPY_FLOOR),
            ("CANOPY_HEIGHT", CANOPY_HEIGHT),
            ("RIM_DIFFUSE", RIM_DIFFUSE),
            ("RIM_STRAND", RIM_STRAND),
        ] {
            let found = shader_float(&source, name)
                .unwrap_or_else(|| panic!("grass.wgsl must declare `{name}`"));
            assert!(
                (found - expected).abs() < 1e-3,
                "{name}: shader has {found}, this module says {expected}"
            );
        }
    }

    /// Pull `const NAME: vec3<f32> = vec3<f32>(a, b, c);` out of the shader.
    ///
    /// Parsed rather than string-matched so the shader can stay readable —
    /// printing an `f32` at full precision would put `0.61566144` in a file
    /// people have to look at.
    fn shader_vec3(source: &str, name: &str) -> Option<Vec3> {
        let marker = format!("const {name}: vec3<f32> = vec3<f32>(");
        let start = source.find(&marker)? + marker.len();
        let body = &source[start..start + source[start..].find(')')?];
        let parts: Vec<f32> = body
            .split(',')
            .filter_map(|part| part.trim().parse::<f32>().ok())
            .collect();
        (parts.len() == 3).then(|| Vec3::new(parts[0], parts[1], parts[2]))
    }

    fn shader_float(source: &str, name: &str) -> Option<f32> {
        let marker = format!("const {name}: f32 = ");
        let start = source.find(&marker)? + marker.len();
        let body = &source[start..start + source[start..].find(';')?];
        body.trim().parse::<f32>().ok()
    }
}

//! A CPU model of where the clump shader puts things.
//!
//! Every temporal metric in this suite needs to know what a plant does on
//! screen, and the honest way to find out is to run the shader. That is not
//! available to a benchmark: it needs a GPU, a window, and a capture, none of
//! which survive CI or a second run in the same minute. So the vertex stage is
//! mirrored here instead, and the mirror is kept honest by
//! [`assert_matches_shader`], which reads the constants back out of the WGSL and
//! fails if any of them has moved.
//!
//! ## Why this is worth the duplication
//!
//! Because the interesting failures live between the simulation and the screen,
//! and neither end can see them. The field can be perfectly smooth while the
//! picture flickers, for three reasons this module can measure and neither the
//! field nor a screenshot can:
//!
//! - **The pixel grid.** Motion becomes visible only when it crosses a pixel
//!   boundary. Smooth sub-pixel drift that happens to sit on a boundary reads
//!   as a pixel flipping back and forth — chatter — and the field it came from
//!   looks blameless.
//! - **The depth sort.** A clump's depth includes its height, and leaning
//!   shortens it. Two overlapping plants can therefore swap draw order in the
//!   middle of a gust, which is a whole sprite popping in front of another.
//! - **Stiction and per-clump stiffness.** The shader deliberately does not
//!   pass the field through unchanged. A field metric measures the input to
//!   that, not the output.
//!
//! Everything here is mirrored from `assets/shaders/clump.wgsl`. Read that
//! first; the comments there explain *why* each term is shaped the way it is,
//! and are not repeated.

use bevy::math::{Vec2, Vec3};
use bw_grass::clump;
use bw_grass::field::GrassField;
use bw_grass::iso;

/// Mirrored from `clump.wgsl`.
pub const COMPLIANCE_MIN: f32 = 0.30;
pub const COMPLIANCE_MAX: f32 = 1.70;
pub const STICTION: f32 = 0.16;
pub const FULL_LEAN: f32 = 0.62;
pub const ALPHA_CUT: f32 = 0.45;
pub const MAX_TIP_ANGLE: f32 = 1.4835;

/// Rows of vertices up a card. Mirrored from `clump::CARD_ROWS`.
///
/// Two would be a plain quad, and a plain quad is why `root_stiffness` could
/// never have worked: `up` takes only the values zero and one, `pow` fixes both
/// of them whatever the exponent, and the rasteriser fills a straight line in
/// between. `grass.card.stiffness_effect` is that fact as a number, and it read
/// zero for as long as this was two.
pub const CARD_ROWS: usize = clump::CARD_ROWS;

/// Points along the drawn centreline that [`profile`] reports.
///
/// More than the card has rows, deliberately. What matters is the line the
/// *rasteriser* draws, not the vertices it draws it from, and the difference
/// between the two is exactly the thing being measured.
const PROFILE_SAMPLES: usize = 24;

/// A clump as the vertex shader sees it.
#[derive(Clone, Copy, Debug)]
pub struct Clump {
    pub root: Vec2,
    pub width: f32,
    pub height: f32,
    /// Already snapped to a palette rung when the chunk was built.
    pub shade: f32,
    pub random: f32,
}

/// Where a clump ends up this frame.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    /// Screen position of the root, which never moves.
    pub root: Vec2,
    /// Screen position of the top-centre of the sprite — the part that leans,
    /// and therefore the part whose motion a viewer actually reads.
    pub tip: Vec2,
    /// Isometric depth of the plant's root.
    ///
    /// The whole quad sits at this one depth — see the shader. It cannot change
    /// while the grass moves, which is the entire point, so `depth_pop_rate`
    /// measuring zero is a structural fact rather than a lucky reading.
    pub depth: f32,
    /// Height of the drawn silhouette, in screen units.
    pub silhouette: f32,
}

/// The shader's `hash11`, bit for bit.
///
/// Reproduced rather than replaced with a better hash on purpose: the point is
/// to predict what the shader does, and a *different* random number would give
/// every clump a different stiffness from the one it is drawn with.
fn hash11(x: f32) -> f32 {
    let value = (x * 78.233).sin() * 43758.547;
    value - value.floor()
}

/// WGSL `smoothstep`.
fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Settings the shader is driven with, mirrored from `ClumpSettings::default`.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub bend_angle: f32,
    pub squash: f32,
    pub root_stiffness: f32,
    pub max_angle: f32,
}

impl Settings {
    /// Read the shipped defaults, so the mirror cannot be tuned separately from
    /// the thing it mirrors.
    pub fn shipped(field: &GrassField) -> Self {
        Self {
            max_angle: field.params().max_angle,
            ..Self::shipped_defaults()
        }
    }

    /// The same, without a field to read the angular cap from.
    pub fn shipped_defaults() -> Self {
        let settings = clump::ClumpSettings::default();
        Self {
            bend_angle: settings.bend_angle,
            squash: settings.squash,
            root_stiffness: settings.root_stiffness,
            max_angle: settings.max_angle,
        }
    }
}

/// How much of the field's bend this clump takes, in 0..1, and which way.
///
/// Split out because it is the whole of the shader's editorial: the field says
/// what the wind is doing and this decides how much of it a given plant admits.
pub fn response(clump: &Clump, field: &GrassField, settings: &Settings) -> (Vec2, f32) {
    let bend = field.bend_at(clump.root);
    let strength = (bend.length() / settings.max_angle.max(1e-4)).clamp(0.0, 1.0);
    let direction = if strength > 1e-4 {
        bend.normalize()
    } else {
        Vec2::ZERO
    };
    (direction, taken(strength, clump.random))
}

/// The share of the field's bend a plant of this randomness takes.
///
/// Split out of [`response`] so the geometry benchmarks can drive a clump to a
/// chosen bend without building a field that happens to produce it.
pub fn taken(strength: f32, random: f32) -> f32 {
    let compliance = COMPLIANCE_MIN + (COMPLIANCE_MAX - COMPLIANCE_MIN) * hash11(random + 4.1);
    smoothstep(STICTION, FULL_LEAN, strength.clamp(0.0, 1.0)) * compliance
}

/// Where one row of the card sits, in world metres from the root.
///
/// `x` is horizontal displacement along the bend direction and `y` is height.
/// This is the whole of the card's shape, and it is the function that changes
/// when the card does. Mirrored from the loop in `clump.wgsl`, midpoint rule and
/// all — approximating it with the closed-form arc would be a different curve
/// from the one the GPU draws, which is the one being measured.
fn row(clump: &Clump, settings: &Settings, share: f32, exponent: f32, up: f32) -> Vec2 {
    let bands = ((CARD_ROWS - 1) as f32 * up).round() as usize;
    let step = clump.height / (CARD_ROWS - 1) as f32;
    let tip = (settings.bend_angle * share).min(MAX_TIP_ANGLE);
    let (mut along, mut lift) = (0.0, 0.0);
    for band in 0..bands {
        let mid = (band as f32 + 0.5) / (CARD_ROWS - 1) as f32;
        let angle = tip * mid.powf(exponent);
        along += angle.sin() * step;
        lift += angle.cos() * step;
    }
    Vec2::new(along, lift * (1.0 - settings.squash * share.min(1.0)))
}

/// The drawn centreline, sampled evenly up the sprite.
///
/// Interpolated between card rows the way the rasteriser does, so a card with
/// two rows reports a straight line however curved its intent was.
pub fn profile_with(clump: &Clump, share: f32, settings: &Settings, exponent: f32) -> Vec<Vec2> {
    let segments = (CARD_ROWS - 1) as f32;
    (0..PROFILE_SAMPLES)
        .map(|index| {
            let up = index as f32 / (PROFILE_SAMPLES - 1) as f32;
            let scaled = up * segments;
            let low = scaled.floor().min(segments - 1.0);
            let t = scaled - low;
            let a = row(clump, settings, share, exponent, low / segments);
            let b = row(clump, settings, share, exponent, (low + 1.0) / segments);
            a.lerp(b, t)
        })
        .collect()
}

/// [`profile_with`] at the shipped settings.
pub fn profile_with_exponent(clump: &Clump, share: f32, exponent: f32) -> Vec<Vec2> {
    profile_with(clump, share, &Settings::shipped_defaults(), exponent)
}

/// [`profile_with_exponent`] at the shipped exponent.
///
/// `share` is how much of the field's bend the plant is admitting — one being a
/// plant giving the wind everything it has. Driven directly rather than through
/// a field and a [`taken`] roll, because the shape of the card is the thing
/// being measured and a particular plant's compliance only scales it. Measuring
/// through a roll made every number depend on what `hash11` happened to return
/// for one clump, which is how `lean_shortening` came to be reported at a
/// nineteen-degree bend and read as a regression when the card started
/// shortening correctly.
pub fn profile(clump: &Clump, share: f32) -> Vec<Vec2> {
    let exponent = Settings::shipped_defaults().root_stiffness;
    profile_with_exponent(clump, share, exponent)
}

/// Vertex and index bytes one clump occupies.
///
/// Root at full precision, corner as four bytes, shape as four halves, and
/// sixteen-bit indices — a chunk never reaches sixty-five thousand vertices.
pub fn bytes_per_clump() -> f64 {
    let vertices = CARD_ROWS * 2;
    let bands = CARD_ROWS - 1;
    (vertices * (8 + 4 + 8) + bands * 6 * 2) as f64
}

/// Place one clump for one frame.
pub fn place(clump: &Clump, field: &GrassField, settings: &Settings) -> Placement {
    let (direction, share) = response(clump, field, settings);

    // The top row, where `up` is one and the centreline has been walked all the
    // way to the tip.
    let tip_offset = row(clump, settings, share, settings.root_stiffness, 1.0);
    let top = Vec3::new(
        clump.root.x + direction.x * tip_offset.x,
        clump.root.y + direction.y * tip_offset.x,
        tip_offset.y,
    );
    let base = Vec3::new(clump.root.x, clump.root.y, 0.0);

    let tip = iso::project(top);
    let root = iso::project(base);
    Placement {
        root,
        tip,
        depth: iso::depth(base),
        silhouette: tip.y - root.y,
    }
}

/// Pull a representative set of clumps out of the shipped placement code.
///
/// Taken from `clump::build_chunk` rather than invented, so the sample has the
/// real distribution of sizes, stiffnesses and neighbours. An evenly spaced
/// lattice of identical plants would make every temporal metric look better
/// than it is — no two neighbours would ever disagree, which is precisely the
/// disagreement these metrics are about.
pub fn sample(field: &GrassField, chunks: i32, seed: u32) -> Vec<Clump> {
    let mut out = Vec::new();
    for y in 0..chunks {
        for x in 0..chunks {
            let batch = clump::build_chunk(field, bevy::math::IVec2::new(x, y), 1.0, seed);
            for (root, shape) in batch.roots().zip(batch.shapes()) {
                out.push(Clump {
                    root,
                    width: shape[0],
                    height: shape[1],
                    shade: shape[2],
                    random: shape[3],
                });
            }
        }
    }
    out
}

/// Which atlas variant each of [`sample`]'s clumps draws, in the same order.
///
/// Separate rather than folded into [`Clump`] because the vertex stage never
/// sees it — the cell is chosen when the chunk is built and only the fragment
/// stage cares. Tone measurement does care, because a variant's own brightness
/// is half of what a clump contributes to the field's tonal spread.
pub fn sample_variants(field: &GrassField, chunks: i32, seed: u32) -> Vec<usize> {
    let mut out = Vec::new();
    for y in 0..chunks {
        for x in 0..chunks {
            let batch = clump::build_chunk(field, bevy::math::IVec2::new(x, y), 1.0, seed);
            for (column, row) in batch.cells() {
                out.push(row * clump::COLUMNS + column);
            }
        }
    }
    out
}

/// Fail if the mirror has drifted from the shader.
///
/// The one thing that makes duplicating a shader in Rust defensible. Without
/// it, someone tunes `STICTION` in the WGSL, every stability metric silently
/// starts describing a renderer that no longer exists, and the suite reports
/// that nothing changed.
pub fn assert_matches_shader(source: &str) {
    for (name, ours) in [
        ("COMPLIANCE_MIN", COMPLIANCE_MIN),
        ("COMPLIANCE_MAX", COMPLIANCE_MAX),
        ("STICTION", STICTION),
        ("FULL_LEAN", FULL_LEAN),
        ("ALPHA_CUT", ALPHA_CUT),
        ("MAX_TIP_ANGLE", MAX_TIP_ANGLE),
        ("SHAPE_METRES", clump::SHAPE_METRES),
        ("CARD_BANDS", (clump::CARD_ROWS - 1) as f32),
    ] {
        let marker = format!("const {name}: f32 = ");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("{name} is no longer declared in clump.wgsl"))
            + marker.len();
        let end = start + source[start..].find(';').expect("unterminated constant");
        let theirs: f32 = source[start..end]
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("could not parse {name}"));
        assert!(
            (theirs - ours).abs() < 1e-6,
            "benches/mirror.rs has {name} = {ours}, clump.wgsl has {theirs}. \
             The stability metrics describe the shader, so they are wrong until \
             this is updated."
        );
    }
}

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
pub const STIFFNESS_MIN: f32 = 0.30;
pub const STIFFNESS_MAX: f32 = 1.70;
pub const STICTION: f32 = 0.16;
pub const FULL_LEAN: f32 = 0.62;
pub const POSE_STEPS: f32 = 5.0;
pub const POSE_JITTER: f32 = 1.0;
pub const ALPHA_CUT: f32 = 0.45;

/// A clump as the vertex shader sees it.
#[derive(Clone, Copy, Debug)]
pub struct Clump {
    pub root: Vec2,
    pub width: f32,
    pub height: f32,
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
    /// Isometric depth halfway up the sprite.
    ///
    /// Halfway rather than at the tip because depth is interpolated across the
    /// quad and what decides which of two overlapping plants wins is the depth
    /// where they overlap — which is their middles, not their extremes. Taking
    /// it at the tip would report pops that the depth buffer never sees, and
    /// miss the ones it does.
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
    pub lean: f32,
    pub squash: f32,
    pub root_stiffness: f32,
    pub max_angle: f32,
}

impl Settings {
    /// Read the shipped defaults, so the mirror cannot be tuned separately from
    /// the thing it mirrors.
    pub fn shipped(field: &GrassField) -> Self {
        let settings = clump::ClumpSettings::default();
        Self {
            lean: settings.lean,
            squash: settings.squash,
            root_stiffness: settings.root_stiffness,
            max_angle: field.params().max_angle,
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
    let stiffness = STIFFNESS_MIN + (STIFFNESS_MAX - STIFFNESS_MIN) * hash11(clump.random + 4.1);
    let continuous = smoothstep(STICTION, FULL_LEAN, strength);
    let pose_offset = hash11(clump.random + 9.7) * POSE_JITTER;
    let responsive = (continuous * POSE_STEPS + pose_offset).floor() / POSE_STEPS;
    let direction = if strength > 1e-4 {
        bend.normalize()
    } else {
        Vec2::ZERO
    };
    (direction, responsive * stiffness)
}

/// Place one clump for one frame.
pub fn place(clump: &Clump, field: &GrassField, settings: &Settings) -> Placement {
    let (direction, taken) = response(clump, field, settings);

    let lean = direction * (taken * settings.lean * clump.height);
    let squash = 1.0 - settings.squash * taken;

    // The top edge, where `up` is one, so the height weighting is one too.
    let top = Vec3::new(
        clump.root.x + lean.x,
        clump.root.y + lean.y,
        clump.height * squash,
    );
    let base = Vec3::new(clump.root.x, clump.root.y, 0.0);

    // Halfway up, where the height weighting bites. `pow(0.5, root_stiffness)`
    // is well under a half at the shipped exponent, which is the whole point of
    // the exponent: almost none of the lean belongs in the bottom third.
    let middle_weight = 0.5f32.powf(settings.root_stiffness);
    let middle = Vec3::new(
        clump.root.x + lean.x * middle_weight,
        clump.root.y + lean.y * middle_weight,
        0.5 * clump.height * squash,
    );

    let tip = iso::project(top);
    let root = iso::project(base);
    Placement {
        root,
        tip,
        depth: iso::depth(middle),
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
                    random: shape[3],
                });
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
        ("STIFFNESS_MIN", STIFFNESS_MIN),
        ("STIFFNESS_MAX", STIFFNESS_MAX),
        ("STICTION", STICTION),
        ("FULL_LEAN", FULL_LEAN),
        ("POSE_STEPS", POSE_STEPS),
        ("POSE_JITTER", POSE_JITTER),
        ("ALPHA_CUT", ALPHA_CUT),
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

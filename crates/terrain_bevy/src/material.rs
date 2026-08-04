//! The runtime surface material.
//!
//! The cache does the expensive work, so this is deliberately the cheapest
//! material in the game: one texture sample, one multiply, no discard, no
//! branching, no procedural noise per fragment. Roughly nine tenths of the
//! grass pixels on screen come through here, and the whole point of baking is
//! that they cost about as much as a background image.
//!
//! It is a custom [`Material2d`] rather than a plain sprite because of what
//! comes next. Wind shimmer, the trampling field and time-of-day grading all
//! want to modulate the cached page a little without rebaking it, and all three
//! are uniform reads in this shader. A sprite would have to become one anyway.
//!
//! Opaque, and that matters: pages tile the ground with no gaps, so there is
//! nothing behind them to blend with, and an alpha-blended ground layer would
//! give up early-z for nothing.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d};

/// Where the surface shader lives.
pub const SHADER_PATH: &str = "shaders/grass_surface.wgsl";

/// Per-page constants.
///
/// Field order matches `grass_surface.wgsl`. WGSL structs are laid out by
/// declaration order, so a field inserted here and appended there silently
/// misreads every value after it — which shows up as grass that is the wrong
/// colour rather than as anything that fails to compile.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct SurfaceSettings {
    /// Multiplied into the cached colour. Grading, not lighting.
    pub tint: Vec3,
    /// Overall brightness. One is the page exactly as it was baked.
    pub exposure: f32,
}

impl Default for SurfaceSettings {
    fn default() -> Self {
        Self {
            tint: Vec3::ONE,
            exposure: 1.0,
        }
    }
}

/// One baked page of ground.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct GrassSurfaceMaterial {
    #[uniform(0)]
    pub settings: SurfaceSettings,
    #[texture(1)]
    #[sampler(2)]
    pub page: Handle<Image>,
}

impl Material2d for GrassSurfaceMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Opaque
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The uniform lives in two places and nothing but this stops them drifting.
    /// A mismatch does not fail to compile; it produces grass of the wrong
    /// colour, which is a much worse way to find out.
    #[test]
    fn the_shader_declares_the_same_uniform_fields() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/grass_surface.wgsl"
        );
        let source = std::fs::read_to_string(path).expect("the surface shader must exist");
        let start = source
            .find("struct SurfaceSettings")
            .expect("no settings struct");
        let end = source[start..].find('}').expect("unterminated struct") + start;
        let body = &source[start..end];
        for (field, kind) in [("tint", "vec3<f32>"), ("exposure", "f32")] {
            assert!(
                body.contains(&format!("{field}: {kind}")),
                "grass_surface.wgsl must declare `{field}: {kind}`"
            );
        }
        // Order, not just presence.
        assert!(body.find("tint").unwrap() < body.find("exposure").unwrap());
    }
}

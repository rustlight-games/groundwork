//! Generator registries.
//!
//! Terrain and rock generators produce data, not simulation side effects, so
//! their traits live here alongside the schemas. Effect *handlers* are
//! different: running one needs mutable access to the battle world, so that
//! trait lives in `bw_sim` and only the set of registered keys is passed back
//! here for validation.
//!
//! Registration happens at startup from the plugin crates under `plugins/`.
//! Nothing is loaded at runtime — Rust has no stable ABI, so dynamic plugin
//! loading would mean every plugin had to be built with a byte-identical
//! compiler and dependency graph. Compile-time crates give the same modularity
//! with none of that fragility, and content data stays hot-reloadable, which is
//! where iteration speed actually matters.

use bw_core::Vec2Fx;
use indexmap::IndexMap;
use rand_chacha::ChaCha8Rng;
use smol_str::SmolStr;

use crate::error::ContentResult;
use crate::params::Params;
use crate::terrain::{TerrainGenContext, TerrainMap};

/// Builds a battlefield.
pub trait TerrainGenerator: Send + Sync + 'static {
    /// Registry key, matching `terrain_generator` in an encounter definition.
    fn key(&self) -> &'static str;

    /// Reject bad parameters at load time rather than mid-battle.
    fn validate(&self, _params: &Params) -> ContentResult<()> {
        Ok(())
    }

    /// Fill `out`. Must depend only on `ctx` and `rng` — the same inputs have
    /// to produce the same battlefield in the trainer and in the game.
    fn generate(&self, ctx: &TerrainGenContext<'_>, rng: &mut ChaCha8Rng, out: &mut TerrainMap);
}

/// Builds 2D rock artwork.
pub trait RockGenerator: Send + Sync + 'static {
    fn key(&self) -> &'static str;

    fn validate(&self, _params: &Params) -> ContentResult<()> {
        Ok(())
    }

    fn generate(&self, params: &Params, rng: &mut ChaCha8Rng) -> RockShape;
}

/// A generated rock, in local space centred on the origin.
///
/// Geometry rather than pixels: the renderer rasterises it, the simulation
/// takes the outline as a collider, and `bw_bench` scores its silhouette. Had
/// this been an image, only the first of those three would work.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RockShape {
    /// Closed outline, counter-clockwise, first point not repeated.
    pub outline: Vec<Vec2Fx>,
    /// Interior polygons used for flat shading.
    pub facets: Vec<Facet>,
    pub palette: RockPalette,
}

impl RockShape {
    /// Signed area via the shoelace formula; positive when counter-clockwise.
    pub fn signed_area(&self) -> f64 {
        let n = self.outline.len();
        if n < 3 {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in 0..n {
            let a = self.outline[i];
            let b = self.outline[(i + 1) % n];
            sum += a.x.to_num::<f64>() * b.y.to_num::<f64>()
                - b.x.to_num::<f64>() * a.y.to_num::<f64>();
        }
        sum / 2.0
    }

    pub fn perimeter(&self) -> f64 {
        let n = self.outline.len();
        if n < 2 {
            return 0.0;
        }
        (0..n)
            .map(|i| {
                self.outline[i]
                    .distance(self.outline[(i + 1) % n])
                    .to_num::<f64>()
            })
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Facet {
    pub polygon: Vec<Vec2Fx>,
    /// Relative brightness, 128 being unshaded.
    pub shade: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RockPalette {
    pub base: [u8; 3],
    pub light: [u8; 3],
    pub shadow: [u8; 3],
}

impl Default for RockPalette {
    fn default() -> Self {
        Self {
            base: [122, 118, 112],
            light: [166, 162, 154],
            shadow: [72, 70, 68],
        }
    }
}

/// Everything registered by the generator plugins.
#[derive(Default)]
pub struct GeneratorRegistry {
    terrain: IndexMap<SmolStr, Box<dyn TerrainGenerator>>,
    rocks: IndexMap<SmolStr, Box<dyn RockGenerator>>,
}

impl GeneratorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a terrain generator, replacing any existing one with the same
    /// key so a plugin can deliberately override a built-in.
    pub fn add_terrain(&mut self, generator: impl TerrainGenerator) -> &mut Self {
        self.terrain
            .insert(SmolStr::new(generator.key()), Box::new(generator));
        self
    }

    pub fn add_rock(&mut self, generator: impl RockGenerator) -> &mut Self {
        self.rocks
            .insert(SmolStr::new(generator.key()), Box::new(generator));
        self
    }

    pub fn terrain(&self, key: &str) -> Option<&dyn TerrainGenerator> {
        self.terrain.get(key).map(|b| b.as_ref())
    }

    pub fn rock(&self, key: &str) -> Option<&dyn RockGenerator> {
        self.rocks.get(key).map(|b| b.as_ref())
    }

    pub fn terrain_keys(&self) -> impl Iterator<Item = &str> {
        self.terrain.keys().map(SmolStr::as_str)
    }

    pub fn rock_keys(&self) -> impl Iterator<Item = &str> {
        self.rocks.keys().map(SmolStr::as_str)
    }
}

#[cfg(test)]
mod tests {
    use bw_core::real_from_int;

    use super::*;

    struct Flat;
    impl TerrainGenerator for Flat {
        fn key(&self) -> &'static str {
            "flat"
        }
        fn generate(&self, _: &TerrainGenContext<'_>, _: &mut ChaCha8Rng, _: &mut TerrainMap) {}
    }

    struct Blob;
    impl RockGenerator for Blob {
        fn key(&self) -> &'static str {
            "blob"
        }
        fn generate(&self, _: &Params, _: &mut ChaCha8Rng) -> RockShape {
            RockShape::default()
        }
    }

    #[test]
    fn registers_and_looks_up_by_key() {
        let mut r = GeneratorRegistry::new();
        r.add_terrain(Flat).add_rock(Blob);
        assert!(r.terrain("flat").is_some());
        assert!(r.rock("blob").is_some());
        assert!(r.terrain("missing").is_none());
    }

    #[test]
    fn later_registration_overrides_the_same_key() {
        let mut r = GeneratorRegistry::new();
        r.add_terrain(Flat).add_terrain(Flat);
        assert_eq!(r.terrain_keys().count(), 1);
    }

    #[test]
    fn signed_area_is_positive_for_counter_clockwise_outlines() {
        let square = RockShape {
            outline: vec![
                Vec2Fx::from_ints(0, 0),
                Vec2Fx::from_ints(2, 0),
                Vec2Fx::from_ints(2, 2),
                Vec2Fx::from_ints(0, 2),
            ],
            ..Default::default()
        };
        assert_eq!(square.signed_area(), 4.0);
        assert_eq!(square.perimeter(), 8.0);
    }

    #[test]
    fn degenerate_outlines_have_zero_area_rather_than_panicking() {
        let line = RockShape {
            outline: vec![
                Vec2Fx::ZERO,
                Vec2Fx::new(real_from_int(1), bw_core::Real::ZERO),
            ],
            ..Default::default()
        };
        assert_eq!(line.signed_area(), 0.0);
    }
}

//! Plugin registration.
//!
//! One place where every content plugin is named. Both the game and
//! `tools/bw_train` call these, which is the point: a battle in training runs
//! against exactly the same effect handlers and generators as a battle in the
//! game. If the two lists ever diverged, a policy would be trained against
//! rules the player never sees.

use bw_content::registry::GeneratorRegistry;
use bw_sim::effects::EffectRegistry;

/// Every effect primitive from every plugin.
pub fn build_effect_registry() -> EffectRegistry {
    let mut registry = EffectRegistry::new();
    bw_fx_abilities::register(&mut registry);
    bw_fx_terrain::register_effects(&mut registry);
    registry
}

/// Every terrain and rock generator from every plugin.
pub fn build_generator_registry() -> GeneratorRegistry {
    let mut registry = GeneratorRegistry::new();
    bw_fx_terrain::register_generators(&mut registry);
    bw_fx_rocks::register_generators(&mut registry);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ability_primitive_is_registered() {
        let registry = build_effect_registry();
        for key in ["damage", "heal", "apply_status", "sequence", "terrain_mud"] {
            assert!(
                registry.contains(key),
                "{key} is missing from the effect registry"
            );
        }
    }

    #[test]
    fn every_generator_is_registered() {
        let registry = build_generator_registry();
        assert!(registry.terrain("rolling_hills").is_some());
        assert!(registry.rock("boulder").is_some());
    }

    #[test]
    fn registry_keys_are_unique_across_plugins() {
        // Two plugins claiming one key would silently shadow each other.
        let registry = build_effect_registry();
        let mut keys: Vec<&str> = registry.keys().collect();
        let before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), before);
    }
}

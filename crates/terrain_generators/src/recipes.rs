//! The recipes this build knows how to grow.
//!
//! Four, and only one of them is finished. That is deliberate and the shape is
//! the point: **adding a material should be content and a recipe, not another
//! architectural change.** These three unfinished ones are here to prove that
//! claim before there is pressure to break it — a document can name wildflowers
//! today, validation checks its parameters today, and the thing that is missing
//! is the geometry rather than the plumbing.
//!
//! | recipe                          | status                                  |
//! | ------------------------------- | --------------------------------------- |
//! | `population.grass_lush`         | the real one; emits ribbons             |
//! | `population.wildflowers_meadow` | validates, emits a stem and a head      |
//! | `population.granite_rocks`      | validates, emits an analytic boulder    |
//! | `population.dirt_scatter`       | validates, emits pebbles                |
//!
//! "Minimal" here means *the simplest thing that is honestly that content*
//! rather than a stub that emits nothing. A wildflower that is a curve and a
//! disc is a poor wildflower and it is a wildflower; a recipe that emits nothing
//! would let every downstream assumption about non-grass content go untested
//! until the day somebody wrote the good version.
//!
//! ## What none of them do
//!
//! None sets a painter order, a stable id, or its own bounds. Those are
//! properties of the *scene* rather than of the content, and a recipe that chose
//! its own painter order could put itself in front of everything.

use terrain_core::diagnostics::DiagnosticReport;
use terrain_core::document::PopulationDef;
use terrain_core::ids::{RecipeKey, StreamKey};
use terrain_core::seed::RandomAddress;
use terrain_scene::mark::{MarkAttributes, RibbonGeometry, Stratum, TipShape, WidthProfile};

use crate::population::{
    Candidate, EmittedMark, PopulationContext, PopulationOutput, PopulationRecipe,
    PopulationRegistry, cell_size_for, positive_parameter,
};

/// Every recipe this build knows about.
///
/// One function, listing everything. That is the whole benefit of explicit
/// registration over the link-time kind: a reviewer can read what the binary can
/// do, and two recipes claiming one key fail here rather than by link order.
pub fn register_all(registry: &mut PopulationRegistry) {
    registry.register(Box::new(GrassRecipe));
    registry.register(Box::new(WildflowerRecipe));
    registry.register(Box::new(RockRecipe));
    registry.register(Box::new(DirtScatterRecipe));
}

/// A registry with everything registered.
pub fn default_registry() -> PopulationRegistry {
    let mut registry = PopulationRegistry::new();
    register_all(&mut registry);
    registry
}

/// How many candidates one cell offers.
///
/// Eight. A lattice with one candidate per cell shows its own grid however hard
/// the position is jittered, because the *count* is uniform even when the
/// placement is not — and the eye finds a uniform count faster than it finds a
/// uniform position.
const CANDIDATES_PER_CELL: u16 = 8;

fn stream(name: &str) -> StreamKey {
    StreamKey::new(name).expect("recipe stream names are valid by construction")
}

/// Draw a value in a range at a named stream.
fn draw(
    context: &PopulationContext<'_>,
    candidate: &Candidate,
    name: &str,
    low: f32,
    high: f32,
) -> f32 {
    let key = stream(name);
    context.seeds.range(
        &RandomAddress::new(candidate.id, &key),
        low as f64,
        high as f64,
    ) as f32
}

// ---------------------------------------------------------------------------
// Grass
// ---------------------------------------------------------------------------

/// Lush grass: a canopy of tapered ribbons.
///
/// The finished one, and the shape every other recipe is written against.
pub struct GrassRecipe;

impl PopulationRecipe for GrassRecipe {
    fn key(&self) -> RecipeKey {
        RecipeKey::new("population.grass_lush").expect("valid")
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["plant.grass_blade"]
    }

    fn validate(&self, definition: &PopulationDef, diagnostics: &mut DiagnosticReport) {
        let at = format!("populations.{}", definition.key);
        positive_parameter(&definition.parameters, "density", &at, 400.0, diagnostics);
        positive_parameter(&definition.parameters, "length_m", &at, 0.22, diagnostics);
    }

    fn maximum_reach_m(&self, definition: &PopulationDef) -> f64 {
        // A blade cannot displace its own tip further than its arc length, and
        // the longest a blade may be is the authored length at the top of its
        // scatter. A genuine bound, not a typical one — see the module note in
        // `population`.
        let length = definition.parameters.number("length_m").unwrap_or(0.22);
        length * LENGTH_SCATTER_HIGH as f64
    }

    fn emit(&self, context: &PopulationContext<'_>, output: &mut dyn PopulationOutput) {
        let density = context
            .definition
            .parameters
            .number("density")
            .unwrap_or(400.0);
        let length = context
            .definition
            .parameters
            .number("length_m")
            .unwrap_or(0.22) as f32;
        let cell = cell_size_for(density, CANDIDATES_PER_CELL);
        let indices = crate::population::PopulationIndices::default();

        let accept = stream("accept");
        for candidate in context.candidates(CANDIDATES_PER_CELL, cell) {
            let ground = context.ground(candidate.position);
            let abundance = context.abundance_at(&ground, &indices);
            // The acceptance test, and the reason candidates exist: turning the
            // density down changes which candidates survive and moves none of
            // the survivors.
            if !context
                .seeds
                .chance(&RandomAddress::new(candidate.id, &accept), abundance as f64)
            {
                continue;
            }

            let geometry = RibbonGeometry {
                length_m: length
                    * draw(
                        context,
                        &candidate,
                        "length",
                        LENGTH_SCATTER_LOW,
                        LENGTH_SCATTER_HIGH,
                    ),
                azimuth_rad: draw(context, &candidate, "azimuth", 0.0, std::f32::consts::TAU),
                bend_rad: draw(context, &candidate, "bend", 0.35, 1.40),
                twist_rad: draw(context, &candidate, "twist", -1.2, 1.2),
                width_m: draw(context, &candidate, "width", 0.002, 0.006),
                tip_width_m: 0.0004,
                profile: WidthProfile::Leaf,
                tip: TipShape::Pointed,
                ..RibbonGeometry::default()
            };
            output.emit(
                candidate,
                EmittedMark::Ribbon {
                    root: [
                        candidate.position.u_m,
                        candidate.position.v_m,
                        ground.surface_height_m() as f64,
                    ],
                    geometry,
                    attributes: MarkAttributes {
                        maturity: draw(context, &candidate, "maturity", 0.0, 1.0),
                        moisture: ground
                            .modifiers
                            .get_or(terrain_core::ids::ModifierIndex(0), 0.5),
                        exposure: 1.0,
                        tint: draw(context, &candidate, "tint", -0.4, 0.4),
                        variation: draw(context, &candidate, "variation", 0.0, 1.0),
                    },
                    stratum: Stratum::Canopy,
                    appearance: 0,
                },
            );
        }
    }
}

/// The shortest a blade may be, as a fraction of the authored length.
const LENGTH_SCATTER_LOW: f32 = 0.55;

/// The longest. Also what [`GrassRecipe::maximum_reach_m`] is computed from, so
/// the two cannot drift.
const LENGTH_SCATTER_HIGH: f32 = 1.45;

// ---------------------------------------------------------------------------
// Wildflowers
// ---------------------------------------------------------------------------

/// Meadow wildflowers: a stem and a head.
///
/// Minimal, and honest about it: the head is an analytic disc rather than an
/// authored silhouette, so these read as *dots on stems* at any distance where
/// a real flower would be recognisable. What they are here to prove is that the
/// pipeline carries a second population — two mark kinds, its own abundance
/// channel, its own reach — without any of it being grass-shaped.
pub struct WildflowerRecipe;

impl PopulationRecipe for WildflowerRecipe {
    fn key(&self) -> RecipeKey {
        RecipeKey::new("population.wildflowers_meadow").expect("valid")
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["plant.wildflower_stem", "plant.wildflower_head"]
    }

    fn validate(&self, definition: &PopulationDef, diagnostics: &mut DiagnosticReport) {
        let at = format!("populations.{}", definition.key);
        positive_parameter(&definition.parameters, "density", &at, 6.0, diagnostics);
        positive_parameter(
            &definition.parameters,
            "stem_length_m",
            &at,
            0.30,
            diagnostics,
        );
        positive_parameter(
            &definition.parameters,
            "head_radius_m",
            &at,
            0.012,
            diagnostics,
        );
    }

    fn maximum_reach_m(&self, definition: &PopulationDef) -> f64 {
        let stem = definition
            .parameters
            .number("stem_length_m")
            .unwrap_or(0.30);
        let head = definition
            .parameters
            .number("head_radius_m")
            .unwrap_or(0.012);
        stem * 1.4 + head
    }

    fn emit(&self, context: &PopulationContext<'_>, output: &mut dyn PopulationOutput) {
        let density = context
            .definition
            .parameters
            .number("density")
            .unwrap_or(6.0);
        let stem_length = context
            .definition
            .parameters
            .number("stem_length_m")
            .unwrap_or(0.30) as f32;
        let head_radius = context
            .definition
            .parameters
            .number("head_radius_m")
            .unwrap_or(0.012) as f32;
        let cell = cell_size_for(density, CANDIDATES_PER_CELL);
        let indices = crate::population::PopulationIndices::default();
        let accept = stream("accept");

        for candidate in context.candidates(CANDIDATES_PER_CELL, cell) {
            let ground = context.ground(candidate.position);
            let abundance = context.abundance_at(&ground, &indices);
            if !context
                .seeds
                .chance(&RandomAddress::new(candidate.id, &accept), abundance as f64)
            {
                continue;
            }

            let height = ground.surface_height_m() as f64;
            let length = stem_length * draw(context, &candidate, "length", 0.7, 1.3);
            let lean = draw(context, &candidate, "lean", 0.0, 0.5);
            let azimuth = draw(context, &candidate, "azimuth", 0.0, std::f32::consts::TAU);
            let attributes = MarkAttributes {
                maturity: draw(context, &candidate, "maturity", 0.0, 1.0),
                tint: draw(context, &candidate, "tint", -1.0, 1.0),
                ..MarkAttributes::default()
            };

            output.emit(
                candidate,
                EmittedMark::Curve {
                    root: [candidate.position.u_m, candidate.position.v_m, height],
                    length_m: length,
                    azimuth_rad: azimuth,
                    bend_rad: lean,
                    radius_m: 0.0008,
                    tip_radius_m: 0.0006,
                    attributes,
                    // Emergent: a flower stands above the canopy, which is what
                    // makes it read as a flower rather than as a bright blade.
                    stratum: Stratum::Emergent,
                    appearance: 0,
                },
            );
            // The head sits at the stem's tip, which the stem's own lean puts
            // off to one side.
            let (sin_a, cos_a) = azimuth.sin_cos();
            let reach = (length * lean.sin()) as f64;
            output.emit(
                candidate,
                EmittedMark::Analytic {
                    centre: [
                        candidate.position.u_m + reach * cos_a as f64,
                        candidate.position.v_m + reach * sin_a as f64,
                        height + (length * lean.cos()) as f64,
                    ],
                    radius_m: [head_radius, head_radius],
                    height_m: head_radius * 0.4,
                    rotation_rad: azimuth,
                    attributes,
                    appearance: 1,
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rocks
// ---------------------------------------------------------------------------

/// Granite boulders.
///
/// Minimal: an analytic ellipsoid rather than the faceted silhouette the old
/// rock generator produced. The silhouette code is recoverable from
/// `pre-terrain-refactor` and is worth porting when rocks matter; what is here
/// establishes that a population can be *instanced content* — bigger than a
/// mark, sparser, with its own abundance channel — rather than vegetation.
///
/// What deliberately did not come across: `blocks_movement`, the collider
/// assumptions and the navigation cost field. Those were gameplay, and a
/// terrain framework has no opinion about whether you can walk through a rock.
pub struct RockRecipe;

impl PopulationRecipe for RockRecipe {
    fn key(&self) -> RecipeKey {
        RecipeKey::new("population.granite_rocks").expect("valid")
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["rock.granite"]
    }

    fn validate(&self, definition: &PopulationDef, diagnostics: &mut DiagnosticReport) {
        let at = format!("populations.{}", definition.key);
        positive_parameter(&definition.parameters, "density", &at, 0.15, diagnostics);
        positive_parameter(&definition.parameters, "radius_m", &at, 0.25, diagnostics);
    }

    fn maximum_reach_m(&self, definition: &PopulationDef) -> f64 {
        definition.parameters.number("radius_m").unwrap_or(0.25) * 2.0
    }

    fn emit(&self, context: &PopulationContext<'_>, output: &mut dyn PopulationOutput) {
        let density = context
            .definition
            .parameters
            .number("density")
            .unwrap_or(0.15);
        let radius = context
            .definition
            .parameters
            .number("radius_m")
            .unwrap_or(0.25) as f32;
        let cell = cell_size_for(density, CANDIDATES_PER_CELL);
        let indices = crate::population::PopulationIndices::default();
        let accept = stream("accept");

        for candidate in context.candidates(CANDIDATES_PER_CELL, cell) {
            let ground = context.ground(candidate.position);
            let abundance = context.abundance_at(&ground, &indices);
            if !context
                .seeds
                .chance(&RandomAddress::new(candidate.id, &accept), abundance as f64)
            {
                continue;
            }
            // Non-uniform, because a boulder that is round in plan reads as a
            // ball. Two axes and a rotation is the cheapest thing that does not.
            let across = radius * draw(context, &candidate, "across", 0.6, 1.4);
            let along = radius * draw(context, &candidate, "along", 0.6, 1.4);
            output.emit(
                candidate,
                EmittedMark::Analytic {
                    centre: [
                        candidate.position.u_m,
                        candidate.position.v_m,
                        // Settled into the ground rather than resting on it: a
                        // rock sitting exactly on the surface reads as placed.
                        ground.surface_height_m() as f64 - (radius * 0.25) as f64,
                    ],
                    radius_m: [across, along],
                    height_m: radius * draw(context, &candidate, "height", 0.5, 1.1),
                    rotation_rad: draw(context, &candidate, "rotation", 0.0, std::f32::consts::PI),
                    attributes: MarkAttributes {
                        tint: draw(context, &candidate, "tint", -1.0, 1.0),
                        variation: draw(context, &candidate, "variation", 0.0, 1.0),
                        ..MarkAttributes::default()
                    },
                    appearance: 0,
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Dirt scatter
// ---------------------------------------------------------------------------

/// Pebbles and grit on bare ground.
///
/// The substrate population, and the one that will matter first at a boundary:
/// what makes a path read as a path rather than as brown grass is the loose
/// material on it. Minimal today — flat analytic discs — and enough to prove
/// that a population can key on a material other than grass.
pub struct DirtScatterRecipe;

impl PopulationRecipe for DirtScatterRecipe {
    fn key(&self) -> RecipeKey {
        RecipeKey::new("population.dirt_scatter").expect("valid")
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["rock.pebble"]
    }

    fn validate(&self, definition: &PopulationDef, diagnostics: &mut DiagnosticReport) {
        let at = format!("populations.{}", definition.key);
        positive_parameter(&definition.parameters, "density", &at, 40.0, diagnostics);
        positive_parameter(&definition.parameters, "radius_m", &at, 0.012, diagnostics);
    }

    fn maximum_reach_m(&self, definition: &PopulationDef) -> f64 {
        definition.parameters.number("radius_m").unwrap_or(0.012) * 2.0
    }

    fn emit(&self, context: &PopulationContext<'_>, output: &mut dyn PopulationOutput) {
        let density = context
            .definition
            .parameters
            .number("density")
            .unwrap_or(40.0);
        let radius = context
            .definition
            .parameters
            .number("radius_m")
            .unwrap_or(0.012) as f32;
        let cell = cell_size_for(density, CANDIDATES_PER_CELL);
        let indices = crate::population::PopulationIndices::default();
        let accept = stream("accept");

        for candidate in context.candidates(CANDIDATES_PER_CELL, cell) {
            let ground = context.ground(candidate.position);
            let abundance = context.abundance_at(&ground, &indices);
            if !context
                .seeds
                .chance(&RandomAddress::new(candidate.id, &accept), abundance as f64)
            {
                continue;
            }
            let across = radius * draw(context, &candidate, "across", 0.5, 1.5);
            output.emit(
                candidate,
                EmittedMark::Analytic {
                    centre: [
                        candidate.position.u_m,
                        candidate.position.v_m,
                        ground.surface_height_m() as f64,
                    ],
                    radius_m: [
                        across,
                        radius * draw(context, &candidate, "along", 0.5, 1.5),
                    ],
                    height_m: across * 0.3,
                    rotation_rad: draw(context, &candidate, "rotation", 0.0, std::f32::consts::PI),
                    attributes: MarkAttributes {
                        tint: draw(context, &candidate, "tint", -0.6, 0.6),
                        ..MarkAttributes::default()
                    },
                    appearance: 0,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::population::CollectedMarks;
    use terrain_core::coords::{WorldPoint, WorldRect};
    use terrain_core::document::{MaterialAffinity, ParameterObject, ParameterValue};
    use terrain_core::ids::{MaterialIndex, MaterialKey, ModifierIndex, PopulationKey};
    use terrain_core::sample::{MaterialWeightSet, ModifierSet, SampleQuery, TerrainSample};
    use terrain_core::seed::{RootSeed, SeedContext};

    fn definition(key: &str, recipe: &str) -> PopulationDef {
        PopulationDef {
            key: PopulationKey::new(key).expect("valid"),
            recipe: RecipeKey::new(recipe).expect("valid"),
            enabled: true,
            seed_stream: stream("scatter"),
            material_affinity: vec![MaterialAffinity {
                material: MaterialKey::new("grass_lush").expect("valid"),
                weight: 1.0,
            }],
            abundance_channel: None,
            parameters: ParameterObject::new(),
        }
    }

    fn ground(_: &SampleQuery) -> TerrainSample {
        TerrainSample {
            material_weights: MaterialWeightSet::solid(MaterialIndex(0)),
            modifiers: ModifierSet::from_defaults(&[1.0]),
            ..TerrainSample::default()
        }
    }

    fn grow(
        recipe: &dyn PopulationRecipe,
        definition: &PopulationDef,
        side: f64,
    ) -> CollectedMarks {
        let context = PopulationContext {
            definition,
            bounds: WorldRect::centred(WorldPoint::ORIGIN, side),
            seeds: SeedContext::new(RootSeed::new(0x8df7_82f9_5ce1_a4d4), recipe.version()),
            sample: &ground,
        };
        let mut collected = CollectedMarks::default();
        recipe.emit(&context, &mut collected);
        collected
    }

    #[test]
    fn every_registered_recipe_is_reachable_by_its_key() {
        let registry = default_registry();
        assert_eq!(registry.len(), 4);
        for key in [
            "population.grass_lush",
            "population.wildflowers_meadow",
            "population.granite_rocks",
            "population.dirt_scatter",
        ] {
            assert!(
                registry.contains(&RecipeKey::new(key).expect("valid")),
                "{key} is not registered"
            );
        }
    }

    #[test]
    fn every_recipe_binds_at_least_one_distinct_appearance() {
        // A collision would mean two families of content sharing one shader in
        // every renderer.
        let registry = default_registry();
        let mut seen: Vec<&str> = Vec::new();
        for key in registry.keys() {
            let recipe = registry
                .get(&RecipeKey::new(key).expect("valid"))
                .expect("registered");
            let appearances = recipe.appearances();
            assert!(!appearances.is_empty(), "{key} binds nothing");
            for appearance in appearances {
                assert!(
                    terrain_core::ids::AppearanceKey::new(appearance).is_ok(),
                    "{appearance} is not a usable key"
                );
                assert!(!seen.contains(&appearance), "{appearance} is bound twice");
                seen.push(appearance);
            }
        }
    }

    #[test]
    fn every_recipe_grows_something_on_ground_it_likes() {
        // A recipe that emits nothing would let every downstream assumption
        // about non-grass content go untested.
        for (key, side) in [
            ("population.grass_lush", 1.0),
            ("population.wildflowers_meadow", 8.0),
            ("population.granite_rocks", 40.0),
            ("population.dirt_scatter", 4.0),
        ] {
            let registry = default_registry();
            let recipe = registry
                .get(&RecipeKey::new(key).expect("valid"))
                .expect("registered");
            let definition = definition("p", key);
            let grown = grow(recipe, &definition, side);
            assert!(!grown.marks.is_empty(), "{key} grew nothing over {side} m");
        }
    }

    #[test]
    fn a_recipe_grows_the_same_thing_twice() {
        let registry = default_registry();
        let recipe = registry
            .get(&RecipeKey::new("population.grass_lush").expect("valid"))
            .expect("registered");
        let definition = definition("grass_population", "population.grass_lush");
        let first = grow(recipe, &definition, 1.0);
        let second = grow(recipe, &definition, 1.0);
        assert_eq!(first.marks.len(), second.marks.len());
        for ((a, _), (b, _)) in first.marks.iter().zip(&second.marks) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn turning_the_density_down_removes_candidates_without_moving_the_rest() {
        // The property candidates exist for. Density changes the acceptance
        // rate; it must not renumber or relocate the survivors.
        //
        // Density enters through the *cell size*, so halving it changes the
        // lattice — which is why this test varies the abundance channel instead,
        // which is the axis a document actually modulates at run time.
        let mut definition = definition("grass_population", "population.grass_lush");
        definition.abundance_channel =
            Some(terrain_core::ids::ModifierKey::new("vegetation_density").expect("valid"));

        let full = |_: &SampleQuery| TerrainSample {
            material_weights: MaterialWeightSet::solid(MaterialIndex(0)),
            modifiers: ModifierSet::from_defaults(&[1.0]),
            ..TerrainSample::default()
        };
        let half = |_: &SampleQuery| TerrainSample {
            material_weights: MaterialWeightSet::solid(MaterialIndex(0)),
            modifiers: ModifierSet::from_defaults(&[0.5]),
            ..TerrainSample::default()
        };

        let run = |sample: &dyn Fn(&SampleQuery) -> TerrainSample| {
            let indices = crate::population::PopulationIndices {
                material_affinity: Vec::new(),
                abundance_channel: Some(ModifierIndex(0)),
            };
            let context = PopulationContext {
                definition: &definition,
                bounds: WorldRect::centred(WorldPoint::ORIGIN, 1.0),
                seeds: SeedContext::new(RootSeed::new(7), 1),
                sample,
            };
            // Walk the candidates the way the recipe does, so the comparison is
            // against the acceptance rather than against the emission.
            let accept = stream("accept");
            context
                .candidates(
                    CANDIDATES_PER_CELL,
                    cell_size_for(400.0, CANDIDATES_PER_CELL),
                )
                .into_iter()
                .filter(|candidate| {
                    let abundance = context
                        .abundance_at(&sample(&SampleQuery::at(candidate.position)), &indices);
                    context
                        .seeds
                        .chance(&RandomAddress::new(candidate.id, &accept), abundance as f64)
                })
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };

        let dense = run(&full);
        let sparse = run(&half);
        assert!(
            sparse.len() < dense.len(),
            "the density did not thin anything"
        );
        for id in &sparse {
            assert!(
                dense.contains(id),
                "{id} survives at half density and not at full — the survivors moved"
            );
        }
    }

    #[test]
    fn every_recipe_bounds_its_own_reach() {
        // A mark rooted just outside a region still shades and occludes inward.
        // Getting this wrong in the small direction is a bright seam at every
        // page edge.
        let registry = default_registry();
        for key in registry.keys() {
            let recipe = registry
                .get(&RecipeKey::new(key).expect("valid"))
                .expect("registered");
            let definition = definition("p", key);
            let reach = recipe.maximum_reach_m(&definition);
            assert!(reach > 0.0 && reach.is_finite(), "{key} reaches {reach}");
            assert!(
                reach < 10.0,
                "{key} claims a {reach} m reach, which is a page"
            );
        }
    }

    #[test]
    fn a_grass_blades_reach_bounds_its_own_longest_blade() {
        // The bound has to be genuine rather than typical, so it is computed
        // from the same constant the scatter draws against.
        let definition = definition("grass_population", "population.grass_lush");
        let reach = GrassRecipe.maximum_reach_m(&definition);
        let grown = grow(&GrassRecipe, &definition, 1.0);
        for (_, mark) in &grown.marks {
            let EmittedMark::Ribbon { geometry, .. } = mark else {
                continue;
            };
            assert!(
                geometry.length_m as f64 <= reach + 1.0e-9,
                "a blade {} m long against a {reach} m reach",
                geometry.length_m
            );
        }
    }

    #[test]
    fn a_recipe_reports_a_bad_parameter_rather_than_using_it() {
        let mut definition = definition("grass_population", "population.grass_lush");
        definition
            .parameters
            .insert("density", ParameterValue::Number(-1.0));
        let mut diagnostics = DiagnosticReport::new();
        GrassRecipe.validate(&definition, &mut diagnostics);
        assert!(diagnostics.has_errors(), "{diagnostics}");
    }

    #[test]
    fn a_wildflower_is_a_stem_and_a_head() {
        // Two mark kinds from one candidate, which is what proves the pipeline
        // is not grass-shaped.
        let definition = definition("meadow_flowers", "population.wildflowers_meadow");
        let grown = grow(&WildflowerRecipe, &definition, 8.0);
        let curves = grown
            .marks
            .iter()
            .filter(|(_, m)| matches!(m, EmittedMark::Curve { .. }))
            .count();
        let heads = grown
            .marks
            .iter()
            .filter(|(_, m)| matches!(m, EmittedMark::Analytic { .. }))
            .count();
        assert!(curves > 0);
        assert_eq!(curves, heads, "every stem should carry exactly one head");
    }

    #[test]
    fn a_flower_stands_above_the_canopy() {
        let definition = definition("meadow_flowers", "population.wildflowers_meadow");
        for (_, mark) in grow(&WildflowerRecipe, &definition, 8.0).marks {
            if let EmittedMark::Curve { stratum, .. } = mark {
                assert_eq!(stratum, Stratum::Emergent);
            }
        }
    }

    #[test]
    fn a_rock_settles_into_the_ground_rather_than_resting_on_it() {
        // A rock sitting exactly on the surface reads as placed, which is the
        // tell that gives a scatter away faster than any spacing artefact.
        let definition = definition("granite_rocks", "population.granite_rocks");
        let grown = grow(&RockRecipe, &definition, 40.0);
        assert!(!grown.marks.is_empty());
        for (_, mark) in &grown.marks {
            let EmittedMark::Analytic { centre, .. } = mark else {
                continue;
            };
            assert!(centre[2] < 0.0, "a rock is resting on the surface");
        }
    }

    #[test]
    fn a_rock_is_not_round_in_plan() {
        // A boulder that is round in plan reads as a ball.
        let definition = definition("granite_rocks", "population.granite_rocks");
        let grown = grow(&RockRecipe, &definition, 60.0);
        let mut eccentric = 0;
        for (_, mark) in &grown.marks {
            if let EmittedMark::Analytic { radius_m, .. } = mark
                && (radius_m[0] - radius_m[1]).abs() > radius_m[0] * 0.05
            {
                eccentric += 1;
            }
        }
        assert!(
            eccentric * 2 > grown.marks.len(),
            "only {eccentric} of {} rocks are eccentric",
            grown.marks.len()
        );
    }

    #[test]
    fn a_document_naming_an_unregistered_recipe_is_caught() {
        // The end the registry exists for. A binary that knows only grass must
        // refuse a document that asks for rocks, rather than growing nothing and
        // saying nothing.
        let mut partial = PopulationRegistry::new();
        partial.register(Box::new(GrassRecipe));

        let document = terrain_core::document::TerrainDocument {
            materials: vec![terrain_core::document::MaterialDef {
                key: MaterialKey::new("grass_lush").expect("valid"),
                display_name: "Lush Grass".into(),
                appearance: terrain_core::ids::AppearanceKey::new("surface.grass_lush")
                    .expect("valid"),
            }],
            layers: vec![terrain_core::document::LayerDef {
                key: terrain_core::ids::LayerKey::new("base_grass").expect("valid"),
                enabled: true,
                mask: terrain_core::document::Mask::Everywhere,
                operation: terrain_core::document::LayerOperation::Material(
                    terrain_core::document::MaterialLayer {
                        material: MaterialKey::new("grass_lush").expect("valid"),
                        mode: terrain_core::document::MaterialMode::Replace,
                        amount: 1.0,
                    },
                ),
            }],
            populations: vec![definition("granite_rocks", "population.granite_rocks")],
            ..terrain_core::document::TerrainDocument::default()
        };

        let report = terrain_core::validate::validate_against(&document, &partial.known());
        assert!(
            report.entries().iter().any(|e| e.code == "unknown_recipe"),
            "{report}"
        );
        // And the full registry accepts it.
        let full = terrain_core::validate::validate_against(&document, &default_registry().known());
        assert!(
            !full.entries().iter().any(|e| e.code == "unknown_recipe"),
            "{full}"
        );
    }
}

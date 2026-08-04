//! Recipes: code a document can name, and the registry that finds it.
//!
//! ## The renderer has no `render_wildflowers`
//!
//! That sentence is the whole design. A recipe emits **primitives** — ribbons,
//! curves, analytic shapes, stamps, instances — and every renderer knows only
//! those five. Grass is a recipe that emits ribbons; wildflowers are a recipe
//! that emits curves and stamps; rocks are a recipe that emits instances. None
//! of them is a case in a match statement anywhere downstream.
//!
//! The alternative degenerates predictably. Add a method per content type and
//! the renderer grows one per ecological category, each one duplicating most of
//! the last; then a recipe that wants a blade *and* a seed head has nowhere to
//! put the second, and the answer is a sixth method.
//!
//! ## A recipe declares its reach before it emits anything
//!
//! [`PopulationRecipe::maximum_reach_m`] is asked *first*, and it is what a bake
//! sizes its halo from. A mark rooted just outside a region still shades and
//! occludes inward, so a region generated to its own edge has a bright seam
//! there. Getting the reach wrong in the small direction is that seam; getting
//! it wrong in the large direction is only wasted work.
//!
//! It has to be a genuine bound rather than a typical value, which is why it
//! takes the definition: a recipe whose parameters can double a blade's length
//! has to say so before anybody asks it for blades.
//!
//! ## Candidates, not counts
//!
//! A recipe does not decide how many things to make. It walks the **candidates**
//! its cells offer — each with a stable identity that exists whether or not
//! anything is grown there — and accepts or rejects each one. That is what lets
//! a density change move the acceptance rate without moving the survivors, and
//! it is the mechanism that will later let two materials share one candidate
//! field instead of each generating a full set and doubling the marks in a
//! transition.

use std::collections::BTreeMap;

use terrain_core::coords::{CellGrid, WorldPoint, WorldRect};
use terrain_core::diagnostics::{DiagnosticReport, Location};
use terrain_core::document::{ParameterObject, PopulationDef};
use terrain_core::ids::{RecipeKey, StreamKey};
use terrain_core::sample::{SampleChannels, SampleQuery, TerrainSample};
use terrain_core::seed::{CandidateId, PopulationHash, RandomAddress, SeedContext};

/// One potential piece of content, before anything about it is decided.
///
/// The identity exists whether or not the candidate is accepted, which is the
/// property everything else rests on: rejecting one does not renumber the rest,
/// so changing a density moves the acceptance rate and not the survivors.
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub id: CandidateId,
    /// Where it sits, jittered inside its own cell.
    pub position: WorldPoint,
}

/// Everything a recipe is given.
pub struct PopulationContext<'a> {
    pub definition: &'a PopulationDef,
    /// The region to fill, already grown by the recipe's own reach.
    pub bounds: WorldRect,
    pub seeds: SeedContext,
    /// Sample the terrain at a point.
    pub sample: &'a dyn Fn(&SampleQuery) -> TerrainSample,
}

impl PopulationContext<'_> {
    /// The terrain at a point, for a scatter's purposes.
    pub fn ground(&self, at: WorldPoint) -> TerrainSample {
        (self.sample)(&SampleQuery::at(at).with_channels(SampleChannels::SCATTER))
    }

    /// Every candidate this population offers over `bounds`.
    ///
    /// Ordered by cell and then by rank, so two runs walk them identically. The
    /// order is not merely tidy: a recipe that accepted the first *n* it liked
    /// would produce different content from an unordered walk, and a threaded
    /// build would produce different content from itself.
    pub fn candidates(&self, per_cell: u16, cell_m: f64) -> Vec<Candidate> {
        let population = PopulationHash::of(&self.definition.key);
        let grid = CellGrid::new(cell_m);
        let jitter_u = StreamKey::new("candidate_u").expect("valid");
        let jitter_v = StreamKey::new("candidate_v").expect("valid");

        let mut candidates = Vec::new();
        for cell in grid.cells_over(self.bounds) {
            let rect = grid.cell_rect(cell);
            for rank in 0..per_cell {
                let id = CandidateId::new(population, cell, rank);
                // Jittered inside its own cell, so the lattice does not show.
                let u = self.seeds.unit(&RandomAddress::new(id, &jitter_u));
                let v = self.seeds.unit(&RandomAddress::new(id, &jitter_v));
                candidates.push(Candidate {
                    id,
                    position: WorldPoint::new(
                        rect.min.u_m + u * rect.width_m(),
                        rect.min.v_m + v * rect.height_m(),
                    ),
                });
            }
        }
        candidates
    }

    /// How readily this population grows on the ground at a point.
    ///
    /// Material affinity times the abundance channel, which is the composition
    /// the whole document model is arranged around: a layer decides what the
    /// ground *is*, and a population decides how much it wants that.
    pub fn abundance_at(&self, sample: &TerrainSample, indices: &PopulationIndices) -> f32 {
        let affinity = if indices.material_affinity.is_empty() {
            1.0
        } else {
            indices
                .material_affinity
                .iter()
                .map(|(material, weight)| sample.material_weights.weight_of(*material) * weight)
                .sum()
        };
        let abundance = match indices.abundance_channel {
            Some(channel) => sample.modifiers.get_or(channel, 1.0),
            None => 1.0,
        };
        (affinity * abundance).max(0.0)
    }
}

/// A population's keys, resolved to indices once.
#[derive(Clone, Debug, Default)]
pub struct PopulationIndices {
    pub material_affinity: Vec<(terrain_core::ids::MaterialIndex, f32)>,
    pub abundance_channel: Option<terrain_core::ids::ModifierIndex>,
}

/// Where a recipe puts what it makes.
///
/// A trait rather than a concrete builder, so that this crate does not have to
/// depend on the scene builder to define the interface — and so a test can
/// collect emissions without building a scene at all.
pub trait PopulationOutput {
    /// Emit one mark.
    fn emit(&mut self, candidate: Candidate, mark: EmittedMark);
}

/// What a recipe produces, in the generic vocabulary.
///
/// Deliberately not `terrain_scene::SceneMark`: a recipe says what it wants and
/// the caller decides the stable id, the painter order and the bounds, all three
/// of which are properties of the *scene* rather than of the content. A recipe
/// that set its own painter order could put itself in front of everything.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum EmittedMark {
    Ribbon {
        root: [f64; 3],
        geometry: terrain_scene::mark::RibbonGeometry,
        attributes: terrain_scene::mark::MarkAttributes,
        stratum: terrain_scene::mark::Stratum,
        /// Which of this recipe's appearances the mark uses.
        appearance: u8,
    },
    Curve {
        root: [f64; 3],
        length_m: f32,
        azimuth_rad: f32,
        bend_rad: f32,
        radius_m: f32,
        tip_radius_m: f32,
        attributes: terrain_scene::mark::MarkAttributes,
        stratum: terrain_scene::mark::Stratum,
        appearance: u8,
    },
    Analytic {
        centre: [f64; 3],
        radius_m: [f32; 2],
        height_m: f32,
        rotation_rad: f32,
        attributes: terrain_scene::mark::MarkAttributes,
        appearance: u8,
    },
}

/// Code a document can name in a population's `recipe` field.
pub trait PopulationRecipe: Send + Sync {
    /// The key a document names this by.
    fn key(&self) -> RecipeKey;

    /// A version, mixed into every seed this recipe derives.
    ///
    /// Bump it when the recipe is *meant* to produce something different.
    /// Without it, an improvement and a regression are indistinguishable in
    /// every cache and manifest downstream.
    fn version(&self) -> u32 {
        1
    }

    /// The appearance keys this recipe's marks bind to, in the order its
    /// [`EmittedMark`]s index them.
    fn appearances(&self) -> Vec<&'static str>;

    /// Check a definition's parameters, reporting everything wrong with them.
    ///
    /// Collects rather than returning, for the same reason document validation
    /// does: an author with four bad parameters should be told about four.
    fn validate(&self, definition: &PopulationDef, diagnostics: &mut DiagnosticReport);

    /// How far outside a region this population's marks can reach into it.
    ///
    /// A genuine upper bound, taken from the definition rather than from
    /// experience. See the module note.
    fn maximum_reach_m(&self, definition: &PopulationDef) -> f64;

    /// Emit content over the context's bounds.
    fn emit(&self, context: &PopulationContext<'_>, output: &mut dyn PopulationOutput);
}

/// The population recipes a binary knows about.
///
/// Explicit, for the reasons in [`terrain_core::registry`]: with automatic
/// registration a duplicate key is resolved by link order, so the same document
/// produces different terrain depending on how the binary was built.
#[derive(Default)]
pub struct PopulationRegistry {
    recipes: BTreeMap<String, Box<dyn PopulationRecipe>>,
}

impl PopulationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a recipe, returning whether it was new.
    pub fn register(&mut self, recipe: Box<dyn PopulationRecipe>) -> bool {
        let key = recipe.key().as_str().to_string();
        if self.recipes.contains_key(&key) {
            return false;
        }
        self.recipes.insert(key, recipe);
        true
    }

    pub fn get(&self, key: &RecipeKey) -> Option<&dyn PopulationRecipe> {
        self.recipes.get(key.as_str()).map(|r| r.as_ref())
    }

    pub fn contains(&self, key: &RecipeKey) -> bool {
        self.recipes.contains_key(key.as_str())
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    /// Every registered key, in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.recipes.keys().map(|k| k.as_str())
    }

    /// The keys, as a set validation can check a document against.
    pub fn known(&self) -> terrain_core::validate::KnownRecipes {
        self.keys()
            .fold(terrain_core::validate::KnownRecipes::new(), |known, key| {
                known.with_population(RecipeKey::new(key).expect("registered keys are valid"))
            })
    }
}

impl std::fmt::Debug for PopulationRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopulationRegistry")
            .field("recipes", &self.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Read a positive number from a definition's parameters, reporting if absent
/// or unusable.
///
/// The shape every recipe's `validate` reaches for, written once so that four
/// recipes do not each invent a slightly different message.
pub fn positive_parameter(
    parameters: &ParameterObject,
    name: &str,
    at: &str,
    default: f64,
    diagnostics: &mut DiagnosticReport,
) -> f64 {
    match parameters.number(name) {
        None => default,
        Some(value) if value.is_finite() && value > 0.0 => value,
        Some(value) => {
            diagnostics.error(
                "invalid_parameter",
                Location::at(format!("{at}.parameters.{name}")),
                format!("`{name}` is {value}; it must be finite and positive"),
            );
            default
        }
    }
}

/// A cell size that produces roughly `per_square_metre` candidates per cell.
///
/// A recipe states a density in things per square metre — which is what an
/// author can reason about — and this turns it into the lattice the candidate
/// identities are addressed on. Sized so a cell holds a handful of candidates
/// rather than one: a lattice with one candidate per cell shows its own grid
/// however hard the position is jittered, because the *count* is uniform even
/// when the placement is not.
pub fn cell_size_for(per_square_metre: f64, candidates_per_cell: u16) -> f64 {
    if !(per_square_metre.is_finite() && per_square_metre > 0.0) {
        return 1.0;
    }
    (candidates_per_cell.max(1) as f64 / per_square_metre).sqrt()
}

/// A candidate collector, for a test or a debug view.
#[derive(Default)]
pub struct CollectedMarks {
    pub marks: Vec<(CandidateId, EmittedMark)>,
}

impl PopulationOutput for CollectedMarks {
    fn emit(&mut self, candidate: Candidate, mark: EmittedMark) {
        self.marks.push((candidate.id, mark));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::document::{MaterialAffinity, ParameterObject};
    use terrain_core::ids::{MaterialIndex, ModifierIndex, PopulationKey};
    use terrain_core::sample::{MaterialWeightSet, ModifierSet};
    use terrain_core::seed::RootSeed;

    fn definition(key: &str) -> PopulationDef {
        PopulationDef {
            key: PopulationKey::new(key).expect("valid"),
            recipe: RecipeKey::new("population.grass_lush").expect("valid"),
            enabled: true,
            seed_stream: StreamKey::new("grass").expect("valid"),
            material_affinity: vec![MaterialAffinity {
                material: terrain_core::ids::MaterialKey::new("grass_lush").expect("valid"),
                weight: 1.0,
            }],
            abundance_channel: None,
            parameters: ParameterObject::new(),
        }
    }

    fn context<'a>(
        definition: &'a PopulationDef,
        bounds: WorldRect,
        sample: &'a dyn Fn(&SampleQuery) -> TerrainSample,
    ) -> PopulationContext<'a> {
        PopulationContext {
            definition,
            bounds,
            seeds: SeedContext::new(RootSeed::new(0x8df7_82f9_5ce1_a4d4), 1),
            sample,
        }
    }

    fn flat(_: &SampleQuery) -> TerrainSample {
        TerrainSample {
            material_weights: MaterialWeightSet::solid(MaterialIndex(0)),
            modifiers: ModifierSet::from_defaults(&[1.0]),
            ..TerrainSample::default()
        }
    }

    #[test]
    fn candidates_are_the_same_wherever_the_region_is_cut() {
        // The property everything rests on. A candidate's identity and position
        // come from its cell, so the same ground offers the same candidates
        // whether it is asked for on its own or as part of something larger.
        let definition = definition("grass_population");
        let whole = context(
            &definition,
            WorldRect::new(WorldPoint::new(0.0, 0.0), WorldPoint::new(2.0, 2.0)),
            &flat,
        )
        .candidates(4, 0.5);
        let quarter = context(
            &definition,
            WorldRect::new(WorldPoint::new(0.0, 0.0), WorldPoint::new(1.0, 1.0)),
            &flat,
        )
        .candidates(4, 0.5);

        assert!(!quarter.is_empty());
        for candidate in &quarter {
            let found = whole
                .iter()
                .find(|c| c.id == candidate.id)
                .expect("the smaller region's candidates are a subset");
            assert_eq!(
                found.position, candidate.position,
                "{:?} moved",
                candidate.id
            );
        }
    }

    #[test]
    fn candidates_are_ordered_by_cell_and_rank() {
        // A recipe that accepted the first n it liked would otherwise produce
        // different content from an unordered walk, and a threaded build would
        // produce different content from itself.
        let definition = definition("grass_population");
        let bounds = WorldRect::new(WorldPoint::new(-1.0, -1.0), WorldPoint::new(1.0, 1.0));
        let first = context(&definition, bounds, &flat).candidates(3, 0.5);
        let second = context(&definition, bounds, &flat).candidates(3, 0.5);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.position, b.position);
        }
        // Ranks ascend within a cell.
        let ranks: Vec<u16> = first.iter().take(3).map(|c| c.id.rank).collect();
        assert_eq!(ranks, [0, 1, 2]);
    }

    #[test]
    fn every_candidate_lands_inside_its_own_cell() {
        // A candidate outside its cell would be generated by one region and
        // claimed by another, which is a mark that appears twice or not at all.
        let definition = definition("grass_population");
        let bounds = WorldRect::new(WorldPoint::new(-2.0, -2.0), WorldPoint::new(2.0, 2.0));
        let grid = CellGrid::new(0.5);
        for candidate in context(&definition, bounds, &flat).candidates(4, 0.5) {
            assert!(
                grid.cell_rect(candidate.id.cell)
                    .contains(candidate.position),
                "{:?} at {} is outside its cell",
                candidate.id,
                candidate.position
            );
        }
    }

    #[test]
    fn two_populations_offer_different_candidates_in_the_same_cells() {
        let grass = definition("grass_population");
        let flowers = definition("meadow_flowers");
        let bounds = WorldRect::new(WorldPoint::ORIGIN, WorldPoint::new(1.0, 1.0));
        let a = context(&grass, bounds, &flat).candidates(2, 0.5);
        let b = context(&flowers, bounds, &flat).candidates(2, 0.5);
        assert_eq!(a.len(), b.len());
        assert!(
            a.iter().zip(&b).all(|(x, y)| x.position != y.position),
            "two populations landed on top of each other"
        );
    }

    #[test]
    fn abundance_multiplies_affinity_by_the_channel() {
        // The composition the document model is arranged around: a layer decides
        // what the ground is, and a population decides how much it wants that.
        let definition = definition("grass_population");
        let indices = PopulationIndices {
            material_affinity: vec![(MaterialIndex(0), 1.0)],
            abundance_channel: Some(ModifierIndex(0)),
        };
        let context = context(
            &definition,
            WorldRect::centred(WorldPoint::ORIGIN, 1.0),
            &flat,
        );

        let full = TerrainSample {
            material_weights: MaterialWeightSet::solid(MaterialIndex(0)),
            modifiers: ModifierSet::from_defaults(&[1.0]),
            ..TerrainSample::default()
        };
        assert!((context.abundance_at(&full, &indices) - 1.0).abs() < 1.0e-6);

        // Suppressed by the channel.
        let suppressed = TerrainSample {
            modifiers: ModifierSet::from_defaults(&[0.15]),
            ..full.clone()
        };
        assert!((context.abundance_at(&suppressed, &indices) - 0.15).abs() < 1.0e-6);

        // On ground that is only a quarter this material.
        let sparse = TerrainSample {
            material_weights: MaterialWeightSet::from_scores([
                (MaterialIndex(0), 1.0),
                (MaterialIndex(1), 3.0),
            ]),
            ..full.clone()
        };
        assert!((context.abundance_at(&sparse, &indices) - 0.25).abs() < 1.0e-5);
    }

    #[test]
    fn a_population_with_no_affinity_grows_anywhere() {
        let definition = definition("granite_rocks");
        let context = context(
            &definition,
            WorldRect::centred(WorldPoint::ORIGIN, 1.0),
            &flat,
        );
        let indices = PopulationIndices::default();
        let sample = TerrainSample {
            material_weights: MaterialWeightSet::solid(MaterialIndex(7)),
            ..TerrainSample::default()
        };
        assert_eq!(context.abundance_at(&sample, &indices), 1.0);
    }

    #[test]
    fn a_cell_holds_several_candidates_rather_than_one() {
        // A lattice with one candidate per cell shows its own grid however hard
        // the position is jittered, because the *count* is uniform even when the
        // placement is not.
        let cell = cell_size_for(400.0, 8);
        assert!((cell - (8.0f64 / 400.0).sqrt()).abs() < 1.0e-12);
        // Eight candidates in a cell of that size is four hundred a square metre.
        assert!((8.0 / (cell * cell) - 400.0).abs() < 1.0e-6);
        // And a nonsense density does not produce a nonsense lattice.
        assert_eq!(cell_size_for(0.0, 8), 1.0);
        assert_eq!(cell_size_for(f64::NAN, 8), 1.0);
    }

    #[test]
    fn a_duplicate_recipe_is_refused() {
        struct Stub(RecipeKey);
        impl PopulationRecipe for Stub {
            fn key(&self) -> RecipeKey {
                self.0.clone()
            }
            fn appearances(&self) -> Vec<&'static str> {
                vec!["plant.grass_blade"]
            }
            fn validate(&self, _: &PopulationDef, _: &mut DiagnosticReport) {}
            fn maximum_reach_m(&self, _: &PopulationDef) -> f64 {
                0.5
            }
            fn emit(&self, _: &PopulationContext<'_>, _: &mut dyn PopulationOutput) {}
        }
        let mut registry = PopulationRegistry::new();
        let key = RecipeKey::new("population.grass_lush").expect("valid");
        assert!(registry.register(Box::new(Stub(key.clone()))));
        assert!(!registry.register(Box::new(Stub(key.clone()))));
        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&key));
    }

    #[test]
    fn a_bad_parameter_is_reported_and_falls_back() {
        let mut parameters = ParameterObject::new();
        parameters.insert(
            "density",
            terrain_core::document::ParameterValue::Number(-3.0),
        );
        let mut diagnostics = DiagnosticReport::new();
        let value = positive_parameter(
            &parameters,
            "density",
            "populations[0]",
            400.0,
            &mut diagnostics,
        );
        assert_eq!(value, 400.0);
        assert!(diagnostics.has_errors());

        // An absent parameter is the default and not an error.
        let mut clean = DiagnosticReport::new();
        assert_eq!(
            positive_parameter(&ParameterObject::new(), "density", "p", 400.0, &mut clean),
            400.0
        );
        assert!(clean.is_empty());
    }
}

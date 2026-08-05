//! What a recipe is, in the shared-candidate world.
//!
//! ## The difference from the older interface
//!
//! [`crate::population::PopulationRecipe`] generates its own candidates and
//! decides its own acceptance. That is the right shape for one population on its
//! own ground and the wrong shape the moment two of them meet, because each one
//! scattering privately is exactly what doubles the density at a boundary.
//!
//! A [`TerrainRecipe`] is handed a candidate that has **already** been accepted
//! and **already** been assigned to it. It cannot decline, cannot scatter, and
//! cannot see the lattice. All it does is answer "given that something grows
//! here and it is mine, what is it?"
//!
//! That is a smaller job, and the smallness is the point: acceptance and
//! ownership are enforced by the compiler rather than left to each recipe to
//! implement the same way.
//!
//! ## What a recipe still owns
//!
//! - the **domain** it draws from, and that domain's capacity;
//! - the **density** it wants, per square metre;
//! - its **reach**, as a genuine upper bound, because the halo is sized from it;
//! - the **primitives** it emits, and their intrinsic attributes.
//!
//! It does not own a painter order, a stable id, its own bounds, or a material
//! index. Those are properties of the scene — see [`crate::compiler`].
//!
//! ## Recipes emit primitives, never content types
//!
//! There is no `render_wildflowers` anywhere downstream and there must not be.
//! A recipe emits ribbons, curves and analytic shapes; a renderer knows only
//! those. The alternative degenerates predictably into one renderer method per
//! ecological category, each duplicating most of the last.

use terrain_core::diagnostics::DiagnosticReport;
use terrain_core::document::ParameterObject;
use terrain_core::ids::{DomainKey, PopulationKey, RecipeKey};
use terrain_core::seed::SeedContext;
use terrain_scene::field::TerrainFieldStack;

use crate::domain::{CandidateDomainDef, DomainCandidate};
use crate::ground::{GroundEvaluator, GroundSample};
use crate::population::EmittedMark;
use crate::transition::RealisedSubstrate;
use crate::tuned::RecipeRenderClass;

/// Where a recipe puts what it makes.
///
/// A trait rather than the scene builder, so a test can collect emissions
/// without building a scene and so this crate's interface does not depend on
/// how the compiler happens to lower them.
pub trait RecipeOutput {
    fn emit(&mut self, mark: EmittedMark);

    /// Declare an obstacle other content must grow around.
    ///
    /// A default no-op, so only the recipes that place obstacles implement it —
    /// which is one of six. The alternative, a `RecipeEmission` enum every
    /// recipe matches on, would make every flower's emit path carry a variant it
    /// can never produce.
    ///
    /// The footprint is the recipe's own declaration rather than something
    /// derived from its geometry afterwards. That direction matters: a stone's
    /// physical footprint is a *conservative* ellipse it guarantees to stay
    /// inside, and recovering one from a mesh after the fact would be an
    /// estimate that is sometimes smaller than the object.
    fn emit_interaction(&mut self, _interaction: EmittedInteraction) {}
}

/// An obstacle a recipe is declaring, in world terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmittedInteraction {
    /// Centre on the ground, world metres.
    pub centre: [f64; 2],
    pub semi_u_m: f32,
    pub semi_v_m: f32,
    pub yaw_rad: f32,
    /// Inside this, a root is refused outright.
    pub hard_clearance_m: f32,
    /// How far past the hard boundary the smooth response reaches.
    pub response_reach_m: f32,
    /// Which passes it affects.
    pub channels: terrain_scene::scene::InteractionChannels,
}

/// Everything a recipe is told about where it is growing.
pub struct RecipeContext<'a> {
    /// The matrix, for anything the recipe wants to read: moisture, wetness,
    /// slope, curvature, exposure, flow.
    pub fields: &'a TerrainFieldStack,
    /// The one ground evaluator, for anything the matrix does not answer.
    ///
    /// A recipe should reach for [`RecipeContext::ground_sample`] first — the
    /// compiler has already paid for one sample at this candidate and reusing
    /// it is free. This is here for the rarer case of asking about a *different*
    /// point, which a leaf tip probing away from its crown genuinely needs.
    pub ground: &'a GroundEvaluator,
    /// The ground at this candidate, sampled once by the compiler.
    ///
    /// Realised substrate, state, relief, cavity and wet film in one value. It
    /// is the same sample the ownership draw and the affinity score used, which
    /// is what keeps a recipe's idea of its own ground identical to the one that
    /// decided it should be there at all.
    pub ground_sample: &'a GroundSample,
    /// Seeded for this recipe's own version, so a change to one recipe moves
    /// only what that recipe drew.
    pub seeds: SeedContext,
    pub parameters: &'a ParameterObject,
    /// The realised substrate under this candidate — after the transition
    /// solver, so a recipe sees the ragged boundary rather than the smooth ramp.
    pub substrate: RealisedSubstrate,
    /// The height the ground Cycles renders actually sits at here.
    ///
    /// [`GroundEvaluator::final_surface_z_m`], not the matrix's surface height:
    /// authored elevation plus authored microrelief plus the profile's geometry
    /// displacement. A recipe rooting content at anything else puts it on a
    /// surface that does not exist in the rendered scene.
    pub surface_z_m: f64,
    pub root_seed: u64,
}

impl RecipeContext<'_> {
    /// A named modifier channel at a point, or a fallback.
    pub fn modifier_named(
        &self,
        _name: &str,
        _at: terrain_core::coords::WorldPoint,
        fallback: f32,
    ) -> f32 {
        // Channels reach a recipe by index through the compiler rather than by
        // name; this exists so a recipe can state a fallback in one place.
        fallback
    }
}

/// Code a document can name, in the shared-candidate pipeline.
pub trait TerrainRecipe: Send + Sync {
    /// The key a document names this by.
    fn key(&self) -> RecipeKey;

    /// Who draws what this recipe describes.
    ///
    /// Deliberately without a default. A recipe added without an answer here is
    /// a compile error, which is the only way to be sure a new grass family
    /// cannot arrive in the production image beside the tuned one — see
    /// [`crate::tuned`] for what that costs.
    fn render_class(&self) -> RecipeRenderClass;

    /// Whether this recipe's content is *vegetation*.
    ///
    /// ## Why the compiler has to be told
    ///
    /// Every population that names its materials is scaled by the document's
    /// `VegetationDensity` channel, and for a plant that is exactly right: an
    /// author who thins the grass along a path means the flowers to thin with
    /// it.
    ///
    /// For loose mineral debris it is precisely backwards. Grit lies on ground
    /// that has *nothing* growing on it, so gating it on vegetation deletes it
    /// from the only place it belongs. Measured: a comparison card whose bare
    /// stripes run at a vegetation of 0.02 placed grit on its grass edges and
    /// nowhere else, which is the opposite of the intent, and the population had
    /// no way to say so.
    ///
    /// Defaulted to true because most recipes here grow something. A recipe that
    /// scatters stone answers false.
    fn is_vegetation(&self) -> bool {
        true
    }

    /// A version, mixed into every seed this recipe derives.
    fn version(&self) -> u32 {
        1
    }

    /// Which shared lattice this recipe's content sits on.
    ///
    /// Several recipes naming one domain is the normal case and the whole
    /// mechanism: grass and dirt detail share `vegetation.tuft_anchor` so that a
    /// transition emits one thing per candidate rather than two.
    fn domain(&self) -> DomainKey;

    /// The domain's capacity, when this recipe is the one that defines it.
    ///
    /// The first recipe naming a domain supplies it. Capacity is a property of
    /// the lattice rather than of any one occupant, so two recipes sharing a
    /// domain must agree — and the way to make them agree is for it to be stated
    /// once, in the domain, rather than negotiated.
    fn domain_definition(&self) -> CandidateDomainDef;

    /// The appearance keys this recipe's marks index, in order.
    fn appearances(&self) -> Vec<&'static str>;

    /// How much of this the author asked for, per square metre.
    fn target_density(&self, parameters: &ParameterObject) -> f64;

    /// How far outside a region this recipe's marks can reach into it.
    ///
    /// A genuine upper bound taken from the parameters, because the halo is
    /// sized from it and a mark wrongly excluded is present on one side of a
    /// join and missing on the other.
    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64;

    /// Check the parameters against the soils actually in play.
    ///
    /// Separate from [`validate`](TerrainRecipe::validate) because it needs
    /// something the parameters alone cannot supply: a recipe that places what
    /// a *profile* declares has to know that the document left it room to. The
    /// default does nothing, because most recipes describe what they place
    /// entirely themselves.
    fn validate_against_profiles(
        &self,
        parameters: &ParameterObject,
        profiles: &[&terrain_core::ground_material::GroundMaterialProfile],
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        let _ = (parameters, profiles, population, diagnostics);
    }

    /// Check the parameters, reporting everything wrong with them.
    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    );

    /// Grow one candidate that has already been accepted and assigned here.
    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    );
}

/// The recipes a binary knows about, for the shared-candidate pipeline.
///
/// Explicit registration, for the reason in [`terrain_core::registry`]: with
/// automatic registration a duplicate key resolves by link order, so the same
/// document produces different terrain depending on how the binary was built.
#[derive(Default)]
pub struct TerrainRecipeRegistry {
    recipes: std::collections::BTreeMap<String, Box<dyn TerrainRecipe>>,
}

impl TerrainRecipeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a recipe, returning whether it was new.
    pub fn register(&mut self, recipe: Box<dyn TerrainRecipe>) -> bool {
        let key = recipe.key().as_str().to_string();
        if self.recipes.contains_key(&key) {
            return false;
        }
        self.recipes.insert(key, recipe);
        true
    }

    pub fn get(&self, key: &RecipeKey) -> Option<&dyn TerrainRecipe> {
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

impl std::fmt::Debug for TerrainRecipeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainRecipeRegistry")
            .field("recipes", &self.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Collect emissions, for a test or a debug view.
#[derive(Default)]
pub struct CollectedEmissions {
    pub marks: Vec<EmittedMark>,
}

impl RecipeOutput for CollectedEmissions {
    fn emit(&mut self, mark: EmittedMark) {
        self.marks.push(mark);
    }
}

//! `PreparedTerrain` → one `TerrainScene`.
//!
//! The production path, and the piece the framework was missing. Everything
//! above it — the document, the sampler, the field stack — existed; everything
//! below it — the scene, the renderers, the corpus — existed; and the arrow
//! between them was a bench fixture that converted an already-grown grass scene.
//!
//! ## The order is the design
//!
//! ```text
//! resolve populations, domains and the halo they imply
//! sample one field stack over the generated bounds
//! derive slope, curvature, flow, exposure and the boundary frame
//! generate each shared candidate domain once
//!   ├─ realise the substrate at the candidate          (transition solver)
//!   ├─ blend one target density from every claimant
//!   ├─ accept or reject, once                          (fixes the count)
//!   ├─ score every claimant and draw one owner          (fixes the identity)
//!   └─ hand the candidate to that owner's recipe
//! lower emissions into the scene, with candidate-derived ids
//! sort once, fingerprint, done
//! ```
//!
//! Acceptance happening **before** ownership is the whole reason a transition
//! does not double its density: the number of things is settled while the
//! materials are still an undecided question, so a 70/30 boundary emits exactly
//! what the pure ground on either side does.
//!
//! ## The halo is derived, never guessed
//!
//! A mark rooted outside the visible rectangle still leans into it, shades into
//! it and occludes it, and a neighbourhood term reads further still. So the
//! generated bounds are the visible bounds grown by the largest reach anything
//! needs:
//!
//! ```text
//! halo = max(recipe reach, conflict radius, flow reach, source reach)
//! ```
//!
//! Getting it wrong in the small direction is a bright seam at the frame edge.
//! Getting it wrong in the large direction is only wasted work, which is why
//! every term is an upper bound rather than a typical value.
//!
//! ## Tiles decide framing, never generation
//!
//! Nothing in here knows which tile is the subject. The compiler is handed one
//! rectangle and fills it continuously; the nine-tile layout is a *composition*
//! resolved by `terrain_scene::frame`, and it reaches this module only as the
//! bounds it asked for. That is what keeps grass crossing the internal joins and
//! shadows falling across them.

use std::collections::BTreeMap;
use std::sync::Arc;

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::diagnostics::{DiagnosticReport, Location};
use terrain_core::document::ParameterObject;
use terrain_core::ids::{DomainKey, MaterialIndex, ModifierIndex, RecipeKey};
use terrain_core::prepare::PreparedTerrain;
use terrain_core::seed::SeedContext;
use terrain_scene::derive::{DerivedFieldRequest, derive_fields, flow_reach_m, sample_fields};
use terrain_scene::field::{FieldGridSpec, TerrainFieldStack};
use terrain_scene::mark::{
    Aabb3, AnalyticMark, AnchorIndex, CurveMark, MarkId, PainterOrder, RibbonMark, SceneMark,
    SceneMaterialBinding, SceneMaterialIndex,
};
use terrain_scene::projection::ScenePoint;
use terrain_scene::scene::{PlacementAnchor, SceneBuilder, SceneRequest, TerrainScene};

use crate::domain::{
    CandidateDomainDef, DOMAIN_ALGORITHM_VERSION, DomainCandidate, DomainRequest, accepts, generate,
};
use crate::ownership::{OwnerOption, assign};
use crate::population::EmittedMark;
use crate::recipe::{RecipeContext, RecipeOutput, TerrainRecipeRegistry};
use crate::transition::TransitionProfile;
use crate::tuned::{RecipeRenderClass, TunedPass};

/// The version the compiler stamps on the scenes it builds.
///
/// Its own domain. A change to how a candidate becomes a mark must move every
/// scene fingerprint without pretending the document changed, and a document
/// change must not look like a compiler change.
pub const COMPILER_VERSION: u32 = 1;

/// How much work the compiler is asked to do.
#[derive(Clone, Debug)]
pub struct SceneCompileOptions {
    /// Metres between field-stack samples. `None` derives one from the request.
    pub field_spacing_m: Option<f64>,
    /// Which derived fields to compute.
    pub derive: DerivedFieldRequest,
    /// How the boundary between two substrates is realised.
    pub transition: TransitionProfile,
    /// Check the finished scene before returning it.
    pub validate: bool,
    /// The ground lattice to split relief bands at, when no profile chooses one.
    ///
    /// Only reached by a document whose materials carry no ground profile at
    /// all: with profiles present, [`BandSplit::spacing_for`] derives the
    /// spacing from the finest band any of them declares, which is the whole
    /// point of the three-tier ladder. A constant here would decide the ladder
    /// for materials that had already said what they need.
    pub fallback_ground_spacing_m: f32,
}

impl Default for SceneCompileOptions {
    fn default() -> Self {
        Self {
            field_spacing_m: None,
            derive: DerivedFieldRequest::PLACEMENT,
            transition: TransitionProfile::default(),
            validate: true,
            // Four centimetres: what the CLI used before this became an option,
            // preserved so that adding the field moved nothing.
            fallback_ground_spacing_m: 0.04,
        }
    }
}

/// How many field samples to put across one output pixel.
///
/// A quarter: the matrix carries the *macro* fields, and the detail below that
/// comes from the transition solver and the marks, both of which are evaluated
/// analytically rather than read off the grid. Sampling the matrix at pixel rate
/// would quadruple its cost to carry frequencies nothing reads from it.
const FIELD_SAMPLES_PER_PIXEL: f64 = 0.25;

/// The coarsest and finest the derived spacing may be, in metres.
const SPACING_BOUNDS_M: (f64, f64) = (0.005, 0.10);

/// What a compile produced.
///
/// Every member is immutable and shared. The point of returning them together
/// rather than letting each caller rebuild what it needs is that they must be
/// the *same* objects: a `GroundEvaluator` built twice from the same inputs
/// agrees to the bit, but only until somebody changes one construction site and
/// not the other — and the symptom of that is a stone floating a centimetre
/// above the mesh it is supposed to be resting in.
pub struct SceneCompilation {
    pub scene: Arc<TerrainScene>,
    pub fields: Arc<TerrainFieldStack>,
    /// The one answer about the ground, shared by the mesh that carries its
    /// relief, the shader that colours it, the overlay that decides how much
    /// grass grows on it, and every secondary root registered to it.
    pub ground: Arc<crate::ground::GroundEvaluator>,
    pub report: SceneCompileReport,
}

/// What the compile did, in numbers.
///
/// Counted rather than estimated, because these are the quality counter-metrics
/// every speed claim has to carry: an optimisation that got faster by accepting
/// fewer candidates is a quality-tier change, and nothing else in the pipeline
/// would say so.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneCompileReport {
    pub field_samples: usize,
    pub field_spacing_m: f64,
    pub halo_m: f64,
    pub candidates_generated: usize,
    pub candidates_accepted: usize,
    /// Accepted, but no recipe wanted them. Bare ground.
    pub candidates_unowned: usize,
    pub marks_emitted: usize,
    /// Per population key, so a document author can see which one went quiet.
    pub marks_by_population: BTreeMap<String, usize>,
    /// Who owns each population's rendering, by population key.
    ///
    /// Reported rather than inferred, because "why is my flower not in the
    /// picture" and "why is my grass not doubled" are the same question asked
    /// from two directions, and the answer to both is this table.
    pub render_classes: BTreeMap<String, RecipeRenderClass>,
    /// Populations declared, understood, and deliberately not drawn.
    ///
    /// A separate list rather than a filter over `render_classes`, so that a
    /// caller printing a report cannot omit it by forgetting to filter.
    pub deferred_populations: Vec<String>,
}

/// Why a compile could not finish.
#[derive(Debug)]
pub enum SceneCompileError {
    /// The document names something this binary cannot grow, or a parameter is
    /// unusable. Collected, so an author with four problems is told about four.
    Diagnostics(DiagnosticReport),
    /// The request asks for a grid that cannot be built.
    Grid(String),
}

impl std::fmt::Display for SceneCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diagnostics(report) => write!(f, "{report}"),
            Self::Grid(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SceneCompileError {}

/// One population, resolved against the registry and the prepared terrain.
struct Claimant {
    key: String,
    /// Index into the compiler's own list, and what ownership returns.
    owner: u16,
    recipe: RecipeKey,
    /// Who draws this. Only `Secondary` reaches the scene — see [`crate::tuned`].
    render_class: RecipeRenderClass,
    domain: DomainKey,
    affinity: Vec<(MaterialIndex, f32)>,
    abundance_channel: Option<ModifierIndex>,
    parameters: ParameterObject,
    density_per_m2: f64,
    reach_m: f64,
    /// Where this recipe's appearances start in the scene's binding table.
    appearance_base: u16,
}

impl Claimant {
    /// How readily this population takes ground of a given realised substrate.
    ///
    /// Empty affinity means "anywhere", which is what a rock wants: a stone does
    /// not care what grows around it.
    fn affinity_for(&self, substrate: &crate::transition::RealisedSubstrate) -> f32 {
        if self.affinity.is_empty() {
            return 1.0;
        }
        self.affinity
            .iter()
            .map(|(material, weight)| substrate.weight_of(*material) * weight)
            .sum()
    }
}

/// Compile one scene.
pub fn compile_scene(
    terrain: &PreparedTerrain,
    request: &SceneRequest,
    recipes: &TerrainRecipeRegistry,
    options: &SceneCompileOptions,
) -> Result<SceneCompilation, SceneCompileError> {
    let mut diagnostics = DiagnosticReport::new();

    // ---- Phase 1: resolve what the document asked for ----------------------
    let mut claimants: Vec<Claimant> = Vec::new();
    let mut domains: BTreeMap<String, CandidateDomainDef> = BTreeMap::new();
    let mut appearances: Vec<&'static str> = Vec::new();
    // Which population claimed each tuned pass. One each, in version 1 — see
    // below for why a merge is an error rather than a silent fold.
    let mut tuned_claims: BTreeMap<TunedPass, String> = BTreeMap::new();

    for population in terrain.populations() {
        let Some(recipe) = recipes.get(&population.recipe) else {
            diagnostics.error(
                "unknown_recipe",
                Location::at(format!("populations.{}", population.key)),
                format!(
                    "`{}` is not a recipe this build knows how to grow",
                    population.recipe
                ),
            );
            continue;
        };
        recipe.validate(&population.parameters, &population.key, &mut diagnostics);

        let render_class = recipe.render_class();
        // One population per tuned pass. The tuned generator has exactly one
        // pass identity, so folding two authored populations into it would
        // destroy persistent population identity and leave the density
        // semantics ambiguous — is the pass the sum of the two, the max, or the
        // last one resolved? A future version may define a stable merge; this
        // one says so rather than picking.
        if let Some(pass) = render_class.tuned_pass() {
            // The *first* claimant is kept and later ones are reported against
            // it. Overwriting instead would diagnose the third population
            // against the second, so a document with three claimants on one
            // pass would name a different "original" in each message and send
            // the author looking at the wrong file.
            match tuned_claims.get(&pass) {
                None => {
                    tuned_claims.insert(pass, population.key.as_str().to_string());
                }
                Some(first) => {
                    diagnostics.error(
                        "duplicate_tuned_pass",
                        Location::at(format!("populations.{}", population.key)),
                        format!(
                            "`{}` and `{first}` both drive the tuned {pass} pass; \
                         one population may control a tuned pass",
                            population.key
                        ),
                    );
                }
            }
        }

        let domain = recipe.domain();
        let definition = recipe.domain_definition();
        // A domain is one lattice, and two recipes sharing its name must agree
        // about what it is. Keeping whichever definition arrived first would
        // make the cell size — and therefore every candidate address in it —
        // depend on the order populations happen to be declared in.
        match domains.get(domain.as_str()) {
            None => {
                domains.insert(domain.as_str().to_string(), definition);
            }
            Some(existing) if *existing == definition => {}
            Some(_) => {
                diagnostics.error(
                    "conflicting_domain_definition",
                    Location::at(format!("populations.{}", population.key)),
                    format!(
                        "`{}` defines domain `{domain}` differently from another population \
                     that shares it; a domain is one lattice and its cell size decides \
                     every candidate address on it",
                        population.key
                    ),
                );
            }
        }

        let appearance_base = appearances.len() as u16;
        appearances.extend(recipe.appearances());

        claimants.push(Claimant {
            key: population.key.as_str().to_string(),
            // Provisional. Replaced below by a rank derived from the population
            // key, so that reordering declarations in a document does not
            // reassign candidates.
            owner: 0,
            recipe: population.recipe.clone(),
            render_class,
            domain,
            affinity: population.material_affinity.clone(),
            abundance_channel: population.abundance_channel,
            parameters: population.parameters.clone(),
            density_per_m2: recipe.target_density(&population.parameters),
            reach_m: recipe.maximum_reach_m(&population.parameters),
            appearance_base,
        });
    }

    // The owner index is a rank in *key* order, not declaration order.
    //
    // `ownership::assign` sorts its options by this index and lays them out as
    // consecutive intervals of one draw, so the index decides which population
    // wins a given candidate. Deriving it from declaration order would mean
    // that moving a population up a document — a pure edit, changing no value —
    // swapped the intervals and reassigned every contested candidate between
    // them. Identity here is the authored key, the same rule `terrain_core::ids`
    // states for everything else.
    {
        let ranks = owner_ranks(claimants.iter().map(|c| c.key.as_str()));
        for claimant in &mut claimants {
            claimant.owner = ranks[&claimant.key];
        }
    }

    // A shared domain must agree about who renders it. Two recipes sharing one
    // lattice share one acceptance decision, so a secondary recipe drawing from
    // the same domain as a tuned one would have its candidate count changed by
    // a population that emits nothing — an invisible density coupling between
    // the tuned canopy and the flowers. Version 1 refuses rather than defines a
    // meaning for it.
    for name in domains.keys() {
        let mut secondary: Vec<&str> = Vec::new();
        let mut other: Vec<&str> = Vec::new();
        for claimant in &claimants {
            if claimant.domain.as_str() != name {
                continue;
            }
            if claimant.render_class.emits_secondary() {
                secondary.push(&claimant.key);
            } else {
                other.push(&claimant.key);
            }
        }
        if !secondary.is_empty() && !other.is_empty() {
            diagnostics.error(
                "mixed_domain_render_classes",
                Location::at(format!("domains.{name}")),
                format!(
                    "domain `{name}` is claimed by secondary population(s) {} and \
                     non-secondary population(s) {}; one domain is one acceptance \
                     decision, so mixing them would let a population that draws \
                     nothing change how many flowers exist",
                    secondary.join(", "),
                    other.join(", "),
                ),
            );
        }
    }

    if diagnostics.has_errors() {
        return Err(SceneCompileError::Diagnostics(diagnostics));
    }

    // ---- Phase 2: the halo, from every reach that exists -------------------
    let spacing = options
        .field_spacing_m
        .unwrap_or_else(|| derive_spacing(request));
    let mut halo = request.halo_m.max(terrain.reach_m());
    for claimant in &claimants {
        halo = halo.max(claimant.reach_m);
    }
    for domain in domains.values() {
        halo = halo.max(domain.spacing.conflict_reach_m());
    }
    if options.derive.flow {
        halo = halo.max(flow_reach_m(spacing));
    }

    let generated = request.bounds.expanded(halo);
    let grid = FieldGridSpec::covering(generated, spacing);
    if !grid.is_well_formed() {
        return Err(SceneCompileError::Grid(format!(
            "a grid of {} x {} samples at {spacing} m is not buildable",
            grid.samples_across(),
            grid.samples_down()
        )));
    }

    // ---- Phase 3 and 4: the matrix, and what follows from it ---------------
    let mut fields = sample_fields(terrain, grid);
    derive_fields(&mut fields, options.derive);
    let fields = Arc::new(fields);

    // ---- Phase 4b: the one ground evaluator --------------------------------
    //
    // Built here, before a single recipe emits, and returned. Every previous
    // version of this pipeline had the CLI construct its own evaluator *after*
    // the compile, which meant secondary content was rooted at
    // `fields.surface_height` while the mesh Cycles rendered added profile
    // relief on top. The two surfaces differ by centimetres — enough to float a
    // pebble or bury a stem — and nothing anywhere reported the disagreement,
    // because from each side's own point of view it was on the ground.
    let band_spacing = crate::ground::BandSplit::spacing_for(
        (0..terrain.materials().len())
            .filter_map(|index| terrain.material_profile(MaterialIndex(index as u16)))
            .map(|profile| profile.as_ref()),
    )
    .unwrap_or(options.fallback_ground_spacing_m);
    let ground = Arc::new(crate::ground::GroundEvaluator::new(
        terrain,
        Arc::clone(&fields),
        options.transition,
        band_spacing,
    ));

    // ---- Phase 5: build the scene ------------------------------------------
    let mut builder = SceneBuilder::new(*request, terrain.document_digest(), COMPILER_VERSION);
    builder.set_ground(fields.to_ground_surface());

    // Appearances are bound up front, in recipe order, so a mark's appearance
    // index does not depend on which marks happened to be emitted first.
    let mut bound: Vec<SceneMaterialIndex> = Vec::new();
    for appearance in &appearances {
        let key = terrain_core::ids::AppearanceKey::new(*appearance)
            .expect("recipe appearance keys are valid by construction");
        bound.push(builder.bind_material(SceneMaterialBinding {
            appearance: key,
            terrain_material: None,
        }));
    }

    let root_seed = terrain.root_seed().bits();
    let mut report = SceneCompileReport {
        field_samples: grid.sample_count(),
        field_spacing_m: spacing,
        halo_m: halo,
        ..Default::default()
    };

    for claimant in &claimants {
        report
            .render_classes
            .insert(claimant.key.clone(), claimant.render_class);
        if claimant.render_class == RecipeRenderClass::Deferred {
            report.deferred_populations.push(claimant.key.clone());
        }
    }

    for (name, domain) in &domains {
        let members: Vec<&Claimant> = claimants
            .iter()
            .filter(|claimant| claimant.domain.as_str() == name)
            .collect();
        if members.is_empty() {
            continue;
        }
        // Only secondary content reaches the scene, and generating a domain
        // whose every claimant renders elsewhere is pure waste — `vegetation.fine`
        // alone offers several million candidates across a nine-tile plate, all
        // of them destined to be thinned, owned, grown into ribbons and then
        // never drawn. The domain-agreement check above has already guaranteed
        // this is all-or-nothing per domain, so testing the first member would
        // do; testing every member is the same cost and does not depend on that
        // guarantee holding.
        if !members
            .iter()
            .any(|claimant| claimant.render_class.emits_secondary())
        {
            continue;
        }

        // One seed context per domain, so a recipe version change moves that
        // recipe's content and not the lattice underneath it.
        let seeds = SeedContext::new(terrain.root_seed(), DOMAIN_ALGORITHM_VERSION);
        let candidates = generate(&DomainRequest {
            definition: domain,
            bounds: generated,
            seeds,
        });
        report.candidates_generated += candidates.len();

        let capacity = domain.max_density_per_m2();
        let mut options_buffer: Vec<OwnerOption> = Vec::with_capacity(members.len());

        for candidate in &candidates {
            // The realised substrate, from the same function the ground shading
            // calls. One answer, so a tuft stands on the mud it belongs to.
            //
            // Read straight out of the evaluator rather than realised again
            // here. The old code called `realise` with its own copy of the
            // transition profile, which agreed with the evaluator right up
            // until somebody changed one of them.
            let substrate = ground.substrates(vec2(candidate.position));

            // One blended target density from every claimant, so acceptance is
            // decided before any material owns anything.
            options_buffer.clear();
            let mut target = 0.0f64;
            for claimant in &members {
                let affinity = claimant.affinity_for(&substrate);
                let abundance = match claimant.abundance_channel {
                    Some(channel) => fields.modifier(channel, candidate.position, 1.0),
                    None => 1.0,
                };
                let want = crate::ownership::score(affinity, abundance, 1.0, 1.0);
                if want > 0.0 {
                    target += claimant.density_per_m2 * want as f64;
                    options_buffer.push(OwnerOption {
                        owner: claimant.owner,
                        score: want,
                    });
                }
            }
            if target <= 0.0 {
                continue;
            }

            if !accepts(candidate, domain, &seeds, target.min(capacity)) {
                continue;
            }
            report.candidates_accepted += 1;

            let Some(owner) = assign(candidate, &mut options_buffer, &seeds) else {
                report.candidates_unowned += 1;
                continue;
            };
            let Some(claimant) = claimants.iter().find(|c| c.owner == owner) else {
                report.candidates_unowned += 1;
                continue;
            };
            let Some(recipe) = recipes.get(&claimant.recipe) else {
                report.candidates_unowned += 1;
                continue;
            };

            // One full ground sample, paid for once, only for candidates that
            // survived acceptance and found an owner. Sampling every candidate
            // would evaluate every relief band of every profile for a lattice
            // that rejects the great majority of them.
            let ground_sample = ground.sample(vec2(candidate.position));
            let context = RecipeContext {
                fields: &fields,
                ground: &ground,
                ground_sample: &ground_sample,
                seeds: seeds.for_recipe(recipe.version()),
                parameters: &claimant.parameters,
                substrate,
                // The surface the *mesh* has, not the analytic one.
                //
                // Between two mesh vertices the rendered ground is the chord and
                // the analytic surface is the curve, so a root registered to the
                // latter stands a visible gap above the former near a crest —
                // see `GroundEvaluator::mesh_surface_z_m`.
                surface_z_m: ground
                    .mesh_surface_z_m(vec2(candidate.position), ground.mesh_spacing_m())
                    as f64,
                root_seed,
            };
            // One placement group per accepted, owned candidate. Everything the
            // recipe emits below names it, so a trace slice keeps a flower's
            // stem and head together or drops both.
            let anchor = builder.bind_anchor(PlacementAnchor {
                candidate: candidate.id,
                root: ScenePoint::new(
                    candidate.position.u_m,
                    candidate.position.v_m,
                    context.surface_z_m,
                ),
            });
            let mut sink = MarkSink {
                builder: &mut builder,
                projection: request.projection,
                candidate,
                anchor,
                appearance_base: claimant.appearance_base,
                bound: &bound,
                emitted: 0,
            };
            recipe.emit(candidate, &context, &mut sink);
            let emitted = sink.emitted;
            report.marks_emitted += emitted;
            *report
                .marks_by_population
                .entry(claimant.key.clone())
                .or_default() += emitted;
        }
    }

    let scene = builder.build();

    if options.validate {
        validate_scene(&scene, &mut diagnostics);
        if diagnostics.has_errors() {
            return Err(SceneCompileError::Diagnostics(diagnostics));
        }
    }

    Ok(SceneCompilation {
        scene: Arc::new(scene),
        fields,
        ground,
        report,
    })
}

/// A world point as the flat vector the ground evaluator speaks.
///
/// The evaluator works in `f32` because its noise does; the field stack works
/// in `f64` because world positions do. One conversion function rather than a
/// cast at each call site, so the narrowing is visible and happens in exactly
/// one place.
fn vec2(at: WorldPoint) -> glam::Vec2 {
    glam::Vec2::new(at.u_m as f32, at.v_m as f32)
}

/// Rank every population key, so ownership does not depend on file order.
///
/// A `BTreeMap` collects in key order by construction, so the enumeration is
/// the rank. Extracted from the compile so it can be tested without preparing a
/// document: the property it carries — that ranks come from the keys and not
/// from the sequence — is exactly the kind of thing that is easy to reintroduce
/// by writing `claimants.len()` back into the loop.
fn owner_ranks<'a>(keys: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, u16> {
    let sorted: std::collections::BTreeSet<&str> = keys.into_iter().collect();
    sorted
        .into_iter()
        .enumerate()
        .map(|(rank, key)| (key.to_string(), rank as u16))
        .collect()
}

/// A field spacing that resolves what the output can show.
fn derive_spacing(request: &SceneRequest) -> f64 {
    let pixels_per_metre = request.effective_pixels_per_metre().max(1.0) as f64;
    (1.0 / (pixels_per_metre * FIELD_SAMPLES_PER_PIXEL))
        .clamp(SPACING_BOUNDS_M.0, SPACING_BOUNDS_M.1)
}

/// Turn a recipe's emissions into scene marks.
///
/// The compiler assigns the stable id, the painter order, the material index and
/// the bounds — all four are properties of the *scene* rather than of the
/// content, and a recipe that chose its own painter order could put itself in
/// front of everything.
struct MarkSink<'a> {
    builder: &'a mut SceneBuilder,
    projection: terrain_scene::projection::Projection,
    candidate: &'a DomainCandidate,
    /// The placement group everything this candidate grows belongs to.
    ///
    /// Bound before the recipe runs rather than on its first emission, so a
    /// recipe that emits nothing still has an identity — which is what lets a
    /// report distinguish "no candidate here" from "a candidate that chose to
    /// grow nothing".
    anchor: AnchorIndex,
    appearance_base: u16,
    bound: &'a [SceneMaterialIndex],
    emitted: usize,
}

impl MarkSink<'_> {
    fn material(&self, appearance: u8) -> SceneMaterialIndex {
        let slot = self.appearance_base as usize + appearance as usize;
        self.bound
            .get(slot)
            .copied()
            .or_else(|| self.bound.first().copied())
            .unwrap_or(SceneMaterialIndex(0))
    }
}

impl RecipeOutput for MarkSink<'_> {
    fn emit(&mut self, mark: EmittedMark) {
        // The part index is the count so far *for this candidate*, which is
        // what makes the id a function of identity rather than of how many
        // marks the scene already held.
        let part = self.emitted as u16;
        let id = MarkId::of(self.candidate.id, part);
        let scene_mark = match mark {
            EmittedMark::Ribbon {
                root,
                geometry,
                attributes,
                stratum,
                appearance,
            } => {
                let root = ScenePoint::new(root[0], root[1], root[2]);
                SceneMark::Ribbon(RibbonMark {
                    stable_id: id,
                    anchor: self.anchor,
                    order: PainterOrder::at(stratum, self.projection, root, 0, id),
                    material: self.material(appearance),
                    root,
                    geometry,
                    attributes,
                    bounds: Aabb3::around(root, geometry.reach_m() as f64),
                })
            }
            EmittedMark::Curve {
                root,
                length_m,
                azimuth_rad,
                bend_rad,
                radius_m,
                tip_radius_m,
                attributes,
                stratum,
                appearance,
            } => {
                let root = ScenePoint::new(root[0], root[1], root[2]);
                SceneMark::Curve(CurveMark {
                    stable_id: id,
                    anchor: self.anchor,
                    order: PainterOrder::at(stratum, self.projection, root, 0, id),
                    material: self.material(appearance),
                    root,
                    length_m,
                    azimuth_rad,
                    bend_rad,
                    radius_m,
                    tip_radius_m,
                    attributes,
                    bounds: Aabb3::around(root, (length_m + radius_m) as f64),
                })
            }
            EmittedMark::Analytic {
                centre,
                radius_m,
                height_m,
                rotation_rad,
                attributes,
                appearance,
            } => {
                let centre = ScenePoint::new(centre[0], centre[1], centre[2]);
                let reach = radius_m[0].max(radius_m[1]).max(height_m) as f64;
                SceneMark::Analytic(AnalyticMark {
                    stable_id: id,
                    anchor: self.anchor,
                    order: PainterOrder::at(
                        terrain_scene::mark::Stratum::Ground,
                        self.projection,
                        centre,
                        0,
                        id,
                    ),
                    material: self.material(appearance),
                    centre,
                    radius_m,
                    height_m,
                    rotation_rad,
                    attributes,
                    bounds: Aabb3::around(centre, reach),
                })
            }
        };
        self.builder.push_mark(scene_mark);
        self.emitted += 1;
    }
}

/// Check a finished scene for the things that are silent when wrong.
fn validate_scene(scene: &TerrainScene, diagnostics: &mut DiagnosticReport) {
    if !scene.is_sorted() {
        diagnostics.error(
            "scene_unsorted",
            Location::at("scene.marks"),
            "marks are not in painter order".to_string(),
        );
    }
    let materials = scene.materials.len();
    let mut non_finite = 0usize;
    let mut bad_material = 0usize;
    for mark in &scene.marks {
        let root = mark.root();
        if !(root.u_m.is_finite() && root.v_m.is_finite() && root.z_m.is_finite()) {
            non_finite += 1;
        }
        if mark.material().0 as usize >= materials {
            bad_material += 1;
        }
    }
    if non_finite > 0 {
        diagnostics.error(
            "non_finite_geometry",
            Location::at("scene.marks"),
            format!("{non_finite} marks have a non-finite root"),
        );
    }
    if bad_material > 0 {
        diagnostics.error(
            "unbound_appearance",
            Location::at("scene.marks"),
            format!("{bad_material} marks name an appearance the scene has not bound"),
        );
    }

    // A dangling index is silent at every layer that carries it and loud only
    // at the renderer, which by then has no idea which recipe produced it.
    let anchors = scene.anchors.len();
    let dangling_marks = scene
        .marks
        .iter()
        .filter(|mark| mark.anchor().index() >= anchors)
        .count();
    if dangling_marks > 0 {
        diagnostics.error(
            "dangling_anchor",
            Location::at("scene.marks"),
            format!("{dangling_marks} marks name a placement group the scene has no entry for"),
        );
    }

    let prototypes = scene.prototypes.len();
    let dangling_instances = scene
        .instances
        .iter()
        .filter(|instance| {
            instance.prototype.0 as usize >= prototypes || instance.anchor.index() >= anchors
        })
        .count();
    if dangling_instances > 0 {
        diagnostics.error(
            "dangling_instance_reference",
            Location::at("scene.instances"),
            format!(
                "{dangling_instances} instances name a prototype or placement group \
                 the scene has no entry for"
            ),
        );
    }

    // A prototype key claimed twice with different geometry is a validation
    // error rather than "last one wins": resolving it by order would make the
    // shape depend on which recipe was traversed first.
    let mut by_key: BTreeMap<(&str, u64), usize> = BTreeMap::new();
    let mut collisions = 0usize;
    for binding in &scene.prototypes {
        let slot = by_key
            .entry((binding.recipe.as_str(), binding.seed))
            .or_insert(0);
        *slot += 1;
        if *slot > 1 {
            collisions += 1;
        }
    }
    if collisions > 0 {
        diagnostics.error(
            "prototype_key_collision",
            Location::at("scene.prototypes"),
            format!(
                "{collisions} prototype bindings share a recipe and seed but describe \
                 different geometry; two shapes cannot answer to one name"
            ),
        );
    }

    let mut bad_interactions = 0usize;
    for interaction in &scene.interactions {
        if interaction.anchor.index() >= anchors {
            bad_interactions += 1;
            continue;
        }
        if interaction.hard_clearance_m < 0.0 || interaction.response_reach_m < 0.0 {
            bad_interactions += 1;
            continue;
        }
        // A wildcard arm because the shape enum is `non_exhaustive`: a shape
        // added later must not silently pass this check, so anything this
        // build does not recognise is counted as bad rather than as fine.
        match interaction.shape {
            terrain_scene::scene::InteractionShape::Ellipse {
                semi_u_m, semi_v_m, ..
            } if semi_u_m > 0.0 && semi_v_m > 0.0 => {}
            _ => bad_interactions += 1,
        }
    }
    if bad_interactions > 0 {
        diagnostics.error(
            "invalid_interaction",
            Location::at("scene.interactions"),
            format!(
                "{bad_interactions} interaction primitives have a nonpositive axis, a \
                 negative reach, or name a placement group that does not exist"
            ),
        );
    }
}

/// The world rectangle a request generates, halo included.
pub fn generated_bounds(request: &SceneRequest, halo_m: f64) -> WorldRect {
    request.bounds.expanded(halo_m)
}

/// A point at the ground surface, for a recipe placing something.
pub fn ground_point(fields: &TerrainFieldStack, at: WorldPoint) -> [f64; 3] {
    [at.u_m, at.v_m, fields.surface_height(at) as f64]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_ranks_come_from_the_keys_and_not_from_the_order() {
        // The property the fix exists for. `ownership::assign` lays owners out
        // as consecutive intervals of one draw, so this index decides which
        // population wins a contested candidate — and a document edit that
        // moved a population up the file must not reassign anything.
        let declared = ["meadow_flowers", "field_stones", "meadow_undergrowth"];
        let reversed = ["meadow_undergrowth", "field_stones", "meadow_flowers"];
        assert_eq!(
            owner_ranks(declared.iter().copied()),
            owner_ranks(reversed.iter().copied())
        );
    }

    #[test]
    fn owner_ranks_are_dense_and_start_at_zero() {
        // Dense because ownership walks them as intervals; from zero because a
        // gap would leave an interval nothing can win.
        let ranks = owner_ranks(["c", "a", "b"].iter().copied());
        let mut values: Vec<u16> = ranks.values().copied().collect();
        values.sort_unstable();
        assert_eq!(values, vec![0, 1, 2]);
        assert_eq!(ranks["a"], 0);
        assert_eq!(ranks["b"], 1);
        assert_eq!(ranks["c"], 2);
    }

    #[test]
    fn a_repeated_key_collapses_rather_than_producing_two_ranks() {
        // Duplicate population keys are refused upstream by document
        // validation. If one ever reached here, one key must still mean one
        // owner — two ranks for one key would give that population two
        // intervals and double its share of every contested candidate.
        let ranks = owner_ranks(["a", "b", "a"].iter().copied());
        assert_eq!(ranks.len(), 2);
    }
}

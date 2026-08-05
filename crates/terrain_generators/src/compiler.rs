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
    Aabb3, AnalyticMark, CurveMark, MarkId, PainterOrder, RibbonMark, SceneMark,
    SceneMaterialBinding, SceneMaterialIndex,
};
use terrain_scene::projection::ScenePoint;
use terrain_scene::scene::{SceneBuilder, SceneRequest, TerrainScene};

use crate::domain::{
    CandidateDomainDef, DOMAIN_ALGORITHM_VERSION, DomainCandidate, DomainRequest, accepts, generate,
};
use crate::ownership::{OwnerOption, assign};
use crate::population::EmittedMark;
use crate::recipe::{RecipeContext, RecipeOutput, TerrainRecipeRegistry};
use crate::transition::{TransitionProfile, realise};

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
}

impl Default for SceneCompileOptions {
    fn default() -> Self {
        Self {
            field_spacing_m: None,
            derive: DerivedFieldRequest::PLACEMENT,
            transition: TransitionProfile::default(),
            validate: true,
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
pub struct SceneCompilation {
    pub scene: Arc<TerrainScene>,
    pub fields: Arc<TerrainFieldStack>,
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

        let domain = recipe.domain();
        domains
            .entry(domain.as_str().to_string())
            .or_insert_with(|| recipe.domain_definition());

        let appearance_base = appearances.len() as u16;
        appearances.extend(recipe.appearances());

        claimants.push(Claimant {
            key: population.key.as_str().to_string(),
            owner: claimants.len() as u16,
            recipe: population.recipe.clone(),
            domain,
            affinity: population.material_affinity.clone(),
            abundance_channel: population.abundance_channel,
            parameters: population.parameters.clone(),
            density_per_m2: recipe.target_density(&population.parameters),
            reach_m: recipe.maximum_reach_m(&population.parameters),
            appearance_base,
        });
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

    for (name, domain) in &domains {
        let members: Vec<&Claimant> = claimants
            .iter()
            .filter(|claimant| claimant.domain.as_str() == name)
            .collect();
        if members.is_empty() {
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
            let mut weights: Vec<(MaterialIndex, f32)> = Vec::new();
            fields.substrate_weights_into(candidate.position, &mut weights);
            let substrate = realise(
                weights.iter().copied(),
                candidate.position,
                &options.transition,
                root_seed,
            );

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

            let context = RecipeContext {
                fields: &fields,
                seeds: seeds.for_recipe(recipe.version()),
                parameters: &claimant.parameters,
                substrate,
                surface_z_m: fields.surface_height(candidate.position) as f64,
                root_seed,
            };
            let mut sink = MarkSink {
                builder: &mut builder,
                projection: request.projection,
                candidate,
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
        fields: Arc::new(fields),
        report,
    })
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
}

/// The world rectangle a request generates, halo included.
pub fn generated_bounds(request: &SceneRequest, halo_m: f64) -> WorldRect {
    request.bounds.expanded(halo_m)
}

/// A point at the ground surface, for a recipe placing something.
pub fn ground_point(fields: &TerrainFieldStack, at: WorldPoint) -> [f64; 3] {
    [at.u_m, at.v_m, fields.surface_height(at) as f64]
}

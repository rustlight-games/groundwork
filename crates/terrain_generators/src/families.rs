//! The content families: what actually grows.
//!
//! Six recipes, sharing five lattices. The sharing is the interesting part — see
//! [`crate::domain`] — and the geometry below is what the reference plates in
//! `docs/references/` ask for.
//!
//! ## The unit of grass is the tuft, not the blade
//!
//! One accepted candidate becomes a **tuft**: five to nine blades from one root
//! neighbourhood, sharing a lean, a length family, a maturity and a hue. Look at
//! either reference plate and the grass is unmistakably made of clumps with
//! their own direction; a field of independently scattered blades reads as
//! carpet however carefully each blade is shaped, because nothing at the
//! centimetre scale groups them.
//!
//! It is also what makes the boundary work. In
//! `grass_to_mud_transition.jpg` the last grass before the mud is a scatter of
//! *whole clumps*, full height and full density, standing on bare ground. That
//! is what falling acceptance looks like when the unit is a tuft. If the unit
//! were a blade, falling acceptance would thin every clump uniformly and the
//! edge would fade into a haze instead of breaking into islands.
//!
//! ## Everything reads the matrix
//!
//! Dryness from moisture, height from the vegetation channels, lean from the
//! flow direction, and — for dirt — wetness from where water collects. A recipe
//! that ignored the fields would produce content that is internally consistent
//! and unrelated to the ground it stands on, which is the look of a scatter
//! plugin rather than of terrain.

use terrain_core::diagnostics::{DiagnosticReport, Location};
use terrain_core::document::ParameterObject;
use terrain_core::ids::{DomainKey, PopulationKey, RecipeKey, StreamKey};
use terrain_scene::mark::{MarkAttributes, RibbonGeometry, Stratum, TipShape, WidthProfile};

use crate::domain::{CandidateDomainDef, DomainCandidate, SpacingPolicy};
use crate::population::EmittedMark;
use crate::recipe::{RecipeContext, RecipeOutput, TerrainRecipe, TerrainRecipeRegistry};
use crate::tuned::{RecipeRenderClass, TunedPass};

/// Every family this build knows how to grow.
///
/// One function listing everything, which is the whole benefit of explicit
/// registration: a reviewer can read what the binary can do, and two recipes
/// claiming one key fail here rather than by link order.
pub fn register_families(registry: &mut TerrainRecipeRegistry) {
    registry.register(Box::new(GrassTuft));
    registry.register(Box::new(GrassFine));
    registry.register(Box::new(GroundThatch));
    registry.register(Box::new(MeadowFlowers));
    registry.register(Box::new(MeadowUndergrowth));
    registry.register(Box::new(FieldStones));
    registry.register(Box::new(DirtClods));
}

/// A registry with every family registered.
pub fn family_registry() -> TerrainRecipeRegistry {
    let mut registry = TerrainRecipeRegistry::new();
    register_families(&mut registry);
    registry
}

fn key(name: &str) -> RecipeKey {
    RecipeKey::new(name).expect("family keys are valid by construction")
}

fn domain_key(name: &str) -> DomainKey {
    DomainKey::new(name).expect("domain keys are valid by construction")
}

fn stream(name: &str) -> StreamKey {
    StreamKey::new(name).expect("stream names are valid by construction")
}

/// A parameter, or a default, reporting anything unusable.
fn number(
    parameters: &ParameterObject,
    name: &str,
    default: f64,
    population: &PopulationKey,
    diagnostics: &mut DiagnosticReport,
) -> f64 {
    match parameters.number(name) {
        None => default,
        Some(value) if value.is_finite() && value > 0.0 => value,
        Some(value) => {
            diagnostics.error(
                "invalid_parameter",
                Location::at(format!("populations.{population}.parameters.{name}")),
                format!("`{name}` is {value}; it must be finite and positive"),
            );
            default
        }
    }
}

/// A parameter in `0..1`, reporting anything outside it.
///
/// Separate from [`number`] because that one refuses zero and negatives, which
/// is right for a length and wrong for a hue: zero is red and zero saturation
/// is white, and both are things an author legitimately asks for.
fn unit_number(
    parameters: &ParameterObject,
    name: &str,
    default: f64,
    population: &PopulationKey,
    diagnostics: &mut DiagnosticReport,
) -> f64 {
    match parameters.number(name) {
        None => default,
        Some(value) if value.is_finite() && (0.0..=1.0).contains(&value) => value,
        Some(value) => {
            diagnostics.error(
                "invalid_parameter",
                Location::at(format!("populations.{population}.parameters.{name}")),
                format!("`{name}` is {value}; it must be finite and between zero and one"),
            );
            default
        }
    }
}

/// A `0..1` parameter, without diagnostics, for the emit path.
fn read_unit(parameters: &ParameterObject, name: &str, default: f64) -> f32 {
    match parameters.number(name) {
        Some(value) if value.is_finite() && (0.0..=1.0).contains(&value) => value as f32,
        _ => default as f32,
    }
}

/// A parameter, without diagnostics, for the emit path.
fn read(parameters: &ParameterObject, name: &str, default: f64) -> f64 {
    match parameters.number(name) {
        Some(value) if value.is_finite() && value > 0.0 => value,
        _ => default,
    }
}

/// Smooth step between two edges.
fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    if (high - low).abs() < 1.0e-6 {
        return if x >= high { 1.0 } else { 0.0 };
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---------------------------------------------------------------------------
// Grass tufts — the statement layer
// ---------------------------------------------------------------------------

/// Clumps of blades sharing a root, a lean and a family.
pub struct GrassTuft;

/// How far a tuft's blades spread from its anchor, in metres.
///
/// Three centimetres. Wider and the clump stops reading as one plant; narrower
/// and the blades occlude each other so completely that the count stops
/// mattering.
const TUFT_SPREAD_M: f64 = 0.03;

impl TerrainRecipe for GrassTuft {
    fn key(&self) -> RecipeKey {
        key("population.grass_tuft")
    }

    fn render_class(&self) -> RecipeRenderClass {
        // The tuned tuft pass already grows this, and grows it better. What a
        // document declaring `population.grass_tuft` buys is *control* over that
        // pass — not a second scatter of generic clumps standing in the same
        // ground.
        RecipeRenderClass::Tuned(TunedPass::Tuft)
    }

    fn domain(&self) -> DomainKey {
        domain_key("vegetation.tuft_anchor")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            // Eight candidates per 12 cm cell is about 550 anchors a square
            // metre at saturation, which is denser than any meadow wants — the
            // headroom is what lets an author raise density without the lattice
            // saturating and flattening the variation.
            cell_m: 0.12,
            candidates_per_cell: 8,
            // Tufts exclude each other: real ones compete for root space, and
            // pure jitter puts two anchors a millimetre apart often enough to
            // read as a doubled clump.
            // Centre distance, not footprint. A tuft's exclusion radius is
            // about root competition rather than about how much ground the
            // clump physically covers, and reading the old number as a
            // footprint would have halved the spacing of every meadow.
            spacing: SpacingPolicy::PriorityDistance {
                minimum_centre_distance_m: 0.035,
            },
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["plant.grass_blade", "plant.grass_dry", "plant.broad_leaf"]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 220.0)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        // A blade can lean its whole length from a root already offset from the
        // anchor, so both add. An upper bound, not a typical value.
        read(parameters, "length_m", 0.20) * read(parameters, "length_variation", 1.6)
            + TUFT_SPREAD_M
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 220.0, population, diagnostics);
        number(parameters, "length_m", 0.20, population, diagnostics);
        number(parameters, "width_m", 0.0035, population, diagnostics);
        number(parameters, "blades", 7.0, population, diagnostics);
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let at = candidate.position;
        let base_length = read(context.parameters, "length_m", 0.20) as f32;
        let base_width = read(context.parameters, "width_m", 0.0035) as f32;
        let blade_count = read(context.parameters, "blades", 7.0).clamp(1.0, 16.0) as u16;

        // The tuft's shared personality. Drawn once, at the anchor, so every
        // blade in the clump agrees — which is the entire reason a tuft is the
        // unit.
        let family = candidate.latent(seeds, &stream("tuft_family"));
        let maturity = candidate.latent(seeds, &stream("tuft_maturity"));
        let hue = candidate.latent_range(seeds, &stream("tuft_hue"), -1.0, 1.0);
        let lean = candidate.latent_range(seeds, &stream("tuft_lean"), 0.0, std::f32::consts::TAU);

        // The ground decides the rest. A tuft on wet, sheltered, hollow ground
        // grows taller and greener than one on a dry exposed crest, and reading
        // that from the matrix is what ties the content to the terrain.
        let moisture = context
            .fields
            .derived
            .flow_accumulation
            .as_ref()
            .map(|plane| {
                let value = plane.sample(&context.fields.grid, at);
                // Accumulated area is unbounded above, so compress it.
                (value / (value + 0.25)).clamp(0.0, 1.0)
            })
            .unwrap_or(0.5);
        let exposure = context.fields.exposure(at);
        let curvature = context.fields.curvature(at);
        // Hollows collect: negative curvature is a dip.
        let sheltered = smoothstep(0.4, -0.4, curvature);

        // Grass standing on ground that is mostly not grass is the sparse edge
        // of a meadow: shorter, drier, and it is what makes the transition read
        // as a boundary rather than as a line where one texture stops.
        let own_ground = context.substrate.dominant().map(|(_, w)| w).unwrap_or(1.0);
        let contested = 1.0 - own_ground;

        let length_scale = (0.75 + 0.5 * family)
            * (0.85 + 0.30 * moisture)
            * (1.0 - 0.35 * contested)
            * (0.90 + 0.20 * sheltered);
        let dryness =
            (0.25 + 0.5 * (1.0 - moisture) + 0.4 * contested - 0.2 * sheltered).clamp(0.0, 1.0);

        for blade in 0..blade_count {
            let path = [blade as u32];
            let draw = |name: &str| {
                seeds.unit_with_path(
                    &terrain_core::seed::RandomAddress::new(candidate.id, &stream(name)),
                    &path,
                ) as f32
            };

            // Blades sit around the anchor rather than exactly on it, so the
            // clump has a footprint instead of being a fan from one point.
            let spread = TUFT_SPREAD_M as f32 * (0.3 + 0.7 * draw("blade_spread"));
            let around = draw("blade_around") * std::f32::consts::TAU;
            let root = terrain_core::coords::WorldPoint::new(
                at.u_m + (spread * around.cos()) as f64,
                at.v_m + (spread * around.sin()) as f64,
            );

            // Each blade leans near the tuft's direction, not at random. The
            // spread is what stops the clump reading as a comb.
            let azimuth = lean + (draw("blade_azimuth") - 0.5) * 1.5;
            let length = base_length * length_scale * (0.7 + 0.6 * draw("blade_length"));
            let bend = 0.35 + 0.85 * draw("blade_bend") + 0.3 * dryness;

            // A tenth of the blades in a mature tuft are broader leaves, and a
            // dry tuft carries some straw. Chosen per blade so one clump can
            // hold both.
            let appearance = if draw("blade_kind") < 0.12 + 0.10 * maturity {
                2
            } else if draw("blade_dry") < dryness * 0.45 {
                1
            } else {
                0
            };

            output.emit(EmittedMark::Ribbon {
                root: [root.u_m, root.v_m, context.surface_z_m],
                geometry: RibbonGeometry {
                    length_m: length,
                    azimuth_rad: azimuth,
                    bend_rad: bend,
                    curl_rad: 0.15 + 0.5 * draw("blade_curl"),
                    sway_rad: (draw("blade_sway") - 0.5) * 0.5,
                    // An elbow. Every smooth arc in a field of smooth arcs
                    // advertises the function that drew it; no continuous
                    // parameter produces a kink.
                    kink_rad: if draw("blade_kinks") < 0.25 {
                        (draw("blade_kink") - 0.5) * 0.7
                    } else {
                        0.0
                    },
                    kink_at: 0.45 + 0.35 * draw("blade_kink_at"),
                    kink_turn_rad: (draw("blade_kink_turn") - 0.5) * 0.4,
                    // The cheapest valuable parameter in the vocabulary:
                    // without it every blade presents the same face to the sun
                    // and the tuft reads as a comb.
                    twist_rad: (draw("blade_twist") - 0.5) * 2.2,
                    width_m: base_width * (0.7 + 0.6 * draw("blade_width")),
                    tip_width_m: base_width * 0.18,
                    profile: if appearance == 2 {
                        WidthProfile::Oval
                    } else {
                        WidthProfile::Leaf
                    },
                    tip: if draw("blade_fork") < 0.08 {
                        TipShape::Forked {
                            split_at: 0.7,
                            opening_rad: 0.25,
                            long: 0.35,
                            short: 0.2,
                        }
                    } else {
                        TipShape::Pointed
                    },
                    ridge: 0.25 + 0.3 * draw("blade_ridge"),
                },
                attributes: MarkAttributes {
                    maturity,
                    moisture,
                    exposure,
                    tint: hue * 0.6 + (draw("blade_tint") - 0.5) * 0.5,
                    variation: family,
                },
                stratum: Stratum::Canopy,
                appearance,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Fine grass — the undergrowth that closes the canopy
// ---------------------------------------------------------------------------

/// Short filler blades between the tufts.
///
/// A separate domain from the tufts, at much higher density and without
/// exclusion, because its job is to close the gaps the statement clumps leave.
/// Sharing the tuft lattice would make the two compete for the same candidates
/// and the meadow would be either clumps or filler, never both.
pub struct GrassFine;

impl TerrainRecipe for GrassFine {
    fn key(&self) -> RecipeKey {
        key("population.grass_fine")
    }

    fn render_class(&self) -> RecipeRenderClass {
        // The closed canopy. Emitting these generically would put a coarse
        // ribbon field behind the tuned one at four thousand marks a square
        // metre, which is the most expensive way there is to make a meadow
        // look worse.
        RecipeRenderClass::Tuned(TunedPass::Fine)
    }

    fn domain(&self) -> DomainKey {
        domain_key("vegetation.fine")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            cell_m: 0.06,
            candidates_per_cell: 8,
            // No exclusion: fine grass has no business keeping its neighbours
            // at arm's length, and the thinning pass is the expensive part.
            spacing: SpacingPolicy::Jittered,
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["plant.grass_blade", "plant.grass_dry"]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 900.0)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "length_m", 0.09) * 1.8
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 900.0, population, diagnostics);
        number(parameters, "length_m", 0.09, population, diagnostics);
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let base_length = read(context.parameters, "length_m", 0.09) as f32;
        let own_ground = context.substrate.dominant().map(|(_, w)| w).unwrap_or(1.0);
        let dryness = (1.0 - own_ground).clamp(0.0, 1.0);

        output.emit(EmittedMark::Ribbon {
            root: [
                candidate.position.u_m,
                candidate.position.v_m,
                context.surface_z_m,
            ],
            geometry: RibbonGeometry {
                length_m: base_length
                    * (0.6 + 0.8 * candidate.latent(seeds, &stream("fine_length"))),
                azimuth_rad: candidate.latent_range(
                    seeds,
                    &stream("fine_azimuth"),
                    0.0,
                    std::f32::consts::TAU,
                ),
                bend_rad: 0.5 + 0.9 * candidate.latent(seeds, &stream("fine_bend")),
                curl_rad: 0.2 * candidate.latent(seeds, &stream("fine_curl")),
                sway_rad: 0.0,
                kink_rad: 0.0,
                kink_at: 0.5,
                kink_turn_rad: 0.0,
                twist_rad: candidate.latent_range(seeds, &stream("fine_twist"), -1.0, 1.0),
                width_m: 0.0022 * (0.7 + 0.6 * candidate.latent(seeds, &stream("fine_width"))),
                tip_width_m: 0.0005,
                profile: WidthProfile::Leaf,
                tip: TipShape::Pointed,
                ridge: 0.2,
            },
            attributes: MarkAttributes {
                maturity: 0.35,
                moisture: 0.5,
                exposure: context.fields.exposure(candidate.position),
                tint: candidate.latent_range(seeds, &stream("fine_tint"), -0.5, 0.5),
                variation: candidate.latent(seeds, &stream("fine_variation")),
            },
            stratum: Stratum::Canopy,
            appearance: if candidate.latent(seeds, &stream("fine_dry")) < dryness * 0.5 {
                1
            } else {
                0
            },
        });
    }
}

// ---------------------------------------------------------------------------
// Thatch — the dull mat between green grass and bare ground
// ---------------------------------------------------------------------------

/// The flattened dead layer under a canopy and at its edge.
///
/// Both reference plates show it and it is easy to leave out: between the green
/// and the clean mud there is a band that is neither, made of flattened dead
/// material. Without it the boundary is grass-then-dirt with nothing in between,
/// which reads as a cut.
pub struct GroundThatch;

impl TerrainRecipe for GroundThatch {
    fn key(&self) -> RecipeKey {
        key("population.ground_thatch")
    }

    fn render_class(&self) -> RecipeRenderClass {
        RecipeRenderClass::Tuned(TunedPass::Thatch)
    }

    fn domain(&self) -> DomainKey {
        domain_key("vegetation.fine")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            cell_m: 0.06,
            candidates_per_cell: 8,
            spacing: SpacingPolicy::Jittered,
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["plant.thatch"]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 260.0)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "length_m", 0.07) * 1.5
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 260.0, population, diagnostics);
        number(parameters, "length_m", 0.07, population, diagnostics);
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let length = read(context.parameters, "length_m", 0.07) as f32;
        output.emit(EmittedMark::Ribbon {
            root: [
                candidate.position.u_m,
                candidate.position.v_m,
                context.surface_z_m,
            ],
            geometry: RibbonGeometry {
                length_m: length * (0.6 + 0.7 * candidate.latent(seeds, &stream("thatch_length"))),
                azimuth_rad: candidate.latent_range(
                    seeds,
                    &stream("thatch_azimuth"),
                    0.0,
                    std::f32::consts::TAU,
                ),
                // Nearly flat. Thatch is lying down — that is what makes it
                // thatch rather than short grass, and it is why it belongs to
                // the ground stratum where it can be buried.
                bend_rad: 1.25 + 0.3 * candidate.latent(seeds, &stream("thatch_bend")),
                curl_rad: 0.0,
                sway_rad: 0.0,
                kink_rad: 0.0,
                kink_at: 0.5,
                kink_turn_rad: 0.0,
                twist_rad: 0.0,
                width_m: 0.0025,
                tip_width_m: 0.0008,
                profile: WidthProfile::Stem,
                tip: TipShape::Notched { depth: 0.3 },
                ridge: 0.1,
            },
            attributes: MarkAttributes {
                maturity: 0.9,
                moisture: 0.3,
                exposure: 0.4,
                tint: candidate.latent_range(seeds, &stream("thatch_tint"), -0.4, 0.4),
                variation: candidate.latent(seeds, &stream("thatch_variation")),
            },
            stratum: Stratum::Ground,
            appearance: 0,
        });
    }
}

// ---------------------------------------------------------------------------
// Flowers
// ---------------------------------------------------------------------------

/// A stem and a head, standing above the canopy.
pub struct MeadowFlowers;

impl TerrainRecipe for MeadowFlowers {
    fn key(&self) -> RecipeKey {
        key("population.meadow_flowers")
    }

    fn render_class(&self) -> RecipeRenderClass {
        // Nothing in the tuned generator grows a flower — `placement.rs` has no
        // petal, bloom or flower anywhere in it. This recipe is the only source,
        // so it is the one that renders.
        RecipeRenderClass::Secondary
    }

    fn domain(&self) -> DomainKey {
        domain_key("vegetation.emergent")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            cell_m: 0.25,
            candidates_per_cell: 6,
            spacing: SpacingPolicy::PriorityDistance {
                minimum_centre_distance_m: 0.08,
            },
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["flower.stem", "flower.head", "flower.petal"]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 6.0)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "stem_length_m", 0.28) * 1.6
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 6.0, population, diagnostics);
        number(parameters, "stem_length_m", 0.28, population, diagnostics);
        number(parameters, "head_radius_m", 0.013, population, diagnostics);
        // Colour. Not validated as positive-only, because a hue of zero is red
        // and a saturation of zero is white, and both are things an author
        // legitimately asks for.
        unit_number(parameters, "petal_hue", 0.14, population, diagnostics);
        unit_number(
            parameters,
            "petal_hue_spread",
            0.05,
            population,
            diagnostics,
        );
        unit_number(
            parameters,
            "petal_saturation",
            0.18,
            population,
            diagnostics,
        );
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let stem_length = read(context.parameters, "stem_length_m", 0.24) as f32
            * (0.75 + 0.5 * candidate.latent(seeds, &stream("flower_length")));
        let head_radius = read(context.parameters, "head_radius_m", 0.011) as f32
            * (0.8 + 0.4 * candidate.latent(seeds, &stream("flower_head")));
        let azimuth =
            candidate.latent_range(seeds, &stream("flower_azimuth"), 0.0, std::f32::consts::TAU);
        let bend = 0.12 + 0.35 * candidate.latent(seeds, &stream("flower_bend"));
        let root = [
            candidate.position.u_m,
            candidate.position.v_m,
            context.surface_z_m,
        ];
        let variation = candidate.latent(seeds, &stream("flower_variation"));

        // ## The colour of the flowers is the document's to decide
        //
        // A meadow generator whose flowers are always white is a daisy
        // generator. What an author wants to say is "buttercups here, knapweed
        // over there", and the way to say it is a hue and a spread — a species
        // has a colour and a population of one species has a *narrow band* of
        // it, which is what the spread is for. Zero spread gives a bed of
        // identical blooms, which is what a cultivated planting looks like.
        //
        // Carried on the petal mark's `tint` and `variation` because a
        // `MarkAttributes` has one scalar tint and a colour needs two numbers.
        // For `flower.petal` and only for it, `tint` *is* the hue and
        // `variation` *is* the saturation; the bridge knows that and turns them
        // back into linear RGB. Documented here rather than inferred, because a
        // channel that means one thing under one appearance and another under
        // the next is exactly the sort of thing that gets rewired by accident.
        let base_hue = read_unit(context.parameters, "petal_hue", 0.14);
        let spread = read_unit(context.parameters, "petal_hue_spread", 0.05);
        let saturation = read_unit(context.parameters, "petal_saturation", 0.18);
        let hue = (base_hue
            + spread * candidate.latent_range(seeds, &stream("petal_hue_drift"), -1.0, 1.0))
        .rem_euclid(1.0);
        // Mapped into the attribute's own `-1..1`, so nothing downstream has to
        // know the range changed.
        let tint = hue * 2.0 - 1.0;

        output.emit(EmittedMark::Curve {
            root,
            length_m: stem_length,
            azimuth_rad: azimuth,
            bend_rad: bend,
            radius_m: 0.0013,
            tip_radius_m: 0.0009,
            attributes: MarkAttributes {
                maturity: 0.7,
                moisture: 0.5,
                exposure: 1.0,
                tint: candidate.latent_range(seeds, &stream("flower_stem_tint"), -0.3, 0.3),
                variation,
            },
            stratum: Stratum::Emergent,
            appearance: 0,
        });

        // The tip of the bent stem, in closed form.
        //
        // A stem is a circular arc of curvature `bend/length`, so its tip is at
        // `(L/θ)(1 − cos θ)` along the lean and `(L/θ) sin θ` up. The small-angle
        // limit is handled by the guard, because the divided form is `0/0` at
        // zero bend and evaluating it at a tiny angle and hoping the
        // cancellation is harmless is exactly the kind of thing that puts one
        // flower head a metre away from its own stem.
        let (lean_m, rise_m) = if bend.abs() < 1.0e-3 {
            (0.0, stem_length)
        } else {
            let radius = stem_length / bend;
            (radius * (1.0 - bend.cos()), radius * bend.sin())
        };
        let head = [
            root[0] + (lean_m * azimuth.cos()) as f64,
            root[1] + (lean_m * azimuth.sin()) as f64,
            root[2] + rise_m as f64,
        ];

        // ## Petals, because a disk on a stick is not a flower
        //
        // A single ellipsoid head reads as a pin at any framing where the plant
        // is more than a few pixels tall — which is every framing this project
        // renders at. What makes a flower recognisable is its *silhouette*: a
        // ring of separate blades with gaps between them, each catching the sun
        // at its own angle and shadowing the disk underneath.
        //
        // Five to eight, from the candidate's own address rather than a draw, so
        // adding a petal parameter later cannot shift any other decision.
        let petals = 5 + (candidate.latent(seeds, &stream("petal_count")) * 4.0) as u16;
        let petal_length =
            head_radius * (1.5 + 0.9 * candidate.latent(seeds, &stream("petal_reach")));
        let petal_width =
            petal_length * (0.34 + 0.22 * candidate.latent(seeds, &stream("petal_width")));
        let phase =
            candidate.latent_range(seeds, &stream("petal_phase"), 0.0, std::f32::consts::TAU);

        for index in 0..petals {
            // Evenly spaced, then jittered — a perfectly regular whorl is as
            // recognisable as a perfectly regular scatter, and for the same
            // reason.
            let jitter = candidate.latent_range(
                seeds,
                &stream(match index % 4 {
                    0 => "petal_jitter_a",
                    1 => "petal_jitter_b",
                    2 => "petal_jitter_c",
                    _ => "petal_jitter_d",
                }),
                -0.18,
                0.18,
            );
            let angle = phase + std::f32::consts::TAU * index as f32 / petals as f32 + jitter;
            // Set out from the disk so the petals ring it rather than growing
            // out of its centre, and lifted a little so they sit on top of it.
            let reach = head_radius * 0.55 + petal_length * 0.5;
            output.emit(EmittedMark::Analytic {
                centre: [
                    head[0] + (reach * angle.cos()) as f64,
                    head[1] + (reach * angle.sin()) as f64,
                    head[2] + (head_radius * 0.18) as f64,
                ],
                // Long along its own radial direction, narrow across, and thin.
                radius_m: [petal_length * 0.5, petal_width * 0.5],
                height_m: petal_length * 0.10,
                rotation_rad: angle,
                attributes: MarkAttributes {
                    maturity: 0.85,
                    moisture: 0.4,
                    exposure: 1.0,
                    // Hue and saturation. See the note above.
                    tint,
                    variation: saturation,
                },
                appearance: 2,
            });
        }

        // The disk last, so it sorts over the petal roots.
        output.emit(EmittedMark::Analytic {
            centre: head,
            radius_m: [head_radius, head_radius],
            height_m: head_radius * 0.55,
            rotation_rad: azimuth,
            attributes: MarkAttributes {
                maturity: 0.8,
                moisture: 0.5,
                exposure: 1.0,
                tint,
                variation,
            },
            appearance: 1,
        });
    }
}

// ---------------------------------------------------------------------------
// Undergrowth
// ---------------------------------------------------------------------------

/// Low broad-leaved rosettes, below and between the canopy.
///
/// ## Not another grass pass
///
/// The tuned generator already grows four layers of grass and a fifth would be
/// more of the same at a lower quality. What a meadow has that none of those
/// four supply is *broad* leaves near the ground — plantain, dock, sorrel —
/// which read completely differently because they are wide, curved and low
/// where every grass blade is narrow and upright.
///
/// That difference is most of the value. A canopy of nothing but blades reads
/// as one plant repeated however varied its lengths are, and the eye finds the
/// repetition long before it can name it. One broad leaf per square metre
/// breaks it.
///
/// ## What went wrong the first time, and what an arch fixes
///
/// The first version emitted each leaf as a flattened lozenge lying *flat* on
/// the ground at a fixed lift, with only a yaw to tell one from another. Every
/// leaf in a plate was therefore the same shape, at the same pitch, with the
/// same normal — so every one of them took the same light. What that renders as
/// is not undergrowth: it is a scatter of green stains on the soil, and the
/// larger the plate the more obviously they are stamps of one decal.
///
/// A real ground leaf is not flat and its silhouette is not the point. It
/// *arches*: it leaves the crown steeply, rolls over past the horizontal and
/// droops its tip back toward the earth, and it is folded along a midrib so that
/// the two halves face different ways. Both of those are what makes a rosette
/// read — the arch gives it height and a shadow of its own, the fold gives it a
/// lit half and a dark half from any sun angle. Neither survives being drawn as
/// a horizontal ellipse.
///
/// So a leaf here is a [`RibbonGeometry`] like every other piece of foliage in
/// the framework: bent past vertical, curled at the tip, twisted about its own
/// axis and ridged along its centre. It costs a tessellated ribbon instead of an
/// instance, which at one to two plants a square metre is nothing.
///
/// ## A rosette is not a starburst
///
/// The leaves of one plant are not interchangeable. The outer ones are longer,
/// splay further and lie almost on the ground; the inner ones are shorter,
/// stand nearly upright and are narrower. Growing every leaf from one
/// distribution produces a pinwheel — radially symmetric, uniformly pitched,
/// and unmistakably generated. The ladder from outer to inner is applied by
/// leaf index below, and it is most of what separates this from the lozenges.
///
/// ## Visible where the grass is not
///
/// A rosette sits under the canopy, so in thick grass it contributes shadow and
/// almost nothing else. Where the sward thins — a path fringe, a scuff, a poor
/// patch — it is suddenly the thing you see. That is why its abundance is a
/// channel rather than a constant: an author wants it exactly where the grass
/// has gone.
pub struct MeadowUndergrowth;

impl TerrainRecipe for MeadowUndergrowth {
    fn key(&self) -> RecipeKey {
        key("population.meadow_undergrowth")
    }

    fn render_class(&self) -> RecipeRenderClass {
        // Nothing in the tuned generator grows a broad ground leaf: its
        // `Stream::Leaf` pass is a *cluster of blades*, not a rosette. This
        // recipe is the only source, so it is the one that renders.
        RecipeRenderClass::Secondary
    }

    fn domain(&self) -> DomainKey {
        domain_key("vegetation.rosette")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            cell_m: 0.35,
            candidates_per_cell: 5,
            // A rosette holds its own ground: two overlapping crowns read as
            // one torn plant rather than as two.
            spacing: SpacingPolicy::PriorityDistance {
                minimum_centre_distance_m: 0.13,
            },
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["plant.undergrowth_leaf"]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 1.4)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        // Crown radius plus the longest leaf, both at their upper latent. An
        // arc of length L cannot put its tip further than L from its root
        // whatever the bend does, so the length bound holds without knowing the
        // curvature — the same argument `RibbonGeometry::reach_m` makes. The
        // half-width is added because a leaf is wide, and a ribbon's edge is
        // further out than its centreline.
        read(parameters, "crown_radius_m", 0.02)
            + read(parameters, "leaf_length_m", 0.12) * LEAF_LENGTH_MAX as f64
            + read(parameters, "leaf_width_m", 0.035) * 0.75
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 1.4, population, diagnostics);
        number(parameters, "leaf_length_m", 0.12, population, diagnostics);
        number(parameters, "leaf_width_m", 0.035, population, diagnostics);
        number(parameters, "crown_radius_m", 0.02, population, diagnostics);
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let base_length = read(context.parameters, "leaf_length_m", 0.12) as f32;
        let base_width = read(context.parameters, "leaf_width_m", 0.035) as f32;
        let crown = read(context.parameters, "crown_radius_m", 0.02) as f32;

        // Four to nine leaves. Addressed rather than drawn, so adding a leaf
        // parameter later cannot shift any other decision this plant makes.
        //
        // Three was too few once the leaves stopped lying flat: a rosette of
        // three arches is a tripod, and the eye counts the legs.
        let leaves = 4 + (candidate.latent(seeds, &stream("rosette_leaves")) * 6.0) as u16;
        let phase =
            candidate.latent_range(seeds, &stream("rosette_phase"), 0.0, std::f32::consts::TAU);

        // How much the rosette combs with the meadow's flow.
        //
        // A perfect radial rosette repeated everywhere is procedural, and a
        // rosette fully combed into the flow is a comb. Between a tenth and
        // half, varying per plant, keeps enough radial structure that it still
        // reads as one plant.
        let flow_mix = candidate.latent_range(seeds, &stream("rosette_flow"), 0.10, 0.45);
        // Which way water runs here. Absent when the document did not ask for
        // the flow field, in which case the rosette stays purely radial — which
        // is the honest fallback, not a made-up direction.
        let flow_angle = context
            .fields
            .derived
            .flow_direction
            .as_ref()
            .map(|plane| {
                let v = plane.sample_unit(&context.fields.grid, candidate.position);
                v[1].atan2(v[0])
            })
            .unwrap_or(0.0);
        let flow_mix = if context.fields.derived.flow_direction.is_some() {
            flow_mix
        } else {
            0.0
        };

        let variation = candidate.latent(seeds, &stream("rosette_variation"));
        let tint = candidate.latent_range(seeds, &stream("rosette_tint"), -1.0, 1.0);

        for index in 0..leaves {
            let jitter = candidate.latent_range(
                seeds,
                &stream(match index % 4 {
                    0 => "rosette_jitter_a",
                    1 => "rosette_jitter_b",
                    2 => "rosette_jitter_c",
                    _ => "rosette_jitter_d",
                }),
                -0.30,
                0.30,
            );
            let radial = phase + std::f32::consts::TAU * index as f32 / leaves as f32 + jitter;
            // Blended toward the flow as a *vector*: an angular blend takes the
            // short way round and can swing a leaf through its neighbour.
            let (rs, rc) = radial.sin_cos();
            let (fs, fc) = flow_angle.sin_cos();
            let angle = (rs * (1.0 - flow_mix) + fs * flow_mix)
                .atan2(rc * (1.0 - flow_mix) + fc * flow_mix);

            // Where this leaf sits on the outer-to-inner ladder. Zero is the
            // outermost, one the innermost. Index order is arbitrary and that is
            // fine: what matters is that a plant holds the whole range rather
            // than that any particular leaf is on the outside.
            let rank = if leaves > 1 {
                index as f32 / (leaves - 1) as f32
            } else {
                0.0
            };

            let length = base_length
                * candidate.latent_range(
                    seeds,
                    &stream(if index % 2 == 0 {
                        "rosette_len_a"
                    } else {
                        "rosette_len_b"
                    }),
                    LEAF_LENGTH_MIN,
                    LEAF_LENGTH_MAX,
                )
                // The outer leaves are the long ones. Real rosettes grow from
                // the centre out, so the oldest leaf is the outermost and has
                // had the longest to get there.
                * (1.0 - 0.35 * rank);

            // Bend past vertical, which is what makes it an arch rather than a
            // fan. A leaf at π/2 points straight out sideways; everything above
            // that is a tip on its way back down to the ground.
            //
            // The outer leaves fall furthest — they are long enough that their
            // own weight beats them — and the innermost stand close to upright.
            let bend = candidate.latent_range(
                seeds,
                &stream(if index % 2 == 0 {
                    "rosette_bend_a"
                } else {
                    "rosette_bend_b"
                }),
                -0.16,
                0.16,
            ) + 2.15
                - 1.05 * rank;

            // Narrower toward the centre, and never so narrow that it reads as
            // a blade of grass — the width is the whole reason this family
            // exists beside the tuned passes.
            let width = base_width * 0.5 * (0.82 + 0.36 * variation) * (1.0 - 0.28 * rank);

            output.emit(EmittedMark::Ribbon {
                // Every leaf of one plant leaves the same crown. A rosette whose
                // leaves start at scattered points is a patch of separate
                // seedlings, and it reads as one.
                root: [
                    candidate.position.u_m + (crown * 0.35 * angle.cos()) as f64,
                    candidate.position.v_m + (crown * 0.35 * angle.sin()) as f64,
                    context.surface_z_m,
                ],
                geometry: RibbonGeometry {
                    length_m: length,
                    azimuth_rad: angle,
                    bend_rad: bend,
                    // The hook at the tip. On a leaf this is the last centimetre
                    // turning down into the grass rather than ending in mid-air,
                    // and it is where the shadow under the plant comes from.
                    curl_rad: candidate.latent_range(seeds, &stream("rosette_curl"), 0.10, 0.55)
                        * (0.4 + 0.6 * (1.0 - rank)),
                    // A leaf that leaves the crown straight and stays straight
                    // is a ruler. A slight lateral drift is what a real one does
                    // reaching for its own gap in the canopy.
                    sway_rad: candidate.latent_range(seeds, &stream("rosette_sway"), -0.45, 0.45),
                    // No kink. A grass blade gets an elbow because it is thin
                    // enough to be broken by a foot; a broad leaf with a midrib
                    // creases rather than kinks, and the crease is the ridge.
                    kink_rad: 0.0,
                    kink_at: 0.5,
                    kink_turn_rad: 0.0,
                    // Rolled about its own axis, so the two halves of the fold
                    // present different faces to the sun. Half a radian is not
                    // subtle and it is not meant to be: it is the difference
                    // between a rosette and a cut-out.
                    twist_rad: candidate.latent_range(seeds, &stream("rosette_twist"), -0.6, 0.6),
                    width_m: width,
                    // A blunt tip rather than a needle. Plantain and dock end in
                    // a short point on a wide blade, and tapering to nothing
                    // makes the last third read as grass.
                    tip_width_m: width * 0.22,
                    // Narrow at the attachment, broadest a third of the way out,
                    // then a long taper. What a ground leaf does, and what the
                    // profile was named for.
                    profile: WidthProfile::Leaf,
                    tip: TipShape::Pointed,
                    // The midrib. Strongly folded, because this is the parameter
                    // that gives one leaf a lit half and a shaded half — the
                    // single largest difference between the arch and the lozenge
                    // it replaced.
                    ridge: candidate.latent_range(seeds, &stream("rosette_ridge"), 0.34, 0.62),
                },
                attributes: MarkAttributes {
                    maturity: 0.55,
                    moisture: context.ground_sample.state.moisture,
                    exposure: context.fields.exposure(candidate.position),
                    tint,
                    variation,
                },
                stratum: Stratum::Ground,
                appearance: 0,
            });
        }
    }
}

/// The narrowest and widest a leaf is drawn, as a multiple of the authored
/// length.
///
/// Named because [`MeadowUndergrowth::maximum_reach_m`] has to agree with
/// [`MeadowUndergrowth::emit`] exactly: a reach bound below what the emitter
/// actually grows makes a leaf present on one side of a page join and missing on
/// the other, which is the one artefact in this framework that no amount of
/// looking at a single plate will reveal.
const LEAF_LENGTH_MIN: f32 = 0.65;
const LEAF_LENGTH_MAX: f32 = 1.25;

// ---------------------------------------------------------------------------
// Stones
// ---------------------------------------------------------------------------

/// Analytic boulders, settled into the ground.
pub struct FieldStones;

impl TerrainRecipe for FieldStones {
    fn key(&self) -> RecipeKey {
        key("population.field_stones")
    }

    fn render_class(&self) -> RecipeRenderClass {
        RecipeRenderClass::Secondary
    }

    fn domain(&self) -> DomainKey {
        domain_key("rock.large")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            cell_m: 0.5,
            candidates_per_cell: 4,
            // Physical footprints, because a stone's exclusion radius *is* its
            // footprint — two stones interpenetrating is the one artefact in
            // this family that is unmistakable, and the rule that prevents it is
            // "their disks do not overlap" rather than "their centres are far
            // apart".
            //
            // Variable, so a big stone keeps more room than a small one. Sharing
            // one radius makes a field of stones read as a lattice of equal
            // cells with different-sized objects rattling around in them.
            spacing: SpacingPolicy::PriorityFootprints {
                radius: crate::domain::CandidateRadiusPolicy::Uniform {
                    min_m: 0.035,
                    max_m: 0.115,
                },
                // A centimetre of soil between them. Stones that touch read as
                // one broken stone rather than as two.
                clearance_m: 0.01,
            },
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        // Four silhouettes, not four rocks. A meadow stone needs enough
        // variety that the eye stops finding the repeat, and nothing like a
        // unique mesh per instance — at this framing the silhouette is nearly
        // all of what is legible, and high-frequency detail on a
        // five-centimetre object is invisible.
        vec![
            "rock.rounded",
            "rock.fractured",
            "rock.flat",
            "rock.elongated",
        ]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 1.2)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "radius_m", 0.06) * 2.5
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 1.2, population, diagnostics);
        number(parameters, "radius_m", 0.06, population, diagnostics);
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let radius = read(context.parameters, "radius_m", 0.06) as f32
            * (0.55 + 0.9 * candidate.latent(seeds, &stream("stone_size")));
        // Not round. A stone with equal semi-axes reads as a ball, and a field
        // of balls is the giveaway of an analytic rock.
        let squash = 0.55 + 0.45 * candidate.latent(seeds, &stream("stone_squash"));

        // Which of the four silhouettes, addressed rather than drawn.
        //
        // A flat pebble is not a rounded stone that happens to be short: the
        // families differ in *proportion* as well as in shape, so the family
        // decides the height range and the burial before any of it is scaled.
        let family = (candidate.latent(seeds, &stream("stone_family")) * 4.0).min(3.99) as u8;
        let (height_low, height_high, burial_low, burial_high) = match family {
            // Rounded: a waterworn cobble. Sits proud, buries a third.
            0 => (0.62, 1.05, 0.20, 0.38),
            // Fractured: broken rock, taller and more angular, sits shallower
            // because its flat faces do not bed in.
            1 => (0.70, 1.15, 0.16, 0.32),
            // Flat: a slab. Low, wide, and buried deepest — a flat stone works
            // its way down until only its face shows.
            2 => (0.24, 0.42, 0.28, 0.48),
            // Elongated: a long fragment lying along its own axis.
            _ => (0.44, 0.78, 0.22, 0.40),
        };
        let height = radius
            * (height_low
                + (height_high - height_low) * candidate.latent(seeds, &stream("stone_height")));
        // Correlated weakly with flatness: a small flat fragment sits deeper
        // than a big round one. A constant burial fraction is visible as a
        // common horizon line across a whole field of stones.
        let burial = burial_low
            + (burial_high - burial_low) * candidate.latent(seeds, &stream("stone_burial"));

        output.emit(EmittedMark::Analytic {
            centre: [
                candidate.position.u_m,
                candidate.position.v_m,
                // Settled: a stone sits *in* the ground rather than on it, and
                // the buried part is what stops it reading as a decal. Measured
                // from the object's own height rather than from its radius, so
                // a slab and a cobble bury by the same *fraction of themselves*.
                context.surface_z_m - (height * burial) as f64,
            ],
            radius_m: [
                radius,
                radius * if family == 3 { squash * 0.6 } else { squash },
            ],
            height_m: height,
            rotation_rad: candidate.latent_range(
                seeds,
                &stream("stone_rotation"),
                0.0,
                std::f32::consts::TAU,
            ),
            attributes: MarkAttributes {
                maturity: 1.0,
                moisture: context.fields.blend(candidate.position),
                exposure: context.fields.exposure(candidate.position),
                tint: candidate.latent_range(seeds, &stream("stone_tint"), -1.0, 1.0),
                variation: candidate.latent(seeds, &stream("stone_variation")),
            },
            appearance: family,
        });

        // What the stone does to what grows near it.
        //
        // Declared here rather than derived from the mark afterwards, because
        // the footprint is a *conservative* ellipse this recipe guarantees to
        // stay inside — and a footprint recovered from geometry is an estimate
        // that is sometimes smaller than the object it is supposed to bound.
        //
        // The response reaches further for a bigger stone: what it is taking is
        // light and root space, and a stone twice the size takes about twice as
        // much. Scaled off a reference radius rather than made proportional, so
        // a field of pebbles does not end up with a response band you could not
        // see and a boulder with one you could not miss.
        const REFERENCE_RADIUS_M: f32 = 0.06;
        output.emit_interaction(crate::recipe::EmittedInteraction {
            centre: [candidate.position.u_m, candidate.position.v_m],
            semi_u_m: radius,
            semi_v_m: radius * squash,
            yaw_rad: candidate.latent_range(
                seeds,
                &stream("stone_rotation"),
                0.0,
                std::f32::consts::TAU,
            ),
            // Eight millimetres of soil between a root and the stone. Zero
            // would let a blade sprout from the exact edge, which reads as
            // growing *out of* the rock.
            hard_clearance_m: 0.008,
            response_reach_m: 0.11 * (0.8 + 0.45 * (radius / REFERENCE_RADIUS_M).min(2.0)),
            channels: terrain_scene::scene::InteractionChannels::ALL_TUNED,
        });
    }
}

// ---------------------------------------------------------------------------
// Dirt clods and grit
// ---------------------------------------------------------------------------

/// The lumps and grit that make bare ground read as ground.
///
/// `grass_to_mud_bumpy.jpg` is mostly this: clods of three to eight centimetres
/// casting their own shadows under a low sun. A normal map cannot produce that
/// silhouette, which is why these are geometry rather than shading.
pub struct DirtClods;

impl TerrainRecipe for DirtClods {
    fn key(&self) -> RecipeKey {
        key("population.dirt_clods")
    }

    fn render_class(&self) -> RecipeRenderClass {
        // Deferred, not deleted. The ground profile's aggregate relief band
        // already carries clod-scale structure as displaced mesh; drawing this
        // population beside it would count one physical signal twice. The
        // declaration stays so the intent is not lost, and the compiler reports
        // it rather than dropping it in silence.
        RecipeRenderClass::Deferred
    }

    fn domain(&self) -> DomainKey {
        domain_key("surface.grit")
    }

    fn domain_definition(&self) -> CandidateDomainDef {
        CandidateDomainDef {
            key: self.domain(),
            cell_m: 0.08,
            candidates_per_cell: 8,
            spacing: SpacingPolicy::Jittered,
        }
    }

    fn appearances(&self) -> Vec<&'static str> {
        vec!["soil.clod", "soil.grit"]
    }

    fn target_density(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "density", 220.0)
    }

    fn maximum_reach_m(&self, parameters: &ParameterObject) -> f64 {
        read(parameters, "radius_m", 0.022) * 3.0
    }

    fn validate(
        &self,
        parameters: &ParameterObject,
        population: &PopulationKey,
        diagnostics: &mut DiagnosticReport,
    ) {
        number(parameters, "density", 220.0, population, diagnostics);
        number(parameters, "radius_m", 0.022, population, diagnostics);
    }

    fn emit(
        &self,
        candidate: &DomainCandidate,
        context: &RecipeContext<'_>,
        output: &mut dyn RecipeOutput,
    ) {
        let seeds = &context.seeds;
        let size = candidate.latent(seeds, &stream("clod_size"));
        let base = read(context.parameters, "radius_m", 0.022) as f32;
        // Two populations in one: a few big clods and a lot of fine grit. A
        // single size reads as gravel, which is the wrong material.
        let big = size > 0.72;
        let radius = if big {
            base * (1.4 + 1.6 * size)
        } else {
            base * (0.25 + 0.6 * size)
        };

        // Loose material sorts: fines wash into the hollows and the coarse
        // fragments stay on the crowns. Reading curvature here is what makes
        // the scatter look deposited rather than sprinkled.
        let curvature = context.fields.curvature(candidate.position);
        let hollow = smoothstep(0.5, -0.5, curvature);
        let wetness = hollow;

        output.emit(EmittedMark::Analytic {
            centre: [
                candidate.position.u_m,
                candidate.position.v_m,
                context.surface_z_m - (radius * 0.35) as f64,
            ],
            radius_m: [
                radius,
                radius * (0.6 + 0.4 * candidate.latent(seeds, &stream("clod_squash"))),
            ],
            height_m: radius * (0.45 + 0.5 * candidate.latent(seeds, &stream("clod_height"))),
            rotation_rad: candidate.latent_range(
                seeds,
                &stream("clod_rotation"),
                0.0,
                std::f32::consts::TAU,
            ),
            attributes: MarkAttributes {
                maturity: 0.6,
                // Darker where water collects, which is the tonal sweep across
                // the mud in both reference plates.
                moisture: wetness,
                exposure: context.fields.exposure(candidate.position),
                tint: candidate.latent_range(seeds, &stream("clod_tint"), -1.0, 1.0),
                variation: size,
            },
            appearance: if big { 0 } else { 1 },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::coords::{CellCoord, WorldPoint, WorldRect};
    use terrain_core::seed::{CandidateId, PopulationHash, RootSeed, SeedContext};
    use terrain_scene::field::{FieldGridSpec, TerrainFieldStack};

    fn context_fields() -> TerrainFieldStack {
        TerrainFieldStack::flat(FieldGridSpec::covering(
            WorldRect::new(WorldPoint::new(-1.0, -1.0), WorldPoint::new(1.0, 1.0)),
            0.05,
        ))
    }

    fn candidate() -> DomainCandidate {
        DomainCandidate {
            id: CandidateId::new(PopulationHash::from_bits(0x1234), CellCoord::new(1, 2), 3),
            position: WorldPoint::new(0.1, 0.2),
            priority: 0.5,
            footprint_radius_m: 0.0,
        }
    }

    fn emit_all(recipe: &dyn TerrainRecipe) -> crate::recipe::CollectedEmissions {
        let fields = std::sync::Arc::new(context_fields());
        let ground = crate::ground::GroundEvaluator::bare(
            std::sync::Arc::clone(&fields),
            crate::transition::TransitionProfile::SMOOTH,
            0xabcd,
        );
        let ground_sample = ground.sample(glam::Vec2::new(0.1, 0.2));
        let context = RecipeContext {
            fields: &fields,
            ground: &ground,
            ground_sample: &ground_sample,
            seeds: SeedContext::new(RootSeed::new(0xabcd), recipe.version()),
            parameters: &ParameterObject::default(),
            substrate: crate::transition::realise(
                [(terrain_core::ids::MaterialIndex(0), 1.0)],
                WorldPoint::new(0.1, 0.2),
                &crate::transition::TransitionProfile::SMOOTH,
                0xabcd,
            ),
            surface_z_m: 0.0,
            root_seed: 0xabcd,
        };
        let mut out = crate::recipe::CollectedEmissions::default();
        recipe.emit(&candidate(), &context, &mut out);
        out
    }

    #[test]
    fn every_family_registers_under_its_own_key() {
        let registry = family_registry();
        assert_eq!(registry.len(), 7, "seven families");
        for key in [
            "population.grass_tuft",
            "population.grass_fine",
            "population.ground_thatch",
            "population.meadow_flowers",
            "population.meadow_undergrowth",
            "population.field_stones",
            "population.dirt_clods",
        ] {
            assert!(
                registry.contains(&RecipeKey::new(key).expect("valid")),
                "{key} is not registered"
            );
        }
    }

    #[test]
    fn every_family_grows_something() {
        let registry = family_registry();
        for name in registry.keys() {
            let recipe = registry
                .get(&RecipeKey::new(name).expect("valid"))
                .expect("registered");
            let emitted = emit_all(recipe);
            assert!(
                !emitted.marks.is_empty(),
                "{name} emitted nothing for an accepted candidate"
            );
        }
    }

    #[test]
    fn a_tuft_is_several_blades_and_not_one() {
        // The property the whole family rests on: falling acceptance has to
        // remove whole clumps, which only means anything if a clump is more
        // than one blade.
        let emitted = emit_all(&GrassTuft);
        assert!(
            emitted.marks.len() >= 4,
            "a tuft came out as {} marks",
            emitted.marks.len()
        );
    }

    #[test]
    fn a_flower_is_a_stem_a_whorl_and_a_disk() {
        // A disk on a stick is not a flower. What makes one recognisable at
        // this framing is the *silhouette* — separate blades with gaps between
        // them — so the petals are their own marks rather than a texture on the
        // head.
        let emitted = emit_all(&MeadowFlowers);
        assert!(matches!(emitted.marks[0], EmittedMark::Curve { .. }));
        let petals = emitted.marks.len() - 2;
        assert!(
            (5..=8).contains(&petals),
            "a flower grew {petals} petals, outside the declared five to eight"
        );
        // Every petal, then the disk last so it sorts over their roots.
        for mark in &emitted.marks[1..] {
            assert!(matches!(mark, EmittedMark::Analytic { .. }));
        }
        let EmittedMark::Analytic { appearance, .. } = emitted.marks[emitted.marks.len() - 1]
        else {
            panic!("the last mark is the disk");
        };
        assert_eq!(appearance, 1, "the disk is emitted last");
    }

    #[test]
    fn every_petal_rings_the_disk_rather_than_growing_from_its_centre() {
        // A whorl whose petals all start at the head's own centre reads as a
        // star rather than as a flower: real petals attach around a
        // receptacle, and the gap is most of what the eye reads.
        let emitted = emit_all(&MeadowFlowers);
        let EmittedMark::Analytic { centre: disk, .. } = emitted.marks[emitted.marks.len() - 1]
        else {
            panic!("the last mark is the disk");
        };
        let mut offsets = Vec::new();
        for mark in &emitted.marks[1..emitted.marks.len() - 1] {
            let EmittedMark::Analytic { centre, .. } = mark else {
                panic!("a petal is analytic");
            };
            let d = ((centre[0] - disk[0]).powi(2) + (centre[1] - disk[1]).powi(2)).sqrt();
            assert!(d > 0.0, "a petal sits on the disk's own centre");
            offsets.push(d);
        }
        // And they ring it at a consistent radius rather than scattering.
        let low = offsets.iter().cloned().fold(f64::INFINITY, f64::min);
        let high = offsets.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        // A relative tolerance: the offsets are computed in `f32` and
        // converted, so they agree to about seven digits rather than exactly.
        assert!(
            (high - low) < high * 1.0e-5,
            "petals sit at {low}..{high}, which is not one ring"
        );
    }

    #[test]
    fn every_familys_reach_bounds_what_it_emits() {
        // The halo is sized from `maximum_reach_m`, and a mark that reaches
        // further is present on one side of a page join and missing on the
        // other. A bound that is not a bound is the silent version of a seam.
        let registry = family_registry();
        let parameters = ParameterObject::default();
        for name in registry.keys() {
            let recipe = registry
                .get(&RecipeKey::new(name).expect("valid"))
                .expect("registered");
            let reach = recipe.maximum_reach_m(&parameters);
            assert!(reach > 0.0 && reach.is_finite(), "{name} has no reach");

            let emitted = emit_all(recipe);
            let anchor = candidate().position;
            for mark in &emitted.marks {
                let (position, extent) = match mark {
                    EmittedMark::Ribbon { root, geometry, .. } => {
                        ([root[0], root[1]], geometry.reach_m() as f64)
                    }
                    EmittedMark::Curve {
                        root,
                        length_m,
                        radius_m,
                        ..
                    } => ([root[0], root[1]], (*length_m + *radius_m) as f64),
                    EmittedMark::Analytic {
                        centre,
                        radius_m,
                        height_m,
                        ..
                    } => (
                        [centre[0], centre[1]],
                        radius_m[0].max(radius_m[1]).max(*height_m) as f64,
                    ),
                };
                let offset = ((position[0] - anchor.u_m).powi(2)
                    + (position[1] - anchor.v_m).powi(2))
                .sqrt();
                assert!(
                    offset + extent <= reach + 1.0e-6,
                    "{name} reaches {} from its anchor but declares {reach}",
                    offset + extent
                );
            }
        }
    }

    #[test]
    fn the_same_candidate_grows_the_same_thing_twice() {
        let first = emit_all(&GrassTuft);
        let second = emit_all(&GrassTuft);
        assert_eq!(first.marks.len(), second.marks.len());
        for (a, b) in first.marks.iter().zip(second.marks.iter()) {
            match (a, b) {
                (
                    EmittedMark::Ribbon {
                        root: ra,
                        geometry: ga,
                        ..
                    },
                    EmittedMark::Ribbon {
                        root: rb,
                        geometry: gb,
                        ..
                    },
                ) => {
                    assert_eq!(ra, rb);
                    assert_eq!(ga.length_m, gb.length_m);
                    assert_eq!(ga.azimuth_rad, gb.azimuth_rad);
                }
                _ => panic!("a tuft changed shape between runs"),
            }
        }
    }
}

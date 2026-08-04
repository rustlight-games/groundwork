//! Compiling a document into something that can be sampled.
//!
//! ## Why the document is not sampled directly
//!
//! A [`TerrainDocument`] is a good thing to author, edit, diff and validate, and
//! a bad thing to evaluate ten million times. Every layer names its source by
//! string; every material weight names its material by string. Sampling it
//! directly would mean a map lookup per layer per point, and it would mean every
//! sample carrying the possibility of a reference that does not resolve.
//!
//! So the document is *compiled* first, once, and what comes out cannot fail:
//!
//! - Keys are resolved to dense indices, so the inner loop compares integers.
//! - Sources are compiled to [`ScalarField`]s, so a raster's transform is
//!   precomputed and a spline's index is built.
//! - Layers are flattened into an evaluation order.
//! - Anything unsupported is rejected **here**, before any sampling happens.
//!
//! That last point is the one that matters most. A sampler that can fail is a
//! sampler whose caller has to decide what to do about a failure ten million
//! times, in the middle of a scatter, with no sensible answer available. Moving
//! every possible failure to a single fallible step at the front means the
//! sampling API is total.
//!
//! ## Immutable and shared
//!
//! [`PreparedTerrain`] is `Send + Sync` and holds nothing mutable. Baking is
//! embarrassingly parallel — every page is an independent pure function of world
//! position — and the only way to keep it that way is for the thing every thread
//! reads to be genuinely read-only. An `Arc` of one of these is what a worker
//! pool shares.
//!
//! ## What is not built yet
//!
//! Rasters, splines and shapes compile to a reported error rather than a field.
//! They need an image decoder and a spatial index, which are their own commit;
//! what matters now is that the *shape* is right — a document naming one gets a
//! diagnostic saying so, rather than silently sampling zero.

use std::sync::Arc;

use crate::coords::{WorldPoint, WorldRect};
use crate::diagnostics::{DiagnosticReport, Location};
use crate::digest::Fingerprint;
use crate::document::*;
use crate::ids::*;
use crate::registry::{
    AssetResolver, ConstantField, ScalarField, SourceContext, SourceRegistry, document_assets,
};
use crate::sample::*;
use crate::seed::{RootSeed, SeedContext};

/// How to compile.
///
/// Both default to off, and both defaults are the permissive one on purpose:
/// the most common caller is validation in CI, which has no assets checked out
/// and should still be able to check everything that does not need them.
#[derive(Clone, Debug, Default)]
pub struct PrepareOptions {
    /// Check that every asset a document names can be read.
    pub require_assets: bool,
    /// Refuse a document that produces warnings as well as one that errors.
    pub deny_warnings: bool,
}

/// Everything that went wrong while compiling.
#[derive(Debug)]
pub struct PrepareReport {
    pub diagnostics: DiagnosticReport,
}

impl std::fmt::Display for PrepareReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.diagnostics)
    }
}

impl std::error::Error for PrepareReport {}

/// One compiled layer.
struct CompiledLayer {
    key: LayerKey,
    /// Index into [`PreparedTerrain::fields`], or `None` for `Everywhere`.
    source: Option<usize>,
    shape: Profile,
    operation: CompiledOperation,
}

enum CompiledOperation {
    Material {
        material: MaterialIndex,
        mode: MaterialMode,
        amount: f32,
    },
    Elevation {
        mode: HeightMode,
        height_m: f32,
    },
    Microrelief {
        mode: HeightMode,
        displacement_m: f32,
    },
    Modifier {
        channel: ModifierIndex,
        mode: ModifierComposition,
        value: f32,
    },
}

/// One compiled population.
pub struct CompiledPopulation {
    pub key: PopulationKey,
    pub index: PopulationIndex,
    pub recipe: RecipeKey,
    pub seed_stream: StreamKey,
    /// Affinity per material index, in index order. Empty means "anywhere".
    pub material_affinity: Vec<(MaterialIndex, f32)>,
    pub abundance_channel: Option<ModifierIndex>,
    pub parameters: ParameterObject,
}

/// A document, compiled and ready to sample.
///
/// Immutable, `Send + Sync`, and cheap to share through an [`Arc`].
pub struct PreparedTerrain {
    document_digest: Fingerprint,
    root_seed: RootSeed,
    materials: Vec<MaterialDef>,
    channels: Vec<ModifierChannelDef>,
    /// Each channel's default, so a sample starts from them without a lookup.
    channel_defaults: Vec<f32>,
    fields: Vec<Box<dyn ScalarField>>,
    /// The source key each field came from, for debugging and soloing.
    field_keys: Vec<SourceKey>,
    layers: Vec<CompiledLayer>,
    populations: Vec<CompiledPopulation>,
    /// The largest reach of any source, for sizing a bake's halo.
    reach_m: f64,
}

// The fields are boxed trait objects, which are `Send + Sync` by their own
// bound; everything else is plain data. Stated rather than derived so that
// adding a non-shareable field is a compile error here.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PreparedTerrain>();
};

impl std::fmt::Debug for PreparedTerrain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedTerrain")
            .field("digest", &self.document_digest)
            .field("materials", &self.materials.len())
            .field("channels", &self.channels.len())
            .field("fields", &self.fields.len())
            .field("layers", &self.layers.len())
            .field("populations", &self.populations.len())
            .finish()
    }
}

/// Compile a document.
pub fn prepare(
    document: &TerrainDocument,
    resolver: &dyn AssetResolver,
    registry: &SourceRegistry,
    options: &PrepareOptions,
) -> Result<Arc<PreparedTerrain>, PrepareReport> {
    let mut diagnostics = crate::validate::validate(document);

    // Assets, before anything is compiled. A document naming six missing masks
    // should say so once rather than failing on the first.
    if options.require_assets {
        for asset in document_assets(document) {
            if !resolver.exists(asset) {
                diagnostics.error(
                    "missing_asset",
                    Location::at("sources"),
                    format!("`{asset}` cannot be read"),
                );
            }
        }
    }

    let materials = document.materials.clone();
    let material_index = |key: &MaterialKey| -> Option<MaterialIndex> {
        materials
            .iter()
            .position(|m| &m.key == key)
            .map(|i| MaterialIndex(i as u16))
    };

    let channels = document.modifier_channels.clone();
    let channel_defaults: Vec<f32> = channels.iter().map(|c| c.default_value).collect();
    let channel_index = |key: &ModifierKey| -> Option<ModifierIndex> {
        channels
            .iter()
            .position(|c| &c.key == key)
            .map(|i| ModifierIndex(i as u16))
    };

    // Sources become fields, in document order, so a layer's reference is an
    // index into this list.
    let mut fields: Vec<Box<dyn ScalarField>> = Vec::with_capacity(document.sources.len());
    let mut field_keys: Vec<SourceKey> = Vec::with_capacity(document.sources.len());
    for (index, definition) in document.sources.iter().enumerate() {
        let at = Location::at(format!("sources[{index}]"));
        let field = compile_source(definition, &at, resolver, registry, &mut diagnostics);
        // A source that failed still occupies its slot, holding a field that
        // reads zero — so every later index stays correct and the diagnostics
        // stay aligned with the document. Without it, one bad source would
        // renumber every reference after it and produce a cascade of unrelated
        // errors.
        fields.push(field.unwrap_or_else(|| Box::new(ConstantField { value: 0.0 })));
        field_keys.push(definition.key.clone());
    }

    let source_slot =
        |key: &SourceKey| -> Option<usize> { field_keys.iter().position(|k| k == key) };

    let mut layers = Vec::with_capacity(document.layers.len());
    for definition in document.layers.iter().filter(|l| l.enabled) {
        let (source, shape) = match &definition.mask {
            Mask::Everywhere => (None, Profile::Passthrough),
            Mask::Source(key) => (source_slot(key), Profile::Passthrough),
            Mask::Profile { source, shape } => (source_slot(source), *shape),
        };
        // A layer whose source did not resolve was already reported by
        // validation; skipping it here keeps the compiled form free of
        // dangling references.
        if matches!(definition.mask, Mask::Source(_) | Mask::Profile { .. }) && source.is_none() {
            continue;
        }

        let operation = match &definition.operation {
            LayerOperation::Material(operation) => {
                let Some(material) = material_index(&operation.material) else {
                    continue;
                };
                CompiledOperation::Material {
                    material,
                    mode: operation.mode,
                    amount: operation.amount,
                }
            }
            LayerOperation::Elevation(operation) => CompiledOperation::Elevation {
                mode: operation.mode,
                height_m: operation.height_m,
            },
            LayerOperation::Microrelief(operation) => CompiledOperation::Microrelief {
                mode: operation.mode,
                displacement_m: operation.displacement_m,
            },
            LayerOperation::Modifier(operation) => {
                let Some(channel) = channel_index(&operation.channel) else {
                    continue;
                };
                CompiledOperation::Modifier {
                    channel,
                    mode: operation.mode,
                    value: operation.value,
                }
            }
        };

        layers.push(CompiledLayer {
            key: definition.key.clone(),
            source,
            shape,
            operation,
        });
    }

    let mut populations = Vec::with_capacity(document.populations.len());
    for (index, definition) in document
        .populations
        .iter()
        .filter(|p| p.enabled)
        .enumerate()
    {
        let material_affinity = definition
            .material_affinity
            .iter()
            .filter_map(|affinity| {
                material_index(&affinity.material).map(|index| (index, affinity.weight))
            })
            .collect();
        populations.push(CompiledPopulation {
            key: definition.key.clone(),
            index: PopulationIndex(index as u16),
            recipe: definition.recipe.clone(),
            seed_stream: definition.seed_stream.clone(),
            material_affinity,
            abundance_channel: definition
                .abundance_channel
                .as_ref()
                .and_then(&channel_index),
            parameters: definition.parameters.clone(),
        });
    }

    if diagnostics.has_errors() || (options.deny_warnings && !diagnostics.is_empty()) {
        return Err(PrepareReport { diagnostics });
    }

    let reach_m = fields
        .iter()
        .map(|field| field.reach_m())
        .fold(0.0f64, f64::max);

    Ok(Arc::new(PreparedTerrain {
        document_digest: document.digest(),
        root_seed: document.root_seed,
        materials,
        channels,
        channel_defaults,
        fields,
        field_keys,
        layers,
        populations,
        reach_m,
    }))
}

/// Compile one source into a field, or report why not.
fn compile_source(
    definition: &SourceDef,
    at: &Location,
    resolver: &dyn AssetResolver,
    registry: &SourceRegistry,
    diagnostics: &mut DiagnosticReport,
) -> Option<Box<dyn ScalarField>> {
    match &definition.source {
        Source::Constant(constant) => Some(Box::new(ConstantField {
            value: constant.value,
        })),
        Source::Custom(custom) => match registry.get(&custom.recipe) {
            Some(recipe) => {
                let mut context = SourceContext {
                    parameters: &custom.parameters,
                    resolver,
                    diagnostics,
                };
                recipe.compile(&mut context)
            }
            None => {
                diagnostics.error(
                    "unknown_recipe",
                    at.clone(),
                    format!(
                        "`{}` names the source recipe `{}`, which is not registered \
                         in this binary",
                        definition.key, custom.recipe
                    ),
                );
                None
            }
        },
        // Not built yet, and reported rather than silently sampling zero. See
        // the module note.
        other => {
            diagnostics.error(
                "unsupported_source",
                at.clone(),
                format!(
                    "`{}` is a {} source, which this build cannot compile yet",
                    definition.key,
                    other.kind_name()
                ),
            );
            None
        }
    }
}

impl PreparedTerrain {
    /// The digest of the document this was compiled from.
    pub fn document_digest(&self) -> Fingerprint {
        self.document_digest
    }

    pub fn root_seed(&self) -> RootSeed {
        self.root_seed
    }

    /// A seed context for a recipe of the given version.
    pub fn seeds(&self, recipe_version: u32) -> SeedContext {
        SeedContext::new(self.root_seed, recipe_version)
    }

    /// How far outside a region a bake has to look.
    ///
    /// The halo a page needs so that a source reaching in from outside is
    /// accounted for. Zero for a document of unbounded sources.
    pub fn reach_m(&self) -> f64 {
        self.reach_m
    }

    pub fn materials(&self) -> &[MaterialDef] {
        &self.materials
    }

    pub fn channels(&self) -> &[ModifierChannelDef] {
        &self.channels
    }

    pub fn populations(&self) -> &[CompiledPopulation] {
        &self.populations
    }

    /// The layers that survived compilation, in evaluation order.
    ///
    /// Not the document's layer list: disabled layers and layers whose
    /// references did not resolve are gone. That difference is the point — an
    /// author looking at a terrain that is missing something wants to know which
    /// layers *ran*, and a list that still showed the ones that were skipped
    /// would answer a different question.
    pub fn layer_keys(&self) -> impl Iterator<Item = &LayerKey> {
        self.layers.iter().map(|layer| &layer.key)
    }

    /// Every source key, in document order.
    pub fn source_keys(&self) -> impl Iterator<Item = &SourceKey> {
        self.field_keys.iter()
    }

    /// The index a material key resolves to.
    pub fn material_index(&self, key: &MaterialKey) -> Option<MaterialIndex> {
        self.materials
            .iter()
            .position(|m| &m.key == key)
            .map(|i| MaterialIndex(i as u16))
    }

    /// The key an index came from.
    pub fn material_key(&self, index: MaterialIndex) -> Option<&MaterialKey> {
        self.materials.get(index.index()).map(|m| &m.key)
    }

    pub fn channel_index(&self, key: &ModifierKey) -> Option<ModifierIndex> {
        self.channels
            .iter()
            .position(|c| &c.key == key)
            .map(|i| ModifierIndex(i as u16))
    }

    pub fn channel_key(&self, index: ModifierIndex) -> Option<&ModifierKey> {
        self.channels.get(index.index()).map(|c| &c.key)
    }

    /// Read one source directly, for a debug view or `terrain inspect`.
    pub fn source_value(&self, key: &SourceKey, query: &SampleQuery) -> Option<f32> {
        let slot = self.field_keys.iter().position(|k| k == key)?;
        Some(self.fields[slot].value_at(query.position, query.footprint))
    }

    /// Sample the terrain.
    ///
    /// Total: there is no failure to handle here, because everything that could
    /// fail was rejected by [`prepare`].
    pub fn sample(&self, query: &SampleQuery) -> TerrainSample {
        let channels = query.channels;
        let mut scores: Vec<f32> = vec![0.0; self.materials.len()];
        let mut elevation_m = 0.0f32;
        let mut displacement_m = 0.0f32;
        let mut modifiers = ModifierSet::from_defaults(&self.channel_defaults);

        for layer in &self.layers {
            // The mask, once per layer. Layers that do not apply here cost one
            // field read and nothing else.
            let mask = match layer.source {
                None => 1.0,
                Some(slot) => {
                    let raw = self.fields[slot].value_at(query.position, query.footprint);
                    apply_profile(layer.shape, raw)
                }
            };
            if mask <= 0.0 {
                continue;
            }

            match &layer.operation {
                CompiledOperation::Material {
                    material,
                    mode,
                    amount,
                } => {
                    if !channels.contains(SampleChannels::MATERIALS) {
                        continue;
                    }
                    let slot = material.index();
                    let contribution = amount * mask;
                    match mode {
                        MaterialMode::Replace => {
                            // Everything else is cleared in proportion to how
                            // much this layer claims. A partial `Replace` at a
                            // mask of 0.5 leaves half of what was there, which
                            // is what makes a hard-edged claim fade rather than
                            // pop as its mask falls off.
                            for (index, score) in scores.iter_mut().enumerate() {
                                if index != slot {
                                    *score *= 1.0 - mask;
                                }
                            }
                            scores[slot] = scores[slot] * (1.0 - mask) + contribution;
                        }
                        MaterialMode::AddScore => scores[slot] += contribution,
                        MaterialMode::MultiplyScore => {
                            // Interpolated by the mask, so a partial claim is a
                            // partial multiply rather than an all-or-nothing
                            // one.
                            scores[slot] *= 1.0 + (amount - 1.0) * mask;
                        }
                    }
                }
                CompiledOperation::Elevation { mode, height_m } => {
                    if channels.contains(SampleChannels::ELEVATION) {
                        elevation_m = combine_height(*mode, elevation_m, *height_m, mask);
                    }
                }
                CompiledOperation::Microrelief {
                    mode,
                    displacement_m: value,
                } => {
                    if channels.contains(SampleChannels::MICRORELIEF) {
                        displacement_m = combine_height(*mode, displacement_m, *value, mask);
                    }
                }
                CompiledOperation::Modifier {
                    channel,
                    mode,
                    value,
                } => {
                    if !channels.contains(SampleChannels::MODIFIERS) {
                        continue;
                    }
                    let current = modifiers.get_or(*channel, 0.0);
                    // Interpolated by the mask, so a suppression fades in over
                    // its own transition rather than switching on.
                    let combined = mode.combine(current, *value);
                    modifiers.set(*channel, current + (combined - current) * mask);
                }
            }
        }

        // Clamp every channel to its declared range, once, at the end. Clamping
        // per layer would make the result depend on the order of layers that
        // each individually overshoot.
        for (index, channel) in self.channels.iter().enumerate() {
            let slot = ModifierIndex(index as u16);
            if let Some(value) = modifiers.get(slot) {
                modifiers.set(slot, channel.range.clamp(value));
            }
        }

        TerrainSample {
            material_weights: if channels.contains(SampleChannels::MATERIALS) {
                MaterialWeightSet::from_scores(
                    scores
                        .iter()
                        .enumerate()
                        .map(|(index, score)| (MaterialIndex(index as u16), *score)),
                )
            } else {
                MaterialWeightSet::empty()
            },
            elevation_m,
            microrelief: MicroreliefSample {
                displacement_m,
                gradient: [0.0, 0.0],
            },
            modifiers,
            feature_context: None,
        }
    }

    /// Sample many points at once.
    ///
    /// The shape a bake actually wants. It is a loop today and the *signature*
    /// is what matters: a caller written against this can be handed a threaded
    /// or vectorised implementation later without changing, whereas a caller
    /// written against `sample` in its own loop cannot.
    pub fn sample_batch(&self, queries: &[SampleQuery], into: &mut Vec<TerrainSample>) {
        into.clear();
        into.reserve(queries.len());
        for query in queries {
            into.push(self.sample(query));
        }
    }

    /// Sample a regular grid over a rectangle, row-major from the minimum
    /// corner.
    ///
    /// Each sample's footprint is one cell, which is the whole reason this is a
    /// method rather than something a caller writes: the footprint has to match
    /// the spacing, and a caller computing it by hand gets it wrong in exactly
    /// the way that aliases.
    pub fn sample_grid(
        &self,
        bounds: WorldRect,
        columns: u32,
        rows: u32,
        channels: SampleChannels,
    ) -> Vec<TerrainSample> {
        let (columns, rows) = (columns.max(1), rows.max(1));
        let step_u = bounds.width_m() / columns as f64;
        let step_v = bounds.height_m() / rows as f64;
        let footprint = SampleFootprint::Ellipse {
            axis_u: crate::coords::WorldVector::new(step_u * 0.5, 0.0),
            axis_v: crate::coords::WorldVector::new(0.0, step_v * 0.5),
        };

        let mut out = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for column in 0..columns {
                // Cell centres, so the grid samples the area it covers rather
                // than its corners.
                let position = WorldPoint::new(
                    bounds.min.u_m + (column as f64 + 0.5) * step_u,
                    bounds.min.v_m + (row as f64 + 0.5) * step_v,
                );
                out.push(self.sample(&SampleQuery {
                    position,
                    footprint,
                    channels,
                }));
            }
        }
        out
    }
}

/// Shape a source's raw value into a `0..1` mask.
fn apply_profile(shape: Profile, raw: f32) -> f32 {
    match shape {
        Profile::Passthrough => raw.clamp(0.0, 1.0),
        Profile::Threshold { at } => f32::from(raw < at),
        Profile::Ramp { low, high } => {
            if low == high {
                return 0.0;
            }
            ((raw - low) / (high - low)).clamp(0.0, 1.0)
        }
        Profile::SmoothBand { inner_m, outer_m } => {
            let (inner, outer) = (inner_m as f32, outer_m as f32);
            if outer <= inner {
                return f32::from(raw <= inner);
            }
            // One inside, zero outside, smoothstep between. Smoothstep rather
            // than linear because a linear falloff has a visible crease at both
            // ends — the derivative jumps — and a path edge is exactly where
            // that crease is most legible.
            let t = ((outer - raw) / (outer - inner)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }
    }
}

/// Combine a height contribution, faded by its mask.
fn combine_height(mode: HeightMode, current: f32, value: f32, mask: f32) -> f32 {
    let combined = match mode {
        HeightMode::Add => current + value,
        HeightMode::Replace => value,
        HeightMode::Max => current.max(value),
        HeightMode::Min => current.min(value),
    };
    current + (combined - current) * mask
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::NoAssets;

    fn material(key: &str) -> MaterialDef {
        MaterialDef {
            key: MaterialKey::new(key).expect("valid"),
            display_name: key.into(),
            appearance: AppearanceKey::new(format!("surface.{key}")).expect("valid"),
        }
    }

    fn constant_source(key: &str, value: f32) -> SourceDef {
        SourceDef {
            key: SourceKey::new(key).expect("valid"),
            source: Source::Constant(ConstantSource { value }),
        }
    }

    fn constant_grass() -> TerrainDocument {
        TerrainDocument {
            root_seed: RootSeed::new(0x8df7_82f9_5ce1_a4d4),
            materials: vec![material("grass_lush")],
            modifier_channels: vec![ModifierChannelDef {
                key: ModifierKey::new("vegetation_density").expect("valid"),
                display_name: "Vegetation density".into(),
                range: ValueRange::new(0.0, 1.5),
                default_value: 1.0,
                composition: ModifierComposition::Multiply,
                unit: ModifierUnit::Unitless,
            }],
            sources: vec![constant_source("everywhere", 1.0)],
            layers: vec![LayerDef {
                key: LayerKey::new("base_grass").expect("valid"),
                enabled: true,
                mask: Mask::Source(SourceKey::new("everywhere").expect("valid")),
                operation: LayerOperation::Material(MaterialLayer {
                    material: MaterialKey::new("grass_lush").expect("valid"),
                    mode: MaterialMode::Replace,
                    amount: 1.0,
                }),
            }],
            populations: vec![PopulationDef {
                key: PopulationKey::new("grass_population").expect("valid"),
                recipe: RecipeKey::new("population.grass_lush").expect("valid"),
                enabled: true,
                seed_stream: StreamKey::new("grass").expect("valid"),
                material_affinity: vec![MaterialAffinity {
                    material: MaterialKey::new("grass_lush").expect("valid"),
                    weight: 1.0,
                }],
                abundance_channel: Some(ModifierKey::new("vegetation_density").expect("valid")),
                parameters: ParameterObject::new(),
            }],
            ..TerrainDocument::default()
        }
    }

    fn prepared(document: &TerrainDocument) -> Arc<PreparedTerrain> {
        match prepare(
            document,
            &NoAssets,
            &SourceRegistry::new(),
            &PrepareOptions::default(),
        ) {
            Ok(prepared) => prepared,
            Err(report) => panic!("{report}"),
        }
    }

    #[test]
    fn the_constant_grass_document_prepares_and_samples() {
        // The milestone: from document to a sampled point, with nothing in
        // between that can fail.
        let terrain = prepared(&constant_grass());
        let sample = terrain.sample(&SampleQuery::at(WorldPoint::new(3.0, -7.0)));

        assert_eq!(sample.material_weights.len(), 1);
        let grass = terrain
            .material_index(&MaterialKey::new("grass_lush").expect("valid"))
            .expect("declared");
        assert!((sample.material_weights.weight_of(grass) - 1.0).abs() < 1.0e-6);
        assert_eq!(sample.elevation_m, 0.0);
        assert_eq!(sample.microrelief.displacement_m, 0.0);
        assert_eq!(
            sample.modifiers.get(ModifierIndex(0)),
            Some(1.0),
            "the channel did not start from its default"
        );
    }

    #[test]
    fn constant_grass_is_the_same_everywhere() {
        // It is called constant grass. If this ever varies, a source is reading
        // something it should not.
        let terrain = prepared(&constant_grass());
        let reference = terrain.sample(&SampleQuery::at(WorldPoint::ORIGIN));
        for point in [
            WorldPoint::new(1.0, 1.0),
            WorldPoint::new(-4096.0, 8192.0),
            WorldPoint::new(0.5, -0.5),
        ] {
            assert_eq!(
                terrain.sample(&SampleQuery::at(point)),
                reference,
                "{point}"
            );
        }
    }

    #[test]
    fn sampling_is_a_pure_function_of_position() {
        // The property every page-independence claim rests on.
        let terrain = prepared(&constant_grass());
        let query = SampleQuery::at(WorldPoint::new(12.25, -3.5));
        let first = terrain.sample(&query);
        for _ in 0..4 {
            assert_eq!(terrain.sample(&query), first);
        }
    }

    #[test]
    fn a_second_material_blends_by_its_mask() {
        // The composition the whole blending design rests on: a partial claim
        // produces a mixture, not a switch.
        let mut document = constant_grass();
        document.materials.push(material("dirt_compacted"));
        document.sources.push(constant_source("half", 0.25));
        document.layers.push(LayerDef {
            key: LayerKey::new("path_material").expect("valid"),
            enabled: true,
            mask: Mask::Source(SourceKey::new("half").expect("valid")),
            operation: LayerOperation::Material(MaterialLayer {
                material: MaterialKey::new("dirt_compacted").expect("valid"),
                mode: MaterialMode::AddScore,
                amount: 1.0,
            }),
        });

        let terrain = prepared(&document);
        let sample = terrain.sample(&SampleQuery::at(WorldPoint::ORIGIN));
        assert_eq!(sample.material_weights.len(), 2);
        let dirt = terrain
            .material_index(&MaterialKey::new("dirt_compacted").expect("valid"))
            .expect("declared");
        // Scores of 1.0 grass and 0.25 dirt, normalised.
        assert!(
            (sample.material_weights.weight_of(dirt) - 0.2).abs() < 1.0e-5,
            "{:?}",
            sample.material_weights
        );
        let total: f32 = sample.material_weights.iter().map(|w| w.weight).sum();
        assert!((total - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn a_suppression_fades_in_over_its_own_mask() {
        // A modifier at a mask of 0.5 must be half applied, not switched on.
        // Without this, every suppression has a hard edge wherever its mask
        // crosses zero, however smooth the mask is.
        let mut document = constant_grass();
        document.sources.push(constant_source("half", 0.5));
        document.layers.push(LayerDef {
            key: LayerKey::new("suppression").expect("valid"),
            enabled: true,
            mask: Mask::Source(SourceKey::new("half").expect("valid")),
            operation: LayerOperation::Modifier(ModifierLayer {
                channel: ModifierKey::new("vegetation_density").expect("valid"),
                mode: ModifierComposition::Multiply,
                value: 0.0,
            }),
        });

        let terrain = prepared(&document);
        let sample = terrain.sample(&SampleQuery::at(WorldPoint::ORIGIN));
        // Starts at 1.0; multiplying by zero at half strength gives a half.
        assert!(
            (sample.modifiers.get(ModifierIndex(0)).expect("declared") - 0.5).abs() < 1.0e-6,
            "{:?}",
            sample.modifiers
        );
    }

    #[test]
    fn a_channel_is_clamped_to_its_range_once_at_the_end() {
        // Clamping per layer would make the result depend on the order of
        // layers that each individually overshoot.
        let mut document = constant_grass();
        document.layers.push(LayerDef {
            key: LayerKey::new("boost").expect("valid"),
            enabled: true,
            mask: Mask::Everywhere,
            operation: LayerOperation::Modifier(ModifierLayer {
                channel: ModifierKey::new("vegetation_density").expect("valid"),
                mode: ModifierComposition::Multiply,
                value: 100.0,
            }),
        });
        let terrain = prepared(&document);
        let sample = terrain.sample(&SampleQuery::at(WorldPoint::ORIGIN));
        assert_eq!(sample.modifiers.get(ModifierIndex(0)), Some(1.5));
    }

    #[test]
    fn a_disabled_layer_does_not_reach_the_compiled_form() {
        let mut document = constant_grass();
        document.materials.push(material("dirt_compacted"));
        document.sources.push(constant_source("all", 1.0));
        document.layers.push(LayerDef {
            key: LayerKey::new("dirt").expect("valid"),
            enabled: false,
            mask: Mask::Source(SourceKey::new("all").expect("valid")),
            operation: LayerOperation::Material(MaterialLayer {
                material: MaterialKey::new("dirt_compacted").expect("valid"),
                mode: MaterialMode::AddScore,
                amount: 1.0,
            }),
        });
        let terrain = prepared(&document);
        let sample = terrain.sample(&SampleQuery::at(WorldPoint::ORIGIN));
        assert_eq!(sample.material_weights.len(), 1, "a disabled layer applied");
    }

    #[test]
    fn a_source_this_build_cannot_compile_is_reported_rather_than_read_as_zero() {
        // The failure mode this rejection exists to prevent: a raster source
        // that silently samples zero produces a document that loads, validates,
        // and describes nothing.
        let mut document = constant_grass();
        document.sources.push(SourceDef {
            key: SourceKey::new("rock_zone").expect("valid"),
            source: Source::ScalarRaster(ScalarRasterSource {
                asset: AssetPath::new("masks/rock_abundance.png").expect("valid"),
                placement: RasterPlacement {
                    origin_m: WorldPoint::new(-32.0, -32.0),
                    size_m: crate::coords::WorldVector::new(64.0, 64.0),
                    anchor: crate::coords::TexelAnchor::Centre,
                    row_order: crate::coords::RowOrder::TopDown,
                },
                filter: RasterFilter::Bilinear,
                wrap: RasterWrap::Clamp,
            }),
        });
        document.layers.push(LayerDef {
            key: LayerKey::new("rocks").expect("valid"),
            enabled: true,
            mask: Mask::Source(SourceKey::new("rock_zone").expect("valid")),
            operation: LayerOperation::Modifier(ModifierLayer {
                channel: ModifierKey::new("vegetation_density").expect("valid"),
                mode: ModifierComposition::Multiply,
                value: 0.5,
            }),
        });

        let report = prepare(
            &document,
            &NoAssets,
            &SourceRegistry::new(),
            &PrepareOptions::default(),
        )
        .expect_err("refused");
        assert!(
            report
                .diagnostics
                .entries()
                .iter()
                .any(|e| e.code == "unsupported_source"),
            "{report}"
        );
    }

    #[test]
    fn missing_assets_are_only_checked_when_asked_for() {
        // The common caller is CI validation with no assets checked out.
        let mut document = constant_grass();
        document.sources.push(SourceDef {
            key: SourceKey::new("spline").expect("valid"),
            source: Source::SplineDistance(SplineDistanceSource {
                asset: AssetPath::new("features/main_path.spline.ron").expect("valid"),
                max_distance_m: 5.0,
            }),
        });
        document.layers.push(LayerDef {
            key: LayerKey::new("path").expect("valid"),
            enabled: true,
            mask: Mask::Source(SourceKey::new("spline").expect("valid")),
            operation: LayerOperation::Modifier(ModifierLayer {
                channel: ModifierKey::new("vegetation_density").expect("valid"),
                mode: ModifierComposition::Multiply,
                value: 0.5,
            }),
        });

        let without = prepare(
            &document,
            &NoAssets,
            &SourceRegistry::new(),
            &PrepareOptions::default(),
        )
        .expect_err("splines are not compiled yet");
        assert!(
            !without
                .diagnostics
                .entries()
                .iter()
                .any(|e| e.code == "missing_asset"),
            "assets were checked without being asked for"
        );

        let with = prepare(
            &document,
            &NoAssets,
            &SourceRegistry::new(),
            &PrepareOptions {
                require_assets: true,
                ..PrepareOptions::default()
            },
        )
        .expect_err("refused");
        assert!(
            with.diagnostics
                .entries()
                .iter()
                .any(|e| e.code == "missing_asset"),
            "{with}"
        );
    }

    #[test]
    fn a_prepared_terrain_carries_its_documents_digest() {
        // What a cache is keyed on, and what a manifest pins.
        let document = constant_grass();
        let terrain = prepared(&document);
        assert_eq!(terrain.document_digest(), document.digest());

        let mut other = document.clone();
        other.metadata.name = "something else".into();
        assert_ne!(
            prepared(&other).document_digest(),
            terrain.document_digest()
        );
    }

    #[test]
    fn keys_and_indices_round_trip() {
        let terrain = prepared(&constant_grass());
        let key = MaterialKey::new("grass_lush").expect("valid");
        let index = terrain.material_index(&key).expect("declared");
        assert_eq!(terrain.material_key(index), Some(&key));
        assert_eq!(
            terrain.material_index(&MaterialKey::new("nothing").expect("valid")),
            None
        );
    }

    #[test]
    fn a_source_can_be_read_on_its_own_for_a_debug_view() {
        let terrain = prepared(&constant_grass());
        let query = SampleQuery::at(WorldPoint::ORIGIN);
        assert_eq!(
            terrain.source_value(&SourceKey::new("everywhere").expect("valid"), &query),
            Some(1.0)
        );
        assert_eq!(
            terrain.source_value(&SourceKey::new("nothing").expect("valid"), &query),
            None
        );
    }

    #[test]
    fn a_batch_matches_the_same_points_sampled_one_at_a_time() {
        // The signature is the point — a caller written against this can be
        // handed a threaded implementation later — but it has to agree with the
        // single-point path or the two will drift.
        let terrain = prepared(&constant_grass());
        let queries: Vec<SampleQuery> = (0..16)
            .map(|i| SampleQuery::at(WorldPoint::new(i as f64 * 0.25, -i as f64)))
            .collect();
        let mut batch = Vec::new();
        terrain.sample_batch(&queries, &mut batch);
        assert_eq!(batch.len(), queries.len());
        for (query, sampled) in queries.iter().zip(&batch) {
            assert_eq!(&terrain.sample(query), sampled);
        }
    }

    #[test]
    fn a_grid_samples_cell_centres_with_cell_sized_footprints() {
        // The footprint has to match the spacing. A caller computing it by hand
        // gets it wrong in exactly the way that aliases, which is why the grid
        // is a method rather than something a caller writes.
        let terrain = prepared(&constant_grass());
        let bounds = WorldRect::new(WorldPoint::ORIGIN, WorldPoint::new(4.0, 2.0));
        let grid = terrain.sample_grid(bounds, 4, 2, SampleChannels::ALL);
        assert_eq!(grid.len(), 8);
        for sample in &grid {
            assert_eq!(sample.material_weights.len(), 1);
        }
    }

    #[test]
    fn asking_for_fewer_channels_leaves_the_rest_at_their_defaults() {
        let terrain = prepared(&constant_grass());
        let sample = terrain
            .sample(&SampleQuery::at(WorldPoint::ORIGIN).with_channels(SampleChannels::MATERIALS));
        assert_eq!(sample.material_weights.len(), 1);
        assert_eq!(sample.elevation_m, 0.0);

        let neither = terrain
            .sample(&SampleQuery::at(WorldPoint::ORIGIN).with_channels(SampleChannels::ELEVATION));
        assert!(neither.material_weights.is_empty());
    }

    #[test]
    fn a_smooth_band_is_one_inside_zero_outside_and_smooth_between() {
        assert_eq!(
            apply_profile(
                Profile::SmoothBand {
                    inner_m: 1.5,
                    outer_m: 2.6
                },
                1.0
            ),
            1.0
        );
        assert_eq!(
            apply_profile(
                Profile::SmoothBand {
                    inner_m: 1.5,
                    outer_m: 2.6
                },
                3.0
            ),
            0.0
        );
        let middle = apply_profile(
            Profile::SmoothBand {
                inner_m: 1.5,
                outer_m: 2.6,
            },
            2.05,
        );
        assert!((middle - 0.5).abs() < 0.02, "{middle}");
    }

    #[test]
    fn a_profile_never_leaves_the_unit_range() {
        // Everything downstream multiplies by a mask and would produce
        // nonsense outside it.
        for shape in [
            Profile::Passthrough,
            Profile::Threshold { at: 0.5 },
            Profile::Ramp {
                low: 0.2,
                high: 0.8,
            },
            Profile::SmoothBand {
                inner_m: 1.0,
                outer_m: 2.0,
            },
            // Degenerate ones, which validation refuses but which must still
            // not produce a value outside the range if they get here.
            Profile::Ramp {
                low: 0.5,
                high: 0.5,
            },
            Profile::SmoothBand {
                inner_m: 2.0,
                outer_m: 1.0,
            },
        ] {
            for raw in [-10.0, -1.0, 0.0, 0.5, 1.0, 2.0, 100.0, f32::NAN] {
                let mask = apply_profile(shape, raw);
                assert!(
                    (0.0..=1.0).contains(&mask) || raw.is_nan(),
                    "{shape:?} at {raw} gave {mask}"
                );
            }
        }
    }

    #[test]
    fn populations_compile_with_their_indices_resolved() {
        let terrain = prepared(&constant_grass());
        assert_eq!(terrain.populations().len(), 1);
        let population = &terrain.populations()[0];
        assert_eq!(population.key.as_str(), "grass_population");
        assert_eq!(population.abundance_channel, Some(ModifierIndex(0)));
        assert_eq!(population.material_affinity.len(), 1);
        assert_eq!(population.material_affinity[0].0, MaterialIndex(0));
    }

    #[test]
    fn a_prepared_terrain_is_shareable_across_threads() {
        // Baking is embarrassingly parallel, and the only way to keep it that
        // way is for the thing every thread reads to be genuinely read-only.
        let terrain = prepared(&constant_grass());
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let terrain = Arc::clone(&terrain);
                std::thread::spawn(move || {
                    terrain
                        .sample(&SampleQuery::at(WorldPoint::new(i as f64, 0.0)))
                        .material_weights
                        .len()
                })
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().expect("no panic"), 1);
        }
    }
}

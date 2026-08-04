//! Everything wrong with a document, found in one pass.
//!
//! ## What is checked here, and what is not
//!
//! This pass sees the document and nothing else. It can tell you that a layer
//! names a source that does not exist, that two materials share a key, or that a
//! smooth band's inner radius is outside its outer one. It cannot tell you that
//! `masks/rock_abundance.png` is missing, because it has no filesystem, or that
//! `population.granite_rocks` is unregistered unless you hand it the registry.
//!
//! That split is deliberate. A document should be checkable in a test, in a
//! language server, and in a CI job that has no assets checked out — and the
//! checks that need the world are a strictly smaller, later set.
//!
//! Pass a [`KnownRecipes`] to have recipe bindings checked too.
//!
//! ## Why an unknown reference is an error and an empty mask is a warning
//!
//! The line is whether the document still has a meaning. A layer naming
//! `main_pth` has no meaning — there is no field to sample, and no sensible
//! default that is not a guess about the author's intent. A layer whose mask is
//! zero everywhere has an unambiguous meaning: it does nothing. It is probably a
//! mistake, and it is probably somebody halfway through writing something, and
//! refusing to prepare the document would make the framework hostile to the way
//! terrain actually gets authored.

use std::collections::BTreeMap;

use crate::diagnostics::{DiagnosticReport, Location};
use crate::document::*;
use crate::ids::RecipeKey;

/// The recipe keys a registry knows about.
///
/// Passed in rather than looked up, because `terrain_core` deliberately does not
/// own a registry — the set of recipes is a property of the *binary*, and a
/// document validated against one binary's registry should not silently pass
/// against another's.
#[derive(Clone, Debug, Default)]
pub struct KnownRecipes {
    sources: Vec<RecipeKey>,
    populations: Vec<RecipeKey>,
}

impl KnownRecipes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_source(mut self, key: RecipeKey) -> Self {
        self.sources.push(key);
        self
    }

    pub fn with_population(mut self, key: RecipeKey) -> Self {
        self.populations.push(key);
        self
    }

    fn knows_source(&self, key: &RecipeKey) -> bool {
        self.sources.contains(key)
    }

    fn knows_population(&self, key: &RecipeKey) -> bool {
        self.populations.contains(key)
    }

    fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.populations.is_empty()
    }
}

/// The closest known name, for a "did you mean" line.
///
/// Cheap edit distance, capped: a suggestion that is wrong is worse than no
/// suggestion, so anything more than a third of the name away is not offered.
fn nearest<'a>(wanted: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let limit = (wanted.len() / 3).max(1);
    candidates
        .map(|candidate| (edit_distance(wanted, candidate), candidate))
        .filter(|(distance, _)| *distance <= limit)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.to_string())
}

/// Levenshtein distance, two rows at a time.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Check a document against itself.
///
/// Never stops early. Returns everything it found, whether or not anything was
/// fatal — see [`DiagnosticReport::has_errors`].
pub fn validate(document: &TerrainDocument) -> DiagnosticReport {
    validate_against(document, &KnownRecipes::new())
}

/// [`validate`], and check recipe bindings against a registry.
pub fn validate_against(document: &TerrainDocument, recipes: &KnownRecipes) -> DiagnosticReport {
    let mut report = DiagnosticReport::new();
    check_duplicate_keys(document, &mut report);
    check_materials(document, &mut report);
    check_channels(document, &mut report);
    check_sources(document, recipes, &mut report);
    check_layers(document, &mut report);
    check_populations(document, recipes, &mut report);
    check_composition(document, &mut report);
    report
}

/// Duplicate keys, in every table.
fn check_duplicate_keys(document: &TerrainDocument, report: &mut DiagnosticReport) {
    fn duplicates<'a, T>(
        items: &'a [T],
        section: &str,
        key: impl Fn(&'a T) -> &'a str,
        report: &mut DiagnosticReport,
    ) {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, item) in items.iter().enumerate() {
            let name = key(item);
            match seen.get(name) {
                Some(first) => {
                    report.error(
                        "duplicate_key",
                        Location::at(format!("{section}[{index}].key")),
                        format!("`{name}` is already defined at {section}[{first}]"),
                    );
                }
                None => {
                    seen.insert(name, index);
                }
            }
        }
    }

    duplicates(&document.materials, "materials", |m| m.key.as_str(), report);
    duplicates(
        &document.modifier_channels,
        "modifier_channels",
        |c| c.key.as_str(),
        report,
    );
    duplicates(&document.sources, "sources", |s| s.key.as_str(), report);
    duplicates(&document.layers, "layers", |l| l.key.as_str(), report);
    duplicates(
        &document.populations,
        "populations",
        |p| p.key.as_str(),
        report,
    );
}

fn check_materials(document: &TerrainDocument, report: &mut DiagnosticReport) {
    if document.materials.is_empty() {
        report.error(
            "no_materials",
            Location::at("materials"),
            "a document must declare at least one material; there would be nothing \
             for a material weight to name",
        );
    }
}

fn check_channels(document: &TerrainDocument, report: &mut DiagnosticReport) {
    for (index, channel) in document.modifier_channels.iter().enumerate() {
        let at = |field: &str| Location::at(format!("modifier_channels[{index}].{field}"));
        if !channel.range.is_valid() {
            report.error(
                "invalid_range",
                at("range"),
                format!(
                    "`{}` has the range ({}, {}), which is not a usable interval",
                    channel.key, channel.range.low, channel.range.high
                ),
            );
        } else if !channel.range.contains(channel.default_value) {
            report.error(
                "default_outside_range",
                at("default_value"),
                format!(
                    "`{}` defaults to {} but its range is ({}, {})",
                    channel.key, channel.default_value, channel.range.low, channel.range.high
                ),
            );
        }
        if !channel.default_value.is_finite() {
            report.error(
                "non_finite",
                at("default_value"),
                format!("`{}` defaults to a non-finite value", channel.key),
            );
        }
        // A `Replace` channel has no identity, so its default is the only thing
        // an unwritten point can be — which is fine, and worth saying once so
        // nobody expects composition to be order-independent there.
        if channel.composition == ModifierComposition::Replace {
            report.note(
                "order_dependent_channel",
                at("composition"),
                format!(
                    "`{}` composes by Replace, so its value depends on layer order",
                    channel.key
                ),
            );
        }
    }
}

fn check_sources(
    document: &TerrainDocument,
    recipes: &KnownRecipes,
    report: &mut DiagnosticReport,
) {
    for (index, source) in document.sources.iter().enumerate() {
        let at = |field: &str| Location::at(format!("sources[{index}].{field}"));
        match &source.source {
            Source::Constant(constant) => {
                if !constant.value.is_finite() {
                    report.error(
                        "non_finite",
                        at("source.value"),
                        format!("`{}` is a non-finite constant", source.key),
                    );
                }
            }
            Source::Noise(noise) => {
                if !(noise.frequency_per_m.is_finite() && noise.frequency_per_m > 0.0) {
                    report.error(
                        "invalid_noise",
                        at("source.frequency_per_m"),
                        format!(
                            "`{}` has a frequency of {}; it must be finite and positive",
                            source.key, noise.frequency_per_m
                        ),
                    );
                }
                if noise.octaves == 0 {
                    report.error(
                        "invalid_noise",
                        at("source.octaves"),
                        format!("`{}` has no octaves, so it produces nothing", source.key),
                    );
                }
                if !(noise.lacunarity.is_finite() && noise.gain.is_finite()) {
                    report.error(
                        "non_finite",
                        at("source"),
                        format!("`{}` has a non-finite lacunarity or gain", source.key),
                    );
                }
            }
            Source::ScalarRaster(raster) => {
                check_placement(
                    &source.key,
                    raster.placement,
                    &at("source.placement"),
                    report,
                );
            }
            Source::CategoricalRaster(raster) => {
                check_placement(
                    &source.key,
                    raster.placement,
                    &at("source.placement"),
                    report,
                );
                if raster.classes.is_empty() {
                    report.error(
                        "no_classes",
                        at("source.classes"),
                        format!(
                            "`{}` is a categorical raster with no classes, so no layer \
                             could name one",
                            source.key
                        ),
                    );
                }
                let mut seen: BTreeMap<u32, &str> = BTreeMap::new();
                for class in &raster.classes {
                    if let Some(first) = seen.insert(class.value, &class.name) {
                        report.error(
                            "duplicate_class",
                            at("source.classes"),
                            format!(
                                "`{}` maps the stored value {} to both `{first}` and `{}`",
                                source.key, class.value, class.name
                            ),
                        );
                    }
                }
            }
            Source::WeightRaster(raster) => {
                check_placement(
                    &source.key,
                    raster.placement,
                    &at("source.placement"),
                    report,
                );
                if raster.channels.is_empty() {
                    report.error(
                        "no_channels",
                        at("source.channels"),
                        format!("`{}` is a weight raster carrying no materials", source.key),
                    );
                }
                for (channel, material) in raster.channels.iter().enumerate() {
                    if document.material(material).is_none() {
                        report.error(
                            "unknown_material",
                            at(&format!("source.channels[{channel}]")),
                            format!("no material named `{material}`"),
                        );
                        suggest_material(document, material.as_str(), report);
                    }
                }
            }
            Source::SplineDistance(spline) => {
                check_distance(&source.key, spline.max_distance_m, &at("source"), report);
            }
            Source::ShapeDistance(shape) => {
                check_distance(&source.key, shape.max_distance_m, &at("source"), report);
            }
            Source::Custom(custom) => {
                if !recipes.is_empty() && !recipes.knows_source(&custom.recipe) {
                    report.error(
                        "unknown_recipe",
                        at("source.recipe"),
                        format!("no source recipe named `{}` is registered", custom.recipe),
                    );
                }
                check_parameters(&custom.parameters, &at("source.parameters"), report);
            }
        }
    }
}

fn check_placement(
    key: &crate::ids::SourceKey,
    placement: RasterPlacement,
    at: &Location,
    report: &mut DiagnosticReport,
) {
    if !placement.is_valid() {
        report.error(
            "invalid_transform",
            at.clone(),
            format!(
                "`{key}` is placed over a {} by {} metre rectangle, which cannot be \
                 sampled",
                placement.size_m.du_m, placement.size_m.dv_m
            ),
        );
    }
}

fn check_distance(
    key: &crate::ids::SourceKey,
    max_distance_m: f64,
    at: &Location,
    report: &mut DiagnosticReport,
) {
    if !(max_distance_m.is_finite() && max_distance_m > 0.0) {
        report.error(
            "invalid_distance",
            at.clone(),
            format!(
                "`{key}` has a maximum distance of {max_distance_m}; it must be finite \
                 and positive, because it bounds the spatial index as well as the value"
            ),
        );
    }
}

fn check_parameters(parameters: &ParameterObject, at: &Location, report: &mut DiagnosticReport) {
    for (name, value) in parameters.iter() {
        let mut bad = Vec::new();
        value.non_finite_paths(name, &mut bad);
        for path in bad {
            report.error(
                "non_finite",
                Location::at(format!("{at}.{path}")),
                format!("the parameter `{path}` is not a finite number"),
            );
        }
    }
}

fn suggest_material(document: &TerrainDocument, wanted: &str, report: &mut DiagnosticReport) {
    if let Some(near) = nearest(wanted, document.materials.iter().map(|m| m.key.as_str()))
        && let Some(last) = report.last_mut()
    {
        last.help = Some(format!("did you mean `{near}`?"));
    }
}

fn check_layers(document: &TerrainDocument, report: &mut DiagnosticReport) {
    for (index, layer) in document.layers.iter().enumerate() {
        let at = |field: &str| Location::at(format!("layers[{index}].{field}"));

        if let Some(source) = layer.mask.source()
            && document.source(source).is_none()
        {
            report.error(
                "unknown_source",
                at("mask.source"),
                format!("no source named `{source}`"),
            );
            if let Some(near) = nearest(
                source.as_str(),
                document.sources.iter().map(|s| s.key.as_str()),
            ) && let Some(last) = report.last_mut()
            {
                last.help = Some(format!("did you mean `{near}`?"));
            }
        }

        if let Mask::Profile { shape, .. } = &layer.mask {
            check_profile(*shape, &at("mask.shape"), report);
        }

        match &layer.operation {
            LayerOperation::Material(operation) => {
                if document.material(&operation.material).is_none() {
                    report.error(
                        "unknown_material",
                        at("operation.material"),
                        format!("no material named `{}`", operation.material),
                    );
                    suggest_material(document, operation.material.as_str(), report);
                }
                if !operation.amount.is_finite() {
                    report.error(
                        "non_finite",
                        at("operation.amount"),
                        "a material amount must be a finite number",
                    );
                } else if operation.amount < 0.0 {
                    report.error(
                        "negative_amount",
                        at("operation.amount"),
                        format!(
                            "a material score of {} is negative; scores are normalised \
                             against one another and a negative one has no meaning",
                            operation.amount
                        ),
                    );
                }
            }
            LayerOperation::Elevation(operation) => {
                if !operation.height_m.is_finite() {
                    report.error(
                        "non_finite",
                        at("operation.height_m"),
                        "an elevation must be a finite number of metres",
                    );
                }
            }
            LayerOperation::Microrelief(operation) => {
                if !operation.displacement_m.is_finite() {
                    report.error(
                        "non_finite",
                        at("operation.displacement_m"),
                        "a microrelief displacement must be a finite number of metres",
                    );
                }
            }
            LayerOperation::Modifier(operation) => {
                match document.channel(&operation.channel) {
                    None => {
                        report.error(
                            "unknown_channel",
                            at("operation.channel"),
                            format!(
                                "no modifier channel named `{}` is declared",
                                operation.channel
                            ),
                        );
                        if let Some(near) = nearest(
                            operation.channel.as_str(),
                            document.modifier_channels.iter().map(|c| c.key.as_str()),
                        ) && let Some(last) = report.last_mut()
                        {
                            last.help = Some(format!("did you mean `{near}`?"));
                        }
                    }
                    Some(channel) => {
                        if operation.mode != channel.composition {
                            report.error(
                                "composition_mismatch",
                                at("operation.mode"),
                                format!(
                                    "this layer composes `{}` by {}, but the channel is \
                                     declared as {}",
                                    operation.channel,
                                    operation.mode.name(),
                                    channel.composition.name()
                                ),
                            );
                        }
                        if channel.range.is_valid()
                            && operation.value.is_finite()
                            && channel.composition == ModifierComposition::Replace
                            && !channel.range.contains(operation.value)
                        {
                            report.warning(
                                "value_outside_range",
                                at("operation.value"),
                                format!(
                                    "{} is outside `{}`'s range ({}, {}) and will be \
                                     clamped",
                                    operation.value,
                                    operation.channel,
                                    channel.range.low,
                                    channel.range.high
                                ),
                            );
                        }
                    }
                }
                if !operation.value.is_finite() {
                    report.error(
                        "non_finite",
                        at("operation.value"),
                        "a modifier value must be a finite number",
                    );
                }
            }
        }
    }

    // Nothing to normalise. A document with no material layer produces a sample
    // whose weights sum to zero, and every consumer downstream then has to
    // invent a fallback — which they would each invent differently.
    let claims_material = document
        .layers
        .iter()
        .any(|l| l.enabled && matches!(l.operation, LayerOperation::Material(_)));
    if !claims_material && !document.layers.is_empty() {
        report.error(
            "no_material_layer",
            Location::at("layers"),
            "no enabled layer claims any material, so every sample's weights would \
             sum to zero and there would be nothing to normalise",
        );
    }
}

fn check_profile(shape: Profile, at: &Location, report: &mut DiagnosticReport) {
    match shape {
        Profile::SmoothBand { inner_m, outer_m } => {
            if !(inner_m.is_finite() && outer_m.is_finite()) {
                report.error(
                    "non_finite",
                    at.clone(),
                    "a smooth band's radii must both be finite",
                );
            } else if inner_m < 0.0 || outer_m < 0.0 {
                report.error(
                    "negative_width",
                    at.clone(),
                    format!(
                        "a smooth band from {inner_m} to {outer_m} metres has a negative radius"
                    ),
                );
            } else if outer_m <= inner_m {
                report.error(
                    "negative_width",
                    at.clone(),
                    format!(
                        "a smooth band's outer radius ({outer_m} m) must be greater than \
                         its inner ({inner_m} m); as written it has no transition to \
                         make and would be a hard edge"
                    ),
                );
            }
        }
        Profile::Threshold { at: value } => {
            if !value.is_finite() {
                report.error(
                    "non_finite",
                    at.clone(),
                    "a threshold must be a finite number",
                );
            }
        }
        Profile::Ramp { low, high } => {
            if !(low.is_finite() && high.is_finite()) {
                report.error(
                    "non_finite",
                    at.clone(),
                    "a ramp's ends must both be finite",
                );
            } else if low == high {
                report.error(
                    "negative_width",
                    at.clone(),
                    format!("a ramp from {low} to {high} has no width and cannot be evaluated"),
                );
            }
        }
        Profile::Passthrough => {}
    }
}

fn check_populations(
    document: &TerrainDocument,
    recipes: &KnownRecipes,
    report: &mut DiagnosticReport,
) {
    for (index, population) in document.populations.iter().enumerate() {
        let at = |field: &str| Location::at(format!("populations[{index}].{field}"));

        if !recipes.is_empty() && !recipes.knows_population(&population.recipe) {
            report.error(
                "unknown_recipe",
                at("recipe"),
                format!(
                    "no population recipe named `{}` is registered",
                    population.recipe
                ),
            );
        }

        for (slot, affinity) in population.material_affinity.iter().enumerate() {
            if document.material(&affinity.material).is_none() {
                report.error(
                    "unknown_material",
                    at(&format!("material_affinity[{slot}].material")),
                    format!("no material named `{}`", affinity.material),
                );
                suggest_material(document, affinity.material.as_str(), report);
            }
            if !affinity.weight.is_finite() {
                report.error(
                    "non_finite",
                    at(&format!("material_affinity[{slot}].weight")),
                    "a material affinity must be a finite number",
                );
            }
        }

        if let Some(channel) = &population.abundance_channel
            && document.channel(channel).is_none()
        {
            report.error(
                "unknown_channel",
                at("abundance_channel"),
                format!("no modifier channel named `{channel}` is declared"),
            );
            if let Some(near) = nearest(
                channel.as_str(),
                document.modifier_channels.iter().map(|c| c.key.as_str()),
            ) && let Some(last) = report.last_mut()
            {
                last.help = Some(format!("did you mean `{near}`?"));
            }
        }

        check_parameters(&population.parameters, &at("parameters"), report);
    }

    // Two populations sharing a seed stream draw the same numbers for the same
    // cell, so their candidates land on top of one another. It is legal — an
    // author may want a flower and its leaf to agree — and it is much more often
    // a copy-paste.
    let mut streams: BTreeMap<&str, &str> = BTreeMap::new();
    for (index, population) in document.populations.iter().enumerate() {
        let stream = population.seed_stream.as_str();
        if let Some(first) = streams.insert(stream, population.key.as_str()) {
            report.warning(
                "shared_seed_stream",
                Location::at(format!("populations[{index}].seed_stream")),
                format!(
                    "`{}` and `{first}` both draw from the stream `{stream}`, so their \
                     candidates will land in the same places",
                    population.key
                ),
            );
        }
    }
}

/// Checks that need the whole document rather than one item.
fn check_composition(document: &TerrainDocument, report: &mut DiagnosticReport) {
    // A declared channel nothing reads or writes is dead weight in the document
    // and usually a leftover from something that was removed.
    for (index, channel) in document.modifier_channels.iter().enumerate() {
        let written = document.layers.iter().any(|layer| {
            matches!(&layer.operation, LayerOperation::Modifier(m) if m.channel == channel.key)
        });
        let read = document
            .populations
            .iter()
            .any(|p| p.abundance_channel.as_ref() == Some(&channel.key));
        if !written && !read {
            report.warning(
                "unused_channel",
                Location::at(format!("modifier_channels[{index}].key")),
                format!("nothing writes or reads `{}`", channel.key),
            );
        }
    }

    // A source nothing reads, likewise.
    for (index, source) in document.sources.iter().enumerate() {
        let read = document
            .layers
            .iter()
            .any(|layer| layer.mask.source() == Some(&source.key));
        if !read {
            report.warning(
                "unused_source",
                Location::at(format!("sources[{index}].key")),
                format!("no layer reads `{}`", source.key),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Severity;
    use crate::ids::*;
    use crate::seed::RootSeed;

    fn material(key: &str) -> MaterialDef {
        MaterialDef {
            key: MaterialKey::new(key).expect("valid"),
            display_name: key.into(),
            appearance: AppearanceKey::new(format!("surface.{key}")).expect("valid"),
        }
    }

    /// The smallest document that says something and validates cleanly.
    fn constant_grass() -> TerrainDocument {
        TerrainDocument {
            coordinate_system: CoordinateSystem::PlanarMetres,
            root_seed: RootSeed::new(0x8df7_82f9_5ce1_a4d4),
            materials: vec![material("grass_lush")],
            modifier_channels: Vec::new(),
            sources: vec![SourceDef {
                key: SourceKey::new("everywhere").expect("valid"),
                source: Source::Constant(ConstantSource { value: 1.0 }),
            }],
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
            populations: Vec::new(),
            metadata: DocumentMetadata {
                name: "constant grass".into(),
                description: "The smallest document that says anything.".into(),
            },
        }
    }

    fn codes(report: &DiagnosticReport) -> Vec<&str> {
        report.entries().iter().map(|e| e.code).collect()
    }

    #[test]
    fn the_constant_grass_document_validates_cleanly() {
        // The milestone this whole step exists to reach.
        let report = validate(&constant_grass());
        assert!(
            !report.has_errors(),
            "constant grass did not validate:\n{report}"
        );
        assert!(report.is_empty(), "unexpected diagnostics:\n{report}");
    }

    #[test]
    fn one_pass_reports_several_unrelated_problems() {
        // The property the collecting design exists for. Three separate
        // mistakes, all found in one run.
        let mut document = constant_grass();
        document.materials.push(material("grass_lush"));
        document.layers.push(LayerDef {
            key: LayerKey::new("bad_source").expect("valid"),
            enabled: true,
            mask: Mask::Source(SourceKey::new("nowhere").expect("valid")),
            operation: LayerOperation::Elevation(ElevationLayer {
                mode: HeightMode::Add,
                height_m: f32::NAN,
            }),
        });
        let report = validate(&document);
        let found = codes(&report);
        assert!(found.contains(&"duplicate_key"), "{found:?}");
        assert!(found.contains(&"unknown_source"), "{found:?}");
        assert!(found.contains(&"non_finite"), "{found:?}");
    }

    #[test]
    fn a_misspelled_reference_suggests_the_name_it_almost_matched() {
        let mut document = constant_grass();
        document.layers[0].mask = Mask::Source(SourceKey::new("everywere").expect("valid"));
        let report = validate(&document);
        let entry = report
            .entries()
            .iter()
            .find(|e| e.code == "unknown_source")
            .expect("reported");
        assert_eq!(
            entry.help.as_deref(),
            Some("did you mean `everywhere`?"),
            "{entry}"
        );
    }

    #[test]
    fn a_wildly_wrong_reference_suggests_nothing() {
        // A wrong suggestion is worse than none.
        let mut document = constant_grass();
        document.layers[0].mask = Mask::Source(SourceKey::new("qqqqqqqqqq").expect("valid"));
        let report = validate(&document);
        let entry = report
            .entries()
            .iter()
            .find(|e| e.code == "unknown_source")
            .expect("reported");
        assert_eq!(entry.help, None);
    }

    #[test]
    fn a_document_with_no_material_layer_is_refused() {
        // Every sample's weights would sum to zero, and each consumer would
        // invent a different fallback.
        let mut document = constant_grass();
        document.layers[0].operation = LayerOperation::Elevation(ElevationLayer {
            mode: HeightMode::Add,
            height_m: 1.0,
        });
        assert!(codes(&validate(&document)).contains(&"no_material_layer"));
    }

    #[test]
    fn a_disabled_material_layer_does_not_count_as_one() {
        let mut document = constant_grass();
        document.layers[0].enabled = false;
        assert!(codes(&validate(&document)).contains(&"no_material_layer"));
    }

    #[test]
    fn a_backwards_smooth_band_is_an_error() {
        // The path shape everything else is built on. Written backwards it has
        // no transition to make, which is a hard edge rather than the soft one
        // the author asked for.
        let mut document = constant_grass();
        document.layers[0].mask = Mask::Profile {
            source: SourceKey::new("everywhere").expect("valid"),
            shape: Profile::SmoothBand {
                inner_m: 2.6,
                outer_m: 1.5,
            },
        };
        let report = validate(&document);
        assert!(codes(&report).contains(&"negative_width"), "{report}");

        // And a negative radius, separately.
        document.layers[0].mask = Mask::Profile {
            source: SourceKey::new("everywhere").expect("valid"),
            shape: Profile::SmoothBand {
                inner_m: -1.0,
                outer_m: 2.0,
            },
        };
        assert!(codes(&validate(&document)).contains(&"negative_width"));
    }

    #[test]
    fn a_layer_composing_a_channel_the_wrong_way_is_reported() {
        // The check that makes the channel's declared rule mean something. A
        // layer that Adds to a Multiply channel would silently be doing
        // something the document does not say.
        let mut document = constant_grass();
        document.modifier_channels.push(ModifierChannelDef {
            key: ModifierKey::new("vegetation_density").expect("valid"),
            display_name: "Vegetation density".into(),
            range: ValueRange::new(0.0, 1.5),
            default_value: 1.0,
            composition: ModifierComposition::Multiply,
            unit: ModifierUnit::Unitless,
        });
        document.layers.push(LayerDef {
            key: LayerKey::new("suppression").expect("valid"),
            enabled: true,
            mask: Mask::Everywhere,
            operation: LayerOperation::Modifier(ModifierLayer {
                channel: ModifierKey::new("vegetation_density").expect("valid"),
                mode: ModifierComposition::Add,
                value: 0.15,
            }),
        });
        assert!(codes(&validate(&document)).contains(&"composition_mismatch"));
    }

    #[test]
    fn a_channel_defaulting_outside_its_own_range_is_an_error() {
        let mut document = constant_grass();
        document.modifier_channels.push(ModifierChannelDef {
            key: ModifierKey::new("rock_abundance").expect("valid"),
            display_name: "Rock abundance".into(),
            range: ValueRange::new(0.0, 1.0),
            default_value: 2.0,
            composition: ModifierComposition::Max,
            unit: ModifierUnit::Unitless,
        });
        assert!(codes(&validate(&document)).contains(&"default_outside_range"));
    }

    #[test]
    fn an_undeclared_channel_is_an_error_wherever_it_is_named() {
        let mut document = constant_grass();
        document.layers.push(LayerDef {
            key: LayerKey::new("suppression").expect("valid"),
            enabled: true,
            mask: Mask::Everywhere,
            operation: LayerOperation::Modifier(ModifierLayer {
                channel: ModifierKey::new("vegetation_density").expect("valid"),
                mode: ModifierComposition::Multiply,
                value: 0.15,
            }),
        });
        document.populations.push(PopulationDef {
            key: PopulationKey::new("meadow_flowers").expect("valid"),
            recipe: RecipeKey::new("population.wildflowers_meadow").expect("valid"),
            enabled: true,
            seed_stream: StreamKey::new("flowers").expect("valid"),
            material_affinity: Vec::new(),
            abundance_channel: Some(ModifierKey::new("flower_abundance").expect("valid")),
            parameters: ParameterObject::new(),
        });
        let report = validate(&document);
        assert_eq!(
            report
                .entries()
                .iter()
                .filter(|e| e.code == "unknown_channel")
                .count(),
            2,
            "{report}"
        );
    }

    #[test]
    fn recipes_are_only_checked_when_a_registry_is_supplied() {
        // A document should be checkable in CI with no binary's registry
        // available, so an empty registry checks nothing rather than rejecting
        // everything.
        let mut document = constant_grass();
        document.populations.push(PopulationDef {
            key: PopulationKey::new("grass_population").expect("valid"),
            recipe: RecipeKey::new("population.grass_lush").expect("valid"),
            enabled: true,
            seed_stream: StreamKey::new("grass").expect("valid"),
            material_affinity: vec![MaterialAffinity {
                material: MaterialKey::new("grass_lush").expect("valid"),
                weight: 1.0,
            }],
            abundance_channel: None,
            parameters: ParameterObject::new(),
        });
        assert!(!validate(&document).has_errors());

        let known = KnownRecipes::new()
            .with_population(RecipeKey::new("population.grass_lush").expect("valid"));
        assert!(!validate_against(&document, &known).has_errors());

        let other = KnownRecipes::new()
            .with_population(RecipeKey::new("population.something_else").expect("valid"));
        assert!(codes(&validate_against(&document, &other)).contains(&"unknown_recipe"));
    }

    #[test]
    fn an_unread_source_is_a_warning_and_not_an_error() {
        let mut document = constant_grass();
        document.sources.push(SourceDef {
            key: SourceKey::new("orphan").expect("valid"),
            source: Source::Constant(ConstantSource { value: 0.5 }),
        });
        let report = validate(&document);
        assert!(!report.has_errors(), "{report}");
        assert_eq!(report.count(Severity::Warning), 1);
        assert!(codes(&report).contains(&"unused_source"));
    }

    #[test]
    fn two_populations_sharing_a_stream_are_warned_about() {
        let mut document = constant_grass();
        for key in ["grass_population", "meadow_flowers"] {
            document.populations.push(PopulationDef {
                key: PopulationKey::new(key).expect("valid"),
                recipe: RecipeKey::new("population.grass_lush").expect("valid"),
                enabled: true,
                seed_stream: StreamKey::new("scatter").expect("valid"),
                material_affinity: Vec::new(),
                abundance_channel: None,
                parameters: ParameterObject::new(),
            });
        }
        let report = validate(&document);
        assert!(!report.has_errors(), "{report}");
        assert!(codes(&report).contains(&"shared_seed_stream"));
    }

    #[test]
    fn a_raster_over_a_zero_sized_rectangle_is_an_error() {
        let mut document = constant_grass();
        document.sources.push(SourceDef {
            key: SourceKey::new("rock_zone").expect("valid"),
            source: Source::ScalarRaster(ScalarRasterSource {
                asset: AssetPath::new("masks/rock_abundance.png").expect("valid"),
                placement: RasterPlacement {
                    origin_m: crate::coords::WorldPoint::new(-32.0, -32.0),
                    size_m: crate::coords::WorldVector::new(0.0, 64.0),
                    anchor: crate::coords::TexelAnchor::Centre,
                    row_order: crate::coords::RowOrder::TopDown,
                },
                filter: RasterFilter::Bilinear,
                wrap: RasterWrap::Clamp,
            }),
        });
        assert!(codes(&validate(&document)).contains(&"invalid_transform"));
    }

    #[test]
    fn a_spline_with_no_reach_is_an_error() {
        // It bounds the spatial index as well as the value, so zero is not
        // "no falloff", it is "no index".
        let mut document = constant_grass();
        document.sources.push(SourceDef {
            key: SourceKey::new("main_path").expect("valid"),
            source: Source::SplineDistance(SplineDistanceSource {
                asset: AssetPath::new("features/main_path.spline.ron").expect("valid"),
                max_distance_m: 0.0,
            }),
        });
        assert!(codes(&validate(&document)).contains(&"invalid_distance"));
    }

    #[test]
    fn a_document_with_no_materials_is_refused() {
        let mut document = constant_grass();
        document.materials.clear();
        assert!(codes(&validate(&document)).contains(&"no_materials"));
    }

    #[test]
    fn edit_distance_measures_what_it_claims_to() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("grass", "grass"), 0);
        assert_eq!(edit_distance("everywere", "everywhere"), 1);
        assert_eq!(edit_distance("main_pth", "main_path"), 1);
        assert_eq!(edit_distance("abc", ""), 3);
    }
}

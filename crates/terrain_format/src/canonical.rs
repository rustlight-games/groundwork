//! Turning a parsed file into a document.
//!
//! Every string becomes a validated key, every enum name becomes a variant, and
//! every problem found on the way is *collected* rather than returned. That last
//! part is the whole reason this is a function rather than a `From` impl: an
//! author who has misspelled four keys should be told about four keys.
//!
//! ## Enum names are matched exactly
//!
//! `Multiply` and not `multiply`, `AddScore` and not `add_score`. Being lenient
//! here is tempting and wrong: once two spellings are accepted, both appear in
//! authored files, and the one that gets deprecated later breaks documents that
//! were legal when they were written. One spelling, and a message that shows the
//! whole list when it does not match.

use terrain_core::coords::{RowOrder, TexelAnchor, WorldPoint, WorldVector};
use terrain_core::diagnostics::{DiagnosticReport, Location};
use terrain_core::document::*;
use terrain_core::ids::*;
use terrain_core::seed::RootSeed;

use crate::raw::*;

/// Read a parsed file into a document, collecting every problem.
///
/// Returns a document even when problems were found, so long as it could be
/// built at all — a caller that wants to show an author what is wrong needs the
/// partial result to show it against.
pub fn canonicalise(raw: &RawDocument) -> (Option<TerrainDocument>, DiagnosticReport) {
    let mut report = DiagnosticReport::new();
    let mut context = Context {
        report: &mut report,
    };

    let coordinate_system = match raw.coordinate_system.as_str() {
        "PlanarMetres" => CoordinateSystem::PlanarMetres,
        other => {
            context.bad_variant("coordinate_system", other, &["PlanarMetres"]);
            CoordinateSystem::PlanarMetres
        }
    };

    let root_seed = match raw.root_seed.parse::<RootSeed>() {
        Ok(seed) => seed,
        Err(error) => {
            context.report.error(
                "invalid_seed",
                Location::at("root_seed"),
                format!("`{}` is not sixteen hex digits: {error}", raw.root_seed),
            );
            RootSeed::new(0)
        }
    };

    let materials = raw
        .materials
        .iter()
        .enumerate()
        .filter_map(|(index, material)| context.material(index, material))
        .collect();
    let modifier_channels = raw
        .modifier_channels
        .iter()
        .enumerate()
        .filter_map(|(index, channel)| context.channel(index, channel))
        .collect();
    let sources = raw
        .sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| context.source(index, source))
        .collect();
    let layers = raw
        .layers
        .iter()
        .enumerate()
        .filter_map(|(index, layer)| context.layer(index, layer))
        .collect();
    let populations = raw
        .populations
        .iter()
        .enumerate()
        .filter_map(|(index, population)| context.population(index, population))
        .collect();

    let document = TerrainDocument {
        coordinate_system,
        root_seed,
        materials,
        modifier_channels,
        sources,
        layers,
        populations,
        metadata: DocumentMetadata {
            name: raw.metadata.name.clone(),
            description: raw.metadata.description.clone(),
        },
    };

    if report.has_errors() {
        // The partial document is still returned, so a caller can show what it
        // managed to read beside what it could not.
        (Some(document), report)
    } else {
        (Some(document), report)
    }
}

struct Context<'a> {
    report: &'a mut DiagnosticReport,
}

impl Context<'_> {
    fn bad_variant(&mut self, path: &str, found: &str, expected: &[&str]) {
        self.report.error(
            "unknown_variant",
            Location::at(path),
            format!("`{found}` is not one of {}", expected.join(", ")),
        );
    }

    /// Validate a key, reporting where it went wrong.
    fn key<T>(
        &mut self,
        path: String,
        text: &str,
        make: impl Fn(&str) -> Result<T, KeyError>,
    ) -> Option<T> {
        match make(text) {
            Ok(key) => Some(key),
            Err(error) => {
                self.report.error(
                    "invalid_key",
                    Location::at(path),
                    format!("`{text}` is not a usable key: {error}"),
                );
                None
            }
        }
    }

    fn asset(&mut self, path: String, text: &str) -> Option<AssetPath> {
        match AssetPath::new(text) {
            Ok(asset) => Some(asset),
            Err(error) => {
                self.report.error(
                    "invalid_asset_path",
                    Location::at(path),
                    format!("`{text}` is not a usable asset path: {error}"),
                );
                None
            }
        }
    }

    fn material(&mut self, index: usize, raw: &RawMaterial) -> Option<MaterialDef> {
        let key = self.key(format!("materials[{index}].key"), &raw.key, |t| {
            MaterialKey::new(t)
        })?;
        let appearance = self.key(
            format!("materials[{index}].appearance"),
            &raw.appearance,
            |t| AppearanceKey::new(t),
        )?;
        let profile = match &raw.profile {
            None => None,
            Some(path) => match AssetPath::new(path.clone()) {
                Ok(path) => Some(path),
                Err(problem) => {
                    self.report.error(
                        "bad_asset_path",
                        Location::at(format!("materials[{index}].profile")),
                        format!("`{path}` is not a usable asset path: {problem}"),
                    );
                    None
                }
            },
        };
        if let Some(affinity) = raw.vegetation_affinity
            && !(0.0..=1.0).contains(&affinity)
        {
            self.report.error(
                "out_of_range",
                Location::at(format!("materials[{index}].vegetation_affinity")),
                format!("{affinity} is outside 0..1"),
            );
        }
        Some(MaterialDef {
            display_name: if raw.display_name.is_empty() {
                key.as_str().to_string()
            } else {
                raw.display_name.clone()
            },
            key,
            appearance,
            profile,
            vegetation_affinity: raw.vegetation_affinity.filter(|a| (0.0..=1.0).contains(a)),
        })
    }

    fn channel(&mut self, index: usize, raw: &RawModifierChannel) -> Option<ModifierChannelDef> {
        let key = self.key(format!("modifier_channels[{index}].key"), &raw.key, |t| {
            ModifierKey::new(t)
        })?;
        let composition = self.composition(
            &format!("modifier_channels[{index}].composition"),
            &raw.composition,
        )?;
        let unit = match raw.unit.as_str() {
            "Unitless" => ModifierUnit::Unitless,
            "Metres" => ModifierUnit::Metres,
            "Radians" => ModifierUnit::Radians,
            "PerSquareMetre" => ModifierUnit::PerSquareMetre,
            other => {
                self.bad_variant(
                    &format!("modifier_channels[{index}].unit"),
                    other,
                    &["Unitless", "Metres", "Radians", "PerSquareMetre"],
                );
                return None;
            }
        };
        let role = match &raw.role {
            None => None,
            Some(text) => match ModifierRole::parse(text) {
                Some(role) => Some(role),
                None => {
                    let known: Vec<&str> = ModifierRole::ALL.iter().map(|r| r.name()).collect();
                    self.bad_variant(&format!("modifier_channels[{index}].role"), text, &known);
                    return None;
                }
            },
        };
        Some(ModifierChannelDef {
            display_name: if raw.display_name.is_empty() {
                key.as_str().to_string()
            } else {
                raw.display_name.clone()
            },
            key,
            range: ValueRange::new(raw.range.0, raw.range.1),
            default_value: raw.default_value,
            composition,
            unit,
            role,
        })
    }

    fn composition(&mut self, path: &str, text: &str) -> Option<ModifierComposition> {
        match text {
            "Multiply" => Some(ModifierComposition::Multiply),
            "Add" => Some(ModifierComposition::Add),
            "Max" => Some(ModifierComposition::Max),
            "Min" => Some(ModifierComposition::Min),
            "Replace" => Some(ModifierComposition::Replace),
            other => {
                self.bad_variant(path, other, &["Multiply", "Add", "Max", "Min", "Replace"]);
                None
            }
        }
    }

    fn placement(&mut self, path: &str, raw: &RawPlacement) -> Option<RasterPlacement> {
        let anchor = match raw.anchor.as_str() {
            "Centre" => TexelAnchor::Centre,
            "Edge" => TexelAnchor::Edge,
            other => {
                self.bad_variant(&format!("{path}.anchor"), other, &["Centre", "Edge"]);
                return None;
            }
        };
        let row_order = match raw.row_order.as_str() {
            "TopDown" => RowOrder::TopDown,
            "BottomUp" => RowOrder::BottomUp,
            other => {
                self.bad_variant(
                    &format!("{path}.row_order"),
                    other,
                    &["TopDown", "BottomUp"],
                );
                return None;
            }
        };
        Some(RasterPlacement {
            origin_m: WorldPoint::new(raw.origin_m.0, raw.origin_m.1),
            size_m: WorldVector::new(raw.size_m.0, raw.size_m.1),
            anchor,
            row_order,
        })
    }

    fn filter(&mut self, path: &str, text: &str) -> Option<RasterFilter> {
        match text {
            "Bilinear" => Some(RasterFilter::Bilinear),
            "Nearest" => Some(RasterFilter::Nearest),
            other => {
                self.bad_variant(path, other, &["Bilinear", "Nearest"]);
                None
            }
        }
    }

    fn source(&mut self, index: usize, raw: &RawSourceEntry) -> Option<SourceDef> {
        let key = self.key(format!("sources[{index}].key"), &raw.key, |t| {
            SourceKey::new(t)
        })?;
        let path = format!("sources[{index}].source");
        let source = match &raw.source {
            RawSource::Constant(constant) => Source::Constant(ConstantSource {
                value: constant.value,
            }),
            RawSource::Noise(noise) => {
                let kind = match noise.kind.as_str() {
                    "Perlin" => NoiseKind::Perlin,
                    "Worley" => NoiseKind::Worley,
                    other => {
                        self.bad_variant(&format!("{path}.kind"), other, &["Perlin", "Worley"]);
                        return None;
                    }
                };
                let stream = self.key(format!("{path}.stream"), &noise.stream, |t| {
                    StreamKey::new(t)
                })?;
                Source::Noise(NoiseSource {
                    kind,
                    frequency_per_m: noise.frequency_per_m,
                    octaves: noise.octaves,
                    lacunarity: noise.lacunarity,
                    gain: noise.gain,
                    stream,
                })
            }
            RawSource::ScalarRaster(raster) => Source::ScalarRaster(ScalarRasterSource {
                asset: self.asset(format!("{path}.asset"), &raster.asset)?,
                placement: self
                    .placement(&format!("{path}.world_transform"), &raster.world_transform)?,
                filter: self.filter(&format!("{path}.filter"), &raster.filter)?,
                wrap: match raster.wrap {
                    RawWrap::Clamp => RasterWrap::Clamp,
                    RawWrap::Repeat => RasterWrap::Repeat,
                    RawWrap::Value(value) => RasterWrap::Value(value),
                },
            }),
            RawSource::CategoricalRaster(raster) => {
                Source::CategoricalRaster(CategoricalRasterSource {
                    asset: self.asset(format!("{path}.asset"), &raster.asset)?,
                    placement: self
                        .placement(&format!("{path}.world_transform"), &raster.world_transform)?,
                    classes: raster
                        .classes
                        .iter()
                        .map(|class| RasterClass {
                            value: class.value,
                            name: class.name.clone(),
                        })
                        .collect(),
                })
            }
            RawSource::WeightRaster(raster) => {
                let mut channels = Vec::with_capacity(raster.channels.len());
                for (slot, name) in raster.channels.iter().enumerate() {
                    channels.push(self.key(format!("{path}.channels[{slot}]"), name, |t| {
                        MaterialKey::new(t)
                    })?);
                }
                Source::WeightRaster(WeightRasterSource {
                    asset: self.asset(format!("{path}.asset"), &raster.asset)?,
                    placement: self
                        .placement(&format!("{path}.world_transform"), &raster.world_transform)?,
                    filter: self.filter(&format!("{path}.filter"), &raster.filter)?,
                    channels,
                })
            }
            RawSource::SplineDistance(spline) => Source::SplineDistance(SplineDistanceSource {
                asset: self.asset(format!("{path}.asset"), &spline.asset)?,
                max_distance_m: spline.max_distance_m,
            }),
            RawSource::ShapeDistance(shape) => Source::ShapeDistance(ShapeDistanceSource {
                asset: self.asset(format!("{path}.asset"), &shape.asset)?,
                max_distance_m: shape.max_distance_m,
                signed: shape.signed,
            }),
            RawSource::Custom(custom) => Source::Custom(CustomSourceRef {
                recipe: self.key(format!("{path}.recipe"), &custom.recipe, |t| {
                    RecipeKey::new(t)
                })?,
                parameters: self.parameters(&format!("{path}.parameters"), &custom.parameters),
            }),
        };
        Some(SourceDef { key, source })
    }

    fn parameters(&mut self, path: &str, raw: &RawParameters) -> ParameterObject {
        let mut parameters = ParameterObject::new();
        let mut seen: Vec<&str> = Vec::new();
        for (name, value) in &raw.0 {
            if seen.contains(&name.as_str()) {
                self.report.error(
                    "duplicate_parameter",
                    Location::at(format!("{path}.{name}")),
                    format!("`{name}` is given more than once"),
                );
                continue;
            }
            seen.push(name);
            parameters.insert(name.clone(), self.parameter_value(value));
        }
        parameters
    }

    fn parameter_value(&mut self, raw: &RawParameterValue) -> ParameterValue {
        match raw {
            RawParameterValue::Bool(value) => ParameterValue::Bool(*value),
            RawParameterValue::Integer(value) => ParameterValue::Integer(*value),
            RawParameterValue::Number(value) => ParameterValue::Number(*value),
            RawParameterValue::Text(value) => ParameterValue::Text(value.clone()),
            RawParameterValue::List(items) => ParameterValue::List(
                items
                    .iter()
                    .map(|item| self.parameter_value(item))
                    .collect(),
            ),
        }
    }

    fn layer(&mut self, index: usize, raw: &RawLayer) -> Option<LayerDef> {
        let key = self.key(format!("layers[{index}].key"), &raw.key, |t| {
            LayerKey::new(t)
        })?;
        let mask_path = format!("layers[{index}].mask");
        let mask = match &raw.mask {
            RawMask::Everywhere => Mask::Everywhere,
            RawMask::Source(name) => {
                Mask::Source(self.key(format!("{mask_path}.source"), name, |t| SourceKey::new(t))?)
            }
            RawMask::Profile(profile) => Mask::Profile {
                source: self.key(format!("{mask_path}.source"), &profile.source, |t| {
                    SourceKey::new(t)
                })?,
                shape: match profile.shape {
                    RawProfile::SmoothBand(band) => Profile::SmoothBand {
                        inner_m: band.inner_m,
                        outer_m: band.outer_m,
                    },
                    RawProfile::Threshold(threshold) => Profile::Threshold { at: threshold.at },
                    RawProfile::Ramp(ramp) => Profile::Ramp {
                        low: ramp.low,
                        high: ramp.high,
                    },
                    RawProfile::Passthrough => Profile::Passthrough,
                },
            },
        };

        let operation_path = format!("layers[{index}].operation");
        let operation = match &raw.operation {
            RawOperation::Material(operation) => LayerOperation::Material(MaterialLayer {
                material: self.key(
                    format!("{operation_path}.material"),
                    &operation.material,
                    |t| MaterialKey::new(t),
                )?,
                mode: match operation.mode.as_str() {
                    "Replace" => MaterialMode::Replace,
                    "AddScore" => MaterialMode::AddScore,
                    "MultiplyScore" => MaterialMode::MultiplyScore,
                    other => {
                        self.bad_variant(
                            &format!("{operation_path}.mode"),
                            other,
                            &["Replace", "AddScore", "MultiplyScore"],
                        );
                        return None;
                    }
                },
                amount: operation.amount,
            }),
            RawOperation::Elevation(operation) => LayerOperation::Elevation(ElevationLayer {
                mode: self.height_mode(&format!("{operation_path}.mode"), &operation.mode)?,
                height_m: operation.metres,
            }),
            RawOperation::Microrelief(operation) => LayerOperation::Microrelief(MicroreliefLayer {
                mode: self.height_mode(&format!("{operation_path}.mode"), &operation.mode)?,
                displacement_m: operation.metres,
            }),
            RawOperation::Modifier(operation) => LayerOperation::Modifier(ModifierLayer {
                channel: self.key(
                    format!("{operation_path}.channel"),
                    &operation.channel,
                    |t| ModifierKey::new(t),
                )?,
                mode: self.composition(&format!("{operation_path}.mode"), &operation.mode)?,
                value: operation.value,
            }),
        };

        Some(LayerDef {
            key,
            enabled: raw.enabled,
            mask,
            operation,
        })
    }

    fn height_mode(&mut self, path: &str, text: &str) -> Option<HeightMode> {
        match text {
            "Add" => Some(HeightMode::Add),
            "Replace" => Some(HeightMode::Replace),
            "Max" => Some(HeightMode::Max),
            "Min" => Some(HeightMode::Min),
            other => {
                self.bad_variant(path, other, &["Add", "Replace", "Max", "Min"]);
                None
            }
        }
    }

    fn population(&mut self, index: usize, raw: &RawPopulation) -> Option<PopulationDef> {
        let key = self.key(format!("populations[{index}].key"), &raw.key, |t| {
            PopulationKey::new(t)
        })?;
        let recipe = self.key(format!("populations[{index}].recipe"), &raw.recipe, |t| {
            RecipeKey::new(t)
        })?;
        let seed_stream = self.key(
            format!("populations[{index}].seed_stream"),
            &raw.seed_stream,
            |t| StreamKey::new(t),
        )?;

        let mut material_affinity = Vec::with_capacity(raw.material_affinity.len());
        for (slot, (material, weight)) in raw.material_affinity.iter().enumerate() {
            let material = self.key(
                format!("populations[{index}].material_affinity[{slot}]"),
                material,
                |t| MaterialKey::new(t),
            )?;
            material_affinity.push(MaterialAffinity {
                material,
                weight: *weight,
            });
        }

        let abundance_channel = match &raw.abundance_channel {
            None => None,
            Some(name) => Some(self.key(
                format!("populations[{index}].abundance_channel"),
                name,
                |t| ModifierKey::new(t),
            )?),
        };

        Some(PopulationDef {
            key,
            recipe,
            enabled: raw.enabled,
            seed_stream,
            material_affinity,
            abundance_channel,
            parameters: self
                .parameters(&format!("populations[{index}].parameters"), &raw.parameters),
        })
    }
}

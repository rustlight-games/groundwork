//! The canonical digest of a document.
//!
//! Written by hand rather than derived, and split out of `document.rs` so the
//! omissions are visible in one place instead of scattered through three hundred
//! lines of type definitions.
//!
//! Two rules govern everything here:
//!
//! - **Every semantic field is absorbed.** A field that is not is a field that
//!   can change without invalidating a cache, which means a cached bake can be
//!   served for a document that no longer describes it.
//! - **Every variant is tagged.** Without tags, two differently-shaped values
//!   whose payload bits happen to coincide digest identically — and a
//!   `Threshold { at: 0.0 }` and a `Passthrough` are exactly that sort of pair.
//!
//! Order is part of the digest, deliberately. Layers compose in the order they
//! are written, so two documents with the same layers in different orders
//! describe different terrain and must not share a digest.

use crate::coords::{RowOrder, TexelAnchor, WorldPoint, WorldVector};
use crate::digest::{Digest, Digestible};
use crate::document::*;

fn point(digest: &mut Digest, point: WorldPoint) {
    digest.f64(point.u_m).f64(point.v_m);
}

fn vector(digest: &mut Digest, vector: WorldVector) {
    digest.f64(vector.du_m).f64(vector.dv_m);
}

fn anchor(digest: &mut Digest, anchor: TexelAnchor) {
    digest.tag(match anchor {
        TexelAnchor::Centre => 0,
        TexelAnchor::Edge => 1,
    });
}

fn row_order(digest: &mut Digest, order: RowOrder) {
    digest.tag(match order {
        RowOrder::TopDown => 0,
        RowOrder::BottomUp => 1,
    });
}

impl Digestible for ValueRange {
    fn absorb(&self, digest: &mut Digest) {
        digest.f32(self.low).f32(self.high);
    }
}

impl Digestible for RasterPlacement {
    fn absorb(&self, digest: &mut Digest) {
        point(digest, self.origin_m);
        vector(digest, self.size_m);
        anchor(digest, self.anchor);
        row_order(digest, self.row_order);
    }
}

impl Digestible for MaterialDef {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .str(self.key.as_str())
            .str(&self.display_name)
            .str(self.appearance.as_str())
            .str(self.profile.as_ref().map_or("", |p| p.as_str()));
        match self.vegetation_affinity {
            // Tagged rather than folded to a default, because "the author said
            // nothing" and "the author said what the profile already says" are
            // different documents and must not share a digest.
            None => digest.tag(0),
            Some(affinity) => digest.tag(1).f32(affinity),
        };
    }
}

impl Digestible for ModifierChannelDef {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(self.key.as_str()).str(&self.display_name);
        self.range.absorb(digest);
        digest
            .f32(self.default_value)
            .tag(composition_tag(self.composition))
            .tag(unit_tag(self.unit))
            .str(self.role.map_or("", |role| role.name()));
    }
}

fn composition_tag(composition: ModifierComposition) -> u8 {
    match composition {
        ModifierComposition::Multiply => 0,
        ModifierComposition::Add => 1,
        ModifierComposition::Max => 2,
        ModifierComposition::Min => 3,
        ModifierComposition::Replace => 4,
    }
}

fn unit_tag(unit: ModifierUnit) -> u8 {
    match unit {
        ModifierUnit::Unitless => 0,
        ModifierUnit::Metres => 1,
        ModifierUnit::Radians => 2,
        ModifierUnit::PerSquareMetre => 3,
    }
}

fn filter_tag(filter: RasterFilter) -> u8 {
    match filter {
        RasterFilter::Bilinear => 0,
        RasterFilter::Nearest => 1,
    }
}

fn wrap(digest: &mut Digest, wrap: RasterWrap) {
    match wrap {
        RasterWrap::Clamp => {
            digest.tag(0);
        }
        RasterWrap::Repeat => {
            digest.tag(1);
        }
        RasterWrap::Value(value) => {
            digest.tag(2).f32(value);
        }
    }
}

impl Digestible for Source {
    fn absorb(&self, digest: &mut Digest) {
        match self {
            Self::Constant(source) => {
                digest.tag(0).f32(source.value);
            }
            Self::Noise(source) => {
                digest
                    .tag(1)
                    .tag(match source.kind {
                        NoiseKind::Perlin => 0,
                        NoiseKind::Worley => 1,
                    })
                    .f64(source.frequency_per_m)
                    .u32(source.octaves)
                    .f64(source.lacunarity)
                    .f64(source.gain)
                    .str(source.stream.as_str());
            }
            Self::ScalarRaster(source) => {
                digest.tag(2).str(source.asset.as_str());
                source.placement.absorb(digest);
                digest.tag(filter_tag(source.filter));
                wrap(digest, source.wrap);
            }
            Self::CategoricalRaster(source) => {
                digest.tag(3).str(source.asset.as_str());
                source.placement.absorb(digest);
                digest.slice(&source.classes, |d, class| {
                    d.u32(class.value).str(&class.name);
                });
            }
            Self::WeightRaster(source) => {
                digest.tag(4).str(source.asset.as_str());
                source.placement.absorb(digest);
                digest
                    .tag(filter_tag(source.filter))
                    .slice(&source.channels, |d, material| {
                        d.str(material.as_str());
                    });
            }
            Self::SplineDistance(source) => {
                digest
                    .tag(5)
                    .str(source.asset.as_str())
                    .f64(source.max_distance_m);
            }
            Self::ShapeDistance(source) => {
                digest
                    .tag(6)
                    .str(source.asset.as_str())
                    .f64(source.max_distance_m)
                    .bool(source.signed);
            }
            Self::Custom(source) => {
                digest.tag(7).str(source.recipe.as_str());
                source.parameters.absorb(digest);
            }
        }
    }
}

impl Digestible for SourceDef {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(self.key.as_str());
        self.source.absorb(digest);
    }
}

impl Digestible for Profile {
    fn absorb(&self, digest: &mut Digest) {
        match *self {
            Self::SmoothBand { inner_m, outer_m } => {
                digest.tag(0).f64(inner_m).f64(outer_m);
            }
            Self::Threshold { at } => {
                digest.tag(1).f32(at);
            }
            Self::Ramp { low, high } => {
                digest.tag(2).f32(low).f32(high);
            }
            Self::Passthrough => {
                digest.tag(3);
            }
        }
    }
}

impl Digestible for Mask {
    fn absorb(&self, digest: &mut Digest) {
        match self {
            Self::Everywhere => {
                digest.tag(0);
            }
            Self::Source(key) => {
                digest.tag(1).str(key.as_str());
            }
            Self::Profile { source, shape } => {
                digest.tag(2).str(source.as_str());
                shape.absorb(digest);
            }
        }
    }
}

fn height_mode_tag(mode: HeightMode) -> u8 {
    match mode {
        HeightMode::Add => 0,
        HeightMode::Replace => 1,
        HeightMode::Max => 2,
        HeightMode::Min => 3,
    }
}

impl Digestible for LayerOperation {
    fn absorb(&self, digest: &mut Digest) {
        match self {
            Self::Material(layer) => {
                digest
                    .tag(0)
                    .str(layer.material.as_str())
                    .tag(match layer.mode {
                        MaterialMode::Replace => 0,
                        MaterialMode::AddScore => 1,
                        MaterialMode::MultiplyScore => 2,
                    })
                    .f32(layer.amount);
            }
            Self::Elevation(layer) => {
                digest
                    .tag(1)
                    .tag(height_mode_tag(layer.mode))
                    .f32(layer.height_m);
            }
            Self::Microrelief(layer) => {
                digest
                    .tag(2)
                    .tag(height_mode_tag(layer.mode))
                    .f32(layer.displacement_m);
            }
            Self::Modifier(layer) => {
                digest
                    .tag(3)
                    .str(layer.channel.as_str())
                    .tag(composition_tag(layer.mode))
                    .f32(layer.value);
            }
        }
    }
}

impl Digestible for LayerDef {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(self.key.as_str()).bool(self.enabled);
        self.mask.absorb(digest);
        self.operation.absorb(digest);
    }
}

impl Digestible for ParameterObject {
    fn absorb(&self, digest: &mut Digest) {
        // Length first, then key-ordered entries — the `BTreeMap` guarantees the
        // order, which is why it is one.
        digest.usize(self.len());
        for (name, value) in self.iter() {
            digest.str(name);
            value.absorb_into(digest);
        }
    }
}

impl Digestible for PopulationDef {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .str(self.key.as_str())
            .str(self.recipe.as_str())
            .bool(self.enabled)
            .str(self.seed_stream.as_str())
            .slice(&self.material_affinity, |d, affinity| {
                d.str(affinity.material.as_str()).f32(affinity.weight);
            });
        match &self.abundance_channel {
            Some(channel) => {
                digest.tag(1).str(channel.as_str());
            }
            None => {
                digest.tag(0);
            }
        }
        self.parameters.absorb(digest);
    }
}

impl Digestible for DocumentMetadata {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(&self.name).str(&self.description);
    }
}

impl Digestible for TerrainDocument {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .tag(match self.coordinate_system {
                CoordinateSystem::PlanarMetres => 0,
            })
            .u64(self.root_seed.bits());
        // Metadata is absorbed, and that is a real decision rather than an
        // oversight. A document's name is part of what it is: two documents
        // identical but for their description are two documents, and a cache
        // that served one for the other would be showing the wrong provenance in
        // every manifest downstream.
        self.metadata.absorb(digest);
        digest.slice(&self.materials, |d, m| m.absorb(d));
        digest.slice(&self.modifier_channels, |d, c| c.absorb(d));
        digest.slice(&self.sources, |d, s| s.absorb(d));
        digest.slice(&self.layers, |d, l| l.absorb(d));
        digest.slice(&self.populations, |d, p| p.absorb(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{LayerKey, MaterialKey, ModifierKey, SourceKey};

    fn material(key: &str) -> MaterialDef {
        MaterialDef {
            key: MaterialKey::new(key).expect("valid"),
            display_name: key.to_string(),
            appearance: crate::ids::AppearanceKey::new(format!("surface.{key}")).expect("valid"),
            profile: None,
            vegetation_affinity: None,
        }
    }

    fn document() -> TerrainDocument {
        TerrainDocument {
            materials: vec![material("grass_lush"), material("dirt_compacted")],
            sources: vec![SourceDef {
                key: SourceKey::new("everywhere").expect("valid"),
                source: Source::Constant(ConstantSource { value: 1.0 }),
            }],
            layers: vec![
                LayerDef {
                    key: LayerKey::new("base_grass").expect("valid"),
                    enabled: true,
                    mask: Mask::Everywhere,
                    operation: LayerOperation::Material(MaterialLayer {
                        material: MaterialKey::new("grass_lush").expect("valid"),
                        mode: MaterialMode::Replace,
                        amount: 1.0,
                    }),
                },
                LayerDef {
                    key: LayerKey::new("path_material").expect("valid"),
                    enabled: true,
                    mask: Mask::Source(SourceKey::new("everywhere").expect("valid")),
                    operation: LayerOperation::Material(MaterialLayer {
                        material: MaterialKey::new("dirt_compacted").expect("valid"),
                        mode: MaterialMode::AddScore,
                        amount: 1.0,
                    }),
                },
            ],
            ..TerrainDocument::default()
        }
    }

    #[test]
    fn an_identical_document_digests_identically() {
        assert_eq!(document().digest(), document().digest());
    }

    #[test]
    fn reordering_layers_changes_the_digest() {
        // Layers compose in order, so two documents with the same layers in a
        // different order describe different terrain.
        let mut swapped = document();
        swapped.layers.swap(0, 1);
        assert_ne!(document().digest(), swapped.digest());
    }

    #[test]
    fn every_top_level_section_reaches_the_digest() {
        let base = document().digest();

        let mut seeded = document();
        seeded.root_seed = crate::seed::RootSeed::new(7);
        assert_ne!(base, seeded.digest(), "root_seed");

        let mut named = document();
        named.metadata.name = "grass lab".into();
        assert_ne!(base, named.digest(), "metadata");

        let mut materials = document();
        materials.materials.push(material("water"));
        assert_ne!(base, materials.digest(), "materials");

        let mut channels = document();
        channels.modifier_channels.push(ModifierChannelDef {
            key: ModifierKey::new("vegetation_density").expect("valid"),
            display_name: "Vegetation density".into(),
            range: ValueRange::new(0.0, 1.5),
            default_value: 1.0,
            composition: ModifierComposition::Multiply,
            unit: ModifierUnit::Unitless,
            role: None,
        });
        assert_ne!(base, channels.digest(), "modifier_channels");

        let mut sources = document();
        sources.sources.push(SourceDef {
            key: SourceKey::new("noise").expect("valid"),
            source: Source::Constant(ConstantSource { value: 0.5 }),
        });
        assert_ne!(base, sources.digest(), "sources");

        let mut layers = document();
        layers.layers.pop();
        assert_ne!(base, layers.digest(), "layers");

        let mut populations = document();
        populations.populations.push(PopulationDef {
            key: crate::ids::PopulationKey::new("grass_population").expect("valid"),
            recipe: crate::ids::RecipeKey::new("population.grass_lush").expect("valid"),
            enabled: true,
            seed_stream: crate::ids::StreamKey::new("grass").expect("valid"),
            material_affinity: Vec::new(),
            abundance_channel: None,
            parameters: ParameterObject::new(),
        });
        assert_ne!(base, populations.digest(), "populations");
    }

    #[test]
    fn a_changed_number_deep_inside_a_layer_reaches_the_digest() {
        let mut nudged = document();
        if let LayerOperation::Material(layer) = &mut nudged.layers[0].operation {
            layer.amount = 0.999;
        }
        assert_ne!(document().digest(), nudged.digest());
    }

    #[test]
    fn variants_with_coinciding_payloads_are_told_apart() {
        // The pair tags exist for. Both hold nothing interesting, and without a
        // tag both would digest as an empty run.
        let threshold = Profile::Threshold { at: 0.0 };
        let passthrough = Profile::Passthrough;
        assert_ne!(
            threshold.fingerprint("profile"),
            passthrough.fingerprint("profile")
        );

        // And two masks that name the same source differently.
        let key = SourceKey::new("main_path").expect("valid");
        assert_ne!(
            Mask::Source(key.clone()).fingerprint("mask"),
            Mask::Profile {
                source: key,
                shape: Profile::Passthrough
            }
            .fingerprint("mask")
        );
    }

    #[test]
    fn an_absent_abundance_channel_differs_from_a_present_one() {
        let base = PopulationDef {
            key: crate::ids::PopulationKey::new("granite_rocks").expect("valid"),
            recipe: crate::ids::RecipeKey::new("population.granite_rocks").expect("valid"),
            enabled: true,
            seed_stream: crate::ids::StreamKey::new("rocks").expect("valid"),
            material_affinity: Vec::new(),
            abundance_channel: None,
            parameters: ParameterObject::new(),
        };
        let with_channel = PopulationDef {
            abundance_channel: Some(ModifierKey::new("rock_abundance").expect("valid")),
            ..base.clone()
        };
        assert_ne!(
            base.fingerprint("population"),
            with_channel.fingerprint("population")
        );
    }

    #[test]
    fn parameter_insertion_order_does_not_reach_the_digest() {
        // The reason parameters are a BTreeMap. Two identical documents authored
        // in different orders must share a digest, or every cache keyed on it
        // misses for no reason.
        let mut first = ParameterObject::new();
        first
            .insert("b", ParameterValue::Integer(2))
            .insert("a", ParameterValue::Integer(1));
        let mut second = ParameterObject::new();
        second
            .insert("a", ParameterValue::Integer(1))
            .insert("b", ParameterValue::Integer(2));
        assert_eq!(
            first.fingerprint("parameters"),
            second.fingerprint("parameters")
        );

        // But a changed value does reach it.
        let mut changed = ParameterObject::new();
        changed
            .insert("a", ParameterValue::Integer(1))
            .insert("b", ParameterValue::Integer(3));
        assert_ne!(
            first.fingerprint("parameters"),
            changed.fingerprint("parameters")
        );
    }

    #[test]
    fn a_parameter_of_a_different_type_digests_differently() {
        // `1` and `1.0` and `"1"` are three different things to a recipe.
        let integer = ParameterValue::Integer(1);
        let number = ParameterValue::Number(1.0);
        let text = ParameterValue::Text("1".into());
        let of = |value: &ParameterValue| {
            let mut digest = Digest::for_domain("parameter");
            value.absorb_into(&mut digest);
            digest.finish()
        };
        assert_ne!(of(&integer), of(&number));
        assert_ne!(of(&number), of(&text));
        assert_ne!(of(&integer), of(&text));
    }

    #[test]
    fn an_empty_document_has_a_stable_digest() {
        // The base case a constant-grass document is measured against, and the
        // one that catches a digest that forgot to absorb its own lengths.
        assert_eq!(
            TerrainDocument::default().digest(),
            TerrainDocument::default().digest()
        );
        assert_ne!(TerrainDocument::default().digest(), document().digest());
    }
}

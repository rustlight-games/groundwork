//! What an author wrote, as a value.
//!
//! This is the semantic model: the terrain document after it has been read,
//! migrated and canonicalised, and before it has been compiled into anything
//! samplable. Every key here is a validated key rather than a string, every
//! number has been checked for finiteness, and nothing in it knows what a file
//! or a format version is — that half lives in `terrain_format`.
//!
//! ## Three vocabularies, and the boundary between them
//!
//! An author has three kinds of thing to say, and keeping them apart is most of
//! why this model has the shape it does.
//!
//! - **Sources** are *fields*: a constant, some noise, a painted mask, the
//!   distance to a spline. They say nothing about terrain on their own; they are
//!   raw material.
//! - **Layers** say what the terrain **is**, continuously: this material here,
//!   this much lower there, this modifier suppressed along that. A layer takes a
//!   source, shapes it into a mask, and applies one operation.
//! - **Populations** say what **grows or sits on** it: discrete, countable
//!   things with their own identities.
//!
//! The line between the last two is the one that matters. Layers produce
//! continuous fields that every population reads; populations produce marks that
//! no layer reads. Collapsing them — letting a population contribute to material
//! weight, say — would make the composition order circular, and there would be
//! no answer to "what is the material here" that did not depend on which
//! population had been evaluated first.
//!
//! ## Modifier channels are declared, not conjured
//!
//! A population that reads `vegetation_density` and a layer that writes
//! `vegetaion_density` are, without declarations, two channels that will never
//! meet and never complain. So every channel is declared once with a range, a
//! default, a unit and a composition rule, and a layer or population naming an
//! undeclared one is an error with the misspelling in it.
//!
//! That is worth more than it sounds. The alternative — an untyped bag of
//! parameters — becomes the permanent terrain API within about a month, at which
//! point nothing can be renamed and nothing can be range-checked.

use std::collections::BTreeMap;

use crate::coords::{RowOrder, TexelAnchor, WorldPoint, WorldVector};
use crate::digest::{Digest, Digestible, Fingerprint};
use crate::ids::{
    AppearanceKey, LayerKey, MaterialKey, ModifierKey, PopulationKey, RecipeKey, SourceKey,
    StreamKey,
};
use crate::seed::RootSeed;

/// Which space the document's coordinates are in.
///
/// One variant, and `non_exhaustive` because a second is plausible — a geographic
/// projection, say — and adding it must not be a breaking change for anyone
/// matching on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CoordinateSystem {
    /// A plane, measured in metres. What everything here assumes.
    #[default]
    PlanarMetres,
}

/// A path to an asset beside the document.
///
/// Relative, always. An absolute path in an authored file is a document that
/// only works on the machine it was written on, and `..` is a document that can
/// read anything the process can.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetPath(String);

/// Why an asset path was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetPathError {
    Empty,
    Absolute,
    /// Contains a `..` component.
    Escaping,
    /// A backslash, which is a path separator on one platform and a filename
    /// character on another.
    Backslash,
}

impl std::fmt::Display for AssetPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "an asset path may not be empty"),
            Self::Absolute => write!(
                f,
                "an asset path must be relative to the document; an absolute one \
                 only works on the machine it was written on"
            ),
            Self::Escaping => write!(f, "an asset path may not contain `..`"),
            Self::Backslash => write!(
                f,
                "use `/` in an asset path; a backslash is a separator on one \
                 platform and a filename character on another"
            ),
        }
    }
}

impl std::error::Error for AssetPathError {}

impl AssetPath {
    pub fn new(text: impl Into<String>) -> Result<Self, AssetPathError> {
        let text = text.into();
        if text.is_empty() {
            return Err(AssetPathError::Empty);
        }
        if text.contains('\\') {
            return Err(AssetPathError::Backslash);
        }
        if text.starts_with('/') {
            return Err(AssetPathError::Absolute);
        }
        if text.split('/').any(|part| part == "..") {
            return Err(AssetPathError::Escaping);
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AssetPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A closed interval a value must lie in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueRange {
    pub low: f32,
    pub high: f32,
}

impl ValueRange {
    pub const fn new(low: f32, high: f32) -> Self {
        Self { low, high }
    }

    pub const UNIT: Self = Self::new(0.0, 1.0);

    pub fn contains(self, value: f32) -> bool {
        value >= self.low && value <= self.high
    }

    pub fn clamp(self, value: f32) -> f32 {
        value.clamp(self.low, self.high)
    }

    pub fn is_valid(self) -> bool {
        self.low.is_finite() && self.high.is_finite() && self.low < self.high
    }
}

/// How two contributions to the same modifier channel combine.
///
/// Declared on the channel rather than chosen per layer, because a channel whose
/// combination rule varies by writer has no well-defined value: the result would
/// depend on layer order in a way that is invisible in the document.
///
/// A layer may still *say* which of these it is using, and validation checks it
/// against the channel — so a mismatch is reported rather than silently
/// overridden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ModifierComposition {
    /// Scale what is there. The right default for a density or an abundance:
    /// two suppressions compound rather than fighting.
    #[default]
    Multiply,
    Add,
    /// Take the strongest claim. The right rule for an abundance that regions
    /// *grant* rather than modulate — a rock zone and a scree zone overlapping
    /// should not double the rocks.
    Max,
    Min,
    /// Last writer wins. Order-dependent by construction, so it is worth
    /// reaching for last.
    Replace,
}

impl ModifierComposition {
    pub fn combine(self, current: f32, incoming: f32) -> f32 {
        match self {
            Self::Multiply => current * incoming,
            Self::Add => current + incoming,
            Self::Max => current.max(incoming),
            Self::Min => current.min(incoming),
            Self::Replace => incoming,
        }
    }

    /// The value that leaves `combine` unchanged.
    ///
    /// What an unwritten channel starts from, so that "no layer touched this"
    /// and "a layer touched it with the identity" agree.
    pub fn identity(self) -> Option<f32> {
        match self {
            Self::Multiply => Some(1.0),
            Self::Add => Some(0.0),
            Self::Max => Some(f32::NEG_INFINITY),
            Self::Min => Some(f32::INFINITY),
            Self::Replace => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Multiply => "Multiply",
            Self::Add => "Add",
            Self::Max => "Max",
            Self::Min => "Min",
            Self::Replace => "Replace",
        }
    }
}

/// What a modifier channel's numbers mean.
///
/// Carried so a debug view can label an axis and a validator can catch a
/// document that suppresses vegetation with a value in metres.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ModifierUnit {
    /// A fraction, a multiplier, or an abundance.
    #[default]
    Unitless,
    Metres,
    Radians,
    PerSquareMetre,
}

impl ModifierUnit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unitless => "Unitless",
            Self::Metres => "Metres",
            Self::Radians => "Radians",
            Self::PerSquareMetre => "PerSquareMetre",
        }
    }
}

/// Who and what a document is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocumentMetadata {
    pub name: String,
    pub description: String,
}

/// A material the terrain may be made of.
///
/// Note what is *not* here: nothing about colour, roughness, or how it is drawn.
/// A material in this document is a **semantic identity** — "this ground is
/// compacted dirt" — and the two bindings below are the only bridges out of that,
/// deliberately separate keys so that two documents can share a look without
/// sharing a meaning, and a look can be swapped without touching a weight.
///
/// The two bindings answer different questions and it is worth being clear which
/// is which. [`appearance`](MaterialDef::appearance) names a **renderer-side
/// implementation** — which shader graph knows how to draw this at all.
/// [`profile`](MaterialDef::profile) names **what this ground is made of** — its
/// colours, its clod scales, how it responds to being wet. Nearly every ground
/// material shares one appearance and has its own profile.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    pub key: MaterialKey,
    pub display_name: String,
    /// The renderer-side binding: `surface.ground`.
    pub appearance: AppearanceKey,
    /// The ground material profile asset, if this material has one.
    ///
    /// `None` is legal and means "this material has no physical description" —
    /// which is right for a material that is never ground, and which leaves the
    /// renderer to fall back on its appearance key alone.
    pub profile: Option<AssetPath>,
    /// How much this ground supports plants, `0..1`.
    ///
    /// `None` defers to the profile's own default. Overriding it is how the same
    /// beach sand supports dune grass on one map and nothing on another.
    ///
    /// What this replaces was worse than either: deciding whether a material
    /// grew grass by looking for the substring `dirt` in its key. That works
    /// until a document contains `dirty_snow`, or `loam`, and then it works
    /// wrongly and silently.
    pub vegetation_affinity: Option<f32>,
}

/// What a modifier channel *means*, as opposed to what an author called it.
///
/// A consumer needs to find the moisture channel. Without this it finds it by
/// looking up the exact string `soil_moisture`, which holds until the first
/// document that calls it `wetness` or `saturation` — and then the ground is
/// silently bone dry and nothing anywhere says why.
///
/// So the *role* is declared and the *key* stays the author's own word for it. A
/// document may name its channel `path_wetness` and give it the
/// [`SoilMoisture`](ModifierRole::SoilMoisture) role, and every consumer finds
/// it.
///
/// Roles are exclusive: two channels claiming the same role is an error, because
/// there is no defensible rule for which one a consumer should pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ModifierRole {
    /// How much grows here. Scales every population's abundance.
    VegetationDensity,
    /// How wet the ground is, `0..1`.
    SoilMoisture,
    /// How packed it is, `0..1`. Flattens relief and suppresses loose material.
    SoilCompaction,
    /// How churned it is, `0..1`. Breaks up compaction and exposes fresh material.
    SoilDisturbance,
    /// How much loose material lies on top, `0..1`.
    LooseMaterial,
    /// How far the ground has dried out, `0..1`. Gates cracking.
    Desiccation,
    /// Organic content, `0..1`. Darkens and enriches.
    OrganicMatter,
    /// How exposed to wind, `0..1`. Drives ripple amplitude.
    WindExposure,
    /// How much water arrives here, `0..1`, before any is redistributed.
    WaterSupply,
}

impl ModifierRole {
    pub const ALL: [Self; 9] = [
        Self::VegetationDensity,
        Self::SoilMoisture,
        Self::SoilCompaction,
        Self::SoilDisturbance,
        Self::LooseMaterial,
        Self::Desiccation,
        Self::OrganicMatter,
        Self::WindExposure,
        Self::WaterSupply,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::VegetationDensity => "VegetationDensity",
            Self::SoilMoisture => "SoilMoisture",
            Self::SoilCompaction => "SoilCompaction",
            Self::SoilDisturbance => "SoilDisturbance",
            Self::LooseMaterial => "LooseMaterial",
            Self::Desiccation => "Desiccation",
            Self::OrganicMatter => "OrganicMatter",
            Self::WindExposure => "WindExposure",
            Self::WaterSupply => "WaterSupply",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.name() == text)
    }

    /// Whether a channel in this role must be a normalised unitless fraction.
    ///
    /// Every state role is, and saying so lets validation catch a document that
    /// declares its moisture in metres — which would otherwise reach a shader as
    /// a wetness of 0.035 and look like nothing happened.
    pub fn is_normalised_state(self) -> bool {
        !matches!(self, Self::VegetationDensity)
    }
}

/// A declared modifier channel.
#[derive(Clone, Debug, PartialEq)]
pub struct ModifierChannelDef {
    pub key: ModifierKey,
    pub display_name: String,
    pub range: ValueRange,
    pub default_value: f32,
    pub composition: ModifierComposition,
    pub unit: ModifierUnit,
    /// What this channel means to a consumer, if it means anything canonical.
    pub role: Option<ModifierRole>,
}

/// Where a raster sits in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterPlacement {
    pub origin_m: WorldPoint,
    pub size_m: WorldVector,
    pub anchor: TexelAnchor,
    pub row_order: RowOrder,
}

impl RasterPlacement {
    pub fn is_valid(self) -> bool {
        self.origin_m.is_finite()
            && self.size_m.is_finite()
            && self.size_m.du_m > 0.0
            && self.size_m.dv_m > 0.0
    }
}

/// What happens outside a raster's own rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum RasterWrap {
    /// Hold the edge value. The right default for a painted region: a mask that
    /// says "rocks here" should not say "rocks everywhere" past its border.
    #[default]
    Clamp,
    /// Repeat. For a tiling detail mask.
    Repeat,
    /// A stated value. For a mask that means nothing outside itself.
    Value(f32),
}

impl RasterWrap {}

/// How a raster is filtered between texels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RasterFilter {
    /// Smooth. For anything continuous.
    #[default]
    Bilinear,
    /// The containing texel's value, unblended. **Required** for a categorical
    /// raster: averaging two class indices produces a third class that means
    /// nothing.
    Nearest,
}

impl RasterFilter {}

/// A constant field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConstantSource {
    pub value: f32,
}

/// Which noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum NoiseKind {
    /// Smooth gradient noise, roughly `-1..1`.
    #[default]
    Perlin,
    /// Cellular distance, `0..1`, for anything that wants edges.
    Worley,
}

impl NoiseKind {}

/// Procedural noise, in world space.
///
/// Frequency is per metre rather than per texel, which is the whole point:
/// resampling the output at a different resolution must not change what the
/// noise looks like on the ground.
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseSource {
    pub kind: NoiseKind,
    pub frequency_per_m: f64,
    pub octaves: u32,
    pub lacunarity: f64,
    pub gain: f64,
    /// Which named stream this noise draws from, so two noise sources in one
    /// document are independent without the author having to pick a seed.
    pub stream: StreamKey,
}

/// A painted single-channel mask.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarRasterSource {
    pub asset: AssetPath,
    pub placement: RasterPlacement,
    pub filter: RasterFilter,
    pub wrap: RasterWrap,
}

/// One class of a categorical raster.
#[derive(Clone, Debug, PartialEq)]
pub struct RasterClass {
    /// The stored value this class occupies.
    pub value: u32,
    /// What that value means, as a source-local name the layers can profile on.
    pub name: String,
}

/// A painted map of discrete classes: a semantic tile grid, as a raster.
#[derive(Clone, Debug, PartialEq)]
pub struct CategoricalRasterSource {
    pub asset: AssetPath,
    pub placement: RasterPlacement,
    pub classes: Vec<RasterClass>,
}

/// A painted multi-channel weight map.
#[derive(Clone, Debug, PartialEq)]
pub struct WeightRasterSource {
    pub asset: AssetPath,
    pub placement: RasterPlacement,
    pub filter: RasterFilter,
    /// Which material each channel carries, in channel order.
    pub channels: Vec<MaterialKey>,
}

/// Distance to an authored spline: a path, a stream bank, a fence line.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineDistanceSource {
    pub asset: AssetPath,
    /// Past this, the source reports its maximum and stops being interesting.
    /// Also what bounds the spatial index, so it is a performance statement as
    /// well as a semantic one.
    pub max_distance_m: f64,
}

/// Distance to an authored closed shape: a clearing, a lake, a plot.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeDistanceSource {
    pub asset: AssetPath,
    pub max_distance_m: f64,
    /// Whether the inside reads as negative distance.
    pub signed: bool,
}

/// A source supplied by registered code rather than by this enum.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomSourceRef {
    pub recipe: RecipeKey,
    pub parameters: ParameterObject,
}

/// What a source produces.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Source {
    Constant(ConstantSource),
    Noise(NoiseSource),
    ScalarRaster(ScalarRasterSource),
    CategoricalRaster(CategoricalRasterSource),
    WeightRaster(WeightRasterSource),
    SplineDistance(SplineDistanceSource),
    ShapeDistance(ShapeDistanceSource),
    Custom(CustomSourceRef),
}

impl Source {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Constant(_) => "Constant",
            Self::Noise(_) => "Noise",
            Self::ScalarRaster(_) => "ScalarRaster",
            Self::CategoricalRaster(_) => "CategoricalRaster",
            Self::WeightRaster(_) => "WeightRaster",
            Self::SplineDistance(_) => "SplineDistance",
            Self::ShapeDistance(_) => "ShapeDistance",
            Self::Custom(_) => "Custom",
        }
    }
}

/// A named source.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceDef {
    pub key: SourceKey,
    pub source: Source,
}

/// How a source's value becomes a `0..1` mask.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Profile {
    /// One at `inner_m` and below, zero at `outer_m` and beyond, smooth between.
    ///
    /// The shape a path wants. Two radii rather than a width and a feather,
    /// because the two radii are the numbers an author can see on a map.
    SmoothBand { inner_m: f64, outer_m: f64 },
    /// One below `at`, zero above.
    Threshold { at: f32 },
    /// Zero at `low`, one at `high`, linear between. `low > high` inverts.
    Ramp { low: f32, high: f32 },
    /// The source's own value, clamped.
    Passthrough,
}

impl Profile {}

/// Where a layer applies.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Mask {
    /// Everywhere, at full strength.
    Everywhere,
    /// A source's own value, clamped to `0..1`.
    Source(SourceKey),
    /// A source, shaped.
    Profile { source: SourceKey, shape: Profile },
}

impl Mask {
    /// The source this mask reads, if any.
    pub fn source(&self) -> Option<&SourceKey> {
        match self {
            Self::Everywhere => None,
            Self::Source(key) => Some(key),
            Self::Profile { source, .. } => Some(source),
        }
    }
}

/// How a material layer contributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MaterialMode {
    /// Clear every other material's score and claim the ground.
    Replace,
    /// Add to this material's score, to be normalised against the others.
    ///
    /// The usual one, and the reason material composition is expressed as
    /// *scores* rather than as final weights: an author writing several
    /// overlapping claims should not have to keep them summing to one.
    #[default]
    AddScore,
    /// Scale this material's existing score.
    MultiplyScore,
}

impl MaterialMode {}

/// How a height layer contributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HeightMode {
    #[default]
    Add,
    Replace,
    Max,
    Min,
}

impl HeightMode {}

/// This ground is made of that material.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialLayer {
    pub material: MaterialKey,
    pub mode: MaterialMode,
    pub amount: f32,
}

/// This ground is higher or lower.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevationLayer {
    pub mode: HeightMode,
    pub height_m: f32,
}

/// This ground has a fine displacement on it.
///
/// Separate from elevation, and the separation is load-bearing: elevation is
/// terrain the camera can see the shape of, microrelief is the centimetre-scale
/// texture a renderer displaces or shades with. A path that is six centimetres
/// lower is microrelief; a hill is elevation. Merging them would make every
/// consumer choose a filter width without knowing which it was looking at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MicroreliefLayer {
    pub mode: HeightMode,
    pub displacement_m: f32,
}

/// This ground modifies a declared channel.
#[derive(Clone, Debug, PartialEq)]
pub struct ModifierLayer {
    pub channel: ModifierKey,
    pub mode: ModifierComposition,
    pub value: f32,
}

/// What a layer does where its mask is.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum LayerOperation {
    Material(MaterialLayer),
    Elevation(ElevationLayer),
    Microrelief(MicroreliefLayer),
    Modifier(ModifierLayer),
}

impl LayerOperation {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Material(_) => "Material",
            Self::Elevation(_) => "Elevation",
            Self::Microrelief(_) => "Microrelief",
            Self::Modifier(_) => "Modifier",
        }
    }
}

/// One continuous statement about the terrain.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerDef {
    pub key: LayerKey,
    pub enabled: bool,
    pub mask: Mask,
    pub operation: LayerOperation,
}

/// How much a population wants a material's ground.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialAffinity {
    pub material: MaterialKey,
    pub weight: f32,
}

/// Something discrete that grows or sits on the terrain.
#[derive(Clone, Debug, PartialEq)]
pub struct PopulationDef {
    pub key: PopulationKey,
    pub recipe: RecipeKey,
    pub enabled: bool,
    /// The named stream this population's candidate identities derive from.
    pub seed_stream: StreamKey,
    /// Which materials it will grow on, and how readily. Empty means anywhere.
    pub material_affinity: Vec<MaterialAffinity>,
    /// The declared channel that scales its abundance.
    pub abundance_channel: Option<ModifierKey>,
    pub parameters: ParameterObject,
}

/// A value a recipe understands.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ParameterValue {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    List(Vec<ParameterValue>),
}

impl ParameterValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "Bool",
            Self::Integer(_) => "Integer",
            Self::Number(_) => "Number",
            Self::Text(_) => "Text",
            Self::List(_) => "List",
        }
    }

    /// Every non-finite number this value holds, by path.
    ///
    /// A list rather than a bool, because a parameter object holding three
    /// infinities should report three locations rather than one "somewhere in
    /// here".
    pub fn non_finite_paths(&self, path: &str, into: &mut Vec<String>) {
        match self {
            Self::Number(value) if !value.is_finite() => into.push(path.to_string()),
            Self::List(items) => {
                for (index, item) in items.iter().enumerate() {
                    item.non_finite_paths(&format!("{path}[{index}]"), into);
                }
            }
            _ => {}
        }
    }

    /// Absorb into a digest.
    ///
    /// Not the `Digestible` trait, because a parameter value is a *part* of a
    /// document rather than a thing with its own domain, and giving it a domain
    /// would invite somebody to digest one on its own and compare it against a
    /// value absorbed in context.
    pub fn absorb_into(&self, digest: &mut Digest) {
        match self {
            Self::Bool(value) => {
                digest.tag(0).bool(*value);
            }
            Self::Integer(value) => {
                digest.tag(1).i64(*value);
            }
            Self::Number(value) => {
                digest.tag(2).f64(*value);
            }
            Self::Text(value) => {
                digest.tag(3).str(value);
            }
            Self::List(items) => {
                digest.tag(4).slice(items, |d, item| item.absorb_into(d));
            }
        }
    }
}

/// A recipe's parameters, in a stable order.
///
/// A `BTreeMap` rather than a `HashMap`, and the reason is the digest: an
/// unordered map would serialise and digest differently run to run, so two
/// identical documents would compare unequal and every cache keyed on the digest
/// would miss.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParameterObject {
    entries: BTreeMap<String, ParameterValue>,
}

impl ParameterObject {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, value: ParameterValue) -> &mut Self {
        self.entries.insert(name.into(), value);
        self
    }

    pub fn get(&self, name: &str) -> Option<&ParameterValue> {
        self.entries.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Every entry, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ParameterValue)> {
        self.entries.iter()
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        match self.entries.get(name) {
            Some(ParameterValue::Number(value)) => Some(*value),
            Some(ParameterValue::Integer(value)) => Some(*value as f64),
            _ => None,
        }
    }

    pub fn integer(&self, name: &str) -> Option<i64> {
        match self.entries.get(name) {
            Some(ParameterValue::Integer(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        match self.entries.get(name) {
            Some(ParameterValue::Text(value)) => Some(value),
            _ => None,
        }
    }

    pub fn boolean(&self, name: &str) -> Option<bool> {
        match self.entries.get(name) {
            Some(ParameterValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }
}

impl FromIterator<(String, ParameterValue)> for ParameterObject {
    fn from_iter<I: IntoIterator<Item = (String, ParameterValue)>>(iter: I) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

/// A whole authored terrain, canonicalised.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainDocument {
    pub coordinate_system: CoordinateSystem,
    pub root_seed: RootSeed,
    pub materials: Vec<MaterialDef>,
    pub modifier_channels: Vec<ModifierChannelDef>,
    pub sources: Vec<SourceDef>,
    pub layers: Vec<LayerDef>,
    pub populations: Vec<PopulationDef>,
    pub metadata: DocumentMetadata,
}

impl Default for TerrainDocument {
    fn default() -> Self {
        Self {
            coordinate_system: CoordinateSystem::PlanarMetres,
            root_seed: RootSeed::new(0),
            materials: Vec::new(),
            modifier_channels: Vec::new(),
            sources: Vec::new(),
            layers: Vec::new(),
            populations: Vec::new(),
            metadata: DocumentMetadata::default(),
        }
    }
}

impl TerrainDocument {
    pub fn material(&self, key: &MaterialKey) -> Option<&MaterialDef> {
        self.materials.iter().find(|m| &m.key == key)
    }

    pub fn channel(&self, key: &ModifierKey) -> Option<&ModifierChannelDef> {
        self.modifier_channels.iter().find(|c| &c.key == key)
    }

    pub fn source(&self, key: &SourceKey) -> Option<&SourceDef> {
        self.sources.iter().find(|s| &s.key == key)
    }

    pub fn layer(&self, key: &LayerKey) -> Option<&LayerDef> {
        self.layers.iter().find(|l| &l.key == key)
    }

    pub fn population(&self, key: &PopulationKey) -> Option<&PopulationDef> {
        self.populations.iter().find(|p| &p.key == key)
    }

    /// The canonical digest of everything in this document.
    ///
    /// What a cache is keyed on and what a dataset manifest pins. Two documents
    /// with this digest produce the same terrain, and two that differ anywhere
    /// meaningful produce different digests.
    pub fn digest(&self) -> Fingerprint {
        self.fingerprint("terrain-document")
    }
}

/// The domain name every document digest is taken in.
pub const DOCUMENT_DIGEST_DOMAIN: &str = "terrain-document";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_asset_path_must_be_relative_and_contained() {
        assert!(AssetPath::new("masks/rock_abundance.png").is_ok());
        assert_eq!(AssetPath::new(""), Err(AssetPathError::Empty));
        assert_eq!(AssetPath::new("/etc/passwd"), Err(AssetPathError::Absolute));
        assert_eq!(
            AssetPath::new("../../secrets"),
            Err(AssetPathError::Escaping)
        );
        assert_eq!(
            AssetPath::new("masks/../../secrets"),
            Err(AssetPathError::Escaping)
        );
        assert_eq!(
            AssetPath::new("masks\\rock.png"),
            Err(AssetPathError::Backslash)
        );
        // A `..` inside a name is a filename, not an escape.
        assert!(AssetPath::new("masks/rock..png").is_ok());
    }

    #[test]
    fn a_composition_rules_identity_leaves_a_value_alone() {
        // What an unwritten channel starts from. If these disagreed, "no layer
        // touched this" and "a layer touched it with the identity" would give
        // different answers.
        for rule in [
            ModifierComposition::Multiply,
            ModifierComposition::Add,
            ModifierComposition::Max,
            ModifierComposition::Min,
        ] {
            let identity = rule.identity().expect("has an identity");
            assert_eq!(rule.combine(identity, 0.42), 0.42, "{}", rule.name());
        }
        assert_eq!(ModifierComposition::Replace.identity(), None);
    }

    #[test]
    fn suppressions_compound_under_multiply_and_do_not_under_max() {
        // The reason a channel declares its rule rather than each writer
        // choosing. Two path-side suppressions should compound; two rock zones
        // should not double the rocks.
        let multiply = ModifierComposition::Multiply;
        assert_eq!(multiply.combine(multiply.combine(1.0, 0.5), 0.5), 0.25);
        let max = ModifierComposition::Max;
        assert_eq!(max.combine(max.combine(0.0, 0.85), 0.85), 0.85);
    }

    #[test]
    fn a_value_range_rejects_a_backwards_or_infinite_one() {
        assert!(ValueRange::new(0.0, 1.5).is_valid());
        assert!(!ValueRange::new(1.0, 0.0).is_valid());
        assert!(!ValueRange::new(0.0, 0.0).is_valid());
        assert!(!ValueRange::new(0.0, f32::INFINITY).is_valid());
        assert!(!ValueRange::new(f32::NAN, 1.0).is_valid());
    }

    #[test]
    fn a_raster_placement_needs_a_positive_extent() {
        let good = RasterPlacement {
            origin_m: WorldPoint::new(-32.0, -32.0),
            size_m: WorldVector::new(64.0, 64.0),
            anchor: TexelAnchor::Centre,
            row_order: RowOrder::TopDown,
        };
        assert!(good.is_valid());
        assert!(
            !RasterPlacement {
                size_m: WorldVector::new(0.0, 64.0),
                ..good
            }
            .is_valid()
        );
        assert!(
            !RasterPlacement {
                size_m: WorldVector::new(f64::NAN, 64.0),
                ..good
            }
            .is_valid()
        );
    }

    #[test]
    fn parameters_are_ordered_by_name() {
        // So a document's digest does not depend on insertion order.
        let mut first = ParameterObject::new();
        first
            .insert("zeta", ParameterValue::Integer(1))
            .insert("alpha", ParameterValue::Integer(2));
        let mut second = ParameterObject::new();
        second
            .insert("alpha", ParameterValue::Integer(2))
            .insert("zeta", ParameterValue::Integer(1));
        assert_eq!(first, second);
        assert_eq!(
            first.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
    }

    #[test]
    fn parameters_read_back_at_the_type_they_were_written() {
        let mut parameters = ParameterObject::new();
        parameters
            .insert("sides", ParameterValue::Integer(14))
            .insert("scale", ParameterValue::Number(0.5))
            .insert("name", ParameterValue::Text("boulder".into()))
            .insert("shadowed", ParameterValue::Bool(true));
        assert_eq!(parameters.integer("sides"), Some(14));
        // An integer reads as a number, because an author writing `2` for a
        // scale meant two and should not have to write `2.0`.
        assert_eq!(parameters.number("sides"), Some(14.0));
        assert_eq!(parameters.number("scale"), Some(0.5));
        assert_eq!(parameters.text("name"), Some("boulder"));
        assert_eq!(parameters.boolean("shadowed"), Some(true));
        // And a number does not read as an integer, because rounding silently
        // is worse than reporting.
        assert_eq!(parameters.integer("scale"), None);
        assert_eq!(parameters.number("missing"), None);
    }

    #[test]
    fn a_mask_reports_the_source_it_reads() {
        let key = SourceKey::new("main_path").expect("valid");
        assert_eq!(Mask::Everywhere.source(), None);
        assert_eq!(Mask::Source(key.clone()).source(), Some(&key));
        assert_eq!(
            Mask::Profile {
                source: key.clone(),
                shape: Profile::SmoothBand {
                    inner_m: 1.5,
                    outer_m: 2.6
                }
            }
            .source(),
            Some(&key)
        );
    }
}

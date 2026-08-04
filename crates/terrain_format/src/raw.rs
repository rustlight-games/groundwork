//! The document exactly as it appears on disk.
//!
//! ## Why this exists at all
//!
//! It would be less code to derive `Deserialize` on the semantic types in
//! `terrain_core` and be done. That is the obvious approach and it is a trap,
//! for three reasons that only become visible later:
//!
//! **A file has a version and a value does not.** A document written a year ago
//! is not shaped like today's, and something has to hold the older shape long
//! enough to migrate it. If the only shape in the codebase is the current one,
//! there is nowhere for the old one to live and the answer to "can you still
//! open this" is no.
//!
//! **Deserialisation stops at the first error, and validation must not.** Every
//! key here is a plain `String`. If they were validated key types, serde would
//! reject the document at the first misspelling and the author would find their
//! six mistakes one rebuild at a time. Parsing the keys as text and validating
//! them afterwards is what lets one pass report all six.
//!
//! **The wire format is a compatibility surface and the semantic model is not.**
//! Renaming a semantic field is a refactor; renaming a wire field breaks every
//! document anyone has written. Keeping them the same type means every refactor
//! is a format change, which in practice means the refactor does not happen.
//!
//! ## Unknown fields are errors
//!
//! Every struct here denies them. A misspelled `transition_width_m` that
//! silently does nothing is the worst possible failure for authored content: the
//! document loads, the terrain is wrong, and nothing anywhere says why. The
//! author's next move is to change the value — which also does nothing — and
//! conclude the feature is broken.
//!
//! The cost is that a document cannot carry a comment field the format does not
//! know about. That is a real cost and it is worth paying.

use serde::{Deserialize, Serialize};

/// A document as written.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDocument {
    #[serde(default = "planar_metres")]
    pub coordinate_system: String,
    pub root_seed: String,
    #[serde(default)]
    pub metadata: RawMetadata,
    #[serde(default)]
    pub materials: Vec<RawMaterial>,
    #[serde(default)]
    pub modifier_channels: Vec<RawModifierChannel>,
    #[serde(default)]
    pub sources: Vec<RawSourceEntry>,
    #[serde(default)]
    pub layers: Vec<RawLayer>,
    #[serde(default)]
    pub populations: Vec<RawPopulation>,
}

fn planar_metres() -> String {
    "PlanarMetres".to_string()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMetadata {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMaterial {
    pub key: String,
    #[serde(default)]
    pub display_name: String,
    pub appearance: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawModifierChannel {
    pub key: String,
    #[serde(default)]
    pub display_name: String,
    pub range: (f32, f32),
    pub default_value: f32,
    #[serde(default = "multiply")]
    pub composition: String,
    #[serde(default = "unitless")]
    pub unit: String,
}

fn multiply() -> String {
    "Multiply".to_string()
}

fn unitless() -> String {
    "Unitless".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSourceEntry {
    pub key: String,
    pub source: RawSource,
}

/// Where a raster sits.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPlacement {
    pub origin_m: (f64, f64),
    pub size_m: (f64, f64),
    #[serde(default = "centre")]
    pub anchor: String,
    #[serde(default = "top_down")]
    pub row_order: String,
}

fn centre() -> String {
    "Centre".to_string()
}

fn top_down() -> String {
    "TopDown".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawSource {
    Constant(RawConstant),
    Noise(RawNoise),
    ScalarRaster(RawScalarRaster),
    CategoricalRaster(RawCategoricalRaster),
    WeightRaster(RawWeightRaster),
    SplineDistance(RawSplineDistance),
    ShapeDistance(RawShapeDistance),
    Custom(RawCustomSource),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConstant {
    pub value: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawNoise {
    #[serde(default = "perlin")]
    pub kind: String,
    pub frequency_per_m: f64,
    #[serde(default = "one_octave")]
    pub octaves: u32,
    #[serde(default = "two")]
    pub lacunarity: f64,
    #[serde(default = "half")]
    pub gain: f64,
    pub stream: String,
}

fn perlin() -> String {
    "Perlin".to_string()
}

fn one_octave() -> u32 {
    1
}

fn two() -> f64 {
    2.0
}

fn half() -> f64 {
    0.5
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawScalarRaster {
    pub asset: String,
    pub world_transform: RawPlacement,
    #[serde(default = "bilinear")]
    pub filter: String,
    #[serde(default)]
    pub wrap: RawWrap,
}

fn bilinear() -> String {
    "Bilinear".to_string()
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawWrap {
    #[default]
    Clamp,
    Repeat,
    Value(f32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRasterClass {
    pub value: u32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCategoricalRaster {
    pub asset: String,
    pub world_transform: RawPlacement,
    pub classes: Vec<RawRasterClass>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawWeightRaster {
    pub asset: String,
    pub world_transform: RawPlacement,
    #[serde(default = "bilinear")]
    pub filter: String,
    pub channels: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSplineDistance {
    pub asset: String,
    pub max_distance_m: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawShapeDistance {
    pub asset: String,
    pub max_distance_m: f64,
    #[serde(default)]
    pub signed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCustomSource {
    pub recipe: String,
    #[serde(default)]
    pub parameters: RawParameters,
}

/// A recipe's parameters as written.
///
/// A `Vec` of pairs rather than a map, because RON's map syntax and its struct
/// syntax look confusingly alike in a file that already has both, and because
/// duplicate keys in a map are silently resolved by whichever the parser saw
/// last. As a list, a duplicate is visible and can be reported.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawParameters(pub Vec<(String, RawParameterValue)>);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawParameterValue {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    List(Vec<RawParameterValue>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawLayer {
    pub key: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub mask: RawMask,
    pub operation: RawOperation,
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawMask {
    Everywhere,
    /// A bare source name, at its own value.
    Source(String),
    Profile(RawProfileMask),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawProfileMask {
    pub source: String,
    pub shape: RawProfile,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawProfile {
    SmoothBand(RawSmoothBand),
    Threshold(RawThreshold),
    Ramp(RawRamp),
    Passthrough,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSmoothBand {
    pub inner_m: f64,
    pub outer_m: f64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawThreshold {
    pub at: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRamp {
    pub low: f32,
    pub high: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RawOperation {
    Material(RawMaterialOperation),
    Elevation(RawHeightOperation),
    Microrelief(RawHeightOperation),
    Modifier(RawModifierOperation),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMaterialOperation {
    pub material: String,
    #[serde(default = "add_score")]
    pub mode: String,
    #[serde(default = "one")]
    pub amount: f32,
}

fn add_score() -> String {
    "AddScore".to_string()
}

fn one() -> f32 {
    1.0
}

/// Elevation and microrelief share a shape: a mode and a number of metres.
///
/// One raw struct for both, because they differ only in what the number means
/// and the semantic model is where that distinction belongs. The field is named
/// for neither, so a document reads `(mode: Add, metres: -0.06)` in both cases.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawHeightOperation {
    #[serde(default = "add")]
    pub mode: String,
    pub metres: f32,
}

fn add() -> String {
    "Add".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawModifierOperation {
    pub channel: String,
    #[serde(default = "multiply")]
    pub mode: String,
    pub value: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPopulation {
    pub key: String,
    pub recipe: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub seed_stream: String,
    #[serde(default)]
    pub material_affinity: Vec<(String, f32)>,
    #[serde(default)]
    pub abundance_channel: Option<String>,
    #[serde(default)]
    pub parameters: RawParameters,
}

//! Where code that a document can name gets registered.
//!
//! ## Explicit, not automatic
//!
//! There is a well-known pattern for this: a linker-section trick or an
//! inventory crate, so a custom source registers itself by existing and the
//! registry assembles at startup. It is convenient and it is the wrong choice
//! here, for three reasons.
//!
//! **Duplicate detection stops being deterministic.** Two crates registering the
//! same key is a real mistake, and with automatic registration which one wins
//! depends on link order — so the same document produces different terrain
//! depending on how the binary was built.
//!
//! **Tests cannot vary it.** A test that wants a registry with one source in it
//! has to arrange for exactly one source to be linked, which is not something a
//! test can arrange.
//!
//! **The set of recipes becomes invisible.** With explicit registration, one
//! function lists everything the binary can do, and a reviewer can read it.
//!
//! So a registry is built by calling `register` and passing it around. The cost
//! is a function that has to be kept up to date; the benefit is that the
//! function exists and can be read.
//!
//! ## Resolving assets is a trait, not a path
//!
//! [`AssetResolver`] exists because `prepare` must be callable with no
//! filesystem. A test wants an in-memory mask, a language server wants the
//! editor's unsaved buffer, and a CI job wants to validate a document with no
//! assets checked out at all. Handing `prepare` a directory would make all three
//! impossible.

use std::collections::BTreeMap;

use crate::coords::WorldPoint;
use crate::diagnostics::DiagnosticReport;
use crate::document::{ParameterObject, SourceDef};
use crate::ids::RecipeKey;
use crate::sample::SampleFootprint;

/// One scalar field, ready to be read.
///
/// The interface every source compiles down to. Deliberately narrow: a source
/// answers "what is your value here", and everything about *what that value
/// means* — whether it is a material score, a height or a mask — is the layer's
/// business rather than the source's. That separation is what lets one spline
/// drive a material, a depression and a suppression without being three sources.
pub trait ScalarField: Send + Sync {
    /// The value at a point.
    ///
    /// `footprint` is the area the sample covers. A source is free to ignore it
    /// — most do — but one that filters or drops octaves should read it rather
    /// than aliasing.
    fn value_at(&self, point: WorldPoint, footprint: SampleFootprint) -> f32;

    /// How far this field's influence reaches beyond the region being baked.
    ///
    /// Used to size a bake's halo. A source that is unbounded — noise, a
    /// constant — returns zero, meaning "no halo needed"; one with a finite
    /// reach, like a spline distance, returns it. Getting this wrong in the
    /// small direction is a seam at every page edge.
    fn reach_m(&self) -> f64 {
        0.0
    }

    /// A short description, for a debug view.
    fn describe(&self) -> String {
        "field".to_string()
    }
}

/// Everything a custom source is given when it is compiled.
pub struct SourceContext<'a> {
    pub parameters: &'a ParameterObject,
    pub resolver: &'a dyn AssetResolver,
    /// Problems found while compiling. A source reports here rather than
    /// returning an error, so several bad parameters produce several messages.
    pub diagnostics: &'a mut DiagnosticReport,
}

/// Code that turns an authored custom source into a field.
pub trait SourceRecipe: Send + Sync {
    /// The key a document names this by.
    fn key(&self) -> RecipeKey;

    /// A version, mixed into every seed this recipe derives.
    ///
    /// Bump it when the recipe is *meant* to produce something different.
    /// Without it, an improvement to a recipe is indistinguishable from a bug
    /// in every cache and manifest downstream.
    fn version(&self) -> u32 {
        1
    }

    /// Compile the source, or report why not.
    fn compile(&self, context: &mut SourceContext<'_>) -> Option<Box<dyn ScalarField>>;
}

/// The custom sources a binary knows about.
#[derive(Default)]
pub struct SourceRegistry {
    recipes: BTreeMap<String, Box<dyn SourceRecipe>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a recipe.
    ///
    /// Returns whether it was new. A caller registering a duplicate key has a
    /// real problem — two pieces of code claiming the same name — and gets told
    /// about it at the moment it happens rather than discovering later that one
    /// of them silently never runs.
    pub fn register(&mut self, recipe: Box<dyn SourceRecipe>) -> bool {
        let key = recipe.key().as_str().to_string();
        if self.recipes.contains_key(&key) {
            return false;
        }
        self.recipes.insert(key, recipe);
        true
    }

    pub fn get(&self, key: &RecipeKey) -> Option<&dyn SourceRecipe> {
        self.recipes.get(key.as_str()).map(|r| r.as_ref())
    }

    pub fn contains(&self, key: &RecipeKey) -> bool {
        self.recipes.contains_key(key.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }

    /// Every registered key, in order.
    ///
    /// Ordered because it appears in diagnostics and in `terrain inspect`, and a
    /// list that reordered between runs would make two otherwise-identical
    /// outputs differ.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.recipes.keys().map(|k| k.as_str())
    }
}

impl std::fmt::Debug for SourceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceRegistry")
            .field("recipes", &self.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Why an asset could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    NotFound,
    /// Found, and not readable as what it claims to be.
    Unreadable(String),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Unreadable(why) => write!(f, "unreadable: {why}"),
        }
    }
}

impl std::error::Error for AssetError {}

/// Where a document's assets come from.
///
/// Bytes rather than decoded images, so this trait stays free of an image
/// codec — and so an implementation can be a directory, a zip, an in-memory map
/// or an editor's unsaved buffers without any of them needing to agree on a
/// decoder.
pub trait AssetResolver: Send + Sync {
    /// Read an asset by its document-relative path.
    fn read(&self, path: &str) -> Result<Vec<u8>, AssetError>;

    /// Whether an asset exists, without reading it.
    ///
    /// Separate so validation can check every reference cheaply. The default
    /// reads and throws the bytes away, which is correct and slow; an
    /// implementation over a filesystem should override it.
    fn exists(&self, path: &str) -> bool {
        self.read(path).is_ok()
    }
}

/// A resolver with nothing in it.
///
/// What `prepare` gets in a test that has no assets, and what a CI validation
/// job uses. Every read fails, which is the honest answer.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAssets;

impl AssetResolver for NoAssets {
    fn read(&self, _path: &str) -> Result<Vec<u8>, AssetError> {
        Err(AssetError::NotFound)
    }

    fn exists(&self, _path: &str) -> bool {
        false
    }
}

/// A resolver over a map held in memory.
///
/// For tests, and for anything that has already loaded its assets.
#[derive(Clone, Debug, Default)]
pub struct MemoryAssets {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemoryAssets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, path: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.files.insert(path.into(), bytes.into());
        self
    }

    pub fn insert(&mut self, path: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        self.files.insert(path.into(), bytes.into());
    }
}

impl AssetResolver for MemoryAssets {
    fn read(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        self.files.get(path).cloned().ok_or(AssetError::NotFound)
    }

    fn exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
}

/// A constant field, for the simplest source there is.
pub struct ConstantField {
    pub value: f32,
}

impl ScalarField for ConstantField {
    fn value_at(&self, _point: WorldPoint, _footprint: SampleFootprint) -> f32 {
        self.value
    }

    fn describe(&self) -> String {
        format!("constant {}", self.value)
    }
}

/// The version a built-in source contributes to a seed.
///
/// One number for every source this crate implements, rather than one each.
/// They are versioned together because they change together — a fix to the
/// shared noise basis moves all of them — and separate constants would give a
/// false impression of independence.
pub const BUILTIN_SOURCE_VERSION: u32 = 1;

/// Whether a source needs any asset at all.
///
/// Lets validation check reachability without a resolver having to be present.
pub fn source_asset(source: &crate::document::Source) -> Option<&str> {
    use crate::document::Source;
    match source {
        Source::ScalarRaster(raster) => Some(raster.asset.as_str()),
        Source::CategoricalRaster(raster) => Some(raster.asset.as_str()),
        Source::WeightRaster(raster) => Some(raster.asset.as_str()),
        Source::SplineDistance(spline) => Some(spline.asset.as_str()),
        Source::ShapeDistance(shape) => Some(shape.asset.as_str()),
        Source::Constant(_) | Source::Noise(_) | Source::Custom(_) => None,
    }
}

/// Every asset a document refers to, in document order, without duplicates.
pub fn document_assets(document: &crate::document::TerrainDocument) -> Vec<&str> {
    let mut assets = Vec::new();
    for SourceDef { source, .. } in &document.sources {
        if let Some(asset) = source_asset(source)
            && !assets.contains(&asset)
        {
            assets.push(asset);
        }
    }
    assets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::*;
    use crate::ids::SourceKey;

    struct Stub(RecipeKey);

    impl SourceRecipe for Stub {
        fn key(&self) -> RecipeKey {
            self.0.clone()
        }

        fn compile(&self, _context: &mut SourceContext<'_>) -> Option<Box<dyn ScalarField>> {
            Some(Box::new(ConstantField { value: 1.0 }))
        }
    }

    fn recipe(key: &str) -> Box<dyn SourceRecipe> {
        Box::new(Stub(RecipeKey::new(key).expect("valid")))
    }

    #[test]
    fn a_duplicate_registration_is_refused_rather_than_silently_winning() {
        // With automatic registration this would be decided by link order, and
        // the same document would produce different terrain depending on how the
        // binary was built.
        let mut registry = SourceRegistry::new();
        assert!(registry.register(recipe("source.noise")));
        assert!(!registry.register(recipe("source.noise")));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_registry_lists_its_keys_in_a_stable_order() {
        let mut registry = SourceRegistry::new();
        registry.register(recipe("source.zeta"));
        registry.register(recipe("source.alpha"));
        registry.register(recipe("source.middle"));
        assert_eq!(
            registry.keys().collect::<Vec<_>>(),
            ["source.alpha", "source.middle", "source.zeta"]
        );
    }

    #[test]
    fn an_unregistered_key_is_absent_rather_than_defaulted() {
        let registry = SourceRegistry::new();
        assert!(registry.is_empty());
        assert!(!registry.contains(&RecipeKey::new("source.noise").expect("valid")));
        assert!(
            registry
                .get(&RecipeKey::new("source.noise").expect("valid"))
                .is_none()
        );
    }

    #[test]
    fn the_empty_resolver_fails_honestly() {
        // Rather than returning empty bytes, which every decoder would then
        // report as a corrupt file.
        assert_eq!(NoAssets.read("masks/rock.png"), Err(AssetError::NotFound));
        assert!(!NoAssets.exists("masks/rock.png"));
    }

    #[test]
    fn a_memory_resolver_serves_what_it_was_given() {
        let assets = MemoryAssets::new().with("masks/rock.png", vec![1, 2, 3]);
        assert_eq!(assets.read("masks/rock.png"), Ok(vec![1, 2, 3]));
        assert!(assets.exists("masks/rock.png"));
        assert_eq!(assets.read("masks/other.png"), Err(AssetError::NotFound));
    }

    #[test]
    fn a_documents_assets_are_listed_once_each_in_order() {
        let raster = |asset: &str| {
            Source::ScalarRaster(ScalarRasterSource {
                asset: AssetPath::new(asset).expect("valid"),
                placement: RasterPlacement {
                    origin_m: WorldPoint::ORIGIN,
                    size_m: crate::coords::WorldVector::new(1.0, 1.0),
                    anchor: crate::coords::TexelAnchor::Centre,
                    row_order: crate::coords::RowOrder::TopDown,
                },
                filter: RasterFilter::Bilinear,
                wrap: RasterWrap::Clamp,
            })
        };
        let document = TerrainDocument {
            sources: vec![
                SourceDef {
                    key: SourceKey::new("b").expect("valid"),
                    source: raster("masks/b.png"),
                },
                SourceDef {
                    key: SourceKey::new("a").expect("valid"),
                    source: raster("masks/a.png"),
                },
                SourceDef {
                    key: SourceKey::new("b_again").expect("valid"),
                    source: raster("masks/b.png"),
                },
                SourceDef {
                    key: SourceKey::new("nothing").expect("valid"),
                    source: Source::Constant(ConstantSource { value: 1.0 }),
                },
            ],
            ..TerrainDocument::default()
        };
        assert_eq!(document_assets(&document), ["masks/b.png", "masks/a.png"]);
    }

    #[test]
    fn a_constant_field_ignores_where_it_is_asked() {
        let field = ConstantField { value: 0.25 };
        assert_eq!(
            field.value_at(WorldPoint::ORIGIN, SampleFootprint::Point),
            0.25
        );
        assert_eq!(
            field.value_at(WorldPoint::new(1e6, -1e6), SampleFootprint::circle(3.0)),
            0.25
        );
        assert_eq!(field.reach_m(), 0.0, "a constant needs no halo");
    }
}

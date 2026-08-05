//! What to bake, and what a bake produced.
//!
//! ## Four words that were doing two jobs
//!
//! Fixed here, because the ambiguity was costing real confusion:
//!
//! - A **plate** is one logical raster output over a requested world rectangle.
//!   It is what somebody asked for.
//! - A **page** is one storage tile *within* a plate. An implementation detail
//!   of holding a plate, and the unit a runtime cache evicts.
//! - A **scene** is renderable geometry and attributes.
//! - A **render** is one renderer's output from a scene.
//!
//! The old code used "page" for all four, which meant a sentence about page
//! borders could be about the cache, about a bake's tiling, or about a training
//! crop, and the reader had to work out which.
//!
//! ## A cache filename is not a record of what produced it
//!
//! [`BakeManifest`] carries the document digest, the root seed, the recipe
//! versions, the scene fingerprint, the bounds, the texel mapping, the page
//! layout and per-page checksums. That looks like a lot to write beside a PNG,
//! and every field is there because its absence has a specific failure:
//!
//! - Without the **document digest**, a cache serves a plate for a document
//!   that has since changed, and the only symptom is terrain that does not
//!   match what the author is editing.
//! - Without the **recipe versions**, a plate baked before and after a
//!   generator fix are indistinguishable, and half a corpus has one look.
//! - Without the **texel mapping**, a plate cannot be resampled correctly,
//!   because nothing records whether its texels were centres or edges.
//! - Without **per-page checksums**, a truncated write is a page of black that
//!   nothing reports.

use terrain_core::coords::{WorldRect, WorldVector};
use terrain_core::digest::{Digest, Digestible, Fingerprint};
use terrain_core::ids::{LayerKey, MaterialKey, ModifierKey, PopulationKey};

/// How finely to bake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BakeResolution {
    /// Texels per world metre.
    TexelsPerMetre(f32),
    /// A fixed output size, whatever ground it covers.
    Fixed { width: u32, height: u32 },
}

impl BakeResolution {
    /// The output size for a rectangle of ground.
    pub fn size_for(self, bounds: WorldRect) -> [u32; 2] {
        match self {
            Self::TexelsPerMetre(density) => {
                let density = density.max(1.0e-3) as f64;
                [
                    (bounds.width_m() * density).round().max(1.0) as u32,
                    (bounds.height_m() * density).round().max(1.0) as u32,
                ]
            }
            Self::Fixed { width, height } => [width.max(1), height.max(1)],
        }
    }
}

/// How a plate is cut into pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLayout {
    /// One page. The whole plate.
    Single,
    /// Square pages of the given side, in texels.
    Square { side: u32 },
}

impl PageLayout {
    /// How many pages across and down a plate of this size holds.
    pub fn pages_for(self, size: [u32; 2]) -> [u32; 2] {
        match self {
            Self::Single => [1, 1],
            Self::Square { side } => {
                let side = side.max(1);
                [size[0].div_ceil(side), size[1].div_ceil(side)]
            }
        }
    }
}

/// Whether a plate carries reduced copies of itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MipPolicy {
    /// None. What an offline framework wants: a plate is baked at the scale it
    /// is looked at.
    #[default]
    None,
    /// Down to a single texel.
    Full,
    /// A fixed number of levels.
    Levels(u8),
}

/// One channel a bake can write.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BakeOutput {
    /// Every material's weight, one plane each.
    MaterialWeights,
    Elevation,
    MicroDisplacement,
    /// The microrelief's gradient, encoded as a normal.
    MicroNormal,
    /// One declared channel.
    Modifier(ModifierKey),
    /// One material's weight alone, for a debug view.
    MaterialDebug(MaterialKey),
    /// One layer's mask, so an author can see where it applies.
    LayerDebug(LayerKey),
    /// Where one population's accepted candidates landed.
    PopulationDebug(PopulationKey),
}

impl BakeOutput {
    /// The name this channel is written under.
    pub fn name(&self) -> String {
        match self {
            Self::MaterialWeights => "material_weights".into(),
            Self::Elevation => "elevation".into(),
            Self::MicroDisplacement => "micro_displacement".into(),
            Self::MicroNormal => "micro_normal".into(),
            Self::Modifier(key) => format!("modifier.{key}"),
            Self::MaterialDebug(key) => format!("material.{key}"),
            Self::LayerDebug(key) => format!("layer.{key}"),
            Self::PopulationDebug(key) => format!("population.{key}"),
        }
    }
}

/// Whether a bake may use every core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExecutionPreference {
    /// Threaded. Pages are independent by construction, so this changes nothing
    /// about the result.
    #[default]
    Parallel,
    /// One thread. For a measurement that wants a number it can compare.
    Serial,
}

/// What to bake.
#[derive(Clone, Debug)]
pub struct BakeRequest {
    pub bounds: WorldRect,
    pub resolution: BakeResolution,
    pub page_layout: PageLayout,
    /// Texels of overlap between neighbouring pages.
    ///
    /// Not the same as the *halo*. A halo is world-space reach — how far outside
    /// a region a mark can shade into it — and is derived from the recipes. A
    /// border is texels of duplicated output, so a page can be filtered without
    /// reading its neighbour. A page with no border still has correct pixels;
    /// it just cannot be bilinearly sampled at its own edge.
    pub border_texels: u32,
    pub mip_policy: MipPolicy,
    pub outputs: Vec<BakeOutput>,
    pub execution: ExecutionPreference,
}

impl BakeRequest {
    /// A single plate of one channel over a rectangle.
    pub fn plate(bounds: WorldRect, texels_per_metre: f32, output: BakeOutput) -> Self {
        Self {
            bounds,
            resolution: BakeResolution::TexelsPerMetre(texels_per_metre),
            page_layout: PageLayout::Single,
            border_texels: 0,
            mip_policy: MipPolicy::None,
            outputs: vec![output],
            execution: ExecutionPreference::Parallel,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.resolution.size_for(self.bounds)
    }

    pub fn pages(&self) -> [u32; 2] {
        self.page_layout.pages_for(self.size())
    }

    /// The world extent of one texel.
    pub fn texel_size(&self) -> WorldVector {
        let size = self.size();
        WorldVector::new(
            self.bounds.width_m() / size[0] as f64,
            self.bounds.height_m() / size[1] as f64,
        )
    }
}

impl Digestible for BakeRequest {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .f64(self.bounds.min.u_m)
            .f64(self.bounds.min.v_m)
            .f64(self.bounds.max.u_m)
            .f64(self.bounds.max.v_m);
        match self.resolution {
            BakeResolution::TexelsPerMetre(density) => {
                digest.tag(0).f32(density);
            }
            BakeResolution::Fixed { width, height } => {
                digest.tag(1).u32(width).u32(height);
            }
        }
        match self.page_layout {
            PageLayout::Single => {
                digest.tag(0);
            }
            PageLayout::Square { side } => {
                digest.tag(1).u32(side);
            }
        }
        digest.u32(self.border_texels);
        match self.mip_policy {
            MipPolicy::None => {
                digest.tag(0);
            }
            MipPolicy::Full => {
                digest.tag(1);
            }
            MipPolicy::Levels(levels) => {
                digest.tag(2).u32(levels as u32);
            }
        }
        digest.slice(&self.outputs, |d, output| {
            d.str(&output.name());
        });
        // Execution deliberately absent: threading changes nothing about the
        // result, so two plates that differ only in how they were computed must
        // share a digest or every cache misses for no reason.
    }
}

/// One page's record in a manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct PageRecord {
    pub column: u32,
    pub row: u32,
    pub path: String,
    /// A checksum of the page's bytes.
    ///
    /// Not for tamper detection — for truncation. A short write is a page of
    /// black that nothing reports, and a length plus a digest is the cheapest
    /// thing that turns it into an error.
    pub checksum: Fingerprint,
    pub bytes: usize,
}

/// Everything needed to say what produced a plate.
///
/// See the module note for why each field is here.
#[derive(Clone, Debug)]
pub struct BakeManifest {
    pub manifest_version: u32,
    /// The document this came from.
    pub document_digest: Fingerprint,
    /// The compiled sampler's compatibility version.
    pub prepared_version: u32,
    pub root_seed: u64,
    /// Every recipe that contributed, and its version.
    pub recipe_versions: Vec<(String, u32)>,
    /// The scene, if one was built.
    pub scene_fingerprint: Option<Fingerprint>,
    pub request: BakeRequest,
    /// Whether texels are centres or edges.
    pub texel_anchor: terrain_core::coords::TexelAnchor,
    pub row_order: terrain_core::coords::RowOrder,
    /// The materials this plate's weight planes carry, in plane order.
    pub materials: Vec<String>,
    pub modifiers: Vec<String>,
    pub pages: Vec<PageRecord>,
    /// How each channel is encoded on disk.
    pub encodings: Vec<(String, String)>,
    /// The Cycles profile's digest, when one was involved.
    pub render_profile_digest: Option<Fingerprint>,
}

/// The version this build writes.
pub const MANIFEST_VERSION: u32 = 1;

impl BakeManifest {
    /// A digest of the whole manifest, for a cache key.
    pub fn digest(&self) -> Fingerprint {
        let mut digest = Digest::for_domain("terrain-bake-manifest");
        digest
            .u32(self.manifest_version)
            .digest(self.document_digest)
            .u32(self.prepared_version)
            .u64(self.root_seed);
        digest.slice(&self.recipe_versions, |d, (key, version)| {
            d.str(key).u32(*version);
        });
        match self.scene_fingerprint {
            Some(fingerprint) => {
                digest.tag(1).digest(fingerprint);
            }
            None => {
                digest.tag(0);
            }
        }
        self.request.absorb(&mut digest);
        digest
            .tag(match self.texel_anchor {
                terrain_core::coords::TexelAnchor::Centre => 0,
                terrain_core::coords::TexelAnchor::Edge => 1,
            })
            .tag(match self.row_order {
                terrain_core::coords::RowOrder::TopDown => 0,
                terrain_core::coords::RowOrder::BottomUp => 1,
            });
        digest.slice(&self.materials, |d, name| {
            d.str(name);
        });
        digest.slice(&self.modifiers, |d, name| {
            d.str(name);
        });
        digest.finish()
    }

    /// Whether this manifest describes the same bake as another.
    ///
    /// Compares everything that decides the *content* and deliberately not the
    /// page checksums: a cache asking "is this still valid" wants to know
    /// whether the inputs moved, not whether the bytes it already has are the
    /// bytes it wrote.
    pub fn describes_same_bake(&self, other: &Self) -> bool {
        self.digest() == other.digest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::coords::{RowOrder, TexelAnchor, WorldPoint};

    fn bounds() -> WorldRect {
        WorldRect::new(WorldPoint::new(-8.0, -8.0), WorldPoint::new(8.0, 8.0))
    }

    fn manifest() -> BakeManifest {
        BakeManifest {
            manifest_version: MANIFEST_VERSION,
            document_digest: Fingerprint::from_u128(0x1111),
            prepared_version: 1,
            root_seed: 0x8df7_82f9_5ce1_a4d4,
            recipe_versions: vec![("population.grass_lush".into(), 1)],
            scene_fingerprint: Some(Fingerprint::from_u128(0x2222)),
            request: BakeRequest::plate(bounds(), 96.0, BakeOutput::Elevation),
            texel_anchor: TexelAnchor::Centre,
            row_order: RowOrder::TopDown,
            materials: vec!["grass_lush".into()],
            modifiers: vec!["vegetation_density".into()],
            pages: vec![PageRecord {
                column: 0,
                row: 0,
                path: "page-000-000.exr".into(),
                checksum: Fingerprint::from_u128(0x3333),
                bytes: 1024,
            }],
            encodings: vec![("elevation".into(), "f32".into())],
            render_profile_digest: None,
        }
    }

    #[test]
    fn a_resolution_in_texels_per_metre_scales_with_the_ground() {
        let request = BakeRequest::plate(bounds(), 96.0, BakeOutput::Elevation);
        assert_eq!(request.size(), [1536, 1536]);
        // And a fixed size ignores the ground.
        let fixed = BakeRequest {
            resolution: BakeResolution::Fixed {
                width: 512,
                height: 256,
            },
            ..request
        };
        assert_eq!(fixed.size(), [512, 256]);
    }

    #[test]
    fn a_texel_covers_the_ground_the_resolution_claims() {
        let request = BakeRequest::plate(bounds(), 96.0, BakeOutput::Elevation);
        let texel = request.texel_size();
        assert!((texel.du_m - 1.0 / 96.0).abs() < 1.0e-9, "{}", texel.du_m);
        assert!((texel.dv_m - 1.0 / 96.0).abs() < 1.0e-9);
    }

    #[test]
    fn pages_cover_a_plate_that_does_not_divide_evenly() {
        // A plate is not obliged to be a multiple of its page size, and a
        // rounding-down here is a strip of the plate that is never written.
        let layout = PageLayout::Square { side: 256 };
        assert_eq!(layout.pages_for([512, 512]), [2, 2]);
        assert_eq!(layout.pages_for([513, 512]), [3, 2]);
        assert_eq!(layout.pages_for([1, 1]), [1, 1]);
        assert_eq!(PageLayout::Single.pages_for([4096, 4096]), [1, 1]);
    }

    #[test]
    fn every_output_has_a_name_that_says_which_channel_it_is() {
        let material = MaterialKey::new("grass_lush").expect("valid");
        let modifier = ModifierKey::new("vegetation_density").expect("valid");
        assert_eq!(BakeOutput::Elevation.name(), "elevation");
        assert_eq!(
            BakeOutput::Modifier(modifier).name(),
            "modifier.vegetation_density"
        );
        assert_eq!(
            BakeOutput::MaterialDebug(material).name(),
            "material.grass_lush"
        );
        // A debug channel and the real one cannot collide.
        assert_ne!(
            BakeOutput::MaterialWeights.name(),
            BakeOutput::MaterialDebug(MaterialKey::new("grass_lush").expect("valid")).name()
        );
    }

    #[test]
    fn threading_does_not_change_a_bakes_identity() {
        // Pages are independent by construction, so two plates that differ only
        // in how they were computed must share a digest — or every cache misses
        // for no reason.
        let serial = BakeManifest {
            request: BakeRequest {
                execution: ExecutionPreference::Serial,
                ..manifest().request
            },
            ..manifest()
        };
        assert!(manifest().describes_same_bake(&serial));
    }

    #[test]
    fn everything_that_decides_the_content_reaches_the_digest() {
        let base = manifest();
        let reference = base.digest();

        let mut other_document = base.clone();
        other_document.document_digest = Fingerprint::from_u128(0x9999);
        assert_ne!(reference, other_document.digest(), "document");

        let mut other_seed = base.clone();
        other_seed.root_seed = 7;
        assert_ne!(reference, other_seed.digest(), "seed");

        let mut other_recipe = base.clone();
        other_recipe.recipe_versions[0].1 = 2;
        assert_ne!(reference, other_recipe.digest(), "recipe version");

        let mut other_scene = base.clone();
        other_scene.scene_fingerprint = Some(Fingerprint::from_u128(0x4444));
        assert_ne!(reference, other_scene.digest(), "scene");

        let mut other_bounds = base.clone();
        other_bounds.request.bounds = WorldRect::new(
            terrain_core::coords::WorldPoint::ORIGIN,
            terrain_core::coords::WorldPoint::new(1.0, 1.0),
        );
        assert_ne!(reference, other_bounds.digest(), "bounds");

        let mut other_anchor = base.clone();
        other_anchor.texel_anchor = TexelAnchor::Edge;
        assert_ne!(reference, other_anchor.digest(), "texel anchor");

        let mut other_rows = base.clone();
        other_rows.row_order = RowOrder::BottomUp;
        assert_ne!(reference, other_rows.digest(), "row order");

        let mut other_materials = base.clone();
        other_materials.materials.push("dirt_compacted".into());
        assert_ne!(reference, other_materials.digest(), "materials");
    }

    #[test]
    fn the_page_checksums_do_not_reach_the_digest() {
        // A cache asking "is this still valid" wants to know whether the inputs
        // moved, not whether the bytes it has are the bytes it wrote.
        let mut rewritten = manifest();
        rewritten.pages[0].checksum = Fingerprint::from_u128(0xdead);
        rewritten.pages[0].bytes = 2048;
        assert!(manifest().describes_same_bake(&rewritten));
    }

    #[test]
    fn a_manifest_records_the_texel_mapping() {
        // Without it a plate cannot be resampled correctly, because nothing
        // records whether its texels were centres or edges.
        let manifest = manifest();
        assert_eq!(manifest.texel_anchor, TexelAnchor::Centre);
        assert_eq!(manifest.row_order, RowOrder::TopDown);
    }
}

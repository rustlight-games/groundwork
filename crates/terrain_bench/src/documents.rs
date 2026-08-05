//! Loading a shipped terrain document, the way the CLI does.
//!
//! A benchmark that measured a synthetic in-memory document would measure the
//! test's idea of a document rather than the ones the project actually renders.
//! The laboratory scenarios and the pinned baselines both compile real files out
//! of `assets/terrain/`, which means this crate needs the same four steps the
//! CLI performs: read, resolve profiles, prepare, and know where the assets sit
//! relative to the document that names them.
//!
//! It is here rather than duplicated in each test because the asset-root rule is
//! the kind of thing that gets subtly re-derived: a document in
//! `assets/terrain/documents/` names `features/main_path.spline.ron`, which is
//! two directories up and back down. Getting that wrong produces "source not
//! found" rather than a wrong picture, so it is not dangerous — but three copies
//! of it is three places to fix when the layout moves.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use terrain_core::prepare::{PrepareOptions, PreparedTerrain};

/// Why a document could not be brought up for measurement.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("could not read {path}: {message}")]
    Read { path: String, message: String },
    #[error("ground profiles for {path}:\n{message}")]
    Profiles { path: String, message: String },
    #[error("{path} does not prepare:\n{message}")]
    Prepare { path: String, message: String },
}

/// Assets, resolved beside the document that names them.
struct BesideDocument {
    root: PathBuf,
}

impl terrain_core::AssetResolver for BesideDocument {
    fn read(&self, path: &str) -> Result<Vec<u8>, terrain_core::AssetError> {
        std::fs::read(self.root.join(path)).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => terrain_core::AssetError::NotFound,
            _ => terrain_core::AssetError::Unreadable(error.to_string()),
        })
    }

    fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }
}

/// Where a document's assets live: one level up from `documents/`.
pub fn asset_root(document: &Path) -> PathBuf {
    let directory = document.parent().unwrap_or(Path::new("."));
    if directory.file_name().and_then(|n| n.to_str()) == Some("documents") {
        directory.parent().unwrap_or(directory).to_path_buf()
    } else {
        directory.to_path_buf()
    }
}

/// The repository root, found from this crate's own manifest directory.
///
/// `CARGO_MANIFEST_DIR` is `crates/terrain_bench`, so the root is two up. Used
/// rather than the working directory because `cargo test` runs with the working
/// directory set to the crate, and a scenario naming `assets/terrain/...` should
/// mean the same file whichever crate is running it.
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate manifest lives two directories below the repository root")
        .to_path_buf()
}

/// A path under the repository root.
pub fn in_repository(relative: impl AsRef<Path>) -> PathBuf {
    repository_root().join(relative)
}

/// Read, resolve and prepare one authored document.
pub fn prepare(document: &Path) -> Result<Arc<PreparedTerrain>, LoadError> {
    let loaded = terrain_format::load(document).map_err(|error| LoadError::Read {
        path: document.display().to_string(),
        message: error.to_string(),
    })?;

    let assets = BesideDocument {
        root: asset_root(document),
    };

    let named = loaded
        .document
        .materials
        .iter()
        .filter_map(|material| material.profile.clone());
    let (profiles, problems) = terrain_format::load_library(named, &assets);
    if !problems.is_empty() {
        return Err(LoadError::Profiles {
            path: document.display().to_string(),
            message: problems
                .iter()
                .map(|problem| problem.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }

    terrain_core::prepare(
        &loaded.document,
        &assets,
        &terrain_core::SourceRegistry::new(),
        &PrepareOptions {
            profiles,
            ..PrepareOptions::default()
        },
    )
    .map_err(|report| LoadError::Prepare {
        path: document.display().to_string(),
        message: report.to_string(),
    })
}

/// Prepare a document with every layer that reads one source removed.
///
/// ## What a control has to hold fixed
///
/// The instrument this exists for is a paired measurement: the same world with
/// and without a feature, so that everything the feature did not cause divides
/// out. That only works if the two halves differ by the feature *and nothing
/// else*.
///
/// Dropping the whole `SemanticOverlay` is not that. It removes the document's
/// tuned population controls and every stone interaction along with the path,
/// so a difference measured against it is a difference against a world that is
/// not the same world in several ways at once — and a far-field agreement
/// cannot prove local equivalence, which is exactly where the measurement is
/// taken.
///
/// Removing the layers that read one source leaves materials, channels,
/// populations, seeds and every other layer untouched. The document is edited
/// in memory rather than on disk, because a control written into the asset tree
/// is an asset somebody will later find and wonder about.
pub fn prepare_without_source(
    document: &Path,
    source: &str,
) -> Result<Arc<PreparedTerrain>, LoadError> {
    let mut loaded = terrain_format::load(document).map_err(|error| LoadError::Read {
        path: document.display().to_string(),
        message: error.to_string(),
    })?;

    let before = loaded.document.layers.len();
    loaded
        .document
        .layers
        .retain(|layer| layer.mask.source().map(|key| key.as_str()) != Some(source));
    assert!(
        loaded.document.layers.len() < before,
        "{}: no layer reads `{source}`, so this control is the document itself",
        document.display()
    );

    let assets = BesideDocument {
        root: asset_root(document),
    };
    let named = loaded
        .document
        .materials
        .iter()
        .filter_map(|material| material.profile.clone());
    let (profiles, problems) = terrain_format::load_library(named, &assets);
    if !problems.is_empty() {
        return Err(LoadError::Profiles {
            path: document.display().to_string(),
            message: problems
                .iter()
                .map(|problem| problem.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }

    terrain_core::prepare(
        &loaded.document,
        &assets,
        &terrain_core::SourceRegistry::new(),
        &PrepareOptions {
            profiles,
            ..PrepareOptions::default()
        },
    )
    .map_err(|report| LoadError::Prepare {
        path: document.display().to_string(),
        message: report.to_string(),
    })
}

/// The documents shipped in the repository, by name.
///
/// Listed rather than globbed. A glob would silently start measuring a document
/// somebody dropped into the directory to try something out, and a pinned
/// baseline that grows rows on its own is not pinned.
pub const SHIPPED: &[&str] = &[
    "constant_grass",
    "blend_lab",
    "meadow_path",
    "narrow_track",
    "flower_meadow",
    "stony_pasture",
    "wet_hollow",
    "wet_and_dry",
];

/// The documents the shared-candidate compiler can actually build.
///
/// Not all of them, and the gap is real rather than an oversight in this list.
/// `constant_grass` and `blend_lab` name recipes from the older
/// [`terrain_generators::population`] registry — `population.grass_lush`,
/// `population.wildflowers_meadow`, `population.granite_rocks`,
/// `population.dirt_scatter` — which generate their own candidates and decide
/// their own acceptance. `compile_scene` reads the
/// [`terrain_generators::recipe`] registry instead, so it refuses them with
/// `unknown_recipe`.
///
/// That is a documented state, not a bug to paper over here: the two documents
/// still validate, still prepare, and still describe what they were written to
/// describe. What they cannot do is take part in shared candidate domains, and
/// a baseline that pretended otherwise by silently skipping them would hide the
/// day somebody migrates them.
pub const COMPILABLE: &[&str] = &[
    "meadow_path",
    "narrow_track",
    "flower_meadow",
    "stony_pasture",
    "wet_hollow",
    "wet_and_dry",
];

/// The documents that prepare but cannot compile, and why.
pub const NOT_COMPILABLE: &[(&str, &str)] = &[
    (
        "constant_grass",
        "names population.grass_lush from the older population registry",
    ),
    (
        "blend_lab",
        "names population.grass_lush, wildflowers_meadow, granite_rocks and \
         dirt_scatter from the older population registry",
    ),
];

/// The path to one shipped document.
pub fn shipped(name: &str) -> PathBuf {
    in_repository(format!("assets/terrain/documents/{name}.terrain.ron"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_document_prepares() {
        // The cheapest possible guard on the asset tree: a document that stopped
        // loading would otherwise be discovered by a benchmark reporting zero of
        // everything, which reads as a generator regression.
        for name in SHIPPED {
            let path = shipped(name);
            assert!(path.exists(), "{} is missing", path.display());
            prepare(&path).unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    #[test]
    fn the_asset_root_climbs_out_of_the_documents_directory() {
        assert_eq!(
            asset_root(Path::new("/x/assets/terrain/documents/a.terrain.ron")),
            PathBuf::from("/x/assets/terrain")
        );
        // A document outside that layout resolves against its own directory,
        // which is what a one-off laboratory file in a temporary directory
        // needs.
        assert_eq!(
            asset_root(Path::new("/tmp/scratch/a.terrain.ron")),
            PathBuf::from("/tmp/scratch")
        );
    }
}

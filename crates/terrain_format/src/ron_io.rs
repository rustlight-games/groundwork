//! Reading and writing terrain documents as RON.
//!
//! The one thing worth explaining here is the two-pass read. A document is
//! parsed **twice**: once as a bare [`EnvelopeHeader`] to learn its version, and
//! then as a whole envelope. That looks wasteful and is the only way the format
//! can survive its own evolution — a version-two body cannot be parsed by a
//! build that only knows version one, so the version has to be readable without
//! parsing the body at all.
//!
//! The cost is parsing a small file twice. The alternative is a format that can
//! only ever be read by the build that wrote it.
//!
//! ## A large error is fine here
//!
//! [`LoadError`] is a few hundred bytes, because it carries a path, a parser's
//! span, and — for the interesting variant — every diagnostic found. Clippy
//! objects to a wide `Result`, and the objection is aimed at hot paths where the
//! success value is small and returned millions of times. Loading a document
//! happens once per document, and the alternative is boxing four variants to
//! save a memcpy that happens once. The report is boxed because it is unbounded;
//! the rest is not, on purpose.
#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};

use terrain_core::diagnostics::{DiagnosticReport, Location};
use terrain_core::document::TerrainDocument;

use crate::canonical::canonicalise;
use crate::envelope::{
    CURRENT_FORMAT_VERSION, EnvelopeHeader, TERRAIN_DOCUMENT_FORMAT, TerrainEnvelope,
};
use crate::migration::{MigrationError, MigrationLog, migrate};
use crate::raw::RawDocument;

/// Why a document could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not valid RON: {source}")]
    Syntax {
        path: PathBuf,
        #[source]
        source: ron::error::SpannedError,
    },

    #[error(
        "{path} says its format is `{found}`, not `{TERRAIN_DOCUMENT_FORMAT}` — this \
         is not a terrain document"
    )]
    WrongFormat { path: PathBuf, found: String },

    #[error("{path}: {source}")]
    Migration {
        path: PathBuf,
        #[source]
        source: MigrationError,
    },

    /// The document parsed and does not mean anything.
    ///
    /// The report is boxed because it carries every diagnostic found, which
    /// makes it by far the largest variant — and an unboxed one would widen
    /// every `Result` in the crate to its size, including the overwhelmingly
    /// common success path.
    #[error("{path} has problems:\n{report}")]
    Invalid {
        path: PathBuf,
        report: Box<DiagnosticReport>,
    },
}

/// A document, and everything learned while reading it.
#[derive(Debug)]
pub struct LoadedDocument {
    pub document: TerrainDocument,
    /// What the file claimed to be before migration.
    pub source_version: u32,
    pub migration: MigrationLog,
    /// Warnings and notes. Errors would have come back as [`LoadError`].
    pub report: DiagnosticReport,
}

/// Read a document from text.
///
/// `name` is used only to label diagnostics, so this is usable from a test with
/// no filesystem.
pub fn from_str(text: &str, name: &str) -> Result<LoadedDocument, LoadError> {
    let path = PathBuf::from(name);

    // Pass one: the version, without the body. See the module note.
    let header: EnvelopeHeader = ron::from_str(text).map_err(|source| LoadError::Syntax {
        path: path.clone(),
        source,
    })?;
    if header.format != TERRAIN_DOCUMENT_FORMAT {
        return Err(LoadError::WrongFormat {
            path,
            found: header.format,
        });
    }
    let source_version = header.format_version;

    // Pass two: the whole thing.
    let envelope: TerrainEnvelope = ron::from_str(text).map_err(|source| LoadError::Syntax {
        path: path.clone(),
        source,
    })?;

    let (raw, migration) =
        migrate(envelope.document, source_version).map_err(|source| LoadError::Migration {
            path: path.clone(),
            source,
        })?;

    let (document, mut report) = canonicalise(&raw);
    let Some(document) = document else {
        return Err(LoadError::Invalid {
            path,
            report: Box::new(report),
        });
    };
    if report.has_errors() {
        return Err(LoadError::Invalid {
            path,
            report: Box::new(report),
        });
    }

    // Semantic validation, on the canonicalised document.
    report.absorb(terrain_core::validate::validate(&document));
    if report.has_errors() {
        return Err(LoadError::Invalid {
            path,
            report: Box::new(report),
        });
    }

    Ok(LoadedDocument {
        document,
        source_version,
        migration,
        report,
    })
}

/// Read a document from a file.
pub fn load(path: &Path) -> Result<LoadedDocument, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_str(&text, &path.display().to_string())
}

/// Serialise a raw document at the current format version.
///
/// Takes a [`RawDocument`] rather than a [`TerrainDocument`] deliberately.
/// Writing is a *round trip through the wire types*, so anything that cannot be
/// expressed on the wire cannot be written — which is what stops the two models
/// drifting apart in the direction nobody tests.
pub fn to_string(document: &RawDocument) -> Result<String, ron::Error> {
    let envelope = TerrainEnvelope::current(document.clone());
    let config = ron::ser::PrettyConfig::new()
        .depth_limit(8)
        .indentor("    ")
        .struct_names(true);
    ron::ser::to_string_pretty(&envelope, config)
}

/// Write a document beside its assets.
pub fn save(path: &Path, document: &RawDocument) -> Result<(), LoadError> {
    let text = to_string(document).map_err(|error| LoadError::Invalid {
        path: path.to_path_buf(),
        report: Box::new({
            let mut report = DiagnosticReport::new();
            report.error(
                "serialise_failed",
                Location::default(),
                format!("could not serialise the document: {error}"),
            );
            report
        }),
    })?;
    std::fs::write(path, text).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The version this build writes, for a caller that wants to say so.
pub const fn current_version() -> u32 {
    CURRENT_FORMAT_VERSION
}

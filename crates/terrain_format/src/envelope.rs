//! What a terrain file says it is, before anything reads what it contains.
//!
//! Two fields, both load-bearing.
//!
//! **`format`** is a string, and its only job is to fail loudly when somebody
//! hands the loader the wrong file. Without it, a spline asset fed to the
//! document reader produces a deserialisation error about a missing `root_seed`
//! — which reads as "this document is broken" rather than "this is not a
//! document", and sends the author looking in the wrong place.
//!
//! **`format_version`** is what makes a document openable next year. It is
//! checked before the body is read, so a file from the future is refused with
//! its own version number in the message rather than through whatever confusing
//! shape mismatch it happens to produce.
//!
//! ## The version is on the envelope, not in the body
//!
//! Deliberately, and it is the difference between a format that can migrate and
//! one that cannot. Reading the version requires parsing the file; if the
//! version lives inside the body then parsing the body has to succeed before the
//! version is known — and the body is exactly the thing whose shape changed. So
//! the envelope's own shape is frozen forever, and everything that may move
//! lives inside it.

use serde::{Deserialize, Serialize};

use crate::raw::RawDocument;

/// The `format` string every terrain document carries.
pub const TERRAIN_DOCUMENT_FORMAT: &str = "terrain-document";

/// The version this build writes.
pub const CURRENT_FORMAT_VERSION: u32 = 1;

/// The oldest version this build can still migrate from.
///
/// Equal to the current one today, because there is only one. It exists so the
/// refusal message can say "too old, the oldest readable is N" rather than
/// leaving somebody to work that out.
pub const OLDEST_READABLE_VERSION: u32 = 1;

/// A document, with what it is and which version of it.
///
/// **This struct's shape is frozen.** Adding a field here would mean older
/// readers could not parse the envelope of a newer file, which is precisely the
/// situation the envelope exists to prevent. Anything new goes in the document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerrainEnvelope {
    pub format: String,
    pub format_version: u32,
    pub document: RawDocument,
}

impl TerrainEnvelope {
    /// Wrap a raw document at the current version.
    pub fn current(document: RawDocument) -> Self {
        Self {
            format: TERRAIN_DOCUMENT_FORMAT.to_string(),
            format_version: CURRENT_FORMAT_VERSION,
            document,
        }
    }

    /// Whether this envelope claims to be a terrain document at all.
    pub fn is_terrain_document(&self) -> bool {
        self.format == TERRAIN_DOCUMENT_FORMAT
    }
}

/// Just the envelope's header, for reading a version without reading a body.
///
/// The whole reason the envelope's shape is frozen: this parses successfully
/// against a document of *any* version, including one whose body this build
/// cannot represent.
#[derive(Clone, Debug, Deserialize)]
pub struct EnvelopeHeader {
    pub format: String,
    pub format_version: u32,
}

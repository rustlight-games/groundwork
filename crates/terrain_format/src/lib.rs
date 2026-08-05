//! Reading, migrating and writing versioned terrain documents.
//!
//! ```text
//! file on disk
//!   → parse the envelope        which format, which version
//!   → parse the body            RawDocument, unknown fields refused
//!   → migrate                   one step per version, up to the current one
//!   → canonicalise              strings become validated keys; problems collected
//!   → validate                  the semantic pass, in terrain_core
//!   → TerrainDocument
//! ```
//!
//! Five stages where one would do, and every one of them earns its place by
//! being the stage that *cannot* be merged with its neighbours:
//!
//! - The envelope parses without the body, so a document written by a newer
//!   build is refused by its version number instead of by a confusing shape
//!   mismatch.
//! - The body is a separate set of types from the semantic model, so an old
//!   shape has somewhere to live while it is being migrated, and so every key is
//!   a plain string that validation can report on rather than a parse error that
//!   stops at the first one.
//! - Migration is pure and never validates, so an old *broken* document still
//!   opens and can be shown to its author.
//! - Canonicalisation collects, so four misspellings are four diagnostics.
//! - Validation is semantic and lives in `terrain_core`, so a document can be
//!   checked by anything that has a document — a test, a language server, a CI
//!   job with no assets checked out.
//!
//! ## Unknown fields are errors
//!
//! Every wire struct denies them. A misspelled `transition_width_m` that
//! silently does nothing is the worst failure mode authored content has: the
//! file loads, the terrain is wrong, and nothing says why. The author's next
//! move is to change the value — which also does nothing — and conclude the
//! feature is broken.

#![forbid(unsafe_code)]

pub mod canonical;
pub mod envelope;
pub mod ground_profile;
pub mod migration;
pub mod raw;
pub mod ron_io;

pub use canonical::canonicalise;
pub use envelope::{
    CURRENT_FORMAT_VERSION, EnvelopeHeader, OLDEST_READABLE_VERSION, TERRAIN_DOCUMENT_FORMAT,
    TerrainEnvelope,
};
pub use ground_profile::{
    CURRENT_PROFILE_VERSION, GROUND_PROFILE_FORMAT, ProfileError, load_library,
};
pub use migration::{MigrationError, MigrationLog, migrate};
pub use raw::RawDocument;
pub use ron_io::{LoadError, LoadedDocument, from_str, load, save, to_string};

//! Content errors.
//!
//! Content is authored by hand and increasingly by generation tooling, so most
//! of these will be read by someone trying to work out what they typed wrong.
//! Every variant names the offending key.

use smol_str::SmolStr;
use thiserror::Error;

pub type ContentResult<T> = Result<T, ContentError>;

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("{file}: {source}")]
    Parse {
        file: String,
        #[source]
        source: Box<ron::error::SpannedError>,
    },

    #[error("could not read {file}: {source}")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },

    #[error("duplicate {kind} key '{key}'")]
    DuplicateKey { kind: &'static str, key: SmolStr },

    #[error("{referrer} refers to {kind} '{key}', which does not exist")]
    UnknownReference {
        referrer: SmolStr,
        kind: &'static str,
        key: SmolStr,
    },

    #[error("effect kind '{kind}' is not registered (referenced by '{referrer}')")]
    UnknownEffectKind { referrer: SmolStr, kind: SmolStr },

    #[error("generator '{key}' is not registered (referenced by '{referrer}')")]
    UnknownGenerator { referrer: SmolStr, key: SmolStr },

    #[error("'{context}' is missing required parameter '{param}'")]
    MissingParam { context: SmolStr, param: SmolStr },

    #[error("'{context}' parameter '{param}' should be {expected}, found {found}")]
    WrongParamType {
        context: SmolStr,
        param: SmolStr,
        expected: &'static str,
        found: &'static str,
    },

    #[error("'{context}': {message}")]
    Invalid { context: SmolStr, message: String },
}

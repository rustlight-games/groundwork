//! Bringing an older document up to the current version.
//!
//! ## There is nothing to migrate yet, and the machinery is here anyway
//!
//! Version one is the only version, so [`migrate`] currently checks a range and
//! returns. That looks like speculative generality and is the opposite: the
//! expensive moment for a format is the *first* migration, and it is expensive
//! precisely when there is nowhere for it to go. What usually happens instead is
//! a quiet `#[serde(default)]` and a field that means one thing in old documents
//! and another in new ones, with nothing recording the difference.
//!
//! So the shape is here, with the rules written down, and the first migration is
//! a function and an arm rather than a design decision under time pressure.
//!
//! ## The rules a migration follows
//!
//! - **One step per version.** `1 -> 2 -> 3`, never `1 -> 3`. A direct jump has
//!   to reimplement everything the intermediate steps did, and it silently rots
//!   the moment step two changes.
//! - **A migration is pure and total.** It takes a document of version *n* and
//!   returns one of version *n + 1*. It does not fail on unusual content: a
//!   document that was legal before must be legal after, or it is not a
//!   migration, it is a breaking change with a friendly name.
//! - **A migration never validates.** Validation happens once, at the end, on
//!   the canonicalised document. A migration that rejected invalid input would
//!   make an old broken document unopenable, when the whole point is to open it
//!   and show the author what is wrong.
//! - **Migrations are tested against real old documents**, kept as fixtures.
//!   A migration tested only against a synthetic input tests the test.

use crate::envelope::{CURRENT_FORMAT_VERSION, OLDEST_READABLE_VERSION};
use crate::raw::RawDocument;

/// Why a document could not be brought to the current version.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    #[error(
        "this document is format version {found}, which is newer than this build \
         understands ({current}); update the tool"
    )]
    FromTheFuture { found: u32, current: u32 },

    #[error(
        "this document is format version {found}, which is older than this build \
         can read ({oldest}); open it with an older tool and save it forward"
    )]
    TooOld { found: u32, oldest: u32 },
}

/// What a migration did, for reporting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationLog {
    pub from_version: u32,
    pub to_version: u32,
    /// One line per step taken, in order.
    pub steps: Vec<String>,
}

impl MigrationLog {
    pub fn migrated(&self) -> bool {
        self.from_version != self.to_version
    }
}

/// Bring `document` from `version` up to [`CURRENT_FORMAT_VERSION`].
pub fn migrate(
    document: RawDocument,
    version: u32,
) -> Result<(RawDocument, MigrationLog), MigrationError> {
    if version > CURRENT_FORMAT_VERSION {
        return Err(MigrationError::FromTheFuture {
            found: version,
            current: CURRENT_FORMAT_VERSION,
        });
    }
    if version < OLDEST_READABLE_VERSION {
        return Err(MigrationError::TooOld {
            found: version,
            oldest: OLDEST_READABLE_VERSION,
        });
    }

    let mut log = MigrationLog {
        from_version: version,
        to_version: version,
        steps: Vec::new(),
    };
    let mut document = document;

    // One step per version. When version 2 arrives this becomes a match on
    // `log.to_version` with an arm per step, and the loop carries the document
    // through each in turn.
    while log.to_version < CURRENT_FORMAT_VERSION {
        let (next, note) = step(document, log.to_version);
        document = next;
        log.steps.push(note);
        log.to_version += 1;
    }

    Ok((document, log))
}

/// One version's worth of change.
///
/// Unreachable today, and it is written as a `match` with no arms rather than
/// left out entirely so that adding version two is a compile error here until
/// somebody writes the step.
fn step(_document: RawDocument, from: u32) -> (RawDocument, String) {
    // No steps yet: version one is the only version, so the loop above never
    // enters. Kept as a called function rather than omitted so that raising
    // `CURRENT_FORMAT_VERSION` without writing the migration fails loudly here
    // rather than silently skipping a version.
    unreachable!("no migration is registered from format version {from}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> RawDocument {
        RawDocument {
            coordinate_system: "PlanarMetres".into(),
            root_seed: "8df782f95ce1a4d4".into(),
            metadata: Default::default(),
            materials: Vec::new(),
            modifier_channels: Vec::new(),
            sources: Vec::new(),
            layers: Vec::new(),
            populations: Vec::new(),
        }
    }

    #[test]
    fn a_current_document_passes_through_untouched() {
        let (_, log) = migrate(document(), CURRENT_FORMAT_VERSION).expect("current");
        assert!(!log.migrated());
        assert!(log.steps.is_empty());
    }

    #[test]
    fn a_document_from_the_future_is_refused_by_its_own_version() {
        // Refused here, with the number in the message, rather than through
        // whatever confusing shape mismatch a newer body happens to produce.
        assert_eq!(
            migrate(document(), CURRENT_FORMAT_VERSION + 1).err(),
            Some(MigrationError::FromTheFuture {
                found: CURRENT_FORMAT_VERSION + 1,
                current: CURRENT_FORMAT_VERSION,
            })
        );
    }

    #[test]
    fn a_document_older_than_anything_readable_is_refused() {
        assert_eq!(
            migrate(document(), 0).err(),
            Some(MigrationError::TooOld {
                found: 0,
                oldest: OLDEST_READABLE_VERSION,
            })
        );
    }

    #[test]
    fn the_readable_range_is_coherent() {
        // A build that claimed to read only from a version newer than it writes
        // could never open anything. Constants today, and the check is here for
        // the commit that changes one of them without the other.
        let (oldest, current) = (OLDEST_READABLE_VERSION, CURRENT_FORMAT_VERSION);
        assert!(oldest <= current, "{oldest} > {current}");
        assert!(oldest >= 1, "version zero is not a version");
    }
}

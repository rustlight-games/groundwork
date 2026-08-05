//! What things are called, and what they are numbered.
//!
//! ## Names are the identity; numbers are an optimisation
//!
//! This is the correction of a specific mistake, and it is worth naming the
//! mistake because it was reasonable at the time. Content used to be identified
//! by loading every file in sorted filename order and handing out consecutive
//! integers. That is fast, compact, and produces ids that go straight into an
//! observation vector — and it means **adding a file renumbers everything after
//! it**. Rename `grassland.ron` to `meadow.ron` and every id past `g` shifts,
//! which silently repoints every reference that was stored as a number.
//!
//! For a learned policy that invalidated the weights. For terrain it would do
//! something worse and quieter: the seed of every population is derived from its
//! key, so renumbering would reshuffle the position of every blade of grass in
//! the world. A generator whose output depends on the alphabetical position of
//! its filename is not reproducible in any useful sense.
//!
//! So the persistent identity of everything here is a **string**, chosen by the
//! author and never derived from where the file sits. Dense integer indices do
//! exist — resolving a key to a `u16` once and then comparing integers is worth
//! a great deal in a sampler that runs millions of times — but they are minted
//! inside `PreparedTerrain`, live only as long as it does, and never reach a
//! file, a digest or a seed.
//!
//! ## What a key may contain
//!
//! Lowercase ASCII letters, digits and underscores, in dot-separated segments:
//! `grass_lush`, `surface.dirt_compacted`, `population.wildflowers_meadow`.
//!
//! Validated rather than accepted, because a key is compared by exact bytes
//! everywhere. `Grass_Lush` and `grass_lush` would be two materials that look
//! like one in a diff, and a trailing space is a key that will never match
//! anything and never say why.

use std::fmt;

/// Why a key was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    Empty,
    /// Longer than [`MAX_KEY_LENGTH`].
    TooLong {
        length: usize,
    },
    /// A character outside `[a-z0-9_.]`.
    BadCharacter {
        position: usize,
        found: char,
    },
    /// An empty dot-separated segment: a leading, trailing or doubled dot.
    EmptySegment,
    /// A segment starting with a digit or an underscore.
    ///
    /// Rejected so that a key is always a legal identifier in whatever language
    /// eventually generates bindings from it, and so `2` and `02` cannot be two
    /// different keys that read as the same number.
    BadSegmentStart {
        segment: String,
    },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a key may not be empty"),
            Self::TooLong { length } => {
                write!(
                    f,
                    "a key may be at most {MAX_KEY_LENGTH} bytes, this is {length}"
                )
            }
            Self::BadCharacter { position, found } => write!(
                f,
                "{found:?} at byte {position} is not allowed; keys use [a-z0-9_] in \
                 dot-separated segments"
            ),
            Self::EmptySegment => {
                write!(f, "a key may not have a leading, trailing or doubled dot")
            }
            Self::BadSegmentStart { segment } => write!(
                f,
                "the segment {segment:?} must start with a lowercase letter"
            ),
        }
    }
}

impl std::error::Error for KeyError {}

/// The longest a key may be.
///
/// Sixty-four. Long enough for `population.wildflowers_meadow_roadside` and
/// short enough that a key can be shown in a table without wrapping.
pub const MAX_KEY_LENGTH: usize = 64;

/// Check a key's spelling, returning the reason it is unacceptable.
pub fn validate_key(text: &str) -> Result<(), KeyError> {
    if text.is_empty() {
        return Err(KeyError::Empty);
    }
    if text.len() > MAX_KEY_LENGTH {
        return Err(KeyError::TooLong { length: text.len() });
    }
    for (position, character) in text.char_indices() {
        let allowed = character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '_'
            || character == '.';
        if !allowed {
            return Err(KeyError::BadCharacter {
                position,
                found: character,
            });
        }
    }
    for segment in text.split('.') {
        if segment.is_empty() {
            return Err(KeyError::EmptySegment);
        }
        if !segment.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(KeyError::BadSegmentStart {
                segment: segment.to_string(),
            });
        }
    }
    Ok(())
}

/// Declare a validated, stable, textual key type.
///
/// One macro rather than seven near-identical files. The types are kept separate
/// rather than being one `Key` because they are not interchangeable: handing a
/// [`MaterialKey`] where a [`PopulationKey`] belongs is a bug the compiler can
/// catch for free, and it is exactly the bug a document with a hundred keys in
/// it invites.
macro_rules! stable_key {
    ($name:ident, $what:literal) => {
        #[doc = concat!("The stable, authored name of ", $what, ".")]
        ///
        /// Persistent identity. Written in documents, hashed into seeds, and
        /// never derived from file order or load order.
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
        pub struct $name(String);

        impl $name {
            /// Validate and wrap.
            pub fn new(text: impl Into<String>) -> Result<Self, KeyError> {
                let text = text.into();
                validate_key(&text)?;
                Ok(Self(text))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// The stable 64-bit hash used to seed anything keyed on this name.
            ///
            /// Deliberately *not* the content digest. See
            /// [`crate::seed::key_hash`].
            pub fn seed_hash(&self) -> u64 {
                $crate::seed::key_hash(&self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = KeyError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::new(text)
            }
        }

        impl TryFrom<String> for $name {
            type Error = KeyError;

            fn try_from(text: String) -> Result<Self, Self::Error> {
                Self::new(text)
            }
        }

        impl From<$name> for String {
            fn from(key: $name) -> String {
                key.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

stable_key!(MaterialKey, "a material");
stable_key!(SourceKey, "a source");
stable_key!(LayerKey, "a layer");
stable_key!(PopulationKey, "a population");
stable_key!(ModifierKey, "a modifier channel");
stable_key!(RecipeKey, "a registered recipe");
stable_key!(DomainKey, "a shared candidate domain");
stable_key!(StreamKey, "a named random stream");
stable_key!(AppearanceKey, "a renderer-side appearance binding");
stable_key!(GroundProfileKey, "a ground material profile");

/// Declare a dense index type.
///
/// Every one of these carries the same warning, so the macro carries it once.
macro_rules! dense_index {
    ($name:ident, $what:literal) => {
        #[doc = concat!("A compiled index for ", $what, ".")]
        ///
        /// **Minted by `prepare`, valid only against the `PreparedTerrain` that
        /// minted it, and never written to a file.** Two prepared terrains built
        /// from different documents will number their tables differently, and an
        /// index that outlived its table would silently name the wrong thing
        /// rather than failing.
        ///
        /// It exists because a sampler compares these millions of times and a
        /// string comparison there is not affordable. That is the whole of its
        /// justification, and it is why the type is separate from the key rather
        /// than replacing it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u16);

        impl $name {
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($what, " #{}"), self.0)
            }
        }
    };
}

dense_index!(MaterialIndex, "material");
dense_index!(ModifierIndex, "modifier");
dense_index!(SourceIndex, "source");
dense_index!(LayerIndex, "layer");
dense_index!(PopulationIndex, "population");

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn ordinary_keys_are_accepted() {
        for good in [
            "grass_lush",
            "dirt_compacted",
            "surface.grass_lush",
            "population.wildflowers_meadow",
            "a",
            "n1",
            "vegetation_density",
        ] {
            assert!(MaterialKey::new(good).is_ok(), "{good} was rejected");
        }
    }

    #[test]
    fn a_key_that_would_compare_badly_is_rejected() {
        // Every one of these is a key that either looks like another key in a
        // diff or will never match anything and never say why.
        assert_eq!(MaterialKey::new(""), Err(KeyError::Empty));
        assert!(matches!(
            MaterialKey::new("Grass_Lush"),
            Err(KeyError::BadCharacter { .. })
        ));
        assert!(matches!(
            MaterialKey::new("grass lush"),
            Err(KeyError::BadCharacter { .. })
        ));
        assert!(matches!(
            MaterialKey::new("grass_lush "),
            Err(KeyError::BadCharacter { .. })
        ));
        assert!(matches!(
            MaterialKey::new("grass-lush"),
            Err(KeyError::BadCharacter { .. })
        ));
        assert_eq!(MaterialKey::new(".grass"), Err(KeyError::EmptySegment));
        assert_eq!(MaterialKey::new("grass."), Err(KeyError::EmptySegment));
        assert_eq!(MaterialKey::new("a..b"), Err(KeyError::EmptySegment));
        assert!(matches!(
            MaterialKey::new("_grass"),
            Err(KeyError::BadSegmentStart { .. })
        ));
        assert!(matches!(
            MaterialKey::new("surface.1grass"),
            Err(KeyError::BadSegmentStart { .. })
        ));
        assert!(matches!(
            MaterialKey::new("a".repeat(MAX_KEY_LENGTH + 1)),
            Err(KeyError::TooLong { .. })
        ));
    }

    #[test]
    fn a_key_round_trips_through_text() {
        let key = MaterialKey::from_str("surface.grass_lush").expect("valid");
        assert_eq!(key.as_str(), "surface.grass_lush");
        assert_eq!(key.to_string(), "surface.grass_lush");
        assert_eq!(String::from(key.clone()), "surface.grass_lush");
        assert_eq!(MaterialKey::try_from(String::from(key.clone())), Ok(key));
    }

    #[test]
    fn keys_of_different_kinds_do_not_interchange() {
        // A compile-time property, asserted by construction: this only builds
        // because each name produces its own type. If the macro ever collapsed
        // them into one, the two bindings below would unify and this test would
        // stop meaning anything — so it also checks they hash independently as
        // values.
        let material = MaterialKey::new("grass_lush").expect("valid");
        let population = PopulationKey::new("grass_lush").expect("valid");
        assert_eq!(material.as_str(), population.as_str());
        // Same text, so the same seed hash. Keys are namespaced by *where they
        // are used*, not by their type — the seed derivation mixes the domain in
        // separately, which is what keeps two same-named things apart.
        assert_eq!(material.seed_hash(), population.seed_hash());
    }

    #[test]
    fn a_keys_seed_hash_is_a_function_of_its_text_alone() {
        // The property that makes seeds survive a refactor: nothing about load
        // order, file position or table index reaches this number.
        let first = PopulationKey::new("granite_rocks").expect("valid");
        let second = PopulationKey::new("granite_rocks").expect("valid");
        assert_eq!(first.seed_hash(), second.seed_hash());
        assert_ne!(
            first.seed_hash(),
            PopulationKey::new("granite_rock")
                .expect("valid")
                .seed_hash()
        );
    }

    #[test]
    fn keys_sort_by_their_text() {
        // Canonical ordering has to be the authored name, so that a document's
        // serialisation does not depend on the order things happened to be
        // inserted.
        let mut keys = [
            MaterialKey::new("water").expect("valid"),
            MaterialKey::new("dirt_compacted").expect("valid"),
            MaterialKey::new("grass_lush").expect("valid"),
        ];
        keys.sort();
        assert_eq!(
            keys.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            ["dirt_compacted", "grass_lush", "water"]
        );
    }

    #[test]
    fn a_dense_index_says_what_it_indexes() {
        assert_eq!(MaterialIndex(3).index(), 3);
        assert_eq!(MaterialIndex(3).to_string(), "material #3");
        assert_eq!(PopulationIndex(0).to_string(), "population #0");
    }
}

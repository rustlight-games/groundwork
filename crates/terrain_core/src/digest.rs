//! Whether two things are the same thing.
//!
//! Three questions get asked over and over in this framework, and all three
//! reduce to comparing a number:
//!
//! - *Is this the same document that produced the cache on disk?*
//! - *Did moving that code between crates change the meadow?*
//! - *Did these two dataset shards come from the same generator?*
//!
//! A digest answers all of them in constant space, and the alternative — keeping
//! the thing itself around to compare against — is not available for any of
//! them. You cannot store a scene beside every render.
//!
//! ## Not the seed hash
//!
//! [`crate::seed`] also hashes. It must not be this one, and the separation is
//! deliberate rather than an oversight:
//!
//! - This hash decides **whether two things are equal**. Improving it is a
//!   maintenance change with no visible consequences.
//! - That hash decides **where every plant in the world goes**. Changing it
//!   relocates the entire world.
//!
//! Merge them and the first kind of change silently becomes the second. The two
//! live in separate modules with separate version constants precisely so that
//! nobody has to remember this.
//!
//! ## What may go into a digest
//!
//! Semantic values, and nothing else. Not pointer addresses, not `Vec`
//! capacities, not `Debug` strings, not the order a `HashMap` happened to
//! iterate in. A digest that moves for a reason nobody can explain is a digest
//! whose failures get accepted without looking, which is worse than not having
//! one — it manufactures the habit of re-accepting baselines unread.
//!
//! ## Exact against quantised
//!
//! [`Digest::f32`] and [`Digest::f64`] absorb an exact bit pattern, for a value
//! somebody *chose*. [`Digest::quantised`] rounds first, for a value that fell
//! out of a long chain of transcendental functions where the last bit is
//! arithmetic noise. Using the first where the second belongs produces a digest
//! that changes when the compiler's optimiser does; using the second where the
//! first belongs produces one that misses a real change.

use std::fmt;

/// The version of the digest construction in this module.
///
/// Separate from [`crate::seed::SEED_ALGORITHM_VERSION`], and that separation is
/// the point: this one may move for maintenance, and moving it invalidates
/// cached comparisons and nothing else.
pub const DIGEST_ALGORITHM_VERSION: u32 = 1;

/// FNV-1a's 128-bit offset basis.
const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;

/// FNV-1a's 128-bit prime, `2^88 + 0x13B`.
const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

/// An order-sensitive accumulator.
///
/// FNV-1a's structure over 64-bit words rather than bytes: xor the word into the
/// low half, multiply the whole 128 bits by a prime with a high set bit. The
/// multiply carries the low half's information upward, so a change to the very
/// first value absorbed still moves the top byte of the result.
///
/// Not cryptographic and not trying to be. A digest here is compared against a
/// value in a file in this repository, by a test, on a machine that also holds
/// the code that produced it. The threat is a silent reordering, not an
/// adversary.
#[derive(Clone, Copy, Debug)]
pub struct Digest {
    state: u128,
}

impl Default for Digest {
    fn default() -> Self {
        Self::new()
    }
}

impl Digest {
    pub const fn new() -> Self {
        Self {
            state: OFFSET_BASIS,
        }
    }

    /// An accumulator already separated by what is being digested.
    ///
    /// A material table and a layer table can hold the same values in the same
    /// order and are not the same thing. Starting from a domain keeps them
    /// apart without every caller having to remember to write a marker first.
    pub fn for_domain(domain: &str) -> Self {
        let mut digest = Self::new();
        digest.u32(DIGEST_ALGORITHM_VERSION).str(domain);
        digest
    }

    #[inline]
    pub fn u64(&mut self, word: u64) -> &mut Self {
        self.state ^= word as u128;
        self.state = self.state.wrapping_mul(PRIME);
        self
    }

    #[inline]
    pub fn u32(&mut self, word: u32) -> &mut Self {
        self.u64(word as u64)
    }

    #[inline]
    pub fn i64(&mut self, word: i64) -> &mut Self {
        self.u64(word as u64)
    }

    /// Absorb a length or an index.
    ///
    /// Widened to 64 bits, so a digest taken on a 32-bit target matches one
    /// taken on a 64-bit one.
    #[inline]
    pub fn usize(&mut self, word: usize) -> &mut Self {
        self.u64(word as u64)
    }

    #[inline]
    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.u64(u64::from(value))
    }

    /// Absorb an enum discriminant or a structural marker.
    ///
    /// Separate from [`Digest::u32`] only for readability, but the readability
    /// matters: a tag is what stops two differently-shaped values with the same
    /// field bits digesting identically.
    #[inline]
    pub fn tag(&mut self, tag: u8) -> &mut Self {
        self.u64(tag as u64)
    }

    /// Absorb text, length first.
    ///
    /// The length matters. Without it, `["ab", "c"]` and `["a", "bc"]` digest
    /// identically, and a table of keys is exactly the kind of place that
    /// happens.
    pub fn str(&mut self, text: &str) -> &mut Self {
        self.usize(text.len());
        // Eight bytes at a time, tail padded with zeroes. The length above is
        // what makes the padding unambiguous.
        let bytes = text.as_bytes();
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.u64(u64::from_le_bytes(word));
        }
        self
    }

    /// Absorb an `f32` by its exact bit pattern.
    ///
    /// Negative zero becomes zero, because `-0.0 == 0.0` everywhere else and a
    /// digest that disagreed with `==` would report a difference nothing can
    /// observe. Every NaN becomes one NaN, because a NaN's payload bits are not
    /// a decision anybody made.
    #[inline]
    pub fn f32(&mut self, value: f32) -> &mut Self {
        let bits = if value == 0.0 {
            0
        } else if value.is_nan() {
            0x7fc0_0000
        } else {
            value.to_bits()
        };
        self.u32(bits)
    }

    /// Absorb an `f64` by its exact bit pattern.
    #[inline]
    pub fn f64(&mut self, value: f64) -> &mut Self {
        let bits = if value == 0.0 {
            0
        } else if value.is_nan() {
            0x7ff8_0000_0000_0000
        } else {
            value.to_bits()
        };
        self.u64(bits)
    }

    /// Absorb a real to a fixed number of steps per unit.
    ///
    /// For quantities that fall out of a long chain of transcendental functions,
    /// where the last bit is arithmetic noise rather than an authored value.
    /// A non-finite value absorbs as a distinct marker rather than saturating,
    /// so an infinity cannot digest as a very large number.
    #[inline]
    pub fn quantised(&mut self, value: f64, steps: f64) -> &mut Self {
        if !value.is_finite() {
            return self.tag(0xff).f64(value);
        }
        self.i64((value * steps).round() as i64)
    }

    /// Absorb a sequence, length first.
    pub fn slice<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) -> &mut Self {
        self.usize(items.len());
        for item in items {
            each(self, item);
        }
        self
    }

    /// Absorb another digest's result.
    ///
    /// For building a digest of digests: a document's digest from each of its
    /// sections', a scene's from its ground and its marks.
    pub fn digest(&mut self, other: Fingerprint) -> &mut Self {
        self.u64((other.0 >> 64) as u64).u64(other.0 as u64)
    }

    pub fn finish(&self) -> Fingerprint {
        Fingerprint(self.state)
    }
}

/// A 128-bit digest.
///
/// One value type rather than several, with the *domain* mixed in at
/// construction — see [`Digest::for_domain`]. Distinct newtypes per kind were
/// the alternative and would have meant a conversion at every boundary between
/// a document digest and the scene digest that quotes it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(u128);

impl Fingerprint {
    pub const fn from_u128(bits: u128) -> Self {
        Self(bits)
    }

    pub const fn to_u128(self) -> u128 {
        self.0
    }

    /// The leading hex digits, for a log line or a directory name.
    ///
    /// Eight by default, which is four billion — plenty to tell apart the
    /// handful of things a human is looking at, and short enough to read. The
    /// full value is what a test compares.
    pub fn short(self) -> String {
        format!("{:032x}", self.0)[..8].to_string()
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl std::str::FromStr for Fingerprint {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        u128::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).map(Self)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Fingerprint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Fingerprint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Anything with a canonical digest.
///
/// Implemented rather than derived, deliberately. A derive would absorb whatever
/// fields happen to exist, including the ones that are caches, indices or
/// scratch space — and the whole discipline here is that a digest holds semantic
/// values only. Writing it by hand is a few lines and makes the omissions
/// visible in review.
pub trait Digestible {
    /// Absorb this value's semantic content into `digest`.
    fn absorb(&self, digest: &mut Digest);

    /// This value's digest on its own, in a named domain.
    fn fingerprint(&self, domain: &str) -> Fingerprint {
        let mut digest = Digest::for_domain(domain);
        self.absorb(&mut digest);
        digest.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn of(build: impl FnOnce(&mut Digest)) -> Fingerprint {
        let mut digest = Digest::new();
        build(&mut digest);
        digest.finish()
    }

    #[test]
    fn a_digest_is_order_sensitive() {
        assert_ne!(
            of(|d| {
                d.u64(1).u64(2);
            }),
            of(|d| {
                d.u64(2).u64(1);
            })
        );
    }

    #[test]
    fn text_is_absorbed_length_first() {
        // Without the length, ["ab", "c"] and ["a", "bc"] digest identically —
        // and a table of keys is exactly where that happens.
        assert_ne!(
            of(|d| {
                d.str("ab").str("c");
            }),
            of(|d| {
                d.str("a").str("bc");
            })
        );
        // And the chunk padding does not make a short string collide with a
        // longer one that shares its prefix.
        assert_ne!(
            of(|d| {
                d.str("grass");
            }),
            of(|d| {
                d.str("grass\0\0\0");
            })
        );
    }

    #[test]
    fn text_of_every_length_round_trips_distinctly() {
        // The chunking is where an off-by-one would hide: a bug at the eight
        // byte boundary would collide two keys that differ only past it.
        let mut seen: Vec<Fingerprint> = Vec::new();
        for length in 0..40usize {
            let text = "a".repeat(length);
            let fingerprint = of(|d| {
                d.str(&text);
            });
            assert!(!seen.contains(&fingerprint), "length {length} collided");
            seen.push(fingerprint);
        }
    }

    #[test]
    fn a_domain_separates_identical_content() {
        // A material table and a layer table can hold the same values in the
        // same order and are not the same thing.
        let materials = {
            let mut d = Digest::for_domain("materials");
            d.str("grass_lush");
            d.finish()
        };
        let layers = {
            let mut d = Digest::for_domain("layers");
            d.str("grass_lush");
            d.finish()
        };
        assert_ne!(materials, layers);
    }

    #[test]
    fn negative_zero_digests_as_zero() {
        assert_eq!(
            of(|d| {
                d.f32(0.0);
            }),
            of(|d| {
                d.f32(-0.0);
            })
        );
        assert_eq!(
            of(|d| {
                d.f64(0.0);
            }),
            of(|d| {
                d.f64(-0.0);
            })
        );
    }

    #[test]
    fn every_nan_digests_as_one_nan() {
        let a = f32::from_bits(0x7fc0_0001);
        let b = f32::from_bits(0x7fe0_0000);
        assert!(a.is_nan() && b.is_nan());
        assert_eq!(
            of(|d| {
                d.f32(a);
            }),
            of(|d| {
                d.f32(b);
            })
        );
    }

    #[test]
    fn quantisation_ignores_noise_and_catches_a_real_change() {
        let steps = 1000.0;
        assert_eq!(
            of(|d| {
                d.quantised(0.1, steps);
            }),
            of(|d| {
                d.quantised(0.1 + 1.0e-9, steps);
            })
        );
        assert_ne!(
            of(|d| {
                d.quantised(0.1, steps);
            }),
            of(|d| {
                d.quantised(0.101, steps);
            })
        );
    }

    #[test]
    fn a_non_finite_quantity_does_not_quantise_to_a_large_number() {
        // Saturating would let an infinity digest as some very large value, and
        // a NaN as whatever `as i64` produces — which is zero, the same as a
        // legitimately zero measurement.
        let steps = 1000.0;
        let infinite = of(|d| {
            d.quantised(f64::INFINITY, steps);
        });
        let nan = of(|d| {
            d.quantised(f64::NAN, steps);
        });
        let zero = of(|d| {
            d.quantised(0.0, steps);
        });
        assert_ne!(infinite, zero);
        assert_ne!(nan, zero);
        assert_ne!(infinite, nan);
    }

    #[test]
    fn a_sequence_absorbs_its_own_length() {
        // So that an empty tail cannot be confused with a missing one.
        assert_ne!(
            of(|d| {
                d.slice(&[1u64, 2], |d, v| {
                    d.u64(*v);
                });
            }),
            of(|d| {
                d.slice(&[1u64, 2, 0], |d, v| {
                    d.u64(*v);
                });
            })
        );
    }

    #[test]
    fn a_digest_of_digests_depends_on_all_of_them() {
        let first = of(|d| {
            d.str("a");
        });
        let second = of(|d| {
            d.str("b");
        });
        let combined = of(|d| {
            d.digest(first).digest(second);
        });
        assert_ne!(
            combined,
            of(|d| {
                d.digest(first).digest(first);
            })
        );
        assert_ne!(
            combined,
            of(|d| {
                d.digest(second).digest(first);
            })
        );
    }

    #[test]
    fn a_fingerprint_round_trips_through_text() {
        let fingerprint = of(|d| {
            d.str("grass_lush").u32(7);
        });
        assert_eq!(fingerprint.to_string().len(), 32);
        assert_eq!(
            Fingerprint::from_str(&fingerprint.to_string()),
            Ok(fingerprint)
        );
        assert_eq!(fingerprint.short().len(), 8);
        assert!(fingerprint.to_string().starts_with(&fingerprint.short()));
    }

    #[test]
    fn the_digestible_trait_produces_the_same_value_as_a_manual_digest() {
        struct Material {
            key: &'static str,
            weight: f64,
        }
        impl Digestible for Material {
            fn absorb(&self, digest: &mut Digest) {
                digest.str(self.key).f64(self.weight);
            }
        }
        let material = Material {
            key: "grass_lush",
            weight: 0.75,
        };
        let mut manual = Digest::for_domain("material");
        material.absorb(&mut manual);
        assert_eq!(material.fingerprint("material"), manual.finish());
        // And the domain reaches the result.
        assert_ne!(
            material.fingerprint("material"),
            material.fingerprint("layer")
        );
    }

    /// Pinned. See [`DIGEST_ALGORITHM_VERSION`].
    ///
    /// Moving these invalidates cached comparisons and nothing else — which is
    /// exactly why they are pinned separately from the seed vectors, where the
    /// consequence of a change is the whole world moving.
    #[test]
    fn the_digest_is_pinned() {
        assert_eq!(DIGEST_ALGORITHM_VERSION, 1);
        assert_eq!(Digest::new().finish().to_u128(), OFFSET_BASIS);
        assert_eq!(
            of(|d| {
                d.u64(0);
            })
            .to_string(),
            "d228cb69101a8caf78912b704e4a147f"
        );
        assert_eq!(
            of(|d| {
                d.str("grass_lush");
            })
            .to_string(),
            "3e6a679bb78ab6c582db3b685083c71f"
        );
    }
}

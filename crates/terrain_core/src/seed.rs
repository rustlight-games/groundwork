//! Where randomness comes from, and why it is not a stream.
//!
//! ## Addressed, not sequential
//!
//! The instinctive way to place ten thousand blades of grass is to seed a
//! generator and pull ten thousand numbers out of it. Everything about that is
//! wrong here, and the reason is worth stating once at length because every rule
//! in this module follows from it.
//!
//! A sequential generator makes each draw depend on **how many draws came
//! before it**. So the third blade in a tuft is only where it is because the
//! first two asked for the number of values they happened to ask for. Add a
//! parameter to the second blade and the third moves. Skip a candidate that
//! failed a density test and everything after it shifts. Bake the same ground as
//! part of a different page — with a different number of neighbours inside the
//! rectangle — and it is a different meadow.
//!
//! That last one is fatal rather than merely annoying. This framework's central
//! promise is that terrain is a **continuous function of world position**: two
//! pages that have never met must agree along their shared edge, a tile must be
//! croppable out of a larger bake, and a training crop must be the same ground
//! as the render it was cut from. None of that survives a sequential generator.
//!
//! So a random value here is **addressed**. You do not ask for "the next
//! number"; you ask for "the value at this address", and the address is built
//! from things that are properties of the *thing being decided* rather than of
//! the order it was decided in:
//!
//! ```text
//! seed algorithm version   this module's own version, so a fix is visible
//! root seed                which world
//! recipe version           which generation of the recipe
//! population key           which population, by its authored name
//! integer world cell       where, on a lattice that knows nothing about pixels
//! candidate rank           which candidate within that cell
//! named stream             which decision about that candidate
//! child path               optional, for a candidate's own sub-parts
//! ```
//!
//! Every one of those is knowable without having generated anything else. That
//! is the property, and it is the only property that matters.
//!
//! ## Named streams, not positional ones
//!
//! A candidate needs a length, a lean, a colour and a dozen other things. Each
//! comes from its own **named** stream — `"length"`, `"lean"` — rather than
//! from consecutive draws.
//!
//! The cost is a hash per draw. What it buys is that adding a decision does not
//! move the existing ones. With positional draws, inserting a new `sway` between
//! `bend` and `twist` shifts every subsequent parameter of every mark in the
//! world, and the diff that did it is one line long.
//!
//! ## Two hashes, and they must stay apart
//!
//! This module's hash decides **where things go**. [`crate::digest`]'s hash
//! decides **whether two documents are the same**. They look similar and they
//! are used for completely different things, and merging them would mean that
//! improving the content digest — a maintenance change, made for good reasons,
//! in a module that has nothing to do with vegetation — silently relocates every
//! plant in every world.
//!
//! So they are separate implementations, in separate modules, with separate
//! version constants. [`SEED_ALGORITHM_VERSION`] is the one that must not move
//! without intent.

use std::fmt;

use crate::coords::CellCoord;
use crate::ids::{PopulationKey, StreamKey};

/// The version of the derivation in this module.
///
/// Changing anything below — the mixer, the order fields are absorbed in, the
/// key hash — changes where everything in every world is, and this constant is
/// how that becomes visible rather than mysterious. Bump it in the same commit,
/// and expect every pinned fixture in the repository to need re-accepting.
pub const SEED_ALGORITHM_VERSION: u32 = 1;

/// Which world.
///
/// Authored as sixteen hex digits, because a seed is copied between a document,
/// a command line and a bug report, and a decimal `u64` is a number nobody can
/// check they typed correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct RootSeed(u64);

impl RootSeed {
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RootSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl std::str::FromStr for RootSeed {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let text = text.strip_prefix("0x").unwrap_or(text);
        // Underscores are allowed so a seed can be written in readable groups.
        let cleaned: String = text.chars().filter(|c| *c != '_').collect();
        u64::from_str_radix(&cleaned, 16).map(Self)
    }
}

impl TryFrom<String> for RootSeed {
    type Error = std::num::ParseIntError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<RootSeed> for String {
    fn from(seed: RootSeed) -> String {
        seed.to_string()
    }
}

/// A population's name, reduced to a number once.
///
/// Carried on a [`CandidateId`] rather than the key itself so an id stays
/// `Copy` and cheap — a scatter builds millions of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct PopulationHash(u64);

impl PopulationHash {
    pub fn of(key: &PopulationKey) -> Self {
        Self(key.seed_hash())
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }
}

/// Which candidate.
///
/// The whole identity of one potential piece of scattered content, knowable
/// before anything about it has been decided — which is what lets a candidate be
/// generated, rejected, and generated identically again by a different page.
///
/// `rank` is the candidate's position within its cell, not within the world. It
/// counts from zero in every cell, so no cell's numbering depends on any other
/// cell's population.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CandidateId {
    pub population: PopulationHash,
    pub cell: CellCoord,
    pub rank: u16,
}

impl CandidateId {
    pub const fn new(population: PopulationHash, cell: CellCoord, rank: u16) -> Self {
        Self {
            population,
            cell,
            rank,
        }
    }
}

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:016x}{}#{}",
            self.population.bits(),
            self.cell,
            self.rank
        )
    }
}

/// A candidate, and which decision about it is being made.
#[derive(Clone, Copy, Debug)]
pub struct RandomAddress<'a> {
    pub candidate: CandidateId,
    pub stream: &'a StreamKey,
}

impl<'a> RandomAddress<'a> {
    pub const fn new(candidate: CandidateId, stream: &'a StreamKey) -> Self {
        Self { candidate, stream }
    }
}

/// A world's randomness, ready to be addressed.
///
/// Holds only what every draw shares — the world and the recipe generation — so
/// that the rest of an address can be built at the call site from things the
/// caller already knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedContext {
    root: RootSeed,
    recipe_version: u32,
}

impl SeedContext {
    pub const fn new(root: RootSeed, recipe_version: u32) -> Self {
        Self {
            root,
            recipe_version,
        }
    }

    pub const fn root(self) -> RootSeed {
        self.root
    }

    pub const fn recipe_version(self) -> u32 {
        self.recipe_version
    }

    /// A recipe generation of its own, sharing the world.
    ///
    /// Two recipes in one document must not draw the same numbers for the same
    /// cell, and this is how a recipe declares which generation of itself is
    /// asking.
    pub const fn for_recipe(self, recipe_version: u32) -> Self {
        Self {
            root: self.root,
            recipe_version,
        }
    }

    /// The raw 64 bits at an address.
    pub fn bits(self, address: &RandomAddress<'_>) -> u64 {
        self.bits_with_path(address, &[])
    }

    /// The raw bits at an address, further qualified by a child path.
    ///
    /// For a candidate's own sub-parts: the third blade of this tuft, the second
    /// petal of that flower. A path rather than another rank because sub-parts
    /// nest, and because a path keeps a tuft's blades addressable without the
    /// tuft having to be generated first.
    pub fn bits_with_path(self, address: &RandomAddress<'_>, path: &[u32]) -> u64 {
        let candidate = address.candidate;
        // The order is part of the contract and must not be rearranged. Each
        // value is mixed on the way in, so two fields cannot cancel.
        let mut state = mix(SEED_ALGORITHM_VERSION as u64);
        state = mix(state ^ self.root.bits());
        state = mix(state ^ self.recipe_version as u64);
        state = mix(state ^ candidate.population.bits());
        state = mix(state ^ candidate.cell.x as u64);
        state = mix(state ^ candidate.cell.y as u64);
        state = mix(state ^ candidate.rank as u64);
        state = mix(state ^ key_hash(address.stream.as_str()));
        for step in path {
            state = mix(state ^ *step as u64);
        }
        state
    }

    /// A value in `[0, 1)`.
    ///
    /// The top 53 bits, which is exactly an `f64`'s mantissa: every result is
    /// representable, the distribution is uniform, and no value can come out as
    /// `1.0` through rounding. Taking the *low* bits instead is the classic
    /// mistake — they are the least mixed part of most integer hashes.
    pub fn unit(self, address: &RandomAddress<'_>) -> f64 {
        unit_from_bits(self.bits(address))
    }

    /// A value in `[low, high)`.
    pub fn range(self, address: &RandomAddress<'_>, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit(address)
    }

    /// A value in `[0, 1)` for a sub-part.
    pub fn unit_with_path(self, address: &RandomAddress<'_>, path: &[u32]) -> f64 {
        unit_from_bits(self.bits_with_path(address, path))
    }

    /// True with probability `chance`.
    ///
    /// Clamped rather than asserted, because a probability computed from an
    /// authored modifier can legitimately arrive slightly outside the range and
    /// panicking there would be a poor trade.
    pub fn chance(self, address: &RandomAddress<'_>, chance: f64) -> bool {
        self.unit(address) < chance.clamp(0.0, 1.0)
    }

    /// An index in `0..count`, or `None` for an empty range.
    pub fn index(self, address: &RandomAddress<'_>, count: usize) -> Option<usize> {
        (count > 0).then(|| ((self.unit(address) * count as f64) as usize).min(count - 1))
    }
}

/// Bits to a `[0, 1)` real.
#[inline]
pub fn unit_from_bits(bits: u64) -> f64 {
    // 2^-53, applied to the top 53 bits.
    (bits >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// SplitMix64's finaliser.
///
/// Pinned. It is fast, it is well studied, and every bit of its output depends
/// on every bit of its input — which is what an addressed scheme needs, because
/// neighbouring cells differ in one low bit and must produce unrelated values.
/// A weaker mixer shows up as visible structure in a scatter: rows, diagonals,
/// or clumps that move when the world seed changes but never go away.
#[inline]
pub fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The stable 64-bit hash of a textual key, for seeding.
///
/// FNV-1a, pinned, and **separate from the content digest on purpose**. See the
/// module note: merging the two would mean that a change to how documents are
/// digested relocates every plant in every world.
#[inline]
pub fn key_hash(text: &str) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    // Finalised, so that keys differing only in their last byte — which FNV
    // separates only in its low bits — are separated everywhere.
    mix(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> SeedContext {
        SeedContext::new(RootSeed::new(0x8df7_82f9_5ce1_a4d4), 1)
    }

    fn candidate(x: i64, y: i64, rank: u16) -> CandidateId {
        CandidateId::new(
            PopulationHash::of(&PopulationKey::new("grass_population").expect("valid")),
            CellCoord::new(x, y),
            rank,
        )
    }

    fn stream(name: &str) -> StreamKey {
        StreamKey::new(name).expect("valid")
    }

    #[test]
    fn the_same_address_always_gives_the_same_value() {
        // The whole promise. Two pages that have never met derive the same
        // number for the same candidate, which is what makes a shared edge
        // agree.
        let context = context();
        let length = stream("length");
        let address = RandomAddress::new(candidate(3, -7, 2), &length);
        assert_eq!(context.bits(&address), context.bits(&address));
        assert_eq!(context.unit(&address), context.unit(&address));
    }

    #[test]
    fn every_field_of_an_address_changes_the_value() {
        // A field that did not reach the mixer would make two different things
        // share a number, and the symptom is a visible repeat rather than an
        // error.
        let context = context();
        let length = stream("length");
        let base = context.bits(&RandomAddress::new(candidate(3, -7, 2), &length));

        assert_ne!(
            base,
            context.bits(&RandomAddress::new(candidate(4, -7, 2), &length))
        );
        assert_ne!(
            base,
            context.bits(&RandomAddress::new(candidate(3, -6, 2), &length))
        );
        assert_ne!(
            base,
            context.bits(&RandomAddress::new(candidate(3, -7, 3), &length))
        );

        let lean = stream("lean");
        assert_ne!(
            base,
            context.bits(&RandomAddress::new(candidate(3, -7, 2), &lean))
        );

        let other_population = CandidateId::new(
            PopulationHash::of(&PopulationKey::new("meadow_flowers").expect("valid")),
            CellCoord::new(3, -7),
            2,
        );
        assert_ne!(
            base,
            context.bits(&RandomAddress::new(other_population, &length))
        );

        let other_world = SeedContext::new(RootSeed::new(1), 1);
        assert_ne!(
            base,
            other_world.bits(&RandomAddress::new(candidate(3, -7, 2), &length))
        );

        let other_recipe = context.for_recipe(2);
        assert_ne!(
            base,
            other_recipe.bits(&RandomAddress::new(candidate(3, -7, 2), &length))
        );
    }

    #[test]
    fn a_new_stream_does_not_move_the_existing_ones() {
        // The property named streams buy, and the reason they are worth a hash
        // per draw. With positional draws, inserting a decision shifts every
        // parameter after it on every mark in the world.
        let context = context();
        let candidate = candidate(11, 4, 0);
        let before: Vec<f64> = ["length", "lean", "twist"]
            .iter()
            .map(|name| context.unit(&RandomAddress::new(candidate, &stream(name))))
            .collect();

        // A `sway` stream is added between `lean` and `twist`.
        let after: Vec<f64> = ["length", "lean", "twist"]
            .iter()
            .map(|name| context.unit(&RandomAddress::new(candidate, &stream(name))))
            .collect();
        let _sway = context.unit(&RandomAddress::new(candidate, &stream("sway")));
        assert_eq!(before, after);
    }

    #[test]
    fn a_child_path_addresses_a_sub_part_without_disturbing_its_parent() {
        let context = context();
        let length = stream("length");
        let address = RandomAddress::new(candidate(0, 0, 0), &length);
        let parent = context.bits(&address);
        let first = context.bits_with_path(&address, &[0]);
        let second = context.bits_with_path(&address, &[1]);
        let nested = context.bits_with_path(&address, &[0, 0]);

        assert_ne!(parent, first);
        assert_ne!(first, second);
        assert_ne!(first, nested);
        assert_eq!(parent, context.bits(&address), "the parent moved");
        // An empty path is the parent itself.
        assert_eq!(parent, context.bits_with_path(&address, &[]));
    }

    #[test]
    fn neighbouring_cells_are_uncorrelated() {
        // Cells differ in one low bit. A weak mixer leaves visible rows and
        // diagonals in a scatter, and no amount of downstream jitter removes
        // them.
        let context = context();
        let length = stream("length");
        let values: Vec<f64> = (0..64)
            .map(|x| context.unit(&RandomAddress::new(candidate(x, 0, 0), &length)))
            .collect();

        let mean = values.iter().sum::<f64>() / values.len() as f64;
        assert!((mean - 0.5).abs() < 0.12, "mean {mean}");

        // Lag-one autocorrelation. Structure along a row would show here first.
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>();
        let covariance: f64 = values
            .windows(2)
            .map(|w| (w[0] - mean) * (w[1] - mean))
            .sum();
        let correlation = covariance / variance;
        assert!(
            correlation.abs() < 0.3,
            "adjacent cells correlate at {correlation}"
        );
    }

    #[test]
    fn values_are_uniform_and_never_reach_one() {
        // Ten equal buckets over ten thousand draws. A `1.0` would index one
        // past the end of every table this feeds.
        let context = context();
        let length = stream("length");
        let mut buckets = [0usize; 10];
        for rank in 0..10_000u16 {
            let value = context.unit(&RandomAddress::new(candidate(0, 0, rank), &length));
            assert!((0.0..1.0).contains(&value), "{value} is outside [0, 1)");
            buckets[(value * 10.0) as usize] += 1;
        }
        for (bucket, count) in buckets.iter().enumerate() {
            assert!(
                (700..1300).contains(count),
                "bucket {bucket} holds {count} of 10000"
            );
        }
    }

    #[test]
    fn an_index_stays_inside_its_range() {
        let context = context();
        let pick = stream("pick");
        for rank in 0..2000u16 {
            let address = RandomAddress::new(candidate(1, 1, rank), &pick);
            assert!(context.index(&address, 4).expect("in range") < 4);
        }
        assert_eq!(
            context.index(&RandomAddress::new(candidate(0, 0, 0), &pick), 0),
            None
        );
    }

    #[test]
    fn a_root_seed_round_trips_through_hex() {
        let seed = RootSeed::new(0x8df7_82f9_5ce1_a4d4);
        assert_eq!(seed.to_string(), "8df782f95ce1a4d4");
        assert_eq!("8df782f95ce1a4d4".parse(), Ok(seed));
        assert_eq!("0x8df782f95ce1a4d4".parse(), Ok(seed));
        assert_eq!("8df7_82f9_5ce1_a4d4".parse(), Ok(seed));
        assert_eq!(" 8DF782F95CE1A4D4 ".parse::<RootSeed>(), Ok(seed));
        assert!("not a seed".parse::<RootSeed>().is_err());
    }

    /// Pinned vectors.
    ///
    /// These are the numbers, written down. They exist so that a change to the
    /// derivation cannot pass review as an accident: every one of them moving is
    /// every plant in every world moving, and the only acceptable way for that
    /// to happen is deliberately, with `SEED_ALGORITHM_VERSION` bumped in the
    /// same commit.
    #[test]
    fn the_derivation_is_pinned() {
        assert_eq!(SEED_ALGORITHM_VERSION, 1);
        assert_eq!(mix(0), 0xe220_a839_7b1d_cdaf);
        assert_eq!(mix(1), 0x910a_2dec_8902_5cc1);
        assert_eq!(key_hash(""), mix(0xcbf2_9ce4_8422_2325));
        assert_eq!(key_hash("length"), 0xbad4_7f7d_3d48_7416);
        assert_eq!(key_hash("grass_population"), 0x9763_3746_92b0_1432);

        let context = SeedContext::new(RootSeed::new(0x8df7_82f9_5ce1_a4d4), 1);
        let length = StreamKey::new("length").expect("valid");
        let address = RandomAddress::new(
            CandidateId::new(
                PopulationHash::of(&PopulationKey::new("grass_population").expect("valid")),
                CellCoord::new(3, -7),
                2,
            ),
            &length,
        );
        assert_eq!(context.bits(&address), 0x8f83_646f_dc7f_e571);
    }
}

//! Stable hashing of simulation state.
//!
//! This is the instrument the determinism tests read. Hash the world after N
//! ticks, compare against a recorded value, and any accidental non-determinism
//! — a `HashMap` iteration, a stray float, an unsorted effect queue — shows up
//! as a failing test instead of as a training run that quietly refuses to
//! converge.
//!
//! Deliberately not `std::hash::Hash`: `DefaultHasher` is explicitly not
//! guaranteed stable across releases, and `Hash` implementations for
//! collections depend on iteration order. This is a small, fixed FNV-1a with no
//! such freedom.

/// FNV-1a, 64-bit.
#[derive(Clone, Copy, Debug)]
pub struct StableHasher {
    state: u64,
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    pub const fn new() -> Self {
        Self {
            state: FNV_OFFSET_BASIS,
        }
    }

    pub fn write_u8(&mut self, v: u8) {
        self.state ^= v as u64;
        self.state = self.state.wrapping_mul(FNV_PRIME);
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u8(b);
        }
    }

    pub fn write_u32(&mut self, v: u32) {
        self.write_bytes(&v.to_le_bytes());
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.write_bytes(&v.to_le_bytes());
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_u8(v as u8);
    }

    pub fn write_str(&mut self, v: &str) {
        self.write_u64(v.len() as u64);
        self.write_bytes(v.as_bytes());
    }

    pub fn finish(&self) -> u64 {
        self.state
    }
}

/// Contribute a value to a [`StableHasher`].
///
/// Implementations must feed a length before any variable-length sequence, so
/// that `[a] + [b, c]` cannot hash the same as `[a, b] + [c]`.
pub trait StableHash {
    fn stable_hash(&self, hasher: &mut StableHasher);

    /// Convenience for hashing a single value on its own.
    fn stable_hash_value(&self) -> u64
    where
        Self: Sized,
    {
        let mut h = StableHasher::new();
        self.stable_hash(&mut h);
        h.finish()
    }
}

impl StableHash for u8 {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_u8(*self);
    }
}

impl StableHash for u32 {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_u32(*self);
    }
}

impl StableHash for u64 {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_u64(*self);
    }
}

impl StableHash for i32 {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_i64(*self as i64);
    }
}

impl StableHash for i64 {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_i64(*self);
    }
}

impl StableHash for bool {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_bool(*self);
    }
}

impl StableHash for str {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_str(self);
    }
}

impl StableHash for crate::fx::Vec2Fx {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_i64(self.x.to_bits());
        h.write_i64(self.y.to_bits());
    }
}

impl<T: StableHash> StableHash for [T] {
    fn stable_hash(&self, h: &mut StableHasher) {
        h.write_u64(self.len() as u64);
        for item in self {
            item.stable_hash(h);
        }
    }
}

impl<T: StableHash> StableHash for Vec<T> {
    fn stable_hash(&self, h: &mut StableHasher) {
        self.as_slice().stable_hash(h);
    }
}

impl<T: StableHash> StableHash for Option<T> {
    fn stable_hash(&self, h: &mut StableHasher) {
        match self {
            None => h.write_u8(0),
            Some(v) => {
                h.write_u8(1);
                v.stable_hash(h);
            }
        }
    }
}

/// Hash a [`Real`](crate::fx::Real) by its exact bit pattern.
///
/// A free function rather than a trait impl, because `Real` is a type alias to
/// a foreign type and the orphan rule forbids implementing our trait for it.
pub fn hash_real(h: &mut StableHasher, v: crate::fx::Real) {
    h.write_i64(v.to_bits());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::{Vec2Fx, real_from_int};

    #[test]
    fn same_input_same_hash() {
        assert_eq!(42u64.stable_hash_value(), 42u64.stable_hash_value());
    }

    #[test]
    fn different_input_different_hash() {
        assert_ne!(42u64.stable_hash_value(), 43u64.stable_hash_value());
    }

    #[test]
    fn length_prefix_prevents_sequence_ambiguity() {
        let split_one: Vec<Vec<u8>> = vec![vec![1], vec![2, 3]];
        let split_two: Vec<Vec<u8>> = vec![vec![1, 2], vec![3]];
        assert_ne!(split_one.stable_hash_value(), split_two.stable_hash_value());
    }

    #[test]
    fn order_is_significant() {
        assert_ne!(
            vec![1u8, 2].stable_hash_value(),
            vec![2u8, 1].stable_hash_value()
        );
    }

    #[test]
    fn option_none_differs_from_zero() {
        assert_ne!(
            None::<u8>.stable_hash_value(),
            Some(0u8).stable_hash_value()
        );
    }

    #[test]
    fn vectors_hash_by_exact_bits() {
        let a = Vec2Fx::from_ints(3, 4);
        let b = Vec2Fx::new(real_from_int(3), real_from_int(4));
        assert_eq!(a.stable_hash_value(), b.stable_hash_value());
        assert_ne!(
            a.stable_hash_value(),
            Vec2Fx::from_ints(4, 3).stable_hash_value()
        );
    }

    #[test]
    fn hash_scheme_is_pinned() {
        // Pinned to a literal so that changing the hashing scheme is a
        // deliberate, visible edit rather than a silent invalidation of every
        // golden determinism file in the repo.
        let mut h = StableHasher::new();
        h.write_str("backseat");
        h.write_u64(7);
        assert_eq!(
            h.finish(),
            PINNED_HASH,
            "hashing scheme changed; regenerate golden files"
        );
    }

    const PINNED_HASH: u64 = 2_379_420_642_405_762_200;
}

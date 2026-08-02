//! Deterministic randomness via seed splitting.
//!
//! The obvious design — one `Rng` in a resource that systems draw from — is not
//! usable here. Bevy runs systems in parallel, so the order draws happen in is
//! not fixed, and a shared generator would hand different values to different
//! units between runs.
//!
//! Instead nothing holds mutable generator state. [`SimRng`] holds only a root
//! seed, and each call site derives its own independent stream from
//! `(root, stream, tick, salt)`. Two systems drawing in either order get the
//! same answers, because neither one's draw depends on the other having
//! happened. The cost is constructing a generator per call site per tick, which
//! is a few nanoseconds and worth it.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::tick::Tick;

/// SplitMix64 — the standard finaliser for turning a counter into a
/// well-distributed seed. Deterministic, no allocation, `const`.
pub const fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Which subsystem is drawing.
///
/// Separating streams means adding a random draw in one system cannot shift the
/// values another system sees. Without this, adding a crit roll would silently
/// change every targeting decision in the game and invalidate trained policies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u64)]
pub enum RngStream {
    Damage = 1,
    Crit = 2,
    Targeting = 3,
    Spawn = 4,
    AbilitySelection = 5,
    Movement = 6,
    TerrainGen = 7,
    RockGen = 8,
    AiExploration = 9,
    Misc = 10,
}

/// Root seed for one battle.
///
/// Cheap to copy and holds no mutable state, so it can live in a resource that
/// parallel systems read simultaneously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimRng {
    root: u64,
}

impl SimRng {
    pub const fn new(seed: u64) -> Self {
        Self { root: seed }
    }

    pub const fn root(&self) -> u64 {
        self.root
    }

    /// An independent generator for this `(stream, tick, salt)`.
    ///
    /// `salt` distinguishes call sites within one stream and tick — pass the
    /// acting unit's id so two units rolling damage on the same tick get
    /// different numbers.
    pub fn stream(&self, stream: RngStream, tick: Tick, salt: u64) -> ChaCha8Rng {
        let mut seed = [0u8; 32];
        let mut acc = self.root;
        for (chunk, mixin) in seed
            .chunks_exact_mut(8)
            .zip([stream as u64, tick.0, salt, self.root])
        {
            acc = splitmix64(acc ^ splitmix64(mixin));
            chunk.copy_from_slice(&acc.to_le_bytes());
        }
        ChaCha8Rng::from_seed(seed)
    }

    /// A generator not tied to a tick, for one-off procedural generation such
    /// as building a terrain map or a rock sprite.
    pub fn generator(&self, stream: RngStream, salt: u64) -> ChaCha8Rng {
        self.stream(stream, Tick(0), salt)
    }
}

#[cfg(test)]
mod tests {
    use rand::RngCore;

    use super::*;

    fn first_u64(rng: &mut ChaCha8Rng) -> u64 {
        rng.next_u64()
    }

    #[test]
    fn same_inputs_give_same_stream() {
        let a = SimRng::new(42);
        let b = SimRng::new(42);
        assert_eq!(
            first_u64(&mut a.stream(RngStream::Damage, Tick(7), 3)),
            first_u64(&mut b.stream(RngStream::Damage, Tick(7), 3)),
        );
    }

    #[test]
    fn streams_are_independent() {
        let r = SimRng::new(42);
        let damage = first_u64(&mut r.stream(RngStream::Damage, Tick(7), 3));
        let crit = first_u64(&mut r.stream(RngStream::Crit, Tick(7), 3));
        assert_ne!(damage, crit, "stream discriminant must affect the seed");
    }

    #[test]
    fn tick_and_salt_both_affect_the_stream() {
        let r = SimRng::new(42);
        let base = first_u64(&mut r.stream(RngStream::Damage, Tick(7), 3));
        assert_ne!(
            base,
            first_u64(&mut r.stream(RngStream::Damage, Tick(8), 3))
        );
        assert_ne!(
            base,
            first_u64(&mut r.stream(RngStream::Damage, Tick(7), 4))
        );
    }

    #[test]
    fn different_roots_diverge() {
        let a = SimRng::new(1);
        let b = SimRng::new(2);
        assert_ne!(
            first_u64(&mut a.stream(RngStream::Damage, Tick(0), 0)),
            first_u64(&mut b.stream(RngStream::Damage, Tick(0), 0)),
        );
    }

    #[test]
    fn draw_order_does_not_matter() {
        // The property that makes parallel systems safe: drawing for unit B
        // before unit A gives both the same values as the other order.
        let r = SimRng::new(99);
        let forward: Vec<u64> = (0..8)
            .map(|i| first_u64(&mut r.stream(RngStream::Targeting, Tick(5), i)))
            .collect();
        let mut backward: Vec<u64> = (0..8)
            .rev()
            .map(|i| first_u64(&mut r.stream(RngStream::Targeting, Tick(5), i)))
            .collect();
        backward.reverse();
        assert_eq!(forward, backward);
    }
}

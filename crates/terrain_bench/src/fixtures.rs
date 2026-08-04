//! Fixed inputs.
//!
//! Every benchmark draws its seeds from here. Two measurements are only
//! comparable if they ran against the same input, and "the same input" has to
//! mean something more durable than "whatever the RNG produced that day".
//!
//! ## What used to be here
//!
//! Three named battle scenarios — 32×32 with 8 units a side, 128×128 with 40,
//! 512×512 with 200 — and a fixed-point grid to run them on. They are gone with
//! the simulation they measured. The terrain fixtures that replace them are a
//! different shape entirely: a page, a grid of pages, a view, a blend boundary,
//! a path junction. They arrive with `terrain_bench`, and the reason they are
//! not here yet is that a fixture is only worth pinning once the thing it
//! measures has settled — a scenario named now and redefined in three commits is
//! worse than no scenario, because a benchmark history spanning the redefinition
//! reads as a regression.
//!
//! [`SEEDS`] survives the change untouched, and must.

/// The canonical seeds.
///
/// Ten, because a single seed can flatter or punish a generator by luck, and
/// ten samples is enough to see a real regression through the noise without
/// making the suite slow. Never reorder or extend in the middle — a benchmark
/// history is only meaningful if seed *n* means the same thing it did last
/// month. Append only.
pub const SEEDS: [u64; 10] = [
    0x0000_0001,
    0x5EED_1234,
    0xDEAD_BEEF,
    0x1357_9BDF,
    0x2468_ACE0,
    0xFACE_0FF1,
    0xC0FF_EE00,
    0xBADD_CAFE,
    0x0BAD_F00D,
    0xFEED_FACE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_distinct() {
        // A duplicate would silently halve the sample size.
        let mut seen = SEEDS.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before);
    }
}

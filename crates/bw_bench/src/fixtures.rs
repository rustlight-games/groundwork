//! Fixed inputs.
//!
//! Every benchmark draws its seeds from here. Two measurements are only
//! comparable if they ran against the same input, and "the same input" has to
//! mean something more durable than "whatever the RNG produced that day".

use bw_core::{Grid, GridDims, real_from_int};

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

/// A standard battlefield size.
///
/// Three sizes rather than one: performance characteristics change shape with
/// scale, and a regression that only shows up on a large map is exactly the one
/// worth catching before it reaches a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// A skirmish. Fast enough to run on every commit.
    Small,
    /// A typical battle. The default for tracking.
    Medium,
    /// A stress case, for finding the cliff before players do.
    Large,
}

impl Scenario {
    pub const ALL: [Scenario; 3] = [Scenario::Small, Scenario::Medium, Scenario::Large];

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Small => "small",
            Scenario::Medium => "medium",
            Scenario::Large => "large",
        }
    }

    /// Battlefield dimensions in cells.
    pub fn grid_dims(self) -> GridDims {
        match self {
            Scenario::Small => GridDims::new(32, 32),
            Scenario::Medium => GridDims::new(128, 128),
            Scenario::Large => GridDims::new(512, 512),
        }
    }

    /// Units per side.
    pub fn units_per_team(self) -> usize {
        match self {
            Scenario::Small => 8,
            Scenario::Medium => 40,
            Scenario::Large => 200,
        }
    }

    /// How many ticks a throughput benchmark should run.
    pub fn ticks(self) -> u64 {
        match self {
            Scenario::Small => 600,
            Scenario::Medium => 600,
            Scenario::Large => 300,
        }
    }

    /// A grid centred on the origin at one world unit per cell.
    pub fn grid(self) -> Grid {
        Grid::centered(self.grid_dims(), real_from_int(1))
    }
}

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

    #[test]
    fn scenarios_increase_in_size() {
        let sizes: Vec<usize> = Scenario::ALL
            .iter()
            .map(|s| s.grid_dims().cell_count())
            .collect();
        assert!(sizes.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn scenario_names_are_unique() {
        let mut names: Vec<_> = Scenario::ALL.iter().map(|s| s.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
    }
}

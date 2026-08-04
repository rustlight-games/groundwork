//! The ground every measurement is taken on.
//!
//! Two measurements are comparable only if they ran against the same input, and
//! "the same input" has to mean something more durable than "whatever the
//! generator produced that day". These are that something.
//!
//! ## Append only, never reorder, never edit
//!
//! The same rule [`crate::fixtures::SEEDS`] obeys, and for the same reason: a
//! benchmark history means something only if scenario three is the same patch of
//! ground it was last month. Editing one silently makes every measurement taken
//! before it incomparable with every measurement taken after, and nothing
//! reports that — the numbers simply stop meaning what they used to.
//!
//! ## Why these particular ones
//!
//! Each buys something the others cannot, and several are here because they are
//! where a specific class of bug lives rather than because they are typical:
//!
//! - **`page.constant_grass`** is the base case. If this moves, everything has.
//! - **`page.diagonal_path`** puts a boundary across a page at an angle, which
//!   is where axis-aligned assumptions show.
//! - **`page.one_texel_mask`** is a feature narrower than the sampling rate. It
//!   is the case a filter either handles or aliases, and it is invisible in
//!   anything larger.
//! - **`page.edge_transition`** puts the boundary *on* a page edge, so a seam
//!   and a material transition coincide.
//! - **`page.external_root_mark`** is content rooted outside the region that
//!   reaches into it — the halo's whole reason for existing.
//! - **`grid.four_page_junction`** is the corner where four independently baked
//!   pages meet, which is the hardest place for them to agree.
//! - **`grid.worst_grass_density`** and **`grid.worst_rock_density`** are the
//!   expensive cases. A benchmark run only on typical ground reports a mean and
//!   misses the cliff.
//! - **`view.reference_close`** and **`view.reference_rts`** are the two
//!   framings the look is judged at, and they are far enough apart that an
//!   optimisation can be nearly free at one and obvious at the other.
//!
//! Two more are named in the design and not yet pinned: `path.t_junction` and
//! `path.x_junction`, where a boundary meets itself and a distance field's
//! nearest-segment answer becomes ambiguous. They wait on a spline asset that
//! actually branches — pinning a scenario against ground that does not have the
//! feature it is named for would be worse than not having it, because the row
//! would look like coverage.

use terrain_core::coords::{WorldPoint, WorldRect};

/// One named patch of ground and how to look at it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scenario {
    /// The stable, dotted name a measurement is recorded under.
    pub name: &'static str,
    /// Which document to use.
    pub document: Document,
    /// Where to look.
    pub centre: WorldPoint,
    /// How much ground, in metres.
    pub side_m: f64,
    /// Texels per metre.
    pub texels_per_metre: f32,
    /// Which seed, as an index into [`crate::fixtures::SEEDS`].
    pub seed: usize,
}

/// Which committed document a scenario runs against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Document {
    /// `constant_grass.terrain.ron` — one material, everywhere.
    ConstantGrass,
    /// `blend_lab.terrain.ron` — a path through a meadow.
    BlendLab,
}

impl Document {
    /// The document's filename, relative to `assets/terrain/documents`.
    pub const fn file(self) -> &'static str {
        match self {
            Self::ConstantGrass => "constant_grass.terrain.ron",
            Self::BlendLab => "blend_lab.terrain.ron",
        }
    }
}

impl Scenario {
    pub fn bounds(self) -> WorldRect {
        WorldRect::centred(self.centre, self.side_m)
    }

    /// The output size this scenario bakes at.
    pub fn size(self) -> [u32; 2] {
        let side = (self.side_m * self.texels_per_metre as f64)
            .round()
            .max(1.0) as u32;
        [side, side]
    }
}

/// The authoring scale the art is drawn at, in texels per metre.
pub const AUTHORING_TEXELS_PER_METRE: f32 = 96.0;

/// World metres visible vertically at the close framing the look was tuned at.
pub const CLOSE_VIEW_M: f64 = 13.0;

/// World metres visible vertically at the framing a strategy camera uses.
///
/// Four times the close view, which is sixteen times the ground and about a
/// fifth of the texel density. Every level-of-detail decision has to hold here,
/// and it is the framing most likely to be skipped because it is slow.
pub const RTS_VIEW_M: f64 = 55.0;

/// The pinned scenarios. **Append only.**
pub const SCENARIOS: [Scenario; 12] = [
    Scenario {
        name: "page.constant_grass",
        document: Document::ConstantGrass,
        centre: WorldPoint::new(0.0, 0.0),
        side_m: 4.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 0,
    },
    Scenario {
        name: "page.diagonal_path",
        document: Document::BlendLab,
        // Where the path runs at an angle rather than along an axis.
        centre: WorldPoint::new(-22.0, -8.0),
        side_m: 8.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 0,
    },
    Scenario {
        name: "page.one_texel_mask",
        document: Document::BlendLab,
        // A whole page for the width of one texel of path edge: the feature is
        // narrower than the sampling rate, which is the case a filter either
        // handles or aliases.
        centre: WorldPoint::new(4.0, 5.0),
        side_m: 0.5,
        texels_per_metre: 8.0,
        seed: 0,
    },
    Scenario {
        name: "page.edge_transition",
        document: Document::BlendLab,
        // Framed so the path boundary lands on the page's own edge, making a
        // seam and a material transition coincide.
        centre: WorldPoint::new(14.0, 4.0),
        side_m: 4.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 0,
    },
    Scenario {
        name: "page.external_root_mark",
        document: Document::BlendLab,
        // A small window beside the path, so most of what shades it is rooted
        // outside — the halo's whole reason for existing.
        centre: WorldPoint::new(24.0, 6.0),
        side_m: 1.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 0,
    },
    Scenario {
        name: "grid.four_page_junction",
        document: Document::BlendLab,
        // Centred exactly on a page corner at the default layout, so four
        // independently baked pages meet here.
        centre: WorldPoint::new(0.0, 0.0),
        side_m: 16.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 1,
    },
    Scenario {
        name: "grid.mixed_materials",
        document: Document::BlendLab,
        centre: WorldPoint::new(-6.0, 2.0),
        side_m: 12.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 1,
    },
    Scenario {
        name: "grid.worst_grass_density",
        document: Document::BlendLab,
        // Away from the path, where nothing suppresses the canopy.
        centre: WorldPoint::new(-38.0, 24.0),
        side_m: 12.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 2,
    },
    Scenario {
        name: "grid.worst_rock_density",
        document: Document::BlendLab,
        centre: WorldPoint::new(30.0, 30.0),
        side_m: 24.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 2,
    },
    Scenario {
        name: "view.reference_close",
        document: Document::BlendLab,
        centre: WorldPoint::new(0.0, 4.0),
        side_m: CLOSE_VIEW_M,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 0,
    },
    Scenario {
        name: "view.reference_rts",
        document: Document::BlendLab,
        centre: WorldPoint::new(0.0, 4.0),
        side_m: RTS_VIEW_M,
        // A fifth of the authoring density, which is roughly what the ground is
        // actually presented at. A judgement made at 1:1 is made at more than
        // twice the size anybody sees.
        texels_per_metre: 20.0,
        seed: 0,
    },
    Scenario {
        name: "blend.grass_dirt",
        document: Document::BlendLab,
        // Straddling the path edge, so the frame is half one material and half
        // the other with the transition through the middle.
        centre: WorldPoint::new(-16.0, -2.0),
        side_m: 6.0,
        texels_per_metre: AUTHORING_TEXELS_PER_METRE,
        seed: 0,
    },
];

/// Find a scenario by name.
pub fn scenario(name: &str) -> Option<Scenario> {
    SCENARIOS.into_iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_has_a_distinct_dotted_name() {
        // The name is what a measurement is recorded under, so a collision
        // silently merges two rows of a benchmark history.
        let mut seen: Vec<&str> = Vec::new();
        for scenario in SCENARIOS {
            assert!(
                !seen.contains(&scenario.name),
                "{} is used twice",
                scenario.name
            );
            assert!(
                scenario.name.contains('.'),
                "{} is not a dotted name",
                scenario.name
            );
            assert_eq!(scenario.name, scenario.name.to_lowercase());
            seen.push(scenario.name);
        }
        assert_eq!(
            scenario("page.constant_grass").map(|s| s.name),
            Some("page.constant_grass")
        );
        assert_eq!(scenario("not.a.scenario"), None);
    }

    #[test]
    fn every_scenario_covers_real_ground_at_a_real_density() {
        for scenario in SCENARIOS {
            assert!(scenario.side_m > 0.0, "{} covers nothing", scenario.name);
            assert!(
                scenario.texels_per_metre > 0.0,
                "{} bakes at nothing",
                scenario.name
            );
            let size = scenario.size();
            assert!(size[0] >= 1 && size[1] >= 1);
            // And none of them is so large that running the suite is a job.
            assert!(
                size[0] <= 4096,
                "{} bakes {} texels across",
                scenario.name,
                size[0]
            );
        }
    }

    #[test]
    fn every_scenario_names_a_seed_that_exists() {
        for scenario in SCENARIOS {
            assert!(
                scenario.seed < crate::fixtures::SEEDS.len(),
                "{} names seed {} of {}",
                scenario.name,
                scenario.seed,
                crate::fixtures::SEEDS.len()
            );
        }
    }

    #[test]
    fn the_two_framings_differ_enough_to_measure_different_things() {
        // An optimisation that trades fine detail for speed is nearly free at
        // the wide framing and obvious at the close one. A suite photographing
        // a single framing certifies half the changes that damage the look.
        let close = scenario("view.reference_close").expect("pinned");
        let wide = scenario("view.reference_rts").expect("pinned");
        assert!(wide.side_m / close.side_m > 3.0);
        assert!(close.texels_per_metre / wide.texels_per_metre > 3.0);
    }

    #[test]
    fn the_one_texel_scenario_really_is_about_one_texel() {
        // The whole point is that the feature is narrower than the sampling
        // rate. If this ever bakes finely enough to resolve the path edge, it
        // has stopped testing what it exists for.
        let scenario = scenario("page.one_texel_mask").expect("pinned");
        let texel_m = 1.0 / scenario.texels_per_metre as f64;
        assert!(
            texel_m > 0.1,
            "a texel is {texel_m} m, fine enough to resolve a path edge"
        );
    }

    #[test]
    fn the_scenarios_exercise_both_committed_documents() {
        // A suite that only ran constant grass would measure the one case where
        // nothing composes.
        assert!(
            SCENARIOS
                .iter()
                .any(|s| s.document == Document::ConstantGrass)
        );
        assert!(SCENARIOS.iter().any(|s| s.document == Document::BlendLab));
        for document in [Document::ConstantGrass, Document::BlendLab] {
            assert!(document.file().ends_with(".terrain.ron"));
        }
    }

    #[test]
    fn a_scenarios_bounds_are_centred_where_it_says() {
        let scenario = Scenario {
            name: "test",
            document: Document::ConstantGrass,
            centre: WorldPoint::new(3.0, -7.0),
            side_m: 4.0,
            texels_per_metre: 96.0,
            seed: 0,
        };
        let bounds = scenario.bounds();
        assert_eq!(bounds.centre(), WorldPoint::new(3.0, -7.0));
        assert!((bounds.width_m() - 4.0).abs() < 1.0e-9);
        assert_eq!(scenario.size(), [384, 384]);
    }
}

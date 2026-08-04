//! The fixed inputs the grass is measured against.
//!
//! Mirrors [`bw_bench::fixtures`](../../bw_bench/fixtures/index.html) and obeys
//! the same rule: **append only, never reorder, never edit**. A benchmark
//! history means something only if place three is the same patch of ground it
//! was last month, and a zoom level that shifts by a metre makes every snapshot
//! taken before it incomparable with every snapshot taken after.
//!
//! ## Why this is in the library and not in the benchmarks
//!
//! Two tools have to agree about it. `benches/bake.rs` times these places and
//! the `grass_snapshot` example photographs these zooms, and if the two drifted
//! apart the suite would be timing one thing and looking at another — which is
//! the specific way a benchmark stops catching what it was written to catch. It
//! costs a small module in the library to make that impossible.

use glam::Vec2;

/// World metres visible vertically when the game frames a battle.
///
/// The same unit as `bw_render::BattleCamera::view_height`, deliberately, so a
/// number decided here can be typed straight into the camera.
pub const BATTLE_VIEW: f32 = 26.0;

/// World metres visible vertically when the game actually opens.
///
/// `bw_render::BattleCamera::default().view_height`, copied because the
/// dependency does not point that way. It is **not** [`BATTLE_VIEW`], and the
/// two having drifted apart is worth naming rather than quietly picking one:
/// [`BATTLE_VIEW`] is the middle rung of the snapshot ladder, and moving it
/// moves [`ZOOMS`] and invalidates every accepted snapshot, so the reconciliation
/// is its own change.
///
/// The difference is not small. At 26 metres the ground is presented at about
/// 43 percent; at 55 it is 20, which is a quarter of the pixels and the scale
/// every streaming and level-of-detail decision should be made against.
pub const SHIPPING_VIEW: f32 = 55.0;

/// The camera heights every snapshot is taken at, in world metres.
///
/// Four rather than one, because the cache is baked at a single scale and seen
/// at all of them. An optimisation that trades fine detail for speed is nearly
/// free at 48 metres and obvious at 13, and one that coarsens the mound field is
/// the other way round — so a suite that photographs a single height will
/// certify half of the changes that damage the look.
///
/// [`BATTLE_VIEW`] is in the middle of the ladder on purpose: it is the height
/// that actually ships, and the one whose row should be read first.
pub const ZOOMS: [f32; 4] = [13.0, BATTLE_VIEW, 35.0, 48.0];

/// The window a snapshot is composed for, in pixels.
///
/// Fixed rather than taken from the machine. The scale a page is displayed at is
/// `screen height / metres / PX_PER_METRE`, so a snapshot taken on a laptop
/// panel and one taken on a 4K monitor are pictures of different things.
pub const SCREEN: (usize, usize) = (1920, 1080);

/// Where in the world plates are taken from, in cache pixels.
///
/// Far enough apart that no two share a mound, a regional drift or a clump
/// field. One place measures the generator and the *place* together and cannot
/// tell them apart: two regions of a single world differ in mean luminance by as
/// much as two worlds do, which is the regional field doing its job and is
/// indistinguishable from a seed-dependent generator if you only ever look at
/// one patch.
///
/// For the performance suite the same spread matters for a different reason.
/// Bake cost is not uniform — it follows tuft density and canopy height — so a
/// timing taken at one place is a timing of that place. These three differ
/// enough to bracket it.
pub const PLACES: [Vec2; 3] = [
    Vec2::new(-724.0, -543.0),
    Vec2::new(4800.0, 2600.0),
    Vec2::new(-9100.0, 5300.0),
];

/// A short stable name for a place, for benchmark ids and snapshot filenames.
pub fn place_name(index: usize) -> &'static str {
    match index {
        0 => "home",
        1 => "east",
        2 => "west",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_ladder_holds_its_own_middle_rung() {
        // Named for what it actually checks. [`BATTLE_VIEW`] is the ladder's
        // middle rung, and a ladder that did not contain it would leave the row
        // its documentation tells you to read first unphotographed.
        assert!(ZOOMS.contains(&BATTLE_VIEW));
    }

    /// The ladder does not reach the height the game opens at, and that is a
    /// known gap rather than an oversight.
    ///
    /// `SHIPPING_VIEW` is 55 metres and the ladder stops at 48, so the scale
    /// every level-of-detail decision is made against — about a fifth — is
    /// photographed by nothing. Closing it means moving `BATTLE_VIEW`, which
    /// moves `ZOOMS`, which invalidates every accepted snapshot; that is its own
    /// change and wants its own review of the resulting pictures.
    ///
    /// This asserts the gap rather than the fix, so that whoever closes it is
    /// told to delete a test rather than left wondering whether the mismatch was
    /// deliberate.
    #[test]
    fn the_ladder_does_not_yet_reach_the_shipping_height() {
        assert!(
            !ZOOMS.contains(&SHIPPING_VIEW),
            "the ladder now reaches {SHIPPING_VIEW} m — delete this test and \
             fold SHIPPING_VIEW into BATTLE_VIEW"
        );
        assert!(
            SHIPPING_VIEW > ZOOMS[ZOOMS.len() - 1],
            "the shipping height moved inside the ladder's range without \
             joining it, which is worse than being outside it"
        );
    }

    #[test]
    fn the_zoom_ladder_climbs() {
        assert!(ZOOMS.windows(2).all(|w| w[0] < w[1]));
        // And spans enough to change the display scale by more than a factor of
        // three, which is what makes the far rows measure something the near
        // rows cannot.
        assert!(ZOOMS[ZOOMS.len() - 1] / ZOOMS[0] > 3.0);
    }

    #[test]
    fn places_share_no_ground() {
        // A page is 256 cache pixels. Anything closer than a few thousand would
        // be sampling the same mounds twice and calling it two measurements.
        for (i, a) in PLACES.iter().enumerate() {
            for b in &PLACES[i + 1..] {
                assert!(a.distance(*b) > 4000.0, "{a:?} and {b:?} are neighbours");
            }
        }
    }

    #[test]
    fn every_place_has_its_own_name() {
        let mut names: Vec<&str> = (0..PLACES.len()).map(place_name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PLACES.len());
    }
}

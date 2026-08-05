//! Are the meadow floor and the compacted track one soil or two?
//!
//! ## The question, and why it needed asking
//!
//! `meadow_floor` and `compacted_loam` differ by a factor of two and a bit in
//! brightness, and there was a real possibility that they were *the same
//! earth under different cover* — one shaded by a canopy, one open. If so,
//! keeping them apart would be double-counting: Cycles already computes the
//! canopy occlusion, so a darker profile under grass darkens it twice, and the
//! same soil could then never transition consistently from covered to exposed.
//!
//! The specification's rule for settling it is explicit: **choose a shared
//! profile unless separate profiles demonstrate a composition signal not
//! explainable by state or occlusion** — a stable hue difference, a different
//! aggregate shape, a different cohesion.
//!
//! ## The answer
//!
//! Two soils. The hue differs, and it differs in the direction and by the
//! amount that composition rather than lighting produces: the meadow floor is
//! greyer and greener, the track is redder. Occlusion scales every channel by
//! the same factor and cannot do that. The measurements are asserted below, so
//! the day somebody retunes one of them into agreement with the other, this
//! test says the question is open again.

use terrain_bench::documents;
use terrain_bench::ground::optics::{self, ColourMetric};
use terrain_core::ground_material::GroundMaterialProfile;

fn profile(name: &str) -> GroundMaterialProfile {
    let path = documents::in_repository(format!("assets/terrain/materials/{name}.ground.ron"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    terrain_format::ground_profile::from_str(&path.display().to_string(), &text)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn mid(profile: &GroundMaterialProfile) -> ColourMetric {
    ColourMetric::of(profile.optics.dry_palette.mid)
}

#[test]
fn the_two_soils_differ_in_hue_and_not_only_in_brightness() {
    // The deciding measurement. Occlusion multiplies every channel by one
    // number, so it moves luminance and leaves the channel ratios exactly
    // where they were. A ratio difference is therefore a *composition* signal
    // and nothing else can produce it.
    let floor = mid(&profile("meadow_floor"));
    let track = mid(&profile("compacted_loam"));

    let ratio_gap = (floor.g_over_r - track.g_over_r).abs();
    assert!(
        ratio_gap > 0.10,
        "the two soils' green-to-red ratios differ by only {ratio_gap:.3} \
         ({:.3} against {:.3}), which occlusion alone could produce — they may \
         be one soil and the profiles should be merged",
        floor.g_over_r,
        track.g_over_r
    );

    // And in the direction a meadow floor actually goes: organic matter is
    // grey-green, mineral track is red-brown.
    assert!(
        floor.g_over_r > track.g_over_r,
        "the meadow floor is not the greener of the two"
    );
}

#[test]
fn the_brightness_difference_alone_would_not_have_settled_it() {
    // Stated so the finding above is not mistaken for "they look different".
    // They *do* differ in brightness by more than a factor of two, and that on
    // its own is exactly what a canopy would produce — which is why it is not
    // the evidence.
    let floor = mid(&profile("meadow_floor"));
    let track = mid(&profile("compacted_loam"));
    let brightness = track.luminance / floor.luminance;
    assert!(
        brightness > 1.5,
        "the two soils differ in brightness by only {brightness:.2}x"
    );
}

#[test]
fn both_soils_darken_with_moisture_rather_than_dimming() {
    // The wet response, on both. A grey multiplier would be the tell that one
    // of them was authored by eye against a render rather than measured: it
    // moves luminance and leaves the channel ratios flat.
    for name in ["meadow_floor", "compacted_loam"] {
        let metrics = optics::measure(&profile(name), 9);
        assert!(metrics.moisture_albedo_monotone, "{name} is not monotone");
        assert!(metrics.finite_and_non_negative, "{name} left the range");
        assert!(
            metrics.endpoints_match_declaration,
            "{name} does not hit its own declared dry and wet mid"
        );
        assert!(
            metrics.hue_ratio_span > 1.0e-4,
            "{name} behaves like a grey dimmer: hue span {}",
            metrics.hue_ratio_span
        );
    }
}

#[test]
fn the_track_is_the_coarser_of_the_two() {
    // The second composition signal, independent of colour. A trodden mineral
    // track carries larger aggregates than a meadow floor bound by roots, and
    // the profiles say so — which is a structural difference that no amount of
    // state could produce from one band list.
    let floor = profile("meadow_floor");
    let track = profile("compacted_loam");
    let coarsest = |p: &GroundMaterialProfile| {
        p.coarsest_band()
            .map(|band| band.wavelength_m)
            .unwrap_or(0.0)
    };
    assert!(
        coarsest(&track) > coarsest(&floor),
        "the track's coarsest band is {:.4} m and the floor's is {:.4} m",
        coarsest(&track),
        coarsest(&floor)
    );
}

#[test]
fn a_shared_profile_would_lose_something_measurable() {
    // The counterfactual, stated as a number rather than as an opinion: if the
    // track were rendered with the floor's optics, this is how far the ground
    // would move perceptually.
    //
    // Held against five rather than a round ten. A CIEDE2000 distance of one is
    // roughly a just-noticeable difference under ideal viewing; five is two
    // colours nobody would call the same. The measured figure is about eight
    // and a half — comfortably a different soil, and not so large that the
    // threshold needed inflating to reach a conclusion.
    let floor = mid(&profile("meadow_floor"));
    let track = mid(&profile("compacted_loam"));
    let distance = optics::delta_e_2000(floor.lab, track.lab);
    assert!(
        distance > 5.0,
        "merging the two profiles would move the ground by only {distance:.1} \
         CIEDE2000, which is close enough that state and occlusion could carry \
         it — the merge should be reconsidered"
    );
}

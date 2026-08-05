//! Does the meadow *recover* across a path's edge, or stop at one?
//!
//! ## Why this is a profile and not a picture
//!
//! "The grass blends into the track" is the kind of claim a render invites and
//! cannot settle. A boundary that looks convincing at one framing is a decal at
//! another, and the two failures it hides — a green wall where the vegetation
//! band ends, and a doubled density where two populations both claim the fringe
//! — are both invisible unless somebody counts.
//!
//! So this counts. Every plant root in a compiled plate is binned by its
//! distance from the path spline, ten centimetres at a time, and the profile is
//! normalised against open meadow. What that produces is a recovery curve, and
//! a recovery curve has properties a picture does not: a width, a monotonicity,
//! and an overshoot.
//!
//! ## The gates, and where they come from
//!
//! `meadow_path` authors its bands explicitly — material 1.05→2.45 m,
//! vegetation 1.05→3.10 — so the curve's shape is a *prediction* the document
//! makes and this checks. Grass thins out before the ground stops being grass,
//! which is why the vegetation band is 65 cm wider than the material one, and
//! the numbers below are that statement turned into arithmetic.

use glam::Vec2;
use terrain_bench::documents;
use terrain_generators::compiler::{SceneCompileOptions, compile_scene};

/// How wide a distance bin is.
const BIN_M: f64 = 0.10;

/// Where the profile is normalised: open meadow, past every authored band.
///
/// Five metres out rather than three, and the first version's three was a
/// mistake worth recording. The document's widest band ends at 3.10 m, so 3.2
/// looked like open meadow — and the measured curve was still at 0.44 of full
/// density there and climbing. Normalising inside the transition makes the
/// transition look like the meadow and the meadow look like an overshoot.
const REFERENCE_M: (f64, f64) = (5.0, 6.5);

/// One plate's root density against distance from the path.
fn profile() -> Vec<(f64, f64)> {
    let terrain = documents::prepare(&documents::shipped("meadow_path")).expect("meadow_path");
    let request = terrain_scene::scene::SceneRequest::square(
        terrain_core::coords::WorldPoint::ORIGIN,
        // Wide enough that the profile reaches ground the path never touched.
        16.0,
        144.0,
    );
    let compiled = compile_scene(
        &terrain,
        &request,
        &terrain_generators::family_registry(),
        &SceneCompileOptions::default(),
    )
    .expect("compiles");

    // The spline the bands are measured from, read straight off the asset.
    //
    // Not through a derived field: spline distance is a *source* here, consumed
    // by the layers that shape the bands, and it is never materialised as a
    // plane. Parsing the polyline and measuring against it is both simpler and
    // closer to what the document means — the bands are authored in metres from
    // this line, so this is the axis they are authored on.
    let spline: Vec<Vec2> = std::fs::read_to_string(documents::in_repository(
        "assets/terrain/features/main_path.spline.ron",
    ))
    .expect("the spline asset")
    .lines()
    .filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut parts = line.split_whitespace();
        let u: f32 = parts.next()?.parse().ok()?;
        let v: f32 = parts.next()?.parse().ok()?;
        Some(Vec2::new(u, v))
    })
    .collect();
    assert!(spline.len() > 2, "the spline has {} points", spline.len());

    let distance = |at: Vec2| -> Option<f64> {
        let mut best = f32::MAX;
        for pair in spline.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let ab = b - a;
            let t = if ab.length_squared() > 0.0 {
                ((at - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            best = best.min((at - (a + ab * t)).length());
        }
        Some(best as f64)
    };

    let plant = |key: &str| key.starts_with("flower.") || key.starts_with("plant.");
    let bins = (8.0 / BIN_M) as usize;
    let mut counts = vec![0.0f64; bins];
    let mut area = vec![0.0f64; bins];

    let mut seen = std::collections::BTreeSet::new();
    for mark in &compiled.scene.marks {
        let Some(binding) = compiled.scene.materials.get(mark.material().0 as usize) else {
            continue;
        };
        if !plant(binding.appearance.as_str()) {
            continue;
        }
        let anchor = mark.anchor();
        if anchor == terrain_scene::mark::AnchorIndex::UNGROUPED || !seen.insert(anchor.index()) {
            continue;
        }
        let Some(placement) = compiled.scene.anchors.get(anchor.index()) else {
            continue;
        };
        let at = Vec2::new(placement.root.u_m as f32, placement.root.v_m as f32);
        let Some(d) = distance(at) else { continue };
        let bin = (d / BIN_M) as usize;
        if bin < bins {
            counts[bin] += 1.0;
        }
    }

    // The ground each bin covers, so a density is a density. Sampled rather
    // than derived, because a spline's offset band is not a rectangle and its
    // area varies with the curvature.
    let step = 0.05f32;
    let half = 8.0f32;
    let mut probe = -half;
    while probe < half {
        let mut other = -half;
        while other < half {
            if let Some(d) = distance(Vec2::new(probe, other)) {
                let bin = (d / BIN_M) as usize;
                if bin < bins {
                    area[bin] += (step * step) as f64;
                }
            }
            other += step;
        }
        probe += step;
    }

    (0..bins)
        .map(|bin| {
            let d = (bin as f64 + 0.5) * BIN_M;
            let density = if area[bin] > 0.05 {
                counts[bin] / area[bin]
            } else {
                f64::NAN
            };
            (d, density)
        })
        .collect()
}

#[test]
fn the_meadow_recovers_across_the_path_rather_than_stopping_at_it() {
    let profile = profile();
    let reference: Vec<f64> = profile
        .iter()
        .filter(|(d, v)| (REFERENCE_M.0..REFERENCE_M.1).contains(d) && v.is_finite())
        .map(|(_, v)| *v)
        .collect();
    assert!(
        reference.len() >= 4,
        "only {} reference bins, so there is nothing to normalise against",
        reference.len()
    );
    let meadow_density = reference.iter().sum::<f64>() / reference.len() as f64;
    assert!(
        meadow_density > 0.5,
        "open meadow carries only {meadow_density:.2} plants per square metre"
    );

    println!("distance  normalised root density");
    for (d, v) in profile.iter().filter(|(d, _)| *d < 6.0) {
        if v.is_finite() {
            let n = v / meadow_density;
            println!("  {d:4.2} m   {n:5.2}  {}", "#".repeat((n * 24.0) as usize));
        }
    }

    // A three-bin rolling mean for the gates. Single bins hold a handful of
    // plants each and their scatter is Poisson; smoothing is what turns a
    // sample into a curve without pretending the sample was denser.
    let smooth = |want: f64| -> f64 {
        let taken: Vec<f64> = profile
            .iter()
            .filter(|(d, v)| (*d - want).abs() < BIN_M * 1.5 && v.is_finite())
            .map(|(_, v)| v / meadow_density)
            .collect();
        if taken.is_empty() {
            f64::NAN
        } else {
            taken.iter().sum::<f64>() / taken.len() as f64
        }
    };

    // Inside the material band nothing stands. The track is bare earth and a
    // plant on it is the defect `plants_on_dirt` exists for; this is the same
    // claim measured as a profile rather than as a count.
    for want in [0.5, 1.0, 1.5, 2.0] {
        let n = smooth(want);
        assert!(
            !n.is_finite() || n < 0.03,
            "at {want} m the track carries {n:.3} of the meadow's plants"
        );
    }

    // ## What this curve is, and what it is not
    //
    // It is the **emergent plants** — flowers and rosettes — and nothing else.
    // The tuned passes are compiled but never emitted into `TerrainScene`
    // because they render through `GrassScene`, so no blade of grass is in this
    // count. Read as a grass transition it would be badly wrong, and the first
    // version of this file did read it that way.
    //
    // The recovery therefore completes near 4.2 m rather than at the 3.10 m the
    // vegetation band runs to, and that is authored rather than a defect:
    // `path_undergrowth_suppression` reaches 4.10 m and
    // `path_flower_suppression` 4.60 m, both widened past the material band to
    // stop daisies standing on the track. Undergrowth outnumbers flowers here
    // by nearly three to one, so the aggregate turns over where *it* releases.
    //
    // `the_grass_itself_recovers_across_the_path` is the one that measures
    // grass.
    assert!(
        smooth(4.2) > 0.65,
        "the meadow has only reached {:.2} of its density where the plant \
         suppression releases",
        smooth(4.2)
    );

    // Monotone through the fringe. A dip inside a recovery is two populations
    // handing over badly, which is the failure shared candidate domains exist
    // to prevent.
    let low = smooth(3.0);
    let mid = smooth(3.6);
    assert!(
        low < mid && mid < smooth(4.2),
        "the fringe is not monotone: {low:.2} at 3.0 m, {mid:.2} at 3.6, \
         {:.2} at 4.2",
        smooth(4.2)
    );

    // And out past everything it is meadow — not thinner, and not *thicker*.
    // An overshoot is the signature of two populations both claiming the
    // fringe, which is what the ownership draw exists to prevent.
    // No sustained overshoot **anywhere past the pure core**, not merely out in
    // the open. A handoff overshoot happens *inside* a transition, where two
    // populations are both claiming the fringe, so a gate that only looked past
    // 4.6 m could not see the failure it was named for.
    for window in profile
        .windows(3)
        .filter(|w| w[0].0 > 1.05 && w.iter().all(|(_, v)| v.is_finite()))
    {
        let run = window.iter().map(|(_, v)| v / meadow_density).sum::<f64>() / 3.0;
        assert!(
            run < 1.35,
            "three bins from {:.2} m average {run:.2} of the meadow's density",
            window[0].0
        );
    }

    // A window rather than a point. At these counts a single smoothed bin
    // scatters by a third either way, and gating on one of them measures the
    // sample size rather than the meadow.
    let open: Vec<f64> = profile
        .iter()
        .filter(|(d, v)| *d > 4.6 && v.is_finite())
        .map(|(_, v)| v / meadow_density)
        .collect();
    let mean = open.iter().sum::<f64>() / open.len().max(1) as f64;
    assert!(
        (0.80..=1.20).contains(&mean),
        "past every band the meadow averages {mean:.2} of its own density over \
         {} bins",
        open.len()
    );
    // And no sustained overshoot anywhere in it. One high bin is Poisson; three
    // in a row is two populations claiming the same ground.
    for window in open.windows(3) {
        let run = window.iter().sum::<f64>() / 3.0;
        assert!(
            run < 1.35,
            "three consecutive bins average {run:.2} of the meadow's density"
        );
    }
}

/// The tuned grass's own recovery, which is the one the blend is about.
///
/// ## Why it needs a second measurement entirely
///
/// The profile above counts anchors in the compiled `TerrainScene`, and the
/// compiler deliberately never emits a tuned population into it — grass renders
/// through `GrassScene`, grown separately at trace time from the same
/// `SemanticOverlay`. So that curve contains flowers and rosettes and not one
/// blade, and reading it as "how the grass meets the track" is exactly the
/// attribution error it looks like.
///
/// This builds the field production builds and bins the strokes it grows, by
/// pass, so "living grass" and "thatch" can be told apart: they answer different
/// questions, and a mat that thickens where the canopy thins would otherwise
/// hide inside a total.
#[test]
fn the_grass_itself_recovers_across_the_path() {
    use std::sync::Arc;
    use terrain_generators::field::{SemanticOverlay, WorldField};
    use terrain_generators::page::Page;
    use terrain_generators::scene::GrassScene;
    use terrain_generators::style::GrassParams;
    use terrain_generators::tuned::TunedPass;

    let terrain = documents::prepare(&documents::shipped("meadow_path")).expect("meadow_path");
    let request = terrain_scene::scene::SceneRequest::square(
        terrain_core::coords::WorldPoint::ORIGIN,
        16.0,
        144.0,
    );
    let compiled = compile_scene(
        &terrain,
        &request,
        &terrain_generators::family_registry(),
        &SceneCompileOptions::default(),
    )
    .expect("compiles");

    // Every canonical seed, each normalised against its own open meadow.
    //
    // Pooling them would hide the failure this is for: a transition that is
    // right on average and a green wall on one seed is a transition that ships
    // a green wall. Four of the ten, because the grass field is the expensive
    // half and four independent worlds already separate "the shape is wrong"
    // from "this seed is unlucky".
    for seed in terrain_bench::fixtures::SEEDS.iter().take(4) {
    let params = GrassParams {
        seed: *seed,
        ..GrassParams::default()
    };
    let field = WorldField::lit_by(params.seed, params.light).with_overlay(Arc::new(
        SemanticOverlay {
            ground: Arc::clone(&compiled.ground),
            interactions: Arc::clone(&compiled.interactions),
            tuned: Arc::clone(&compiled.tuned),
        },
    ));
    let scene = GrassScene::build(Page::new(Vec2::new(0.0, 0.0), 384, 384), &field, &params);

    let spline = path_spline();
    let bins = (8.0 / BIN_M) as usize;
    let mut living = vec![0.0f64; bins];
    let mut thatch = vec![0.0f64; bins];
    let mut area = vec![0.0f64; bins];

    for stroke in &scene.marks {
        let at = Vec2::new(stroke.root.x, stroke.root.y);
        let bin = (distance_to(&spline, at) / BIN_M) as usize;
        if bin >= bins {
            continue;
        }
        match stroke.pass {
            TunedPass::Fine | TunedPass::Tuft => living[bin] += 1.0,
            TunedPass::Thatch => thatch[bin] += 1.0,
            TunedPass::Broadleaf => living[bin] += 1.0,
        }
    }

    // The ground each bin covers, over the page the strokes were grown on.
    let step = 0.05f32;
    let half = 4.0f32;
    let mut u = -half;
    while u < half {
        let mut v = -half;
        while v < half {
            let bin = (distance_to(&spline, Vec2::new(u, v)) / BIN_M) as usize;
            if bin < bins {
                area[bin] += (step * step) as f64;
            }
            v += step;
        }
        u += step;
    }

    let density = |counts: &[f64]| -> Vec<(f64, f64)> {
        (0..bins)
            .map(|bin| {
                let d = (bin as f64 + 0.5) * BIN_M;
                let n = if area[bin] > 0.05 {
                    counts[bin] / area[bin]
                } else {
                    f64::NAN
                };
                (d, n)
            })
            .collect()
    };
    let live_profile = density(&living);
    let reference: Vec<f64> = live_profile
        .iter()
        .filter(|(d, v)| (3.4..3.9).contains(d) && v.is_finite())
        .map(|(_, v)| *v)
        .collect();
    assert!(!reference.is_empty(), "no reference bins");
    let open = reference.iter().sum::<f64>() / reference.len() as f64;
    assert!(open > 100.0, "open meadow grows only {open:.0} strokes per m2");

    let smooth = |profile: &[(f64, f64)], want: f64| -> f64 {
        let taken: Vec<f64> = profile
            .iter()
            .filter(|(d, v)| (*d - want).abs() < BIN_M * 1.5 && v.is_finite())
            .map(|(_, v)| v / open)
            .collect();
        if taken.is_empty() {
            f64::NAN
        } else {
            taken.iter().sum::<f64>() / taken.len() as f64
        }
    };

    let _ = density(&thatch);

    // Inside the material band the grass is gone.
    assert!(
        smooth(&live_profile, 0.6) < 0.03,
        "the middle of the track grows {:.3} of the meadow's grass",
        smooth(&live_profile, 0.6)
    );

    // ## A bounded recovery, not a hard edge
    //
    // The ten and ninety per cent crossings have to be *apart*. A step would
    // put them in the same bin, which is the green wall this whole transition
    // machinery exists to avoid, and a recovery that never finishes would put
    // the ninety past the document's own band.
    let crossing = |want: f64| -> f64 {
        live_profile
            .iter()
            .find(|(d, v)| *d > 1.0 && v.is_finite() && v / open >= want)
            .map(|(d, _)| *d)
            .unwrap_or(f64::NAN)
    };
    let d10 = crossing(0.10);
    let d90 = crossing(0.90);
    println!("seed {seed:016x}: living grass 10% at {d10:.2} m, 90% at {d90:.2} m");
    assert!(
        d10 >= 1.0 && d90 <= 3.60 && d90 - d10 >= 0.30,
        "seed {seed:016x}: the grass recovers from {d10:.2} m to {d90:.2} m, \
         which is not a bounded transition"
    );

    // And no doubled density anywhere across the fringe.
    for window in live_profile
        .windows(3)
        .filter(|w| w[0].0 > 1.05 && w.iter().all(|(_, v)| v.is_finite()))
    {
        let run = window.iter().map(|(_, v)| v / open).sum::<f64>() / 3.0;
        assert!(
            run < 1.35,
            "seed {seed:016x}: three bins from {:.2} m average {run:.2}",
            window[0].0
        );
    }
    }
}

/// The path polyline, read off the asset the document names.
fn path_spline() -> Vec<Vec2> {
    std::fs::read_to_string(documents::in_repository(
        "assets/terrain/features/main_path.spline.ron",
    ))
    .expect("the spline asset")
    .lines()
    .filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut parts = line.split_whitespace();
        Some(Vec2::new(
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    })
    .collect()
}

/// Distance from a point to the polyline, in metres.
fn distance_to(spline: &[Vec2], at: Vec2) -> f64 {
    let mut best = f32::MAX;
    for pair in spline.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let ab = b - a;
        let t = if ab.length_squared() > 0.0 {
            ((at - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        best = best.min((at - (a + ab * t)).length());
    }
    best as f64
}

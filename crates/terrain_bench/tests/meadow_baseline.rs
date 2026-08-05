//! The meadow tier, pinned, so that every phase has a before and an after.
//!
//! Companion to `refactor_fingerprints`. That test pins the *tuned* generator's
//! marks; this pins everything the tuned generator never sees — which
//! populations compile, how the shared domains behave, who renders what, and how
//! many marks reach the scene.
//!
//! ## Accepting a new set
//!
//! `TERRAIN_ACCEPT_MEADOW=1 cargo test -p terrain_bench --test meadow_baseline`
//!
//! Do that only when a phase was *meant* to move these numbers, and say which
//! phase in the commit message. The whole value of the file is that a diff to it
//! is a question somebody has to answer.

use std::collections::{BTreeMap, BTreeSet};

use terrain_bench::documents::{COMPILABLE, NOT_COMPILABLE};
use terrain_bench::meadow;
use terrain_generators::tuned::TunedPass;

/// The seed the tuned stroke counts are taken at.
///
/// One of the committed set rather than an arbitrary number, so the counts can
/// be compared against anything else measured at the same seed.
const TUNED_SEED: u64 = 0x5a17_e33b_0c9d_2f14;

fn measure() -> (Vec<meadow::MeadowRow>, BTreeMap<TunedPass, usize>) {
    let rows = COMPILABLE
        .iter()
        .map(|name| meadow::row(name).unwrap_or_else(|error| panic!("{name}: {error}")))
        .collect();
    (rows, meadow::tuned_strokes_by_pass(TUNED_SEED))
}

#[test]
fn the_pinned_meadow_tier_is_unchanged() {
    let (rows, passes) = measure();
    let text = meadow::render(&rows, &passes);
    let path = meadow::baseline_path();

    if std::env::var_os("TERRAIN_ACCEPT_MEADOW").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture directory is writable");
        }
        std::fs::write(&path, &text).expect("the baseline is writable");
        return;
    }

    // Read and normalised rather than compared byte for byte. A Windows
    // checkout with `core.autocrlf=true` rewrites the fixture's line endings,
    // and a baseline that failed on that would be reporting the checkout rather
    // than the meadow. `.gitattributes` pins the file as well; this is the belt
    // to that braces.
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "no committed baseline at {}: {error}\n\
             create one with TERRAIN_ACCEPT_MEADOW=1",
            path.display()
        )
    });

    if committed.replace("\r\n", "\n") != text {
        // The whole diff rather than the first differing line. A phase that
        // moves four documents should be reviewed as four changes, not
        // discovered one test run at a time.
        panic!(
            "the meadow tier moved.\n\n--- committed\n{committed}\n--- measured\n{text}\n\
             If a phase meant to move these numbers, accept with \
             TERRAIN_ACCEPT_MEADOW=1 and say which phase in the commit."
        );
    }
}

#[test]
fn no_tuned_population_reaches_the_secondary_scene() {
    // The invariant the render-class split exists to establish, asserted
    // directly rather than read off the counts. A tuned population emitting
    // marks into the compiled scene is a second, lower-quality canopy waiting
    // for the moment that scene is rendered.
    for name in COMPILABLE {
        let compiled = meadow::compile(name).unwrap_or_else(|error| panic!("{name}: {error}"));
        let leaked = meadow::tuned_populations_that_emitted(&compiled);
        assert!(
            leaked.is_empty(),
            "{name}: {} emitted into the secondary scene despite not being drawn from it",
            leaked.join(", ")
        );
    }
}

#[test]
fn every_population_is_classified() {
    // Compared against the *document's* population keys, not against the ones
    // that emitted marks. `marks_by_population` only gains an entry after a
    // population emits something, so a tuned or deferred population dropped
    // from the class table would emit zero marks and be missing from both
    // sides of a comparison between them — which is to say, the obvious test
    // passes precisely when the bug is present.
    use terrain_bench::documents;
    for name in COMPILABLE {
        let terrain = documents::prepare(&documents::shipped(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let compiled = meadow::compile(name).unwrap_or_else(|error| panic!("{name}: {error}"));

        let authored: BTreeSet<String> = terrain
            .populations()
            .iter()
            .map(|population| population.key.as_str().to_string())
            .collect();
        let classified: BTreeSet<String> = compiled.report.render_classes.keys().cloned().collect();
        assert_eq!(
            authored,
            classified,
            "{name}: the document declares {} populations and the report classifies {}",
            authored.len(),
            classified.len()
        );
    }
}

#[test]
fn the_tuned_passes_all_plant_something() {
    // Guards the pass tag itself. If `scatter` stopped stamping a pass — or
    // stamped the same one twice — the counts would collapse into one bucket
    // and every per-pass control built on top would silently address the wrong
    // layer.
    let counts = meadow::tuned_strokes_by_pass(TUNED_SEED);
    for pass in TunedPass::ALL {
        let count = counts.get(&pass).copied().unwrap_or(0);
        assert!(count > 0, "the {pass} pass planted nothing");
    }
}

#[test]
fn the_fine_pass_is_the_densest_layer() {
    // A weak but load-bearing check that the tags landed on the right passes
    // rather than merely on four distinct buckets: the tuned style asks for
    // roughly ten fine blades per thatch stroke and far more than that per
    // tuft, so a mislabelled pair would show up here.
    let counts = meadow::tuned_strokes_by_pass(TUNED_SEED);
    let fine = counts[&TunedPass::Fine];
    let thatch = counts[&TunedPass::Thatch];
    let broadleaf = counts[&TunedPass::Broadleaf];
    assert!(
        fine > thatch,
        "fine {fine} is not denser than thatch {thatch}"
    );
    assert!(
        thatch > broadleaf,
        "thatch {thatch} is not denser than broadleaf {broadleaf}"
    );
}

#[test]
fn the_documents_that_cannot_compile_still_prepare() {
    // The honest half of the baseline. Two shipped documents name recipes from
    // the older population registry and `compile_scene` refuses them; that is a
    // known state rather than a failure, and this test is what keeps it from
    // drifting into an *unknown* state. If one of them starts compiling, the
    // list is wrong and somebody migrated it without saying so.
    use terrain_bench::documents;
    for (name, why) in NOT_COMPILABLE {
        documents::prepare(&documents::shipped(name))
            .unwrap_or_else(|error| panic!("{name} no longer prepares: {error}"));
        let compiled = terrain_bench::meadow::compile(name);
        assert!(
            compiled.is_err(),
            "{name} now compiles, but the baseline still records that it {why}"
        );
    }
}

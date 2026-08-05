//! What the meadow tier is, in numbers, pinned.
//!
//! `refactor_fingerprints` answers "is it the same meadow?" for the *tuned*
//! generator. This answers the same question for the half the tuned generator
//! never sees: which populations compile, how many candidates each domain
//! offers, how many survive acceptance, who owns the rendering, and how many
//! marks come out the far side.
//!
//! ## Why the numbers and not just a digest
//!
//! A single scene fingerprint tells you something moved. It does not tell you
//! whether a flower population went quiet, a domain stopped generating, or the
//! transition solver shifted a boundary — and during a phase that deliberately
//! removes generic grass from the secondary scene, "something moved" is the
//! expected result rather than the finding. Counts localise the change to a
//! population, which is what makes an intentional move separable from an
//! accident.
//!
//! ## Why the tuned stroke counts sit here too
//!
//! Because the failure this whole tier is built to avoid is *doubling the
//! meadow*: rendering the compiled scene alongside the tuned canopy. The two
//! halves of that failure are a rise in secondary marks and no fall anywhere
//! else, and seeing them in one table is what makes the arithmetic checkable.

use std::collections::BTreeMap;
use std::path::Path;

use terrain_generators::compiler::{SceneCompilation, SceneCompileOptions, compile_scene};
use terrain_generators::style::GrassParams;
use terrain_generators::tuned::TunedPass;

use crate::documents::{self, LoadError};

/// One compiled document, as a row of numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeadowRow {
    pub document: String,
    /// The scene digest, in full.
    ///
    /// All 128 bits rather than the eight-digit short form. The short form is
    /// for a log line a human is skimming; a pinned baseline is compared by
    /// machine, and a geometry-only regression that kept every count would slip
    /// past a thirty-two-bit comparison the moment two digests shared a prefix.
    pub scene_fingerprint: String,
    pub candidates_generated: usize,
    pub candidates_accepted: usize,
    pub candidates_unowned: usize,
    pub marks_emitted: usize,
    /// How many accepted candidates grew something — plants, not primitives.
    pub placements: usize,
    /// How many obstacles other content must grow around.
    pub interactions: usize,
    /// How many distinct prototype shapes the scene refers to.
    pub prototypes: usize,
    /// Population key to mark count, including the ones that emitted nothing.
    pub marks_by_population: BTreeMap<String, usize>,
    /// Population key to who draws it.
    pub render_classes: BTreeMap<String, String>,
}

/// How the baseline compiles a document.
///
/// A small fixed window rather than the nine-tile plate the CLI renders. The
/// question is whether the *compiler* still makes the same decisions, and a
/// four-metre square makes them a few hundred thousand times — enough that a
/// changed rule cannot hide, and fast enough to run on every commit.
pub const BASELINE_BOUNDS_M: f64 = 4.0;

/// The pixels-per-metre the baseline frames at.
///
/// Fixed, because it derives the field spacing and therefore the matrix the
/// candidates are scored against. A baseline that inherited the caller's
/// framing would move whenever a default changed somewhere else.
pub const BASELINE_PX_PER_METRE: f32 = 144.0;

/// Compile one shipped document at the pinned framing.
pub fn compile(name: &str) -> Result<SceneCompilation, LoadError> {
    let terrain = documents::prepare(&documents::shipped(name))?;
    let request = baseline_request();
    let registry = terrain_generators::family_registry();
    let options = SceneCompileOptions::default();
    compile_scene(&terrain, &request, &registry, &options).map_err(|error| LoadError::Prepare {
        path: name.to_string(),
        message: error.to_string(),
    })
}

/// The scene request every baseline row is compiled against.
pub fn baseline_request() -> terrain_scene::scene::SceneRequest {
    terrain_scene::scene::SceneRequest::square(
        terrain_core::coords::WorldPoint::ORIGIN,
        BASELINE_BOUNDS_M,
        BASELINE_PX_PER_METRE,
    )
}

/// Compile one document and reduce it to a comparable row.
pub fn row(name: &str) -> Result<MeadowRow, LoadError> {
    let compiled = compile(name)?;
    let report = &compiled.report;
    Ok(MeadowRow {
        document: name.to_string(),
        scene_fingerprint: compiled.scene.fingerprint().to_string(),
        candidates_generated: report.candidates_generated,
        candidates_accepted: report.candidates_accepted,
        candidates_unowned: report.candidates_unowned,
        marks_emitted: report.marks_emitted,
        placements: compiled.scene.placement_count(),
        interactions: compiled.scene.interactions.len(),
        prototypes: compiled.scene.prototypes.len(),
        marks_by_population: report.marks_by_population.clone(),
        render_classes: report
            .render_classes
            .iter()
            .map(|(key, class)| (key.clone(), class.to_string()))
            .collect(),
    })
}

/// How many tuned strokes each pass plants on one pinned page.
///
/// The counter-metric for the secondary numbers above. If a change adds
/// secondary marks *and* leaves these alone, the meadow has genuinely gained
/// content; if it adds secondary marks that duplicate a tuned pass, this table
/// is where the duplication is visible as a total that no longer matches the
/// picture.
pub fn tuned_strokes_by_pass(seed: u64) -> BTreeMap<TunedPass, usize> {
    use glam::Vec2;
    use terrain_generators::field::WorldField;
    use terrain_generators::page::Page;
    use terrain_generators::scene::GrassScene;

    let params = GrassParams {
        seed,
        ..GrassParams::default()
    };
    let field = WorldField::lit_by(params.seed, params.light);
    let page = Page::new(Vec2::new(0.0, 0.0), 192, 192);
    let scene = GrassScene::build(page, &field, &params);

    let mut counts: BTreeMap<TunedPass, usize> =
        TunedPass::ALL.iter().map(|pass| (*pass, 0usize)).collect();
    for mark in &scene.marks {
        *counts.entry(mark.pass).or_default() += 1;
    }
    counts
}

/// Whether any population that draws through a tuned pass also reached the
/// secondary scene.
///
/// The one invariant this whole phase exists to establish, stated as a
/// predicate so a test can assert it directly rather than by reading counts.
pub fn tuned_populations_that_emitted(compiled: &SceneCompilation) -> Vec<String> {
    compiled
        .report
        .render_classes
        .iter()
        .filter(|(key, class)| {
            !class.emits_secondary()
                && compiled
                    .report
                    .marks_by_population
                    .get(*key)
                    .copied()
                    .unwrap_or(0)
                    > 0
        })
        .map(|(key, _)| key.clone())
        .collect()
}

/// Format the rows as the pinned file's text.
pub fn render(rows: &[MeadowRow], passes: &BTreeMap<TunedPass, usize>) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("// The meadow tier, pinned. Regenerate with:\n");
    out.push_str(
        "//   TERRAIN_ACCEPT_MEADOW=1 cargo test -p terrain_bench --test meadow_baseline\n",
    );
    out.push_str("// A row that moves without a phase behind it is a regression.\n\n");
    for row in rows {
        let _ = writeln!(out, "[{}]", row.document);
        let _ = writeln!(out, "scene = {}", row.scene_fingerprint);
        let _ = writeln!(
            out,
            "candidates = {} generated, {} accepted, {} unowned",
            row.candidates_generated, row.candidates_accepted, row.candidates_unowned
        );
        let _ = writeln!(
            out,
            "marks = {}, placements = {}, prototypes = {}, interactions = {}",
            row.marks_emitted, row.placements, row.prototypes, row.interactions
        );
        for (population, class) in &row.render_classes {
            let marks = row
                .marks_by_population
                .get(population)
                .copied()
                .unwrap_or(0);
            let _ = writeln!(out, "  {population} = {class}, {marks} marks");
        }
        out.push('\n');
    }
    out.push_str("[tuned_strokes]\n");
    for (pass, count) in passes {
        let _ = writeln!(out, "  {pass} = {count}");
    }
    out
}

/// Where the pinned file lives.
pub fn baseline_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("meadow")
        .join("baseline.txt")
}

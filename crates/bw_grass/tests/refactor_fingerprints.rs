//! The meadow, pinned, so that moving code between crates cannot quietly change
//! it.
//!
//! Every fixture here grows a page and digests it. The digests live in
//! `tests/fixtures/refactor/scenes.ron`, in the repository, and this test fails
//! if any of them moves. That is the whole mechanism, and its value is entirely
//! in what it costs to run: a tenth of a second, no window, no Blender, no
//! reference art. During the terrain migration it is the check to run after
//! every single move, which a Cycles render or a snapshot ladder could never be.
//!
//! ## What a failure here means
//!
//! Not "the picture got worse". It means **the generator is producing different
//! marks**, and during a refactor that is supposed to be impossible. The usual
//! causes, roughly in order of how often they turn out to be the culprit:
//!
//! - A random stream consumed in a different order, or a draw added or removed.
//! - Placement passes reordered, so the painter order changed.
//! - A parameter rounded, clamped or defaulted differently after a move.
//! - Floating-point arithmetic reassociated — usually by hoisting something out
//!   of a loop, which is a real change to the result however innocent it looks.
//!
//! ## Accepting a new set
//!
//! `TERRAIN_ACCEPT_FINGERPRINTS=1 cargo test -p bw_grass --test refactor_fingerprints`
//!
//! rewrites the file. Do that only when the meadow was *meant* to change, bump
//! [`bw_grass::fingerprint::GENERATOR_VERSION`] in the same commit, and say why
//! in the message. A fingerprint accepted without a reason is a fingerprint that
//! will be accepted without a reason again.

use std::fmt::Write as _;
use std::path::PathBuf;

use bw_grass::bake::BakeParams;
use bw_grass::field::WorldField;
use bw_grass::fingerprint::GENERATOR_VERSION;
use bw_grass::fixtures::PLACES;
use bw_grass::page::Page;
use bw_grass::scene::GrassScene;
use glam::Vec2;

/// One pinned scene.
struct Fixture {
    /// Stable, and stable is the point — the name is the key in the committed
    /// file, so renaming one silently drops its coverage and adds a new row.
    name: &'static str,
    origin: Vec2,
    side: usize,
    /// Fraction of the authoring scale this page is baked at.
    detail: f32,
    seed: u64,
}

/// A page side small enough that the whole suite runs in a blink and large
/// enough to contain several tufts, several mounds' worth of field, and a guard
/// band's worth of marks rooted outside it.
const SIDE: usize = 96;

/// The pinned set. **Append only.**
///
/// Six rows, each buying something the others cannot:
///
/// - The three [`PLACES`] are the same ground the benchmarks and snapshots use,
///   far enough apart to share no mound and no regional drift.
/// - `origin.straddle` sits on the world origin with a negative corner, which is
///   where sign handling in the lattice and the mound grid goes wrong.
/// - `home.quarter_detail` bakes the same ground at a quarter scale, which is
///   the path where a length in metres and a length in cache pixels must part
///   company. Nearly every scale bug lives in that distinction.
/// - `home.other_seed` proves the digest is a function of the world and not only
///   of the place.
const FIXTURES: [Fixture; 6] = [
    Fixture {
        name: "home.authoring",
        origin: PLACES[0],
        side: SIDE,
        detail: 1.0,
        seed: 0x5eed_1234,
    },
    Fixture {
        name: "east.authoring",
        origin: PLACES[1],
        side: SIDE,
        detail: 1.0,
        seed: 0x5eed_1234,
    },
    Fixture {
        name: "west.authoring",
        origin: PLACES[2],
        side: SIDE,
        detail: 1.0,
        seed: 0x5eed_1234,
    },
    Fixture {
        name: "origin.straddle",
        origin: Vec2::new(-48.0, -48.0),
        side: SIDE,
        detail: 1.0,
        seed: 0x5eed_1234,
    },
    Fixture {
        name: "home.quarter_detail",
        origin: PLACES[0],
        side: SIDE,
        detail: 0.25,
        seed: 0x5eed_1234,
    },
    Fixture {
        name: "home.other_seed",
        origin: PLACES[0],
        side: SIDE,
        detail: 1.0,
        seed: 0x0000_0001,
    },
];

impl Fixture {
    /// Grow it and digest it. Also returns the mark count, which is committed
    /// beside the fingerprint purely so a human reading a failure has something
    /// to reason with — a digest that moved tells you nothing, a digest that
    /// moved with the count unchanged says "the same marks, differently".
    fn measure(&self) -> (String, usize) {
        let params = BakeParams {
            seed: self.seed,
            ..BakeParams::default()
        };
        let field = WorldField::lit_by(params.seed, params.light);
        let page = Page::at_detail(self.origin, self.side, self.side, self.detail);
        let scene = GrassScene::build(page, &field, &params.grass());
        (
            scene.fingerprint(params.seed, &field).to_string(),
            scene.len(),
        )
    }
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/refactor/scenes.ron")
}

/// The committed file, as `(name, fingerprint, marks)` rows in fixture order.
fn render(rows: &[(&'static str, String, usize)]) -> String {
    let mut out = String::new();
    out.push_str(
        "// Scene fingerprints, pinned. See tests/refactor_fingerprints.rs.\n\
         //\n\
         // These are not a performance baseline and not a picture. They are the\n\
         // statement that the generator produces the same meadow it did before,\n\
         // and they are what the terrain migration is checked against after every\n\
         // move. Accept a new set only alongside a deliberate change, with the\n\
         // generator version bumped and the reason in the commit message.\n(\n",
    );
    let _ = writeln!(out, "    generator_version: {GENERATOR_VERSION},");
    out.push_str("    scenes: [\n");
    for (name, fingerprint, marks) in rows {
        let _ = writeln!(
            out,
            "        (name: \"{name}\", fingerprint: \"{fingerprint}\", marks: {marks}),"
        );
    }
    out.push_str("    ],\n)\n");
    out
}

/// Pull the rows back out of the committed file.
///
/// Parsed by hand rather than through `ron`, because a fixture file that needs a
/// deserialiser to read is a fixture file whose failure message depends on that
/// deserialiser still compiling — and this test has to keep working through
/// exactly the churn that might break it.
fn parse(text: &str) -> Vec<(String, String, usize)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = line.strip_prefix("(name: \"")?;
            let (name, rest) = name.split_once('"')?;
            let (_, rest) = rest.split_once("fingerprint: \"")?;
            let (fingerprint, rest) = rest.split_once('"')?;
            let (_, rest) = rest.split_once("marks: ")?;
            let marks = rest.trim_end_matches([')', ',']).parse().ok()?;
            Some((name.to_string(), fingerprint.to_string(), marks))
        })
        .collect()
}

#[test]
fn the_pinned_meadows_are_unchanged() {
    let measured: Vec<(&'static str, String, usize)> = FIXTURES
        .iter()
        .map(|fixture| {
            let (fingerprint, marks) = fixture.measure();
            (fixture.name, fingerprint, marks)
        })
        .collect();

    let path = fixture_path();
    if std::env::var_os("TERRAIN_ACCEPT_FINGERPRINTS").is_some() {
        std::fs::create_dir_all(path.parent().expect("fixture path has a parent"))
            .expect("could not create the fixture directory");
        std::fs::write(&path, render(&measured)).expect("could not write the fixtures");
        eprintln!(
            "accepted {} fingerprints into {}",
            measured.len(),
            path.display()
        );
        return;
    }

    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "no committed fingerprints at {}: {error}\n\
             run with TERRAIN_ACCEPT_FINGERPRINTS=1 to write the first set",
            path.display()
        )
    });
    let committed = parse(&text);
    assert!(
        !committed.is_empty(),
        "{} parsed to no rows at all — the format changed under the parser",
        path.display()
    );

    let mut drifted = Vec::new();
    for (name, fingerprint, marks) in &measured {
        match committed.iter().find(|(committed, _, _)| committed == name) {
            None => drifted.push(format!("  {name}: not in the committed set")),
            Some((_, was, was_marks)) if was != fingerprint => drifted.push(format!(
                "  {name}: {was} -> {fingerprint}  ({was_marks} -> {marks} marks)"
            )),
            Some(_) => {}
        }
    }
    for (name, _, _) in &committed {
        if !measured.iter().any(|(measured, _, _)| measured == name) {
            drifted.push(format!("  {name}: committed but no longer measured"));
        }
    }

    assert!(
        drifted.is_empty(),
        "the generated meadow moved:\n{}\n\n\
         If this was not deliberate, something reordered a random stream, a\n\
         placement pass, or a floating-point expression. If it was deliberate,\n\
         bump GENERATOR_VERSION and re-accept with TERRAIN_ACCEPT_FINGERPRINTS=1.",
        drifted.join("\n")
    );
}

#[test]
fn the_committed_set_matches_the_generator_version() {
    // A stale version line makes every future failure ambiguous: nobody can tell
    // whether the digests were accepted before or after the last deliberate
    // change to the generator.
    let text = std::fs::read_to_string(fixture_path()).expect("committed fingerprints");
    let expected = format!("generator_version: {GENERATOR_VERSION},");
    assert!(
        text.contains(&expected),
        "the fixtures were accepted under a different generator version than {GENERATOR_VERSION}"
    );
}

#[test]
fn every_fixture_grows_something_worth_digesting() {
    // A fixture that grew nothing would pin an empty digest and pass forever.
    for fixture in &FIXTURES {
        let (_, marks) = fixture.measure();
        assert!(marks > 100, "{} grew only {marks} marks", fixture.name);
    }
}

#[test]
fn no_two_fixtures_pin_the_same_meadow() {
    // Six rows are only six rows of coverage if they are six different meadows.
    let mut seen: Vec<(String, &'static str)> = Vec::new();
    for fixture in &FIXTURES {
        let (fingerprint, _) = fixture.measure();
        if let Some((_, other)) = seen.iter().find(|(seen, _)| *seen == fingerprint) {
            panic!("{} and {other} pin the same meadow", fixture.name);
        }
        seen.push((fingerprint, fixture.name));
    }
}

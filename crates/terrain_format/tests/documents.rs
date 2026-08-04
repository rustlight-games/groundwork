//! The committed documents, read the way a tool reads them.
//!
//! Unit tests inside the crate check each stage; this checks the whole pipe
//! against files that are actually in the repository. The difference matters:
//! a synthetic document built in Rust cannot catch a RON spelling that the
//! deserialiser refuses, and that is exactly the class of mistake an author
//! hits first.

use std::path::{Path, PathBuf};

use terrain_format::{CURRENT_FORMAT_VERSION, LoadError, from_str, load};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/terrain/documents")
}

fn constant_grass() -> PathBuf {
    assets().join("constant_grass.terrain.ron")
}

#[test]
fn the_constant_grass_document_loads_and_validates() {
    // The milestone: a version-one document, from disk, through migration,
    // canonicalisation and validation, with nothing to report.
    let loaded = match load(&constant_grass()) {
        Ok(loaded) => loaded,
        Err(error) => panic!("{error}"),
    };
    assert_eq!(loaded.source_version, CURRENT_FORMAT_VERSION);
    assert!(!loaded.migration.migrated());
    assert!(
        loaded.report.is_empty(),
        "unexpected diagnostics:\n{}",
        loaded.report
    );

    let document = &loaded.document;
    assert_eq!(document.materials.len(), 1);
    assert_eq!(document.materials[0].key.as_str(), "grass_lush");
    assert_eq!(
        document.materials[0].appearance.as_str(),
        "surface.grass_lush"
    );
    assert_eq!(document.layers.len(), 1);
    assert_eq!(document.populations.len(), 1);
    assert_eq!(document.root_seed.to_string(), "8df782f95ce1a4d4");
}

#[test]
fn the_constant_grass_document_has_a_stable_digest() {
    // Read twice, digested twice. A digest that depended on anything about the
    // reading — allocation order, a HashMap, the path it was read from — would
    // show up here and nowhere else.
    let first = load(&constant_grass()).expect("loads").document.digest();
    let second = load(&constant_grass()).expect("loads").document.digest();
    assert_eq!(first, second);
    assert_eq!(first.to_string().len(), 32);
}

#[test]
fn whitespace_and_comments_do_not_reach_the_digest() {
    // The property that makes a digest usable as a cache key: reformatting a
    // document must not invalidate every bake taken from it.
    let text = std::fs::read_to_string(constant_grass()).expect("readable");
    let reference = from_str(&text, "reference")
        .expect("loads")
        .document
        .digest();

    let reflowed = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let stripped = from_str(&reflowed, "stripped")
        .expect("loads")
        .document
        .digest();
    assert_eq!(reference, stripped, "comments reached the digest");
}

#[test]
fn a_misspelled_field_is_refused_rather_than_ignored() {
    // The worst failure authored content has: the file loads, the terrain is
    // wrong, and nothing says why. `deny_unknown_fields` is what makes this a
    // parse error instead.
    let text = std::fs::read_to_string(constant_grass())
        .expect("readable")
        .replace(
            "display_name: \"Lush Grass\"",
            "displayname: \"Lush Grass\"",
        );
    let error = from_str(&text, "misspelled").expect_err("refused");
    assert!(
        matches!(error, LoadError::Syntax { .. }),
        "expected a syntax error, got {error}"
    );
}

#[test]
fn a_file_that_is_not_a_terrain_document_says_so() {
    // Rather than complaining about a missing `root_seed`, which reads as "this
    // document is broken" and sends the author looking in the wrong place.
    let text = r#"(format: "terrain-spline", format_version: 1, document: ())"#;
    let error = from_str(text, "spline.ron").expect_err("refused");
    assert!(
        matches!(error, LoadError::WrongFormat { .. }),
        "expected a format error, got {error}"
    );
    assert!(error.to_string().contains("not a terrain document"));
}

#[test]
fn a_document_from_the_future_is_refused_by_its_version() {
    let text = std::fs::read_to_string(constant_grass())
        .expect("readable")
        .replace("format_version: 1", "format_version: 99");
    let error = from_str(&text, "future").expect_err("refused");
    assert!(
        matches!(error, LoadError::Migration { .. }),
        "expected a migration error, got {error}"
    );
    assert!(error.to_string().contains("99"), "{error}");
}

#[test]
fn several_bad_keys_are_reported_together() {
    // The reason canonicalisation collects. An author with three misspellings
    // should be told about three, not shown the first and left to rebuild.
    let text = std::fs::read_to_string(constant_grass())
        .expect("readable")
        .replace("key: \"grass_lush\"", "key: \"Grass Lush\"")
        .replace("key: \"everywhere\"", "key: \"every where\"")
        .replace("key: \"base_grass\"", "key: \"Base_Grass\"");
    let error = from_str(&text, "typos").expect_err("refused");
    let LoadError::Invalid { report, .. } = &error else {
        panic!("expected invalid, got {error}");
    };
    let invalid = report
        .entries()
        .iter()
        .filter(|e| e.code == "invalid_key")
        .count();
    assert!(
        invalid >= 3,
        "only {invalid} key problems reported:\n{report}"
    );
}

#[test]
fn a_semantic_problem_is_reported_after_a_clean_parse() {
    // Parsing and validation are separate passes, and this is the one that says
    // so: the file is perfectly well-formed RON and the document still does not
    // mean anything.
    let text = std::fs::read_to_string(constant_grass())
        .expect("readable")
        .replace("material: \"grass_lush\"", "material: \"grass_lushh\"");
    let error = from_str(&text, "unknown material").expect_err("refused");
    let LoadError::Invalid { report, .. } = &error else {
        panic!("expected invalid, got {error}");
    };
    let entry = report
        .entries()
        .iter()
        .find(|e| e.code == "unknown_material")
        .expect("reported");
    assert_eq!(entry.help.as_deref(), Some("did you mean `grass_lush`?"));
}

#[test]
fn every_committed_document_loads() {
    // A directory sweep, so adding a document to the repository without a test
    // still cannot leave it broken.
    let directory = assets();
    let mut checked = 0;
    for entry in std::fs::read_dir(&directory).expect("the documents directory exists") {
        let path = entry.expect("readable entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("ron") {
            continue;
        }
        match load(&path) {
            Ok(loaded) => {
                assert!(
                    !loaded.report.has_errors(),
                    "{}:\n{}",
                    path.display(),
                    loaded.report
                );
                checked += 1;
            }
            Err(error) => panic!("{error}"),
        }
    }
    assert!(checked > 0, "no documents found in {}", directory.display());
}

#[test]
fn a_missing_file_reports_the_path_it_looked_for() {
    let error = load(Path::new("does/not/exist.terrain.ron")).expect_err("refused");
    assert!(matches!(error, LoadError::Io { .. }));
    assert!(error.to_string().contains("does/not/exist.terrain.ron"));
}

//! The no-regression checkpoint for the scene package.
//!
//! Format version two adds optional secondary sections and nothing else. That
//! claim is worth a test rather than a sentence, because the whole point of
//! shipping the format before the content is to be able to say afterwards —
//! when a flower first appears and the image changes — that the *format* was
//! not what moved it.
//!
//! What is asserted here is the tuned half: with no secondary content, every
//! binary the package writes must be byte-for-byte what it always was, and the
//! header's tuned sections must be unchanged text.

use std::path::Path;

use glam::Vec2;
use terrain_cycles::cycles::{CyclesScene, RenderSettings};
use terrain_cycles::secondary::{
    CYCLES_SCENE_FORMAT_VERSION, CurveSpan, Instance, MaterialBinding, Prototype, PrototypeFamily,
    RibbonSpan, RibbonVertex, SecondaryGeometry, Visibility,
};
use terrain_generators::field::WorldField;
use terrain_generators::page::Page;
use terrain_generators::scene::GrassScene;
use terrain_generators::style::GrassParams;

/// A small scene, written to a scratch directory.
fn write_scene(directory: &Path, secondary: SecondaryGeometry) -> String {
    let params = GrassParams::default();
    let field = WorldField::lit_by(params.seed, params.light);
    let page = Page::new(Vec2::new(0.0, 0.0), 64, 64);
    let grass = GrassScene::build(page, &field, &params);
    let mut scene = CyclesScene::build(&grass, &field, RenderSettings::default());
    scene.secondary = secondary;
    scene.write(directory).expect("the scene writes");
    std::fs::read_to_string(directory.join("scene.json")).expect("a header")
}

fn scratch(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("groundwork-v2-{name}"));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

#[test]
fn the_header_declares_the_format_version() {
    // Mandatory, not advisory. A reader that finds a number it does not know
    // must refuse: the failure mode of reading a newer package with an older
    // reader is not an error, it is a plausible picture with a section silently
    // missing from it.
    let directory = scratch("version");
    let header = write_scene(&directory, SecondaryGeometry::default());
    assert!(
        header.contains(&format!("\"version\": {CYCLES_SCENE_FORMAT_VERSION}")),
        "{header}"
    );
    assert_eq!(CYCLES_SCENE_FORMAT_VERSION, 2);
}

#[test]
fn an_empty_secondary_section_writes_no_secondary_files() {
    // The rule the ground state planes already follow: a manifest that names a
    // file the reader then finds empty is worse than a manifest that names
    // nothing, because the reader trusts the manifest and stops.
    let directory = scratch("empty-files");
    write_scene(&directory, SecondaryGeometry::default());
    for name in [
        "secondary-ribbons.bin",
        "secondary-indices.bin",
        "secondary-curves.bin",
        "instances.bin",
    ] {
        assert!(
            !directory.join(name).exists(),
            "{name} was written for an empty section"
        );
    }
    // And the tuned files are all still there.
    for name in ["blades.bin", "attributes.bin", "ground.bin", "scene.json"] {
        assert!(directory.join(name).exists(), "{name} is missing");
    }
}

#[test]
fn the_section_is_declared_even_when_it_is_empty() {
    // An absent section and an empty one mean the same thing to a careful
    // reader and different things to a careless one, and the careless reading
    // silently drops content the day the section stops being empty.
    let directory = scratch("declared");
    let header = write_scene(&directory, SecondaryGeometry::default());
    assert!(header.contains("\"secondary\""), "{header}");
    assert!(header.contains("\"prototypes\": []"), "{header}");
    assert!(header.contains("\"count\": 0"), "{header}");
}

#[test]
fn secondary_content_does_not_touch_the_tuned_buffers() {
    // The checkpoint. Adding geometry to the new section must not move a single
    // byte of the tuned blades, the ground, or any weight plane — if it did,
    // every visual comparison across this phase would be measuring two changes
    // at once.
    let bare = scratch("bare");
    write_scene(&bare, SecondaryGeometry::default());

    let filled = scratch("filled");
    write_scene(&filled, populated());

    for name in ["blades.bin", "attributes.bin", "ground.bin"] {
        let a = std::fs::read(bare.join(name)).unwrap_or_else(|_| panic!("{name} in bare"));
        let b = std::fs::read(filled.join(name)).unwrap_or_else(|_| panic!("{name} in filled"));
        assert_eq!(a, b, "{name} moved when secondary content was added");
    }
}

#[test]
fn a_populated_section_writes_every_table_it_declares() {
    let directory = scratch("populated");
    let header = write_scene(&directory, populated());
    for name in [
        "secondary-ribbons.bin",
        "secondary-indices.bin",
        "secondary-curves.bin",
        "instances.bin",
    ] {
        assert!(directory.join(name).exists(), "{name} was not written");
    }

    // Every binary's length equals its declared count times its declared
    // stride. A package that disagreed with itself here would produce geometry
    // that is subtly wrong everywhere rather than failing.
    let instances = std::fs::metadata(directory.join("instances.bin")).expect("instances");
    assert_eq!(instances.len() as usize, Instance::STRIDE);
    let ribbons = std::fs::metadata(directory.join("secondary-ribbons.bin")).expect("ribbons");
    assert_eq!(ribbons.len() as usize, 4 * RibbonVertex::STRIDE);

    assert!(header.contains("\"count\": 1"), "{header}");
    assert!(header.contains("stone.rounded.v1"), "{header}");
}

#[test]
fn an_invalid_section_is_refused_rather_than_written() {
    // Refused in Rust, before Blender ever opens it. An index out of range does
    // not crash a renderer; it draws a different triangle, and the render
    // succeeds.
    let directory = scratch("invalid");
    let params = GrassParams::default();
    let field = WorldField::lit_by(params.seed, params.light);
    let page = Page::new(Vec2::new(0.0, 0.0), 32, 32);
    let grass = GrassScene::build(page, &field, &params);
    let mut scene = CyclesScene::build(&grass, &field, RenderSettings::default());
    scene.secondary = SecondaryGeometry {
        materials: vec![MaterialBinding {
            appearance: "surface.stone".into(),
            shader: "stone".into(),
        }],
        ribbons: vec![RibbonSpan {
            vertex_offset: 0,
            vertex_count: 12,
            index_offset: 0,
            index_count: 0,
            material: 0,
            visibility: Visibility::Camera,
        }],
        ..Default::default()
    };
    let error = scene
        .write(&directory)
        .expect_err("a bad package is refused");
    assert!(error.to_string().contains("spans vertices"), "{error}");
}

/// A section with one of everything in it.
fn populated() -> SecondaryGeometry {
    let vertex = |x: f32, along: f32, across: f32| RibbonVertex {
        position: [x, 0.0, 0.0],
        normal: [0.0, 0.0, 1.0],
        along,
        across,
    };
    SecondaryGeometry {
        materials: vec![
            MaterialBinding {
                appearance: "plant.flower_petal".into(),
                shader: "petal".into(),
            },
            MaterialBinding {
                appearance: "surface.stone".into(),
                shader: "stone".into(),
            },
        ],
        ribbon_vertices: vec![
            vertex(0.0, 0.0, -1.0),
            vertex(0.0, 0.0, 1.0),
            vertex(0.02, 1.0, -1.0),
            vertex(0.02, 1.0, 1.0),
        ],
        ribbon_indices: vec![0, 1, 2, 1, 3, 2],
        ribbons: vec![RibbonSpan {
            vertex_offset: 0,
            vertex_count: 4,
            index_offset: 0,
            index_count: 6,
            material: 0,
            visibility: Visibility::Camera,
        }],
        curve_points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.1], [0.01, 0.0, 0.2]],
        curves: vec![CurveSpan {
            point_offset: 0,
            point_count: 3,
            radius_root_m: 0.002,
            radius_tip_m: 0.0012,
            material: 0,
            visibility: Visibility::Camera,
        }],
        prototypes: vec![Prototype {
            key: "stone.rounded.v1".into(),
            family: PrototypeFamily::Superellipsoid,
            semi_axes_m: [1.0, 0.85, 0.6],
            exponents: [0.9, 0.9],
            deformation: vec![[0.05, 2.0, 0.3]],
            clips: Vec::new(),
            tessellation: [12, 16],
            material: 1,
            unit_height_m: 1.2,
        }],
        instances: vec![Instance {
            prototype: 0,
            material_variant: 0,
            visibility: Visibility::Halo,
            translation: [0.3, 0.2, -0.01],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            scale: [0.05, 0.04, 0.03],
            tint: [1.0, 0.98, 0.95],
            variation: 0.25,
        }],
    }
}

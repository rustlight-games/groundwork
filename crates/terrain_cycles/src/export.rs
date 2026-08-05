//! Writing a [`TerrainScene`] out as a package.
//!
//! ## The only input is the scene
//!
//! This function takes a `TerrainScene` and a render profile, and nothing else.
//! No `WorldField`, no `BakeParams`, no grass. That is the whole point of the
//! commit that introduced it: while the exporter took the generator's own types,
//! adding a second kind of content meant teaching the exporter about it, and the
//! path tracer's package format grew a section per plant.
//!
//! Now the scene is the interface. A wildflower is a ribbon and a curve; a rock
//! is an instance of a prototype. The exporter has never heard of either.
//!
//! ## Everything crosses the mirror exactly once
//!
//! The game's projection is left-handed, so a physical renderer is handed a
//! reflected world — see [`terrain_scene::projection`]. Every position, the
//! ground grid's origin, the bounds and the sun's bearing go through the swap
//! here, in one place, and nothing downstream reflects anything again.
//!
//! Getting that wrong does not produce an obviously broken picture. It produces
//! a meadow lit from the wrong side, which looks entirely plausible until it is
//! held next to one that is not.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use terrain_scene::mark::{SceneMark, SceneMaterialIndex};
use terrain_scene::projection::ScenePoint;
use terrain_scene::scene::TerrainScene;

use crate::aov::OutputRequest;
use crate::package::*;

/// How finely a ribbon is described to the path tracer.
///
/// Seven cross-sections. Fewer and a bent blade shows its facets against the
/// sky; more and the vertex count climbs faster than the picture improves — the
/// silhouette of a quarter-metre blade is settled by about five.
pub const SEGMENTS_PER_RIBBON: usize = 7;

/// Vertices per cross-section. A triangle, so a ribbon has a front, a back and a
/// ridge.
pub const VERTICES_PER_SEGMENT: usize = 3;

/// The attributes every mark carries into the shader.
///
/// Named rather than positional, and written into the manifest, so a shader
/// binds by name. Renumbering these silently is how a maturity ends up driving a
/// moisture.
pub const MARK_ATTRIBUTES: [&str; 5] = ["maturity", "moisture", "exposure", "tint", "variation"];

/// What to ask the tracer for.
#[derive(Clone, Debug)]
pub struct RenderProfile {
    pub samples: u32,
    pub denoise: bool,
    /// `"GPU"` or `"CPU"`.
    pub device: String,
    /// Blender view transform. `"Standard"` keeps the render linear-to-sRGB.
    pub view_transform: String,
    pub sun_elevation: f32,
    /// Sun bearing in *world* space. Reflected on the way out.
    pub sun_azimuth: f32,
    pub sun_angle: f32,
    pub sun_strength: f32,
    pub sun_colour: [f32; 3],
    pub sky_strength: f32,
    pub sky_colour: [f32; 3],
    pub outputs: OutputRequest,
    /// Multiplies every exported ribbon width.
    ///
    /// The honest fudge, and it is smaller than it was. Ribbon widths now arrive
    /// in metres rather than in cache pixels, so this no longer has to undo a
    /// unit mismatch — what remains of it is a deliberate mip parameter. See
    /// `plate::blade_width_for`.
    pub width_scale: f32,
}

impl Default for RenderProfile {
    fn default() -> Self {
        Self {
            samples: 256,
            denoise: true,
            device: "GPU".to_string(),
            view_transform: "Standard".to_string(),
            // The elevation the whole renderer is built around, and the lowest
            // it supports: below this a mark shades ground more than one and a
            // half times its own height away and the halo grows faster than the
            // page.
            sun_elevation: 35.0f32.to_radians(),
            sun_azimuth: 125.0f32.to_radians(),
            // Six times life size, which is the same licence the art takes
            // elsewhere: a literal half-degree sun puts a hard edge on every
            // shadow and the field fills with black confetti. What it must not
            // become is *soft* — a wide sun is a second fill light, and fill is
            // what flattens a canopy.
            sun_angle: 3.0f32.to_radians(),
            sun_strength: 18.0,
            sun_colour: [1.0, 0.92, 0.72],
            sky_strength: 1.15,
            sky_colour: [0.30, 0.44, 0.72],
            outputs: OutputRequest::beauty(),
            width_scale: 1.0,
        }
    }
}

/// Write a scene as a package, returning the manifest's path.
pub fn write_package(
    scene: &TerrainScene,
    profile: &RenderProfile,
    directory: &Path,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(directory.join("ground"))?;
    std::fs::create_dir_all(directory.join("geometry"))?;

    let ground = write_ground(scene, directory)?;
    let geometry = write_geometry(scene, profile, directory)?;

    let materials = scene
        .materials
        .iter()
        .enumerate()
        .map(|(index, binding)| MaterialBindingManifest {
            index: SceneMaterialIndex(index as u16),
            appearance: binding.appearance.as_str().to_string(),
        })
        .collect();

    let projection = scene.request.projection;
    let bounds = scene.request.bounds;
    // The bounds cross the mirror like everything else, and a swap exchanges the
    // two axes — so the minimum corner of the reflected rectangle is built from
    // both original corners rather than from the original minimum.
    let reflected = |u: f64, v: f64| [v, u];
    let a = reflected(bounds.min.u_m, bounds.min.v_m);
    let b = reflected(bounds.max.u_m, bounds.max.v_m);

    let manifest = ScenePackageManifest {
        package_version: PACKAGE_VERSION,
        scene_digest: scene.fingerprint(),
        document_digest: scene.document_digest,
        bounds: [
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        ],
        ground,
        geometry,
        prototypes: Vec::new(),
        materials,
        camera: camera_for(scene),
        lighting: LightingManifest {
            sun_elevation: profile.sun_elevation,
            // Reflected across `u = v`, which sends a bearing to its complement.
            sun_azimuth: projection.bearing_to_right_handed(profile.sun_azimuth as f64) as f32,
            sun_angle: profile.sun_angle,
            sun_strength: profile.sun_strength,
            sun_colour: profile.sun_colour,
            sky_strength: profile.sky_strength,
            sky_colour: profile.sky_colour,
        },
        resolution: scene.request.output_size,
        samples: profile.samples,
        denoise: profile.denoise,
        device: profile.device.clone(),
        view_transform: profile.view_transform.clone(),
        outputs: profile
            .outputs
            .names()
            .iter()
            .map(|n| n.to_string())
            .collect(),
    };

    let path = directory.join("manifest.json");
    std::fs::write(&path, manifest.to_json())?;
    Ok(path)
}

/// The camera that photographs a scene exactly as its projection would draw it.
///
/// Framed from [`terrain_scene::scene::SceneRequest::viewport`] rather than from
/// the ground bounds, and that is the whole difference between "this render is
/// of that ground" and "this render is composed like this". A ground rectangle
/// projects to a diamond; deriving the frame from its bounding box means the
/// camera always tightly encloses the diamond, so there is no way to ask for
/// margins, and no way to put the subject tile anywhere but dead centre. The
/// viewport is the rectangle a caller actually wants photographed.
fn camera_for(scene: &TerrainScene) -> CameraManifest {
    let projection = scene.request.projection;
    // A physical right-handed camera above the ground. The world arrives
    // reflected, which is what makes this basis agree with the projection.
    let right = normalise([-1.0, 1.0, 0.0]);
    let up = normalise([-1.0, -1.0, 2.0]);
    let backward = normalise(cross(right, up));

    let viewport = scene.request.viewport;
    // `screen.x` is a dot with `r`, whose length is √2, so a screen-metre span
    // of `s` is a world span of `s / √2` along `r̂`.
    let world_width = viewport.width_m() / 2.0f64.sqrt();
    let world_height = viewport.height_m() / (6.0f64.sqrt() / 2.0);

    let resolution = scene.request.output_size;
    // Blender derives the vertical extent from the horizontal one as
    // `ortho_scale · (res_y · aspect_y) / (res_x · aspect_x)`. Solving for the
    // aspect that yields `world_height` cancels the resolution entirely, which
    // is the check that this is a property of the projection rather than of how
    // big the render happens to be.
    let pixel_aspect_y =
        (world_height * resolution[0] as f64) / (world_width * resolution[1] as f64);

    // The ground under the middle of the frame, reflected like everything else.
    let centre = projection.unproject_ground(viewport.centre());
    let target = [centre.v_m, centre.u_m, 0.0];
    let distance = 40.0 + scene.canopy_ceiling_m() * 4.0;

    CameraManifest {
        location: [
            target[0] + backward[0] * distance,
            target[1] + backward[1] * distance,
            target[2] + backward[2] * distance,
        ],
        basis: [right, up, backward],
        ortho_scale: world_width,
        pixel_aspect_y,
    }
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length == 0.0 {
        return v;
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Reflect a scene point into the renderer's right-handed space.
fn to_renderer(point: ScenePoint) -> [f32; 3] {
    [point.v_m as f32, point.u_m as f32, point.z_m as f32]
}

fn write_ground(scene: &TerrainScene, directory: &Path) -> io::Result<GroundManifest> {
    let ground = &scene.ground;
    write_f32(
        &directory.join("ground/elevation.bin"),
        ground.elevation.iter().copied(),
    )?;

    let microrelief_path = if ground.microrelief.iter().any(|v| *v != 0.0) {
        write_f32(
            &directory.join("ground/microrelief.bin"),
            ground.microrelief.iter().copied(),
        )?;
        Some("ground/microrelief.bin".to_string())
    } else {
        None
    };

    let material_weights_path = if ground.material_channels.is_empty() {
        None
    } else {
        write_f32(
            &directory.join("ground/material_weights.bin"),
            ground
                .material_channels
                .iter()
                .flat_map(|c| c.weights.iter().copied()),
        )?;
        Some("ground/material_weights.bin".to_string())
    };

    let modifiers_path = if ground.modifier_channels.is_empty() {
        None
    } else {
        write_f32(
            &directory.join("ground/modifiers.bin"),
            ground
                .modifier_channels
                .iter()
                .flat_map(|c| c.values.iter().copied()),
        )?;
        Some("ground/modifiers.bin".to_string())
    };

    Ok(GroundManifest {
        rows: ground.rows,
        columns: ground.columns,
        spacing_m: ground.spacing_m,
        // The grid's origin crosses the mirror too.
        origin: [ground.origin.v_m, ground.origin.u_m],
        elevation_path: "ground/elevation.bin".to_string(),
        microrelief_path,
        material_weights_path,
        material_planes: ground
            .material_channels
            .iter()
            .map(|c| c.material)
            .collect(),
        modifiers_path,
        modifier_names: ground
            .modifier_channels
            .iter()
            .map(|c| format!("modifier_{}", c.channel.0))
            .collect(),
    })
}

/// Write the geometry, one buffer per material.
///
/// Grouped by material rather than by mark, because Blender builds one object
/// per material and reading a buffer that interleaves them would mean sorting on
/// the Python side — which is both slower and a second place for the order to be
/// decided.
fn write_geometry(
    scene: &TerrainScene,
    profile: &RenderProfile,
    directory: &Path,
) -> io::Result<Vec<GeometryManifest>> {
    let mut manifests = Vec::new();

    for (index, _) in scene.materials.iter().enumerate() {
        let material = SceneMaterialIndex(index as u16);
        let ribbons: Vec<&SceneMark> = scene
            .marks
            .iter()
            .filter(|mark| mark.material() == material && matches!(mark, SceneMark::Ribbon(_)))
            .collect();
        if ribbons.is_empty() {
            continue;
        }

        let path = format!("geometry/ribbons-{index:03}.bin");
        let attributes_path = format!("geometry/ribbons-{index:03}-attributes.bin");
        let mut points: Vec<f32> =
            Vec::with_capacity(ribbons.len() * SEGMENTS_PER_RIBBON * VERTICES_PER_SEGMENT * 3);
        let mut attributes: Vec<f32> = Vec::with_capacity(ribbons.len() * MARK_ATTRIBUTES.len());

        for mark in &ribbons {
            let SceneMark::Ribbon(ribbon) = mark else {
                continue;
            };
            tessellate_ribbon(ribbon, profile.width_scale, &mut points);
            let a = ribbon.attributes;
            attributes.extend_from_slice(&[
                a.maturity,
                a.moisture,
                a.exposure,
                a.tint,
                a.variation,
            ]);
        }

        write_f32(&directory.join(&path), points.iter().copied())?;
        write_f32(
            &directory.join(&attributes_path),
            attributes.iter().copied(),
        )?;

        manifests.push(GeometryManifest {
            kind: GeometryKind::Ribbons,
            path,
            count: ribbons.len(),
            floats_per_item: SEGMENTS_PER_RIBBON * VERTICES_PER_SEGMENT * 3,
            segments: SEGMENTS_PER_RIBBON,
            vertices_per_segment: VERTICES_PER_SEGMENT,
            attributes_path: Some(attributes_path),
            attribute_names: MARK_ATTRIBUTES.iter().map(|n| n.to_string()).collect(),
            material,
        });
    }

    Ok(manifests)
}

/// Turn one ribbon into cross-sections.
///
/// A simple arc walk: the centreline bends from vertical toward its azimuth, and
/// each cross-section is three points across the width. Deliberately simpler
/// than the rasteriser's walk — no kink, no fork, no twist — because this is the
/// *generic* exporter and those are grass-specific morphology that belongs in a
/// recipe's own tessellation.
///
/// The grass path still goes through `CyclesScene`, which walks the full
/// vocabulary. This is what a second kind of content gets for free.
fn tessellate_ribbon(
    ribbon: &terrain_scene::mark::RibbonMark,
    width_scale: f32,
    into: &mut Vec<f32>,
) {
    let root = ribbon.root;
    let geometry = &ribbon.geometry;
    let (sin_a, cos_a) = geometry.azimuth_rad.sin_cos();

    for segment in 0..SEGMENTS_PER_RIBBON {
        let s = segment as f32 / (SEGMENTS_PER_RIBBON - 1).max(1) as f32;
        // The centreline: an arc leaning from vertical toward the azimuth.
        let angle = geometry.bend_rad * s;
        let along = geometry.length_m * s;
        let lean = along * angle.sin();
        let rise = along * angle.cos();
        let centre = ScenePoint::new(
            root.u_m + (lean * cos_a) as f64,
            root.v_m + (lean * sin_a) as f64,
            root.z_m + rise as f64,
        );

        // Width tapers from the root to the tip, never below the tip width.
        let half = (geometry.width_m * (1.0 - s) + geometry.tip_width_m * s) * width_scale;
        // Across the ribbon, perpendicular to its lean on the ground.
        let across = ((-sin_a * half) as f64, (cos_a * half) as f64);

        for offset in [-1.0f64, 0.0, 1.0] {
            let point = ScenePoint::new(
                centre.u_m + across.0 * offset,
                centre.v_m + across.1 * offset,
                // The middle vertex stands proud, which is the ridge.
                centre.z_m
                    + if offset == 0.0 {
                        (half * geometry.ridge) as f64
                    } else {
                        0.0
                    },
            );
            into.extend_from_slice(&to_renderer(point));
        }
    }
}

fn write_f32(path: &Path, values: impl Iterator<Item = f32>) -> io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    for value in values {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::coords::WorldPoint;
    use terrain_core::digest::Fingerprint;
    use terrain_core::ids::AppearanceKey;
    use terrain_scene::mark::*;
    use terrain_scene::scene::{SceneBuilder, SceneRequest};

    fn scene() -> TerrainScene {
        let request = SceneRequest::square(WorldPoint::ORIGIN, 2.0, 96.0);
        let mut builder = SceneBuilder::new(request, Fingerprint::from_u128(0x99), 1);
        let material = builder.bind_material(SceneMaterialBinding {
            appearance: AppearanceKey::new("plant.grass_blade").expect("valid"),
            terrain_material: None,
        });
        for i in 0..8u64 {
            let id = MarkId(i);
            let root = ScenePoint::new(i as f64 * 0.1, 0.0, 0.0);
            builder.push_mark(SceneMark::Ribbon(RibbonMark {
                stable_id: id,
                anchor: AnchorIndex::UNGROUPED,
                order: PainterOrder::new(Stratum::Canopy, i as f64, 0, id),
                material,
                root,
                geometry: RibbonGeometry::default(),
                attributes: MarkAttributes::default(),
                bounds: Aabb3::around(root, 0.3),
            }));
        }
        builder.build()
    }

    fn export(directory: &Path) -> String {
        let path = write_package(&scene(), &RenderProfile::default(), directory)
            .expect("the package writes");
        std::fs::read_to_string(path).expect("readable")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("terrain-cycles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_package_writes_every_buffer_its_manifest_names() {
        // The check that makes the length check downstream meaningful: a
        // manifest naming a file that is not there is a renderer that fails at
        // the first read rather than at the first wrong pixel.
        let dir = scratch("buffers");
        let json = export(&dir);
        for line in json.lines() {
            for token in line.split('"') {
                if token.ends_with(".bin") {
                    assert!(
                        dir.join(token).exists(),
                        "the manifest names {token}, which was not written"
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_buffer_is_exactly_as_long_as_the_manifest_claims() {
        // A buffer one element short is a renderer reading the tail of one mark
        // as the head of the next, for every mark after the short one.
        let dir = scratch("lengths");
        let scene = scene();
        write_package(&scene, &RenderProfile::default(), &dir).expect("writes");

        let ribbons = dir.join("geometry/ribbons-000.bin");
        let expected = scene.mark_count() * SEGMENTS_PER_RIBBON * VERTICES_PER_SEGMENT * 3 * 4;
        assert_eq!(
            std::fs::metadata(&ribbons).expect("exists").len() as usize,
            expected
        );

        let attributes = dir.join("geometry/ribbons-000-attributes.bin");
        assert_eq!(
            std::fs::metadata(&attributes).expect("exists").len() as usize,
            scene.mark_count() * MARK_ATTRIBUTES.len() * 4
        );

        let elevation = dir.join("ground/elevation.bin");
        assert_eq!(
            std::fs::metadata(&elevation).expect("exists").len() as usize,
            scene.ground.sample_count() * 4
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_package_carries_the_scenes_own_digest() {
        let dir = scratch("digest");
        let scene = scene();
        write_package(&scene, &RenderProfile::default(), &dir).expect("writes");
        let json = std::fs::read_to_string(dir.join("manifest.json")).expect("readable");
        assert!(
            json.contains(&format!("\"scene_digest\": \"{}\"", scene.fingerprint())),
            "{json}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sun_crosses_the_mirror_exactly_once() {
        // A blade reflected while its sun is not is lit from the wrong side, and
        // it looks entirely plausible. The bearing goes to its complement.
        let dir = scratch("sun");
        let profile = RenderProfile {
            sun_azimuth: 0.0,
            ..RenderProfile::default()
        };
        write_package(&scene(), &profile, &dir).expect("writes");
        let json = std::fs::read_to_string(dir.join("manifest.json")).expect("readable");
        // A world bearing of zero reflects to π/2.
        assert!(
            json.contains("\"sun_azimuth\": 1.570796"),
            "the sun did not cross the mirror:\n{json}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn geometry_is_grouped_by_material() {
        // Blender builds one object per material, and a buffer that interleaved
        // them would mean sorting on the Python side — slower, and a second
        // place for the order to be decided.
        let request = SceneRequest::square(WorldPoint::ORIGIN, 2.0, 96.0);
        let mut builder = SceneBuilder::new(request, Fingerprint::from_u128(0), 1);
        let grass = builder.bind_material(SceneMaterialBinding {
            appearance: AppearanceKey::new("plant.grass_blade").expect("valid"),
            terrain_material: None,
        });
        let flower = builder.bind_material(SceneMaterialBinding {
            appearance: AppearanceKey::new("plant.wildflower_head").expect("valid"),
            terrain_material: None,
        });
        for (i, material) in [grass, grass, flower].into_iter().enumerate() {
            let id = MarkId(i as u64);
            let root = ScenePoint::new(i as f64 * 0.1, 0.0, 0.0);
            builder.push_mark(SceneMark::Ribbon(RibbonMark {
                stable_id: id,
                anchor: AnchorIndex::UNGROUPED,
                order: PainterOrder::new(Stratum::Canopy, i as f64, 0, id),
                material,
                root,
                geometry: RibbonGeometry::default(),
                attributes: MarkAttributes::default(),
                bounds: Aabb3::around(root, 0.3),
            }));
        }
        let dir = scratch("materials");
        write_package(&builder.build(), &RenderProfile::default(), &dir).expect("writes");
        let json = std::fs::read_to_string(dir.join("manifest.json")).expect("readable");
        assert!(json.contains("ribbons-000.bin") && json.contains("ribbons-001.bin"));
        assert!(json.contains("\"appearance\": \"plant.wildflower_head\""));
        // Two grass ribbons in the first buffer, one flower in the second.
        assert!(json.contains("\"count\": 2"), "{json}");
        assert!(json.contains("\"count\": 1"), "{json}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_exporter_never_names_grass() {
        // The claim this whole module exists to make. It takes a scene and a
        // profile; a wildflower is a ribbon and a rock is an instance, and the
        // exporter has never heard of either.
        // Everything above the test module, with comments stripped: the tests
        // legitimately name the generator's types to build a scene, and the
        // banned list below would otherwise match itself.
        let source = include_str!("export.rs");
        let code: String = source
            .split("#[cfg(test)]")
            .next()
            .expect("there is code before the tests")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in ["WorldField", "BakeParams", "GrassScene", "Stroke"] {
            assert!(
                !code.contains(banned),
                "the generic exporter mentions `{banned}`"
            );
        }
    }

    #[test]
    fn a_ribbons_tessellation_starts_at_its_root_and_rises() {
        let mut points = Vec::new();
        let ribbon = RibbonMark {
            stable_id: MarkId(0),
            anchor: AnchorIndex::UNGROUPED,
            order: PainterOrder::new(Stratum::Canopy, 0.0, 0, MarkId(0)),
            material: SceneMaterialIndex(0),
            root: ScenePoint::new(1.0, 2.0, 0.0),
            geometry: RibbonGeometry {
                bend_rad: 0.0,
                ..RibbonGeometry::default()
            },
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(ScenePoint::default(), 1.0),
        };
        tessellate_ribbon(&ribbon, 1.0, &mut points);
        assert_eq!(points.len(), SEGMENTS_PER_RIBBON * VERTICES_PER_SEGMENT * 3);
        // Reflected, so the root arrives as (v, u, z) — and the first vertex of
        // a cross-section sits half a width to one side of the centreline.
        let half = ribbon.geometry.width_m;
        assert!((points[0] - (2.0 - half)).abs() < 1.0e-5, "{}", points[0]);
        assert!((points[1] - 1.0).abs() < 1.0e-5, "{}", points[1]);
        // The middle vertex is on the centreline and stands proud by the ridge.
        assert!((points[3] - 2.0).abs() < 1.0e-5, "{}", points[3]);
        assert!(points[5] > 0.0, "the ridge did not lift the middle vertex");
        // The last cross-section stands at the blade's full length.
        let last = points.len() - 9;
        assert!(
            (points[last + 2] - ribbon.geometry.length_m).abs() < 1.0e-4,
            "the tip is at {}",
            points[last + 2]
        );
    }
}

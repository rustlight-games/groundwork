//! The scene, as a directory a renderer can read.
//!
//! ## Why a package and not a format
//!
//! The old export was four files with fixed names: `scene.json`, `blades.bin`,
//! `attributes.bin`, `ground.bin`. It worked, and it could only ever describe
//! grass — the word "blades" is in the filename, the attribute layout is four
//! floats a blade, and there is nowhere for a rock to go.
//!
//! What replaces it is a *manifest plus a set of named buffers*. Adding
//! wildflowers means adding a geometry buffer and a material binding, not
//! renaming a file that the Python side has hard-coded.
//!
//! ```text
//! scene/
//!   manifest.json          what is here, how much, and in what layout
//!   ground/
//!     elevation.bin        f32, row-major
//!     material_weights.bin f32, one plane per material
//!     modifiers.bin        f32, one plane per channel
//!   geometry/
//!     ribbons-000.bin      vertices, then attributes
//!     curves-000.bin
//!     analytic-000.bin
//!     instances-000.bin
//!   prototypes/
//!     prototype-manifest.json
//!   materials/
//!     bindings.json        appearance key per material index
//! ```
//!
//! ## Every buffer declares its own layout
//!
//! A `.bin` is little-endian `f32` and nothing else — no header, no length, no
//! type tag. The manifest says how many elements it holds and how they are
//! grouped, and the reader checks the file's length against that before reading
//! a byte.
//!
//! That check is the whole reason the count is in the manifest rather than
//! implied. A buffer one element short is not a crash: it is a renderer reading
//! the tail of one blade as the head of the next, for every blade after the
//! short one, producing geometry that is subtly and inexplicably wrong.
//!
//! ## The digest travels with it
//!
//! [`ScenePackageManifest::scene_digest`] is the scene's own fingerprint. So a
//! render can be attributed to a scene without the scene being kept, and two
//! halves of a training pair can be checked for having come from one meadow
//! rather than assumed to have.

use terrain_core::digest::Fingerprint;
use terrain_scene::mark::SceneMaterialIndex;

/// The version of this package layout.
///
/// Read by the Python side before anything else. A package from a future
/// version is refused by its number rather than by whatever confusing shape
/// mismatch its buffers happen to produce.
pub const PACKAGE_VERSION: u32 = 2;

/// What one geometry buffer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GeometryKind {
    /// Tapered ribbons: blades, leaves, straps.
    Ribbons,
    /// Round curves: stems, twigs.
    Curves,
    /// Analytic shapes lying on the ground: pebbles, scuffs.
    Analytic,
    /// Prototype placements.
    Instances,
}

impl GeometryKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ribbons => "ribbons",
            Self::Curves => "curves",
            Self::Analytic => "analytic",
            Self::Instances => "instances",
        }
    }
}

/// One buffer of geometry.
#[derive(Clone, Debug)]
pub struct GeometryManifest {
    pub kind: GeometryKind,
    /// Path relative to the package root.
    pub path: String,
    /// How many things this buffer describes.
    pub count: usize,
    /// Floats per thing in the position buffer.
    pub floats_per_item: usize,
    /// Cross-sections each item is described with, where that applies.
    pub segments: usize,
    /// Vertices per cross-section.
    pub vertices_per_segment: usize,
    /// Path to the per-item attribute buffer, if there is one.
    pub attributes_path: Option<String>,
    /// Named attributes, in the order they appear per item.
    ///
    /// Named rather than positional, because a shader binds them by name and a
    /// renumbering that nothing reports is how a maturity ends up driving a
    /// moisture.
    pub attribute_names: Vec<String>,
    /// Which material binding every item in this buffer uses.
    pub material: SceneMaterialIndex,
}

impl GeometryManifest {
    /// How many floats the position buffer must hold.
    pub fn position_floats(&self) -> usize {
        self.count * self.floats_per_item
    }

    /// How many floats the attribute buffer must hold.
    pub fn attribute_floats(&self) -> usize {
        self.count * self.attribute_names.len()
    }
}

/// The ground grid, as buffers.
#[derive(Clone, Debug)]
pub struct GroundManifest {
    pub rows: u32,
    pub columns: u32,
    /// Metres between lattice points.
    pub spacing_m: f64,
    /// World position of sample `(0, 0)`, after the right-handed swap.
    pub origin: [f64; 2],
    pub elevation_path: String,
    pub microrelief_path: Option<String>,
    /// One plane per material, in the order the planes are stored.
    pub material_weights_path: Option<String>,
    /// Which *document* material each plane carries.
    ///
    /// A [`terrain_core::MaterialIndex`] rather than a [`SceneMaterialIndex`],
    /// and the distinction is real rather than pedantic: the ground's planes are
    /// what the terrain is *composed of*, while a mark's material is what it is
    /// *drawn as*. A blade of grass on ground that is seventy percent grass and
    /// thirty percent dirt is still made of grass, and the two indices are
    /// answers to different questions.
    pub material_planes: Vec<terrain_core::ids::MaterialIndex>,
    pub modifiers_path: Option<String>,
    pub modifier_names: Vec<String>,
}

impl GroundManifest {
    /// Samples in one plane. Edge-anchored, so one more than the cell count.
    pub fn samples(&self) -> usize {
        (self.rows as usize + 1) * (self.columns as usize + 1)
    }
}

/// One renderer-side material.
#[derive(Clone, Debug)]
pub struct MaterialBindingManifest {
    pub index: SceneMaterialIndex,
    /// The stable appearance key: `plant.grass_blade`, `surface.dirt_compacted`.
    ///
    /// **This is what the Python side dispatches on.** It is a renderer-side
    /// implementation id and deliberately not a material-weight identity: a
    /// blade of grass growing on ground that is seventy percent grass and thirty
    /// percent dirt is still made of grass.
    pub appearance: String,
}

/// One prototype mesh the scene refers to.
#[derive(Clone, Debug)]
pub struct PrototypeManifest {
    pub index: u16,
    /// The recipe that builds it.
    pub recipe: String,
    pub seed: u64,
    /// A bound on the prototype's own geometry, metres.
    pub radius_m: f32,
    /// Path to the mesh, when one was written rather than named.
    pub path: Option<String>,
}

/// How the camera sees the scene.
#[derive(Clone, Copy, Debug)]
pub struct CameraManifest {
    pub location: [f64; 3],
    /// Right, up and backward — the columns of the rotation matrix.
    pub basis: [[f64; 3]; 3],
    /// World metres the horizontal axis of the frame spans.
    pub ortho_scale: f64,
    /// The projection's anisotropy, as a pixel taller than it is wide.
    ///
    /// See [`terrain_scene::projection`]: no camera transform can express the
    /// 2/√3 stretch, so it is carried here.
    pub pixel_aspect_y: f64,
}

/// Where the light comes from.
#[derive(Clone, Copy, Debug)]
pub struct LightingManifest {
    /// Sun elevation above the horizon, radians.
    pub sun_elevation: f32,
    /// Sun bearing, radians, **already reflected** into the renderer's space.
    pub sun_azimuth: f32,
    /// Angular diameter, radians.
    pub sun_angle: f32,
    pub sun_strength: f32,
    pub sun_colour: [f32; 3],
    pub sky_strength: f32,
    pub sky_colour: [f32; 3],
}

/// Everything the renderer is told.
#[derive(Clone, Debug)]
pub struct ScenePackageManifest {
    pub package_version: u32,
    /// The scene's own fingerprint, so a render can be attributed without the
    /// scene being kept.
    pub scene_digest: Fingerprint,
    /// The terrain document this ultimately came from.
    pub document_digest: Fingerprint,
    /// World rectangle covered, after the right-handed swap.
    pub bounds: [[f64; 2]; 2],
    pub ground: GroundManifest,
    pub geometry: Vec<GeometryManifest>,
    pub prototypes: Vec<PrototypeManifest>,
    pub materials: Vec<MaterialBindingManifest>,
    pub camera: CameraManifest,
    pub lighting: LightingManifest,
    pub resolution: [u32; 2],
    pub samples: u32,
    pub denoise: bool,
    pub device: String,
    pub view_transform: String,
    /// Which passes to write.
    pub outputs: Vec<String>,
}

impl ScenePackageManifest {
    /// Serialise as JSON.
    ///
    /// Hand-written rather than derived, and the reason is the reader: the
    /// Python side is the only consumer, JSON is what it parses without a
    /// dependency, and a serde derive would tie the wire format to Rust field
    /// names — so renaming a field in Rust would silently break a renderer
    /// written in another language.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        push(
            &mut out,
            1,
            &format!("\"package_version\": {}", self.package_version),
        );
        push(
            &mut out,
            1,
            &format!("\"scene_digest\": \"{}\"", self.scene_digest),
        );
        push(
            &mut out,
            1,
            &format!("\"document_digest\": \"{}\"", self.document_digest),
        );
        push(
            &mut out,
            1,
            &format!(
                "\"bounds\": [[{:.6}, {:.6}], [{:.6}, {:.6}]]",
                self.bounds[0][0], self.bounds[0][1], self.bounds[1][0], self.bounds[1][1]
            ),
        );
        push(
            &mut out,
            1,
            &format!(
                "\"resolution\": [{}, {}]",
                self.resolution[0], self.resolution[1]
            ),
        );
        push(&mut out, 1, &format!("\"samples\": {}", self.samples));
        push(&mut out, 1, &format!("\"denoise\": {}", self.denoise));
        push(&mut out, 1, &format!("\"device\": \"{}\"", self.device));
        push(
            &mut out,
            1,
            &format!("\"view_transform\": \"{}\"", self.view_transform),
        );

        let outputs: Vec<String> = self.outputs.iter().map(|o| format!("\"{o}\"")).collect();
        push(
            &mut out,
            1,
            &format!("\"outputs\": [{}]", outputs.join(", ")),
        );

        // Ground.
        let ground = &self.ground;
        let mut g = String::from("{\n");
        push(&mut g, 2, &format!("\"rows\": {}", ground.rows));
        push(&mut g, 2, &format!("\"columns\": {}", ground.columns));
        push(&mut g, 2, &format!("\"samples\": {}", ground.samples()));
        push(
            &mut g,
            2,
            &format!("\"spacing_m\": {:.6}", ground.spacing_m),
        );
        push(
            &mut g,
            2,
            &format!(
                "\"origin\": [{:.6}, {:.6}]",
                ground.origin[0], ground.origin[1]
            ),
        );
        push(
            &mut g,
            2,
            &format!("\"elevation\": \"{}\"", ground.elevation_path),
        );
        push(
            &mut g,
            2,
            &optional(&ground.microrelief_path, "microrelief"),
        );
        push(
            &mut g,
            2,
            &optional(&ground.material_weights_path, "material_weights"),
        );
        push(&mut g, 2, &optional(&ground.modifiers_path, "modifiers"));
        let names: Vec<String> = ground
            .modifier_names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect();
        g.push_str(&format!(
            "        \"modifier_names\": [{}]\n    }}",
            names.join(", ")
        ));
        push(&mut out, 1, &format!("\"ground\": {g}"));

        // Geometry.
        let mut buffers = Vec::new();
        for buffer in &self.geometry {
            let names: Vec<String> = buffer
                .attribute_names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect();
            buffers.push(format!(
                "{{ \"kind\": \"{}\", \"path\": \"{}\", \"count\": {}, \
                 \"floats_per_item\": {}, \"segments\": {}, \"vertices_per_segment\": {}, \
                 \"attributes\": {}, \"attribute_names\": [{}], \"material\": {} }}",
                buffer.kind.name(),
                buffer.path,
                buffer.count,
                buffer.floats_per_item,
                buffer.segments,
                buffer.vertices_per_segment,
                match &buffer.attributes_path {
                    Some(path) => format!("\"{path}\""),
                    None => "null".into(),
                },
                names.join(", "),
                buffer.material.0,
            ));
        }
        push(
            &mut out,
            1,
            &format!("\"geometry\": [{}]", buffers.join(", ")),
        );

        // Materials.
        let bindings: Vec<String> = self
            .materials
            .iter()
            .map(|m| {
                format!(
                    "{{ \"index\": {}, \"appearance\": \"{}\" }}",
                    m.index.0, m.appearance
                )
            })
            .collect();
        push(
            &mut out,
            1,
            &format!("\"materials\": [{}]", bindings.join(", ")),
        );

        // Prototypes.
        let prototypes: Vec<String> = self
            .prototypes
            .iter()
            .map(|p| {
                format!(
                    "{{ \"index\": {}, \"recipe\": \"{}\", \"seed\": {}, \"radius_m\": {:.6}, \
                     \"path\": {} }}",
                    p.index,
                    p.recipe,
                    p.seed,
                    p.radius_m,
                    match &p.path {
                        Some(path) => format!("\"{path}\""),
                        None => "null".into(),
                    }
                )
            })
            .collect();
        push(
            &mut out,
            1,
            &format!("\"prototypes\": [{}]", prototypes.join(", ")),
        );

        // Camera.
        let c = &self.camera;
        let basis = |i: usize| {
            format!(
                "[{:.8}, {:.8}, {:.8}]",
                c.basis[i][0], c.basis[i][1], c.basis[i][2]
            )
        };
        push(
            &mut out,
            1,
            &format!(
                "\"camera\": {{ \"location\": [{:.6}, {:.6}, {:.6}], \"basis\": [{}, {}, {}], \
                 \"ortho_scale\": {:.8}, \"pixel_aspect_y\": {:.8} }}",
                c.location[0],
                c.location[1],
                c.location[2],
                basis(0),
                basis(1),
                basis(2),
                c.ortho_scale,
                c.pixel_aspect_y
            ),
        );

        // Lighting. Last, so it needs no trailing comma.
        let l = &self.lighting;
        out.push_str(&format!(
            "    \"lighting\": {{ \"sun_elevation\": {:.6}, \"sun_azimuth\": {:.6}, \
             \"sun_angle\": {:.6}, \"sun_strength\": {:.4}, \
             \"sun_colour\": [{:.4}, {:.4}, {:.4}], \"sky_strength\": {:.4}, \
             \"sky_colour\": [{:.4}, {:.4}, {:.4}] }}\n",
            l.sun_elevation,
            l.sun_azimuth,
            l.sun_angle,
            l.sun_strength,
            l.sun_colour[0],
            l.sun_colour[1],
            l.sun_colour[2],
            l.sky_strength,
            l.sky_colour[0],
            l.sky_colour[1],
            l.sky_colour[2],
        ));
        out.push_str("}\n");
        out
    }
}

fn push(out: &mut String, indent: usize, entry: &str) {
    out.push_str(&"    ".repeat(indent));
    out.push_str(entry);
    out.push_str(",\n");
}

fn optional(path: &Option<String>, name: &str) -> String {
    match path {
        Some(path) => format!("\"{name}\": \"{path}\""),
        None => format!("\"{name}\": null"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ScenePackageManifest {
        ScenePackageManifest {
            package_version: PACKAGE_VERSION,
            scene_digest: Fingerprint::from_u128(0x1234),
            document_digest: Fingerprint::from_u128(0x5678),
            bounds: [[-1.0, -2.0], [3.0, 4.0]],
            ground: GroundManifest {
                rows: 4,
                columns: 8,
                spacing_m: 0.04,
                origin: [-1.0, -2.0],
                elevation_path: "ground/elevation.bin".into(),
                microrelief_path: None,
                material_weights_path: Some("ground/material_weights.bin".into()),
                material_planes: vec![terrain_core::ids::MaterialIndex(0)],
                modifiers_path: None,
                modifier_names: vec!["vegetation_density".into()],
            },
            geometry: vec![GeometryManifest {
                kind: GeometryKind::Ribbons,
                path: "geometry/ribbons-000.bin".into(),
                count: 1000,
                floats_per_item: 63,
                segments: 7,
                vertices_per_segment: 3,
                attributes_path: Some("geometry/ribbons-000-attributes.bin".into()),
                attribute_names: vec!["maturity".into(), "moisture".into()],
                material: SceneMaterialIndex(0),
            }],
            prototypes: Vec::new(),
            materials: vec![MaterialBindingManifest {
                index: SceneMaterialIndex(0),
                appearance: "plant.grass_blade".into(),
            }],
            camera: CameraManifest {
                location: [0.0, 0.0, 40.0],
                basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                ortho_scale: 4.0,
                pixel_aspect_y: 1.1547,
            },
            lighting: LightingManifest {
                sun_elevation: 0.61,
                sun_azimuth: 2.18,
                sun_angle: 0.05,
                sun_strength: 18.0,
                sun_colour: [1.0, 0.92, 0.72],
                sky_strength: 1.15,
                sky_colour: [0.3, 0.44, 0.72],
            },
            resolution: [512, 512],
            samples: 256,
            denoise: true,
            device: "GPU".into(),
            view_transform: "Standard".into(),
            outputs: vec!["beauty".into()],
        }
    }

    #[test]
    fn the_manifest_is_well_formed_json() {
        // Hand-written, so the balance is worth checking rather than trusting.
        let json = manifest().to_json();
        assert_eq!(
            json.matches('{').count(),
            json.matches('}').count(),
            "unbalanced braces:\n{json}"
        );
        assert_eq!(json.matches('[').count(), json.matches(']').count());
        assert!(!json.contains(",\n}"), "a trailing comma:\n{json}");
        assert!(
            !json.contains(", ]"),
            "a trailing comma in an array:\n{json}"
        );
    }

    #[test]
    fn the_manifest_carries_what_a_render_is_attributed_by() {
        // A render has to be traceable to a scene without the scene being kept,
        // and to a document without the document being kept.
        let json = manifest().to_json();
        assert!(json.contains("\"scene_digest\": \"00000000000000000000000000001234\""));
        assert!(json.contains("\"document_digest\": \"00000000000000000000000000005678\""));
        assert!(json.contains(&format!("\"package_version\": {PACKAGE_VERSION}")));
    }

    #[test]
    fn a_geometry_buffer_declares_enough_to_check_its_own_length() {
        // A buffer one element short is a renderer reading the tail of one blade
        // as the head of the next, for every blade after the short one.
        let buffer = &manifest().geometry[0];
        assert_eq!(buffer.position_floats(), 63_000);
        assert_eq!(buffer.attribute_floats(), 2_000);
        assert_eq!(
            buffer.floats_per_item,
            buffer.segments * buffer.vertices_per_segment * 3,
            "the layout does not describe itself consistently"
        );
    }

    #[test]
    fn attributes_are_named_rather_than_positional() {
        // A shader binds them by name; a renumbering that nothing reports is how
        // a maturity ends up driving a moisture.
        let json = manifest().to_json();
        assert!(json.contains("\"attribute_names\": [\"maturity\", \"moisture\"]"));
    }

    #[test]
    fn materials_are_named_by_appearance_rather_than_by_terrain_material() {
        // A blade of grass growing on ground that is 70% grass and 30% dirt is
        // still made of grass. The Python side dispatches on this string.
        let json = manifest().to_json();
        assert!(json.contains("\"appearance\": \"plant.grass_blade\""));
    }

    #[test]
    fn a_ground_grid_reports_its_own_edge_anchored_sample_count() {
        let ground = &manifest().ground;
        assert_eq!(ground.samples(), 5 * 9);
    }

    #[test]
    fn absent_buffers_are_null_rather_than_missing() {
        // So the reader distinguishes "this scene has no microrelief" from "this
        // manifest is from a version that did not know about microrelief".
        let json = manifest().to_json();
        assert!(json.contains("\"microrelief\": null"));
        assert!(json.contains("\"modifiers\": null"));
        assert!(json.contains("\"material_weights\": \"ground/material_weights.bin\""));
    }

    #[test]
    fn every_geometry_kind_has_a_distinct_name() {
        let kinds = [
            GeometryKind::Ribbons,
            GeometryKind::Curves,
            GeometryKind::Analytic,
            GeometryKind::Instances,
        ];
        let mut seen: Vec<&str> = Vec::new();
        for kind in kinds {
            assert!(!seen.contains(&kind.name()));
            seen.push(kind.name());
        }
    }
}

//! Everything in a Cycles scene that is not tuned grass.
//!
//! ## Why this is a section and not a second format
//!
//! The tuned blade buffers are the production image and have been for the whole
//! life of this renderer. Flowers, stones and undergrowth arrive beside them, not
//! instead of them, so the sane move is to extend the active scene package with
//! optional sections rather than to migrate production onto the generic
//! `write_package` writer at the same time as introducing new geometry.
//!
//! Combining those two would make a visual regression impossible to attribute:
//! the picture would change, and the cause could be the new content, the new
//! writer, or the interaction of the two. So the package gains a version, gains
//! empty secondary sections, and is proven byte-identical in its tuned half
//! before a single petal exists.
//!
//! ## Rust tessellates; Blender transfers
//!
//! A ribbon arrives here as vertices, normals and UVs — not as "a petal with
//! these parameters". Two reasons, and the second is the load-bearing one:
//!
//! - a future consumer that is not Blender should not have to reimplement plant
//!   morphology in its own language;
//! - the neural corpus conditions on what was *rendered*, and if Python decides
//!   the tessellation then the conditioning tensor describes a shape nobody in
//!   Rust can reproduce.
//!
//! Curves are the exception, and a deliberate one: a stem is a centreline plus a
//! radius, Blender can bevel that far more cheaply than a transferred tube, and
//! the centreline *is* the exact geometry rather than an approximation of it.
//!
//! ## No struct layout crosses the boundary
//!
//! Every record is written field by field in little-endian order. The workspace
//! forbids `unsafe`, so a `#[repr(C)]` cast is not available anyway — but even
//! with it available, writing explicitly is the right call: the reader is
//! Python, which cannot see a Rust struct definition, and a padding byte
//! introduced by a later field reordering would silently shift every subsequent
//! value.

use std::io;
use std::path::Path;

/// The version this build writes and Blender is expected to read.
///
/// One was the tuned-only package. Two adds the optional secondary sections and
/// makes the version mandatory rather than advisory: a reader that finds a
/// number it does not know must refuse, because the alternative is producing a
/// plausible picture from a misread file.
///
/// Three widens the ribbon vertex. Ribbons merge into one mesh per material, so
/// unlike an instance they have no Object Info to carry a per-plant tint — every
/// leaf in a plate shaded identically, which is exactly the flatness the
/// undergrowth was rebuilt to escape. The tint therefore travels per vertex,
/// constant along one leaf, and the version moves because the stride did.
pub const CYCLES_SCENE_FORMAT_VERSION: u32 = 3;

/// Whether a piece of secondary geometry is in the picture or only lighting it.
///
/// The same split the tuned blades already have. A halo object is present for
/// shadow, diffuse, glossy and transmission rays and invisible to camera rays,
/// which is what makes a stone just outside the frame still darken the grass
/// inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Visibility {
    #[default]
    Camera = 0,
    Halo = 1,
}

/// One vertex of a tessellated ribbon: a petal, a leaf, a blade of undergrowth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Along the ribbon, `0` at the root and `1` at the tip.
    pub along: f32,
    /// Across the ribbon, `-1` at one edge and `1` at the other.
    pub across: f32,
    /// A multiplier on the shader's base colour, constant along one ribbon.
    ///
    /// The same contract an [`Instance`] gets from Object Info, delivered as an
    /// attribute because a merged mesh has one object between thousands of
    /// plants. Written per vertex and *not* interpolated across a seam: two
    /// leaves never share a vertex, so a constant per ribbon stays constant.
    pub tint: [f32; 3],
    /// The plant's own latent, `0..1`. Free variation for a shader to spend.
    pub variation: f32,
}

impl RibbonVertex {
    /// Bytes one vertex occupies in `secondary-ribbons.bin`.
    pub const STRIDE: usize = 12 * 4;

    fn write(&self, out: &mut Vec<u8>) {
        for value in self.position {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.normal {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.along.to_le_bytes());
        out.extend_from_slice(&self.across.to_le_bytes());
        for value in self.tint {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.variation.to_le_bytes());
    }
}

/// One tessellated ribbon, as a span of the shared vertex and index tables.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RibbonSpan {
    pub vertex_offset: u32,
    pub vertex_count: u32,
    pub index_offset: u32,
    pub index_count: u32,
    pub material: u16,
    pub visibility: Visibility,
}

/// One curve: a stem, a runner, a twig.
///
/// A span of the shared point table plus the radii to bevel it with. Stored as
/// offsets rather than as an owned point list so the whole scene's centrelines
/// are one contiguous upload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveSpan {
    pub point_offset: u32,
    pub point_count: u32,
    pub radius_root_m: f32,
    pub radius_tip_m: f32,
    pub material: u16,
    pub visibility: Visibility,
}

/// A prototype's geometry family.
///
/// Named rather than parameterised at the top level, because a family decides
/// which builder runs and the builders take different parameters. A string here
/// would let a document name a family this build cannot construct and discover
/// it in Python.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrototypeFamily {
    /// A superellipsoid, optionally deformed and clipped. Stones and fragments.
    Superellipsoid,
    /// A shallow oblate disk. Flower receptacles.
    Disk,
    /// A flattened lozenge. Petals.
    ///
    /// Lowered to a superellipsoid in the package — Blender has one builder for
    /// both — but named separately here so the bridge can pick its exponents
    /// and its tessellation without a magic key comparison.
    Petal,
}

impl PrototypeFamily {
    pub fn name(self) -> &'static str {
        match self {
            Self::Superellipsoid => "superellipsoid",
            Self::Disk => "disk",
            // Built by the same Blender routine as a stone; the difference is
            // entirely in the parameters the bridge chose.
            Self::Petal => "superellipsoid",
        }
    }
}

/// One prototype mesh, described so Blender can build it deterministically.
///
/// Every number Blender needs is here. Blender draws no random values and makes
/// no shape decisions — see the spec's rejected alternatives: scattering or
/// randomising in Python breaks addressed world determinism and leaves the
/// conditioning metadata unable to name the objects that were actually
/// rendered.
#[derive(Clone, Debug, PartialEq)]
pub struct Prototype {
    /// The semantic key: `stone.rounded.v1`.
    pub key: String,
    pub family: PrototypeFamily,
    /// Semi-axes before instance scale, metres.
    pub semi_axes_m: [f32; 3],
    /// Superellipsoid exponents. `(1, 1)` is an ellipsoid.
    pub exponents: [f32; 2],
    /// Low-order radial deformation: up to three `(amplitude, frequency, phase)`.
    ///
    /// Bounded at three on purpose. High-frequency displacement turns a small
    /// stone into a noisy potato at the target scale, and the silhouette is what
    /// makes a stone recognisable.
    pub deformation: Vec<[f32; 3]>,
    /// Clipping half-spaces `n · x <= d`, as `(nx, ny, nz, d)`.
    pub clips: Vec<[f32; 4]>,
    /// Rings and segments. Part of the geometry, so part of the identity.
    pub tessellation: [u16; 2],
    /// Which secondary material this prototype's faces take.
    pub material: u16,
    /// Height of the unit prototype before scaling, for burial arithmetic.
    pub unit_height_m: f32,
}

/// One placement of a prototype.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub prototype: u32,
    pub material_variant: u16,
    pub visibility: Visibility,
    pub translation: [f32; 3],
    /// Rotation as a unit quaternion, `xyzw`.
    pub rotation_xyzw: [f32; 4],
    pub scale: [f32; 3],
    /// Multiplicative tint in linear light.
    pub tint: [f32; 3],
    /// A per-instance variation value the shader may use.
    pub variation: f32,
}

impl Instance {
    /// Bytes one instance occupies in `instances.bin`.
    ///
    /// `4 + 2 + 1 + 1` of header then fourteen floats. Asserted against the
    /// writer rather than trusted, because a stride the reader disagrees with
    /// produces a scene full of objects at plausible-looking wrong transforms.
    pub const STRIDE: usize = 8 + 14 * 4;

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.prototype.to_le_bytes());
        out.extend_from_slice(&self.material_variant.to_le_bytes());
        out.push(self.visibility as u8);
        // One reserved byte, so the record is four-byte aligned for a reader
        // that wants to use a structured array rather than unpacking fields.
        out.push(0);
        for value in self.translation {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.rotation_xyzw {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.scale {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in self.tint {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.variation.to_le_bytes());
    }
}

/// An appearance key bound to a shader the Blender side knows how to build.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialBinding {
    pub appearance: String,
    pub shader: String,
}

/// Every non-tuned thing in one scene.
///
/// `Default` is the empty case, and the empty case is load-bearing: format
/// version two ships before any secondary content exists, so that the day a
/// petal first appears the *format* is already proven not to have moved the
/// tuned image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SecondaryGeometry {
    pub ribbon_vertices: Vec<RibbonVertex>,
    pub ribbon_indices: Vec<u32>,
    pub ribbons: Vec<RibbonSpan>,
    pub curve_points: Vec<[f32; 3]>,
    pub curves: Vec<CurveSpan>,
    pub prototypes: Vec<Prototype>,
    pub instances: Vec<Instance>,
    pub materials: Vec<MaterialBinding>,
}

/// What a package claims about itself, so a reader can check before trusting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecondaryCounts {
    pub ribbon_vertices: usize,
    pub ribbon_indices: usize,
    pub ribbons: usize,
    pub curve_points: usize,
    pub curves: usize,
    pub prototypes: usize,
    pub instances: usize,
}

impl SecondaryGeometry {
    pub fn is_empty(&self) -> bool {
        self.ribbons.is_empty()
            && self.curves.is_empty()
            && self.instances.is_empty()
            && self.prototypes.is_empty()
    }

    pub fn counts(&self) -> SecondaryCounts {
        SecondaryCounts {
            ribbon_vertices: self.ribbon_vertices.len(),
            ribbon_indices: self.ribbon_indices.len(),
            ribbons: self.ribbons.len(),
            curve_points: self.curve_points.len(),
            curves: self.curves.len(),
            prototypes: self.prototypes.len(),
            instances: self.instances.len(),
        }
    }

    /// Everything wrong with this geometry, in one pass.
    ///
    /// Run before writing, so a corrupt package fails with a message naming the
    /// offending record rather than producing a plausible but wrong image. The
    /// checks are the ones whose failure is silent: an index that lands one past
    /// the end of a table draws a different triangle, it does not crash.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let vertices = self.ribbon_vertices.len();
        let indices = self.ribbon_indices.len();
        let points = self.curve_points.len();
        let materials = self.materials.len();
        let prototypes = self.prototypes.len();

        for (slot, ribbon) in self.ribbons.iter().enumerate() {
            let end = ribbon.vertex_offset as usize + ribbon.vertex_count as usize;
            if end > vertices {
                out.push(format!(
                    "ribbon {slot} spans vertices {}..{end} of {vertices}",
                    ribbon.vertex_offset
                ));
            }
            let index_end = ribbon.index_offset as usize + ribbon.index_count as usize;
            if index_end > indices {
                out.push(format!(
                    "ribbon {slot} spans indices {}..{index_end} of {indices}",
                    ribbon.index_offset
                ));
            }
            if ribbon.material as usize >= materials {
                out.push(format!(
                    "ribbon {slot} names material {} of {materials}",
                    ribbon.material
                ));
            }
        }
        for (slot, index) in self.ribbon_indices.iter().enumerate() {
            if *index as usize >= vertices {
                out.push(format!(
                    "ribbon index {slot} points at vertex {index} of {vertices}"
                ));
            }
        }
        for (slot, curve) in self.curves.iter().enumerate() {
            let end = curve.point_offset as usize + curve.point_count as usize;
            if end > points {
                out.push(format!(
                    "curve {slot} spans points {}..{end} of {points}",
                    curve.point_offset
                ));
            }
            if curve.point_count < 2 {
                out.push(format!(
                    "curve {slot} has {} point(s); a centreline needs two",
                    curve.point_count
                ));
            }
            if curve.material as usize >= materials {
                out.push(format!(
                    "curve {slot} names material {} of {materials}",
                    curve.material
                ));
            }
        }
        for (slot, instance) in self.instances.iter().enumerate() {
            if instance.prototype as usize >= prototypes {
                out.push(format!(
                    "instance {slot} names prototype {} of {prototypes}",
                    instance.prototype
                ));
            }
            if !instance.translation.iter().all(|v| v.is_finite())
                || !instance.rotation_xyzw.iter().all(|v| v.is_finite())
                || !instance.scale.iter().all(|v| v.is_finite())
            {
                out.push(format!("instance {slot} has a non-finite transform"));
            }
            if !instance.scale.iter().all(|v| *v > 0.0) {
                out.push(format!(
                    "instance {slot} has a nonpositive scale {:?}",
                    instance.scale
                ));
            }
            let length: f32 = instance
                .rotation_xyzw
                .iter()
                .map(|v| v * v)
                .sum::<f32>()
                .sqrt();
            if (length - 1.0).abs() > 1.0e-3 {
                out.push(format!(
                    "instance {slot} has a rotation of length {length}, not a unit quaternion"
                ));
            }
        }
        for (slot, prototype) in self.prototypes.iter().enumerate() {
            if prototype.material as usize >= materials {
                out.push(format!(
                    "prototype {slot} names material {} of {materials}",
                    prototype.material
                ));
            }
            if !prototype.semi_axes_m.iter().all(|v| *v > 0.0) {
                out.push(format!("prototype {slot} has a nonpositive semi-axis"));
            }
            if prototype.deformation.len() > 3 {
                out.push(format!(
                    "prototype {slot} has {} deformation terms; three is the bound",
                    prototype.deformation.len()
                ));
            }
        }
        // Two prototypes under one key with different geometry cannot both
        // answer to that name, and resolving it by table order would make the
        // shape depend on emission order.
        let mut seen: std::collections::BTreeMap<&str, &Prototype> =
            std::collections::BTreeMap::new();
        for prototype in &self.prototypes {
            match seen.insert(prototype.key.as_str(), prototype) {
                Some(first) if *first != *prototype => out.push(format!(
                    "prototype key `{}` describes two different shapes",
                    prototype.key
                )),
                _ => {}
            }
        }
        out
    }

    /// Write the binary tables, returning what was written.
    ///
    /// Files are written only when non-empty, matching the rule the ground state
    /// planes already follow: a manifest that names a file the reader then finds
    /// empty is worse than a manifest that names nothing, because the reader
    /// trusts the manifest and stops.
    pub fn write(&self, directory: &Path) -> io::Result<()> {
        if !self.ribbon_vertices.is_empty() {
            let mut bytes = Vec::with_capacity(self.ribbon_vertices.len() * RibbonVertex::STRIDE);
            for vertex in &self.ribbon_vertices {
                vertex.write(&mut bytes);
            }
            debug_assert_eq!(
                bytes.len(),
                self.ribbon_vertices.len() * RibbonVertex::STRIDE
            );
            std::fs::write(directory.join("secondary-ribbons.bin"), &bytes)?;
        }
        if !self.ribbon_indices.is_empty() {
            let mut bytes = Vec::with_capacity(self.ribbon_indices.len() * 4);
            for index in &self.ribbon_indices {
                bytes.extend_from_slice(&index.to_le_bytes());
            }
            std::fs::write(directory.join("secondary-indices.bin"), &bytes)?;
        }
        if !self.curve_points.is_empty() {
            let mut bytes = Vec::with_capacity(self.curve_points.len() * 12);
            for point in &self.curve_points {
                for value in point {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            std::fs::write(directory.join("secondary-curves.bin"), &bytes)?;
        }
        if !self.instances.is_empty() {
            let mut bytes = Vec::with_capacity(self.instances.len() * Instance::STRIDE);
            for instance in &self.instances {
                instance.write(&mut bytes);
            }
            debug_assert_eq!(bytes.len(), self.instances.len() * Instance::STRIDE);
            std::fs::write(directory.join("instances.bin"), &bytes)?;
        }
        Ok(())
    }

    /// The `"secondary"` object of the scene header.
    ///
    /// Always present, even when empty. An absent section and an empty one mean
    /// the same thing to a careful reader and different things to a careless
    /// one, and the careless reading — "no key, so skip the check" — is the one
    /// that silently drops content the day the section stops being empty.
    pub fn header_json(&self) -> String {
        let counts = self.counts();

        let spans = |items: String| {
            if items.is_empty() {
                String::new()
            } else {
                items
            }
        };

        let ribbons = spans(
            self.ribbons
                .iter()
                .map(|r| {
                    format!(
                        r#"{{"vertex_offset": {}, "vertex_count": {}, "index_offset": {}, "index_count": {}, "material": {}, "visibility": "{}"}}"#,
                        r.vertex_offset,
                        r.vertex_count,
                        r.index_offset,
                        r.index_count,
                        r.material,
                        visibility_name(r.visibility),
                    )
                })
                .collect::<Vec<_>>()
                .join(",\n        "),
        );
        let curves = spans(
            self.curves
                .iter()
                .map(|c| {
                    format!(
                        r#"{{"point_offset": {}, "point_count": {}, "radius_root_m": {:.6}, "radius_tip_m": {:.6}, "material": {}, "visibility": "{}"}}"#,
                        c.point_offset,
                        c.point_count,
                        c.radius_root_m,
                        c.radius_tip_m,
                        c.material,
                        visibility_name(c.visibility),
                    )
                })
                .collect::<Vec<_>>()
                .join(",\n        "),
        );
        let prototypes = spans(
            self.prototypes
                .iter()
                .map(prototype_json)
                .collect::<Vec<_>>()
                .join(",\n        "),
        );
        let materials = spans(
            self.materials
                .iter()
                .map(|m| {
                    format!(
                        r#"{{"appearance": "{}", "shader": "{}"}}"#,
                        m.appearance, m.shader
                    )
                })
                .collect::<Vec<_>>()
                .join(",\n        "),
        );

        format!(
            r#"{{
    "ribbons": {{
      "path": "secondary-ribbons.bin",
      "indices": "secondary-indices.bin",
      "vertex_count": {},
      "index_count": {},
      "vertex_stride": {},
      "spans": [{}]
    }},
    "curves": {{
      "path": "secondary-curves.bin",
      "point_count": {},
      "point_stride": 12,
      "spans": [{}]
    }},
    "instances": {{
      "path": "instances.bin",
      "count": {},
      "stride": {}
    }},
    "prototypes": [{}],
    "materials": [{}]
  }}"#,
            counts.ribbon_vertices,
            counts.ribbon_indices,
            RibbonVertex::STRIDE,
            wrap(&ribbons),
            counts.curve_points,
            wrap(&curves),
            counts.instances,
            Instance::STRIDE,
            wrap(&prototypes),
            wrap(&materials),
        )
    }
}

/// Indent a JSON array body, or collapse it to nothing when empty.
fn wrap(items: &str) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!("\n        {items}\n      ")
    }
}

fn visibility_name(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Camera => "camera",
        Visibility::Halo => "halo",
    }
}

fn prototype_json(prototype: &Prototype) -> String {
    let triples = |values: &[[f32; 3]]| {
        values
            .iter()
            .map(|v| format!("[{:.6}, {:.6}, {:.6}]", v[0], v[1], v[2]))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let quads = |values: &[[f32; 4]]| {
        values
            .iter()
            .map(|v| format!("[{:.6}, {:.6}, {:.6}, {:.6}]", v[0], v[1], v[2], v[3]))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        r#"{{"key": "{}", "family": "{}", "semi_axes_m": [{:.6}, {:.6}, {:.6}], "exponents": [{:.6}, {:.6}], "deformation": [{}], "clips": [{}], "tessellation": [{}, {}], "material": {}, "unit_height_m": {:.6}}}"#,
        prototype.key,
        prototype.family.name(),
        prototype.semi_axes_m[0],
        prototype.semi_axes_m[1],
        prototype.semi_axes_m[2],
        prototype.exponents[0],
        prototype.exponents[1],
        triples(&prototype.deformation),
        quads(&prototype.clips),
        prototype.tessellation[0],
        prototype.tessellation[1],
        prototype.material,
        prototype.unit_height_m,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance() -> Instance {
        Instance {
            prototype: 0,
            material_variant: 0,
            visibility: Visibility::Camera,
            translation: [1.0, 2.0, 3.0],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            tint: [1.0, 1.0, 1.0],
            variation: 0.5,
        }
    }

    fn prototype() -> Prototype {
        Prototype {
            key: "stone.rounded.v1".into(),
            family: PrototypeFamily::Superellipsoid,
            semi_axes_m: [1.0, 0.85, 0.6],
            exponents: [0.9, 0.9],
            deformation: vec![[0.05, 2.0, 0.3]],
            clips: Vec::new(),
            tessellation: [12, 16],
            material: 0,
            unit_height_m: 1.2,
        }
    }

    fn material() -> MaterialBinding {
        MaterialBinding {
            appearance: "surface.stone".into(),
            shader: "stone".into(),
        }
    }

    #[test]
    fn an_instance_record_is_exactly_its_declared_stride() {
        // A stride the reader disagrees with does not crash; it produces a scene
        // full of objects at plausible-looking wrong transforms.
        let mut bytes = Vec::new();
        instance().write(&mut bytes);
        assert_eq!(bytes.len(), Instance::STRIDE);
    }

    #[test]
    fn a_ribbon_vertex_is_exactly_its_declared_stride() {
        let mut bytes = Vec::new();
        RibbonVertex {
            position: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            along: 0.0,
            across: 0.0,
            tint: [1.0, 1.0, 1.0],
            variation: 0.0,
        }
        .write(&mut bytes);
        assert_eq!(bytes.len(), RibbonVertex::STRIDE);
    }

    #[test]
    fn an_instance_round_trips_through_little_endian_bytes() {
        // Read back the way Python will read it, rather than the way Rust wrote
        // it, so a field-order mistake shows up here rather than as a rotated
        // stone.
        let mut bytes = Vec::new();
        let original = instance();
        original.write(&mut bytes);

        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        let f32_at = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        assert_eq!(u32_at(0), original.prototype);
        assert_eq!(
            u16::from_le_bytes(bytes[4..6].try_into().expect("2 bytes")),
            original.material_variant
        );
        assert_eq!(bytes[6], Visibility::Camera as u8);
        assert_eq!(bytes[7], 0, "the reserved byte is zero");
        for (slot, expected) in original.translation.iter().enumerate() {
            assert_eq!(f32_at(8 + slot * 4), *expected);
        }
        for (slot, expected) in original.rotation_xyzw.iter().enumerate() {
            assert_eq!(f32_at(20 + slot * 4), *expected);
        }
        assert_eq!(f32_at(60), original.variation);
    }

    #[test]
    fn an_empty_section_still_declares_itself() {
        // An absent section and an empty one mean the same thing to a careful
        // reader and different things to a careless one. The careless reading
        // silently drops content the day the section stops being empty.
        let json = SecondaryGeometry::default().header_json();
        assert!(json.contains("\"ribbons\""));
        assert!(json.contains("\"curves\""));
        assert!(json.contains("\"instances\""));
        assert!(json.contains("\"prototypes\": []"));
        assert!(json.contains("\"count\": 0"));
        assert!(SecondaryGeometry::default().is_empty());
        assert!(SecondaryGeometry::default().problems().is_empty());
    }

    #[test]
    fn a_span_past_the_end_of_its_table_is_reported() {
        // An index one past the end draws a different triangle rather than
        // crashing, which is why this is checked rather than left to Python.
        let geometry = SecondaryGeometry {
            materials: vec![material()],
            ribbons: vec![RibbonSpan {
                vertex_offset: 0,
                vertex_count: 4,
                index_offset: 0,
                index_count: 6,
                material: 0,
                visibility: Visibility::Camera,
            }],
            ..Default::default()
        };
        let problems = geometry.problems();
        assert!(
            problems.iter().any(|p| p.contains("spans vertices")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_dangling_material_or_prototype_is_reported() {
        let geometry = SecondaryGeometry {
            prototypes: vec![prototype()],
            instances: vec![Instance {
                prototype: 7,
                ..instance()
            }],
            ..Default::default()
        };
        let problems = geometry.problems();
        assert!(
            problems.iter().any(|p| p.contains("names prototype 7")),
            "{problems:?}"
        );
        // The prototype itself names material 0 of an empty table.
        assert!(
            problems.iter().any(|p| p.contains("names material 0")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_denormalised_rotation_is_reported() {
        // A quaternion that is not unit length scales the object as well as
        // turning it, which reads as a stone of the wrong size rather than as a
        // maths error.
        let geometry = SecondaryGeometry {
            materials: vec![material()],
            prototypes: vec![prototype()],
            instances: vec![Instance {
                rotation_xyzw: [0.0, 0.0, 0.0, 0.5],
                ..instance()
            }],
            ..Default::default()
        };
        assert!(
            geometry
                .problems()
                .iter()
                .any(|p| p.contains("not a unit quaternion"))
        );
    }

    #[test]
    fn a_nonpositive_scale_is_reported() {
        let geometry = SecondaryGeometry {
            materials: vec![material()],
            prototypes: vec![prototype()],
            instances: vec![Instance {
                scale: [1.0, 0.0, 1.0],
                ..instance()
            }],
            ..Default::default()
        };
        assert!(
            geometry
                .problems()
                .iter()
                .any(|p| p.contains("nonpositive scale"))
        );
    }

    #[test]
    fn one_prototype_key_may_not_describe_two_shapes() {
        let mut other = prototype();
        other.semi_axes_m = [2.0, 1.0, 1.0];
        let geometry = SecondaryGeometry {
            materials: vec![material()],
            prototypes: vec![prototype(), other],
            ..Default::default()
        };
        assert!(
            geometry
                .problems()
                .iter()
                .any(|p| p.contains("two different shapes"))
        );
    }

    #[test]
    fn a_curve_needs_at_least_two_points() {
        let geometry = SecondaryGeometry {
            materials: vec![material()],
            curve_points: vec![[0.0, 0.0, 0.0]],
            curves: vec![CurveSpan {
                point_offset: 0,
                point_count: 1,
                radius_root_m: 0.002,
                radius_tip_m: 0.001,
                material: 0,
                visibility: Visibility::Camera,
            }],
            ..Default::default()
        };
        assert!(
            geometry
                .problems()
                .iter()
                .any(|p| p.contains("a centreline needs two"))
        );
    }

    #[test]
    fn a_well_formed_scene_reports_nothing() {
        let geometry = SecondaryGeometry {
            materials: vec![material()],
            prototypes: vec![prototype()],
            instances: vec![instance()],
            curve_points: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.2]],
            curves: vec![CurveSpan {
                point_offset: 0,
                point_count: 2,
                radius_root_m: 0.002,
                radius_tip_m: 0.001,
                material: 0,
                visibility: Visibility::Halo,
            }],
            ..Default::default()
        };
        assert_eq!(geometry.problems(), Vec::<String>::new());
        assert!(!geometry.is_empty());
        assert_eq!(geometry.counts().curves, 1);
    }
}

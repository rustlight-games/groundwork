//! Blade geometry.
//!
//! Grass is drawn as actual tapered ribbons rather than alpha-cut cards. That
//! costs vertices and buys three things worth having:
//!
//! - **Clean edges from multisampling.** Alpha-tested foliage aliases badly and
//!   crawls when the camera moves; geometric edges are resolved by MSAA, which
//!   the hardware is doing anyway.
//! - **No overdraw on empty pixels.** A card is mostly transparent, and every
//!   one of those transparent pixels still costs a shaded fragment.
//! - **Correct depth sorting for free.** Opaque geometry writes depth, so a
//!   blade leaning across its neighbour interleaves per fragment. Cards would
//!   need sorting, and no sort order is correct for mutually overlapping quads.
//!
//! It also sidesteps needing blade textures, which do not exist yet.
//!
//! ## What a vertex carries
//!
//! Position is the blade's *rest* pose already projected to the screen. The
//! vertex shader overwrites it, so its real job is giving Bevy an honest
//! bounding box in the same space the shader outputs — get that wrong and
//! chunks vanish at the edges of the view.
//!
//! Everything else is packed to eight bits per channel. The per-blade values
//! are repeated across all fourteen of its vertices, so the redundancy is what
//! costs memory, not the precision: sub-millimetre steps in blade length are
//! well past anything visible.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{
    Indices, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues, VertexFormat,
};
use bevy::prelude::*;

use crate::field::GrassField;
use crate::iso;
use crate::noise::{hash_2d, unit_from_hash};

/// Rings up the blade. Six segments carries an eighty-degree bend without
/// visible faceting; four does not.
pub const RINGS: usize = 7;

/// Vertices per blade.
pub const VERTICES_PER_BLADE: usize = RINGS * 2;

/// Indices per blade.
pub const INDICES_PER_BLADE: usize = (RINGS - 1) * 6;

/// Edge of a grass chunk, in metres.
///
/// Small enough that culling is meaningful at typical zoom, large enough that a
/// screenful is a dozen draw calls rather than hundreds.
pub const CHUNK_METRES: f32 = 4.0;

/// Blades per square metre at full detail.
///
/// Blades land independently, so coverage is `1 - exp(-density * area)` rather
/// than proportional: at half this number the canopy is only about seventy per
/// cent covered and the ground shows through as speckle. Going much past it
/// costs memory for coverage nobody can see.
pub const BLADES_PER_SQUARE_METRE: f32 = 260.0;

/// Blade length range, in metres. Mid-length grass: ankle to knee.
///
/// The spread matters as much as the middle. Blades of near-identical height
/// give the canopy a mown flat top, which is the single clearest tell that
/// grass was generated rather than grown.
pub const LENGTH_RANGE: (f32, f32) = (0.12, 0.46);

/// Blade half-width range at the base, in metres.
pub const WIDTH_RANGE: (f32, f32) = (0.016, 0.032);

/// Fraction of a stratum a blade may be jittered across.
const JITTER: f32 = 0.65;

/// World position of the blade's root.
pub const ATTRIBUTE_ROOT: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassRoot", 0x6a72_0001, VertexFormat::Float32x2);

/// `(height along blade, which side of the ribbon, length, base half-width)`.
pub const ATTRIBUTE_SHAPE: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassShape", 0x6a72_0002, VertexFormat::Unorm8x4);

/// `(flutter phase, flutter rate, tint, per-blade random)`.
pub const ATTRIBUTE_VARIANT: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassVariant", 0x6a72_0003, VertexFormat::Unorm8x4);

/// A chunk's worth of blades, ready to become a mesh.
pub struct BladeBatch {
    positions: Vec<[f32; 3]>,
    roots: Vec<[f32; 2]>,
    shapes: Vec<[u8; 4]>,
    variants: Vec<[u8; 4]>,
    indices: Vec<u32>,
    blades: u32,
}

impl BladeBatch {
    fn with_capacity(blades: usize) -> Self {
        Self {
            positions: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            roots: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            shapes: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            variants: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            indices: Vec::with_capacity(blades * INDICES_PER_BLADE),
            blades: 0,
        }
    }

    /// How many blades were placed.
    pub fn blades(&self) -> u32 {
        self.blades
    }

    pub fn is_empty(&self) -> bool {
        self.blades == 0
    }

    /// One root position per blade.
    pub fn roots(&self) -> impl Iterator<Item = Vec2> + '_ {
        self.roots
            .iter()
            .step_by(VERTICES_PER_BLADE)
            .map(|&root| Vec2::from(root))
    }

    /// One length per blade, in metres.
    pub fn lengths(&self) -> impl Iterator<Item = f32> + '_ {
        self.shapes
            .iter()
            .step_by(VERTICES_PER_BLADE)
            .map(|shape| lerp(LENGTH_RANGE.0, LENGTH_RANGE.1, shape[2] as f32 / 255.0))
    }

    /// Bytes of vertex and index data.
    pub fn byte_size(&self) -> usize {
        self.positions.len() * 12
            + self.roots.len() * 8
            + self.shapes.len() * 4
            + self.variants.len() * 4
            + self.indices.len() * 4
    }

    fn push_blade(&mut self, blade: Blade) {
        let base = self.positions.len() as u32;
        let length_t = inverse_lerp(LENGTH_RANGE.0, LENGTH_RANGE.1, blade.length);
        let width_t = inverse_lerp(WIDTH_RANGE.0, WIDTH_RANGE.1, blade.width);

        for ring in 0..RINGS {
            let height = ring as f32 / (RINGS - 1) as f32;
            // The rest pose: a blade standing straight up. Only ever used for
            // the bounding box, since the shader recomputes the real position.
            let rest =
                iso::project_to_vertex(blade.root.extend(0.0) + Vec3::Z * (height * blade.length));
            for side in 0..2 {
                self.positions.push(rest.to_array());
                self.roots.push(blade.root.to_array());
                self.shapes.push([
                    quantise(height),
                    if side == 0 { 0 } else { 255 },
                    quantise(length_t),
                    quantise(width_t),
                ]);
                self.variants.push([
                    quantise(blade.phase),
                    quantise(blade.rate),
                    quantise(blade.tint),
                    quantise(blade.random),
                ]);
            }
        }

        for segment in 0..(RINGS - 1) as u32 {
            let a = base + segment * 2;
            // Counter-clockwise as seen from the front. Back faces are not
            // culled — a blade bends far enough to show either side, and
            // culling one would make grass wink out as it leans over.
            self.indices
                .extend_from_slice(&[a, a + 1, a + 2, a + 1, a + 3, a + 2]);
        }
        self.blades += 1;
    }

    /// Turn the batch into a mesh.
    pub fn into_mesh(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(ATTRIBUTE_ROOT, self.roots)
        // Spelled out rather than inferred: `Vec<[u8; 4]>` is ambiguous between
        // the integer and normalised formats, and picking the integer one would
        // hand the shader values in 0..255 where it expects 0..1.
        .with_inserted_attribute(
            ATTRIBUTE_SHAPE,
            VertexAttributeValues::Unorm8x4(self.shapes),
        )
        .with_inserted_attribute(
            ATTRIBUTE_VARIANT,
            VertexAttributeValues::Unorm8x4(self.variants),
        )
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

struct Blade {
    root: Vec2,
    length: f32,
    width: f32,
    phase: f32,
    rate: f32,
    tint: f32,
    random: f32,
}

/// Build the blades for one chunk.
///
/// `chunk` is a chunk coordinate; the chunk covers
/// `[chunk * CHUNK_METRES, (chunk + 1) * CHUNK_METRES)` in world metres.
/// `detail` scales the blade count for level of detail.
///
/// Placement is a jittered grid rather than uniform random. Uniform random
/// points clump — some spots get four blades and others none — and clumping in
/// grass reads as bald patches. Stratifying first and jittering within each
/// stratum keeps the spacing roughly even while removing any trace of a lattice.
pub fn build_chunk(field: &GrassField, chunk: IVec2, detail: f32, seed: u32) -> BladeBatch {
    let origin = chunk.as_vec2() * CHUNK_METRES;
    let target = (CHUNK_METRES * CHUNK_METRES * BLADES_PER_SQUARE_METRE * detail.clamp(0.0, 1.0))
        .round()
        .max(0.0) as usize;
    let mut batch = BladeBatch::with_capacity(target);
    if target == 0 {
        return batch;
    }

    let strata = (target as f32).sqrt().ceil().max(1.0) as i32;
    let stride = CHUNK_METRES / strata as f32;
    let chunk_seed = seed ^ hash_2d(chunk.x, chunk.y, 0x51A5_5EED);

    for sy in 0..strata {
        for sx in 0..strata {
            let hash = hash_2d(sx, sy, chunk_seed);
            // Jittered across most of the stratum but not all of it. Full-width
            // jitter lets blades in adjacent strata land almost on top of each
            // other, which is the clumping the stratification was meant to
            // avoid; leaving a margin keeps a minimum spacing while still
            // hiding the lattice.
            let jitter = Vec2::new(
                unit_from_hash(hash),
                unit_from_hash(hash.wrapping_mul(0x9e37_79b9)),
            );
            let jitter = Vec2::splat(0.5) + (jitter - Vec2::splat(0.5)) * JITTER;
            // Offset alternate rows by half a stratum. Jitter alone still
            // leaves the strata lined up in columns, and at grass densities
            // that shows as faint vertical banding across the whole field.
            let row_offset = if sy % 2 == 0 { 0.0 } else { 0.5 };
            let root = origin + (Vec2::new(sx as f32 + row_offset, sy as f32) + jitter) * stride;

            // Thin the blades out where the ground is bare, using the same
            // density the solver reads, so patchiness agrees between what is
            // simulated and what is drawn.
            let density = field.density_at_world(root);
            let keep = unit_from_hash(hash.wrapping_mul(0x85eb_ca6b));
            if keep > density {
                continue;
            }

            let a = unit_from_hash(hash.wrapping_mul(0xc2b2_ae35));
            let b = unit_from_hash(hash.wrapping_mul(0x27d4_eb2f));
            let c = unit_from_hash(hash.wrapping_mul(0x1656_67b1));
            let d = unit_from_hash(hash.wrapping_mul(0x2545_f491));

            // Longer where the field says the grass is longer, varied per blade
            // so neighbouring blades in one clump are not all the same height.
            let local = field.length_at_world(root);
            let length = (local * (0.62 + 0.68 * a)).clamp(LENGTH_RANGE.0, LENGTH_RANGE.1);

            batch.push_blade(Blade {
                root,
                length,
                width: lerp(WIDTH_RANGE.0, WIDTH_RANGE.1, b),
                phase: c,
                rate: d,
                tint: unit_from_hash(hash.wrapping_mul(0x7feb_352d)),
                random: unit_from_hash(hash.wrapping_mul(0x846c_a68b)),
            });
        }
    }

    batch
}

fn quantise(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn inverse_lerp(a: f32, b: f32, value: f32) -> f32 {
    if (b - a).abs() <= f32::EPSILON {
        return 0.0;
    }
    ((value - a) / (b - a)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> GrassField {
        let mut field = GrassField::new(128, 0.15, 3);
        field.make_uniform(0.24, 1.0);
        field
    }

    #[test]
    fn a_chunk_places_roughly_the_requested_blade_count() {
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        let expected = CHUNK_METRES * CHUNK_METRES * BLADES_PER_SQUARE_METRE;
        let ratio = batch.blades() as f32 / expected;
        assert!((0.85..=1.2).contains(&ratio), "{} blades", batch.blades());
    }

    #[test]
    fn detail_scales_the_blade_count_down() {
        let full = build_chunk(&field(), IVec2::ZERO, 1.0, 1).blades();
        let half = build_chunk(&field(), IVec2::ZERO, 0.25, 1).blades();
        assert!(half < full);
        assert!(half > 0);
        assert_eq!(build_chunk(&field(), IVec2::ZERO, 0.0, 1).blades(), 0);
    }

    #[test]
    fn blades_stay_inside_their_chunk() {
        // Blades straying outside would be culled with the wrong chunk and pop
        // at the edges of the view.
        let batch = build_chunk(&field(), IVec2::new(2, -3), 1.0, 1);
        let origin = Vec2::new(2.0, -3.0) * CHUNK_METRES;
        for root in &batch.roots {
            assert!(
                root[0] >= origin.x && root[0] <= origin.x + CHUNK_METRES,
                "{root:?}"
            );
            assert!(
                root[1] >= origin.y && root[1] <= origin.y + CHUNK_METRES,
                "{root:?}"
            );
        }
    }

    #[test]
    fn neighbouring_chunks_do_not_produce_the_same_layout() {
        // A per-chunk seed that ignored the coordinate would tile one patch of
        // grass across the whole field, which is extremely visible.
        let a = build_chunk(&field(), IVec2::new(0, 0), 1.0, 1);
        let b = build_chunk(&field(), IVec2::new(1, 0), 1.0, 1);
        let a_local: Vec<[f32; 2]> = a.roots.iter().map(|r| [r[0], r[1]]).collect();
        let b_local: Vec<[f32; 2]> = b
            .roots
            .iter()
            .map(|r| [r[0] - CHUNK_METRES, r[1]])
            .collect();
        assert_ne!(a_local, b_local);
    }

    #[test]
    fn a_chunk_is_reproducible() {
        let a = build_chunk(&field(), IVec2::new(1, 1), 1.0, 7);
        let b = build_chunk(&field(), IVec2::new(1, 1), 1.0, 7);
        assert_eq!(a.roots, b.roots);
        assert_eq!(a.shapes, b.shapes);
    }

    #[test]
    fn placement_is_spread_rather_than_clumped() {
        // The property jittered stratification buys. Measured as the smallest
        // nearest-neighbour distance over the mean: uniform random points score
        // near zero because some pairs land almost on top of each other.
        let batch = build_chunk(&field(), IVec2::ZERO, 0.06, 1);
        let roots: Vec<Vec2> = batch
            .roots
            .chunks(VERTICES_PER_BLADE)
            .map(|group| Vec2::from(group[0]))
            .collect();
        assert!(roots.len() > 20, "need enough blades to measure");

        let nearest: Vec<f32> = roots
            .iter()
            .map(|&p| {
                roots
                    .iter()
                    .filter(|&&q| q != p)
                    .map(|&q| p.distance(q))
                    .fold(f32::MAX, f32::min)
            })
            .collect();
        let mean = nearest.iter().sum::<f32>() / nearest.len() as f32;
        let min = nearest.iter().cloned().fold(f32::MAX, f32::min);
        assert!(min / mean > 0.25, "clumped: min {min}, mean {mean}");
    }

    #[test]
    fn every_blade_is_fully_formed() {
        let batch = build_chunk(&field(), IVec2::ZERO, 0.2, 1);
        let blades = batch.blades() as usize;
        assert_eq!(batch.positions.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.roots.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.shapes.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.variants.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.indices.len(), blades * INDICES_PER_BLADE);
    }

    #[test]
    fn indices_are_all_in_range() {
        let batch = build_chunk(&field(), IVec2::ZERO, 0.2, 1);
        let vertices = batch.positions.len() as u32;
        assert!(batch.indices.iter().all(|&i| i < vertices));
    }

    #[test]
    fn blades_vary_in_length_and_width() {
        // Identical blades read as a printed texture rather than as grass.
        let batch = build_chunk(&field(), IVec2::ZERO, 0.3, 1);
        let lengths: Vec<u8> = batch.shapes.iter().map(|s| s[2]).collect();
        let widths: Vec<u8> = batch.shapes.iter().map(|s| s[3]).collect();
        let distinct_lengths = distinct(&lengths);
        let distinct_widths = distinct(&widths);
        assert!(distinct_lengths > 20, "only {distinct_lengths} lengths");
        assert!(distinct_widths > 20, "only {distinct_widths} widths");
    }

    #[test]
    fn bare_ground_grows_no_grass() {
        let mut bare = GrassField::new(128, 0.15, 3);
        bare.make_uniform(0.24, 1.0);
        bare.set_density_everywhere(0.0);
        assert_eq!(build_chunk(&bare, IVec2::ZERO, 1.0, 1).blades(), 0);
    }

    #[test]
    fn the_mesh_carries_every_attribute() {
        let mesh = build_chunk(&field(), IVec2::ZERO, 0.1, 1).into_mesh();
        assert!(mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_some());
        assert!(mesh.attribute(ATTRIBUTE_ROOT).is_some());
        assert!(mesh.attribute(ATTRIBUTE_SHAPE).is_some());
        assert!(mesh.attribute(ATTRIBUTE_VARIANT).is_some());
        assert!(mesh.indices().is_some());
    }

    /// Blade length and width are quantised to a byte here and expanded back
    /// out in the shader, which only works if both ends agree on the range.
    ///
    /// Drift here is nastier than it sounds: the grass still draws, it is just
    /// systematically the wrong size, and nothing else notices.
    #[test]
    fn shader_ranges_match_this_module() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/grass.wgsl"
        );
        let source = std::fs::read_to_string(path).expect("the grass shader must exist");

        for (name, value) in [
            ("LENGTH_MIN", LENGTH_RANGE.0),
            ("LENGTH_MAX", LENGTH_RANGE.1),
            ("WIDTH_MIN", WIDTH_RANGE.0),
            ("WIDTH_MAX", WIDTH_RANGE.1),
        ] {
            let needle = format!("const {name}: f32 = {value:?};");
            assert!(
                source.contains(&needle),
                "grass.wgsl must declare `{needle}` to stay in step with blade.rs"
            );
        }
    }

    #[test]
    fn quantisation_covers_the_full_byte_range() {
        assert_eq!(quantise(0.0), 0);
        assert_eq!(quantise(1.0), 255);
        assert_eq!(quantise(-5.0), 0);
        assert_eq!(quantise(5.0), 255);
    }

    fn distinct(values: &[u8]) -> usize {
        let mut seen = values.to_vec();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    }
}

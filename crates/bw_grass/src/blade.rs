//! Blade geometry.
//!
//! Grass is drawn as tapered ribbons rather than alpha-cut cards, and at
//! pixel-art resolution that choice earns more than it did at full resolution:
//! no overdraw on empty pixels, correct depth sorting for free from the opaque
//! depth buffer, and — the one that matters most here — the shader controls the
//! exact pixel footprint, so a stroke can be pinned to a whole number of pixels.
//! A textured card's footprint is whatever the texture and the filter decide.
//!
//! ## Two layers, because one never looks like a meadow
//!
//! Real grass is a dense low mat with taller plants standing out of it, and
//! that is what the reference art shows. Either layer alone fails in a specific,
//! recognisable way:
//!
//! | Layer | Length | Job | Alone it looks like |
//! |---|---|---|---|
//! | **Mat** | [`MAT_LENGTH`] | Cover the ground completely; carry fine texture | A bristle brush or a flat texture |
//! | **Tuft** | [`TUFT_LENGTH`] | Break the silhouette; give the canopy a grain | A sparse field of weeds on bare soil |
//!
//! The mat is scattered evenly, two pixels wide and only a few pixels long, and
//! its job is that no pixel is ever bare. Tufts are placed far more sparsely,
//! fan four to nine blades out of a shared root, and stand two to three times
//! taller. Together they give the thing a meadow reads as: a continuous surface
//! with structure sitting on it.
//!
//! Tuft placement is jittered stratification rather than uniform random.
//! Uniform random points clump — some spots get four tufts and others none —
//! and clumping *of tufts* reads as bald patches, which is a different and much
//! worse thing than the deliberate clumping of blades within one tuft.
//!
//! Density gates a whole tuft rather than individual blades, so thin ground
//! loses plants instead of thinning every plant, which is what real patchy
//! grass does. The mat is thinned per blade, because a mat is not made of
//! plants.
//!
//! ## Blades are drawn oversized, on purpose
//!
//! At the camera height an auto-battler needs, real ankle-high grass is under a
//! pixel and could only ever be a flat texture. Every stylised RTS draws grass
//! two to three times life size for exactly this reason — it is the same
//! licence taken with trees, rocks and unit proportions. [`TUFT_LENGTH`] tops
//! out near waist height on a person.
//!
//! ## What a vertex carries
//!
//! Position is the blade's *rest* pose already projected to the screen. The
//! vertex shader overwrites it, so its real job is giving Bevy an honest
//! bounding box in the same space the shader outputs — get that wrong and
//! chunks vanish at the edges of the view.
//!
//! Everything else is packed to eight bits per channel and repeated across the
//! blade's eight vertices, so the redundancy is what costs memory, not the
//! precision.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{
    Indices, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues, VertexFormat,
};
use bevy::prelude::*;

use crate::field::GrassField;
use crate::iso;
use crate::noise::{fbm, hash_2d, unit_from_hash};

/// What a whole patch of tufts shares.
#[derive(Clone, Copy, Debug)]
pub struct Parent {
    /// Bias on the orientation field, in 0..1 around a half.
    pub lean: f32,
    /// Multiplier on plant height.
    pub scale: f32,
    /// Bias on tint, which selects a palette ramp.
    pub tint: f32,
}

/// The patch a point belongs to.
pub fn parent_of(at: Vec2, seed: u32) -> Parent {
    let cell = (at / PARENT_METRES).floor();
    let hash = hash_2d(cell.x as i32, cell.y as i32, seed ^ 0x9A7E_4C11);
    Parent {
        lean: unit_from_hash(hash),
        scale: 0.82 + 0.36 * unit_from_hash(hash.wrapping_mul(0xc2b2_ae35)),
        tint: unit_from_hash(hash.wrapping_mul(0x27d4_eb2f)),
    }
}

/// The direction grass grows at a point, in radians.
///
/// Built from two noise fields read as a vector rather than from one read as an
/// angle. A scalar field mapped onto `0..TAU` wraps, and every wrap is a seam
/// where neighbouring tufts point in opposite directions — the exact
/// discontinuity this function exists to avoid.
///
/// The result is coherent over [`ORIENTATION_METRES`], which is what gives the
/// field a grain: grass combed one way here and another way over there, with a
/// smooth turn between.
pub fn rest_orientation(at: Vec2, seed: u32) -> f32 {
    let x = at.x / ORIENTATION_METRES;
    let y = at.y / ORIENTATION_METRES;
    let dx = fbm(x, y, seed ^ 0x0A1E_1111, 3) - 0.5;
    let dy = fbm(x + 31.7, y - 12.3, seed ^ 0x51DE_2222, 3) - 0.5;
    dy.atan2(dx)
}

/// Metres per cycle of the field that decides where long blades appear.
///
/// Coarse and thresholded, so tufts come in drifts with quiet ground between
/// rather than at an even rate everywhere. This is the field that makes long
/// blades *rare* — see [`tuft_chance`].
pub const TUFT_FIELD_METRES: f32 = 9.0;

/// Metres per cycle of the field that decides how thick the short grass is.
///
/// Finer than the tuft field and never thresholded to zero: the short grass is
/// a surface, so it thins and thickens rather than starting and stopping.
pub const MAT_FIELD_METRES: f32 = 3.2;

/// How likely a long-blade tuft is at this point, in 0..1.
///
/// Raised to a power so most of the field sits near zero and tufts cluster into
/// the peaks. A uniform tuft rate is the thing that makes generated grass read
/// as wallpaper: real meadows have drifts of taller growth and stretches with
/// almost none, and it is that contrast the eye reads as a place rather than a
/// pattern.
pub fn tuft_chance(at: Vec2, seed: u32) -> f32 {
    let n = fbm(
        at.x / TUFT_FIELD_METRES,
        at.y / TUFT_FIELD_METRES,
        seed ^ 0x7F7F_0A0A,
        3,
    );
    // Remapped so the quiet ground is genuinely quiet without the peaks
    // becoming clots. An earlier curve cut at 0.32 and squared the remainder,
    // which made long blades arrive in tight knots with bald ground between —
    // rare is not the same as clumped, and the eye reads a knot as a mistake.
    ((n - 0.18) / 0.82).clamp(0.0, 1.0).powf(1.1)
}

/// Metres per cycle of the field that shifts tuft colour.
///
/// Deliberately a different scale from the density field. Real variation in a
/// meadow is not one map driving everything — a patch can be thick and pale, or
/// sparse and deep green, and it is the *disagreement* between those maps that
/// stops a field looking like a single mask applied several ways.
pub const TINT_FIELD_METRES: f32 = 5.3;

/// Which way tuft colour leans at this point, in 0..1.
pub fn tuft_tint_bias(at: Vec2, seed: u32) -> f32 {
    fbm(
        at.x / TINT_FIELD_METRES,
        at.y / TINT_FIELD_METRES,
        seed ^ 0x3C3C_9E9E,
        3,
    )
    .clamp(0.0, 1.0)
}

/// How thick the short grass is at this point, in 0..1.
pub fn mat_thickness(at: Vec2, seed: u32) -> f32 {
    let n = fbm(
        at.x / MAT_FIELD_METRES,
        at.y / MAT_FIELD_METRES,
        seed ^ 0x2B2B_5151,
        3,
    );
    // Never all the way off. The short grass is the surface everything else
    // sits on; punching holes in it would show the base layer as bald patches.
    0.45 + 0.55 * n.clamp(0.0, 1.0)
}

/// Rings up the blade.
///
/// Three segments. Far fewer than a smooth renderer would need, and that is
/// correct here: the tallest blade is about eleven pixels and every ring is
/// snapped to the pixel grid, so the grid quantises the curve much more coarsely
/// than the geometry does. Extra rings cost memory and land on the same pixels.
pub const RINGS: usize = 4;

/// Vertices per blade.
pub const VERTICES_PER_BLADE: usize = RINGS * 2;

/// Indices per blade.
pub const INDICES_PER_BLADE: usize = (RINGS - 1) * 6;

/// Edge of a grass chunk, in metres.
pub const CHUNK_METRES: f32 = 4.0;

/// Blade length range across both layers, in metres.
///
/// Packed to a byte against this range, so it has to span everything either
/// layer can produce.
pub const LENGTH_RANGE: (f32, f32) = (0.02, 0.50);

/// Blade half-width range at the base, in metres.
///
/// The shader rounds this to whole canvas pixels, so what these really select
/// between is a one-pixel stroke and a two-pixel one.
pub const WIDTH_RANGE: (f32, f32) = (0.010, 0.022);

/// How far a blade leans just from having grown that way, in radians.
pub const REST_LEAN_RANGE: (f32, f32) = (0.08, 0.70);

// --- the mat ----------------------------------------------------------------

/// Mat blades per square metre at full detail.
///
/// A fraction of what it takes to cover the ground, because covering the ground
/// is no longer the mat's job — [`crate::ground`] does that. What is left is
/// the harder job: individual strokes that have to read *as* strokes.
///
/// Forty-two is a compromise between two failures that pull opposite ways, and
/// both were measured. Below about thirty the base shows through as a wash with
/// marks scattered on it. Above about seventy every mark lands on another mark
/// and the field averages to one flat tone — a standard deviation of 0.037
/// against the art target's 0.105, which is *flatter* than the sparse version
/// despite carrying three times the geometry.
///
/// It came down by a factor of eight while the
/// base layer was drawing grass marks of its own, because two sets of marks
/// laid over each other average out to one tone. Once the base went back to
/// being tone — Perlin and grain, no strokes — the marks had to come from
/// somewhere, and geometry is the only place they can come from without
/// weaving: a blade has a direction because it *is* one, so no number of them
/// forms a lattice.
pub const MAT_PER_SQUARE_METRE: f32 = 42.0;

/// Short-grass length, in metres. Three to seven pixels at the default camera.
pub const MAT_LENGTH: (f32, f32) = (0.09, 0.20);

/// Short-grass half-width — one pixel.
pub const MAT_WIDTH: (f32, f32) = (0.010, 0.016);

/// Mat blade rest lean, in radians. Lower than a tuft's: the mat lies down.
pub const MAT_LEAN: (f32, f32) = (0.08, 0.46);

// --- tufts ------------------------------------------------------------------

/// Tufts per square metre at full detail.
pub const TUFTS_PER_SQUARE_METRE: f32 = 7.0;

/// Blades in a tuft, inclusive.
///
/// The spread matters: tufts of identical size tile the eye even when their
/// positions do not.
pub const TUFT_BLADES: (usize, usize) = (3, 6);

/// How far a blade's root may sit from its tuft's centre, in metres.
///
/// Small. A tuft's blades come out of very nearly one point — the splay is in
/// which way they *lean*, not in where they are rooted. Widen this and the
/// tufts dissolve back into even scatter.
pub const TUFT_RADIUS: f32 = 0.085;

/// Angle a tuft's blades fan across, in radians.
///
/// Varied per tuft between these, and mostly narrow. A tuft that fans the whole
/// way round is a *starburst*, and a field of starbursts is the single most
/// recognisable tell of procedural grass — every clump is the same rotationally
/// symmetric shape, so no clump has a silhouette and rotating them changes
/// nothing. Narrow arcs give combs and fans, which have a direction and
/// therefore a shape. The wide end is kept as a rare accent.
pub const FAN_ARC: (f32, f32) = (0.55, 2.3);

/// Weighting that keeps wide fans rare.
///
/// The draw is raised to this power, so most tufts land near the narrow end.
const FAN_BIAS: f32 = 2.6;

/// Long-blade length, in metres. Nine to seventeen pixels at the default camera.
pub const TUFT_LENGTH: (f32, f32) = (0.26, 0.50);

/// Long-blade half-width — one pixel, occasionally two.
///
/// Thin. An earlier revision made these two to three pixels wide on the theory
/// that a stroke needs body to have a silhouette. It does not: at this density
/// broad blades merge into slabs and the field reads as chunky felt rather than
/// as grass. What separates a long blade from the short grass around it is
/// *length and value*, not weight — it is longer, and it is a clear step
/// brighter. Both of those survive being one pixel wide; a slab does not.
pub const TUFT_WIDTH: (f32, f32) = (0.012, 0.022);

/// Long-blade rest lean, in radians.
pub const TUFT_LEAN: (f32, f32) = (0.10, 0.70);

/// Metres over which the rest orientation field stays coherent.
///
/// Neighbouring tufts must lean broadly the same way. Drawing each tuft's
/// facing from its own hash — the obvious implementation, and the one this
/// replaced — destroys any sense that the grass grew somewhere: adjacent plants
/// point in unrelated directions, which no meadow does, and the field reads as
/// scattered decals rather than as vegetation with a grain to it.
pub const ORIENTATION_METRES: f32 = 6.5;

/// How far a tuft may deviate from the orientation field, in radians.
const ORIENTATION_JITTER: f32 = 0.55;

/// Metres across a parent patch.
///
/// Tufts inside one patch share a scale, a tint bias and an orientation bias.
/// Correlating them is what produces islands of similar grass with quieter
/// ground between, instead of one statistically uniform sprawl — the difference
/// between a place and a texture.
pub const PARENT_METRES: f32 = 4.5;

// --- cast shadows -----------------------------------------------------------

/// Shadow quads laid down per tuft.
pub const TUFT_SHADOWS: usize = 2;

/// Shadow length, in metres. Ground-hugging.
pub const SHADOW_LENGTH: (f32, f32) = (0.025, 0.055);

/// Shadow half-width. The widest thing in the field — a shadow is a patch, not
/// a stroke.
pub const SHADOW_WIDTH: (f32, f32) = (0.030, 0.048);

/// How far a shadow is thrown from its tuft, per metre of plant height.
///
/// From the key's elevation: a 38° sun throws a shadow about 1.28 times the
/// height of what casts it. Using the rig's own number rather than a pleasing
/// one is what keeps the shadows agreeing with the way the blades are lit.
pub const SHADOW_THROW: f32 = 1.28;

/// Direction a shadow falls, in the ground plane.
///
/// Directly away from the key. `light::key()` points from the scene toward the
/// sun, so a shadow runs along its negated ground projection.
pub fn shadow_direction() -> Vec2 {
    let key = crate::light::key().direction;
    -Vec2::new(key.x, key.y).normalize()
}

/// Length the field's own length map is quoted against, for turning it into a
/// multiplier rather than an absolute.
const REFERENCE_LENGTH: f32 = 0.31;

/// Fraction of a stratum a blade or tuft may be jittered across.
const JITTER: f32 = 0.65;

const TAU: f32 = std::f32::consts::TAU;

/// World position of the blade's root.
pub const ATTRIBUTE_ROOT: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassRoot", 0x6a72_0001, VertexFormat::Float32x2);

/// `(height along blade, which side of the ribbon, length, base half-width)`.
pub const ATTRIBUTE_SHAPE: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassShape", 0x6a72_0002, VertexFormat::Unorm8x4);

/// `(rest lean direction, rest lean angle, tint, per-blade random)`.
pub const ATTRIBUTE_VARIANT: MeshVertexAttribute =
    MeshVertexAttribute::new("GrassVariant", 0x6a72_0003, VertexFormat::Unorm8x4);

/// Mean blades in a tuft.
pub fn mean_tuft_blades() -> f32 {
    (TUFT_BLADES.0 + TUFT_BLADES.1) as f32 * 0.5
}

/// Blades per square metre at full detail, before density thins anything.
pub fn blades_per_square_metre() -> f32 {
    MAT_PER_SQUARE_METRE + TUFTS_PER_SQUARE_METRE * mean_tuft_blades()
}

/// Which layer a blade belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// The dense low ground cover.
    Mat,
    /// A blade in a fanned tuft.
    Tuft,
}

/// A chunk's worth of blades, ready to become a mesh.
pub struct BladeBatch {
    positions: Vec<[f32; 3]>,
    roots: Vec<[f32; 2]>,
    shapes: Vec<[u8; 4]>,
    variants: Vec<[u8; 4]>,
    indices: Vec<u32>,
    blades: u32,
    mat_blades: u32,
    /// Centre of each tuft that survived density, in world metres.
    centres: Vec<Vec2>,
    /// Blades emitted by each tuft, in the same order. Tuft blades are
    /// contiguous and come after the mat, so these two together recover which
    /// blade belongs to which plant without storing an index on every blade.
    sizes: Vec<u16>,
}

impl BladeBatch {
    fn with_capacity(mat: usize, tufts: usize) -> Self {
        let blades = mat + tufts * TUFT_BLADES.1;
        Self {
            positions: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            roots: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            shapes: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            variants: Vec::with_capacity(blades * VERTICES_PER_BLADE),
            indices: Vec::with_capacity(blades * INDICES_PER_BLADE),
            blades: 0,
            mat_blades: 0,
            centres: Vec::with_capacity(tufts),
            sizes: Vec::with_capacity(tufts),
        }
    }

    /// How many blades were placed, across both layers.
    pub fn blades(&self) -> u32 {
        self.blades
    }

    /// How many of those are mat blades.
    pub fn mat_blades(&self) -> u32 {
        self.mat_blades
    }

    /// How many of those are tuft blades.
    pub fn tuft_blades(&self) -> u32 {
        self.blades - self.mat_blades
    }

    /// How many tufts were placed.
    pub fn tufts(&self) -> u32 {
        self.centres.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.blades == 0
    }

    /// One root position per blade, mat first.
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

    /// One base half-width per blade, in metres.
    pub fn widths(&self) -> impl Iterator<Item = f32> + '_ {
        self.shapes
            .iter()
            .step_by(VERTICES_PER_BLADE)
            .map(|shape| lerp(WIDTH_RANGE.0, WIDTH_RANGE.1, shape[3] as f32 / 255.0))
    }

    /// One rest-lean direction per blade, in radians.
    pub fn rest_angles(&self) -> impl Iterator<Item = f32> + '_ {
        self.variants
            .iter()
            .step_by(VERTICES_PER_BLADE)
            .map(|variant| variant[0] as f32 / 255.0 * TAU)
    }

    /// Which layer each blade belongs to.
    pub fn layers(&self) -> impl Iterator<Item = Layer> + '_ {
        let mat = self.mat_blades as usize;
        (0..self.blades as usize).map(
            move |index| {
                if index < mat { Layer::Mat } else { Layer::Tuft }
            },
        )
    }

    /// Tuft centres, in world metres.
    pub fn centres(&self) -> &[Vec2] {
        &self.centres
    }

    /// `(centre, blade index range)` for each tuft, indexing the blade order
    /// [`roots`](Self::roots) produces.
    pub fn tuft_spans(&self) -> impl Iterator<Item = (Vec2, std::ops::Range<usize>)> + '_ {
        let mut start = self.mat_blades as usize;
        self.centres
            .iter()
            .zip(&self.sizes)
            .map(move |(&centre, &size)| {
                let span = start..start + size as usize;
                start += size as usize;
                (centre, span)
            })
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
        let angle_t = blade.rest_angle.rem_euclid(TAU) / TAU;
        let lean_t = inverse_lerp(REST_LEAN_RANGE.0, REST_LEAN_RANGE.1, blade.rest_lean);

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
                    quantise(angle_t),
                    quantise(lean_t),
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
}

/// Turn a batch into a mesh.
impl BladeBatch {
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
    rest_angle: f32,
    rest_lean: f32,
    tint: f32,
    random: f32,
}

/// Build the blades for one chunk.
///
/// `chunk` is a chunk coordinate; the chunk covers
/// `[chunk * CHUNK_METRES, (chunk + 1) * CHUNK_METRES)` in world metres.
/// `detail` scales both layers for level of detail.
///
/// Mat blades are emitted first and tuft blades after, which is what lets
/// [`BladeBatch::tuft_spans`] recover tuft membership from two small side
/// tables instead of a per-blade index.
pub fn build_chunk(field: &GrassField, chunk: IVec2, detail: f32, seed: u32) -> BladeBatch {
    let detail = detail.clamp(0.0, 1.0);
    let area = CHUNK_METRES * CHUNK_METRES;
    let mat_target = (area * MAT_PER_SQUARE_METRE * detail).round().max(0.0) as usize;
    let tuft_target = (area * TUFTS_PER_SQUARE_METRE * detail).round().max(0.0) as usize;

    let mut batch = BladeBatch::with_capacity(mat_target, tuft_target);
    let origin = chunk.as_vec2() * CHUNK_METRES;
    let chunk_seed = seed ^ hash_2d(chunk.x, chunk.y, 0x51A5_5EED);

    build_mat(&mut batch, field, origin, mat_target, chunk_seed);
    batch.mat_blades = batch.blades;
    build_tufts(&mut batch, field, origin, tuft_target, chunk_seed);
    batch
}

/// Scatter the low ground cover.
fn build_mat(
    batch: &mut BladeBatch,
    field: &GrassField,
    origin: Vec2,
    target: usize,
    chunk_seed: u32,
) {
    if target == 0 {
        return;
    }
    let seed = chunk_seed ^ 0x4D41_5401;
    for (root, hash) in scatter(origin, target, seed) {
        // Thinned per blade rather than per plant: a mat is a surface, not a
        // collection of individuals, so it should get sparser rather than
        // patchier where the ground is thin.
        // Two gates: the simulation's own density map, and this layer's Perlin
        // thickness field. They answer different questions — the first is where
        // grass grows at all, the second is how full it is where it does.
        let keep = unit_from_hash(hash.wrapping_mul(0x85eb_ca6b));
        if keep > field.density_at_world(root) * mat_thickness(root, seed) {
            continue;
        }
        let a = unit_from_hash(hash.wrapping_mul(0xc2b2_ae35));
        let b = unit_from_hash(hash.wrapping_mul(0x27d4_eb2f));
        let c = unit_from_hash(hash.wrapping_mul(0x1656_67b1));
        let scale = field.length_at_world(root) / REFERENCE_LENGTH;

        batch.push_blade(Blade {
            root,
            length: (lerp(MAT_LENGTH.0, MAT_LENGTH.1, a) * scale)
                .clamp(LENGTH_RANGE.0, LENGTH_RANGE.1),
            width: lerp(MAT_WIDTH.0, MAT_WIDTH.1, b),
            // The same coherent field the tufts use, so the mat is combed
            // the same way as the plants standing in it. A mat with its own
            // random direction per blade cross-hatches against the tufts and
            // cancels the grain both layers were meant to share.
            rest_angle: rest_orientation(root, seed)
                + (unit_from_hash(hash.wrapping_mul(0x2545_f491)) - 0.5) * 1.1,
            rest_lean: lerp(MAT_LEAN.0, MAT_LEAN.1, c),
            tint: unit_from_hash(hash.wrapping_mul(0x7feb_352d)),
            random: unit_from_hash(hash.wrapping_mul(0x846c_a68b)),
        });
    }
}

/// Place the tufts that stand out of the mat.
fn build_tufts(
    batch: &mut BladeBatch,
    field: &GrassField,
    origin: Vec2,
    target: usize,
    chunk_seed: u32,
) {
    if target == 0 {
        return;
    }
    let seed = chunk_seed ^ 0x5455_4654;
    for (centre, hash) in scatter(origin, target, seed) {
        // A whole tuft lives or dies together, so thin ground loses plants
        // rather than thinning every plant — and long blades are additionally
        // gated by their own coarse field, which is what keeps them rare and
        // clustered instead of evenly sprinkled.
        let keep = unit_from_hash(hash.wrapping_mul(0x85eb_ca6b));
        if keep > field.density_at_world(centre) * tuft_chance(centre, seed) {
            continue;
        }
        let placed = push_tuft(batch, field, centre, hash, chunk_seed);
        batch.centres.push(centre);
        batch.sizes.push(placed as u16);
    }
}

/// The shadow a tuft throws.
///
/// Ordinary blades, laid flat and tinted to the bottom of the range so they
/// take the same path through the shader everything else does — short means
/// low in the canopy means little light, and a low tint puts them on the
/// shadow ramp. No special case anywhere downstream.
///
/// Only the tall layer casts. A cast shadow is only legible when the thing
/// casting it is clearly taller than what surrounds it, and short grass
/// shadowing short grass is just a darker mat.
fn push_tuft_shadow(batch: &mut BladeBatch, centre: Vec2, height: f32, hash: u32) {
    let direction = shadow_direction();
    let throw = height * SHADOW_THROW;

    for index in 0..TUFT_SHADOWS {
        let h = hash
            .wrapping_mul(0x5EED_5AD0)
            .wrapping_add((index as u32).wrapping_mul(0x9e37_79b9));
        let a = unit_from_hash(h);
        let b = unit_from_hash(h.wrapping_mul(0xc2b2_ae35));
        // Spread along the throw, so the shadow reads as a smear away from the
        // plant rather than as a blob beside it.
        let along = (index as f32 + 0.5) / TUFT_SHADOWS as f32;
        let across = (a - 0.5) * TUFT_RADIUS * 1.4;
        let sideways = Vec2::new(-direction.y, direction.x) * across;

        batch.push_blade(Blade {
            root: centre + direction * (throw * along) + sideways,
            length: lerp(SHADOW_LENGTH.0, SHADOW_LENGTH.1, b),
            width: lerp(SHADOW_WIDTH.0, SHADOW_WIDTH.1, a),
            // Lying along the throw, flat to the ground.
            rest_angle: direction.y.atan2(direction.x),
            rest_lean: REST_LEAN_RANGE.1,
            tint: 0.01 * a,
            random: unit_from_hash(h.wrapping_mul(0x846c_a68b)),
        });
    }
}

/// Fan one tuft's blades out from a shared centre.
fn push_tuft(
    batch: &mut BladeBatch,
    field: &GrassField,
    centre: Vec2,
    hash: u32,
    seed: u32,
) -> usize {
    let span = (TUFT_BLADES.1 - TUFT_BLADES.0 + 1) as f32;
    let count = TUFT_BLADES.0
        + ((unit_from_hash(hash.wrapping_mul(0x2545_f491)) * span) as usize).min(span as usize - 1);

    // Shared by the whole plant: which way it faces, roughly how tall it is,
    // and roughly what colour. Blades vary around these rather than
    // independently of them, which is what makes a tuft read as one plant.
    //
    // Facing comes from the orientation field plus a small jitter, and from the
    // parent patch — never from this tuft's own hash alone. See
    // `ORIENTATION_METRES`.
    let parent = parent_of(centre, seed);
    let facing = rest_orientation(centre, seed)
        + (unit_from_hash(hash.wrapping_mul(0xc2b2_ae35)) - 0.5) * ORIENTATION_JITTER
        + (parent.lean - 0.5) * 0.7;
    let plant = (0.72 + 0.56 * unit_from_hash(hash.wrapping_mul(0x27d4_eb2f))) * parent.scale;
    // Three sources at three scales: this plant's own draw, its parent patch,
    // and a broad colour field that crosses both.
    let tuft_tint = (unit_from_hash(hash.wrapping_mul(0x7feb_352d)) * 0.38
        + parent.tint * 0.27
        + tuft_tint_bias(centre, seed) * 0.35)
        .clamp(0.0, 1.0);
    let scale = field.length_at_world(centre) / REFERENCE_LENGTH;

    // The shadow first, so the plant draws over its own root end of it.
    push_tuft_shadow(batch, centre, TUFT_LENGTH.1 * plant * scale, hash);

    // Mostly narrow, occasionally wide. See `FAN_ARC`.
    let fan = lerp(
        FAN_ARC.0,
        FAN_ARC.1,
        unit_from_hash(hash.wrapping_mul(0x1656_67b1)).powf(FAN_BIAS),
    );

    for index in 0..count {
        let blade_hash = hash
            .wrapping_mul(0x9e37_79b9)
            .wrapping_add((index as u32).wrapping_mul(0x85eb_ca6b) ^ 0x1656_67b1);
        let a = unit_from_hash(blade_hash);
        let b = unit_from_hash(blade_hash.wrapping_mul(0xc2b2_ae35));
        let c = unit_from_hash(blade_hash.wrapping_mul(0x27d4_eb2f));
        let d = unit_from_hash(blade_hash.wrapping_mul(0x1656_67b1));
        let e = unit_from_hash(blade_hash.wrapping_mul(0x2545_f491));

        // Spread evenly across the fan rather than at random. Random angles
        // leave gaps and doubled-up blades, and a tuft with a gap in it stops
        // reading as a fan and starts reading as two smaller tufts.
        let across = if count > 1 {
            index as f32 / (count - 1) as f32 - 0.5
        } else {
            0.0
        };
        let rest_angle = facing + across * fan + (a - 0.5) * 0.35;

        // Blades at the edge of the fan lean furthest, which is what gives a
        // tuft its splayed silhouette instead of a bundle of parallel sticks.
        let rest_lean = lerp(
            TUFT_LEAN.0,
            TUFT_LEAN.1,
            (across.abs() * 1.2 + b * 0.85).clamp(0.0, 1.0),
        );

        // Middle blades run longest. A tuft whose blades are all one length
        // reads as a brush; tapering toward the edges gives it a crown.
        let taper = 1.0 - across.abs() * 0.55;
        // Squared, so most blades in a tuft are short and a few run long.
        // An even spread of lengths gives a tuft a flat crown; real ones are
        // mostly low with a couple of stems standing out.
        let length = (lerp(TUFT_LENGTH.0, TUFT_LENGTH.1, c * c) * plant * taper * scale)
            .clamp(LENGTH_RANGE.0, LENGTH_RANGE.1);

        batch.push_blade(Blade {
            root: centre + Vec2::new(rest_angle.cos(), rest_angle.sin()) * (TUFT_RADIUS * d),
            length,
            width: lerp(TUFT_WIDTH.0, TUFT_WIDTH.1, e),
            rest_angle,
            rest_lean,
            // Varied around the tuft's tint, not independently of it.
            // Wide around the tuft's own tint. Blades of one plant differ in
            // colour as much as they differ in length — a fan of identically
            // tinted strokes reads as a decal however varied its geometry.
            tint: (tuft_tint + (a - 0.5) * 0.55).clamp(0.0, 1.0),
            random: unit_from_hash(blade_hash.wrapping_mul(0x846c_a68b)),
        });
    }

    TUFT_SHADOWS + count
}

/// Jittered stratified points across one chunk, with the hash that made each.
///
/// Stratifying first and jittering within each stratum keeps the spacing
/// roughly even while removing any trace of a lattice. Uniform random points
/// clump, and clumping in ground cover reads as bald patches.
fn scatter(origin: Vec2, target: usize, seed: u32) -> impl Iterator<Item = (Vec2, u32)> {
    let strata = (target as f32).sqrt().ceil().max(1.0) as i32;
    let stride = CHUNK_METRES / strata as f32;

    (0..strata).flat_map(move |sy| {
        (0..strata).map(move |sx| {
            let hash = hash_2d(sx, sy, seed);
            // Jittered across most of the stratum but not all of it. Full-width
            // jitter lets points in adjacent strata land almost on top of each
            // other, which is the clumping the stratification was meant to
            // avoid; leaving a margin keeps a minimum spacing while still
            // hiding the lattice.
            let jitter = Vec2::new(
                unit_from_hash(hash),
                unit_from_hash(hash.wrapping_mul(0x9e37_79b9)),
            );
            let jitter = Vec2::splat(0.5) + (jitter - Vec2::splat(0.5)) * JITTER;
            // Offset alternate rows by half a stratum. Jitter alone still
            // leaves the strata lined up in columns, and at these densities
            // that shows as faint vertical banding across the whole field.
            let row_offset = if sy % 2 == 0 { 0.0 } else { 0.5 };
            let point = origin + (Vec2::new(sx as f32 + row_offset, sy as f32) + jitter) * stride;
            (point, hash)
        })
    })
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
    fn each_layer_is_thinned_by_its_own_field() {
        // The per-square-metre constants are a *ceiling*, not a target. Each
        // layer is then gated by its own Perlin field, and the whole point of
        // those fields is that they thin unevenly: the short grass thickens and
        // thins as a surface, while long blades come in drifts with quiet
        // ground between.
        //
        // So the useful assertion is not "we placed what we asked for" — that
        // would mean the fields were doing nothing. It is that each layer lands
        // in the band its field implies, and that the sparse layer really is
        // much sparser than the dense one.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        let area = CHUNK_METRES * CHUNK_METRES;

        // `mat_thickness` averages about 0.72 and never reaches zero.
        let mat_ratio = batch.mat_blades() as f32 / (area * MAT_PER_SQUARE_METRE);
        assert!((0.55..=1.00).contains(&mat_ratio), "mat {mat_ratio}");

        // `tuft_chance` is thresholded, so most of the field carries none.
        let tuft_ratio = batch.tufts() as f32 / (area * TUFTS_PER_SQUARE_METRE);
        assert!((0.05..=0.80).contains(&tuft_ratio), "tufts {tuft_ratio}");
        assert!(
            tuft_ratio < mat_ratio,
            "long blades must be rarer than short grass: {tuft_ratio} vs {mat_ratio}"
        );
    }

    #[test]
    fn the_layer_fields_actually_vary() {
        // A field that returned a constant would thin both layers uniformly and
        // look exactly like turning the density down — no drifts, no quiet
        // ground, none of the variety the fields exist for.
        let mut tuft_low = f32::MAX;
        let mut tuft_high = f32::MIN;
        let mut mat_low = f32::MAX;
        let mut mat_high = f32::MIN;
        for i in 0..64 {
            for j in 0..64 {
                let at = Vec2::new(i as f32 * 1.7 - 50.0, j as f32 * 1.7 - 50.0);
                let tuft = tuft_chance(at, 11);
                let mat = mat_thickness(at, 11);
                tuft_low = tuft_low.min(tuft);
                tuft_high = tuft_high.max(tuft);
                mat_low = mat_low.min(mat);
                mat_high = mat_high.max(mat);
                assert!((0.0..=1.0).contains(&tuft), "tuft chance {tuft}");
                assert!((0.0..=1.0).contains(&mat), "mat thickness {mat}");
            }
        }
        assert!(tuft_high - tuft_low > 0.4, "tuft field is flat");
        assert!(mat_high - mat_low > 0.2, "mat field is flat");
        // The tuft field must actually reach zero somewhere, or "rare" is only
        // ever "slightly less common".
        assert_eq!(tuft_low, 0.0, "the tuft field never empties");
        // The mat field must not, or the surface would show bald patches.
        assert!(mat_low > 0.2, "the mat field empties: {mat_low}");
    }

    #[test]
    fn both_layers_are_present() {
        // Either one alone is a recognisable failure — a bristle brush or weeds
        // on bare soil — and both are easy to produce by accident while tuning
        // densities.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        assert!(batch.mat_blades() > 0, "no mat");
        assert!(batch.tuft_blades() > 0, "no tufts");
        assert_eq!(batch.mat_blades() + batch.tuft_blades(), batch.blades());
    }

    #[test]
    fn the_mat_is_shorter_and_wider_than_the_tufts() {
        // The whole reason for two layers. If these converged there would be
        // one layer wearing two names.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 3);
        let lengths: Vec<f32> = batch.lengths().collect();
        let widths: Vec<f32> = batch.widths().collect();
        let layers: Vec<Layer> = batch.layers().collect();

        let mean = |want: Layer, values: &[f32]| {
            let picked: Vec<f32> = values
                .iter()
                .zip(&layers)
                .filter(|&(_, &layer)| layer == want)
                .map(|(&v, _)| v)
                .collect();
            picked.iter().sum::<f32>() / picked.len() as f32
        };

        // The margin is not tight. Once the ground took over covering the
        // earth, the mat was free to grow long enough to read as strokes rather
        // than as fill, so the two layers are closer in length than they were —
        // they are still distinguishable, and width separates them further.
        assert!(mean(Layer::Mat, &lengths) < mean(Layer::Tuft, &lengths) * 0.8);
        // Tufts are the *broader* layer, which is the opposite of what a
        // physical reading suggests: a tall plant's blades are not thicker than
        // a short one's. It is a drawing decision. From a camera looking this
        // far down, a one-pixel blade is an edge rather than a shape, and a
        // plant made of edges cannot be picked out of a textured field however
        // tall it is. Giving tufts body is what makes them read as plants.
        assert!(mean(Layer::Tuft, &widths) > mean(Layer::Mat, &widths));
    }

    #[test]
    fn blade_count_per_tuft_stays_in_range() {
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        // A tuft emits its fan *and* the shadow it throws.
        let fan = batch.tuft_blades() as f32 / batch.tufts() as f32 - TUFT_SHADOWS as f32;
        assert!(
            (TUFT_BLADES.0 as f32..=TUFT_BLADES.1 as f32).contains(&fan),
            "{fan} fan blades per tuft"
        );
        // And the average should land near the middle of the range rather than
        // pinned to one end, which is what a biased size draw looks like.
        assert!((mean_tuft_blades() - fan).abs() < 1.0, "{fan}");
    }

    #[test]
    fn detail_scales_both_layers_down() {
        let full = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        let quarter = build_chunk(&field(), IVec2::ZERO, 0.25, 1);
        assert!(quarter.mat_blades() < full.mat_blades());
        assert!(quarter.tufts() < full.tufts());
        assert!(quarter.blades() > 0);
        assert_eq!(build_chunk(&field(), IVec2::ZERO, 0.0, 1).blades(), 0);
    }

    #[test]
    fn blades_stay_inside_their_chunk() {
        // Blades straying outside would be culled with the wrong chunk and pop
        // at the edges of the view. Tufts are placed inside the chunk and their
        // blades sit within TUFT_RADIUS of the centre, so that is the margin.
        let batch = build_chunk(&field(), IVec2::new(2, -3), 1.0, 1);
        let origin = Vec2::new(2.0, -3.0) * CHUNK_METRES;
        // Shadows reach furthest, so they set the margin.
        let margin = TUFT_RADIUS * 1.7 + TUFT_LENGTH.1 * SHADOW_THROW;
        for root in batch.roots() {
            assert!(
                root.x >= origin.x - margin && root.x <= origin.x + CHUNK_METRES + margin,
                "{root:?}"
            );
            assert!(
                root.y >= origin.y - margin && root.y <= origin.y + CHUNK_METRES + margin,
                "{root:?}"
            );
        }
    }

    #[test]
    fn a_tufts_blades_stay_with_their_tuft() {
        // The property that makes a tuft a plant. If roots drifted this would
        // silently become even scatter with extra steps.
        //
        // The shadow is exempt, and has to be: a cast shadow is thrown *away*
        // from what casts it, so it lands outside the tuft's radius by
        // construction. It is checked separately below.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 5);
        let roots: Vec<Vec2> = batch.roots().collect();
        assert!(batch.tufts() > 5);
        for (centre, span) in batch.tuft_spans() {
            for root in &roots[span.start + TUFT_SHADOWS..span.end] {
                assert!(
                    root.distance(centre) <= TUFT_RADIUS + 1e-4,
                    "{root:?} is {:.3}m from {centre:?}",
                    root.distance(centre)
                );
            }
        }
    }

    #[test]
    fn a_tuft_throws_its_shadow_away_from_the_key() {
        // A shadow on the wrong side of the plant is worse than no shadow: it
        // contradicts the lighting every other surface in the scene agrees on,
        // and the eye notices that long before it can say why.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 5);
        let roots: Vec<Vec2> = batch.roots().collect();
        let away = shadow_direction();
        assert!(batch.tufts() > 5);
        for (centre, span) in batch.tuft_spans() {
            for root in &roots[span.start..span.start + TUFT_SHADOWS] {
                let offset = *root - centre;
                assert!(
                    offset.dot(away) > 0.0,
                    "shadow at {root:?} falls toward the key, not away from it"
                );
            }
        }
    }

    #[test]
    fn the_shadow_direction_opposes_the_key() {
        // Derived from the rig rather than authored, so it cannot drift if the
        // sun moves.
        let key = crate::light::key().direction;
        let flat = Vec2::new(key.x, key.y).normalize();
        assert!((shadow_direction() + flat).length() < 1e-5);
    }

    #[test]
    fn a_tufts_blades_fan_out() {
        // A tuft whose blades all lean the same way is a bundle of sticks. The
        // fan is what makes it read as a plant.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 5);
        let angles: Vec<f32> = batch.rest_angles().collect();
        let mut fanned = 0;
        let mut total = 0;
        for (_, span) in batch.tuft_spans() {
            if span.len() < 3 {
                continue;
            }
            total += 1;
            // Angular spread from one edge of the fan to the other, measured
            // against the narrowest fan the generator can produce.
            //
            // Circular *concentration* was the obvious measure and is the wrong
            // one here: a deliberately narrow fan — which most tufts now are,
            // because starbursts are the tell of procedural grass — has a
            // concentration above 0.99 while still being a perfectly good fan.
            // Spread says what the test actually means.
            let mut low = f32::MAX;
            let mut high = f32::MIN;
            let first = angles[span.start];
            for &angle in &angles[span.clone()] {
                // Relative to the first blade and wrapped into ±π, so the
                // measurement does not break across the angle seam.
                let delta = (angle - first).rem_euclid(TAU);
                let delta = if delta > std::f32::consts::PI {
                    delta - TAU
                } else {
                    delta
                };
                low = low.min(delta);
                high = high.max(delta);
            }
            if high - low > FAN_ARC.0 * 0.4 {
                fanned += 1;
            }
        }
        assert!(total > 5, "need tufts to measure");
        assert_eq!(fanned, total, "{fanned} of {total} tufts fan out");
    }

    #[test]
    fn tuft_spans_account_for_every_tuft_blade() {
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 9);
        let counted: usize = batch.tuft_spans().map(|(_, span)| span.len()).sum();
        assert_eq!(counted, batch.tuft_blades() as usize);
        // And the spans must tile the tuft blades without a gap or an overlap,
        // starting where the mat ends.
        let mut expected = batch.mat_blades() as usize;
        for (_, span) in batch.tuft_spans() {
            assert_eq!(span.start, expected);
            expected = span.end;
        }
        assert_eq!(expected, batch.blades() as usize);
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
    fn the_two_layers_do_not_share_a_layout() {
        // Both layers scatter with the same routine, so a shared seed would
        // plant every tuft on top of a mat blade in a visible regular pattern.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 11);
        let roots: Vec<Vec2> = batch.roots().collect();
        let mat = &roots[..batch.mat_blades() as usize];
        let coincident = batch
            .centres()
            .iter()
            .filter(|centre| mat.iter().any(|root| root.distance(**centre) < 1e-4))
            .count();
        assert_eq!(
            coincident, 0,
            "{coincident} tufts sit exactly on a mat blade"
        );
    }

    #[test]
    fn a_chunk_is_reproducible() {
        let a = build_chunk(&field(), IVec2::new(1, 1), 1.0, 7);
        let b = build_chunk(&field(), IVec2::new(1, 1), 1.0, 7);
        assert_eq!(a.roots, b.roots);
        assert_eq!(a.shapes, b.shapes);
        assert_eq!(a.variants, b.variants);
    }

    #[test]
    fn tufts_are_spread_rather_than_clumped() {
        // The property jittered stratification buys, measured on tuft centres
        // rather than on blades — blades within a tuft are *meant* to clump.
        // Measured as the smallest nearest-neighbour distance over the mean:
        // uniform random points score near zero because some pairs land almost
        // on top of each other.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        let centres = batch.centres();
        assert!(centres.len() > 20, "need enough tufts to measure");

        let nearest: Vec<f32> = centres
            .iter()
            .map(|&p| {
                centres
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
        let batch = build_chunk(&field(), IVec2::ZERO, 0.4, 1);
        let blades = batch.blades() as usize;
        assert_eq!(batch.positions.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.roots.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.shapes.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.variants.len(), blades * VERTICES_PER_BLADE);
        assert_eq!(batch.indices.len(), blades * INDICES_PER_BLADE);
    }

    #[test]
    fn indices_are_all_in_range() {
        let batch = build_chunk(&field(), IVec2::ZERO, 0.4, 1);
        let vertices = batch.positions.len() as u32;
        assert!(batch.indices.iter().all(|&i| i < vertices));
    }

    #[test]
    fn blades_vary_in_length_and_width() {
        // Identical blades read as a printed texture rather than as grass.
        let batch = build_chunk(&field(), IVec2::ZERO, 1.0, 1);
        let lengths: Vec<u8> = batch.shapes.iter().map(|s| s[2]).collect();
        let widths: Vec<u8> = batch.shapes.iter().map(|s| s[3]).collect();
        assert!(
            distinct(&lengths) > 20,
            "only {} lengths",
            distinct(&lengths)
        );
        assert!(distinct(&widths) > 20, "only {} widths", distinct(&widths));
    }

    #[test]
    fn bare_ground_grows_no_grass() {
        let mut bare = GrassField::new(128, 0.15, 3);
        bare.make_uniform(0.24, 1.0);
        bare.set_density_everywhere(0.0);
        let batch = build_chunk(&bare, IVec2::ZERO, 1.0, 1);
        assert_eq!(batch.blades(), 0);
        assert_eq!(batch.tufts(), 0);
    }

    #[test]
    fn the_mesh_carries_every_attribute() {
        let mesh = build_chunk(&field(), IVec2::ZERO, 0.3, 1).into_mesh();
        assert!(mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_some());
        assert!(mesh.attribute(ATTRIBUTE_ROOT).is_some());
        assert!(mesh.attribute(ATTRIBUTE_SHAPE).is_some());
        assert!(mesh.attribute(ATTRIBUTE_VARIANT).is_some());
        assert!(mesh.indices().is_some());
    }

    #[test]
    fn both_layers_fit_inside_the_packed_ranges() {
        // Length, width and lean are packed to a byte against the shared
        // ranges. A layer that outgrew them would be silently clamped, and the
        // symptom would be a whole layer subtly the wrong size.
        assert!(MAT_LENGTH.0 >= LENGTH_RANGE.0 && TUFT_LENGTH.1 <= LENGTH_RANGE.1);
        assert!(TUFT_WIDTH.0 >= WIDTH_RANGE.0 && MAT_WIDTH.1 <= WIDTH_RANGE.1);
        assert!(MAT_LEAN.0 >= REST_LEAN_RANGE.0 && TUFT_LEAN.1 <= REST_LEAN_RANGE.1);
    }

    /// Blade length, width and rest lean are quantised to a byte here and
    /// expanded back out in the shader, which only works if both ends agree on
    /// the range.
    ///
    /// Drift here is nastier than it sounds: the grass still draws, it is just
    /// systematically the wrong size or leaning the wrong amount, and nothing
    /// else notices.
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
            ("REST_LEAN_MIN", REST_LEAN_RANGE.0),
            ("REST_LEAN_MAX", REST_LEAN_RANGE.1),
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

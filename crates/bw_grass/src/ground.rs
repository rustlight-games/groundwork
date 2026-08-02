//! The ground the grass grows out of.
//!
//! A pixel artist blocking in a field paints the green first and strokes blades
//! over it. This is the green. Structurally it exists because no canopy is
//! opaque — something always shows between the blades — and whatever shows has
//! to be a colour that belongs to the field. Before this layer existed, roughly
//! a sixth of every frame was the window's clear colour showing through, which
//! reads as holes punched in the world rather than as shade between plants.
//!
//! It is deliberately *not* a texture. The colour comes from two scales of
//! noise evaluated per fragment against world position, so it never tiles, it
//! costs no memory, and it lands on the same palette the blades use.
//!
//! ## Four vertices
//!
//! Both the isometric projection and the depth function are linear in world
//! position, so one quad interpolates world position and depth exactly across
//! the entire field. A denser mesh would compute the same numbers more slowly.
//!
//! ## Depth
//!
//! The ground sits at `z = 0`, which is exactly where a blade's root sits, so
//! the two would z-fight along every root. [`DEPTH_BIAS`] pushes the ground a
//! hair further from the camera. It has to be a bias in depth rather than a
//! separate render pass, because grass and ground genuinely do interleave: a
//! blade leaning downhill can pass behind ground that is nearer the camera.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::VertexFormat;
use bevy::mesh::{
    Indices, Mesh, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dKey};

use crate::iso;
use crate::palette;

/// Where the ground shader lives.
pub const SHADER_PATH: &str = "shaders/ground.wgsl";

/// How far behind the grass roots the ground is pushed, in depth units.
///
/// Small enough to be invisible, large enough to beat floating-point noise in
/// the interpolated depth. Without it every blade draws a shimmering ring of
/// z-fighting around its own root.
pub const DEPTH_BIAS: f32 = 0.02;

/// World position of the ground point, for the fragment shader.
pub const ATTRIBUTE_WORLD: MeshVertexAttribute =
    MeshVertexAttribute::new("GroundWorld", 0x6a72_0011, VertexFormat::Float32x2);

/// Per-frame constants for the ground.
///
/// Field order matches `ground.wgsl`; see [`crate::material::GrassSettings`] for
/// why that is load-bearing. The seven scalars after the two vectors round the
/// header to a whole number of sixteen-byte rows before the palette.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct GroundSettings {
    pub field_origin: Vec2,
    pub field_inverse_extent: Vec2,
    pub field_resolution: f32,
    /// Darkest shade the ground reaches, as a fraction of a ramp.
    pub shade_low: f32,
    /// Lightest shade the ground reaches.
    ///
    /// Both of these stay below the range the blades occupy. The ground is the
    /// floor of a canopy; if it climbs into the blades' range it stops reading
    /// as something they are standing in and starts reading as a green
    /// backdrop they are pasted onto.
    pub shade_high: f32,
    /// Metres per cycle of the broad variation — the sweep of the land.
    pub patch_metres: f32,
    /// Metres per cycle of the fine variation, which breaks up the broad one so
    /// it does not read as an airbrushed gradient.
    pub mottle_metres: f32,
    /// Strength of the ordered dither between palette steps, in steps.
    pub dither: f32,
    /// How far the fine grass strokes swing the shade, in ramp fractions.
    ///
    /// This is where most of the frame's local contrast comes from. The art
    /// target measures a mean neighbouring-pixel difference of 0.085; a base of
    /// smooth noise with blades on top measured 0.015, and the gap was almost
    /// entirely this.
    pub stroke_strength: f32,
    pub palette: [Vec4; palette::PALETTE_SIZE],
}

impl Default for GroundSettings {
    fn default() -> Self {
        Self {
            field_origin: Vec2::ZERO,
            field_inverse_extent: Vec2::ONE,
            field_resolution: 1.0,
            // Low and narrow. The ground is the quiet half of the image: it
            // gives the field its large-scale colour and then gets out of the
            // way. Widen this and it starts competing with the blades for
            // attention, and the picture loses the one thing that lets a stroke
            // read as a stroke — somewhere calm for it to sit against.
            // Range and *harshness* are separate dials, and conflating them
            // was a mistake worth recording. Cutting the stroke swing to soften
            // harsh dark speckle flattened the whole field to a third of the
            // target's spread. The speckle came from pixels dropping onto the
            // shadow ramp, which `SHADOW_BELOW` controls; the swing is what
            // gives the surface relief at all.
            shade_low: 0.015,
            shade_high: 0.135,
            // Around the scale of a small clearing, so a screenful contains
            // several light and dark regions rather than one gradient.
            patch_metres: 11.0,
            mottle_metres: 0.95,
            // Lower than the blades'. The ground is a broad, smooth surface, so
            // its dither has nothing fine to hide behind and reads as grain in
            // its own right.
            dither: 0.35,
            stroke_strength: 0.75,
            palette: palette::flattened(),
        }
    }
}

/// The material the ground draws with.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct GroundMaterial {
    #[uniform(0)]
    pub settings: GroundSettings,
    #[texture(1, sample_type = "float", filterable = false)]
    pub state: Handle<Image>,
}

impl Material2d for GroundMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Opaque
    }

    fn specialize(
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
            ATTRIBUTE_WORLD.at_shader_location(1),
        ])?];
        Ok(())
    }
}

/// A quad covering `[-half_extent, half_extent]` on both world axes.
pub fn ground_mesh(half_extent: f32) -> Mesh {
    let corners = [
        Vec2::new(-half_extent, -half_extent),
        Vec2::new(half_extent, -half_extent),
        Vec2::new(half_extent, half_extent),
        Vec2::new(-half_extent, half_extent),
    ];

    let positions: Vec<[f32; 3]> = corners
        .iter()
        .map(|corner| {
            let mut projected = iso::project_to_vertex(corner.extend(0.0));
            projected.z -= DEPTH_BIAS;
            projected.to_array()
        })
        .collect();
    let world: Vec<[f32; 2]> = corners.iter().map(|corner| corner.to_array()).collect();

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(ATTRIBUTE_WORLD, world)
    // Two triangles. Wound the same way as a blade's, and back faces are not
    // culled anywhere in this crate, so the order is a formality.
    .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ground_sits_behind_the_grass_that_grows_out_of_it() {
        // A blade's root is at z = 0, which is exactly where the ground is.
        // Equal depth means z-fighting along every root in the field.
        let mesh = ground_mesh(10.0);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the ground must carry positions");
        };
        for (position, corner) in positions.iter().zip([
            Vec2::new(-10.0, -10.0),
            Vec2::new(10.0, -10.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(-10.0, 10.0),
        ]) {
            let root = iso::depth(corner.extend(0.0));
            assert!(
                position[2] < root,
                "ground at {:?} is not behind a root at {root}",
                position[2]
            );
        }
    }

    #[test]
    fn the_ground_covers_the_requested_extent() {
        let mesh = ground_mesh(32.0);
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(world)) =
            mesh.attribute(ATTRIBUTE_WORLD)
        else {
            panic!("the ground must carry world positions");
        };
        let xs: Vec<f32> = world.iter().map(|w| w[0]).collect();
        let ys: Vec<f32> = world.iter().map(|w| w[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), -32.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 32.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), -32.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 32.0);
    }

    #[test]
    fn four_vertices_are_enough() {
        // Both the projection and the depth function are linear in world
        // position, so interpolation across one quad is exact. If this ever
        // needs subdividing, one of those two stopped being linear and a lot
        // more than the ground is broken.
        let mesh = ground_mesh(5.0);
        assert_eq!(mesh.count_vertices(), 4);

        let mid = Vec2::new(2.5, -1.0);
        let a = iso::project_to_vertex(Vec3::new(-5.0, -5.0, 0.0));
        let b = iso::project_to_vertex(Vec3::new(5.0, 5.0, 0.0));
        let t = (mid.x + 5.0) / 10.0;
        let u = (mid.y + 5.0) / 10.0;
        // Bilinear interpolation of the corners must land on the true depth.
        let interpolated = a.z + (b.z - a.z) * (t + u) * 0.5;
        assert!((interpolated - iso::depth(mid.extend(0.0))).abs() < 1e-4);
    }

    #[test]
    fn the_ground_stays_below_the_blades() {
        // If the ground climbed into the range the blades use, the field would
        // stop reading as grass standing in something and start reading as
        // sprites on a green backdrop.
        let settings = GroundSettings::default();
        assert!(settings.shade_low < settings.shade_high);
        assert!(settings.shade_high < 0.6);
    }

    #[test]
    fn the_two_noise_scales_are_well_separated() {
        // Octaves close in scale add up to mush rather than to structure.
        let settings = GroundSettings::default();
        assert!(settings.patch_metres > settings.mottle_metres * 3.0);
    }

    #[test]
    fn the_uniform_lays_out_as_the_shader_expects() {
        use bevy::render::render_resource::ShaderSize;
        let header = 16 + 8 * 4;
        assert_eq!(header % 16, 0, "the palette must start on a 16-byte row");
        assert_eq!(
            GroundSettings::SHADER_SIZE.get() as usize,
            header + palette::PALETTE_SIZE * 16
        );
    }
}

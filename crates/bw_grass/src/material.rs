//! The grass material and the field upload.
//!
//! Two textures carry the whole field to the GPU each frame:
//!
//! | Texture | Channels | Format |
//! |---|---|---|
//! | bend | `theta.x`, `theta.y`, `axis.x`, `axis.y` | `Rgba32Float` |
//! | state | compaction | `R32Float` |
//!
//! At the default resolution that is 1.25 MiB per frame, which is nothing on a
//! unified-memory machine and unremarkable over PCIe. The alternative — running
//! the solver in a compute shader so the data never leaves the GPU — would be
//! faster and much harder to test, and the CPU field is already fast enough to
//! be a rounding error in the frame. That trade can be revisited when the field
//! covers a whole battlefield rather than the area around the camera.
//!
//! ## Why the textures are unfilterable and read with `textureLoad`
//!
//! `Rgba32Float` is only filterable where the `float32-filterable` feature
//! happens to be available, so relying on a linear sampler would work on this
//! machine and fail on someone else's. Four `textureLoad`s and a manual lerp
//! cost the same, need no sampler binding at all, and behave identically
//! everywhere.

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::mesh::{Mesh, MeshVertexBufferLayoutRef};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDataOrder, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dKey};

use crate::blade::{ATTRIBUTE_ROOT, ATTRIBUTE_SHAPE, ATTRIBUTE_VARIANT};
use crate::field::GrassField;
use crate::wind::WindField;

/// Where the grass shader lives.
pub const SHADER_PATH: &str = "shaders/grass.wgsl";

/// Per-frame constants shared by every blade.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct GrassSettings {
    /// World position of the field's minimum corner.
    pub field_origin: Vec2,
    /// Reciprocal of the field's width in metres, for turning a world position
    /// into a texture coordinate.
    pub field_inverse_extent: Vec2,
    /// Cells along one edge of the field.
    pub field_resolution: f32,
    /// Seconds, for tip flutter.
    pub time: f32,
    /// Largest bend angle in radians, matching the solver's cap.
    pub max_angle: f32,
    /// Peak tip flutter in radians.
    pub flutter: f32,
    /// Fraction of the blade, from the root up, that barely bends.
    ///
    /// The single most important number for whether this looks like grass. A
    /// blade is anchored by its sheath and its lower third is stiff; let the
    /// whole thing lean uniformly and it reads as a hinged stick.
    pub root_stiffness: f32,
    /// Exponent on the bend profile above the stiff base. Higher concentrates
    /// curvature toward the tip.
    pub bend_exponent: f32,
    /// How much a fully flattened patch shortens on top of its bend.
    pub compaction_shorten: f32,
    /// Extra bend, in radians, applied by full flattening.
    pub matting_angle: f32,
    /// Colour at the base of a blade, in linear space.
    pub base_color: Vec4,
    /// Colour at the tip.
    pub tip_color: Vec4,
    /// Colour crushed grass tends toward.
    pub crushed_color: Vec4,
    /// Direction light arrives from, `xyz` normalised.
    pub light_direction: Vec4,
    /// Ambient fraction.
    pub ambient: f32,
    /// Strength of the wind-driven colour shimmer.
    pub shimmer: f32,
}

impl Default for GrassSettings {
    fn default() -> Self {
        Self {
            field_origin: Vec2::ZERO,
            field_inverse_extent: Vec2::ONE,
            field_resolution: 1.0,
            time: 0.0,
            max_angle: 84.0_f32.to_radians(),
            flutter: 2.5_f32.to_radians(),
            root_stiffness: 0.20,
            bend_exponent: 1.1,
            compaction_shorten: 0.12,
            matting_angle: 62.0_f32.to_radians(),
            // Darker and cooler at the base where light does not reach, warmer
            // and brighter at the tip. Grass that is one flat green reads as
            // plastic; almost all of the sense of depth in a meadow is this
            // vertical gradient plus self-shadowing.
            base_color: Vec4::new(0.030, 0.082, 0.038, 1.0),
            tip_color: Vec4::new(0.404, 0.592, 0.184, 1.0),
            crushed_color: Vec4::new(0.286, 0.300, 0.132, 1.0),
            // Low and off to one side rather than overhead. Overhead light
            // gives every upright blade the same tangent-to-light angle and so
            // the same tone, and a field of identically lit blades reads as a
            // flat texture no matter how much geometry is in it. A raking light
            // separates blades leaning toward it from blades leaning away.
            light_direction: Vec4::new(-0.70, -0.44, 0.56, 0.0),
            ambient: 0.28,
            shimmer: 0.28,
        }
    }
}

/// The material every grass chunk draws with.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct GrassMaterial {
    #[uniform(0)]
    pub settings: GrassSettings,
    #[texture(1, sample_type = "float", filterable = false)]
    pub bend: Handle<Image>,
    #[texture(2, sample_type = "float", filterable = false)]
    pub state: Handle<Image>,
}

impl Material2d for GrassMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    /// Opaque, which is the whole point of drawing real geometry.
    ///
    /// Opaque means the depth buffer sorts blades against each other per
    /// fragment, and later against units, with no sorting work anywhere. Edges
    /// come out clean from multisampling rather than from an alpha test.
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
            ATTRIBUTE_ROOT.at_shader_location(1),
            ATTRIBUTE_SHAPE.at_shader_location(2),
            ATTRIBUTE_VARIANT.at_shader_location(3),
        ])?];
        // A blade bends far enough to turn its back on the camera. Culling
        // would make it wink out halfway through leaning over.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Handles for the two field textures.
///
/// Held separately from the material so the upload does not have to find and
/// mutate every material that happens to reference them.
#[derive(Resource, Debug, Clone)]
pub struct GrassTextures {
    pub bend: Handle<Image>,
    pub state: Handle<Image>,
    resolution: u32,
}

impl FromWorld for GrassTextures {
    fn from_world(world: &mut World) -> Self {
        let resolution = world
            .get_resource::<GrassField>()
            .map_or(crate::field::DEFAULT_RESOLUTION, |f| f.resolution())
            as u32;
        let mut images = world.resource_mut::<Assets<Image>>();
        Self {
            bend: images.add(field_image(resolution, TextureFormat::Rgba32Float)),
            state: images.add(field_image(resolution, TextureFormat::R32Float)),
            resolution,
        }
    }
}

fn field_image(resolution: u32, format: TextureFormat) -> Image {
    let channels = match format {
        TextureFormat::Rgba32Float => 4,
        _ => 1,
    };
    let bytes = (resolution as usize) * (resolution as usize) * channels * 4;
    Image {
        data: Some(vec![0u8; bytes]),
        data_order: TextureDataOrder::default(),
        texture_descriptor: TextureDescriptor {
            label: None,
            size: Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        },
        sampler: ImageSampler::nearest(),
        texture_view_descriptor: None,
        // Kept in the main world too, because the whole point is that the CPU
        // rewrites it every frame.
        asset_usage: RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        copy_on_resize: false,
    }
}

/// Copy the field into its textures and refresh the shared uniforms.
pub fn upload_field(
    field: Res<GrassField>,
    wind: Res<WindField>,
    textures: Res<GrassTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GrassMaterial>>,
) {
    if textures.resolution as usize != field.resolution() {
        // The field was rebuilt at a different size. Skipping is correct: the
        // next frame's textures will have been recreated to match, and writing
        // the wrong number of texels would corrupt the upload.
        return;
    }

    let theta = field.theta();
    let axis = field.axis();
    if let Some(mut image) = images.get_mut(&textures.bend)
        && let Some(data) = image.data.as_mut()
    {
        let texels: &mut [f32] = bytemuck::cast_slice_mut(data);
        for (index, texel) in texels.chunks_exact_mut(4).enumerate() {
            texel[0] = theta[index].x;
            texel[1] = theta[index].y;
            texel[2] = axis[index].x;
            texel[3] = axis[index].y;
        }
    }

    if let Some(mut image) = images.get_mut(&textures.state)
        && let Some(data) = image.data.as_mut()
    {
        bytemuck::cast_slice_mut::<u8, f32>(data).copy_from_slice(field.compaction());
    }

    let extent = field.extent().max(1e-3);
    for (_, material) in materials.iter_mut() {
        material.settings.field_origin = field.origin();
        material.settings.field_inverse_extent = Vec2::splat(1.0 / extent);
        material.settings.field_resolution = field.resolution() as f32;
        material.settings.time = wind.time;
        material.settings.max_angle = field.params().max_angle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bend_texture_is_sized_for_the_field() {
        let image = field_image(64, TextureFormat::Rgba32Float);
        assert_eq!(image.data.as_ref().unwrap().len(), 64 * 64 * 4 * 4);
    }

    #[test]
    fn the_state_texture_is_single_channel() {
        let image = field_image(64, TextureFormat::R32Float);
        assert_eq!(image.data.as_ref().unwrap().len(), 64 * 64 * 4);
    }

    #[test]
    fn field_textures_stay_in_the_main_world() {
        // If they were render-world only, mutating them from the field would
        // silently stop reaching the GPU and the grass would freeze.
        let image = field_image(8, TextureFormat::R32Float);
        assert!(image.asset_usage.contains(RenderAssetUsages::MAIN_WORLD));
        assert!(image.asset_usage.contains(RenderAssetUsages::RENDER_WORLD));
    }

    #[test]
    fn field_textures_can_be_copied_into() {
        let image = field_image(8, TextureFormat::R32Float);
        assert!(
            image
                .texture_descriptor
                .usage
                .contains(TextureUsages::COPY_DST)
        );
        assert!(
            image
                .texture_descriptor
                .usage
                .contains(TextureUsages::TEXTURE_BINDING)
        );
    }

    #[test]
    fn the_default_settings_keep_the_root_stiff() {
        // The number that decides whether grass looks rooted or hinged.
        let settings = GrassSettings::default();
        assert!((0.1..0.35).contains(&settings.root_stiffness));
    }

    #[test]
    fn tips_are_lighter_than_bases() {
        // The vertical gradient is most of the perceived depth in a meadow.
        let settings = GrassSettings::default();
        let luma = |c: Vec4| 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
        assert!(luma(settings.tip_color) > luma(settings.base_color) * 2.0);
    }
}

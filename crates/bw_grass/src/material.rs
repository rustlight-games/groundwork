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
use crate::palette;
use crate::pixel::PixelCanvas;
use crate::wind::WindField;

/// Where the grass shader lives.
pub const SHADER_PATH: &str = "shaders/grass.wgsl";

/// Per-frame constants shared by every blade.
///
/// Field order is load-bearing: WGSL and `encase` lay a uniform out by the same
/// rules, so the two structs match only while their fields stay in step. The
/// sixteen scalars between the vectors and the palette are a whole number of
/// sixteen-byte rows on purpose — leave a gap and the palette shifts by a row
/// on one side and not the other.
#[derive(Clone, Copy, Debug, ShaderType)]
pub struct GrassSettings {
    /// World position of the field's minimum corner.
    pub field_origin: Vec2,
    /// Reciprocal of the field's width in metres, for turning a world position
    /// into a texture coordinate.
    pub field_inverse_extent: Vec2,
    /// Cells along one edge of the field.
    pub field_resolution: f32,
    /// Seconds, for the wind sparkle.
    pub time: f32,
    /// Largest bend angle in radians, matching the solver's cap.
    pub max_angle: f32,
    /// Canvas pixels per world unit.
    ///
    /// What lets the vertex shader snap a blade to the pixel grid and give it a
    /// whole number of pixels of width. Refreshed every frame from
    /// [`crate::pixel::PixelCanvas`], because it changes with both the window
    /// size and the zoom.
    pub pixels_per_unit: f32,
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
    /// Strength of the ordered dither between palette steps, in steps.
    pub dither: f32,
    /// Amplitude of the per-blade tone shimmer, in shade.
    ///
    /// The pixel-art stand-in for tip flutter: at one pixel wide a trembling
    /// tip is invisible, but a blade stepping one shade brighter and back is
    /// not.
    pub sparkle: f32,
    /// Extra shade given to grass that is leaning hard.
    ///
    /// Zero, and it should stay there. This added brightness in proportion to
    /// bend, which meant a shockwave lit a uniform ring as it travelled — every
    /// blade in the front brightening by the same amount at the same moment,
    /// which reads as a floodlight sweeping past rather than as grass moving.
    ///
    /// The rig already produces the right answer for free. A blade's shade
    /// comes from its tangent against the key, so bending genuinely does change
    /// how much light it catches — but per blade, according to which way that
    /// blade happens to be leaning. That is dappling, and adding a uniform term
    /// on top only drowns it out.
    pub gust_lift: f32,
    /// Multiplier on the rig's exposure before it picks a palette step.
    pub shade_gain: f32,
    /// Constant added to the rig's exposure.
    ///
    /// Small. It exists to keep blades off the very bottom of the ramp, and
    /// every unit of it is range given away: with the short grass now the
    /// densest thing on screen, the blades' own shade band *is* the frame's
    /// value distribution, and a high floor compresses it directly. A floor of
    /// 0.16 measured a standard deviation of 0.045 against the art target's
    /// 0.105.
    pub shade_floor: f32,
    /// Exponent on the rig's exposure. Below one spreads the dark end of the
    /// ramp out, which is where most of a canopy lives.
    pub shade_contrast: f32,
    /// Key share below which a blade sits on the shadow ramp.
    pub shadow_cut: f32,
    /// Key share above which a blade sits on the highlight ramp.
    pub highlight_cut: f32,
    /// Metres per cycle of the large-scale light-and-shade variation.
    ///
    /// The same noise, at the same scale, drives the ground — see
    /// [`crate::ground::GroundSettings::patch_metres`]. Sharing it is the whole
    /// point: a field with variation only in its ground reads as grass sitting
    /// on a patterned carpet, and one with variation only in its blades reads
    /// as noise. When both move together the field gets what it was missing,
    /// which is somewhere for the eye to rest and something for it to travel
    /// toward.
    pub macro_metres: f32,
    /// How far that variation moves a blade's shade, in ramp fractions.
    pub macro_strength: f32,
    /// How dark a blade is at its own root, as a fraction of its tip.
    ///
    /// Every blade darkens toward its base regardless of how tall it is. That
    /// is separate from canopy occlusion, which works in absolute metres and
    /// answers a different question — how deep in the canopy a *point* sits.
    /// Both are needed: without the canopy term the two layers light
    /// identically, and without this one a short blade spans so little height
    /// that the canopy term barely moves across it and the blade comes out one
    /// flat colour from root to tip. With the short grass now the densest layer
    /// in the field, that was most of what was on screen.
    pub root_shade: f32,
    /// Padding to close the row. Named rather than anonymous so the layout is
    /// legible from the struct alone.
    pub _pad1: f32,
    /// The palette, flattened as `ramp * RAMP_STEPS + step`, in linear space.
    pub palette: [Vec4; palette::PALETTE_SIZE],
}

impl Default for GrassSettings {
    fn default() -> Self {
        Self {
            field_origin: Vec2::ZERO,
            field_inverse_extent: Vec2::ONE,
            field_resolution: 1.0,
            time: 0.0,
            max_angle: 84.0_f32.to_radians(),
            pixels_per_unit: 15.0,
            root_stiffness: 0.20,
            bend_exponent: 1.1,
            compaction_shorten: 0.12,
            matting_angle: 62.0_f32.to_radians(),
            // A little under one step. Enough to break a flat band into a
            // stipple, not enough to read as noise in its own right.
            // Well under one step. Enough to soften a hard band, not enough to
            // read as grain — the blades are the detailed half of the image and
            // have no room to spare for noise.
            dither: 0.40,
            sparkle: 0.05,
            gust_lift: 0.0,
            // Blades are lifted clear of the ground's range on purpose. The
            // ground sits between 0.11 and 0.32; a blade starts at 0.34 and
            // runs to the top of the ramp. That gap is what makes a stroke
            // legible against what it is standing in, and closing it is what
            // turns a field of grass into a field of noise.
            shade_gain: 1.10,
            shade_floor: 0.10,
            // Below one. A canopy is mostly in its own shade, so most blades
            // land in the bottom third of the rig's range; spreading that third
            // across the ramp is what stops the field being two colours.
            shade_contrast: 0.85,
            // The quartiles of the key share a canopy actually produces, which
            // is the only thing these can sensibly be set against — measured by
            // `light::tests::show_the_key_share`. A share of 0.5 sounds like a
            // reasonable midpoint and is not: the rig's floor is already above
            // it, so cuts picked by eye put the entire field on one ramp and
            // reduce a 24-colour palette to six.
            shadow_cut: 0.690,
            highlight_cut: 0.815,
            macro_metres: 11.0,
            macro_strength: 0.15,
            root_shade: 0.30,
            _pad1: 0.0,
            palette: palette::flattened(),
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
    canvas: Option<Res<PixelCanvas>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GrassMaterial>>,
    mut grounds: ResMut<Assets<crate::ground::GroundMaterial>>,
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
        if let Some(canvas) = canvas.as_deref() {
            material.settings.pixels_per_unit = canvas.pixels_per_unit;
        }
    }

    // The ground needs the same mapping from world position into the field, so
    // that a trail trodden into the grass also shows on the earth beneath it.
    for (_, ground) in grounds.iter_mut() {
        ground.settings.field_origin = field.origin();
        ground.settings.field_inverse_extent = Vec2::splat(1.0 / extent);
        ground.settings.field_resolution = field.resolution() as f32;
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
    fn the_settings_carry_the_whole_palette() {
        let settings = GrassSettings::default();
        assert_eq!(settings.palette.len(), palette::PALETTE_SIZE);
        assert_eq!(settings.palette, palette::flattened());
    }

    #[test]
    fn the_ramp_cuts_leave_room_for_the_middle_ramp() {
        // If these crossed, every blade would be either shadow or highlight and
        // the body ramp — most of the field — would never be drawn.
        let settings = GrassSettings::default();
        assert!(settings.shadow_cut < settings.highlight_cut);
        assert!(settings.shadow_cut > 0.0 && settings.highlight_cut < 1.0);
    }

    #[test]
    fn the_uniform_lays_out_as_the_shader_expects() {
        // The failure this catches is silent and total: a mismatched layout
        // reads the palette from the wrong offset and every blade comes out one
        // colour. `encase` and WGSL agree only while the fields do.
        use bevy::render::render_resource::ShaderSize;
        // Two vec2s, then twenty scalars, then the palette.
        let header = 16 + 20 * 4;
        assert_eq!(header % 16, 0, "the palette must start on a 16-byte row");
        assert_eq!(
            GrassSettings::SHADER_SIZE.get() as usize,
            header + palette::PALETTE_SIZE * 16
        );
    }
}

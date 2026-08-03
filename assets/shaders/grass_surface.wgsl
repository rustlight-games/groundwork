// The baked grass surface.
//
// Every expensive decision about this pixel — which blades cover it, how deep
// inside the canopy it sits, which way its mound faces — was made once when the
// page was baked. What is left at runtime is a texture read.
//
// That asymmetry is the whole design. A field of grass drawn as geometry pays
// for its detail every frame; a field drawn as a cached surface pays once per
// page and then costs what a background costs. The animated part of the grass is
// a separate, much smaller pass that draws over this one.

#import bevy_sprite::mesh2d_vertex_output::VertexOutput

struct SurfaceSettings {
    // Multiplied into the cached colour. Grading, not lighting — the lighting
    // is already in the page, and doing it twice is what makes baked art look
    // like it is sitting under a coloured gel.
    tint: vec3<f32>,
    // One is the page exactly as it was baked.
    exposure: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> settings: SurfaceSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var page_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var page_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Bilinear, not nearest. The reference art is painted rather than pixel
    // art: its strokes have soft two- and three-pixel edges, and point sampling
    // throws exactly that away, leaving a field of hard green polygons.
    let baked = textureSample(page_texture, page_sampler, in.uv);
    return vec4<f32>(baked.rgb * settings.tint * settings.exposure, 1.0);
}

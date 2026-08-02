// The ground under the grass.
//
// A pixel artist drawing a field does not draw blades on a void and hope they
// meet. They block in a green ground first and then stroke grass over it, and
// the ground carries most of the large-scale colour — the lighter sweep where
// the land rises, the darker pool in a hollow. This is that ground.
//
// Structurally it is what stops gaps between blades reading as holes. No canopy
// is ever fully opaque, and whatever shows through has to be a colour that
// belongs to the field. Before this existed those gaps were the window's clear
// colour, and a sixth of the frame was punched through to nothing.
//
// It is four vertices. The isometric projection and the depth function are both
// linear in world position, so one quad interpolates world position and depth
// exactly across the whole field — there is nothing a denser mesh would fix.
// All the variation is per-fragment.

#import bevy_sprite::mesh2d_functions as mesh_functions

#ifdef TONEMAP_IN_SHADER
#import bevy_sprite::mesh2d_view_bindings::view
#import bevy_core_pipeline::tonemapping
#endif
#ifdef SRGB_OUTPUT
#import bevy_render::color_operations::linear_to_srgb
#endif

// Mirrored from palette.rs.
const RAMPS: i32 = 4;
const RAMP_STEPS: i32 = 16;
const PALETTE_SIZE: i32 = 64;
const RAMP_SHADOW: i32 = 0;
const RAMP_BODY: i32 = 1;
const RAMP_DRY: i32 = 3;

// Tone below which the ground uses the shadow ramp.
//
// The body ramp's darkest entry has a luminance of 0.380 and the art target
// reaches 0.335, so the ground does need the shadow ramp to have any real darks
// at all.
//
// What matters is *what the threshold is tested against*. Against a broad field
// alone it draws cloud-shaped dark blotches, because a broad field has broad
// shapes and nothing in a meadow is blotch-shaped — which is exactly what the
// ground had. It is tested here against the broad field with the fine speckle
// already added, so the darks land as grain scattered through the hollows
// rather than as a continuous patch with an edge.
const SHADOW_BELOW: f32 = 0.13;

// Width of the stipple that softens that boundary, in units of variation.
const RAMP_STIPPLE: f32 = 0.09;

struct GroundSettings {
    field_origin: vec2<f32>,
    field_inverse_extent: vec2<f32>,
    field_resolution: f32,
    // Shade at the bottom and top of the ground's own range, in palette steps
    // normalised to 0..1. Deliberately narrow and low: this is the floor of a
    // canopy, and if it climbs into the range the blades use it stops reading
    // as something they are standing in.
    shade_low: f32,
    shade_high: f32,
    // Metres per cycle of the two noise scales.
    patch_metres: f32,
    mottle_metres: f32,
    dither: f32,
    stroke_strength: f32,
    palette: array<vec4<f32>, 64>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> settings: GroundSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var field_state: texture_2d<f32>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) world: vec2<f32>,
}

struct GroundOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec2<f32>,
}

// --- noise ------------------------------------------------------------------

// Integer bit-mixing rather than the usual `fract(sin(dot(...)))`.
//
// The sine trick is fine near the origin and degenerates away from it: `sin` of
// a large argument loses precision in f32, and what comes back stops being
// uniform and starts being *structured*. On a world-space texture that shows up
// as faint concentric ripples centred on wherever the coordinates happen to be
// small — a pattern with no source in the world, which is exactly the kind of
// artefact that is impossible to explain and easy to blame on the geometry.
//
// The input is always an integer lattice point, so an integer hash is both
// exact and cheaper.
fn hash21(p: vec2<f32>) -> f32 {
    var n = u32(i32(p.x) * 374761393 + i32(p.y) * 668265263);
    n = (n ^ (n >> 13u)) * 1274126177u;
    n = n ^ (n >> 16u);
    return f32(n) / 4294967295.0;
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var total = 0.0;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        value += amplitude * value_noise(q);
        total += amplitude;
        // Not exactly two, so the octaves never line up into a visible grid.
        q = q * 2.03 + vec2<f32>(17.3, -9.1);
        amplitude *= 0.5;
    }
    return value / total;
}

// Fine Gaussian speckle, roughly zero-mean over about -0.5..0.5.
//
// The base layer is two things and only two: a large-scale Perlin undulation
// for the lie of the land, and this — very small random variation to keep the
// undulation from reading as an airbrushed wash. Everything else in the field
// goes on top as geometry.
//
// It replaces a second broad noise layer that used to supply the ground's
// "detail". Broad noise cannot supply detail by construction: wherever the
// clumps thinned out, what showed through was a smooth blur several metres
// across, and against a canopy of half-metre plants that reads as a bald patch
// rather than as ground.
//
// Gaussian rather than uniform because uniform speckle has hard shoulders — an
// equal number of pixels at every offset, including the extremes — and quantised
// onto three palette steps that lands as salt and pepper. A bell puts most
// pixels near no change at all and only a few at the edges, which is what grain
// looks like.
//
// Built by the central limit theorem rather than Box–Muller: four white-noise
// lattices summed is close enough to a bell for noise meant to be felt rather
// than seen, and costs four integer hashes instead of a log and a cosine. The
// lattices are rotated by an irrational angle against each other and against the
// world axes, so no two ever line up — a set of axis-aligned lattices at the
// same scale would reappear as a grid, and in this projection a grid becomes the
// isometric crosshatch this layer has been rid of twice already.
fn gaussian(p: vec2<f32>) -> f32 {
    var total = 0.0;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        total += hash21(floor(q));
        q = vec2<f32>(q.x * 0.7986 - q.y * 0.6018, q.x * 0.6018 + q.y * 0.7986) * 1.31
            + vec2<f32>(43.7, -19.3);
    }
    return total * 0.25 - 0.5;
}

// The 4x4 ordered dither matrix in closed form, keyed to the canvas pixel so
// the pattern is fixed to the screen rather than swimming with the geometry.
fn bayer4(x: u32, y: u32) -> f32 {
    let a = x ^ y;
    let value = ((a & 1u) << 3u) | ((y & 1u) << 2u) | (((a >> 1u) & 1u) << 1u) | ((y >> 1u) & 1u);
    return f32(value);
}

fn state_texel(coord: vec2<i32>) -> f32 {
    let last = i32(settings.field_resolution) - 1;
    return textureLoad(field_state, clamp(coord, vec2<i32>(0), vec2<i32>(last)), 0).x;
}

fn sample_state(uv: vec2<f32>) -> f32 {
    let texel = uv * settings.field_resolution - 0.5;
    let base = floor(texel);
    let f = texel - base;
    let i = vec2<i32>(base);
    let a = state_texel(i);
    let b = state_texel(i + vec2<i32>(1, 0));
    let c = state_texel(i + vec2<i32>(0, 1));
    let d = state_texel(i + vec2<i32>(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

@vertex
fn vertex(vertex: Vertex) -> GroundOutput {
    var out: GroundOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh2d_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = mesh_functions::mesh2d_position_world_to_clip(world_position);
    out.world = vertex.world;
    return out;
}

@fragment
fn fragment(in: GroundOutput) -> @location(0) vec4<f32> {
    // The lie of the land: Perlin, at a scale far larger than any plant. This is
    // the ground's whole shape — the lighter sweep where it rises, the darker
    // pool in a hollow — and it is the only thing here with structure.
    let undulation = fbm(in.world / max(settings.patch_metres, 0.01));

    // Then very small random variation on top, and nothing else. It carries no
    // shape of its own; its only job is to stop the undulation reading as an
    // airbrushed gradient wherever the canopy thins enough to show it.
    let speckle = gaussian(in.world / max(settings.mottle_metres, 0.001));
    let tone = clamp(undulation + speckle * settings.stroke_strength, 0.0, 1.0);

    let shade = mix(settings.shade_low, settings.shade_high, tone);

    let pixel = vec2<u32>(in.position.xy);
    let dither = (bayer4(pixel.x, pixel.y) + 0.5) / 16.0 - 0.5;

    var ramp = RAMP_BODY;
    // The darkest hollows drop onto the shadow ramp, which gives the ground a
    // cool blue-green down there instead of simply less of the same green.
    //
    // Tested against `tone`, which already has the speckle in it — see
    // `SHADOW_BELOW`. The threshold is dithered on top of that, and that is not
    // a nicety either. A hard comparison against a smooth field draws its own
    // contour line: every pixel one side of it jumps a whole ramp, so a gentle
    // dip appears as a dark blotch with a crisp edge belonging to no feature in
    // the world.
    if (tone + dither * RAMP_STIPPLE < SHADOW_BELOW) {
        ramp = RAMP_SHADOW;
    }
    let level = clamp(
        shade * f32(RAMP_STEPS) - 0.5 + dither * settings.dither,
        0.0,
        f32(RAMP_STEPS - 1),
    );
    let index = clamp(ramp, 0, RAMPS - 1) * RAMP_STEPS + i32(round(level));
    var colour = settings.palette[clamp(index, 0, PALETTE_SIZE - 1)];

#ifdef TONEMAP_IN_SHADER
    colour = tonemapping::tone_mapping(colour, view.color_grading);
#endif
#ifdef SRGB_OUTPUT
    colour = vec4<f32>(linear_to_srgb(colour.rgb), colour.a);
#endif
    return colour;
}

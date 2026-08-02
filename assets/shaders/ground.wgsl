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

// Metres per cycle of the base grain.
const GRAIN_METRES: f32 = 0.42;

// Metres per cycle of the warp that breaks the grain off its own lattice.
const WARP_METRES: f32 = 2.6;

// How far that warp displaces a sample, in metres.
const WARP_STRENGTH: f32 = 0.55;

// Metres per cell of the fine speckle.
const SPECKLE_METRES: f32 = 0.055;

// Stroke depth below which the ground uses the shadow ramp.
//
// Keyed to the *strokes*, not to the broad variation field. That distinction is
// the whole point. The body ramp's darkest entry has a luminance of 0.236 and
// the art target's second percentile is 0.158, so the ground does need to reach
// the shadow ramp to have any real darks at all — but reaching it via a broad
// noise threshold put cloud-shaped dark blotches across the field, because a
// broad field has broad shapes and nothing in a meadow is blotch-shaped.
//
// Switching on stroke depth instead puts the dark exactly where the gaps
// between blades are: fine, grass-shaped, and the same size as the marks it
// sits between.
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

// Fine grass strokes.
//
// Noise sampled in a frame stretched along the local grain, so a round blob of
// noise comes out as an elongated mark. Two scales: the strokes themselves, and
// a coarser one that breaks them into clumps so they do not read as corduroy.
// Fine grain on the base.
//
// Perlin plus a little white noise — deliberately *not* directional strokes.
//
// An earlier revision drew grass marks here: three families of elongated
// strokes at fixed angles, combined so each family contributed almost
// everywhere. Three fixed directions laid over one another is a lattice, and at
// magnification that is exactly what it looked like — a crosshatch with
// straight runs tens of pixels long. Shortening the strokes or dropping a
// family only ever changes the weave.
//
// The base layer's job is tone. Marks are geometry: the short grass and the
// long blades, which have direction because each blade genuinely has one, and
// which therefore cannot weave however many of them there are.
fn grain(world: vec2<f32>) -> f32 {
    // Warped before it is sampled. Value noise on an unwarped grid is
    // *predictable*: its features sit on a lattice at the octave's own scale
    // and the eye finds that spacing quickly even when the values are random.
    // Displacing the sample point by a coarser noise breaks the grid without
    // adding any frequency of its own.
    let warp = vec2<f32>(
        value_noise(world / WARP_METRES) - 0.5,
        value_noise(world / WARP_METRES + vec2<f32>(23.1, -14.6)) - 0.5,
    ) * WARP_STRENGTH;
    let p = world + warp;

    // Four octaves at incommensurate ratios, so no two ever line up into a
    // beat, plus an octave of unfiltered hash for the fine speckle that keeps
    // it from looking airbrushed.
    let a = value_noise(p / GRAIN_METRES);
    let b = value_noise(p / (GRAIN_METRES * 0.437) + vec2<f32>(7.7, -3.1));
    let c = value_noise(p / (GRAIN_METRES * 2.13) + vec2<f32>(-11.9, 5.4));
    let speckle = hash21(floor(p / SPECKLE_METRES));
    return clamp(a * 0.34 + b * 0.26 + c * 0.26 + speckle * 0.14, 0.0, 1.0);
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
    // Two scales of variation. The broad one is the sweep of the land — the
    // thing that stops a field being one flat colour when you look at it as a
    // whole. The fine one breaks up the broad one so it does not read as a
    // gradient someone airbrushed on.
    let broad = fbm(in.world / max(settings.patch_metres, 0.01));
    let mottle = fbm(in.world / max(settings.mottle_metres, 0.01));
    // Weighted hard toward the broad scale. The fine one is there to keep the
    // broad one from reading as an airbrushed gradient, not to compete with it —
    // give them equal say and the large-scale structure disappears into grain.
    let variation = clamp(broad * 0.85 + mottle * 0.15, 0.0, 1.0);

    var shade = mix(settings.shade_low, settings.shade_high, variation);

    // The fine grass. This is most of the frame's local contrast — without it
    // the base is a smooth wash and the whole field measures flat however many
    // blades stand on it.
    let stroke = grain(in.world);
    shade += (stroke - 0.45) * settings.stroke_strength;
    shade = clamp(shade, 0.0, 1.0);

    let pixel = vec2<u32>(in.position.xy);
    let dither = (bayer4(pixel.x, pixel.y) + 0.5) / 16.0 - 0.5;

    var ramp = RAMP_BODY;
    // The darkest hollows drop onto the shadow ramp, which gives the ground a
    // cool blue-green down there instead of simply less of the same green.
    //
    // The threshold is dithered, and that is not a nicety. A hard comparison
    // against a smooth noise field draws its own contour line: every pixel one
    // side of it jumps a whole ramp, so a gentle dip in the ground appears as a
    // dark blotch with a crisp edge that belongs to no feature in the world.
    // Stippling the boundary spreads the switch over a few pixels and it reads
    // as a gradient again.
    if (stroke + dither * RAMP_STIPPLE < SHADOW_BELOW) {
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

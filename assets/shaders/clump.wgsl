// Grass clumps.
//
// Each clump is one quad sampling a pre-baked sprite from the atlas in
// crates/bw_grass/src/clump.rs. All the detail — overlapping leaves, soft
// edges, a shaded interior, bright tips — was drawn once when the atlas was
// baked, so nothing here pays for it per frame.
//
// What this shader does is place the quad, bend it, and tint it.
//
// ## The bend is a shear, not a rotation
//
// A clump's root is planted. The bottom edge of the quad never moves and the
// top edge slides in the direction the bend field says, by an amount that grows
// with height up the sprite. That is the same rooted deformation the blades
// used, applied to a whole plant at once, and it is why a gust reads as grass
// leaning rather than as sprites sliding across the ground.
//
// Shearing a sprite is normally how grass ends up looking like rubber — the
// silhouette never shortens as it leans. Here it does not, because the shear is
// paired with a vertical squash proportional to how far it has leaned, which is
// what actually happens to the height of something bending over.

#import bevy_sprite::mesh2d_functions as mesh_functions

#ifdef TONEMAP_IN_SHADER
#import bevy_sprite::mesh2d_view_bindings::view
#import bevy_core_pipeline::tonemapping
#endif
#ifdef SRGB_OUTPUT
#import bevy_render::color_operations::linear_to_srgb
#endif

// --- the isometric projection, mirrored from iso.rs -------------------------
const HALF_TILE_W: f32 = 1.0;
const HALF_TILE_H: f32 = 0.5;
const Z_SCALE: f32 = 1.0;
const DEPTH_PER_GROUND: f32 = 0.5;
const DEPTH_PER_HEIGHT: f32 = 0.5;

// --- atlas layout, mirrored from clump.rs -----------------------------------
const COLUMNS: f32 = 6.0;
const ROWS: f32 = 8.0;

const TAU: f32 = 6.2831855;

// How much of the field's bend a clump takes, limpest to stiffest.
//
// A very wide spread. Neighbours that answer the wind alike move as one
// surface, and an undulating surface is water however green it is.
const STIFFNESS_MIN: f32 = 0.30;
const STIFFNESS_MAX: f32 = 1.70;

// Bend below which a clump does not move at all, as a fraction of the cap.
//
// Grass has stiff stems and a canopy that catches on itself; it ignores a light
// breeze entirely. Without this every plant answers every ripple in the field
// and the whole thing flows.
const STICTION: f32 = 0.17;

// Coverage below which a fragment is thrown away. Mirrored from clump.rs.
//
// These sprites are clipped rather than blended, so this is the silhouette:
// everything above it writes depth and sorts per fragment, everything below it
// never existed. Around a half, because a low threshold keeps the soft rim that
// made sorting a problem in the first place.
const ALPHA_CUT: f32 = 0.45;

struct ClumpSettings {
    field_origin: vec2<f32>,
    field_inverse_extent: vec2<f32>,
    field_resolution: f32,
    time: f32,
    max_angle: f32,
    // How far the top of a clump slides, in sprite heights, at full bend.
    lean: f32,
    // How much a fully leaned clump loses in height.
    squash: f32,
    // Amplitude of the per-clump idle sway, in sprite heights.
    sway: f32,
    // How far tint shifts a clump's colour, in 0..1.
    tint_strength: f32,
    // Shade multiplier at the darkest tint.
    tint_floor: f32,
    root_stiffness: f32,
    _pad1: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> settings: ClumpSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var atlas: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var atlas_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var field_bend: texture_2d<f32>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) root: vec2<f32>,
    // corner x, corner y, atlas column, atlas row
    @location(2) corner: vec4<f32>,
    // width, height, tint, per-clump random
    @location(3) shape: vec4<f32>,
}

struct ClumpOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) shade: f32,
}

fn project(world: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(
        (world.x - world.y) * HALF_TILE_W,
        -(world.x + world.y) * HALF_TILE_H + world.z * Z_SCALE,
    );
}

fn iso_depth(world: vec3<f32>) -> f32 {
    return (world.x + world.y) * DEPTH_PER_GROUND + world.z * DEPTH_PER_HEIGHT;
}

fn bend_texel(coord: vec2<i32>) -> vec4<f32> {
    let last = i32(settings.field_resolution) - 1;
    return textureLoad(field_bend, clamp(coord, vec2<i32>(0), vec2<i32>(last)), 0);
}

fn sample_bend(uv: vec2<f32>) -> vec4<f32> {
    let texel = uv * settings.field_resolution - 0.5;
    let base = floor(texel);
    let f = texel - base;
    let i = vec2<i32>(base);
    let a = bend_texel(i);
    let b = bend_texel(i + vec2<i32>(1, 0));
    let c = bend_texel(i + vec2<i32>(0, 1));
    let d = bend_texel(i + vec2<i32>(1, 1));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 78.233) * 43758.5453);
}

@vertex
fn vertex(vertex: Vertex) -> ClumpOutput {
    let across = vertex.corner.x;
    let up = vertex.corner.y;
    let width = vertex.shape.x;
    let height = vertex.shape.y;
    let tint = vertex.shape.z;
    let random = vertex.shape.w;

    let uv = (vertex.root - settings.field_origin) * settings.field_inverse_extent;
    let field = sample_bend(uv);

    // How hard, and which way, in world ground coordinates.
    let bend = field.xy;
    let strength = clamp(length(bend) / max(settings.max_angle, 1e-4), 0.0, 1.0);

    // Every clump takes a very different share of the wind. The spread is wide
    // on purpose: neighbours that respond alike move as a *surface*, and a
    // surface that undulates is water. Grass is a field of separate stiff
    // plants, and the difference between the two is almost entirely how much
    // their neighbours disagree with them.
    let stiffness = STIFFNESS_MIN + (STIFFNESS_MAX - STIFFNESS_MIN) * hash11(random + 4.1);

    // Grass does not respond to a light breeze at all. Below this the plant
    // simply does not move — stems are stiff and there is friction in the
    // canopy, and without a threshold every clump answers every ripple in the
    // field, which is exactly what a liquid does.
    let responsive = smoothstep(STICTION, 1.0, strength);

    var direction = vec2<f32>(0.0, 0.0);
    if (strength > 1e-4) {
        direction = normalize(bend);
    }
    // The lean, in world metres, applied only to the top of the sprite.
    //
    // There is no idle sway term. There used to be — a sine per clump — and it
    // was the single thing that made a still field read as a water surface:
    // continuous, smooth, everywhere at once. Grass at rest is *still*. All the
    // motion should come from the wind field, which already gusts.
    let lean = direction * (responsive * stiffness * settings.lean * height);

    // How much of the lean this height takes.
    //
    // Emphatically *not* linear in `up`. A linear shear puts half the lean at
    // half the height, which means the whole sprite slides sideways together
    // and the plant reads as sliding across the ground rather than bending out
    // of it — even though the root vertex itself never moves. A grass plant is
    // stiff near the ground and limp at the tip, so almost none of the motion
    // belongs in the bottom third.
    //
    // `root_stiffness` is the exponent: at one this is the old linear shear,
    // and around two and a half the base is visibly planted.
    let weight = pow(up, settings.root_stiffness);

    let squash = 1.0 - settings.squash * responsive * stiffness;
    var world = vec3<f32>(
        vertex.root + lean * weight,
        up * height * squash,
    );

    // Widen across the screen's horizontal, so a sprite always faces the
    // camera squarely however the ground beneath it runs.
    let screen = project(world) + vec2<f32>(across * width * 0.5, 0.0);
    let local = vec4<f32>(screen, iso_depth(world), 1.0);

    var out: ClumpOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh2d_position_local_to_world(world_from_local, local);
    out.position = mesh_functions::mesh2d_position_world_to_clip(world_position);

    // Into the atlas cell. `up` runs bottom-to-top in the world and the sprite
    // is stored top-down, so v is flipped.
    let cell = vec2<f32>(1.0 / COLUMNS, 1.0 / ROWS);
    let corner = vec2<f32>((across * 0.5) + 0.5, 1.0 - up);
    out.uv = (vec2<f32>(vertex.corner.z, vertex.corner.w) + corner) * cell;

    // Tint darkens rather than brightens: the atlas already holds the lit
    // colour, and a clump that varies upward would blow past the palette.
    out.shade = mix(settings.tint_floor, 1.0, mix(1.0, tint, settings.tint_strength));
    return out;
}

@fragment
fn fragment(in: ClumpOutput) -> @location(0) vec4<f32> {
    var sampled = textureSample(atlas, atlas_sampler, in.uv);
    // Cut the long tail of near-transparent coverage. Soft edges are the point,
    // but a sprite that fades to nothing over many pixels stacks up across
    // dozens of overlapping clumps into a haze.
    if (sampled.a < ALPHA_CUT) {
        discard;
    }

    var colour = vec4<f32>(sampled.rgb * in.shade, sampled.a);
#ifdef TONEMAP_IN_SHADER
    colour = tonemapping::tone_mapping(colour, view.color_grading);
#endif
#ifdef SRGB_OUTPUT
    colour = vec4<f32>(linear_to_srgb(colour.rgb), colour.a);
#endif
    return colour;
}

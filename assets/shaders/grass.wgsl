// Grass blades.
//
// Every blade is rebuilt here from six numbers: where its root is, how long it
// is, how wide, and three per-blade random values. Its posture comes from the
// bend field, sampled once at the root. Nothing about a blade's pose is stored
// in the mesh, which is why the mesh never has to be rebuilt when the grass
// moves.
//
// The order of operations matters and is the one crates/bw_grass/src/iso.rs
// describes: build the blade as a curve through world (X, Y, Z), *then* project.
// Doing it the other way — bending a flat sprite in screen space — is what makes
// grass look like rubber, because the silhouette never shortens as the blade
// leans and the tip travels in a straight line instead of an arc.
//
// The constants below are duplicated from Rust. `shader_constants_match_this_module`
// in iso.rs and `shader_ranges_match_this_module` in blade.rs read this file and
// fail if they ever drift.

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

// --- blade ranges, mirrored from blade.rs -----------------------------------
const LENGTH_MIN: f32 = 0.12;
const LENGTH_MAX: f32 = 0.46;
const WIDTH_MIN: f32 = 0.016;
const WIDTH_MAX: f32 = 0.032;

const TAU: f32 = 6.2831855;

// Direction from the scene toward the camera, for the sheen term. A true
// isometric camera sits equally along all three axes.
const VIEW_DIRECTION: vec3<f32> = vec3<f32>(0.5773503, 0.5773503, 0.5773503);

// Steps used to integrate the blade centreline. Eight is smooth at an eighty
// degree bend; four visibly kinks.
const ARC_STEPS: i32 = 8;

// Largest lean a blade has just from having grown that way, in radians.
const REST_LEAN: f32 = 0.58;

struct GrassSettings {
    field_origin: vec2<f32>,
    field_inverse_extent: vec2<f32>,
    field_resolution: f32,
    time: f32,
    max_angle: f32,
    flutter: f32,
    root_stiffness: f32,
    bend_exponent: f32,
    compaction_shorten: f32,
    matting_angle: f32,
    base_color: vec4<f32>,
    tip_color: vec4<f32>,
    crushed_color: vec4<f32>,
    light_direction: vec4<f32>,
    ambient: f32,
    shimmer: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> settings: GrassSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var field_bend: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var field_state: texture_2d<f32>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // Rest pose, already projected. Overwritten below; it exists so Bevy can
    // compute a bounding box in the space this shader actually outputs.
    @location(0) position: vec3<f32>,
    @location(1) root: vec2<f32>,
    // height along blade, which side of the ribbon, length, base half-width
    @location(2) shape: vec4<f32>,
    // flutter phase, flutter rate, tint, per-blade random
    @location(3) variant: vec4<f32>,
}

struct GrassOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

// --- projection -------------------------------------------------------------

fn project(world: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(
        (world.x - world.y) * HALF_TILE_W,
        -(world.x + world.y) * HALF_TILE_H + world.z * Z_SCALE,
    );
}

fn iso_depth(world: vec3<f32>) -> f32 {
    return (world.x + world.y) * DEPTH_PER_GROUND + world.z * DEPTH_PER_HEIGHT;
}

// --- field sampling ---------------------------------------------------------
//
// Bilinear by hand. `Rgba32Float` is not filterable everywhere, so a linear
// sampler would work on some machines and fail validation on others.

fn bend_texel(coord: vec2<i32>) -> vec4<f32> {
    let last = i32(settings.field_resolution) - 1;
    return textureLoad(field_bend, clamp(coord, vec2<i32>(0), vec2<i32>(last)), 0);
}

fn state_texel(coord: vec2<i32>) -> f32 {
    let last = i32(settings.field_resolution) - 1;
    return textureLoad(field_state, clamp(coord, vec2<i32>(0), vec2<i32>(last)), 0).x;
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

// --- blade shape ------------------------------------------------------------

// How much of the total bend has accumulated by height `u`.
//
// Flat zero below `root_stiffness`: the base of a blade is held by its sheath
// and barely moves. This is what pins the root — without it the whole blade
// leans as one piece and the grass reads as hinged sticks, or worse, as a
// texture sliding over the ground.
fn bend_profile(u: f32) -> f32 {
    let base = settings.root_stiffness;
    if (u <= base) {
        return 0.0;
    }
    let t = (u - base) / max(1.0 - base, 1e-4);
    return pow(smoothstep(0.0, 1.0, t), settings.bend_exponent);
}

// Flutter is confined to the top of the blade, where there is least mass and
// least stiffness. Applied over the whole blade it looks like the grass is
// underwater.
fn flutter_profile(u: f32) -> f32 {
    let t = smoothstep(0.55, 1.0, u);
    return t * t;
}

// Half-width down the blade, as a fraction of the base half-width.
//
// Near parallel-sided for the lower two thirds and then drawn to a point, which
// is the shape of an actual grass leaf. Tapering from low down instead gives
// long thin triangles that read as pine needles or fur.
fn width_profile(u: f32) -> f32 {
    let rise = smoothstep(0.0, 0.12, u);
    let taper = 1.0 - smoothstep(0.62, 1.0, u);
    return (0.80 + 0.20 * rise) * (0.05 + 0.95 * taper);
}

// A cheap extra random from one stored value, so a blade can have more
// independent properties than the four bytes it carries.
fn hash11(x: f32) -> f32 {
    return fract(sin(x * 78.233) * 43758.5453);
}

// The bend-angle vector at height `u`: direction of lean, magnitude in radians.
fn bend_at(u: f32, total: vec2<f32>, flutter: vec2<f32>) -> vec2<f32> {
    return total * bend_profile(u) + flutter * flutter_profile(u);
}

// Unit tangent of the blade at height `u`, in world space.
//
// Built from an angle rather than from a difference of positions, which is what
// makes the arc length exact: the tangent is a unit vector by construction, so
// integrating it cannot stretch or shrink the blade.
fn tangent_at(u: f32, total: vec2<f32>, flutter: vec2<f32>) -> vec3<f32> {
    let bend = bend_at(u, total, flutter);
    let angle = length(bend);
    var direction = vec2<f32>(1.0, 0.0);
    if (angle > 1e-5) {
        direction = bend / angle;
    }
    return vec3<f32>(direction * sin(angle), cos(angle));
}

// --- shading ----------------------------------------------------------------

// Strand shading. A blade is effectively a cylinder with no single normal, so
// lighting is computed from its tangent instead: brightest across the light,
// darkest along it. Because the tangent turns as the blade bends, the grass
// changes tone as it moves — which is most of why a gust reads as a wave of
// light travelling across a field rather than as geometry wiggling.
fn shade(
    height: f32,
    tangent: vec3<f32>,
    compaction: f32,
    lean: f32,
    tint: f32,
) -> vec4<f32> {
    let light = normalize(settings.light_direction.xyz);

    let along_light = dot(tangent, light);
    let along_view = dot(tangent, VIEW_DIRECTION);
    let across_light = sqrt(max(0.0, 1.0 - along_light * along_light));
    let across_view = sqrt(max(0.0, 1.0 - along_view * along_view));

    let diffuse = across_light;
    let sheen = pow(max(0.0, across_light * across_view - along_light * along_view), 20.0);

    // Blades are packed together and their lower halves sit in each other's
    // shadow. Faking that with height is cheap and does more for the sense of a
    // dense canopy than any amount of extra geometry.
    let occlusion = mix(0.20, 1.0, smoothstep(0.0, 0.62, height));

    var colour = mix(
        settings.base_color.rgb,
        settings.tip_color.rgb,
        smoothstep(0.0, 0.9, height),
    );
    // Per-blade variation, in brightness and in hue. Brightness alone leaves
    // the field one flat colour with a bit of noise on it; letting some blades
    // run yellower and others bluer is what makes it read as many plants rather
    // than one material.
    colour *= 0.70 + 0.60 * tint;
    let warmth = tint - 0.5;
    colour = vec3<f32>(
        colour.r * (1.0 + 0.28 * warmth),
        colour.g,
        colour.b * (1.0 - 0.30 * warmth),
    );
    // Crushed grass goes duller and yellower as it is worn down.
    colour = mix(colour, settings.crushed_color.rgb, compaction * 0.75);

    // Grass leaning hard is catching the light differently; brightening it
    // slightly is what makes gust fronts visible as bands of colour.
    let gust = 1.0 + settings.shimmer * smoothstep(0.05, 0.9, lean);

    let lit = colour * (settings.ambient + (1.0 - settings.ambient) * diffuse) * occlusion * gust
        + vec3<f32>(sheen * 0.35 * occlusion);

    return vec4<f32>(lit, 1.0);
}

// --- vertex -----------------------------------------------------------------

@vertex
fn vertex(vertex: Vertex) -> GrassOutput {
    let height = vertex.shape.x;
    let side = vertex.shape.y * 2.0 - 1.0;
    let blade_length = mix(LENGTH_MIN, LENGTH_MAX, vertex.shape.z);
    let blade_width = mix(WIDTH_MIN, WIDTH_MAX, vertex.shape.w);

    let uv = (vertex.root - settings.field_origin) * settings.field_inverse_extent;
    let field = sample_bend(uv);
    let compaction = sample_state(uv);

    // Every blade starts with its own lean. Grass does not grow as a bed of
    // nails — neighbouring blades splay in every direction, and without this
    // the canopy is a uniform vertical brush that only ever moves when the wind
    // moves it. This one term does more for the look than anything else here.
    let rest_angle = hash11(vertex.variant.x + 1.7) * TAU;
    let rest_lean = REST_LEAN * hash11(vertex.variant.x + 5.3);
    var bend = field.xy + vec2<f32>(cos(rest_angle), sin(rest_angle)) * rest_lean;

    // Flattening. The field stores the axis grass was crushed along without a
    // sign, so that a path walked in both directions still reads as flattened
    // rather than cancelling out. Each blade picks a side of that axis, biased
    // toward whichever way the grass is currently leaning, so a one-way trail
    // still points one way while a two-way one lies both.
    let axis_pair = field.zw;
    let alignment = length(axis_pair);
    if (alignment > 1e-4) {
        let axis_angle = 0.5 * atan2(axis_pair.y, axis_pair.x);
        let axis_direction = vec2<f32>(cos(axis_angle), sin(axis_angle));
        let bias = 0.5 + 0.5 * clamp(
            dot(bend, axis_direction) / max(settings.max_angle, 1e-4),
            -1.0,
            1.0,
        );
        let chosen = select(-1.0, 1.0, vertex.variant.w < bias);
        bend += settings.matting_angle * chosen * alignment * axis_direction;
    }

    // Whatever crushing is left over after the aligned part is directionless:
    // grass trampled from every angle. Each blade falls a stable random way.
    let scattered = max(compaction - alignment, 0.0);
    if (scattered > 1e-4) {
        let scatter_angle = vertex.variant.x * TAU;
        bend += settings.matting_angle * 0.55 * scattered
            * vec2<f32>(cos(scatter_angle), sin(scatter_angle));
    }

    // Tip flutter. Two incommensurate rates so blades do not beat in unison.
    let rate = 5.0 + 6.0 * vertex.variant.y;
    let phase = vertex.variant.x * TAU;
    let wobble = sin(settings.time * rate + phase)
        + 0.35 * sin(settings.time * rate * 1.73 + phase * 2.1);
    // Perpendicular to the lean, which is where a real blade's tip trembles.
    var across = vec2<f32>(0.0, 1.0);
    let lean = length(bend);
    if (lean > 1e-4) {
        across = vec2<f32>(-bend.y, bend.x) / lean;
    }
    let flutter = across * (settings.flutter * wobble);

    // Integrate the centreline from root to this vertex's height. Midpoint
    // rule: at eight steps it is smooth well past the angular cap.
    var centre = vec3<f32>(vertex.root, 0.0);
    let step = height / f32(ARC_STEPS);
    for (var i = 0; i < ARC_STEPS; i = i + 1) {
        let mid = (f32(i) + 0.5) * step;
        centre += tangent_at(mid, bend, flutter) * (blade_length * step);
    }

    // Crushed grass loses a little height beyond what bending already takes.
    // Only a little: most of the shortening should come from the blade leaning
    // over, because that is what actually happens.
    centre.z *= 1.0 - settings.compaction_shorten * compaction;

    let tangent = tangent_at(height, bend, flutter);

    // Project the centreline, then widen along the screen-space perpendicular.
    // Widening in screen space rather than in world space keeps a blade the
    // same apparent thickness however it is oriented, which stops blades lying
    // toward the camera from turning into slivers.
    let screen = project(centre);
    let screen_tangent = project(tangent);
    var normal = vec2<f32>(1.0, 0.0);
    let tangent_length = length(screen_tangent);
    if (tangent_length > 1e-5) {
        normal = vec2<f32>(-screen_tangent.y, screen_tangent.x) / tangent_length;
    }
    let offset = normal * (side * blade_width * width_profile(height));

    let local = vec4<f32>(screen + offset, iso_depth(centre), 1.0);

    var out: GrassOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh2d_position_local_to_world(world_from_local, local);
    out.position = mesh_functions::mesh2d_position_world_to_clip(world_position);
    out.color = shade(height, tangent, compaction, lean, vertex.variant.z);
    return out;
}

@fragment
fn fragment(in: GrassOutput) -> @location(0) vec4<f32> {
    var colour = in.color;
#ifdef TONEMAP_IN_SHADER
    colour = tonemapping::tone_mapping(colour, view.color_grading);
#endif
#ifdef SRGB_OUTPUT
    colour = vec4<f32>(linear_to_srgb(colour.rgb), colour.a);
#endif
    return colour;
}

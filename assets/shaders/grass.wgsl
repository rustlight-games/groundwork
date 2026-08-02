// Grass blades, drawn as pixel art.
//
// Every blade is rebuilt here from a handful of numbers: where its root is, how
// long it is, how wide, which way it grew and a per-blade random. Its posture
// comes from the bend field, sampled once at the root. Nothing about a blade's
// pose is stored in the mesh, which is why the mesh never has to be rebuilt
// when the grass moves.
//
// The order of operations matters and is the one crates/bw_grass/src/iso.rs
// describes: build the blade as a curve through world (X, Y, Z), *then* project.
// Doing it the other way — bending a flat sprite in screen space — is what makes
// grass look like rubber, because the silhouette never shortens as the blade
// leans and the tip travels in a straight line instead of an arc.
//
// ## What makes this pixel art rather than a small render
//
// Three quantisations, all of them here rather than in a post-process, because
// a post-process only ever sees the finished image and by then the information
// needed to do any of this is gone:
//
//   1. **Pose.** A blade's lean snaps to a fixed set of angles and magnitudes.
//      Hand-drawn grass has a handful of frames, not a continuum, and this is
//      what gives the motion its stepped, drawn quality. It also happens to
//      kill sub-pixel crawl outright: a blade holds one pose for many frames
//      instead of sliding a fraction of a pixel every frame.
//   2. **Geometry.** The centreline snaps to pixel centres and the ribbon is a
//      whole number of pixels wide, so a stroke can never thin into a sliver
//      or fall between two pixels and vanish.
//   3. **Colour.** See below — this shader never computes a colour at all.
//
// Each blade offsets the pose grid by its own random, so neighbours snap at
// different moments. Without that the whole field steps at once, which reads as
// a strobe rather than as grass.
//
// ## The lighting rig, and why no colour is computed here
//
// Three suns light this game — a golden key over the viewer's left shoulder, a
// dim saturated blue fill opposite it, and a cool rim from behind — and the
// same rig lights the character sprites baked out of Blender. See
// crates/bw_grass/src/light.rs.
//
// What this shader evaluates is not the rig's colour but its *geometry*: how
// much light a blade is catching, and how much of that light is the key rather
// than the fill. Those two numbers pick a step and a ramp out of the palette in
// crates/bw_grass/src/palette.rs, which was itself baked from this same rig.
//
// The indirection is the point. Computing colour here would put continuous
// values on screen and the image would stop being pixel art; baking the rig
// into the palette and looking it up keeps every pixel exactly on palette while
// a blade still genuinely responds to which way it is leaning.
//
// The constants below are duplicated from Rust. `shader_constants_match_this_module`
// in iso.rs, `shader_ranges_match_this_module` in blade.rs,
// `shader_directions_match_this_module` in light.rs and
// `shader_palette_matches_this_module` in palette.rs read this file and fail if
// they ever drift.

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
const LENGTH_MIN: f32 = 0.08;
const LENGTH_MAX: f32 = 0.5;
const WIDTH_MIN: f32 = 0.01;
const WIDTH_MAX: f32 = 0.022;
const REST_LEAN_MIN: f32 = 0.08;
const REST_LEAN_MAX: f32 = 0.7;

// --- the lighting rig, mirrored from light.rs -------------------------------
//
// Directions point from the scene toward each sun. Energies are the character
// rig's, unchanged, so grass and sprites sit in the same light.
const KEY_DIRECTION: vec3<f32> = vec3<f32>(-0.333125, 0.714229, 0.615661);
const FILL_DIRECTION: vec3<f32> = vec3<f32>(0.410103, -0.879385, 0.241922);
const RIM_DIRECTION: vec3<f32> = vec3<f32>(-0.242432, -0.519837, 0.819152);
const VIEW_DIRECTION: vec3<f32> = vec3<f32>(0.577350, 0.577350, 0.577350);

// How much of the rim arrives as plain diffuse rather than as a glint. Small on
// purpose — a diffuse rim at this energy would swamp the albedo and grey out
// the whole palette. See `light::respond`.
const RIM_DIFFUSE: f32 = 0.12;

// Correction from the rig's surface rim energy to a strand's. A strand
// scatters a rim into a whole cone where a surface reflects it into a narrow
// lobe, so the character rig's rim energy over-contributes here by roughly this
// ratio. See `light::RIM_STRAND`.
const RIM_STRAND: f32 = 0.22;

const KEY_ENERGY: f32 = 5.6;
const FILL_ENERGY: f32 = 0.73;
const RIM_ENERGY: f32 = 4.5;
const SKY_ENERGY: f32 = 1.85;
const FULL_EXPOSURE: f32 = 8.94;
const CANOPY_FLOOR: f32 = 0.26;
const CANOPY_HEIGHT: f32 = 0.46;

// --- palette shape, mirrored from palette.rs --------------------------------
const RAMPS: i32 = 4;
const RAMP_STEPS: i32 = 16;
const PALETTE_SIZE: i32 = 64;
const RAMP_SHADOW: i32 = 0;
const RAMP_BODY: i32 = 1;
const RAMP_HIGHLIGHT: i32 = 2;
const RAMP_DRY: i32 = 3;

const TAU: f32 = 6.2831855;

// Steps used to integrate the blade centreline. Fewer than a smooth renderer
// would need: the result is snapped to a pixel grid afterwards, which quantises
// the curve far more coarsely than the integration does.
const ARC_STEPS: i32 = 6;

// Distinct lean directions a blade can hold.
//
// Twenty-four is a fifteen-degree step, which moves the tip of a ten-pixel
// blade by about two pixels — a visible, deliberate step rather than a slide.
// Raise it much past this and the quantisation stops reading as drawn.
const POSE_ANGLES: f32 = 24.0;

// Distinct lean magnitudes between upright and the solver's cap.
const POSE_STEPS: f32 = 7.0;

// Smallest a blade may draw, in canvas pixels.
//
// Just over one, not exactly one. A ribbon exactly one pixel wide has to
// contain a pixel centre to be rasterised at all, and half a pixel of phase
// error either way would drop the blade entirely. The excess costs an
// occasional two-pixel stroke and buys a stroke that is never missing.
const MIN_BLADE_PIXELS: f32 = 1.05;

struct GrassSettings {
    field_origin: vec2<f32>,
    field_inverse_extent: vec2<f32>,
    field_resolution: f32,
    time: f32,
    max_angle: f32,
    pixels_per_unit: f32,
    root_stiffness: f32,
    bend_exponent: f32,
    compaction_shorten: f32,
    matting_angle: f32,
    dither: f32,
    sparkle: f32,
    gust_lift: f32,
    shade_gain: f32,
    shade_floor: f32,
    shade_contrast: f32,
    shadow_cut: f32,
    highlight_cut: f32,
    macro_metres: f32,
    macro_strength: f32,
    _pad0: f32,
    _pad1: f32,
    palette: array<vec4<f32>, 64>,
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
    // rest lean direction, rest lean angle, tint, per-blade random
    @location(3) variant: vec4<f32>,
}

struct GrassOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) shade: f32,
    // Flat: the ramp is a property of the whole blade, and interpolating an
    // index between two ramps would read out of a third ramp entirely.
    @location(1) @interpolate(flat) ramp: i32,
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

// Move a projected point onto the centre of the canvas pixel containing it.
//
// Pixel *centres* rather than boundaries: a stroke one pixel wide, centred on a
// boundary, straddles two pixels and covers the centre of neither.
fn snap_to_pixel(screen: vec2<f32>) -> vec2<f32> {
    let ppu = settings.pixels_per_unit;
    if (ppu <= 0.0) {
        return screen;
    }
    return (floor(screen * ppu) + 0.5) / ppu;
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

// How much sky reaches a point this many *metres* above the ground.
//
// Metres, not a fraction of the blade. That distinction is the whole reason a
// two-layer canopy reads as one surface: a mat blade's tip at thirty
// centimetres is still buried among its neighbours and gets lit like it, while
// a tuft blade's tip at a metre is out in the open and catches the key. Scale
// by each blade's own length instead and every tip comes out identically lit,
// which flattens the two layers into one.
//
// Mirrored from `light::canopy_occlusion`.
fn canopy_occlusion(height: f32) -> f32 {
    return mix(CANOPY_FLOOR, 1.0, smoothstep(0.0, CANOPY_HEIGHT, height));
}

// A cheap extra random from one stored value, so a blade can have more
// independent properties than the four bytes it carries.
fn hash11(x: f32) -> f32 {
    return fract(sin(x * 78.233) * 43758.5453);
}

// --- large-scale variation --------------------------------------------------
//
// Deliberately identical to the ground's, in both formula and scale, so the two
// layers light and shade together. A meadow needs structure at a scale much
// larger than a blade — the lighter sweep of a rise, the darker pool of a
// hollow — and without it a field is uniformly interesting everywhere, which is
// the same as being uniformly flat. It is the difference between a texture and
// a place.

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

fn macro_variation(world: vec2<f32>) -> f32 {
    let p = world / max(settings.macro_metres, 0.01);
    var value = 0.0;
    var amplitude = 0.5;
    var total = 0.0;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        value += amplitude * value_noise(q);
        total += amplitude;
        q = q * 2.03 + vec2<f32>(17.3, -9.1);
        amplitude *= 0.5;
    }
    return value / total;
}

// Snap `value` onto a grid of spacing `step` that has been shifted by `offset`
// steps. The shift is what staggers when neighbouring blades change pose.
fn snap_with_offset(value: f32, step: f32, offset: f32) -> f32 {
    if (step <= 1e-6) {
        return value;
    }
    return step * (round(value / step - offset) + offset);
}

// Reduce a bend vector to one of a fixed set of poses.
fn quantise_pose(bend: vec2<f32>, random: f32) -> vec2<f32> {
    let magnitude = length(bend);
    if (magnitude < 1e-5) {
        return vec2<f32>(0.0);
    }
    let angle_offset = hash11(random + 2.19);
    let magnitude_offset = hash11(random + 7.41);

    let angle = snap_with_offset(
        atan2(bend.y, bend.x),
        TAU / POSE_ANGLES,
        angle_offset,
    );
    let snapped = max(
        snap_with_offset(magnitude, settings.max_angle / POSE_STEPS, magnitude_offset),
        0.0,
    );
    return vec2<f32>(cos(angle), sin(angle)) * snapped;
}

// The bend-angle vector at height `u`: direction of lean, magnitude in radians.
fn bend_at(u: f32, total: vec2<f32>) -> vec2<f32> {
    return total * bend_profile(u);
}

// Unit tangent of the blade at height `u`, in world space.
//
// Built from an angle rather than from a difference of positions, which is what
// makes the arc length exact: the tangent is a unit vector by construction, so
// integrating it cannot stretch or shrink the blade.
fn tangent_at(u: f32, total: vec2<f32>) -> vec3<f32> {
    let bend = bend_at(u, total);
    let angle = length(bend);
    var direction = vec2<f32>(1.0, 0.0);
    if (angle > 1e-5) {
        direction = bend / angle;
    }
    return vec3<f32>(direction * sin(angle), cos(angle));
}

// --- the lighting rig -------------------------------------------------------

// How much of one sun a strand catches, and how much it glints.
//
// A blade is effectively a cylinder with no single normal, so lighting is
// computed from its tangent instead: brightest across the light, darkest along
// it. Because the tangent turns as the blade bends, grass changes tone as it
// moves — which is most of why a gust reads as a wave of light travelling
// across a field rather than as geometry wiggling.
//
// Mirrored from `light::strand`.
fn strand(tangent: vec3<f32>, direction: vec3<f32>) -> vec2<f32> {
    let along_light = dot(tangent, direction);
    let along_view = dot(tangent, VIEW_DIRECTION);
    let across_light = sqrt(max(0.0, 1.0 - along_light * along_light));
    let across_view = sqrt(max(0.0, 1.0 - along_view * along_view));
    let glint = pow(max(0.0, across_light * across_view - along_light * along_view), 20.0);
    return vec2<f32>(across_light, glint);
}

// What each sun contributes to a strand: `(key, fill, rim, sky visibility)`.
//
// The sky is carried rather than assumed constant. It is the second largest
// term in the rig, so an unoccluded ambient would land a fifth of full exposure
// on every blade whatever its depth and flatten the whole canopy onto one
// palette step.
//
// The rim arrives almost entirely through its glint, which is what separates a
// rim light from a third fill: a strand's diffuse response is near one for
// every orientation except parallel, so a diffuse rim at this energy would coat
// every blade in the field with cool light. The glint is a twentieth-power term
// and fires only where a blade is edge-on to the light and to the camera at
// once — the edge a rim is meant to draw.
//
// Mirrored from `light::respond`.
fn respond(tangent: vec3<f32>, occlusion: f32) -> vec4<f32> {
    let k = strand(tangent, KEY_DIRECTION);
    let f = strand(tangent, FILL_DIRECTION);
    let r = strand(tangent, RIM_DIRECTION);
    return vec4<f32>(
        (k.x + k.y * 0.6) * occlusion,
        f.x * occlusion,
        (r.y + r.x * RIM_DIFFUSE) * occlusion * RIM_STRAND,
        occlusion,
    );
}

// Total light on a strand, normalised so 1.0 is a fully exposed blade square on
// to the key. Picks the step within a ramp.
fn rig_exposure(response: vec4<f32>) -> f32 {
    let total = response.x * KEY_ENERGY
        + response.y * FILL_ENERGY
        + response.z * RIM_ENERGY
        + response.w * SKY_ENERGY;
    return clamp(total / FULL_EXPOSURE, 0.0, 1.0);
}

// Fraction of the light on a strand that is the key rather than the fill.
// Picks the ramp, which is how the rig shows up as colour rather than only as
// brightness.
fn rig_key_share(response: vec4<f32>) -> f32 {
    let warm = response.x * KEY_ENERGY;
    let cool = response.y * FILL_ENERGY + response.z * RIM_ENERGY + response.w * SKY_ENERGY * 0.5;
    if (warm + cool <= 1e-6) {
        return 0.0;
    }
    return clamp(warm / (warm + cool), 0.0, 1.0);
}

// --- dithering --------------------------------------------------------------

// The 4x4 ordered dither matrix, in closed form rather than as a table.
//
// Returns 0..15. Indexed by canvas pixel, so the pattern is fixed to the screen
// and the grass moves through it. Fixing it to the geometry instead makes the
// dither swim with every blade, which is far more visible than the banding it
// was meant to hide.
fn bayer4(x: u32, y: u32) -> f32 {
    let a = x ^ y;
    let value = ((a & 1u) << 3u) | ((y & 1u) << 2u) | (((a >> 1u) & 1u) << 1u) | ((y >> 1u) & 1u);
    return f32(value);
}

// --- vertex -----------------------------------------------------------------

@vertex
fn vertex(vertex: Vertex) -> GrassOutput {
    let height = vertex.shape.x;
    let side = vertex.shape.y * 2.0 - 1.0;
    let blade_length = mix(LENGTH_MIN, LENGTH_MAX, vertex.shape.z);
    let blade_width = mix(WIDTH_MIN, WIDTH_MAX, vertex.shape.w);
    let rest_angle = vertex.variant.x * TAU;
    let rest_lean = mix(REST_LEAN_MIN, REST_LEAN_MAX, vertex.variant.y);
    let tint = vertex.variant.z;
    let random = vertex.variant.w;

    let uv = (vertex.root - settings.field_origin) * settings.field_inverse_extent;
    let field = sample_bend(uv);
    let compaction = sample_state(uv);

    // Every blade starts with its own lean, fanned out from its tuft. Grass
    // does not grow as a bed of nails, and without this the canopy is a uniform
    // vertical brush that only ever moves when the wind moves it.
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
        let chosen = select(-1.0, 1.0, random < bias);
        bend += settings.matting_angle * chosen * alignment * axis_direction;
    }

    // Whatever crushing is left over after the aligned part is directionless:
    // grass trampled from every angle. Each blade falls a stable random way.
    let scattered = max(compaction - alignment, 0.0);
    if (scattered > 1e-4) {
        let scatter_angle = hash11(random + 19.3) * TAU;
        bend += settings.matting_angle * 0.55 * scattered
            * vec2<f32>(cos(scatter_angle), sin(scatter_angle));
    }

    // How hard the *field* is working here, before the blade's own rest lean is
    // counted. Drives the sparkle, so a still meadow does not twinkle.
    let moving = clamp(length(field.xy) / max(settings.max_angle, 1e-4), 0.0, 1.0);

    bend = quantise_pose(bend, random);
    let lean = length(bend);

    // Integrate the centreline from root to this vertex's height. Midpoint
    // rule: the blade cannot stretch, because every tangent is a unit vector.
    var centre = vec3<f32>(vertex.root, 0.0);
    let step = height / f32(ARC_STEPS);
    for (var i = 0; i < ARC_STEPS; i = i + 1) {
        let mid = (f32(i) + 0.5) * step;
        centre += tangent_at(mid, bend) * (blade_length * step);
    }

    // Crushed grass loses a little height beyond what bending already takes.
    // Only a little: most of the shortening should come from the blade leaning
    // over, because that is what actually happens.
    centre.z *= 1.0 - settings.compaction_shorten * compaction;

    let tangent = tangent_at(height, bend);

    // Project the centreline and put it on a pixel, then widen along the
    // screen-space perpendicular by a whole number of pixels. Widening in
    // screen space rather than in world space keeps a blade the same apparent
    // thickness however it is oriented, which stops blades lying toward the
    // camera from turning into slivers.
    let screen = snap_to_pixel(project(centre));
    let screen_tangent = project(tangent);
    var normal = vec2<f32>(1.0, 0.0);
    let tangent_length = length(screen_tangent);
    if (tangent_length > 1e-5) {
        normal = vec2<f32>(-screen_tangent.y, screen_tangent.x) / tangent_length;
    }

    let ppu = max(settings.pixels_per_unit, 1e-4);
    // Full width in pixels, tapered and then rounded, so a blade is one pixel
    // wide or two but never one and a half.
    let full_pixels = max(
        MIN_BLADE_PIXELS,
        round(blade_width * 2.0 * ppu * width_profile(height)),
    );
    let offset = normal * (side * full_pixels * 0.5 / ppu);

    let local = vec4<f32>(screen + offset, iso_depth(centre), 1.0);

    var out: GrassOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world_position = mesh_functions::mesh2d_position_local_to_world(world_from_local, local);
    out.position = mesh_functions::mesh2d_position_world_to_clip(world_position);

    // --- where the rig puts this blade on the palette ------------------------
    let occlusion = canopy_occlusion(centre.z);
    let response = respond(tangent, occlusion);

    var shade = pow(rig_exposure(response), settings.shade_contrast) * settings.shade_gain
        + settings.shade_floor;

    // Per-blade brightness. Without it every blade on a ramp lands on the same
    // step at the same height and the field comes out in flat bands.
    shade *= 0.86 + 0.30 * hash11(random + 11.7);

    // The large sweep of light across the field, shared with the ground so the
    // two never disagree about where a rise is.
    shade += (macro_variation(vertex.root) - 0.5) * settings.macro_strength;

    // Grass leaning hard is catching the light differently. This is what makes
    // gust fronts visible as bands travelling across the field.
    shade += settings.gust_lift * smoothstep(0.05, 0.9, lean);

    // The pixel-art replacement for tip flutter. At one pixel wide a trembling
    // tip is invisible, but a blade stepping one shade brighter and back is
    // not — so the shimmer of wind over a meadow is done in tone rather than in
    // geometry. Scaled by how much this patch is actually moving, so still
    // grass stays still.
    let rate = 4.0 + 7.0 * hash11(random + 3.3);
    shade += settings.sparkle * moving * sin(settings.time * rate + random * TAU);

    out.shade = clamp(shade, 0.0, 1.0);

    // Which hue family this blade belongs to: how much of its light is the key.
    // Constant for the whole blade — shading moves it up and down one ramp,
    // never across ramps. The per-blade jitter keeps the boundary from drawing
    // itself as a hard line across the field.
    let share = rig_key_share(response) + (tint - 0.5) * 0.14;
    var ramp = RAMP_BODY;
    if (share < settings.shadow_cut) {
        ramp = RAMP_SHADOW;
    } else if (share > settings.highlight_cut) {
        ramp = RAMP_HIGHLIGHT;
    }
    out.ramp = ramp;
    return out;
}

@fragment
fn fragment(in: GrassOutput) -> @location(0) vec4<f32> {
    // Ordered dithering across the palette step, in canvas pixels. Six steps
    // per ramp is coarse enough to band visibly on a broad gradient; a little
    // dither trades that banding for the stipple a pixel artist would use, and
    // costs nothing.
    let pixel = vec2<u32>(in.position.xy);
    let dither = (bayer4(pixel.x, pixel.y) + 0.5) / 16.0 - 0.5;

    let level = clamp(
        in.shade * f32(RAMP_STEPS) - 0.5 + dither * settings.dither,
        0.0,
        f32(RAMP_STEPS - 1),
    );
    let index = clamp(in.ramp, 0, RAMPS - 1) * RAMP_STEPS + i32(round(level));
    var colour = settings.palette[clamp(index, 0, PALETTE_SIZE - 1)];

#ifdef TONEMAP_IN_SHADER
    colour = tonemapping::tone_mapping(colour, view.color_grading);
#endif
#ifdef SRGB_OUTPUT
    colour = vec4<f32>(linear_to_srgb(colour.rgb), colour.a);
#endif
    return colour;
}

"""Render an exported grass scene with Cycles.

    blender --background --factory-startup --python render.py -- scene.json out.png

The Rust side owns every placement decision and hands this script an explicit
list of curves in world metres. This script owns light transport and nothing
else: it must never decide where a blade goes, because two pages that have never
met agree along a shared edge only for as long as placement stays a pure function
of a world coordinate. Blender's own scattering would break that quietly.

See `bw_grass::cycles` for the wire format and for why the camera is derived
rather than authored.
"""

import json
import os
import sys
import time

import bpy
import numpy as np
from mathutils import Matrix, Vector


def argv_after_double_dash():
    if "--" not in sys.argv:
        raise SystemExit("usage: render.py -- <scene.json> <output.png>")
    rest = sys.argv[sys.argv.index("--") + 1 :]
    if len(rest) < 2:
        raise SystemExit("usage: render.py -- <scene.json> <output.png>")
    return rest[0], rest[1]


def clear_scene():
    """Start from genuinely nothing.

    `--factory-startup` still gives us the default cube, camera and light, and a
    stray 1000 W point light in the middle of the field is the kind of thing that
    costs an hour to notice.
    """
    bpy.ops.wm.read_factory_settings(use_empty=True)


# --------------------------------------------------------------------------
# Geometry
# --------------------------------------------------------------------------


def build_blades(scene_dir, spec, settings):
    """Folded ribbons, straight from the exported buffer.

    A mesh rather than Cycles curve primitives, and that is not an optimisation
    — see `bw_grass::cycles::VERTICES_PER_RIB`. A `RIBBONS` curve is a
    camera-facing quad whose shading normal faces the viewer, so every blade in
    the field presents the same normal to the sun and the canopy shades flat.
    Three vertices per rib give a real fold, and the fold is what puts a lit side
    and a shaded side inside one blade.

    Built with numpy and `foreach_set` throughout. A page holds a few hundred
    thousand blades and touching them through Python objects one at a time takes
    minutes rather than the fraction of a second this takes.
    """
    count = spec["count"]
    ribs = spec["ribs_per_blade"]
    across = spec["vertices_per_rib"]
    if count == 0:
        return None

    per_blade = ribs * across
    raw = np.fromfile(os.path.join(scene_dir, spec["path"]), dtype=np.float32)
    expected = count * per_blade * 3
    if raw.size != expected:
        raise SystemExit(f"blades.bin has {raw.size} floats, expected {expected}")

    mesh = bpy.data.meshes.new("grass")
    mesh.vertices.add(count * per_blade)
    mesh.vertices.foreach_set("co", raw)

    # Two quads per rib gap — one per facet — as triangles. Indices are built
    # once for a single blade and then broadcast across all of them, which keeps
    # the whole thing to a handful of numpy operations.
    blade = []
    for rib in range(ribs - 1):
        base = rib * across
        for side in range(across - 1):
            a = base + side
            b = a + 1
            c = a + across
            d = b + across
            blade.append((a, c, b))
            blade.append((b, c, d))
    blade = np.asarray(blade, dtype=np.int32)
    offsets = (np.arange(count, dtype=np.int32) * per_blade)[:, None, None]
    faces = (blade[None, :, :] + offsets).reshape(-1, 3)

    mesh.loops.add(faces.size)
    mesh.polygons.add(len(faces))
    mesh.loops.foreach_set("vertex_index", faces.ravel())
    mesh.polygons.foreach_set("loop_start", np.arange(len(faces), dtype=np.int32) * 3)
    mesh.update()
    mesh.validate()
    # Smooth along the blade so the arc has no facets, while the fold still
    # rotates the normal across the width — the vertex normals at the two edges
    # average different pairs of triangles.
    mesh.shade_smooth()

    attributes = np.fromfile(
        os.path.join(scene_dir, spec["attributes"]), dtype=np.float32
    ).reshape(count, spec["attributes_per_blade"])
    # Per-blade attributes reach the shader through Attribute nodes by name, so
    # the material can vary without the geometry changing. Repeated to the face
    # domain because that is the coarsest domain every vertex of a blade shares.
    per_face = faces.shape[0] // count
    for index, name in enumerate(("maturity", "moisture", "tone", "exposure")):
        layer = mesh.attributes.new(name=name, type="FLOAT", domain="FACE")
        layer.data.foreach_set("value", np.repeat(attributes[:, index], per_face))

    obj = bpy.data.objects.new("grass", mesh)
    bpy.context.collection.objects.link(obj)
    obj.data.materials.append(blade_material(settings))
    return obj


def build_ground(scene_dir, spec):
    """A displaced grid over the page's world footprint."""
    rows, columns = spec["rows"], spec["columns"]
    low = Vector((spec["low"][0], spec["low"][1], 0.0))
    high = Vector((spec["high"][0], spec["high"][1], 0.0))
    heights = np.fromfile(os.path.join(scene_dir, spec["path"]), dtype=np.float32)
    if heights.size != rows * columns:
        raise SystemExit(f"ground.bin has {heights.size} floats, expected {rows*columns}")

    xs = np.linspace(low.x, high.x, columns, dtype=np.float32)
    ys = np.linspace(low.y, high.y, rows, dtype=np.float32)
    grid_x, grid_y = np.meshgrid(xs, ys)
    vertices = np.stack(
        [grid_x.ravel(), grid_y.ravel(), heights.reshape(rows, columns).ravel()], axis=1
    )

    # Two triangles per cell, built with numpy rather than a Python loop for the
    # same reason as the curves.
    r = np.arange(rows - 1)[:, None]
    c = np.arange(columns - 1)[None, :]
    top_left = (r * columns + c).ravel()
    top_right = top_left + 1
    bottom_left = top_left + columns
    bottom_right = bottom_left + 1
    faces = np.empty((top_left.size * 2, 3), dtype=np.int32)
    faces[0::2] = np.stack([top_left, bottom_left, top_right], axis=1)
    faces[1::2] = np.stack([top_right, bottom_left, bottom_right], axis=1)

    mesh = bpy.data.meshes.new("ground")
    mesh.from_pydata(vertices.tolist(), [], faces.tolist())
    mesh.update()
    mesh.shade_smooth()

    obj = bpy.data.objects.new("ground", mesh)
    bpy.context.collection.objects.link(obj)
    obj.data.materials.append(ground_material())
    return obj


# --------------------------------------------------------------------------
# Materials
# --------------------------------------------------------------------------


def blade_material(settings):
    """A grass blade: translucent, slightly waxy, greener where it is younger.

    The one shading decision worth stating: **Subsurface Weight and Transmission
    are what make a canopy read as deep.** A blade is a thin translucent sheet,
    so light that enters its lit face leaves through the shaded one, and the
    interior of a tuft is lit almost entirely by green light that has already
    passed through something. That is the multiple scattering a rasteriser cannot
    do and the reason this pipeline exists.
    """
    material = bpy.data.materials.new("blade")
    material.use_nodes = True
    tree = material.node_tree
    nodes, links = tree.nodes, tree.links
    nodes.clear()

    output = nodes.new("ShaderNodeOutputMaterial")
    principled = nodes.new("ShaderNodeBsdfPrincipled")
    principled.location = (-300, 0)

    # Root-to-tip position. With curves this was the curve parameter; on a mesh
    # it has to be carried, and the cheapest honest carrier is height above the
    # soil — a blade's tip is its highest point and its root its lowest, so the
    # two agree everywhere it matters and disagree only on blades lying flat,
    # which is where a tip highlight would be wrong anyway.
    geometry = nodes.new("ShaderNodeNewGeometry")
    geometry.location = (-1500, -200)
    separate = nodes.new("ShaderNodeSeparateXYZ")
    separate.location = (-1300, -200)
    along = nodes.new("ShaderNodeMapRange")
    along.location = (-1100, -200)
    along.inputs["From Min"].default_value = 0.0
    along.inputs["From Max"].default_value = 0.30
    links.new(geometry.outputs["Position"], separate.inputs["Vector"])
    links.new(separate.outputs["Z"], along.inputs["Value"])

    maturity = nodes.new("ShaderNodeAttribute")
    maturity.attribute_name = "maturity"
    maturity.attribute_type = "GEOMETRY"
    maturity.location = (-1100, 100)

    # A ramp along the blade: darker at the root where it is buried, fuller
    # green through the body, warming toward the tip.
    #
    # **These are linear reflectances, not colours picked by eye.** That
    # distinction cost a render: the first set were chosen as though they were
    # sRGB, and linear (0.205, 0.310, 0.070) is a greyish yellow-green that
    # measured a Lab chroma of 13 against the target's 39. A saturated grass
    # green needs far *less* red and blue relative to green than it looks like
    # it should — the eye reads (0.09, 0.28, 0.02) as vivid, and the arithmetic
    # agrees, because saturation lives in the ratio between channels rather than
    # in their size.
    ramp = nodes.new("ShaderNodeValToRGB")
    ramp.location = (-700, -100)
    ramp.color_ramp.interpolation = "EASE"
    stops = [
        (0.00, (0.0030, 0.0345, 0.0014, 1.0)),
        (0.22, (0.0064, 0.0840, 0.0022, 1.0)),
        (0.55, (0.0112, 0.1820, 0.0026, 1.0)),
        (0.82, (0.0178, 0.2760, 0.0032, 1.0)),
        (1.00, (0.0325, 0.3720, 0.0044, 1.0)),
    ]
    first = ramp.color_ramp.elements[0]
    first.position, first.color = stops[0][0], stops[0][1]
    second = ramp.color_ramp.elements[1]
    second.position, second.color = stops[-1][0], stops[-1][1]
    for position, colour in stops[1:-1]:
        element = ramp.color_ramp.elements.new(position)
        element.color = colour

    # Older blades sit a little duller and drier; younger shoots are cleaner.
    mix = nodes.new("ShaderNodeMix")
    mix.data_type = "RGBA"
    mix.location = (-500, 0)
    mix.inputs["B"].default_value = (0.026, 0.145, 0.004, 1.0)

    links.new(along.outputs["Result"], ramp.inputs["Fac"])
    links.new(ramp.outputs["Color"], mix.inputs["A"])
    links.new(maturity.outputs["Fac"], mix.inputs["Factor"])
    links.new(mix.outputs["Result"], principled.inputs["Base Color"])

    # ## Why the specular lobe is kept small
    #
    # A specular highlight is the *light's* colour, not the surface's, so on a
    # green blade it is white. The first pass gave blades a waxy cuticle — high
    # specular, a sheen and a coat — and the result measured a highlight chroma
    # of 36 against the target's 59, with twelve times its clipped-pixel count.
    # Read at native size those highlights were white wires laid over the field
    # rather than blades catching the sun.
    #
    # The target's bright pixels are lit *through* green tissue. So the energy
    # moves out of the specular lobe and into subsurface and transmission, both
    # of which are tinted by the blade — brightness that stays green. What
    # specular remains is broad rather than sharp, because a rough lobe spreads
    # the same energy over a wider band and stops clipping.
    principled.inputs["Roughness"].default_value = 0.48
    principled.inputs["Subsurface Weight"].default_value = 0.70
    principled.inputs["Subsurface Scale"].default_value = 0.008
    principled.inputs["Subsurface Radius"].default_value = (0.004, 0.012, 0.002)
    # A blade is a sheet, not a solid; this is what stops Cycles treating it as
    # a volume with an inside.
    principled.inputs["Thin Wall"].default_value = True
    principled.inputs["Transmission Weight"].default_value = 0.32
    principled.inputs["Specular IOR Level"].default_value = 0.20
    principled.inputs["Sheen Weight"].default_value = 0.06
    principled.inputs["Sheen Roughness"].default_value = 0.55

    links.new(principled.outputs["BSDF"], output.inputs["Surface"])
    return material


def ground_material():
    """What is at the bottom of a gap: moss and dead thatch, not bare earth.

    Deliberately darker than exposed soil would be in isolation, because the
    ground is seen almost entirely through gaps and a soil bright enough to look
    right on its own reads as a hole punched through the grass.
    
    But *too* dark is the opposite failure and the one this had. A canopy gap
    with near-black at the bottom does not read as depth, it reads as missing
    geometry — the eye needs some returned light to understand a recess as a
    recess. Real turf almost never shows clean earth either: what is down there
    is moss, dead thatch and stained soil, all of it green-shifted. So this ramp
    runs from a mossy shadow to a dry olive rather than from black to brown, and
    the green in it is what lets a gap read as deep rather than as punched.
    """
    material = bpy.data.materials.new("ground")
    material.use_nodes = True
    tree = material.node_tree
    nodes, links = tree.nodes, tree.links
    nodes.clear()

    output = nodes.new("ShaderNodeOutputMaterial")
    principled = nodes.new("ShaderNodeBsdfPrincipled")
    principled.location = (-300, 0)

    noise = nodes.new("ShaderNodeTexNoise")
    noise.location = (-900, 0)
    noise.inputs["Scale"].default_value = 12.0
    noise.inputs["Detail"].default_value = 6.0

    ramp = nodes.new("ShaderNodeValToRGB")
    ramp.location = (-600, 0)
    ramp.color_ramp.elements[0].color = (0.026, 0.052, 0.014, 1.0)
    ramp.color_ramp.elements[1].color = (0.090, 0.104, 0.036, 1.0)

    links.new(noise.outputs["Fac"], ramp.inputs["Fac"])
    links.new(ramp.outputs["Color"], principled.inputs["Base Color"])
    principled.inputs["Roughness"].default_value = 0.92
    links.new(principled.outputs["BSDF"], output.inputs["Surface"])
    return material


def build_world(sky):
    """Sky as a gradient rather than a constant.

    A uniform world lights the underside of the canopy as strongly as the top and
    flattens everything. A sky that is bright above and dark at the horizon is
    most of what gives grass its vertical shading for free.
    """
    world = bpy.data.worlds.new("sky")
    bpy.context.scene.world = world
    world.use_nodes = True
    tree = world.node_tree
    nodes, links = tree.nodes, tree.links
    nodes.clear()

    output = nodes.new("ShaderNodeOutputWorld")
    background = nodes.new("ShaderNodeBackground")
    background.inputs["Strength"].default_value = sky["strength"]

    gradient = nodes.new("ShaderNodeTexGradient")
    gradient.gradient_type = "EASING"
    gradient.location = (-800, 0)
    mapping = nodes.new("ShaderNodeMapping")
    mapping.location = (-1000, 0)
    mapping.inputs["Rotation"].default_value = (1.5708, 0.0, 0.0)
    coordinate = nodes.new("ShaderNodeTexCoord")
    coordinate.location = (-1200, 0)

    ramp = nodes.new("ShaderNodeValToRGB")
    ramp.location = (-500, 0)
    # Ground bounce at the bottom, open sky at the top.
    ramp.color_ramp.elements[0].color = (0.075, 0.085, 0.055, 1.0)
    ramp.color_ramp.elements[1].color = (sky["colour"][0], sky["colour"][1], sky["colour"][2], 1.0)

    links.new(coordinate.outputs["Generated"], mapping.inputs["Vector"])
    links.new(mapping.outputs["Vector"], gradient.inputs["Vector"])
    links.new(gradient.outputs["Fac"], ramp.inputs["Fac"])
    links.new(ramp.outputs["Color"], background.inputs["Color"])
    links.new(background.outputs["Background"], output.inputs["Surface"])


def build_sun(sun):
    """A sun light aimed by elevation and bearing, with a real angular size.

    `angle` is the whole reason shadows here have penumbrae at all. The
    rasteriser this replaces had to derive a sun radius large enough that a
    penumbra spanned more than one page pixel; a path tracer just integrates the
    solid angle and the softness comes out right at every distance.
    """
    data = bpy.data.lights.new("sun", type="SUN")
    data.energy = sun["strength"]
    data.angle = sun["angle"]
    data.color = tuple(sun["colour"])

    obj = bpy.data.objects.new("sun", data)
    bpy.context.collection.objects.link(obj)

    elevation, azimuth = sun["elevation"], sun["azimuth"]
    direction = Vector(
        (
            np.cos(elevation) * np.cos(azimuth),
            np.cos(elevation) * np.sin(azimuth),
            np.sin(elevation),
        )
    )
    # A sun object emits along its local -Z, so it is rotated to point *down* the
    # vector from the sun toward the ground.
    obj.rotation_mode = "QUATERNION"
    obj.rotation_quaternion = direction.to_track_quat("Z", "Y")
    obj.location = direction * 100.0
    return obj


def build_camera(spec, page):
    """The orthographic camera `bw_grass::cycles::Camera` derived.

    Nothing here is chosen. The basis, the scale and the pixel aspect all arrive
    computed, because the page is a texture the game samples under a fixed
    projection and a camera half a degree out puts the grass out of register with
    the tiles under it.
    """
    data = bpy.data.cameras.new("camera")
    data.type = "ORTHO"
    data.ortho_scale = spec["ortho_scale"]
    # So `ortho_scale` always means the horizontal extent, whatever the page
    # shape. Under AUTO it would mean the larger axis and a tall page would
    # silently transpose the framing.
    data.sensor_fit = "HORIZONTAL"
    data.clip_start = 1.0
    data.clip_end = 1000.0

    obj = bpy.data.objects.new("camera", data)
    bpy.context.collection.objects.link(obj)

    right, up, backward = (Vector(axis) for axis in spec["basis"])
    obj.matrix_world = Matrix(
        (
            (right.x, up.x, backward.x, spec["location"][0]),
            (right.y, up.y, backward.y, spec["location"][1]),
            (right.z, up.z, backward.z, spec["location"][2]),
            (0.0, 0.0, 0.0, 1.0),
        )
    )
    bpy.context.scene.camera = obj
    return obj


# --------------------------------------------------------------------------
# Render
# --------------------------------------------------------------------------


def configure_render(render_spec, camera_spec, output):
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.render.resolution_x = render_spec["resolution"][0]
    scene.render.resolution_y = render_spec["resolution"][1]
    scene.render.resolution_percentage = 100
    scene.render.filepath = output
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.render.image_settings.color_depth = "8"

    # The dimetric stretch. See `bw_grass::cycles::Camera`.
    scene.render.pixel_aspect_x = 1.0
    scene.render.pixel_aspect_y = camera_spec["pixel_aspect_y"]

    cycles = scene.cycles
    cycles.samples = render_spec["samples"]
    cycles.use_denoising = render_spec["denoise"]
    cycles.denoiser = "OPENIMAGEDENOISE"
    cycles.use_adaptive_sampling = True
    cycles.adaptive_threshold = 0.01
    # Light bounces around inside a canopy more than almost any other subject —
    # the deep green of grass interiors is third- and fourth-bounce light.
    # Cutting these is the fastest way to make a lush field look flat.
    cycles.max_bounces = 16
    cycles.diffuse_bounces = 8
    cycles.transmission_bounces = 12
    cycles.glossy_bounces = 4
    # Deterministic across runs: same scene in, same picture out.
    cycles.seed = 0
    cycles.use_animated_seed = False

    scene.view_settings.view_transform = render_spec["view_transform"]
    scene.view_settings.look = "None"

    configure_device(render_spec["device"], cycles)


def configure_device(wanted, cycles):
    """Put the trace on the GPU when there is one, and say so when there is not."""
    if wanted != "GPU":
        cycles.device = "CPU"
        print("[bw_cycles] device: CPU (requested)")
        return
    try:
        prefs = bpy.context.preferences.addons["cycles"].preferences
        available = [t[0] for t in prefs.get_device_types(bpy.context)]
        for backend in ("OPTIX", "CUDA", "METAL", "HIP", "ONEAPI"):
            if backend not in available:
                continue
            prefs.compute_device_type = backend
            prefs.get_devices()
            usable = [d for d in prefs.devices if d.type == backend]
            if not usable:
                continue
            for device in prefs.devices:
                device.use = device.type == backend
            cycles.device = "GPU"
            print(f"[bw_cycles] device: {backend} — {[d.name for d in usable]}")
            return
    except Exception as error:  # noqa: BLE001
        print(f"[bw_cycles] GPU setup failed ({error}); falling back to CPU")
    cycles.device = "CPU"
    print("[bw_cycles] device: CPU (no GPU backend available)")


def enable_passes(scene):
    """The AOVs the neural renderer is trained against.

    Configuration rather than plumbing. The renderer this replaces carried ten
    channels by hand through its own resolve loop, and each one was a chance for
    the recorded value to drift from the value actually used.
    """
    layer = scene.view_layers[0]
    layer.use_pass_combined = True
    layer.use_pass_z = True
    layer.use_pass_normal = True
    layer.use_pass_diffuse_direct = True
    layer.use_pass_diffuse_indirect = True
    layer.use_pass_diffuse_color = True
    layer.use_pass_glossy_direct = True
    layer.use_pass_transmission_direct = True
    layer.use_pass_ambient_occlusion = True
    layer.use_pass_shadow_catcher = False
    layer.use_pass_cryptomatte_object = True
    layer.cycles.denoising_store_passes = True


def main():
    header_path, output = argv_after_double_dash()
    started = time.time()

    with open(header_path, "r", encoding="utf-8") as handle:
        spec = json.load(handle)
    scene_dir = os.path.dirname(os.path.abspath(header_path))

    clear_scene()
    build_world(spec["sky"])
    build_sun(spec["sun"])
    build_camera(spec["camera"], spec["page"])
    ground = build_ground(scene_dir, spec["ground"])
    blades = build_blades(scene_dir, spec["blades"], spec)
    configure_render(spec["render"], spec["camera"], output)
    if spec["render"].get("passes"):
        enable_passes(bpy.context.scene)

    built = time.time()
    print(
        f"[bw_cycles] built {spec['blades']['count']} curves "
        f"({spec['ground']['rows']}x{spec['ground']['columns']} ground) in {built - started:.1f}s"
    )
    if blades is None:
        print("[bw_cycles] warning: no blades in this scene")
    _ = ground

    bpy.ops.render.render(write_still=True)
    print(f"[bw_cycles] rendered in {time.time() - built:.1f}s -> {output}")


if __name__ == "__main__":
    main()

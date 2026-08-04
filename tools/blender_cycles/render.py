"""Render an exported terrain scene with Cycles.

    blender --background --factory-startup --python render.py -- scene.json out.png

The Rust side owns every placement decision and hands this script an explicit
list of geometry in world metres. This script owns light transport and nothing
else: **it must never decide where anything goes.** Two pages that have never met
agree along a shared edge only for as long as placement stays a pure function of
a world coordinate, and Blender's own scattering would break that quietly — the
seam would appear, nothing would report it, and the cause would be in a different
language from the symptom.

## Materials dispatch on appearance keys

A scene names its materials by key — `plant.grass_blade`, `surface.dirt_compacted`
— and `material_for` turns a key into a shader graph. That indirection is what
lets the Rust side stay generic: adding wildflowers means adding a builder here
and a binding there, not teaching the exporter that Blender has a flower shader.

An appearance key is a renderer-side implementation id, **not** a material-weight
identity. A blade of grass growing on ground that is seventy percent grass and
thirty percent dirt is still made of grass, and the ground under it is separately
seventy-thirty. Conflating the two is what produces transparent grass ghosts at a
boundary.

See `terrain_cycles::package` for the wire format and `terrain_scene::projection`
for why the camera is derived rather than authored — and for why the world
arrives reflected.
"""

import json
import os
import sys
import time

import bpy
import numpy as np
from mathutils import Matrix, Vector


def jobs_from_argv():
    """One scene, or a manifest of many.

    Blender takes several seconds to start and a page takes about one to trace,
    so a process per page spends most of its life starting up. A manifest lets
    one process do a whole pre-bake: `--manifest` names a file of
    `scene.json<TAB>output.png` lines, and startup is paid once for all of them.
    """
    if "--" not in sys.argv:
        raise SystemExit("usage: render.py -- <scene.json> <output.png>")
    rest = sys.argv[sys.argv.index("--") + 1 :]
    if len(rest) >= 2 and rest[0] == "--manifest":
        with open(rest[1], "r", encoding="utf-8") as handle:
            jobs = []
            for line in handle:
                line = line.rstrip("\n")
                if not line:
                    continue
                scene, _, output = line.partition("\t")
                jobs.append((scene, output))
        return jobs
    if len(rest) < 2:
        raise SystemExit("usage: render.py -- <scene.json> <output.png>")
    return [(rest[0], rest[1])]


def live(sockets, name):
    """The *enabled* socket of a given name.

    `ShaderNodeMix` carries one A/B/Result triple per data type and they all
    share their names, so `inputs["A"]` returns whichever comes first — the
    `VALUE` one at index two, which is disabled whenever the node is set to
    RGBA. Linking to it is not an error and draws no warning; the link simply
    has no effect.

    That cost two rounds of tuning. The maturity blend and the hue axis were
    both wired to dead sockets, so a measured hue spread sat at four degrees
    against reference art's seven and would not move however hard the tints were
    pushed — the shader was evaluating neither of them.
    """
    for socket in sockets:
        if socket.name == name and socket.enabled:
            return socket
    raise KeyError(f"no enabled socket named {name!r}")


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
# Material dispatch
# --------------------------------------------------------------------------
#
# A scene names its materials by **appearance key** — `plant.grass_blade`,
# `surface.dirt_compacted`, `rock.granite` — and this is where a key becomes a
# shader graph.
#
# The indirection is the whole reason the exporter can stay generic. Without it,
# adding wildflowers means teaching the Rust side that Blender has a flower
# shader, and the scene format grows a section per plant. With it, the scene says
# "this mark looks like `plant.wildflower_head`" and the two sides agree on a
# string.
#
# An appearance key is a **renderer-side implementation id**, not a
# material-weight identity. A blade of grass growing on ground that is seventy
# percent grass and thirty percent dirt is still made of grass, and the ground
# under it is separately seventy-thirty. Those are answers to different
# questions, and conflating them is what makes a boundary produce transparent
# grass ghosts.


def appearance_builders(settings):
    """Every appearance this build knows how to construct.

    Explicit rather than discovered, for the same reason the Rust-side registry
    is: with automatic registration, two builders claiming one key are resolved
    by import order, and the same scene renders differently depending on how the
    module happened to load.
    """
    return {
        "plant.grass_blade": lambda: blade_material(settings),
        "plant.broad_leaf": lambda: blade_material(settings),
        "plant.thatch": lambda: blade_material(settings),
        "plant.dry_stem": lambda: blade_material(settings),
        "surface.grass_lush": ground_material,
        "surface.bare_soil": ground_material,
        "surface.dirt_compacted": ground_material,
    }


def material_for(appearance, settings, cache):
    """The shader graph for one appearance key, built once per render.

    An unknown key is a *reported* fallback rather than a silent one. A scene
    that named `plant.wildflowe_head` and got grass would render perfectly and be
    wrong, and nothing anywhere would say why — which is the same failure mode
    `deny_unknown_fields` exists to prevent on the authoring side.
    """
    if appearance in cache:
        return cache[appearance]

    builders = appearance_builders(settings)
    builder = builders.get(appearance)
    if builder is None:
        print(
            f"[terrain_cycles] no builder for appearance '{appearance}'; "
            f"falling back to a plain surface. Known: {sorted(builders)}"
        )
        builder = ground_material

    material = builder()
    cache[appearance] = material
    return material


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
        # The root end is lifted well above what a buried blade base physically
        # reflects, and deliberately. This is where the deep-shadow share lives:
        # a canopy this dense buries most of its own material, so the darkest
        # third of the picture is blade *roots* rather than shadow. Left at a
        # physical value it swallowed a third of the frame, and for a surface
        # that has to read under units and effects that is not atmosphere — it is
        # a hole where the gameplay happens.
        (0.00, (0.0068, 0.0680, 0.0026, 1.0)),
        (0.22, (0.0105, 0.1300, 0.0034, 1.0)),
        (0.55, (0.0128, 0.2320, 0.0022, 1.0)),
        (0.82, (0.0208, 0.3320, 0.0025, 1.0)),
        # The lit tip runs warm on purpose. A sunlit blade is yellow-green, and
        # a tip that stays the same hue as its own root reads as a brighter lamp
        # rather than as sunlight landing on it.
        (1.00, (0.0430, 0.4460, 0.0030, 1.0)),
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
    live(mix.inputs, "B").default_value = (0.020, 0.152, 0.003, 1.0)

    links.new(along.outputs["Result"], ramp.inputs["Fac"])
    links.new(ramp.outputs["Color"], live(mix.inputs, "A"))
    links.new(maturity.outputs["Fac"], live(mix.inputs, "Factor"))

    # ## A second colour axis, and why one is not enough
    #
    # Everything above varies a blade's *value* — root to tip, young to old — and
    # a field that varies only in value is one green under a brighter or dimmer
    # lamp. Measured, that is a Lab hue spread of four degrees against reference
    # art's seven: half the variety, and the half the eye reads as "real plants"
    # rather than "one plant recoloured".
    #
    # So this axis varies *which* green. Cool blue-green where a blade is shaded
    # and damp, warm olive where it is exposed and drying, driven by the per-mark
    # light index — which is the attribute that actually varies. `exposure` is the
    # tip highlight and is nearly constant across the field, so hanging the hue
    # axis on it changed nothing at all.
    #
    # Kept deliberately narrow: grass that varies its hue widely reads as several
    # species, and the reference is one species in several moods.
    # Driven by **world-space noise**, not by a per-blade attribute.
    #
    # A per-blade value was tried first and is the obvious thing to reach for. It
    # is also the wrong shape: hue that varies blade-to-blade is salt and pepper,
    # and at canopy density the eye averages it straight back to one green. What
    # the reference actually has is *regional* hue — a stretch of olive here, a
    # cooler damp green there, each running across many plants — which is a low
    # frequency, and only a world-space field has one.
    #
    # Two octaves at about a metre and a half. Slower than a tuft and faster than
    # the mound field, so it cuts across both instead of agreeing with either,
    # which is what stops it reading as another consequence of the terrain.
    coordinate = nodes.new("ShaderNodeTexCoord")
    coordinate.location = (-1500, 320)
    drift = nodes.new("ShaderNodeTexNoise")
    drift.location = (-1300, 320)
    # One octave and a long wavelength. Detail here is actively harmful: extra
    # octaves put fast edges into what is meant to be a slow drift, and a hue
    # boundary with an edge on it stops reading as vegetation and starts reading
    # as a shadow falling across the field.
    drift.inputs["Scale"].default_value = 0.34
    drift.inputs["Detail"].default_value = 0.0
    drift.inputs["Roughness"].default_value = 0.5

    # ## Why this remap is gentle, and was not
    #
    # It first took 0.32–0.68 to the full range **with clamping**, on the
    # reasoning that fractal noise crowds its middle and the tints would
    # otherwise go unused. What that actually produces is a *mask*: most of the
    # field pinned at one extreme or the other with a fast crossing between, and
    # a fast crossing between two greens is indistinguishable from a shadow edge.
    #
    # The whole point of this axis is that it should be impossible to point at
    # where it changes. So the range is now wider than the noise ever reaches,
    # which costs some of the tint's authority and buys a transition with no
    # edge anywhere in it.
    spread = nodes.new("ShaderNodeMapRange")
    spread.location = (-1120, 320)
    spread.inputs["From Min"].default_value = 0.06
    spread.inputs["From Max"].default_value = 0.94
    spread.clamp = True

    links.new(coordinate.outputs["Object"], drift.inputs["Vector"])
    links.new(drift.outputs["Fac"], spread.inputs["Value"])

    # Tints rather than colours — multiplied, so they ride on the value ramp
    # instead of replacing it. Two of them, so the axis runs both ways from the
    # base green rather than only toward olive.
    warm = nodes.new("ShaderNodeMix")
    warm.data_type = "RGBA"
    warm.blend_type = "MULTIPLY"
    warm.location = (-330, 150)
    live(warm.inputs, "Factor").default_value = 1.0
    live(warm.inputs, "B").default_value = (1.95, 0.99, 0.88, 1.0)

    cool = nodes.new("ShaderNodeMix")
    cool.data_type = "RGBA"
    cool.blend_type = "MULTIPLY"
    cool.location = (-330, -70)
    live(cool.inputs, "Factor").default_value = 1.0
    live(cool.inputs, "B").default_value = (0.62, 1.01, 1.70, 1.0)

    graded = nodes.new("ShaderNodeMix")
    graded.data_type = "RGBA"
    graded.location = (-170, 150)

    mix_out = live(mix.outputs, "Result")
    links.new(mix_out, live(warm.inputs, "A"))
    links.new(mix_out, live(cool.inputs, "A"))
    links.new(live(cool.outputs, "Result"), live(graded.inputs, "A"))
    links.new(live(warm.outputs, "Result"), live(graded.inputs, "B"))
    links.new(spread.outputs["Result"], live(graded.inputs, "Factor"))

    # ## Dry blades
    #
    # A small population of tan and bleached straw, keyed on the `tone` the
    # placement already assigns — `Tone::Dry` is 4, everything green is below it.
    # Real turf always carries some dead material, and its absence is one of the
    # quieter ways a generated field says it was generated: every blade the same
    # age, none of them finished.
    #
    # Kept to what the placement chose rather than sprinkled here, so the dry
    # blades sit where the field decided grass was struggling.
    tone = nodes.new("ShaderNodeAttribute")
    tone.attribute_name = "tone"
    tone.attribute_type = "GEOMETRY"
    tone.location = (-1100, -260)

    is_dry = nodes.new("ShaderNodeMapRange")
    is_dry.location = (-900, -260)
    is_dry.inputs["From Min"].default_value = 3.3
    is_dry.inputs["From Max"].default_value = 3.9
    is_dry.clamp = True

    straw = nodes.new("ShaderNodeValToRGB")
    straw.location = (-700, -400)
    straw.color_ramp.elements[0].color = (0.115, 0.088, 0.030, 1.0)
    straw.color_ramp.elements[1].color = (0.235, 0.190, 0.072, 1.0)

    withered = nodes.new("ShaderNodeMix")
    withered.data_type = "RGBA"
    withered.location = (-20, 150)

    links.new(tone.outputs["Fac"], is_dry.inputs["Value"])
    links.new(along.outputs["Result"], straw.inputs["Fac"])
    links.new(live(graded.outputs, "Result"), live(withered.inputs, "A"))
    links.new(live(straw.outputs, "Color"), live(withered.inputs, "B"))
    links.new(is_dry.outputs["Result"], live(withered.inputs, "Factor"))
    links.new(live(withered.outputs, "Result"), principled.inputs["Base Color"])

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
    principled.inputs["Subsurface Weight"].default_value = 0.78
    principled.inputs["Subsurface Scale"].default_value = 0.008
    principled.inputs["Subsurface Radius"].default_value = (0.004, 0.012, 0.002)
    # A blade is a sheet, not a solid; this is what stops Cycles treating it as
    # a volume with an inside.
    principled.inputs["Thin Wall"].default_value = True
    principled.inputs["Transmission Weight"].default_value = 0.40
    principled.inputs["Specular IOR Level"].default_value = 0.20
    principled.inputs["Sheen Weight"].default_value = 0.06
    principled.inputs["Sheen Roughness"].default_value = 0.55

    links.new(principled.outputs["BSDF"], output.inputs["Surface"])
    return material


def ground_material():
    """The soil between the clumps: warm olive-brown earth, procedurally grained.

    This is the only warm colour in the picture and it is doing more work than
    its area suggests. It separates one tuft from the next, makes a density
    change legible, and gives the eye somewhere to rest — a canopy with nothing
    at all between it reads as fur rather than as plants standing in ground.

    ## Four octaves, because one reads as a gradient

    A single noise ramped between two browns is what this was first, and at any
    real magnification it is obviously a smooth blend. Earth is not smooth at any
    scale, so the colour is built from bands that each answer a different
    distance:

    | Scale | What it is |
    | --- | --- |
    | ~2 m | damp and dry regions, the reason one clearing differs from another |
    | ~25 cm | scuffs and patches, the size of a footfall |
    | ~4 cm | grain and grit |
    | ~1 cm | a bump, not a colour — see below |

    The finest band drives **displacement of the normal** rather than colour.
    Grain that is only a colour stays flat under a moving sun, which is exactly
    when a surface announces it is a texture; grain that tilts the normal catches
    light on one side, and that is what makes soil look like soil.

    Everything here is a pure function of world position, so two renders of the
    same ground produce the same dirt — the same rule the placement lives under.
    """
    material = bpy.data.materials.new("ground")
    material.use_nodes = True
    tree = material.node_tree
    nodes, links = tree.nodes, tree.links
    nodes.clear()

    output = nodes.new("ShaderNodeOutputMaterial")
    principled = nodes.new("ShaderNodeBsdfPrincipled")
    principled.location = (-300, 0)

    coordinate = nodes.new("ShaderNodeTexCoord")
    coordinate.location = (-1800, 0)

    def noise(scale, detail, roughness, y):
        node = nodes.new("ShaderNodeTexNoise")
        node.location = (-1600, y)
        node.inputs["Scale"].default_value = scale
        node.inputs["Detail"].default_value = detail
        node.inputs["Roughness"].default_value = roughness
        links.new(coordinate.outputs["Object"], node.inputs["Vector"])
        return node

    region = noise(0.5, 2.0, 0.5, 320)
    patch = noise(4.0, 3.0, 0.55, 140)
    grain = noise(26.0, 4.0, 0.6, -40)
    grit = noise(90.0, 3.0, 0.5, -220)

    # ## How dark the soil has to be, and why it is not a taste question
    #
    # These were three times brighter for one render, and the result was not
    # "pale soil" — it was a **rock**. A broad patch of bare earth sitting on a
    # terrain mound, bright enough to hold its own against the canopy, stops
    # reading as ground seen between plants and starts reading as an *object*
    # lying on top of them. Nothing about the shader said boulder; the value did.
    #
    # So both ramps stay well under the grass they sit between. The soil is
    # allowed to be warm, grained and varied — it is not allowed to compete.
    # Anything that draws the eye at this scale should be a plant.
    damp = nodes.new("ShaderNodeValToRGB")
    damp.location = (-1350, 320)
    damp.color_ramp.elements[0].position = 0.05
    damp.color_ramp.elements[1].position = 0.95
    damp.color_ramp.elements[0].color = (0.0135, 0.0145, 0.0078, 1.0)
    damp.color_ramp.elements[1].color = (0.0310, 0.0275, 0.0145, 1.0)
    links.new(region.outputs["Fac"], damp.inputs["Fac"])

    dry = nodes.new("ShaderNodeValToRGB")
    dry.location = (-1350, 140)
    dry.color_ramp.elements[0].position = 0.05
    dry.color_ramp.elements[1].position = 0.95
    dry.color_ramp.elements[0].color = (0.0240, 0.0205, 0.0110, 1.0)
    dry.color_ramp.elements[1].color = (0.0385, 0.0310, 0.0150, 1.0)
    links.new(patch.outputs["Fac"], dry.inputs["Fac"])

    # Which of the two, decided at the *region* scale and eased.
    #
    # Driven by `region` rather than `patch`, because `patch` carries three
    # octaves and using it as a blend factor puts a visible boundary wherever
    # damp meets dry — earth does not change moisture over four centimetres. The
    # remap narrows the swing further so neither end is ever fully reached, which
    # is what keeps the transition from reading as a stain.
    moisture = nodes.new("ShaderNodeMapRange")
    moisture.location = (-1200, 220)
    moisture.inputs["From Min"].default_value = 0.20
    moisture.inputs["From Max"].default_value = 0.80
    moisture.inputs["To Min"].default_value = 0.15
    moisture.inputs["To Max"].default_value = 0.85
    moisture.clamp = True
    links.new(region.outputs["Fac"], moisture.inputs["Value"])

    earth = nodes.new("ShaderNodeMix")
    earth.data_type = "RGBA"
    earth.location = (-1050, 220)
    links.new(live(damp.outputs, "Color"), live(earth.inputs, "A"))
    links.new(live(dry.outputs, "Color"), live(earth.inputs, "B"))
    links.new(moisture.outputs["Result"], live(earth.inputs, "Factor"))

    # Grain: a darkening, not a second colour. Multiplying keeps the hue the two
    # ramps already agreed on and only varies how much light comes back.
    grained = nodes.new("ShaderNodeMix")
    grained.data_type = "RGBA"
    grained.blend_type = "MULTIPLY"
    grained.location = (-800, 220)
    live(grained.inputs, "Factor").default_value = 0.55
    links.new(live(earth.outputs, "Result"), live(grained.inputs, "A"))
    links.new(live(grain.outputs, "Color"), live(grained.inputs, "B"))
    links.new(live(grained.outputs, "Result"), principled.inputs["Base Color"])

    # The bump. Two scales, because soil has both clods and grit, and a single
    # frequency reads as sandpaper.
    clods = nodes.new("ShaderNodeBump")
    clods.location = (-620, -180)
    clods.inputs["Strength"].default_value = 0.85
    clods.inputs["Distance"].default_value = 0.022
    links.new(grain.outputs["Fac"], clods.inputs["Height"])

    fine = nodes.new("ShaderNodeBump")
    fine.location = (-450, -180)
    fine.inputs["Strength"].default_value = 0.55
    fine.inputs["Distance"].default_value = 0.008
    links.new(grit.outputs["Fac"], fine.inputs["Height"])
    links.new(clods.outputs["Normal"], fine.inputs["Normal"])
    links.new(fine.outputs["Normal"], principled.inputs["Normal"])

    # Earth is matte and not remotely specular. A little variation in how matte
    # keeps damp patches from looking identical to dry ones under the same sun.
    rough = nodes.new("ShaderNodeMapRange")
    rough.location = (-620, -420)
    rough.inputs["To Min"].default_value = 0.78
    rough.inputs["To Max"].default_value = 0.98
    links.new(patch.outputs["Fac"], rough.inputs["Value"])
    links.new(rough.outputs["Result"], principled.inputs["Roughness"])

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
    #
    # The lower stop is the only light a sealed canopy recess ever sees, so it
    # sets where the darkest fifth of the plate lands. Too low and the image
    # jumps from near-black to bright with no medium values between — measured,
    # a fifth-percentile L* of 3.6 against reference art's 12.4, which is what
    # makes a gap read as a hole rather than as depth.
    ramp.color_ramp.elements[0].color = (0.115, 0.130, 0.080, 1.0)
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
        print("[blender_cycles] device: CPU (requested)")
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
            print(f"[blender_cycles] device: {backend} — {[d.name for d in usable]}")
            return
    except Exception as error:  # noqa: BLE001
        print(f"[blender_cycles] GPU setup failed ({error}); falling back to CPU")
    cycles.device = "CPU"
    print("[blender_cycles] device: CPU (no GPU backend available)")


def enable_passes(scene):
    """The AOVs the neural renderer is trained against.

    Configuration rather than plumbing. The renderer this replaces carried ten
    channels by hand through its own resolve loop, and each one was a chance for
    the recorded value to drift from the value actually used.

    ## Why every assignment is guarded

    Blender renames and retires pass flags between releases, and an assignment to
    a flag that no longer exists raises rather than being ignored. That turned a
    cosmetic API change into a *total* render failure: `use_pass_shadow_catcher`
    went away in 5.x, and the only symptom was `--aovs` producing no image at all
    — not a missing channel, no picture. The beauty pass had nothing to do with
    the flag that failed.

    So a pass that this Blender does not have is skipped and named. A missing
    channel is a real problem and worth saying out loud; it is not a reason to
    throw away the render that was going to carry the other nine.
    """
    layer = scene.view_layers[0]
    wanted = {
        "use_pass_combined": True,
        "use_pass_z": True,
        "use_pass_normal": True,
        "use_pass_diffuse_direct": True,
        "use_pass_diffuse_indirect": True,
        "use_pass_diffuse_color": True,
        "use_pass_glossy_direct": True,
        "use_pass_transmission_direct": True,
        "use_pass_ambient_occlusion": True,
        "use_pass_shadow_catcher": False,
        "use_pass_cryptomatte_object": True,
    }
    missing = []
    for flag, value in wanted.items():
        if not hasattr(layer, flag):
            # Only worth reporting for a pass that was asked for. A flag being
            # turned off is satisfied by its own absence.
            if value:
                missing.append(flag)
            continue
        setattr(layer, flag, value)
    if missing:
        print(f"[blender_cycles] this Blender has no {', '.join(missing)} — skipped")
    layer.cycles.denoising_store_passes = True


def render_one(header_path, output):
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
        f"[blender_cycles] built {spec['blades']['count']} curves "
        f"({spec['ground']['rows']}x{spec['ground']['columns']} ground) in {built - started:.1f}s"
    )
    if blades is None:
        print("[blender_cycles] warning: no blades in this scene")
    _ = ground

    bpy.ops.render.render(write_still=True)
    print(f"[blender_cycles] rendered in {time.time() - built:.1f}s -> {output}")


def main():
    jobs = jobs_from_argv()
    if len(jobs) > 1:
        print(f"[blender_cycles] manifest of {len(jobs)} pages in one process")
    started = time.time()
    for index, (header_path, output) in enumerate(jobs):
        try:
            render_one(header_path, output)
        except Exception as error:  # noqa: BLE001
            # One bad page must not cost the whole manifest. The driver checks
            # for the file, so a page that fails here simply is not cached.
            print(f"[blender_cycles] page {index} failed: {error}")
    if len(jobs) > 1:
        elapsed = time.time() - started
        print(
            f"[blender_cycles] {len(jobs)} pages in {elapsed:.0f}s "
            f"({elapsed / max(len(jobs), 1):.1f}s each)"
        )


if __name__ == "__main__":
    main()

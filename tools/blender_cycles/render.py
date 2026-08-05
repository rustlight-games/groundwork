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
    — see `terrain_cycles::export::VERTICES_PER_SEGMENT`. A `RIBBONS` curve is a
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
        return []

    per_blade = ribs * across
    raw = np.fromfile(os.path.join(scene_dir, spec["path"]), dtype=np.float32)
    expected = count * per_blade * 3
    if raw.size != expected:
        raise SystemExit(f"blades.bin has {raw.size} floats, expected {expected}")
    attributes = np.fromfile(
        os.path.join(scene_dir, spec["attributes"]), dtype=np.float32
    ).reshape(count, spec["attributes_per_blade"])

    # The buffer is sorted: the blades rooted inside the ground being rendered
    # come first, and the halo follows. The halo is built as a second object,
    # hidden from the camera and left casting shadows — dropping it instead would
    # take its shadows with it and leave a bright rim exactly at the edge of the
    # picture, which is where the eye goes.
    visible = spec.get("visible", count)
    spans = [("grass", 0, visible), ("grass-halo", visible, count)]
    material = blade_material(settings)
    objects = []
    for name, first, last in spans:
        if last <= first:
            continue
        obj = build_blade_span(
            name,
            raw[first * per_blade * 3 : last * per_blade * 3],
            attributes[first:last],
            last - first,
            ribs,
            across,
            material,
        )
        if name == "grass-halo":
            obj.visible_camera = False
            obj.visible_shadow = True
        objects.append(obj)
    return objects


def build_blade_span(name, raw, attributes, count, ribs, across, material):
    """One mesh from a run of the ribbon buffer."""
    per_blade = ribs * across
    mesh = bpy.data.meshes.new(name)
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

    # Per-blade attributes reach the shader through Attribute nodes by name, so
    # the material can vary without the geometry changing. Repeated to the face
    # domain because that is the coarsest domain every vertex of a blade shares.
    per_face = faces.shape[0] // count
    for index, channel in enumerate(("maturity", "moisture", "tone", "exposure")):
        layer = mesh.attributes.new(name=channel, type="FLOAT", domain="FACE")
        layer.data.foreach_set("value", np.repeat(attributes[:, index], per_face))

    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    obj.data.materials.append(material)
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

    # What each vertex is made of, and what state it is in, carried as mesh
    # attributes so one material can shade every soil in the scene.
    #
    # One material with attributes rather than one material per soil with
    # boundaries between them, because the boundary is a *blend*: the compiler
    # already decided the ragged edge, and splitting the mesh at it would
    # quantise that edge to the triangle grid and undo the work.
    def attribute(name, path):
        values = np.fromfile(os.path.join(scene_dir, path), dtype=np.float32)
        if values.size != rows * columns:
            raise SystemExit(
                f"{path} has {values.size} floats, expected {rows * columns}"
            )
        layer = mesh.attributes.new(name=name, type="FLOAT", domain="POINT")
        layer.data.foreach_set("value", values.tolist())

    materials = spec.get("materials", [])
    for index, entry in enumerate(materials):
        attribute(f"w{index}", entry["weights"])
    for name, path in spec.get("state", {}).items():
        attribute(name, path)

    obj = bpy.data.objects.new("ground", mesh)
    bpy.context.collection.objects.link(obj)
    obj.data.materials.append(ground_material(materials))
    if materials:
        names = ", ".join(entry["key"] for entry in materials)
        print(
            f"[terrain_cycles] ground: {rows}x{columns} at "
            f"{spec.get('spacing_m', 0.0) * 100:.2f} cm — {names}"
        )
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
        # Every ground material shares one implementation. What a particular
        # soil looks like is its *profile*, not its appearance key — see
        # `assets/terrain/materials/`.
        "surface.ground": lambda: ground_material(None),
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
        builder = lambda: ground_material(None)

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


# How far a full hollow pulls a soil's tone down its own palette.
#
# Half, near enough. A photograph of bare earth beside grass spans twenty times
# between its darkest fifth and its brightest, and no palette that also has to
# hold a plausible median covers that on pigment alone — the rest is occlusion,
# which is what this is.
#
# It was pushed to 0.85 chasing the rest of that range, and the range did not
# move: the render's dark end is lifted four times by ambient fill and a filmic
# toe, and no material control reaches that. What the extra did reach was the
# shape skew's narrow crests, which came back as bright dots on dark. Half puts
# the shadow in the hollows without turning the crumbs into speckle.
CAVITY_TONE = 0.55

# The same again for the bands too fine to reach the mesh, as a multiplier on
# the colour rather than a slide along the palette. Grain-scale shadow darkens
# without changing which soil you are looking at.
CAVITY_TONE_FINE = 0.70


def ground_material(materials=None):
    """The ground, built from the soils the scene actually carries.

    ## One graph per soil, blended by realised weight

    This used to be a single hand-built graph with two brown ramps in it, and it
    could only ever draw one soil. Everything exposed anywhere in the world was
    the same colour, because the only thing the exporter could say about a point
    was `earth`: one minus however much of it was grass. That is not a material
    identity — it cannot tell loam from sand from clay — so the shader had
    nothing to branch on even if it had wanted to.

    Now the exporter writes a **weight plane per soil** and a table describing
    each one, and this builds a branch per entry and mixes them:

        C = sum_i w_i C_i        colour
        R = sum_i w_i R_i        roughness
        H = sum_i w_i H_i        micro-height, into *one* Bump node

    The single Bump at the end is the part worth stating. Blending two
    already-perturbed normals gives a normal that is not unit length and points
    somewhere neither soil does; blending the heights and perturbing once gives
    the surface a boundary would actually have.

    The weights are the *realised* ones — the same ragged numbers the transition
    solver handed the grass — so the colour changes exactly where the vegetation
    does rather than a centimetre away from it.

    ## Nothing here is a literal any more

    Every colour, roughness, wavelength and amplitude comes from the manifest,
    which got them from a `.ground.ron` profile. The numbers were measured off
    reference plates and they live in a file an author can open, rather than in
    this function where nothing in Rust could read or test them.
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
    coordinate.location = (-2600, 0)

    if not materials:
        # A scene with no ground profiles at all: the laboratory meadow, which
        # has no document and therefore no soils. A plain matte surface, so the
        # sun still has something to land on behind the grass.
        principled.inputs["Base Color"].default_value = (0.024, 0.021, 0.011, 1.0)
        principled.inputs["Roughness"].default_value = 0.9
        links.new(principled.outputs["BSDF"], output.inputs["Surface"])
        return material

    moisture = nodes.new("ShaderNodeAttribute")
    moisture.location = (-2600, -600)
    moisture.attribute_name = "moisture"

    # How deep in its own relief each point sits. The channel that puts the dark
    # where the ground is low — see the docstring.
    cavity = nodes.new("ShaderNodeAttribute")
    cavity.location = (-2600, -750)
    cavity.attribute_name = "cavity"

    colour_sum = None
    rough_sum = None
    height_sum = None

    for index, entry in enumerate(materials):
        y = 900 - index * 900
        weight = nodes.new("ShaderNodeAttribute")
        weight.location = (-2600, y)
        weight.attribute_name = f"w{index}"

        colour, roughness, height = soil_branch(
            nodes, links, coordinate, moisture, cavity, entry, y
        )

        colour_sum = accumulate_colour(nodes, links, colour_sum, colour, weight, y)
        rough_sum = accumulate_float(nodes, links, rough_sum, roughness, weight, y - 200)
        height_sum = accumulate_float(nodes, links, height_sum, height, weight, y - 400)

    links.new(colour_sum, principled.inputs["Base Color"])
    links.new(rough_sum, principled.inputs["Roughness"])

    # One Bump for the whole surface. See the docstring: blending perturbed
    # normals is not the same operation and does not give a usable one.
    bump = nodes.new("ShaderNodeBump")
    bump.location = (-500, -300)
    bump.inputs["Strength"].default_value = 1.0
    # The heights are already in metres, so the distance is unity and the
    # amplitudes in the profiles mean what they say.
    bump.inputs["Distance"].default_value = 1.0
    links.new(height_sum, bump.inputs["Height"])
    links.new(bump.outputs["Normal"], principled.inputs["Normal"])

    links.new(principled.outputs["BSDF"], output.inputs["Surface"])
    return material


def soil_branch(nodes, links, coordinate, moisture, cavity, entry, y):
    """One soil's colour, roughness and micro-height.

    Returns three sockets. Nothing is connected to the output here — the caller
    blends every soil's three by weight and connects once.
    """
    optics = entry["colour_fields"]

    def noise(wavelength_m, detail, offset):
        node = nodes.new("ShaderNodeTexNoise")
        node.location = (-2400, y + offset)
        # Blender's Scale is cycles per unit and a unit is a metre here, so a
        # wavelength in metres is its reciprocal. Stated because getting this
        # backwards produces a plausible-looking surface at the wrong scale,
        # which is the hardest kind of mistake to see.
        node.inputs["Scale"].default_value = 1.0 / max(wavelength_m, 1e-5)
        node.inputs["Detail"].default_value = detail
        node.inputs["Roughness"].default_value = 0.55
        links.new(coordinate.outputs["Object"], node.inputs["Vector"])
        return node

    region = noise(optics["region_wavelength_m"], 2.0, 300)
    patch = noise(optics["patch_wavelength_m"], 3.0, 150)

    # Where this point sits on the soil's tonal range, 0..1. Two bands, because
    # one reads as a gradient at any real magnification: the broad one says which
    # clearing this is, the fine one says which scuff.
    tone = nodes.new("ShaderNodeMath")
    tone.operation = "MULTIPLY_ADD"
    tone.location = (-2150, y + 300)
    links.new(region.outputs["Fac"], tone.inputs[0])
    tone.inputs[1].default_value = optics["region_strength"]
    tone.inputs[2].default_value = 0.5 * (1.0 - optics["region_strength"])

    tone2 = nodes.new("ShaderNodeMath")
    tone2.operation = "MULTIPLY_ADD"
    tone2.location = (-2000, y + 300)
    links.new(patch.outputs["Fac"], tone2.inputs[0])
    tone2.inputs[1].default_value = optics["patch_strength"]
    links.new(tone.outputs["Value"], tone2.inputs[2])

    # ## Where the dark comes from
    #
    # Not from the noise above. A crevice in bare earth reads twenty times
    # darker than the crest beside it, and that is **occlusion**, not pigment —
    # which is why the deepest fifth of a photograph of soil is nearly neutral
    # in hue while its median is a warm brown. Sky lights a hole; sun lights a
    # crest.
    #
    # So the mesh-scale hollows pull the tone down before any noise touches it.
    # Without this the shading and the form disagree about where the low ground
    # is, and the surface reads as painted paper however much relief the mesh
    # carries. That was the whole failure of the first soil card.
    hollow = nodes.new("ShaderNodeMath")
    hollow.operation = "MULTIPLY_ADD"
    hollow.location = (-1900, y + 220)
    links.new(cavity.outputs["Fac"], hollow.inputs[0])
    hollow.inputs[1].default_value = -CAVITY_TONE
    links.new(tone2.outputs["Value"], hollow.inputs[2])
    tone2 = hollow

    # Back to 0..1, and the *exact* bounds matter. Both noises run 0..1, so the
    # sum above runs from `floor` to `floor + region + patch`; getting the low
    # bound wrong biases every soil toward one end of its own palette, which
    # reads as the wrong material rather than as a mistuned one. This had an
    # extra `- patch_strength` in the low bound for one render and turned dark
    # loam into pale tan.
    #
    # The cavity term only widens the window *downward*. Shifting both ends by
    # it — which this did for one render — moves the whole surface up its own
    # palette instead, and a card of dark loam and grey hardpan came back as
    # pale sand.
    floor = 0.5 * (1.0 - optics["region_strength"])
    tone_norm = nodes.new("ShaderNodeMapRange")
    tone_norm.location = (-1850, y + 300)
    tone_norm.inputs["From Min"].default_value = floor - CAVITY_TONE
    tone_norm.inputs["From Max"].default_value = (
        floor + optics["region_strength"] + optics["patch_strength"]
    )
    tone_norm.clamp = True
    links.new(tone2.outputs["Value"], tone_norm.inputs["Value"])

    # The palette: three measured stops, interpolated through the middle one.
    # Three rather than two because real earth varies in hue as well as value —
    # its dry crests are warmer and less saturated than its damp hollows, and a
    # two-stop ramp can only interpolate a line between two colours.
    palette = nodes.new("ShaderNodeValToRGB")
    palette.location = (-1650, y + 300)
    ramp = palette.color_ramp
    ramp.elements[0].position = 0.0
    ramp.elements[0].color = tuple(entry["dry_palette"]["low"]) + (1.0,)
    ramp.elements[1].position = 1.0
    ramp.elements[1].color = tuple(entry["dry_palette"]["high"]) + (1.0,)
    middle = ramp.elements.new(0.5)
    middle.color = tuple(entry["dry_palette"]["mid"]) + (1.0,)
    links.new(tone_norm.outputs["Result"], palette.inputs["Fac"])

    # Grain: a darkening, not a second colour. Multiplying keeps the hue the
    # palette already chose and only varies how much light comes back.
    grain_band = entry["shader_bands"][0] if entry["shader_bands"] else None
    grain_wavelength = grain_band["wavelength_m"] if grain_band else 0.02
    grain = noise(grain_wavelength, 4.0, -50)
    grained = nodes.new("ShaderNodeMix")
    grained.data_type = "RGBA"
    grained.blend_type = "MULTIPLY"
    grained.location = (-1450, y + 300)
    live(grained.inputs, "Factor").default_value = optics["grain_strength"]
    links.new(live(palette.outputs, "Color"), live(grained.inputs, "A"))
    links.new(grain.outputs["Color"], live(grained.inputs, "B"))

    # ## Wet ground is not dry ground turned down
    #
    # Water fills the pores, so the air-soil boundary that scattered light
    # diffusely becomes a water-soil boundary: internal scattering falls,
    # absorption rises, and the outer surface becomes a smooth water-air
    # interface. Three things follow, and doing only the first is what makes wet
    # ground read as ground in shadow instead of as wet ground.
    #
    #   albedo      darkens toward its own square
    #   hue         warms — the film absorbs blue and green harder than red
    #   roughness   collapses, and does so before the darkening is noticeable
    #
    # The gain below is fitted in Rust so that the soil's *mid* stop lands
    # exactly on the wet colour its profile declares. The author writes two
    # colours they can measure; the square law is what runs between them.
    #
    # No subsurface scattering. Production mud shaders do not use it — mud is a
    # rough dark dielectric under a glossy coat.
    mid = entry["dry_palette"]["mid"]
    wet_mid = entry["wet"]["mid"]
    gain = tuple(
        (wet_mid[c] / (mid[c] * mid[c])) if mid[c] > 0.0 else 0.0 for c in range(3)
    )

    squared = nodes.new("ShaderNodeMix")
    squared.data_type = "RGBA"
    squared.blend_type = "MULTIPLY"
    squared.location = (-1250, y + 200)
    live(squared.inputs, "Factor").default_value = 1.0
    links.new(live(grained.outputs, "Result"), live(squared.inputs, "A"))
    links.new(live(grained.outputs, "Result"), live(squared.inputs, "B"))

    lifted = nodes.new("ShaderNodeMix")
    lifted.data_type = "RGBA"
    lifted.blend_type = "MULTIPLY"
    lifted.location = (-1100, y + 200)
    live(lifted.inputs, "Factor").default_value = 1.0
    links.new(live(squared.outputs, "Result"), live(lifted.inputs, "A"))
    live(lifted.inputs, "B").default_value = gain + (1.0,)

    wetted = nodes.new("ShaderNodeMix")
    wetted.data_type = "RGBA"
    wetted.location = (-950, y + 300)
    links.new(live(grained.outputs, "Result"), live(wetted.inputs, "A"))
    links.new(live(lifted.outputs, "Result"), live(wetted.inputs, "B"))
    links.new(moisture.outputs["Fac"], live(wetted.inputs, "Factor"))

    # Roughness across the dry range, collapsing toward the wet value.
    dry_low, dry_high = entry["roughness_dry"]
    rough = nodes.new("ShaderNodeMapRange")
    rough.location = (-1250, y - 150)
    rough.inputs["To Min"].default_value = dry_low
    rough.inputs["To Max"].default_value = dry_high
    links.new(tone_norm.outputs["Result"], rough.inputs["Value"])

    # Relief finer than a pixel is not a bump, it is a BRDF. Bands below the
    # sampling rate were folded into one roughness figure in Rust; adding them
    # here is what stops a millimetre grain arriving as speckle.
    micro = nodes.new("ShaderNodeMath")
    micro.operation = "ADD"
    micro.location = (-1100, y - 150)
    links.new(rough.outputs["Result"], micro.inputs[0])
    micro.inputs[1].default_value = entry.get("micro_roughness", 0.0)
    micro.use_clamp = True

    wet_rough = nodes.new("ShaderNodeMix")
    wet_rough.data_type = "FLOAT"
    wet_rough.location = (-950, y - 150)
    links.new(micro.outputs["Value"], live(wet_rough.inputs, "A"))
    live(wet_rough.inputs, "B").default_value = entry["wet"]["roughness"]
    links.new(moisture.outputs["Fac"], live(wet_rough.inputs, "Factor"))

    height = band_height(nodes, links, coordinate, moisture, entry, y)

    # The bands the mesh could not carry make hollows too, at grain scale, and
    # they are already summed for the Bump node. Reusing that sum rather than
    # sampling a fresh field is what keeps the fine shading registered with the
    # fine form — the same argument as the mesh-scale cavity, one tier down.
    if entry["shader_bands"]:
        reach = sum(b["amplitude_m"] for b in entry["shader_bands"]) or 1.0
        fine = nodes.new("ShaderNodeMath")
        fine.operation = "MULTIPLY_ADD"
        fine.location = (-1750, y + 160)
        links.new(height, fine.inputs[0])
        fine.inputs[1].default_value = CAVITY_TONE_FINE / reach
        fine.inputs[2].default_value = 0.0

        shaded = nodes.new("ShaderNodeMix")
        shaded.data_type = "RGBA"
        shaded.blend_type = "MULTIPLY"
        shaded.location = (-820, y + 300)
        live(shaded.inputs, "Factor").default_value = 1.0
        links.new(live(wetted.outputs, "Result"), live(shaded.inputs, "A"))

        lift = nodes.new("ShaderNodeMath")
        lift.operation = "ADD"
        lift.location = (-1600, y + 160)
        links.new(fine.outputs["Value"], lift.inputs[0])
        lift.inputs[1].default_value = 1.0
        lift.use_clamp = True

        grey = nodes.new("ShaderNodeCombineColor")
        grey.location = (-1450, y + 160)
        for channel in ("Red", "Green", "Blue"):
            links.new(lift.outputs["Value"], grey.inputs[channel])
        links.new(grey.outputs["Color"], live(shaded.inputs, "B"))
        colour_out = live(shaded.outputs, "Result")
    else:
        colour_out = live(wetted.outputs, "Result")

    return (
        colour_out,
        live(wet_rough.outputs, "Result"),
        height,
    )


def band_height(nodes, links, coordinate, moisture, entry, y):
    """The relief bands the mesh could not carry, summed, in metres.

    Which bands these are was decided in Rust by the lattice spacing, not here
    and not by the profile. A band the mesh resolves is displaced geometry; a
    band below that is this. Each band is drawn exactly once by whichever half
    can actually draw it — the alternative, a fixed rule about which scales are
    "bump", double-counts a band whenever the sampling rate changes.
    """
    total = None
    for slot, band in enumerate(entry["shader_bands"]):
        if band["amplitude_m"] <= 0.0:
            continue
        noise = nodes.new("ShaderNodeTexNoise")
        noise.location = (-2400, y - 600 - slot * 200)
        noise.inputs["Scale"].default_value = 1.0 / max(band["wavelength_m"], 1e-5)
        noise.inputs["Detail"].default_value = 2.0
        noise.inputs["Roughness"].default_value = 0.5
        links.new(coordinate.outputs["Object"], noise.inputs["Vector"])

        centred = nodes.new("ShaderNodeMath")
        centred.operation = "SUBTRACT"
        centred.location = (-2200, y - 600 - slot * 200)
        links.new(noise.outputs["Fac"], centred.inputs[0])
        centred.inputs[1].default_value = 0.5

        shaped = ridge(nodes, links, centred, band["ridge"], y - 600 - slot * 200)

        # Water fills the smallest cavities first, so a wet surface loses its
        # grain long before it loses its clods.
        damped = nodes.new("ShaderNodeMix")
        damped.data_type = "FLOAT"
        damped.location = (-1800, y - 600 - slot * 200)
        links.new(shaped, live(damped.inputs, "A"))
        live(damped.inputs, "B").default_value = 0.0
        flatten = entry["wet"]["flattening"]
        soak = nodes.new("ShaderNodeMath")
        soak.operation = "MULTIPLY"
        soak.location = (-1950, y - 700 - slot * 200)
        links.new(moisture.outputs["Fac"], soak.inputs[0])
        soak.inputs[1].default_value = flatten
        links.new(soak.outputs["Value"], live(damped.inputs, "Factor"))

        scaled = nodes.new("ShaderNodeMath")
        scaled.operation = "MULTIPLY"
        scaled.location = (-1650, y - 600 - slot * 200)
        links.new(live(damped.outputs, "Result"), scaled.inputs[0])
        scaled.inputs[1].default_value = band["amplitude_m"]

        if total is None:
            total = scaled.outputs["Value"]
        else:
            add = nodes.new("ShaderNodeMath")
            add.operation = "ADD"
            add.location = (-1500, y - 600 - slot * 200)
            links.new(total, add.inputs[0])
            links.new(scaled.outputs["Value"], add.inputs[1])
            total = add.outputs["Value"]

    if total is None:
        zero = nodes.new("ShaderNodeValue")
        zero.location = (-1500, y - 600)
        zero.outputs[0].default_value = 0.0
        total = zero.outputs[0]
    return total


def ridge(nodes, links, centred, amount, y):
    """Fold a centred noise band toward a ridge, keeping its mean at zero.

    `1 - 2|c|` is the fold; squaring it creases the crest and flattens the
    trough, which is what separates a soil from gravel. The offset is one third
    — the mean of the squared fold — and not one half: subtracting a half would
    leave the band averaging -1/12, so an author raising a soil's ridge factor
    would sink the ground under it.
    """
    if amount <= 0.0:
        return centred.outputs["Value"]

    absolute = nodes.new("ShaderNodeMath")
    absolute.operation = "ABSOLUTE"
    absolute.location = (-2100, y)
    links.new(centred.outputs["Value"], absolute.inputs[0])

    folded = nodes.new("ShaderNodeMath")
    folded.operation = "MULTIPLY_ADD"
    folded.location = (-2050, y)
    links.new(absolute.outputs["Value"], folded.inputs[0])
    folded.inputs[1].default_value = -2.0
    folded.inputs[2].default_value = 1.0

    squared = nodes.new("ShaderNodeMath")
    squared.operation = "MULTIPLY_ADD"
    squared.location = (-2000, y)
    links.new(folded.outputs["Value"], squared.inputs[0])
    links.new(folded.outputs["Value"], squared.inputs[1])
    squared.inputs[2].default_value = -1.0 / 3.0

    mixed = nodes.new("ShaderNodeMix")
    mixed.data_type = "FLOAT"
    mixed.location = (-1950, y)
    links.new(centred.outputs["Value"], live(mixed.inputs, "A"))
    links.new(squared.outputs["Value"], live(mixed.inputs, "B"))
    live(mixed.inputs, "Factor").default_value = amount
    return live(mixed.outputs, "Result")


def accumulate_colour(nodes, links, running, colour, weight, y):
    """`running + weight * colour`, as nodes. Returns the sum's socket."""
    # A colour multiplied by a scalar means a Mix in MULTIPLY against a grey of
    # that scalar, because Blender has no colour-times-float node.
    grey = nodes.new("ShaderNodeCombineColor")
    grey.location = (-950, y - 60)
    for channel in ("Red", "Green", "Blue"):
        links.new(weight.outputs["Fac"], grey.inputs[channel])

    scaled = nodes.new("ShaderNodeMix")
    scaled.data_type = "RGBA"
    scaled.blend_type = "MULTIPLY"
    scaled.location = (-800, y)
    live(scaled.inputs, "Factor").default_value = 1.0
    links.new(colour, live(scaled.inputs, "A"))
    links.new(grey.outputs["Color"], live(scaled.inputs, "B"))

    if running is None:
        return live(scaled.outputs, "Result")
    total = nodes.new("ShaderNodeMix")
    total.data_type = "RGBA"
    total.blend_type = "ADD"
    total.location = (-650, y)
    live(total.inputs, "Factor").default_value = 1.0
    links.new(running, live(total.inputs, "A"))
    links.new(live(scaled.outputs, "Result"), live(total.inputs, "B"))
    return live(total.outputs, "Result")


def accumulate_float(nodes, links, running, value, weight, y):
    """`running + weight * value`, as nodes. Returns the sum's socket."""
    scaled = nodes.new("ShaderNodeMath")
    scaled.operation = "MULTIPLY"
    scaled.location = (-800, y)
    links.new(value, scaled.inputs[0])
    links.new(weight.outputs["Fac"], scaled.inputs[1])
    if running is None:
        return scaled.outputs["Value"]
    total = nodes.new("ShaderNodeMath")
    total.operation = "ADD"
    total.location = (-650, y)
    links.new(running, total.inputs[0])
    links.new(scaled.outputs["Value"], total.inputs[1])
    return total.outputs["Value"]


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
    """The orthographic camera the scene package derived.

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
    scene.render.image_settings.color_depth = "8"

    # A nine-tile render is a diamond of ground inside a rectangular frame, so
    # the film has to be transparent and the plate has to carry the silhouette.
    # Always RGBA: the driver reads four channels either way, and one plate
    # format is one fewer thing for a caller to know about.
    scene.render.film_transparent = bool(render_spec.get("film_transparent"))
    scene.render.image_settings.color_mode = "RGBA"

    # The dimetric stretch. See `terrain_scene::projection`.
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
    if not blades:
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

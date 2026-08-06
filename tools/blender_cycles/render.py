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
import math
import os
import sys
import time

import bpy
import numpy as np
from mathutils import Matrix, Vector

# The scene-package format this reader understands.
#
# Kept in step with `terrain_cycles::secondary::CYCLES_SCENE_FORMAT_VERSION` by
# hand, and checked at load rather than assumed. The two halves of this pipeline
# are in different languages and cannot share a constant, so the version number
# is the only thing standing between a stale renderer and a picture that is
# quietly missing a section.
SCENE_FORMAT_VERSION = 3


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

    **Use this for every `ShaderNodeMix` and every `ShaderNodeMapRange` socket,
    without exception.** Both node types carry one socket set per data type and
    every set uses the same names, so raw `inputs[...]` returns whichever comes
    first — which is disabled whenever the node is set to anything but that
    type. Linking to a disabled socket is not an error and draws no warning, and
    setting its default is not an error either. The link simply has no effect.

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


def build_secondary(scene_dir, spec, settings):
    """Flowers, stones and undergrowth: everything the tuned generator does not grow.

    Blender's whole job here is transfer. Rust decided every position, every
    rotation, every prototype choice and every tint; this reads the tables and
    builds the objects. It draws no random values and makes no placement
    decisions — see the specification's rejected alternatives, and in particular
    why scattering in Python would break addressed world determinism and leave
    the conditioning metadata unable to name the objects actually rendered.

    Returns the objects it created, which is empty while the section is.
    """
    if not spec:
        return []

    objects = []
    materials = spec.get("materials", [])
    cache = {}

    curves = spec.get("curves", {})
    curve_spans = curves.get("spans", [])
    if curve_spans:
        points = read_floats(
            scene_dir,
            curves["path"],
            curves["point_count"] * 3,
            "secondary curve points",
        ).reshape(-1, 3)
        objects.extend(
            build_secondary_curves(curve_spans, points, materials, settings, cache)
        )

    ribbons = spec.get("ribbons", {})
    ribbon_spans = ribbons.get("spans", [])
    if ribbon_spans:
        stride = ribbons["vertex_stride"] // 4
        vertices = read_floats(
            scene_dir,
            ribbons["path"],
            ribbons["vertex_count"] * stride,
            "secondary ribbon vertices",
        ).reshape(-1, stride)
        indices = np.fromfile(
            os.path.join(scene_dir, ribbons["indices"]), dtype=np.uint32
        )
        if indices.size != ribbons["index_count"]:
            raise SystemExit(
                f"{ribbons['indices']} has {indices.size} indices, "
                f"expected {ribbons['index_count']}"
            )
        objects.extend(
            build_secondary_ribbons(
                ribbon_spans, vertices, indices, materials, settings, cache
            )
        )

    instances = spec.get("instances", {})
    if instances.get("count", 0):
        objects.extend(
            build_instances(
                scene_dir,
                instances,
                spec.get("prototypes", []),
                materials,
                settings,
                cache,
            )
        )

    if objects:
        print(f"[blender_cycles] secondary: {len(objects)} object(s)")
    return objects


def read_floats(scene_dir, name, expected, what):
    """Load a float table and check its length before anything reads it.

    Length-checked because the failure of a short read is not an exception. It
    is a reshape that happens to succeed at the wrong stride, and then a scene of
    geometry that is subtly wrong everywhere.
    """
    raw = np.fromfile(os.path.join(scene_dir, name), dtype=np.float32)
    if raw.size != expected:
        raise SystemExit(f"{name} has {raw.size} floats, expected {expected} ({what})")
    return raw


def apply_visibility(obj, visibility):
    """Camera-visible, or a shadow caster only.

    The same split the tuned blades already use. A halo object is dropped from
    camera rays and kept for every other kind, so a stone just outside the frame
    still darkens the grass inside it. Dropping it instead takes its shadow with
    it and leaves a bright rim exactly at the edge of the picture.
    """
    if visibility != "halo":
        return
    obj.visible_camera = False
    obj.visible_shadow = True
    obj.visible_diffuse = True
    obj.visible_glossy = True
    obj.visible_transmission = True


def secondary_material(index, materials, settings, cache):
    """The shader one secondary span asks for."""
    if index < 0 or index >= len(materials):
        raise SystemExit(
            f"a secondary span names material {index} of {len(materials)}"
        )
    binding = materials[index]
    return material_for(binding["appearance"], settings, cache)


def build_secondary_curves(spans, points, materials, settings, cache):
    """Stems, as bevelled Blender curves.

    Curves rather than transferred tubes, and this is the one place the format
    keeps a description instead of vertices. A stem *is* a centreline plus a
    radius: Blender bevels that more cheaply than a mesh upload, and the
    centreline is the exact geometry rather than an approximation of it.

    One object per material and visibility class, not one per stem. A thousand
    Blender objects costs more in scene synchronisation than the geometry costs
    to trace.
    """
    grouped = {}
    for span in spans:
        grouped.setdefault((span["material"], span["visibility"]), []).append(span)

    objects = []
    for (material_index, visibility), members in sorted(grouped.items()):
        data = bpy.data.curves.new(f"secondary-curves-{material_index}", "CURVE")
        data.dimensions = "3D"
        data.resolution_u = 3
        data.bevel_depth = 1.0
        data.bevel_resolution = 2
        for span in members:
            first = span["point_offset"]
            count = span["point_count"]
            spline = data.splines.new("POLY")
            spline.points.add(count - 1)
            block = points[first : first + count]
            flat = np.ones((count, 4), dtype=np.float32)
            flat[:, :3] = block
            spline.points.foreach_set("co", flat.ravel())
            # Radius is a multiplier on `bevel_depth`, which is why the depth
            # above is one: the per-point radius carries the real metres, and
            # the taper from root to tip comes out of the interpolation.
            radii = np.linspace(
                span["radius_root_m"], span["radius_tip_m"], count, dtype=np.float32
            )
            spline.points.foreach_set("radius", radii)
        obj = bpy.data.objects.new(f"secondary-curves-{material_index}", data)
        data.materials.append(secondary_material(material_index, materials, settings, cache))
        bpy.context.collection.objects.link(obj)
        apply_visibility(obj, visibility)
        objects.append(obj)
    return objects


def build_secondary_ribbons(spans, vertices, indices, materials, settings, cache):
    """Petals, leaves and undergrowth, already tessellated in Rust.

    Positions, normals and the two ribbon coordinates arrive as vertices. Python
    does not reinterpret plant morphology — see the module note in
    `terrain_cycles::secondary` for why the tessellation belongs on the Rust side
    of the boundary.
    """
    grouped = {}
    for span in spans:
        grouped.setdefault((span["material"], span["visibility"]), []).append(span)

    objects = []
    for (material_index, visibility), members in sorted(grouped.items()):
        positions = []
        normals = []
        along = []
        across = []
        tints = []
        variations = []
        triangles = []
        base = 0
        for span in members:
            first = span["vertex_offset"]
            count = span["vertex_count"]
            block = vertices[first : first + count]
            positions.append(block[:, 0:3])
            normals.append(block[:, 3:6])
            along.append(block[:, 6])
            across.append(block[:, 7])
            # The per-plant tint. Ribbons merge into one mesh per material, so
            # unlike an instance there is no Object Info to read it from — see
            # `terrain_cycles::secondary::RibbonVertex`.
            tints.append(block[:, 8:11])
            variations.append(block[:, 11])
            local = indices[
                span["index_offset"] : span["index_offset"] + span["index_count"]
            ]
            # Rebased, because each span indexes its own vertices from zero and
            # the merged mesh concatenates them.
            triangles.append(local.astype(np.int64) - first + base)
            base += count

        positions = np.concatenate(positions).ravel()
        normals = np.concatenate(normals)
        triangles = np.concatenate(triangles)
        mesh = bpy.data.meshes.new(f"secondary-ribbons-{material_index}")
        mesh.vertices.add(base)
        mesh.vertices.foreach_set("co", positions)
        faces = triangles.size // 3
        mesh.loops.add(triangles.size)
        mesh.loops.foreach_set("vertex_index", triangles)
        mesh.polygons.add(faces)
        mesh.polygons.foreach_set("loop_start", np.arange(faces, dtype=np.int32) * 3)
        mesh.polygons.foreach_set("loop_total", np.full(faces, 3, dtype=np.int32))
        mesh.update()
        mesh.validate()
        # Rust-authored normals rather than Blender's face normals: a petal is
        # a one-sided ribbon whose shading normal is the plant's, not the
        # triangle's.
        mesh.normals_split_custom_set_from_vertices(normals)

        ribbon_along = mesh.attributes.new("along", "FLOAT", "POINT")
        ribbon_along.data.foreach_set("value", np.concatenate(along))
        ribbon_across = mesh.attributes.new("across", "FLOAT", "POINT")
        ribbon_across.data.foreach_set("value", np.concatenate(across))
        ribbon_variation = mesh.attributes.new("variation", "FLOAT", "POINT")
        ribbon_variation.data.foreach_set("value", np.concatenate(variations))
        # As a colour rather than three floats, so the shader reads it with one
        # node. Alpha is one throughout: the tint is a multiplier on the base
        # colour and nothing here is transparent.
        tint = np.concatenate(tints)
        rgba = np.ones((tint.shape[0], 4), dtype=np.float32)
        rgba[:, 0:3] = tint
        ribbon_tint = mesh.color_attributes.new("tint", "FLOAT_COLOR", "POINT")
        ribbon_tint.data.foreach_set("color", rgba.ravel())

        mesh.materials.append(
            secondary_material(material_index, materials, settings, cache)
        )
        obj = bpy.data.objects.new(f"secondary-ribbons-{material_index}", mesh)
        bpy.context.collection.objects.link(obj)
        apply_visibility(obj, visibility)
        objects.append(obj)
    return objects


def build_instances(scene_dir, spec, prototypes, materials, settings, cache):
    """Prototype meshes, built once each, linked at explicit transforms.

    Linked duplicates share mesh data and keep independent transforms, which is
    the intended memory model for a few thousand stones drawn from six shapes.
    """
    count = spec["count"]
    stride = spec["stride"]
    raw = np.fromfile(os.path.join(scene_dir, spec["path"]), dtype=np.uint8)
    if raw.size != count * stride:
        raise SystemExit(
            f"{spec['path']} has {raw.size} bytes, expected {count * stride}"
        )
    records = raw.reshape(count, stride)
    header = records[:, :8].copy().view(np.uint32).reshape(count, 2)
    floats = records[:, 8:].copy().view(np.float32).reshape(count, 14)

    built = [
        build_prototype_mesh(prototype, index, materials, settings, cache)
        for index, prototype in enumerate(prototypes)
    ]

    objects = []
    for row in range(count):
        prototype_index = int(header[row, 0])
        if prototype_index >= len(built):
            raise SystemExit(
                f"instance {row} names prototype {prototype_index} of {len(built)}"
            )
        visibility = "halo" if (int(header[row, 1]) >> 16) & 0xFF else "camera"
        obj = bpy.data.objects.new(f"instance-{row}", built[prototype_index])
        obj.location = tuple(float(v) for v in floats[row, 0:3])
        obj.rotation_mode = "QUATERNION"
        x, y, z, w = (float(v) for v in floats[row, 3:7])
        obj.rotation_quaternion = (w, x, y, z)
        obj.scale = tuple(float(v) for v in floats[row, 7:10])
        # The per-instance tint, on the object rather than in the shader.
        #
        # Rust decided this colour — a petal's hue comes from the document and
        # its drift from the plant's own address — so the shader reads it back
        # through Object Info rather than inventing one from noise. That is what
        # keeps the image and the conditioning metadata describing the same
        # cause: a texture-driven hue is a colour nothing upstream can name.
        tint = tuple(float(v) for v in floats[row, 10:13])
        obj.color = (*tint, 1.0)
        bpy.context.collection.objects.link(obj)
        apply_visibility(obj, visibility)
        objects.append(obj)
    return objects


def build_prototype_mesh(spec, index, materials, settings, cache):
    """One prototype, built deterministically from its declared parameters."""
    family = spec["family"]
    if family == "superellipsoid":
        vertices, triangles = superellipsoid(
            spec["semi_axes_m"],
            spec["exponents"],
            spec["tessellation"],
            spec["deformation"],
            spec["clips"],
        )
    elif family == "disk":
        vertices, triangles = oblate_disk(spec["semi_axes_m"], spec["tessellation"])
    else:
        raise SystemExit(f"prototype `{spec['key']}` names unknown family `{family}`")

    mesh = bpy.data.meshes.new(spec["key"])
    mesh.from_pydata(vertices.tolist(), [], triangles.tolist())
    mesh.update()
    mesh.validate()
    mesh.materials.append(
        secondary_material(spec["material"], materials, settings, cache)
    )
    _ = index
    return mesh


def signed_power(values, exponent):
    """`sign(x) * |x| ** e`, finite at zero for every exponent."""
    return np.sign(values) * np.abs(values) ** exponent


def superellipsoid(semi_axes, exponents, tessellation, deformation, clips):
    """A superquadric surface, deformed and clipped by explicit parameters.

    Barr's parameterisation. The signed power is what lets one family span
    rounded, blocky, flattened and pinched silhouettes from two exponents, which
    is why a handful of prototypes can carry a field of stones without repeating
    visibly.
    """
    rings, segments = int(tessellation[0]), int(tessellation[1])
    eta = np.linspace(-np.pi / 2, np.pi / 2, rings)
    omega = np.linspace(-np.pi, np.pi, segments, endpoint=False)
    grid_eta, grid_omega = np.meshgrid(eta, omega, indexing="ij")

    e1, e2 = float(exponents[0]), float(exponents[1])
    cos_eta = signed_power(np.cos(grid_eta), e1)
    sin_eta = signed_power(np.sin(grid_eta), e1)
    x = cos_eta * signed_power(np.cos(grid_omega), e2)
    y = cos_eta * signed_power(np.sin(grid_omega), e2)
    z = sin_eta

    # Low-order radial deformation, over the whole object rather than as
    # high-frequency noise: a small stone displaced at high frequency becomes a
    # noisy potato, and the silhouette is what makes a stone recognisable.
    scale = np.ones_like(x)
    for amplitude, frequency, phase in deformation:
        scale += amplitude * np.sin(frequency * grid_omega + phase) * np.cos(grid_eta)
    x, y, z = x * scale, y * scale, z * scale

    x *= float(semi_axes[0])
    y *= float(semi_axes[1])
    z *= float(semi_axes[2])
    points = np.stack([x.ravel(), y.ravel(), z.ravel()], axis=1)

    # Clipping projects rather than deletes, so the topology is fixed and the
    # triangle list below does not depend on which vertices survived.
    for nx, ny, nz, d in clips:
        normal = np.array([nx, ny, nz], dtype=np.float64)
        distance = points @ normal - d
        outside = distance > 0.0
        points[outside] -= np.outer(distance[outside], normal)

    triangles = []
    for ring in range(rings - 1):
        for segment in range(segments):
            a = ring * segments + segment
            b = ring * segments + (segment + 1) % segments
            c = a + segments
            d = b + segments
            triangles.append((a, c, d))
            triangles.append((a, d, b))
    return points, np.array(triangles, dtype=np.int64)


def oblate_disk(semi_axes, tessellation):
    """A shallow bevelled disk: a flower receptacle."""
    rings, segments = int(tessellation[0]), int(tessellation[1])
    radii = np.linspace(0.0, 1.0, rings)
    omega = np.linspace(-np.pi, np.pi, segments, endpoint=False)
    grid_r, grid_o = np.meshgrid(radii, omega, indexing="ij")
    x = grid_r * np.cos(grid_o) * float(semi_axes[0])
    y = grid_r * np.sin(grid_o) * float(semi_axes[1])
    z = np.sqrt(np.maximum(0.0, 1.0 - grid_r**2)) * float(semi_axes[2])
    points = np.stack([x.ravel(), y.ravel(), z.ravel()], axis=1)

    triangles = []
    for ring in range(rings - 1):
        for segment in range(segments):
            a = ring * segments + segment
            b = ring * segments + (segment + 1) % segments
            triangles.append((a, a + segments, b + segments))
            triangles.append((a, b + segments, b))
    return points, np.array(triangles, dtype=np.int64)


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

    # ## Built through the buffer API, not from Python lists
    #
    # `from_pydata` wants lists, and `.tolist()` on the arrays above materialises
    # one Python float object per coordinate. That was free at the old lattice —
    # a few tens of thousands of vertices — and is not now: soils that ask for a
    # relief hierarchy pull the spacing down to about five millimetres, which is
    # a couple of million vertices and four million triangles per page. The list
    # conversion alone ran to minutes and most of a gigabyte.
    #
    # `foreach_set` writes from the numpy buffer directly. The loop-start /
    # loop-total pair is the same triangle fan every face here has, so it is
    # generated rather than stored.
    mesh = bpy.data.meshes.new("ground")
    mesh.vertices.add(vertices.shape[0])
    mesh.vertices.foreach_set("co", vertices.ravel())
    mesh.loops.add(faces.size)
    mesh.loops.foreach_set("vertex_index", faces.ravel())
    mesh.polygons.add(faces.shape[0])
    mesh.polygons.foreach_set("loop_start", np.arange(0, faces.size, 3, dtype=np.int32))
    mesh.polygons.foreach_set("loop_total", np.full(faces.shape[0], 3, dtype=np.int32))
    mesh.update(calc_edges=True)
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
        layer.data.foreach_set("value", values)

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


def appearance_builders(settings, soils=None):
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
        # Secondary content: flowers, stones, undergrowth. Each is a distinct
        # key even where two currently share an implementation, so that giving
        # petals their own shader later is an edit here rather than a rename
        # everywhere.
        "plant.flower_stem": lambda: stem_material(settings),
        "plant.flower_petal": lambda: petal_material(settings),
        "plant.flower_disk": lambda: disk_material(settings),
        "plant.undergrowth_leaf": lambda: leaf_material(settings),
        # Weathered granite, measured off a reference rather than guessed at.
        # The first value here was 0.055, which is darker than wet asphalt: the
        # stones rendered as holes in the grass rather than as objects sitting
        # in it. Dry silicate rock in daylight sits around a fifth, and reads
        # slightly cool against the warm soil beside it.
        "surface.stone": lambda: stone_material(settings, [0.180, 0.178, 0.172]),
        # The four silhouettes share one shader. They differ in *shape*, which
        # is the prototype's job; a fractured stone is not a different mineral
        # from a rounded one.
        "rock.rounded": lambda: stone_material(settings, [0.180, 0.178, 0.172]),
        "rock.fractured": lambda: stone_material(settings, [0.165, 0.162, 0.158]),
        "rock.flat": lambda: stone_material(settings, [0.195, 0.192, 0.184]),
        "rock.elongated": lambda: stone_material(settings, [0.172, 0.170, 0.166]),
        # Broken soil, at soil's own reflectance. The stone shader is four
        # times brighter and turns grit into a scatter of white eggs.
        "surface.soil_fragment": lambda: soil_fragment_material(settings, soils),
        "surface.shell_fragment": lambda: stone_material(settings, [0.42, 0.40, 0.35]),
        "surface.organic_fragment": lambda: stone_material(settings, [0.045, 0.033, 0.024]),
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

    builders = appearance_builders(settings, settings.get("soils"))
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


def stem_material(settings):
    """A flower stem: a rough green dielectric, slightly waxy.

    Simple on purpose. A stem is a couple of millimetres across at the target
    framing and its job in the image is to hold the head at the right height and
    catch a rim of light; a subsurface model here would cost trace time and be
    invisible.
    """
    _ = settings
    material = bpy.data.materials.new("flower-stem")
    material.use_nodes = True
    principled = material.node_tree.nodes["Principled BSDF"]
    principled.inputs["Base Color"].default_value = (0.055, 0.090, 0.030, 1.0)
    principled.inputs["Roughness"].default_value = 0.55
    if "Subsurface Weight" in principled.inputs:
        principled.inputs["Subsurface Weight"].default_value = 0.08
    return material


def petal_material(settings):
    """A petal: a thin two-sided sheet that light passes through.

    Transmission rather than a flat diffuse, for the same reason the blade
    material uses it — a petal is a translucent membrane, and a flower reads as a
    flower largely because its far petals glow with light that came through them.
    Restrained, because a petal that transmits too freely stops having a
    silhouette.

    ## The colour comes from the instance, not from here

    One material serves every flower in the scene and each instance carries its
    own tint, read back through Object Info. The alternative — a hue driven by a
    texture in this graph — would produce a perfectly good picture that nothing
    upstream could account for: the document could not say what colour its
    flowers were, and the conditioning metadata could not name the cause of what
    was rendered.
    """
    _ = settings
    material = bpy.data.materials.new("flower-petal")
    material.use_nodes = True
    tree = material.node_tree
    principled = tree.nodes["Principled BSDF"]
    principled.inputs["Roughness"].default_value = 0.42
    if "Subsurface Weight" in principled.inputs:
        principled.inputs["Subsurface Weight"].default_value = 0.35
        principled.inputs["Subsurface Radius"].default_value = (0.004, 0.004, 0.004)

    info = tree.nodes.new("ShaderNodeObjectInfo")
    info.location = (-600, 0)
    base = tree.nodes.new("ShaderNodeMixRGB")
    base.location = (-350, 0)
    base.blend_type = "MULTIPLY"
    base.inputs["Fac"].default_value = 1.0
    # A near-white membrane, tinted by the instance. Even a buttercup is mostly
    # white light with a strong cast, so the tint multiplies a pale base rather
    # than replacing it.
    base.inputs["Color1"].default_value = (0.78, 0.76, 0.72, 1.0)
    tree.links.new(info.outputs["Color"], base.inputs["Color2"])
    tree.links.new(base.outputs["Color"], principled.inputs["Base Color"])
    return material


def disk_material(settings):
    """A flower's central disk: warm, rough, and darker than its petals."""
    _ = settings
    material = bpy.data.materials.new("flower-disk")
    material.use_nodes = True
    principled = material.node_tree.nodes["Principled BSDF"]
    principled.inputs["Base Color"].default_value = (0.28, 0.19, 0.035, 1.0)
    principled.inputs["Roughness"].default_value = 0.72
    return material


def leaf_material(settings):
    """A broad ground leaf: a thin membrane, greener than a blade.

    ## Translucency rather than subsurface

    The first version asked for a subsurface weight, which is the right idea and
    the wrong node for this geometry. Subsurface scattering models light walking
    a distance *inside* a solid; the leaf here is a tessellated ribbon with no
    thickness at all, so there is no interior for the walk to happen in and the
    setting bought nothing but noise.

    What a membrane actually does is pass light straight through, and the node
    for that is a translucent BSDF mixed behind the surface one. It is also the
    single thing that most distinguishes a broad leaf from a blade at a glance:
    the leaves lying away from the sun glow, and the ones between the camera and
    the sun go bright and yellow-green while their neighbours stay dark.

    ## The tint comes from the mesh, not from the object

    Ribbons merge into one object per material, so Object Info would give every
    leaf in the plate the same colour — which is exactly the flatness the
    undergrowth was rebuilt to escape. The per-plant tint arrives as a colour
    attribute instead. See `terrain_cycles::secondary::RibbonVertex`.
    """
    _ = settings
    material = bpy.data.materials.new("undergrowth-leaf")
    # A sheet has two sides and both are the same leaf. Without this, Cycles
    # culls nothing but the shading normal points away on half the fold and the
    # leaf reads as if it had a hole in it.
    material.use_backface_culling = False
    material.use_nodes = True
    tree = material.node_tree
    output = tree.nodes["Material Output"]
    principled = tree.nodes["Principled BSDF"]
    principled.location = (-100, 200)
    # Waxier than soil and duller than a wet blade. A ground leaf carries a
    # broad, weak sheen rather than a highlight.
    principled.inputs["Roughness"].default_value = 0.42

    # The per-plant tint, from the mesh.
    tint = tree.nodes.new("ShaderNodeVertexColor")
    tint.layer_name = "tint"
    tint.location = (-900, 300)

    # Darker at the crown, and a touch warmer at the tip. Both are true of a
    # real rosette — the base sits in its own shadow and the tip is the newest,
    # thinnest tissue — and together they stop one leaf being one flat colour.
    along = tree.nodes.new("ShaderNodeAttribute")
    along.attribute_name = "along"
    along.location = (-900, 40)
    gradient = tree.nodes.new("ShaderNodeValToRGB")
    gradient.location = (-700, 40)
    gradient.color_ramp.elements[0].position = 0.0
    gradient.color_ramp.elements[0].color = (0.030, 0.062, 0.018, 1.0)
    gradient.color_ramp.elements[1].position = 1.0
    gradient.color_ramp.elements[1].color = (0.062, 0.122, 0.030, 1.0)
    tree.links.new(along.outputs["Fac"], gradient.inputs["Fac"])

    base = tree.nodes.new("ShaderNodeMixRGB")
    base.location = (-450, 160)
    base.blend_type = "MULTIPLY"
    base.inputs["Fac"].default_value = 1.0
    tree.links.new(gradient.outputs["Color"], base.inputs["Color1"])
    tree.links.new(tint.outputs["Color"], base.inputs["Color2"])
    tree.links.new(base.outputs["Color"], principled.inputs["Base Color"])

    # The light that came through. Brighter and yellower than the reflected
    # colour, because chlorophyll transmits in a narrower band than it reflects.
    through = tree.nodes.new("ShaderNodeMixRGB")
    through.location = (-450, -160)
    through.blend_type = "MULTIPLY"
    through.inputs["Fac"].default_value = 1.0
    through.inputs["Color1"].default_value = (0.155, 0.230, 0.045, 1.0)
    tree.links.new(tint.outputs["Color"], through.inputs["Color2"])

    translucent = tree.nodes.new("ShaderNodeBsdfTranslucent")
    translucent.location = (-100, -160)
    tree.links.new(through.outputs["Color"], translucent.inputs["Color"])

    mix = tree.nodes.new("ShaderNodeMixShader")
    mix.location = (150, 0)
    # A third, which is a membrane rather than a pane of glass. Higher than this
    # and the leaves stop casting a shadow, and the shadow under a rosette is
    # most of what tells the eye it is a separate object sitting on the ground.
    mix.inputs["Fac"].default_value = 0.34
    tree.links.new(principled.outputs["BSDF"], mix.inputs[1])
    tree.links.new(translucent.outputs["BSDF"], mix.inputs[2])
    tree.links.new(mix.outputs["Shader"], output.inputs["Surface"])
    return material


def soil_fragment_material(settings, soils=None):
    """A lump of the ground, not a pebble of granite.

    Its own builder rather than `stone_material` at a darker colour, because the
    colour was never the whole difference. A stone is a dense silicate with a
    weathered but continuous surface; a soil fragment is an aggregate of grains
    with air between them, so it scatters as roughly as the ground it broke off
    and has no specular shoulder at all.

    Left at the stone's 0.78 it caught the sun on every fragment, and a track
    carrying ninety of them a square metre came back covered in white glints —
    the surface read as wet concrete rather than as earth. Soil's own dry
    roughness runs 0.82 to 0.96; this sits near the top of that, because a loose
    fragment is rougher than the packed surface around it.
    """
    # ## The colour comes from the soil, because that is what it is
    #
    # This was the literal `[0.043, 0.035, 0.024]`, chosen once against a
    # palette that has since been recalibrated twice. Measured against the
    # current loam it was **1.2x brighter in red, 1.6x in green and 2.1x in
    # blue** — so every fragment was a pale grey chip lying on warm brown earth,
    # and a track carrying ninety a square metre read as gravel chippings.
    #
    # A fragment is a piece of the ground that broke off. Taking its colour from
    # the ground's own mid stop is not a convenience; it is the only value that
    # can be right, and it means retuning a soil retunes its debris with it.
    #
    # Lifted a little, because a fresh break exposes unweathered material and a
    # loose lump catches more sky than the packed surface it is lying on. A
    # little — the old figure's mistake was the size of the gap, not its sign.
    # ## And a fragment is a *lit* object, not a dark speck
    #
    # A quarter above the soil's mid stop still rendered darker than the lit
    # ground around it, so ninety thousand fragments came out as dark dots
    # peppering a smooth surface. In the reference photograph the crumbs are the
    # brightest thing on the ground — they stand proud, they catch the sun on
    # top, and what reads is the *pair*: a lit crown with its own shadow beside
    # it. Dots have no shadow and no crown.
    #
    # Two and a half times the mid puts a fragment between the soil's mid and
    # high stops, which is where a piece of unweathered material freshly turned
    # out of the surface actually sits.
    mid = (soils or [{}])[0].get("dry_palette", {}).get("mid", [0.036, 0.021, 0.011])
    # Two and a half was set when the fragments were buried and invisible, to
    # make them findable. Now that they are the surface's dominant structure it
    # reads as a scatter of pale gravel on brown earth: a fragment is a lump of
    # the ground it broke out of and cannot be twice its brightness. Half again
    # above the mid is a fresh break catching more sky than the packed surface
    # around it, which is the whole of the difference.
    material = stone_material(settings, [c * 1.5 for c in mid])
    principled = material.node_tree.nodes["Principled BSDF"]
    principled.inputs["Roughness"].default_value = 0.94
    # A mineral aggregate, not a dense silicate.
    principled.inputs["IOR"].default_value = 1.45
    # ## The same specular fix the ground got, which this was left out of
    #
    # `SOIL_SPECULAR` was applied to the ground material and not here, so every
    # fragment kept running at the Principled default — a smooth dielectric's
    # four per cent against an albedo of about five. Specular takes the light's
    # colour rather than the material's, so a fragment desaturated toward white
    # and read as **pale grey gravel scattered on brown earth**, which is exactly
    # what halving its albedo failed to fix: the albedo was never what was wrong.
    #
    # A fragment is a lump of the ground it broke out of. It has the ground's
    # composition, so it has the ground's reflectance.
    live(principled.inputs, "Specular IOR Level").default_value = SOIL_SPECULAR
    return material


def stone_material(settings, base_colour):
    """A stone or fragment: a rough dielectric with per-instance tint.

    The colour arrives as an argument rather than being chosen here, because
    `surface.stone` and `surface.shell_fragment` are the same shader at two
    reflectances and writing it twice would be two places to fix a roughness.

    The per-instance tint multiplies it, from Object Info, so a field of stones
    is not a field of clones. Bounded well inside a factor of two on the Rust
    side: the point is to break the repetition, not to make one of them a
    different rock.
    """
    _ = settings
    material = bpy.data.materials.new("stone")
    material.use_nodes = True
    tree = material.node_tree
    principled = tree.nodes["Principled BSDF"]
    principled.inputs["Roughness"].default_value = 0.78
    principled.inputs["IOR"].default_value = 1.52

    info = tree.nodes.new("ShaderNodeObjectInfo")
    info.location = (-600, 0)
    tinted = tree.nodes.new("ShaderNodeMixRGB")
    tinted.location = (-350, 0)
    tinted.blend_type = "MULTIPLY"
    tinted.inputs["Fac"].default_value = 1.0
    tinted.inputs["Color1"].default_value = (*base_colour, 1.0)
    tree.links.new(info.outputs["Color"], tinted.inputs["Color2"])
    tree.links.new(tinted.outputs["Color"], principled.inputs["Base Color"])
    return material


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
    live(along.inputs, "From Min").default_value = 0.0
    live(along.inputs, "From Max").default_value = 0.30
    links.new(geometry.outputs["Position"], separate.inputs["Vector"])
    links.new(separate.outputs["Z"], live(along.inputs, "Value"))

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

    links.new(live(along.outputs, "Result"), ramp.inputs["Fac"])
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
    live(spread.inputs, "From Min").default_value = 0.06
    live(spread.inputs, "From Max").default_value = 0.94
    spread.clamp = True

    links.new(coordinate.outputs["Object"], drift.inputs["Vector"])
    links.new(drift.outputs["Fac"], live(spread.inputs, "Value"))

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
    links.new(live(spread.outputs, "Result"), live(graded.inputs, "Factor"))

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
    live(is_dry.inputs, "From Min").default_value = 3.3
    live(is_dry.inputs, "From Max").default_value = 3.9
    is_dry.clamp = True

    straw = nodes.new("ShaderNodeValToRGB")
    straw.location = (-700, -400)
    straw.color_ramp.elements[0].color = (0.115, 0.088, 0.030, 1.0)
    straw.color_ramp.elements[1].color = (0.235, 0.190, 0.072, 1.0)

    withered = nodes.new("ShaderNodeMix")
    withered.data_type = "RGBA"
    withered.location = (-20, 150)

    links.new(tone.outputs["Fac"], live(is_dry.inputs, "Value"))
    links.new(live(along.outputs, "Result"), straw.inputs["Fac"])
    links.new(live(graded.outputs, "Result"), live(withered.inputs, "A"))
    links.new(live(straw.outputs, "Color"), live(withered.inputs, "B"))
    links.new(live(is_dry.outputs, "Result"), live(withered.inputs, "Factor"))
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


# How dark the deepest crevices go, as a fraction of their unoccluded colour.
#
# ## Why this is a multiplier and not a slide along the palette
#
# It used to be the latter: the cavity was subtracted from the tone *before* the
# palette ramp, inside the same Map Range that normalises the two pigment
# noises. That made it a pigment term, and it was diluted twice over.
#
# Diluted once by the normalisation. The window the ramp maps runs from
# `floor - CAVITY_TONE` to `floor + region + patch`, which for compacted loam is
# 1.95 wide — so a cavity of one moved the palette position by 0.55/1.95, or
# **0.28**, not by the half the comment claimed.
#
# Diluted again by the cavity's own range. `relief_of` normalises the height
# against the *declared* band amplitudes while compaction and moisture have
# already shrunk the height, so on a packed track the cavity spans about
# 0.26–0.61 rather than 0–1. The two together left a full clod field moving the
# tone by a tenth of the palette, and the measured result was a track with a
# dynamic range of 2.3x standing next to grass at 52x — a flat orange midtone
# behind dark, high-contrast geometry.
#
# Raising the old constant could not fix it: at 0.85 the normalised coefficient
# only reaches 0.38, because the constant appears in the window it is divided by.
#
# So occlusion is now what it physically is — a multiplier on outgoing light,
# applied after the palette rather than inside it. Three quarters means the
# deepest crevices return a quarter of the light their crests do, which is the
# contact darkening that carries depth from a fixed high camera.
# How much harder wetting bites at the top of the palette than the bottom.
#
# Kept in step with `GroundMaterialProfile::WET_BITE` by hand, like every other
# number that has to mean the same thing in both languages. See that constant
# for why the response is a tilted ratio rather than the square law it replaced.
WET_BITE = 0.35

# The finest wavelength colour grain is drawn at, in metres.
#
# About three traced pixels at the default framing. Finer than this and the
# sampler averages it to a flat tone, so it costs a noise lookup and buys
# nothing — see the note in `soil_branch`. What lives below it is roughness.
GRAIN_FLOOR_M = 0.011

# Where the reflection light sits, and how bright.
#
# The camera's azimuth plus half a turn, at the camera's own elevation: where a
# horizontal mirror has to reflect for the reflection to reach the lens. See
# `build_reflection_light` for why this is a lamp rather than a bright patch of
# sky, and why the key light is not simply moved here.
# Where the reflection light sits, and how bright.
#
# The camera's azimuth plus half a turn, at the camera's own elevation: the
# direction a horizontal mirror has to reflect for the reflection to reach the
# lens. See `build_reflection_light` for why this is a lamp rather than a bright
# patch of sky, and why the key light is not simply moved here.
REFLECTION_BEARING_DEG = 225.0
REFLECTION_ELEVATION_DEG = 35.264
# Calibrated against a paired control — the same build with this light removed
# — rather than chosen. Dry soil still has a four-per-cent Fresnel specular, so
# a reflection bright enough to be obvious on a wet film is bright enough to
# lift a dry one: at an energy of 9 dry ground rose eighty per cent against a
# two-per-cent budget.
#
# At this figure dry ground moves **+0.1%** and wet ground gains 90% in the mean
# with its ninety-ninth percentile up 182% — the highlight rising twice as fast
# as the surface it sits on, which is what makes it a highlight. A tenth of this
# keeps the mean inside a tighter band but loses the concentration: the peak
# stops outrunning the mean and it becomes a lift again.
REFLECTION_STRENGTH = 0.006

# ## Painted occlusion, now that the ground can cast its own
#
# This was 0.80 — crevices darkened to a fifth of the crest beside them — and it
# was doing the job the geometry could not. Measured off an export, bare soil ran
# at 2.0 mm peak-to-peak and a mean slope of 5.6 degrees against a 35-degree sun,
# so nothing shadowed anything and the only thing standing between a render and a
# flat card was this multiply. It is why soil came back looking like camouflage:
# a smooth scalar field painted at aggregate scale is a blob, not a pocket.
#
# The mesh now carries three bands down to two centimetres and casts real
# shadows, so this goes back to being what it is meant to be — a small
# sky-occlusion term for hollows the sun never reaches into anyway. Left at 0.80
# it double-counts, and the double-counted version is the blotchy one.
# ## Cut too far, then measured
#
# This was 0.80 when it was doing the geometry's job — a smooth scalar field
# painted at aggregate scale, standing in for shadows the flat ground could not
# cast, and reading as camouflage. Once the mesh started casting real ones it
# went to 0.30 on the argument that keeping it would double-count.
#
# It undercounts instead. A real crevice is dark for two reasons and only one of
# them is a cast shadow: the other is that a hollow *sees less sky*, and sky is a
# quarter of the light in this scene. Cast shadow is a sun term and cannot supply
# it. With the term at 0.30 the soil came back as one flat dry brown with no
# separation between its crowns and its pockets.
#
# Half, which is a hollow at twice the contrast a cast shadow alone gives it and
# well short of the 0.80 that was painting the structure.
CAVITY_OCCLUSION = 0.52

# Where the cavity signal starts and finishes counting.
#
# Not 0 and 1. The cavity a real profile produces occupies the middle of its
# nominal range for the reason above, so a transfer that starts at zero spends
# most of its slope on values that never occur. These bounds put the full swing
# across the band that is actually populated.
CAVITY_LOW = 0.34
CAVITY_HIGH = 0.68

CAVITY_TONE_FINE = 0.70

# Where a water film starts to be a coherent reflecting surface, where it is as
# coherent as it gets, and how much of the hemisphere it can ever cover.
#
# The ceiling is the important one. A real film over earth is broken by
# protruding grains, debris and its own shallow menisci; it is never the
# unbroken dielectric layer that a coat weight of one describes, and rendering
# it as one is the difference between wet mud and poured resin.
# ## A clearcoat is what plastic is
#
# These were 0.30 / 0.85 / 0.45, and a track at a moisture of 0.78 therefore
# rendered under a quarter-strength smooth dielectric layer. That is not a
# description of damp earth — it is a description of a varnished object, and it
# is why the surface read as plastic however the geometry underneath it moved.
#
# The physical case for a coat at all is a *continuous film*: water standing on
# ground that cannot absorb any more, with a real air-water interface of its own.
# Damp soil has no such interface. Water in the pores changes how the substrate
# scatters — which is the albedo and the roughness underneath, and both already
# respond — and adds no second surface at all.
#
# So the onset moves up to where a film genuinely forms, and the ceiling comes
# down to something a broken, grain-studded, debris-strewn sheet of water can
# plausibly cover.
COAT_ONSET = 0.62
COAT_FULL = 0.92
COAT_CEILING = 0.20

# How much of a smooth dielectric's reflectance a porous aggregate keeps.
#
# Blender's neutral is 0.5. This is a twelfth of it — see `ground_material` for
# the measurement that sets it.
SOIL_SPECULAR = 0.04

# What is left of the pore-scale bump at full saturation, and under a wheel.
# ## Both were far too deep
#
# At a moisture of 0.78 and a compaction of 0.59 these multiplied out to **0.42**
# — the pore-scale bump running at two fifths strength on the one surface whose
# whole problem was that it had no fine detail. A crop of the render beside the
# reference at the same magnification came back as soft gradients with nothing
# above a centimetre in it.
#
# Water and compaction do fill pores, and the effect is real; it is not this
# large. Enough to be seen as a difference between damp and dry, not enough to
# erase the surface.
BUMP_WET = 0.82
BUMP_PACKED = 0.72


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

    # How packed the ground is. Reaches the relief bands, so a track loses its
    # grain before its clods rather than the other way round.
    compaction = nodes.new("ShaderNodeAttribute")
    compaction.location = (-2600, -900)
    compaction.attribute_name = "compaction"

    # A continuous film of water on the surface, as opposed to water *in* the
    # soil. Two different things: moisture darkens the substrate and collapses
    # its pore-scale roughness; a film is a dielectric layer sitting on top of
    # it with its own IOR and its own highlight. Driving one from the other is
    # how a merely damp crown ends up looking varnished.
    wet_film = nodes.new("ShaderNodeAttribute")
    wet_film.location = (-2600, -1050)
    wet_film.attribute_name = "wet_film"

    colour_sum = None
    rough_sum = None
    height_sum = None

    for index, entry in enumerate(materials):
        y = 900 - index * 900
        weight = nodes.new("ShaderNodeAttribute")
        weight.location = (-2600, y)
        weight.attribute_name = f"w{index}"

        colour, roughness, height = soil_branch(
            nodes, links, coordinate, moisture, compaction, cavity, entry, y
        )

        colour_sum = accumulate_colour(nodes, links, colour_sum, colour, weight, y)
        rough_sum = accumulate_float(nodes, links, rough_sum, roughness, weight, y - 200)
        height_sum = accumulate_float(nodes, links, height_sum, height, weight, y - 400)

    # ## Occlusion, applied once to the blended colour
    #
    # A crevice in bare earth reads many times darker than the crest beside it,
    # and that is occlusion rather than pigment — which is why the deepest fifth
    # of a photograph of soil is nearly neutral in hue while its median is a
    # warm brown. Sky lights a hole; sun lights a crest.
    #
    # It lives here rather than inside `soil_branch` for two reasons. It is a
    # property of the *point* and not of the soil, so running it per branch was
    # computing one answer several times and blending it with itself. And a
    # multiply buried in a per-soil chain is very hard to prove is connected:
    # four renders went past in which driving it to full strength — crevices to
    # zero light — changed the picture not at all, and no amount of reading the
    # graph found the break. One node on the output is a thing a single render
    # can confirm.
    occlusion = nodes.new("ShaderNodeMapRange")
    occlusion.location = (-140, -260)
    live(occlusion.inputs, "From Min").default_value = CAVITY_LOW
    live(occlusion.inputs, "From Max").default_value = CAVITY_HIGH
    live(occlusion.inputs, "To Min").default_value = 1.0
    live(occlusion.inputs, "To Max").default_value = 1.0 - CAVITY_OCCLUSION
    occlusion.clamp = True
    links.new(cavity.outputs["Fac"], live(occlusion.inputs, "Value"))

    occlusion_grey = nodes.new("ShaderNodeCombineColor")
    occlusion_grey.location = (-140, -420)
    for channel in ("Red", "Green", "Blue"):
        links.new(live(occlusion.outputs, "Result"), occlusion_grey.inputs[channel])

    occluded = nodes.new("ShaderNodeMix")
    occluded.data_type = "RGBA"
    occluded.blend_type = "MULTIPLY"
    occluded.location = (-140, -100)
    live(occluded.inputs, "Factor").default_value = 1.0
    links.new(colour_sum, live(occluded.inputs, "A"))
    links.new(occlusion_grey.outputs["Color"], live(occluded.inputs, "B"))
    links.new(live(occluded.outputs, "Result"), principled.inputs["Base Color"])
    links.new(rough_sum, principled.inputs["Roughness"])

    # ## Soil has almost no coherent specular, and this was giving it a full one
    #
    # The ground never set this, so it ran at the Principled default — a smooth
    # dielectric's four per cent. Against a soil albedo of about three per cent
    # that is not a highlight sitting on a surface, it is **half the light coming
    # back**: computed from the scene's own sun and sky, 51% of the returned red
    # was specular. Specular takes the *light's* colour rather than the
    # material's, so the brown was being diluted with white before it reached the
    # camera, and no palette edit could fix it — the measured render sat at B/R
    # 0.47 against an authored 0.32, and moving the palette just moved the
    # diffuse half.
    #
    # The reference photograph settles what the right figure is. If soil had a
    # meaningful specular its bright end would desaturate toward the sun's
    # colour; measured, its brightest pixels come back at G/R 0.665 against a
    # median of 0.596 — essentially the same brown. Real soil is a porous
    # aggregate whose outer boundary is mostly voids and loose grains, with very
    # little coherent interface to reflect from at all.
    #
    # A water film is a coherent interface, which is what the coat is for and why
    # it is separate from this.
    live(principled.inputs, "Specular IOR Level").default_value = SOIL_SPECULAR

    # ## The micro-relief fades under water and under a wheel
    #
    # The bump ran at full strength whatever state the ground was in, so a
    # saturated hollow carried the same pore-scale texture as dry ground beside
    # it. Both of those fill it in: water bridges the gaps between grains, which
    # is the same physical fact `saturation_flattening` states and the reason wet
    # sand reads as poured; a wheel presses the crumb flat.
    #
    # The coarse relief is untouched, because it is on the mesh and neither
    # process removes an aggregate. This is only the part that was a normal.
    bump_state = nodes.new("ShaderNodeMath")
    bump_state.operation = "MULTIPLY_ADD"
    bump_state.location = (-700, -560)
    links.new(moisture.outputs["Fac"], bump_state.inputs[0])
    bump_state.inputs[1].default_value = BUMP_WET - 1.0
    bump_state.inputs[2].default_value = 1.0

    bump_packed = nodes.new("ShaderNodeMath")
    bump_packed.operation = "MULTIPLY_ADD"
    bump_packed.location = (-700, -700)
    links.new(compaction.outputs["Fac"], bump_packed.inputs[0])
    bump_packed.inputs[1].default_value = BUMP_PACKED - 1.0
    bump_packed.inputs[2].default_value = 1.0

    bump_strength = nodes.new("ShaderNodeMath")
    bump_strength.operation = "MULTIPLY"
    bump_strength.location = (-600, -620)
    links.new(bump_state.outputs["Value"], bump_strength.inputs[0])
    links.new(bump_packed.outputs["Value"], bump_strength.inputs[1])
    bump_strength.use_clamp = True

    # One Bump for the whole surface. See the docstring: blending perturbed
    # normals is not the same operation and does not give a usable one.
    bump = nodes.new("ShaderNodeBump")
    bump.location = (-500, -300)
    links.new(bump_strength.outputs["Value"], bump.inputs["Strength"])
    # The heights are already in metres, so the distance is unity and the
    # amplitudes in the profiles mean what they say.
    bump.inputs["Distance"].default_value = 1.0
    links.new(height_sum, bump.inputs["Height"])
    links.new(bump.outputs["Normal"], principled.inputs["Normal"])

    # ## A film is not a puddle, and it is not wet soil either
    #
    # `wet_film` is derived — it rises where the ground cannot absorb any more
    # and the surface is concave — and it drives the Principled *coat*: a thin
    # dielectric layer over the substrate, with water's IOR and a low roughness
    # of its own. The substrate underneath keeps its own albedo and its own
    # roughness, both already darkened and smoothed by `moisture`.
    #
    # Splitting them is what makes a hollow read as holding water while the
    # crown beside it reads as merely damp. Driving the coat from `moisture`
    # instead would varnish the whole surface the moment any of it was wet.
    #
    # Standing water is still a separate future mesh. A film is a millimetre of
    # specular; a puddle has a bottom you can see.
    # ## And a film has a ceiling
    #
    # The coat weight was `wet_film` itself, straight through, so saturated
    # ground reached a **full-strength** dielectric coat: a perfect varnish, and
    # the reason wet stripes on the comparison card read as poured resin rather
    # than as mud. A water film over earth is broken, thin and full of protruding
    # grains and floating debris; it never covers the whole hemisphere.
    #
    # So it is capped, and it starts late. Below about a third of a film there is
    # no coherent surface to reflect from at all — that ground is damp, which is
    # a substrate property and is already handled by the albedo and the roughness
    # underneath.
    coat = nodes.new("ShaderNodeMapRange")
    coat.location = (-300, -560)
    coat.interpolation_type = "SMOOTHSTEP"
    live(coat.inputs, "From Min").default_value = COAT_ONSET
    live(coat.inputs, "From Max").default_value = COAT_FULL
    live(coat.inputs, "To Min").default_value = 0.0
    live(coat.inputs, "To Max").default_value = COAT_CEILING
    coat.clamp = True
    links.new(wet_film.outputs["Fac"], live(coat.inputs, "Value"))

    try:
        links.new(live(coat.outputs, "Result"), live(principled.inputs, "Coat Weight"))
        film_ior = materials[0]["wet"]["film_ior"]
        # Broader than a mirror. 0.06 is a varnish and puts a small round
        # hotspot on the ground; a water film over earth follows a surface that
        # is itself slightly irregular, so the response is a smear that traces
        # the form. That smear is the primary depth cue on wet ground, and the
        # reason the sun was moved to where one can exist at all — see
        # `RenderSettings::sun_azimuth`.
        #
        # A tighter figure, near 0.03, is reserved for standing water, which
        # needs its own flat surface rather than a coat on a displaced one.
        #
        # Driven rather than fixed: a film thin enough to be patchy follows every
        # grain it is lying over and scatters accordingly, and only a continuous
        # one is smooth. Holding it at 0.10 gave the first drops of water the
        # same specular tightness as a puddle.
        coat_rough = nodes.new("ShaderNodeMapRange")
        coat_rough.location = (-300, -720)
        live(coat_rough.inputs, "From Min").default_value = COAT_ONSET
        live(coat_rough.inputs, "From Max").default_value = COAT_FULL
        # ## And it is not a mirror even when it is there
        #
        # 0.22 falling to 0.06 is a tight lobe: a small hard catch on every
        # convex bump, which is the signature of moulded plastic and was
        # measured directly — the render's brightest pixels came back at G/R
        # 0.928 against the reference photograph's 0.678, meaning they had
        # desaturated toward the light's own colour instead of keeping the
        # material's brown.
        #
        # A film lying over earth follows a surface that is itself irregular at
        # every scale below the film's thickness, so the response is a broad
        # smear that traces the form rather than a point that sits on it.
        live(coat_rough.inputs, "To Min").default_value = 0.42
        live(coat_rough.inputs, "To Max").default_value = 0.22
        coat_rough.clamp = True
        links.new(wet_film.outputs["Fac"], live(coat_rough.inputs, "Value"))
        links.new(
            live(coat_rough.outputs, "Result"),
            live(principled.inputs, "Coat Roughness"),
        )
        live(principled.inputs, "Coat IOR").default_value = film_ior
    except KeyError as missing:
        # Reported rather than swallowed. A Blender without a coat layer renders
        # a scene with no wet highlights anywhere, which looks like a document
        # that declared no moisture — and that is a much harder thing to work
        # out from the picture than a line of output.
        print(f"[terrain_cycles] no Principled coat ({missing}); wet film is not shaded")

    links.new(principled.outputs["BSDF"], output.inputs["Surface"])
    return material


def soil_branch(nodes, links, coordinate, moisture, compaction, cavity, entry, y):
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

    # ## The tonal field is not allowed into the clod band
    #
    # These ran at two and three octaves. Three octaves under a declared
    # wavelength of 25 cm puts pigment at 12.5, 6.3 and 3.1 cm — squarely inside
    # the three-to-nine centimetre band where the reference plate keeps a third
    # of its variance, and where that variance is *clods with shadows beside
    # them*. A colour field there does not read as clods. It reads as
    # camouflage, which is what a render of this soil looked like.
    #
    # The same argument `band_height` makes about hidden octaves, applied to the
    # colour: a declared wavelength has to be the wavelength. One octave each,
    # so the region term says which clearing this is and the patch term says
    # which scuff, and neither of them pretends to be geometry.
    region = noise(optics["region_wavelength_m"], 0.0, 300)
    patch = noise(optics["patch_wavelength_m"], 1.0, 150)

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
    live(tone_norm.inputs, "From Min").default_value = floor
    live(tone_norm.inputs, "From Max").default_value = (
        floor + optics["region_strength"] + optics["patch_strength"]
    )
    tone_norm.clamp = True
    links.new(tone2.outputs["Value"], live(tone_norm.inputs, "Value"))

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
    links.new(live(tone_norm.outputs, "Result"), palette.inputs["Fac"])

    # Grain: a darkening, not a second colour. Multiplying keeps the hue the
    # palette already chose and only varies how much light comes back.
    #
    # ## And it is a dry phenomenon
    #
    # Grain contrast is light scattering off the *tops* of individual grains
    # with air between them. Fill that space with water and the scattering
    # collapses: the grains stop being separate reflectors and become inclusions
    # in a continuous film. Which is why wet ground is visually *smooth* at
    # grain scale — a wet beach reads as a poured surface — and why leaving the
    # grain at full strength through saturation produced mud with dust on it.
    #
    # Faded by the soil's own `saturation_flattening`, so the number that says
    # how much relief water fills also says how much grain contrast it removes.
    # They are the same physical fact measured two ways, and giving them two
    # authored constants would let a profile claim water fills its pores while
    # its shading says otherwise.
    # ## One frequency, and a scalar
    #
    # Two defects lived in these three lines.
    #
    # **Detail was four.** A Blender Noise at detail four is four octaves, so a
    # grain declared at a millimetre also contained content at eight — squarely
    # in the intermediate scale this whole vocabulary is arranged to keep empty,
    # and the exact hidden-octave failure `band_height` was fixed for. A band is
    # a scale; detail zero is one frequency, which is what a band is.
    #
    # **It multiplied by `Color`.** Blender's Noise Texture emits an RGB output
    # whose three channels are three *different* noise fields, so multiplying a
    # palette colour by it moves the hue — the opposite of what the comment
    # above claims and of what grain physically is. `Fac` is the scalar field;
    # replicated across three channels it darkens without recolouring.
    #
    # The fallback wavelength is also gone. When no band reaches the shader
    # there is no grain to draw, and inventing a two-centimetre one put a
    # feature at a scale no profile had declared into every such material.
    # ## And it has to be drawn at a scale the renderer can resolve
    #
    # The wavelength comes from the coarsest band the *mesh* could not carry,
    # which is a sensible-sounding rule that quietly guarantees the opposite of
    # what it wants. Those bands are below the mesh threshold precisely because
    # they are small, and for a fine-grained soil the coarsest of them can be a
    # millimetre — a third of a traced pixel. Drawn there, the grain is averaged
    # away by the sampler before it reaches the image, and `grain_strength`
    # does nothing spatial at any value. Raising it from 0.34 to 0.72 on the
    # sand profile changed the picture not at all, which is what sent me looking.
    #
    # So it is floored at a few traced pixels. Below that the structure belongs
    # in the roughness, which is exactly where `micro_roughness` already put it —
    # this is the same tier boundary the relief plan draws, applied to the
    # colour instead of to the shape.
    grain_band = entry["shader_bands"][0] if entry["shader_bands"] else None
    grain_wavelength = grain_band["wavelength_m"] if grain_band else None
    if grain_wavelength is not None:
        grain_wavelength = max(grain_wavelength, GRAIN_FLOOR_M)

    grain_fade = nodes.new("ShaderNodeMapRange")
    grain_fade.location = (-1600, y + 60)
    live(grain_fade.inputs, "To Min").default_value = optics["grain_strength"]
    live(grain_fade.inputs, "To Max").default_value = optics["grain_strength"] * (
        1.0 - entry["wet"]["flattening"]
    )
    grain_fade.clamp = True
    links.new(moisture.outputs["Fac"], live(grain_fade.inputs, "Value"))

    if grain_wavelength is None:
        grained = palette
    else:
        grain = noise(grain_wavelength, 0.0, -50)
        grain_grey = nodes.new("ShaderNodeCombineColor")
        grain_grey.location = (-1560, y + 300)
        for channel in ("Red", "Green", "Blue"):
            links.new(grain.outputs["Fac"], grain_grey.inputs[channel])

        grained = nodes.new("ShaderNodeMix")
        grained.data_type = "RGBA"
        grained.blend_type = "MULTIPLY"
        grained.location = (-1450, y + 300)
        links.new(live(grain_fade.outputs, "Result"), live(grained.inputs, "Factor"))
        links.new(live(palette.outputs, "Color"), live(grained.inputs, "A"))
        links.new(grain_grey.outputs["Color"], live(grained.inputs, "B"))

    # Whichever node now carries the dry colour: the palette ramp when this
    # soil declares no shader band, the grain multiply when it does.
    grain_out = (
        live(palette.outputs, "Color")
        if grained is palette
        else live(grained.outputs, "Result")
    )

    # ## Wet ground is not dry ground turned down
    #
    # Water fills the pores, so the air-soil boundary that scattered light
    # diffusely becomes a water-soil boundary: internal scattering falls,
    # absorption rises, and the outer surface becomes a smooth water-air
    # interface. Three things follow, and doing only the first is what makes wet
    # ground read as ground in shadow instead of as wet ground.
    #
    #   albedo      darkens, hardest where the surface was brightest
    #   hue         warms — the film absorbs blue and green harder than red
    #   roughness   collapses, and does so before the darkening is noticeable
    #
    # ## A ratio, not a square
    #
    # This ran `wet = dry² × gain` for the whole of its first life, fitted so
    # the palette's mid stop landed on the authored wet colour. The consequence
    # was measured rather than argued: the ratio `wet/dry` under that law is
    # proportional to how bright a point already is, so the brightest parts of a
    # surface darkened *least* — and a surface's mean sits above its own mid, so
    # almost none of it darkened. A loam authored to darken by four and a half
    # rendered at 1.48, next to its own dry stripe, and read as the same
    # material twice.
    #
    # It is now a ratio anchored at the mid and tilted so the crests take more
    # of it than the hollows. See `GroundMaterialProfile::WET_BITE`, which is
    # the same constant on the Rust side — the two halves compute one response
    # and a test asserts the Rust one.
    #
    # No subsurface scattering. Production mud shaders do not use it — mud is a
    # rough dark dielectric under a glossy coat.
    mid = entry["dry_palette"]["mid"]
    wet_mid = entry["wet"]["mid"]
    gain = tuple((wet_mid[c] / mid[c]) if mid[c] > 0.0 else 0.0 for c in range(3))

    # One at the mid, above one below it, below one above it.
    tilt = nodes.new("ShaderNodeMapRange")
    tilt.location = (-1250, y + 40)
    live(tilt.inputs, "To Min").default_value = 1.0 + WET_BITE
    live(tilt.inputs, "To Max").default_value = 1.0 - WET_BITE
    tilt.clamp = True
    links.new(live(tone_norm.outputs, "Result"), live(tilt.inputs, "Value"))

    tilt_grey = nodes.new("ShaderNodeCombineColor")
    tilt_grey.location = (-1150, y + 40)
    for channel in ("Red", "Green", "Blue"):
        links.new(live(tilt.outputs, "Result"), tilt_grey.inputs[channel])

    ratio = nodes.new("ShaderNodeMix")
    ratio.data_type = "RGBA"
    ratio.blend_type = "MULTIPLY"
    ratio.location = (-1050, y + 120)
    live(ratio.inputs, "Factor").default_value = 1.0
    live(ratio.inputs, "A").default_value = gain + (1.0,)
    links.new(tilt_grey.outputs["Color"], live(ratio.inputs, "B"))

    lifted = nodes.new("ShaderNodeMix")
    lifted.data_type = "RGBA"
    lifted.blend_type = "MULTIPLY"
    lifted.location = (-1100, y + 200)
    live(lifted.inputs, "Factor").default_value = 1.0
    links.new(grain_out, live(lifted.inputs, "A"))
    links.new(live(ratio.outputs, "Result"), live(lifted.inputs, "B"))

    # Wetting may not brighten a channel. Water in the pores removes light
    # paths; it never adds one. Rust clamps the same way — see
    # `GroundMaterialProfile::albedo`.
    darkened = nodes.new("ShaderNodeMix")
    darkened.data_type = "RGBA"
    darkened.blend_type = "DARKEN"
    darkened.location = (-1000, y + 200)
    live(darkened.inputs, "Factor").default_value = 1.0
    links.new(live(lifted.outputs, "Result"), live(darkened.inputs, "A"))
    links.new(grain_out, live(darkened.inputs, "B"))

    wetted = nodes.new("ShaderNodeMix")
    wetted.data_type = "RGBA"
    wetted.location = (-950, y + 300)
    links.new(grain_out, live(wetted.inputs, "A"))
    links.new(live(darkened.outputs, "Result"), live(wetted.inputs, "B"))
    links.new(moisture.outputs["Fac"], live(wetted.inputs, "Factor"))

    # Roughness across the dry range, collapsing toward the wet value.
    dry_low, dry_high = entry["roughness_dry"]
    rough = nodes.new("ShaderNodeMapRange")
    rough.location = (-1250, y - 150)
    live(rough.inputs, "To Min").default_value = dry_low
    live(rough.inputs, "To Max").default_value = dry_high
    links.new(live(tone_norm.outputs, "Result"), live(rough.inputs, "Value"))

    # Relief finer than a pixel is not a bump, it is a BRDF. Bands below the
    # sampling rate were folded into one roughness figure in Rust; adding them
    # here is what stops a millimetre grain arriving as speckle.
    micro = nodes.new("ShaderNodeMath")
    micro.operation = "ADD"
    micro.location = (-1100, y - 150)
    links.new(live(rough.outputs, "Result"), micro.inputs[0])
    micro.inputs[1].default_value = entry.get("micro_roughness", 0.0)
    micro.use_clamp = True

    # ## Roughness collapses before the darkening is noticeable
    #
    # `WetResponse`'s own documentation says so and the shader did not do it:
    # albedo and roughness were both mixed linearly by the same moisture value,
    # so a surface at half moisture was half dark and half smooth. Real ground
    # is not — a first shower makes ground *shine* well before it makes it dark,
    # because a film only a grain thick already replaces the air-soil interface
    # while the pores underneath are barely wetted.
    #
    # A square root front-loads it: at a quarter moisture the roughness is
    # already halfway to its wet value while the colour has moved a quarter.
    # That gap is the whole of "damp", and without it there is no damp — only
    # dry and soaked.
    rough_response = nodes.new("ShaderNodeMath")
    rough_response.operation = "POWER"
    rough_response.location = (-1100, y - 260)
    links.new(moisture.outputs["Fac"], rough_response.inputs[0])
    # ## Front-loaded, but not this hard
    #
    # A square root puts a surface at 0.45 moisture two-thirds of the way to its
    # wet roughness — so `meadow_path`'s track, authored as damp, rendered at a
    # roughness of 0.40 and caught the sun across its whole width. It read as wet
    # concrete, and halving its albedo moved the picture by four percent because
    # almost none of its brightness was diffuse.
    #
    # Seven tenths keeps the shape the physics asks for — a first shower makes
    # ground shine before it makes it dark — while leaving damp ground damp.
    rough_response.inputs[1].default_value = 0.7

    # ## Wetting scales the roughness; it does not replace it
    #
    # This mixed toward `roughness_wet` as a *constant*, so a fully wet surface
    # had one roughness everywhere — every scuff, crest and hollow the dry
    # surface distinguished collapsed onto a single microfacet width. That is the
    # melted-chocolate failure exactly: a uniform glossy brown with no structure
    # in its highlight, and it is what the wet stripes on the comparison card
    # were.
    #
    # Real wet ground keeps its variation. Water fills the pores, which lowers
    # the whole distribution; it does not level the surface, because broken film,
    # protruding grains and shallow menisci all survive. So the wet target is a
    # *ratio* against the dry midpoint and it multiplies, which lowers every
    # point by the same proportion and leaves the differences between them.
    dry_mid = 0.5 * (dry_low + dry_high)
    wet_ratio = entry["wet"]["roughness"] / dry_mid if dry_mid > 0.0 else 1.0

    rough_factor = nodes.new("ShaderNodeMapRange")
    rough_factor.location = (-1000, y - 260)
    live(rough_factor.inputs, "To Min").default_value = 1.0
    live(rough_factor.inputs, "To Max").default_value = wet_ratio
    rough_factor.clamp = True
    links.new(rough_response.outputs["Value"], live(rough_factor.inputs, "Value"))

    wet_rough = nodes.new("ShaderNodeMath")
    wet_rough.operation = "MULTIPLY"
    wet_rough.location = (-950, y - 150)
    links.new(micro.outputs["Value"], wet_rough.inputs[0])
    links.new(live(rough_factor.outputs, "Result"), wet_rough.inputs[1])
    wet_rough.use_clamp = True

    height = band_height(nodes, links, coordinate, moisture, compaction, entry, y)

    # ## Mesh-scale occlusion
    #
    # A crevice in bare earth reads many times darker than the crest beside it,
    # and that is **occlusion**, not pigment — which is why the deepest fifth of
    # a photograph of soil is nearly neutral in hue while its median is a warm
    # brown. Sky lights a hole; sun lights a crest.
    #
    # So it multiplies the colour rather than sliding it along the palette. What
    # this replaced did the latter, inside the pigment ramp, and arrived at a
    # tenth of the intended depth — see `CAVITY_OCCLUSION`.
    #
    # Applied *after* the wet mix, because a wet hollow is a dark hollow too:
    # water changes what the surface returns, occlusion changes how much of it
    # gets out, and multiplying them is the correct composition of the two.
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
        wet_rough.outputs["Value"],
        height,
    )


def band_height(nodes, links, coordinate, moisture, compaction, entry, y):
    """The relief bands the mesh could not carry, summed, in metres.

    Which bands these are was decided in Rust by the lattice spacing, not here
    and not by the profile. A band the mesh resolves is displaced geometry; a
    band below that is this. Each band is drawn exactly once by whichever half
    can actually draw it — the alternative, a fixed rule about which scales are
    "bump", double-counts a band whenever the sampling rate changes.

    ## Matching what Rust does, term for term

    This graph used to be a *different surface* from the mesh beside it. Three
    concrete divergences, all now closed:

    - **Detail was two.** A Blender Noise at detail two is two octaves, so a
      five-centimetre band contained content at two and a half — the exact
      hidden-octave failure the Rust basis was fixed for. Detail zero is one
      frequency, which is what a band is.
    - **The shape was a fold.** `1 − 2|c|` is not monotone, so its crests trace
      the *mid-level contours* of the underlying noise rather than its peaks.
      A field of that reads as worms. Rust replaced it with a power skew and so
      does this.
    - **Compaction reached the mesh and not the shader.** A packed track lost
      its clods and kept its grain, which is backwards: pressure flattens the
      fine structure first because there is less of it to resist.

    What still differs is the *phase*: Blender Noise and Rust value noise are
    different functions, so a band moving between tiers still moves its
    features even though it no longer changes its morphology or its response.
    Closing that needs the Rust-authored bump plane; the plan records which
    bands are affected.
    """
    total = None
    for slot, band in enumerate(entry["shader_bands"]):
        if band["amplitude_m"] <= 0.0:
            continue
        row = y - 600 - slot * 200
        noise = nodes.new("ShaderNodeTexNoise")
        noise.location = (-2400, row)
        noise.inputs["Scale"].default_value = 1.0 / max(band["wavelength_m"], 1e-5)
        # One frequency. See the docstring.
        noise.inputs["Detail"].default_value = 0.0
        noise.inputs["Roughness"].default_value = 0.5
        links.new(coordinate.outputs["Object"], noise.inputs["Vector"])

        shaped = aggregate_shape(nodes, links, noise.outputs["Fac"], band, row)

        # State. Both responses multiply, and both are the profile's own
        # numbers rather than anything chosen here.
        packed = nodes.new("ShaderNodeMath")
        packed.operation = "MULTIPLY_ADD"
        packed.location = (-1950, row - 60)
        links.new(compaction.outputs["Fac"], packed.inputs[0])
        packed.inputs[1].default_value = -band["compaction_response"]
        packed.inputs[2].default_value = 1.0
        packed.use_clamp = True

        # Water fills the smallest cavities first, so a wet surface loses its
        # grain long before it loses its clods.
        soaked = nodes.new("ShaderNodeMath")
        soaked.operation = "MULTIPLY_ADD"
        soaked.location = (-1950, row - 130)
        links.new(moisture.outputs["Fac"], soaked.inputs[0])
        soaked.inputs[1].default_value = -entry["wet"]["flattening"]
        soaked.inputs[2].default_value = 1.0
        soaked.use_clamp = True

        state = nodes.new("ShaderNodeMath")
        state.operation = "MULTIPLY"
        state.location = (-1800, row - 95)
        links.new(packed.outputs["Value"], state.inputs[0])
        links.new(soaked.outputs["Value"], state.inputs[1])

        damped = nodes.new("ShaderNodeMath")
        damped.operation = "MULTIPLY"
        damped.location = (-1700, row)
        links.new(shaped, damped.inputs[0])
        links.new(state.outputs["Value"], damped.inputs[1])

        scaled = nodes.new("ShaderNodeMath")
        scaled.operation = "MULTIPLY"
        scaled.location = (-1650, row)
        links.new(damped.outputs["Value"], scaled.inputs[0])
        scaled.inputs[1].default_value = band["amplitude_m"]

        if total is None:
            total = scaled.outputs["Value"]
        else:
            add = nodes.new("ShaderNodeMath")
            add.operation = "ADD"
            add.location = (-1500, row)
            links.new(total, add.inputs[0])
            links.new(scaled.outputs["Value"], add.inputs[1])
            total = add.outputs["Value"]

    if total is None:
        zero = nodes.new("ShaderNodeValue")
        zero.location = (-1500, y - 600)
        zero.outputs[0].default_value = 0.0
        total = zero.outputs[0]
    return total


def aggregate_shape(nodes, links, raw, band, y):
    """Turn a `0..1` band into a fracture surface: two levels and a steep wall.

    The same transform as `terrain_generators::ground::shape`, and it has to stay
    that way: a band can move between the mesh and this graph as the lattice
    changes, and a band that changed *shape* when it changed representation would
    make a clod turn into a different clod because the camera moved closer.

    ```text
    s    = clamp((u - (centre - wall/2)) / wall, 0, 1)
    out  = smoothstep(s) - (1 - centre)
    ```

    Flat at both ends, steep in the middle. The version before it was flat at one
    end and domed at the other — a plane with spheres on it, which is what a
    render of soil looked like, and it was called a nineties video game.

    Monotone, which is the other thing both of these had to be. The fold they
    replaced — `1 − 2|c|` — maps two different inputs to one output, so its
    crests follow the *mid-level contours* of the noise underneath rather than
    its peaks. Mid contours of a smooth random field are long, thin and closed,
    which is why a plate of it read unmistakably as worms.

    Smoothstep integrates to a half over its own width, so subtracting
    `1 - centre` keeps the band zero-mean whatever the wall width — the profile's
    amplitude means what it says and retuning a shape does not move the ground.

    ## The wall is floored here too, against the traced pixel

    On the mesh the limit is the lattice; here there is no lattice, so the limit
    is what the sampler can carry. A transition narrower than a couple of traced
    pixels is not a crisp edge in the image — it is aliasing that the denoiser
    then smears — so the same widening applies, measured against the pixel.

    These bands are bumps, below the mesh, so they do not take the slow
    `centre_shift` field the mesh bands do. At a scale below a lattice cell,
    "what kind of ground this patch is" is not a question the surface is being
    asked; it is grain, and grain is the same everywhere.
    """
    centre = band.get("centre", 0.50)
    wall = max(band.get("wall", 0.78), band.get("flank_floor", 0.0))
    wall = min(max(wall, 1.0e-4), 2.0 * min(centre, 1.0 - centre))

    offset = nodes.new("ShaderNodeMapRange")
    offset.location = (-2200, y)
    offset.interpolation_type = "SMOOTHSTEP"
    live(offset.inputs, "From Min").default_value = centre - 0.5 * wall
    live(offset.inputs, "From Max").default_value = centre + 0.5 * wall
    live(offset.inputs, "To Min").default_value = 0.0
    live(offset.inputs, "To Max").default_value = 1.0
    offset.clamp = True
    links.new(raw, live(offset.inputs, "Value"))

    centred = nodes.new("ShaderNodeMath")
    centred.operation = "SUBTRACT"
    centred.location = (-1980, y)
    links.new(live(offset.outputs, "Result"), centred.inputs[0])
    centred.inputs[1].default_value = 1.0 - centre
    return centred.outputs["Value"]


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
    # ## A bright patch of sky where a wet surface can see it
    #
    # For a wet highlight to reach this camera off horizontal ground, the light
    # has to sit near the camera's azimuth plus half a turn — about 225° — and
    # the key sits at 125° because that is where the grass was tuned. Moving the
    # key was tried and reverted: it put a broad specular wash over the whole
    # plate and turned `meadow_path`'s track into wet concrete.
    #
    # This is the other way round. A small, bright region of *sky* at the
    # reflection bearing contributes almost nothing diffuse, because its solid
    # angle is tiny — but a wet film at a roughness of two tenths reflects it
    # sharply, so the sheen appears exactly on the surfaces that are wet and
    # nowhere else. It is infinite, so it is identical across every trace slice,
    # which a second lamp would not be.
    #
    # It is a fixed-camera art-direction decision and worth naming as one: a
    # real sky does have bright and dark quarters, but this one is placed where
    # it is because of where the camera is.
    links.new(ramp.outputs["Color"], background.inputs["Color"])
    links.new(background.outputs["Background"], output.inputs["Surface"])


def build_reflection_light(sun):
    """A second sun, for the wet film only, that lights nothing diffusely.

    ## Why the sky patch did not work

    The first attempt at a wet highlight put a bright cap of *sky* at the
    reflection bearing, on the argument that its solid angle was too small to
    matter diffusely. Measured against a paired control — the same build with
    the patch off — it lifted the darkest decile of wet ground by 118% and the
    brightest by 5%. A monotonic decrease with brightness is the signature of
    **fill**, not of a highlight: it was filling shadows, which is the opposite
    of what a sheen does and undoes the contact darkening the occlusion term
    exists to create.

    That is not a tuning failure. A world contributes to the diffuse integral by
    construction and there is no way to exclude it. A *lamp* can be excluded,
    and Blender's `diffuse_factor` does exactly that.

    So this is a sun that only the specular and coat lobes can see, aimed where
    a horizontal mirror reflects into the camera. It is a production reflection
    light and physically a fudge; it is here because the key light cannot move
    without re-tuning the whole plate, and because a wet surface that produces
    no highlight reads as ground in shadow — which `WetResponse`'s own
    documentation names as the failure to avoid.
    """
    data = bpy.data.lights.new("reflection", type="SUN")
    data.energy = REFLECTION_STRENGTH
    # Tight, so it is a highlight rather than a wash. A wet film at a roughness
    # of a tenth spreads it enough on its own.
    data.angle = np.radians(2.0)
    data.color = (1.0, 0.98, 0.94)
    # The whole point: nothing diffuse. Dry ground cannot see this at all, and
    # a wet one sees it only through its film.
    #
    # Guarded, because these live on `Light` in some Blender versions and have
    # moved in others — and a light that silently kept its diffuse contribution
    # would be the fill this replaces, wearing a different name. A build that
    # cannot exclude it is told rather than left to render something wrong.
    missing = [
        name
        for name in ("diffuse_factor", "specular_factor")
        if not hasattr(data, name)
    ]
    if missing:
        print(
            f"[terrain_cycles] this Blender has no {missing}; the reflection "
            "light would light the ground diffusely, so it is not added"
        )
        bpy.data.lights.remove(data)
        return None
    data.diffuse_factor = 0.0
    data.specular_factor = 1.0
    if hasattr(data, "volume_factor"):
        data.volume_factor = 0.0

    obj = bpy.data.objects.new("reflection", data)
    bpy.context.collection.objects.link(obj)

    # The camera's azimuth plus half a turn, at the camera's own elevation.
    elevation = np.radians(REFLECTION_ELEVATION_DEG)
    azimuth = np.radians(REFLECTION_BEARING_DEG)
    direction = Vector(
        (
            np.cos(elevation) * np.cos(azimuth),
            np.cos(elevation) * np.sin(azimuth),
            np.sin(elevation),
        )
    )
    obj.rotation_mode = "QUATERNION"
    obj.rotation_quaternion = direction.to_track_quat("Z", "Y")
    obj.location = direction * 100.0
    return obj


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


def check_version(spec):
    """Refuse a package this build does not understand.

    A version mismatch is not a warning. The failure mode of reading a newer
    package with an older reader is not an error message — it is a plausible
    picture with a section silently missing from it, and a missing flower looks
    exactly like a flower that was never placed.
    """
    version = spec.get("version")
    if version is None:
        raise SystemExit(
            "scene.json declares no version; this build reads "
            f"{SCENE_FORMAT_VERSION}"
        )
    if version != SCENE_FORMAT_VERSION:
        raise SystemExit(
            f"scene.json is format version {version}; this build reads "
            f"{SCENE_FORMAT_VERSION}. Rebuild the exporter or the renderer, "
            "do not render it anyway."
        )


def render_one(header_path, output):
    started = time.time()

    with open(header_path, "r", encoding="utf-8") as handle:
        spec = json.load(handle)
    scene_dir = os.path.dirname(os.path.abspath(header_path))
    check_version(spec)

    clear_scene()
    build_world(spec["sky"])
    build_sun(spec["sun"])
    build_reflection_light(spec["sun"])
    build_camera(spec["camera"], spec["page"])
    # The soils, put where the material builders can reach them. A soil fragment
    # is a piece of the ground and takes its colour from the ground's own
    # palette — see `soil_fragment_material` — so the ground spec has to be
    # visible when the secondary materials are built, not only when the ground
    # mesh is.
    spec["soils"] = spec["ground"].get("materials", [])
    ground = build_ground(scene_dir, spec["ground"])
    blades = build_blades(scene_dir, spec["blades"], spec)
    secondary = build_secondary(scene_dir, spec.get("secondary"), spec)
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
    _ = secondary

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

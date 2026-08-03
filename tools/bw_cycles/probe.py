"""Discover what this Blender actually offers, rather than guessing.

Run with:
    blender --background --factory-startup --python tools/bw_cycles/probe.py
"""

import sys

import bpy

print("=" * 60)
print("BLENDER", bpy.app.version_string, "| API", bpy.app.version)
print("=" * 60)

# --- Cycles and the devices it can see -------------------------------------
try:
    prefs = bpy.context.preferences.addons["cycles"].preferences
    print("compute device types:", [t[0] for t in prefs.get_device_types(bpy.context)])
    for backend in ("METAL", "CUDA", "OPTIX", "HIP", "ONEAPI"):
        try:
            prefs.compute_device_type = backend
        except TypeError:
            continue
        prefs.get_devices()
        names = [(d.name, d.type, d.use) for d in prefs.devices]
        print(f"  {backend}: {names}")
except Exception as error:  # noqa: BLE001
    print("cycles prefs failed:", error)

# --- Hair curves: the primitive grass blades want --------------------------
print("-" * 60)
print("hair_curves collection present:", hasattr(bpy.data, "hair_curves"))
try:
    curves = bpy.data.hair_curves.new("probe")
    print("Curves datablock:", type(curves).__name__)
    print("  has add_curves:", hasattr(curves, "add_curves"))
    print("  attributes API:", hasattr(curves, "attributes"))
    if hasattr(curves, "add_curves"):
        curves.add_curves([4, 4])
        print("  after add_curves -> curves:", len(curves.curves), "points:", len(curves.points))
        curves.points.foreach_set("position", [0.0] * (len(curves.points) * 3))
        print("  foreach_set position: ok")
        try:
            curves.points.foreach_set("radius", [0.01] * len(curves.points))
            print("  foreach_set radius: ok")
        except Exception as error:  # noqa: BLE001
            print("  radius via foreach_set failed:", error)
            print("  point attribute names:", [a.name for a in curves.attributes])
    print("  attribute domains:", [(a.name, a.domain, a.data_type) for a in curves.attributes])
except Exception as error:  # noqa: BLE001
    print("hair curves failed:", error)

# --- Render settings we depend on ------------------------------------------
print("-" * 60)
scene = bpy.context.scene
scene.render.engine = "CYCLES"
cycles = scene.cycles
for name in (
    "samples",
    "use_denoising",
    "denoiser",
    "use_adaptive_sampling",
    "adaptive_threshold",
    "max_bounces",
    "diffuse_bounces",
    "transmission_bounces",
    "device",
    "seed",
    "use_animated_seed",
    "hair_shape",
    "curve_shape",
    "hair_subdivisions",
):
    print(f"  cycles.{name}:", getattr(cycles, name, "<missing>"))

print("  curves settings:", getattr(scene, "cycles_curves", "<missing>"))
if hasattr(scene, "cycles_curves"):
    cc = scene.cycles_curves
    for name in ("shape", "subdivisions", "cull_backfacing"):
        print(f"    cycles_curves.{name}:", getattr(cc, name, "<missing>"))

print("  pixel_aspect_x/y:", scene.render.pixel_aspect_x, scene.render.pixel_aspect_y)
print("  view_transform:", scene.view_settings.view_transform)
print("  available view transforms:", [
    t.name for t in scene.display_settings.bl_rna.properties["display_device"].enum_items
])
try:
    print("  view transform options:", [
        i.identifier
        for i in scene.view_settings.bl_rna.properties["view_transform"].enum_items
    ])
except Exception as error:  # noqa: BLE001
    print("  view transform enum failed:", error)

# --- Principled BSDF socket names (they move between versions) -------------
print("-" * 60)
material = bpy.data.materials.new("probe")
material.use_nodes = True
principled = material.node_tree.nodes.get("Principled BSDF")
if principled:
    print("Principled BSDF inputs:")
    for socket in principled.inputs:
        print(f"    {socket.name!r} ({socket.type})")
else:
    print("no Principled BSDF node found; nodes:", [n.name for n in material.node_tree.nodes])

print("=" * 60)
print("PROBE OK")
sys.stdout.flush()

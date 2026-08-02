//! A grass scene you can look at.
//!
//! Places chunks around the origin, and gives the mouse two ways to disturb
//! them: click to set off a blast, drag with the right button to walk something
//! through the grass. Both go through the same [`disturbance`](crate::disturbance)
//! path a unit will, so what you are looking at is the real system rather than a
//! demo of it.

use bevy::camera::primitives::Aabb;
use bevy::mesh::Mesh2d;
use bevy::prelude::*;
use bevy::sprite_render::MeshMaterial2d;
use bw_core::GridPos;

use crate::blade::CHUNK_METRES;
use crate::chunk::GrassChunk;
use crate::clump;
use crate::disturbance::{GrassEvents, GrassInteractor};
use crate::field::GrassField;
use crate::ground;
use crate::iso;
use crate::material::GrassTextures;
use crate::palette;
use crate::pixel::{GrassCamera, PixelCanvas};

/// How the scene is laid out.
#[derive(Resource, Clone, Copy, Debug)]
pub struct GrassScene {
    /// Metres of grass placed either side of the origin.
    ///
    /// Comfortably larger than a screenful, so the edge of the grass is never
    /// visible and the camera has somewhere to move to.
    pub half_extent: f32,
    /// Seed for blade placement.
    pub seed: u32,
}

impl Default for GrassScene {
    fn default() -> Self {
        Self {
            // Enough to cover the visible ground diamond at the battle camera's
            // height with room to spare. Worth doing the arithmetic rather than
            // guessing: at thirty-two units of view height on a 16:9 window the
            // diamond reaches |X − Y| ≤ 28.4 and |X + Y| ≤ 32, so a corner sits
            // just over thirty metres out along an axis.
            //
            // This is the number that decides how much grass exists, and right
            // now every chunk in it is built at full detail on the first frame
            // because chunk streaming is not wired up yet. Raising it costs
            // memory quadratically.
            half_extent: 32.0,
            seed: 0x6A72_A551,
        }
    }
}

/// Marks the thing the cursor drags through the grass.
#[derive(Component)]
pub struct GrassPointer;

/// Places grass and wires up the mouse.
pub struct GrassScenePlugin;

impl Plugin for GrassScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GrassScene>()
            // The darkest entry in the palette, not soil. Gaps in the canopy
            // have to read as shade *between* blades; anything lighter, or any
            // colour off the palette, and the field reads as sparse grass on a
            // backdrop rather than as dense grass with depth in it.
            .insert_resource(ClearColor(palette::ground()))
            .add_systems(Startup, (spawn_grass, spawn_pointer))
            .add_systems(
                Update,
                (blast_on_click, drag_pointer).in_set(crate::GrassSet::Sources),
            );
    }
}

/// Build a mesh for every chunk in the scene.
/// Everything `spawn_grass` needs to build the field.
///
/// Gathered into one parameter because Bevy will happily take a dozen and the
/// signature stops being readable long before it stops compiling.
#[derive(bevy::ecs::system::SystemParam)]
pub struct GrassAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    clumps: ResMut<'w, Assets<clump::ClumpMaterial>>,
    grounds: ResMut<'w, Assets<ground::GroundMaterial>>,
    atlas: Res<'w, clump::ClumpAtlas>,
    textures: Res<'w, GrassTextures>,
}

fn spawn_grass(
    mut commands: Commands,
    scene: Res<GrassScene>,
    field: Res<GrassField>,
    mut assets: GrassAssets,
) {
    // The ground first, so that whatever shows between the blades is a colour
    // that belongs to the field rather than the window's clear colour.
    commands.spawn((
        Mesh2d(assets.meshes.add(ground::ground_mesh(scene.half_extent))),
        MeshMaterial2d(assets.grounds.add(ground::GroundMaterial {
            settings: ground::GroundSettings::default(),
            state: assets.textures.state.clone(),
        })),
        Transform::default(),
        Name::new("ground"),
    ));

    // Clumps, not ribbons. Every scrap of detail in the field is a baked
    // sprite now — see `crate::clump` — and the ribbon path stays in the tree
    // because its tests and benchmarks still describe the simulation the
    // clumps are driven by.
    let material = assets.clumps.add(clump::ClumpMaterial {
        settings: clump::ClumpSettings::default(),
        atlas: assets.atlas.image.clone(),
        bend: assets.textures.bend.clone(),
    });

    let radius = (scene.half_extent / CHUNK_METRES).ceil() as i32;
    let mut blades = 0u32;
    let mut chunks = 0u32;

    for y in -radius..radius {
        for x in -radius..radius {
            let coord = IVec2::new(x, y);
            let batch = clump::build_chunk(&field, coord, 1.0, scene.seed);
            if batch.is_empty() {
                continue;
            }
            let count = batch.clumps();
            blades += count;
            chunks += 1;

            commands.spawn((
                Mesh2d(assets.meshes.add(batch.into_mesh())),
                MeshMaterial2d(material.clone()),
                Transform::default(),
                // Set by hand rather than computed from the mesh. The vertex
                // shader moves every vertex, so the positions Bevy would
                // measure are the rest pose — correct in shape but a blade's
                // worth too small once the grass leans, which shows up as
                // chunks vanishing at the edge of the view.
                chunk_bounds(coord),
                GrassChunk {
                    coord: GridPos::new(x, y),
                    blade_count: count,
                },
                Name::new(format!("grass chunk {x},{y}")),
            ));
        }
    }

    info!("grass: {blades} clumps across {chunks} chunks");
}

/// Screen-space bounds of a chunk, padded for lean.
fn chunk_bounds(coord: IVec2) -> Aabb {
    let origin = coord.as_vec2() * CHUNK_METRES;
    // Sized from the clumps, which are much larger than a blade ever was and
    // overhang their chunk in every direction. Padding for a blade instead left
    // clumps near a chunk edge culled with it, which showed as faint diagonal
    // seams across the whole field on the chunk lattice.
    let tallest = clump::SIZE.1 * 1.4;

    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for corner in [
        Vec2::ZERO,
        Vec2::new(CHUNK_METRES, 0.0),
        Vec2::new(0.0, CHUNK_METRES),
        Vec2::splat(CHUNK_METRES),
    ] {
        // Both on the ground and at full height, since the projection sends
        // height up the screen and depth is a mix of the two.
        for height in [0.0, tallest] {
            let world = (origin + corner).extend(height);
            let projected = iso::project_to_vertex(world);
            min = min.min(projected);
            max = max.max(projected);
        }
    }

    // A blade laid flat reaches its own length sideways.
    let padding = Vec3::splat(tallest);
    Aabb::from_min_max(min - padding, max + padding)
}

fn spawn_pointer(mut commands: Commands) {
    commands.spawn((
        GrassPointer,
        GrassInteractor {
            // Parked well outside the field until the button goes down, so it
            // is not quietly flattening the middle of the scene.
            previous: Vec2::splat(1.0e6),
            current: Vec2::splat(1.0e6),
            radius: 0.30,
            falloff: 0.34,
            mass: 90.0,
        },
        Name::new("grass pointer"),
    ));
}

/// Type alias for the camera query the pickers share.
type GrassCameras<'w, 's> =
    Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<GrassCamera>>;

/// Where the cursor is on the ground, if it is over the window.
///
/// The extra step through the canvas is not optional. The grass camera renders
/// into a low-resolution image, so its viewport is the canvas rather than the
/// window — hand it a raw cursor position and every click lands at roughly a
/// quarter of the distance from the centre that it should.
fn cursor_ground_position(
    windows: &Query<&Window>,
    cameras: &GrassCameras,
    canvas: &PixelCanvas,
) -> Option<Vec2> {
    let window = windows.iter().next()?;
    let cursor = window.cursor_position()?;
    let (camera, transform) = cameras.iter().next()?;
    let on_canvas = canvas.window_to_canvas(cursor, window.size());
    let screen = camera.viewport_to_world_2d(transform, on_canvas).ok()?;
    Some(iso::unproject_ground(screen))
}

/// Left click sets off a blast.
fn blast_on_click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: GrassCameras,
    canvas: Option<Res<PixelCanvas>>,
    mut events: ResMut<GrassEvents>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(canvas) = canvas.as_deref() else {
        return;
    };
    if let Some(ground) = cursor_ground_position(&windows, &cameras, canvas) {
        events.shockwave(ground);
    }
}

/// Holding the right button drags something heavy through the grass.
fn drag_pointer(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: GrassCameras,
    canvas: Option<Res<PixelCanvas>>,
    mut pointers: Query<&mut GrassInteractor, With<GrassPointer>>,
) {
    let Ok(mut pointer) = pointers.single_mut() else {
        return;
    };
    if !buttons.pressed(MouseButton::Right) {
        pointer.previous = Vec2::splat(1.0e6);
        pointer.current = Vec2::splat(1.0e6);
        return;
    }
    let Some(canvas) = canvas.as_deref() else {
        return;
    };
    let Some(ground) = cursor_ground_position(&windows, &cameras, canvas) else {
        return;
    };
    if pointer.current.x > 1.0e5 {
        // First frame of a drag: start the capsule where the cursor is rather
        // than sweeping it in from a million metres away.
        pointer.current = ground;
    }
    pointer.move_to(ground);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_bounds_contain_the_chunks_ground_corners() {
        let coord = IVec2::new(2, -1);
        let bounds = chunk_bounds(coord);
        let origin = coord.as_vec2() * CHUNK_METRES;
        for corner in [
            Vec2::ZERO,
            Vec2::new(CHUNK_METRES, 0.0),
            Vec2::new(0.0, CHUNK_METRES),
            Vec2::splat(CHUNK_METRES),
        ] {
            let point = iso::project_to_vertex((origin + corner).extend(0.0));
            let min = bounds.min();
            let max = bounds.max();
            assert!(
                point.x >= min.x && point.x <= max.x,
                "{point:?} outside {min:?}..{max:?}"
            );
            assert!(point.y >= min.y && point.y <= max.y);
        }
    }

    #[test]
    fn chunk_bounds_leave_room_for_a_leaning_blade() {
        // The failure this prevents: chunks popping out of view at the screen
        // edge because their bounds only covered the upright rest pose.
        let bounds = chunk_bounds(IVec2::ZERO);
        let upright = iso::project_to_vertex(Vec3::new(0.0, 0.0, clump::SIZE.1));
        assert!(
            bounds.max().y > upright.y,
            "no headroom above the tallest blade"
        );
    }

    #[test]
    fn neighbouring_chunk_bounds_do_not_drift() {
        let a = chunk_bounds(IVec2::ZERO);
        let b = chunk_bounds(IVec2::new(1, 0));
        assert!(b.center.x > a.center.x, "chunks must lay out left to right");
        assert!(
            (a.half_extents - b.half_extents).length() < 1e-4,
            "{:?} vs {:?}",
            a.half_extents,
            b.half_extents
        );
    }
}

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

use crate::blade::{self, CHUNK_METRES};
use crate::chunk::GrassChunk;
use crate::disturbance::{GrassEvents, GrassInteractor};
use crate::field::GrassField;
use crate::iso;
use crate::material::{GrassMaterial, GrassSettings, GrassTextures};

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
            // Enough to cover the visible ground diamond at the default zoom
            // with room to spare. The diamond is much smaller than it looks:
            // the projection compresses depth, so a screenful of grass is about
            // seventeen metres across each world axis.
            half_extent: 10.0,
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
            // Soil, so the gaps between blades read as ground rather than as
            // holes in the world. Even a well-covered canopy shows some of what
            // is under it, and what shows needs to be the right colour.
            .insert_resource(ClearColor(Color::srgb(0.086, 0.075, 0.055)))
            .add_systems(Startup, (spawn_grass, spawn_pointer))
            .add_systems(
                Update,
                (blast_on_click, drag_pointer).in_set(crate::GrassSet::Sources),
            );
    }
}

/// Build a mesh for every chunk in the scene.
fn spawn_grass(
    mut commands: Commands,
    scene: Res<GrassScene>,
    field: Res<GrassField>,
    textures: Res<GrassTextures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GrassMaterial>>,
) {
    let material = materials.add(GrassMaterial {
        settings: GrassSettings::default(),
        bend: textures.bend.clone(),
        state: textures.state.clone(),
    });

    let radius = (scene.half_extent / CHUNK_METRES).ceil() as i32;
    let mut blades = 0u32;
    let mut chunks = 0u32;

    for y in -radius..radius {
        for x in -radius..radius {
            let coord = IVec2::new(x, y);
            let batch = blade::build_chunk(&field, coord, 1.0, scene.seed);
            if batch.is_empty() {
                continue;
            }
            let count = batch.blades();
            blades += count;
            chunks += 1;

            commands.spawn((
                Mesh2d(meshes.add(batch.into_mesh())),
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

    info!("grass: {blades} blades across {chunks} chunks");
}

/// Screen-space bounds of a chunk, padded for lean.
fn chunk_bounds(coord: IVec2) -> Aabb {
    let origin = coord.as_vec2() * CHUNK_METRES;
    let tallest = blade::LENGTH_RANGE.1;

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

/// Where the cursor is on the ground, if it is over the window.
fn cursor_ground_position(
    windows: &Query<&Window>,
    cameras: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Vec2> {
    let cursor = windows.iter().find_map(|window| window.cursor_position())?;
    let (camera, transform) = cameras.iter().next()?;
    let screen = camera.viewport_to_world_2d(transform, cursor).ok()?;
    Some(iso::unproject_ground(screen))
}

/// Left click sets off a blast.
fn blast_on_click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut events: ResMut<GrassEvents>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    if let Some(ground) = cursor_ground_position(&windows, &cameras) {
        events.shockwave(ground);
    }
}

/// Holding the right button drags something heavy through the grass.
fn drag_pointer(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
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
    let Some(ground) = cursor_ground_position(&windows, &cameras) else {
        return;
    };
    if pointer.current.x > 1.0e5 {
        // First frame of a drag: start the capsule where the cursor is rather
        // than sweeping it in from a million metres away.
        pointer.current = ground;
    }
    pointer.move_to(ground);
}

/// A camera framed for looking at grass.
///
/// `view_height` is in metres of world height, so it can be reasoned about
/// against the blades: at nine metres a knee-high blade is a comfortable
/// thirty-odd pixels tall on a 1080p window.
pub fn grass_camera(view_height: f32) -> impl Bundle {
    (
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                viewport_height: view_height,
            },
            ..OrthographicProjection::default_2d()
        }),
    )
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
        let upright = iso::project_to_vertex(Vec3::new(0.0, 0.0, blade::LENGTH_RANGE.1));
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

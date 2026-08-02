//! Iterate on the grass renderer without launching the game.
//!
//! `cargo run -p bw_grass --example grass_sandbox`
//!
//! Draws the chunk grid as flat quads tinted by density and shifted by the wind
//! field, which is enough to see chunking, LOD and wind behaving before any
//! shader work exists.

use bevy::prelude::*;
use bw_core::{Grid, GridDims, GridPos, real_from_int};
use bw_grass::chunk::{CHUNK_CELLS, chunks_covering};
use bw_grass::{GrassPlugin, WindField, lod_for_distance};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Backseat Warlord — grass sandbox".to_string(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(GrassPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, sway)
        .run();
}

#[derive(Component)]
struct ChunkQuad {
    origin: Vec2,
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);

    let grid = Grid::centered(GridDims::new(256, 256), real_from_int(1));
    let cell_px = 3.0;
    let chunk_px = CHUNK_CELLS as f32 * cell_px;

    for coord in chunks_covering(&grid) {
        let origin = Vec2::new(coord.x as f32 * chunk_px, coord.y as f32 * chunk_px)
            - Vec2::splat(128.0 * cell_px);
        let distance = origin.length();
        let lod = lod_for_distance(distance, 40.0);
        if lod == bw_grass::GrassLod::Culled {
            continue;
        }

        // Tint by detail tier so the LOD bands are visible at a glance.
        let green = 0.35 + 0.35 * lod.blade_fraction();
        commands.spawn((
            Sprite::from_color(Color::srgb(0.12, green, 0.18), Vec2::splat(chunk_px - 1.0)),
            Transform::from_translation(origin.extend(0.0)),
            ChunkQuad { origin },
            grid_marker(coord),
        ));
    }
}

fn grid_marker(coord: GridPos) -> Name {
    Name::new(format!("chunk {},{}", coord.x, coord.y))
}

/// Nudge each quad by the wind field, standing in for per-blade sway.
fn sway(wind: Res<WindField>, mut quads: Query<(&ChunkQuad, &mut Transform)>) {
    for (quad, mut transform) in &mut quads {
        let offset = wind.sample(quad.origin) * 6.0;
        transform.translation.x = quad.origin.x + offset.x;
        transform.translation.y = quad.origin.y + offset.y;
    }
}

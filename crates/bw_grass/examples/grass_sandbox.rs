//! The grass on its own, in a window.
//!
//! `cargo run --release -p bw_grass --example grass_sandbox`
//!
//! - **Arrow keys / WASD** pan the camera.
//! - **Mouse wheel** zooms.
//! - **F12** saves a screenshot.
//!
//! Panning is the thing worth doing here that the headless baker cannot show:
//! pages are baked independently, and a long diagonal drive is what proves they
//! agree along their edges.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::WindowResolution;
use bw_grass::{GrassPlugin, plugin::grass_camera};

/// Metres of world visible vertically. The height the game frames a battle at.
const RTS_HEIGHT: f32 = 26.0;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Backseat Warlord — grass sandbox".to_string(),
                        resolution: WindowResolution::new(1920, 1080),
                        ..default()
                    }),
                    ..default()
                })
                // Absolute, resolved at compile time. An example's asset root
                // is neither the workspace nor the crate — it is wherever the
                // built binary happens to sit, which differs between `cargo run`
                // and running `target/release/examples/...` by hand. A relative
                // path works in one and not the other, and the failure is a
                // single error line in a log that scrolls past while the grass
                // bakes perfectly and draws nothing at all.
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").to_string(),
                    ..default()
                }),
        )
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(GrassPlugin)
        .insert_resource(Zoom(RTS_HEIGHT))
        .add_systems(Startup, setup)
        .add_systems(Update, (pan, zoom, capture, report, scripted_capture))
        .run();
}

#[derive(Resource)]
struct Zoom(f32);

fn setup(mut commands: Commands) {
    commands.spawn(grass_camera(RTS_HEIGHT));
}

fn pan(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    zoom: Res<Zoom>,
    mut cameras: Query<&mut Transform, With<Camera2d>>,
) {
    let mut direction = Vec2::ZERO;
    for (key, offset) in [
        (KeyCode::ArrowLeft, Vec2::NEG_X),
        (KeyCode::KeyA, Vec2::NEG_X),
        (KeyCode::ArrowRight, Vec2::X),
        (KeyCode::KeyD, Vec2::X),
        (KeyCode::ArrowUp, Vec2::Y),
        (KeyCode::KeyW, Vec2::Y),
        (KeyCode::ArrowDown, Vec2::NEG_Y),
        (KeyCode::KeyS, Vec2::NEG_Y),
    ] {
        if keys.pressed(key) {
            direction += offset;
        }
    }
    if direction == Vec2::ZERO {
        return;
    }
    // Scaled by zoom, so panning covers the same fraction of the screen per
    // second however far out the camera is.
    let step = direction.normalize() * time.delta_secs() * zoom.0 * 0.55;
    for mut transform in &mut cameras {
        transform.translation += step.extend(0.0);
    }
}

fn zoom(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut zoom: ResMut<Zoom>,
    mut cameras: Query<&mut Projection, With<Camera2d>>,
) {
    let change: f32 = wheel.read().map(|event| -event.y).sum();
    if change == 0.0 {
        return;
    }
    zoom.0 = (zoom.0 * (1.0 + change * 0.08)).clamp(2.0, 64.0);
    for mut projection in &mut cameras {
        if let Projection::Orthographic(orthographic) = projection.as_mut() {
            orthographic.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                viewport_height: zoom.0,
            };
        }
    }
}

fn capture(keys: Res<ButtonInput<KeyCode>>, mut commands: Commands) {
    if keys.just_pressed(KeyCode::F12) {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk("grass.png"));
    }
}

/// Grab one frame at a set time, then quit.
///
/// `TERRAIN_CAPTURE=path.png TERRAIN_CAPTURE_AFTER=4` photographs the running renderer
/// without a person sitting in front of it. Worth having for its own sake: the
/// headless baker proves the plate is right, and this is the only thing that
/// proves the plate reaches the screen — a separate claim, and the one that has
/// quietly been false while every other check passed.
fn scripted_capture(
    time: Res<Time>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut stage: Local<u32>,
) {
    let Ok(path) = std::env::var("TERRAIN_CAPTURE") else {
        return;
    };
    let at: f32 = std::env::var("TERRAIN_CAPTURE_AFTER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4.0);
    let now = time.elapsed_secs();
    if *stage == 0 && now >= at {
        *stage = 1;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if *stage == 1 && now >= at + 1.5 {
        exit.write(AppExit::Success);
    }
}

/// Print the numbers that say whether this is behaving, once a second.
fn report(
    time: Res<Time>,
    diagnostics: Res<DiagnosticsStore>,
    zoom: Res<Zoom>,
    pages: Query<(), With<Mesh2d>>,
    mut next: Local<f32>,
) {
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + 1.0;
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    info!(
        "{fps:.0} fps | {} pages | {:.1} m tall view",
        pages.iter().count(),
        zoom.0
    );
}

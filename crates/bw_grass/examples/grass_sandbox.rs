//! Iterate on the grass without launching the game.
//!
//! `cargo run -p bw_grass --example grass_sandbox`
//!
//! - **Left click** sets off a blast.
//! - **Right drag** walks something heavy through the grass.
//! - **Space** stands everything back up.
//! - **Arrow keys** turn the wind; **-** and **=** change its strength.
//! - **F12** saves a screenshot.
//!
//! `BW_CAPTURE=path.png BW_CAPTURE_AFTER=2.5 cargo run -p bw_grass --example
//! grass_sandbox` runs it headlessly enough to grab a frame and exit, which is
//! how the look gets checked without a person sitting in front of it.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bw_grass::disturbance::GrassEvents;
use bw_grass::disturbance::GrassInteractor;
use bw_grass::field::GrassField;
use bw_grass::scene::{GrassPointer, grass_camera};
use bw_grass::{GrassPlugin, GrassScenePlugin, GrassSet, WindField};

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Backseat Warlord — grass sandbox".to_string(),
                        ..default()
                    }),
                    ..default()
                })
                // An example's asset root is its own crate directory, not the
                // workspace. Without this the shader silently fails to load and
                // the grass simulates perfectly while drawing nothing at all.
                .set(AssetPlugin {
                    file_path: "../../assets".to_string(),
                    ..default()
                }),
        )
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins((GrassPlugin, GrassScenePlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, (control_wind, reset_field, report, capture, zoom))
        // Explicitly after the mouse handlers and before the stamp: the scripted
        // drag moves the same pointer the mouse does, and whichever runs last
        // wins. Left unordered it silently loses about half the time.
        .add_systems(
            Update,
            scripted_capture
                .after(GrassSet::Sources)
                .before(GrassSet::Stamp),
        )
        .run();
}

/// Metres of world visible vertically. Close enough to read one blade.
#[derive(Resource)]
struct Zoom(f32);

fn setup(mut commands: Commands) {
    // Five metres of visible height makes a knee-high blade about sixty pixels
    // tall, which is the point at which you can actually watch one bend and
    // spring back rather than judging the field as a texture.
    commands.spawn(grass_camera(5.0));
    commands.insert_resource(Zoom(5.0));
}

/// Mouse wheel zooms, so the same scene serves both close inspection and
/// judging how the field reads at gameplay distance.
fn zoom(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut zoom: ResMut<Zoom>,
    mut cameras: Query<&mut Projection>,
) {
    let mut change = 0.0;
    for event in wheel.read() {
        change -= event.y;
    }
    if change == 0.0 {
        return;
    }
    zoom.0 = (zoom.0 * (1.0 + change * 0.08)).clamp(1.5, 26.0);
    for mut projection in &mut cameras {
        *projection = bw_render_projection(zoom.0);
    }
}

fn bw_render_projection(view_height: f32) -> Projection {
    Projection::Orthographic(OrthographicProjection {
        scaling_mode: bevy::camera::ScalingMode::FixedVertical {
            viewport_height: view_height,
        },
        ..OrthographicProjection::default_2d()
    })
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
/// The point is repeatability: the same seed, the same wind clock and the same
/// blast produce the same picture every run, so two screenshots taken weeks
/// apart are actually comparable.
fn scripted_capture(
    time: Res<Time>,
    mut commands: Commands,
    mut events: ResMut<GrassEvents>,
    mut pointers: Query<&mut GrassInteractor, With<GrassPointer>>,
    mut exit: MessageWriter<AppExit>,
    mut stage: Local<u32>,
) {
    let Ok(path) = std::env::var("BW_CAPTURE") else {
        return;
    };

    // Walk something heavy across the field, so the shot shows a trail rather
    // than a field nothing has been through.
    if std::env::var("BW_CAPTURE_DRAG").is_ok()
        && let Ok(mut pointer) = pointers.single_mut()
    {
        let along = (time.elapsed_secs() * 1.1 - 2.0).clamp(-2.0, 2.0);
        let target = Vec2::new(along, along * 0.35);
        if pointer.current.x > 1.0e5 {
            pointer.current = target;
        }
        pointer.move_to(target);
    }
    let at: f32 = std::env::var("BW_CAPTURE_AFTER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.5);
    let lead: f32 = std::env::var("BW_CAPTURE_LEAD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.45);
    let now = time.elapsed_secs();

    // A blast shortly before the shot, so the picture catches the ring
    // mid-flight rather than a field that has already recovered.
    if *stage == 0 && now >= (at - lead).max(0.0) {
        *stage = 1;
        if std::env::var("BW_CAPTURE_BLAST").is_ok() {
            events.shockwave(Vec2::ZERO);
        }
    }
    if *stage == 1 && now >= at {
        *stage = 2;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if *stage == 2 && now >= at + 1.0 {
        exit.write(AppExit::Success);
    }
}

fn control_wind(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut wind: ResMut<WindField>) {
    let mut turn = 0.0;
    if keys.pressed(KeyCode::ArrowLeft) {
        turn += 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        turn -= 1.0;
    }
    if turn != 0.0 {
        let angle = turn * time.delta_secs();
        let (sin, cos) = angle.sin_cos();
        let d = wind.direction;
        wind.direction = Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos).normalize();
    }

    let mut change = 0.0;
    if keys.pressed(KeyCode::Equal) {
        change += 1.0;
    }
    if keys.pressed(KeyCode::Minus) {
        change -= 1.0;
    }
    if change != 0.0 {
        let step = change * time.delta_secs() * 3.0;
        wind.speed = (wind.speed + step).clamp(0.0, 14.0);
        wind.gust_strength = (wind.gust_strength + step * 1.3).clamp(0.0, 18.0);
    }
}

fn reset_field(keys: Res<ButtonInput<KeyCode>>, mut field: ResMut<GrassField>) {
    if keys.just_pressed(KeyCode::Space) {
        field.reset();
    }
}

/// Print the numbers that say whether this is behaving, once a second.
fn report(
    time: Res<Time>,
    diagnostics: Res<DiagnosticsStore>,
    field: Res<GrassField>,
    wind: Res<WindField>,
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
        "{fps:.0} fps | wind {:.1} m/s | mean bend {:.1} deg | max bend {:.1} deg | crushed {:.3}",
        wind.speed,
        field.mean_bend().to_degrees(),
        field.max_bend().to_degrees(),
        field.mean_compaction(),
    );
}

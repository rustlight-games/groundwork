//! The terrain, live, in a window.
//!
//! ```sh
//! cargo run --release -p terrain_preview
//! cargo run --release -p terrain_preview -- --view 55 --seed 12
//! ```
//!
//! - **Arrow keys / WASD** pan.
//! - **Mouse wheel** zooms.
//! - **1 / 2 / 3** jump to the close-up, the standard and the wide framing.
//! - **G** toggles the page-boundary overlay.
//! - **F12** saves a screenshot.
//!
//! Panning is the thing worth doing here that no headless bake can show: pages
//! are baked independently, and a long diagonal drive is what proves they agree
//! along their edges. A seam that a static plate cannot contain shows up
//! immediately as a line that moves with the camera.
//!
//! ## Why this is a tool and not an example
//!
//! It replaces `grass_sandbox`, and the move is the point rather than a tidy-up.
//! An example lives inside a crate and is built against that crate's dependency
//! graph; this has to demonstrate that the terrain renders with **no game in the
//! graph at all**, which is a claim about the workspace and cannot be made from
//! inside one of its members.
//!
//! `TERRAIN_CAPTURE=path.png TERRAIN_CAPTURE_AFTER=4` photographs the running
//! renderer without a person sitting in front of it. Worth having for its own
//! sake: a headless bake proves the plate is right, and this is the only thing
//! that proves the plate reaches the screen — a separate claim, and the one that
//! has quietly been false while every other check passed.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::WindowResolution;
use terrain_bevy::{GrassPlugin, plugin::grass_camera};

/// The framings the preview offers, in world metres visible vertically.
///
/// Three rather than a free slider, because the useful judgements are made at
/// specific scales and a camera parked between them answers no question. Close
/// is where an individual blade is legible and the look was tuned; standard is
/// the framing the art is composed for; wide is where every level-of-detail
/// decision has to hold, and the one most likely to be skipped.
const FRAMINGS: [(&str, f32); 3] = [("close", 13.0), ("standard", 26.0), ("wide", 55.0)];

/// Which framing the window opens at.
const OPENS_AT: usize = 1;

fn main() {
    let options = Options::parse();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: format!("terrain preview — seed {}", options.seed),
                        resolution: WindowResolution::new(1920, 1080),
                        ..default()
                    }),
                    ..default()
                })
                // Absolute, resolved at compile time. A tool's asset root is
                // neither the workspace nor the crate — it is wherever the built
                // binary happens to sit, which differs between `cargo run` and
                // running the binary by hand. A relative path works in one and
                // not the other, and the failure is a single error line in a log
                // that scrolls past while the terrain bakes perfectly and draws
                // nothing at all.
                .set(AssetPlugin {
                    file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").to_string(),
                    ..default()
                }),
        )
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_plugins(GrassPlugin)
        .insert_resource(View {
            metres: options.view,
        })
        .insert_resource(options)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                pan,
                zoom,
                framing_presets,
                capture,
                report,
                scripted_capture,
            ),
        )
        .run();
}

/// How much world the camera shows vertically, in metres.
#[derive(Resource)]
struct View {
    metres: f32,
}

#[derive(Resource, Debug)]
struct Options {
    view: f32,
    seed: u64,
}

impl Options {
    fn parse() -> Self {
        let mut options = Self {
            view: FRAMINGS[OPENS_AT].1,
            seed: 0,
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || arguments.next().unwrap_or_default();
            match argument.as_str() {
                "--view" => options.view = value().parse().unwrap_or(options.view),
                "--seed" => options.seed = value().parse().unwrap_or(options.seed),
                "--help" | "-h" => {
                    println!("terrain_preview [--view METRES] [--seed N]");
                    std::process::exit(0);
                }
                other => eprintln!("ignoring unknown argument {other}"),
            }
        }
        options
    }
}

fn setup(mut commands: Commands, view: Res<View>) {
    commands.spawn(grass_camera(view.metres));
}

fn pan(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    view: Res<View>,
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
    // Scaled by the framing, so panning covers the same fraction of the screen
    // per second however far out the camera is.
    let step = direction.normalize() * time.delta_secs() * view.metres * 0.55;
    for mut transform in &mut cameras {
        transform.translation += step.extend(0.0);
    }
}

fn zoom(
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut view: ResMut<View>,
    cameras: Query<&mut Projection, With<Camera2d>>,
) {
    let change: f32 = wheel.read().map(|event| -event.y).sum();
    if change == 0.0 {
        return;
    }
    view.metres = (view.metres * (1.0 + change * 0.08)).clamp(2.0, 96.0);
    apply_framing(view.metres, cameras);
}

/// Jump straight to one of the three framings worth judging at.
fn framing_presets(
    keys: Res<ButtonInput<KeyCode>>,
    mut view: ResMut<View>,
    cameras: Query<&mut Projection, With<Camera2d>>,
) {
    for (key, index) in [
        (KeyCode::Digit1, 0),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
    ] {
        if keys.just_pressed(key) {
            let (name, metres) = FRAMINGS[index];
            view.metres = metres;
            info!("framing: {name} ({metres} m)");
            apply_framing(metres, cameras);
            return;
        }
    }
}

fn apply_framing(metres: f32, mut cameras: Query<&mut Projection, With<Camera2d>>) {
    for mut projection in &mut cameras {
        if let Projection::Orthographic(orthographic) = projection.as_mut() {
            orthographic.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                viewport_height: metres,
            };
        }
    }
}

fn capture(keys: Res<ButtonInput<KeyCode>>, mut commands: Commands) {
    if keys.just_pressed(KeyCode::F12) {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk("terrain.png"));
    }
}

/// Grab one frame at a set time, then quit.
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
    view: Res<View>,
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
        view.metres
    );
}

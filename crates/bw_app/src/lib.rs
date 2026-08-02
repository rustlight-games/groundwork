//! Composition root.
//!
//! The only crate that knows about all the others. Everything below it depends
//! downward or sideways, which is what keeps the headless trainer able to link
//! the simulation and the content plugins without dragging in a renderer.

#![forbid(unsafe_code)]

pub mod registries;

use bevy::prelude::*;
use bw_grass::{GrassPlugin, GrassScenePlugin, grass_camera};
use bw_render::{BattleCamera, RenderPlugin};
use bw_ui::UiPlugin;

pub use bw_ui::GameState;
pub use registries::{build_effect_registry, build_generator_registry};

/// The whole game.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((UiPlugin, RenderPlugin, GrassPlugin, GrassScenePlugin))
            .add_systems(Startup, (spawn_camera, finish_boot));
    }
}

/// Spawn the one camera the game has.
///
/// Composed here rather than inside either plugin because it is two crates'
/// business at once: `bw_grass` decides that it renders into a low-resolution
/// canvas with multisampling and tonemapping off, and `bw_render` decides how
/// much ground it frames. Only the composition root knows both.
fn spawn_camera(mut commands: Commands) {
    let framing = BattleCamera::default();
    commands.spawn((grass_camera(framing.view_height), framing));
}

/// Leave the boot screen.
///
/// Goes straight to the battlefield rather than to the menu. There is no roster
/// to draft and no fight to start yet, and the terrain is the thing currently
/// worth looking at — click it to set off a blast, drag with the right button
/// to walk something through the grass.
fn finish_boot(mut next: ResMut<NextState<GameState>>) {
    next.set(GameState::Battle);
}

/// Build the `App` the binary runs.
pub fn build_app() -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Backseat Warlord".to_string(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(GamePlugin);
    app
}

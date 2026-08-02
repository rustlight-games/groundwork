//! Composition root.
//!
//! The only crate that knows about all the others. Everything below it depends
//! downward or sideways, which is what keeps the headless trainer able to link
//! the simulation and the content plugins without dragging in a renderer.

#![forbid(unsafe_code)]

pub mod registries;

use bevy::prelude::*;
use bw_grass::GrassPlugin;
use bw_render::RenderPlugin;
use bw_ui::UiPlugin;

pub use bw_ui::GameState;
pub use registries::{build_effect_registry, build_generator_registry};

/// The whole game.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((UiPlugin, RenderPlugin, GrassPlugin))
            .add_systems(Startup, finish_boot);
    }
}

/// Leave the boot screen.
///
/// Placeholder: this is where content loading and model loading will be waited
/// on. It advances immediately for now so the binary reaches a real screen.
fn finish_boot(mut next: ResMut<NextState<GameState>>) {
    next.set(GameState::MainMenu);
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

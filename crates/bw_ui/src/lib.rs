//! Screens and HUD.
//!
//! [`GameState`] lives here rather than in `bw_app` for a dependency reason:
//! `bw_app` composes this crate, so the state it gates screens on has to be
//! defined below it. `bw_app` re-exports it, and that is the name the rest of
//! the project should use.

#![forbid(unsafe_code)]

pub mod hud;
pub mod menu;
pub mod state;

use bevy::prelude::*;

pub use state::GameState;

/// Adds every screen.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .add_plugins((menu::MenuPlugin, hud::HudPlugin));
    }
}

/// Despawn everything tagged with `T`. Attached to `OnExit` for each screen so
/// that leaving a screen cannot leak its entities into the next one.
pub fn despawn_all<T: Component>(mut commands: Commands, entities: Query<Entity, With<T>>) {
    for entity in &entities {
        commands.entity(entity).despawn();
    }
}

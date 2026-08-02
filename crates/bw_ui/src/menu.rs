//! Main menu.
//!
//! Skeleton: one label and the state transition wiring. The point of it
//! existing now is that the enter/exit despawn pattern is established, so
//! screens added later cannot leak entities.

use bevy::prelude::*;
use bevy::text::FontSize;

use crate::{GameState, despawn_all};

/// Tags everything belonging to the menu screen.
#[derive(Component)]
pub struct MenuScreen;

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::MainMenu), spawn_menu)
            .add_systems(OnExit(GameState::MainMenu), despawn_all::<MenuScreen>);
    }
}

fn spawn_menu(mut commands: Commands) {
    commands.spawn((
        MenuScreen,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        children![(
            Text::new("Backseat Warlord"),
            TextFont {
                font_size: FontSize::Px(48.0),
                ..default()
            },
        )],
    ));
}

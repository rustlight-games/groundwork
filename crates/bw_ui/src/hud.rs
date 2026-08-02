//! Battle HUD.

use bevy::prelude::*;
use bevy::text::FontSize;

use crate::{GameState, despawn_all};

/// Tags everything belonging to the battle HUD.
#[derive(Component)]
pub struct HudScreen;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Battle), spawn_hud)
            .add_systems(OnExit(GameState::Battle), despawn_all::<HudScreen>);
    }
}

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        HudScreen,
        Node {
            width: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
        children![(
            Text::new("Battle"),
            TextFont {
                font_size: FontSize::Px(20.0),
                ..default()
            }
        )],
    ));
}

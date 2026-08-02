//! Screen flow.

use bevy::prelude::*;

/// Which screen the game is on.
///
/// The simulation only advances in [`GameState::Battle`], which is what stops a
/// paused menu from quietly running the fight in the background.
#[derive(States, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GameState {
    /// Loading content and models.
    #[default]
    Boot,
    MainMenu,
    /// Choosing a roster before the fight.
    Draft,
    Battle,
    Results,
}

impl GameState {
    /// Whether the battle simulation should tick.
    pub fn simulation_runs(self) -> bool {
        matches!(self, GameState::Battle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_simulation_only_runs_in_battle() {
        assert!(GameState::Battle.simulation_runs());
        for state in [
            GameState::Boot,
            GameState::MainMenu,
            GameState::Draft,
            GameState::Results,
        ] {
            assert!(
                !state.simulation_runs(),
                "{state:?} must not tick the simulation"
            );
        }
    }

    #[test]
    fn boot_is_the_default() {
        assert_eq!(GameState::default(), GameState::Boot);
    }
}

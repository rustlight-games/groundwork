//! The action space.
//!
//! Movement and ability choice are flattened into a single discrete index,
//! because DQN scores one value per action and a single head over the product
//! is simpler than coordinating two heads. The product is small enough that
//! this costs nothing: eleven movement options times five ability options is
//! fifty-five outputs.

use bw_sim::components::{Intent, MoveIntent};
use serde::{Deserialize, Serialize};

/// An index into the network's output layer.
pub type ActionIndex = u16;

/// Movement choices: hold, engage, retreat, and eight compass directions.
pub const MOVE_OPTIONS: usize = 11;

/// Ability choices: do nothing, or use one of four slots.
pub const ABILITY_OPTIONS: usize = 1 + bw_sim::components::AbilitySlots::MAX_SLOTS;

/// A decoded action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub movement: MoveIntent,
    /// `None` to use no ability, otherwise a slot index.
    pub ability: Option<u8>,
}

impl From<Action> for Intent {
    fn from(action: Action) -> Self {
        Intent {
            movement: action.movement,
            ability: action.ability,
        }
    }
}

/// The flattened discrete action space.
///
/// A unit struct rather than a set of free functions so that its size is
/// recorded in [`ModelManifest`](crate::ModelManifest) and checked at load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSpace;

impl ActionSpace {
    /// Number of network outputs.
    pub const SIZE: usize = MOVE_OPTIONS * ABILITY_OPTIONS;

    pub const fn size() -> usize {
        Self::SIZE
    }

    /// Decode an index. Out-of-range indices clamp to a safe hold, rather than
    /// panicking — a freshly initialised network can emit anything.
    pub fn decode(index: ActionIndex) -> Action {
        let index = (index as usize).min(Self::SIZE - 1);
        let movement = match index / ABILITY_OPTIONS {
            0 => MoveIntent::Hold,
            1 => MoveIntent::Engage,
            2 => MoveIntent::Retreat,
            other => MoveIntent::Direction((other - 3) as u8),
        };
        let ability_slot = index % ABILITY_OPTIONS;
        let ability = (ability_slot > 0).then(|| (ability_slot - 1) as u8);
        Action { movement, ability }
    }

    pub fn encode(action: Action) -> ActionIndex {
        let movement = match action.movement {
            MoveIntent::Hold => 0,
            MoveIntent::Engage => 1,
            MoveIntent::Retreat => 2,
            MoveIntent::Direction(d) => 3 + (d as usize % 8),
        };
        let ability = action.ability.map_or(0, |slot| (slot as usize % 4) + 1);
        (movement * ABILITY_OPTIONS + ability) as ActionIndex
    }

    /// Which actions a unit can currently take.
    ///
    /// Used to mask the network's output before taking an argmax. Without a
    /// mask the network spends much of training learning not to press buttons
    /// that do nothing, which is effort better spent elsewhere.
    pub fn mask(available_abilities: &[bool], out: &mut Vec<bool>) {
        out.clear();
        out.resize(Self::SIZE, true);
        for (index, allowed) in out.iter_mut().enumerate() {
            let slot = index % ABILITY_OPTIONS;
            if slot > 0 {
                *allowed = available_abilities.get(slot - 1).copied().unwrap_or(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_index_round_trips() {
        for index in 0..ActionSpace::SIZE as ActionIndex {
            assert_eq!(ActionSpace::encode(ActionSpace::decode(index)), index);
        }
    }

    #[test]
    fn out_of_range_indices_clamp_rather_than_panic() {
        // An untrained network's argmax is arbitrary, so this must be safe.
        let action = ActionSpace::decode(ActionIndex::MAX);
        assert_eq!(
            ActionSpace::encode(action),
            (ActionSpace::SIZE - 1) as ActionIndex
        );
    }

    #[test]
    fn size_is_the_product_of_both_axes() {
        assert_eq!(ActionSpace::SIZE, 55);
    }

    #[test]
    fn index_zero_is_do_nothing() {
        let action = ActionSpace::decode(0);
        assert_eq!(action.movement, MoveIntent::Hold);
        assert_eq!(action.ability, None);
    }

    #[test]
    fn mask_blocks_abilities_that_are_not_ready() {
        let mut mask = Vec::new();
        ActionSpace::mask(&[true, false, false, false], &mut mask);
        assert_eq!(mask.len(), ActionSpace::SIZE);
        // Slot 0 ready, so "hold + ability 0" is allowed.
        assert!(
            mask[ActionSpace::encode(Action {
                movement: MoveIntent::Hold,
                ability: Some(0)
            }) as usize]
        );
        // Slot 1 not ready.
        assert!(
            !mask[ActionSpace::encode(Action {
                movement: MoveIntent::Hold,
                ability: Some(1)
            }) as usize]
        );
        // Pure movement is always allowed.
        assert!(
            mask[ActionSpace::encode(Action {
                movement: MoveIntent::Engage,
                ability: None
            }) as usize]
        );
    }

    #[test]
    fn actions_convert_into_simulation_intents() {
        let action = Action {
            movement: MoveIntent::Engage,
            ability: Some(2),
        };
        let intent: Intent = action.into();
        assert_eq!(intent.movement, MoveIntent::Engage);
        assert_eq!(intent.ability, Some(2));
    }
}

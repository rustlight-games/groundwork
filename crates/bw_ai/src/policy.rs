//! Policies: things that choose actions.
//!
//! [`ScriptedPolicy`] exists so the game is playable and the trainer has an
//! opponent before any network has been trained. Skipping it is a common early
//! mistake — without a baseline there is nothing to measure a learned policy
//! against, and no way to tell "training is broken" from "training is working
//! but slowly".

use bw_core::UnitId;
use rand::Rng;
use rand_chacha::ChaCha8Rng;

use crate::action::{Action, ActionIndex, ActionSpace};
use crate::obs::ObsBatch;

/// Chooses one action per row of an observation batch.
pub trait Policy: Send + Sync {
    /// Fill `out` with one action per observation row, in the same order.
    fn act(&mut self, batch: &ObsBatch, out: &mut Vec<ActionIndex>);

    /// Pair actions with the units they belong to.
    fn act_for_units(&mut self, batch: &ObsBatch) -> Vec<(UnitId, Action)> {
        let mut actions = Vec::new();
        self.act(batch, &mut actions);
        batch
            .units()
            .iter()
            .copied()
            .zip(actions.into_iter().map(ActionSpace::decode))
            .collect()
    }
}

/// Always close with the nearest enemy and use the first ability available.
///
/// Not intended to be good. It is intended to be a fixed, comprehensible
/// reference point that a learned policy must beat to be worth shipping.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptedPolicy;

impl Policy for ScriptedPolicy {
    fn act(&mut self, batch: &ObsBatch, out: &mut Vec<ActionIndex>) {
        out.clear();
        out.resize(
            batch.len(),
            ActionSpace::encode(Action {
                movement: bw_sim::components::MoveIntent::Engage,
                ability: Some(0),
            }),
        );
    }
}

/// Take the wrapped policy's action most of the time, a random one otherwise.
///
/// Exploration for training. `epsilon` is a per-thousand rate rather than a
/// float so that the schedule is exactly reproducible from a step count.
pub struct EpsilonGreedy<P> {
    inner: P,
    epsilon_per_mille: u32,
    rng: ChaCha8Rng,
}

impl<P: Policy> EpsilonGreedy<P> {
    pub fn new(inner: P, epsilon_per_mille: u32, rng: ChaCha8Rng) -> Self {
        Self {
            inner,
            epsilon_per_mille: epsilon_per_mille.min(1000),
            rng,
        }
    }

    pub fn set_epsilon_per_mille(&mut self, epsilon: u32) {
        self.epsilon_per_mille = epsilon.min(1000);
    }

    pub fn epsilon_per_mille(&self) -> u32 {
        self.epsilon_per_mille
    }

    pub fn inner(&self) -> &P {
        &self.inner
    }
}

impl<P: Policy> Policy for EpsilonGreedy<P> {
    fn act(&mut self, batch: &ObsBatch, out: &mut Vec<ActionIndex>) {
        self.inner.act(batch, out);
        for action in out.iter_mut() {
            if self.rng.random_range(0..1000) < self.epsilon_per_mille {
                *action = self.rng.random_range(0..ActionSpace::SIZE as ActionIndex);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;

    fn batch_of(rows: usize) -> ObsBatch {
        ObsBatch::from_parts(
            vec![0.0; rows * crate::obs::OBS_LEN],
            (0..rows as u32).map(UnitId).collect(),
        )
    }

    #[test]
    fn scripted_policy_emits_one_action_per_row() {
        let mut out = Vec::new();
        ScriptedPolicy.act(&batch_of(5), &mut out);
        assert_eq!(out.len(), 5);
        assert!(out.iter().all(|&a| (a as usize) < ActionSpace::SIZE));
    }

    #[test]
    fn scripted_policy_engages() {
        let actions = ScriptedPolicy.act_for_units(&batch_of(3));
        assert_eq!(actions.len(), 3);
        assert!(
            actions
                .iter()
                .all(|(_, a)| a.movement == bw_sim::components::MoveIntent::Engage)
        );
    }

    #[test]
    fn act_for_units_pairs_actions_with_the_right_units() {
        let actions = ScriptedPolicy.act_for_units(&batch_of(4));
        let ids: Vec<u32> = actions.iter().map(|(id, _)| id.0).collect();
        assert_eq!(ids, [0, 1, 2, 3]);
    }

    #[test]
    fn an_empty_batch_produces_no_actions() {
        let mut out = vec![99];
        ScriptedPolicy.act(&batch_of(0), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn epsilon_is_clamped_to_a_valid_rate() {
        let policy = EpsilonGreedy::new(ScriptedPolicy, 5_000, ChaCha8Rng::seed_from_u64(0));
        assert_eq!(policy.epsilon_per_mille(), 1000);
    }

    #[test]
    fn zero_epsilon_never_deviates_from_the_wrapped_policy() {
        let batch = batch_of(64);
        let mut baseline = Vec::new();
        ScriptedPolicy.act(&batch, &mut baseline);

        let mut policy = EpsilonGreedy::new(ScriptedPolicy, 0, ChaCha8Rng::seed_from_u64(1));
        let mut out = Vec::new();
        policy.act(&batch, &mut out);
        assert_eq!(out, baseline);
    }

    #[test]
    fn full_epsilon_explores_away_from_the_wrapped_policy() {
        let batch = batch_of(64);
        let mut policy = EpsilonGreedy::new(ScriptedPolicy, 1000, ChaCha8Rng::seed_from_u64(2));
        let mut out = Vec::new();
        policy.act(&batch, &mut out);
        let scripted = ActionSpace::encode(Action {
            movement: bw_sim::components::MoveIntent::Engage,
            ability: Some(0),
        });
        assert!(
            out.iter().any(|&a| a != scripted),
            "epsilon=1.0 never explored"
        );
    }

    #[test]
    fn exploration_is_reproducible_from_its_seed() {
        let run = || {
            let mut policy = EpsilonGreedy::new(ScriptedPolicy, 500, ChaCha8Rng::seed_from_u64(7));
            let mut out = Vec::new();
            policy.act(&batch_of(32), &mut out);
            out
        };
        assert_eq!(run(), run());
    }
}

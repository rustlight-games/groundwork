//! The Q-network.
//!
//! A three-layer perceptron. Deliberately small: the observation is already a
//! hand-designed summary rather than raw pixels, so there is little for a
//! deeper network to extract, and a small network is what makes CPU inference
//! for a whole army affordable inside a frame.

use burn::module::Module;
use burn::nn::{Linear, LinearConfig};
use burn::tensor::backend::Backend;
use burn::tensor::{Tensor, activation};
use serde::{Deserialize, Serialize};

/// Network shape. Recorded in the manifest so weights cannot be loaded into a
/// differently-shaped model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DqnNetConfig {
    pub obs_len: usize,
    pub action_count: usize,
    pub hidden: usize,
}

impl DqnNetConfig {
    pub fn new(obs_len: usize, action_count: usize) -> Self {
        Self {
            obs_len,
            action_count,
            hidden: 128,
        }
    }

    pub fn with_hidden(mut self, hidden: usize) -> Self {
        self.hidden = hidden;
        self
    }

    pub fn init<B: Backend>(&self, device: &B::Device) -> DqnNet<B> {
        DqnNet {
            input: LinearConfig::new(self.obs_len, self.hidden).init(device),
            hidden: LinearConfig::new(self.hidden, self.hidden).init(device),
            output: LinearConfig::new(self.hidden, self.action_count).init(device),
        }
    }
}

/// Maps an observation batch to one Q-value per action.
#[derive(Module, Debug)]
pub struct DqnNet<B: Backend> {
    input: Linear<B>,
    hidden: Linear<B>,
    output: Linear<B>,
}

impl<B: Backend> DqnNet<B> {
    /// `[batch, obs_len]` in, `[batch, action_count]` out.
    pub fn forward(&self, observations: Tensor<B, 2>) -> Tensor<B, 2> {
        let x = activation::relu(self.input.forward(observations));
        let x = activation::relu(self.hidden.forward(x));
        self.output.forward(x)
    }

    /// Choose an action for every unit in `batch`.
    ///
    /// The entry point production code should use: it handles the empty batch,
    /// which happens routinely at the end of a fight when the last unit dies
    /// and would otherwise panic inside the tensor backend.
    pub fn act(
        &self,
        batch: &crate::obs::ObsBatch,
        mask: Option<&[bool]>,
        device: &B::Device,
    ) -> Vec<crate::action::ActionIndex> {
        if batch.is_empty() {
            return Vec::new();
        }
        let input =
            observations_to_tensor::<B>(batch.data(), batch.len(), crate::obs::OBS_LEN, device);
        self.best_actions(input, mask)
    }

    /// Best action per row, respecting a per-row availability mask.
    ///
    /// Masking happens here rather than by zeroing Q-values, because a masked
    /// action's true value may well be negative — zeroing would make forbidden
    /// actions look attractive. Subtracting a large constant keeps them last.
    pub fn best_actions(
        &self,
        observations: Tensor<B, 2>,
        mask: Option<&[bool]>,
    ) -> Vec<crate::action::ActionIndex> {
        let [rows, _] = observations.dims();
        let q = self.forward(observations);
        let [_, actions] = q.dims();
        let values: Vec<f32> = q.into_data().into_vec().unwrap_or_default();

        (0..rows)
            .map(|row| {
                let offset = row * actions;
                let mut best = 0usize;
                let mut best_value = f32::NEG_INFINITY;
                for action in 0..actions {
                    let allowed =
                        mask.is_none_or(|m| m.get(offset + action).copied().unwrap_or(true));
                    if !allowed {
                        continue;
                    }
                    let value = values
                        .get(offset + action)
                        .copied()
                        .unwrap_or(f32::NEG_INFINITY);
                    // Strictly-greater keeps the lowest index on a tie, so an
                    // untrained network with equal outputs behaves consistently.
                    if value > best_value {
                        best_value = value;
                        best = action;
                    }
                }
                best as crate::action::ActionIndex
            })
            .collect()
    }
}

/// Build a `[rows, columns]` tensor from a flat observation batch.
///
/// `rows` must be non-zero. A zero-row reshape panics deep inside the tensor
/// backend rather than returning an empty tensor, so callers guard first —
/// [`DqnNet::act`] does this for you, and is what production code should use.
pub fn observations_to_tensor<B: Backend>(
    data: &[f32],
    rows: usize,
    columns: usize,
    device: &B::Device,
) -> Tensor<B, 2> {
    assert!(rows > 0, "observations_to_tensor requires at least one row");
    debug_assert_eq!(
        data.len(),
        rows * columns,
        "observation data does not match its shape"
    );
    Tensor::<B, 1>::from_floats(data, device).reshape([rows, columns])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InferenceBackend;
    use crate::action::ActionSpace;
    use crate::obs::OBS_LEN;

    type B = InferenceBackend;

    fn device() -> burn::backend::ndarray::NdArrayDevice {
        Default::default()
    }

    fn net() -> DqnNet<B> {
        DqnNetConfig::new(OBS_LEN, ActionSpace::SIZE)
            .with_hidden(32)
            .init(&device())
    }

    fn batch(rows: usize) -> crate::obs::ObsBatch {
        crate::obs::ObsBatch::from_parts(
            vec![0.25; rows * OBS_LEN],
            (0..rows as u32).map(bw_core::UnitId).collect(),
        )
    }

    #[test]
    fn forward_produces_one_value_per_action() {
        let rows = 4;
        let input =
            observations_to_tensor::<B>(&vec![0.1; rows * OBS_LEN], rows, OBS_LEN, &device());
        assert_eq!(net().forward(input).dims(), [rows, ActionSpace::SIZE]);
    }

    #[test]
    fn best_actions_returns_one_index_per_row() {
        let actions = net().act(&batch(3), None, &device());
        assert_eq!(actions.len(), 3);
        assert!(actions.iter().all(|&a| (a as usize) < ActionSpace::SIZE));
    }

    #[test]
    fn masked_actions_are_never_chosen() {
        let rows = 2;
        let net = net();
        // Allow exactly one action per row and check it is the one returned.
        let allowed = 7usize;
        let mut mask = vec![false; rows * ActionSpace::SIZE];
        for row in 0..rows {
            mask[row * ActionSpace::SIZE + allowed] = true;
        }
        assert_eq!(
            net.act(&batch(rows), Some(&mask), &device()),
            vec![allowed as u16; rows]
        );
    }

    #[test]
    fn an_empty_batch_produces_no_actions() {
        // Happens every time the last unit on a side dies. Reshaping a
        // zero-row tensor panics inside the backend, so act() guards instead.
        assert!(net().act(&batch(0), None, &device()).is_empty());
    }

    #[test]
    fn inference_is_reproducible_for_a_fixed_model() {
        let net = net();
        let rows = 5;
        let data: Vec<f32> = (0..rows * OBS_LEN)
            .map(|i| (i % 17) as f32 / 17.0)
            .collect();
        let tensor = || observations_to_tensor::<B>(&data, rows, OBS_LEN, &device());
        assert_eq!(
            net.best_actions(tensor(), None),
            net.best_actions(tensor(), None)
        );
    }
}

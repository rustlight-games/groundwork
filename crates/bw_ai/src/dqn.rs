//! DQN training.
//!
//! Behind the `train` feature so the game binary never links autodiff.
//! Skeleton: the loop structure and the pieces that are easy to get subtly
//! wrong (target network, terminal bootstrapping) are here; tuning is not.

use burn::optim::{AdamConfig, GradientsParams, Optimizer};
use burn::tensor::backend::AutodiffBackend;
use burn::tensor::{Int, Tensor};

use crate::net::{DqnNet, DqnNetConfig};
use crate::replay::ReplayBuffer;

/// Training hyperparameters.
#[derive(Clone, Copy, Debug)]
pub struct DqnConfig {
    pub learning_rate: f64,
    pub discount: f32,
    pub batch_size: usize,
    /// Environment steps between copying the online net into the target net.
    pub target_sync_interval: u64,
}

impl Default for DqnConfig {
    fn default() -> Self {
        Self {
            learning_rate: 1e-3,
            discount: 0.99,
            batch_size: 64,
            target_sync_interval: 1_000,
        }
    }
}

/// Online network, target network, and optimiser.
///
/// The target network is the part worth understanding. Q-learning's update uses
/// the network's own output as part of its training signal, so without a
/// periodically-frozen copy the target moves every step and the values chase
/// themselves upward instead of converging.
pub struct DqnLearner<B: AutodiffBackend> {
    pub online: DqnNet<B>,
    pub target: DqnNet<B>,
    optimizer: burn::optim::adaptor::OptimizerAdaptor<burn::optim::Adam, DqnNet<B>, B>,
    config: DqnConfig,
    steps: u64,
}

impl<B: AutodiffBackend> DqnLearner<B> {
    pub fn new(net: DqnNetConfig, config: DqnConfig, device: &B::Device) -> Self {
        Self {
            online: net.init(device),
            target: net.init(device),
            optimizer: AdamConfig::new().init(),
            config,
            steps: 0,
        }
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// One gradient step against a batch sampled from `buffer`.
    ///
    /// Returns the loss, or `None` when the buffer has not filled enough to
    /// sample a full batch. Training before the buffer holds a diverse sample
    /// overfits to the first few seconds of the first episode.
    pub fn train_step(
        &mut self,
        buffer: &ReplayBuffer,
        rng: &mut rand_chacha::ChaCha8Rng,
        device: &B::Device,
    ) -> Option<f32> {
        if buffer.len() < self.config.batch_size {
            return None;
        }

        let mut indices = Vec::new();
        buffer.sample_indices(self.config.batch_size, rng, &mut indices);

        let obs_len = buffer.get(indices[0])?.observation.len();
        let mut observations = Vec::with_capacity(indices.len() * obs_len);
        let mut next_observations = Vec::with_capacity(indices.len() * obs_len);
        let mut actions = Vec::with_capacity(indices.len());
        let mut rewards = Vec::with_capacity(indices.len());
        let mut continues = Vec::with_capacity(indices.len());

        for &i in &indices {
            let t = buffer.get(i)?;
            observations.extend_from_slice(&t.observation);
            next_observations.extend_from_slice(&t.next_observation);
            actions.push(t.action as i32);
            rewards.push(t.reward);
            // Zero at a terminal state: there is no future to discount.
            continues.push(if t.done { 0.0 } else { 1.0 });
        }

        let rows = indices.len();
        let obs =
            Tensor::<B, 1>::from_floats(observations.as_slice(), device).reshape([rows, obs_len]);
        let next_obs = Tensor::<B, 1>::from_floats(next_observations.as_slice(), device)
            .reshape([rows, obs_len]);
        let reward = Tensor::<B, 1>::from_floats(rewards.as_slice(), device).reshape([rows, 1]);
        let continues =
            Tensor::<B, 1>::from_floats(continues.as_slice(), device).reshape([rows, 1]);
        let action = Tensor::<B, 1, Int>::from_ints(actions.as_slice(), device).reshape([rows, 1]);

        // Target values come from the frozen network and carry no gradient.
        let next_q = self.target.forward(next_obs).detach();
        let best_next = next_q.max_dim(1);
        let target_q = reward
            + best_next
                .mul(continues)
                .mul_scalar(self.config.discount as f64);

        let predicted = self.online.forward(obs).gather(1, action);
        let loss = (predicted - target_q.detach()).powi_scalar(2).mean();
        let loss_value = loss.clone().into_scalar().to_string().parse::<f32>().ok();

        let grads = loss.backward();
        let grads = GradientsParams::from_grads(grads, &self.online);
        self.online = self
            .optimizer
            .step(self.config.learning_rate, self.online.clone(), grads);

        self.steps += 1;
        if self.steps.is_multiple_of(self.config.target_sync_interval) {
            self.sync_target();
        }
        loss_value
    }

    /// Copy the online network into the target network.
    pub fn sync_target(&mut self) {
        self.target = self.online.clone();
    }
}

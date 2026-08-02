//! Experience replay.
//!
//! A ring buffer of transitions, sampled uniformly. Uniform sampling is the
//! honest default; prioritised replay is a worthwhile upgrade later, and
//! [`ReplayBuffer::sample_indices`] is deliberately separate from the storage
//! so that swapping the sampling rule does not touch the rest.

use rand::Rng;
use rand_chacha::ChaCha8Rng;

use crate::action::ActionIndex;

/// One environment step.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub observation: Vec<f32>,
    pub action: ActionIndex,
    pub reward: f32,
    pub next_observation: Vec<f32>,
    /// Whether the episode ended here. Terminal states have no bootstrap value,
    /// and forgetting that is the classic reason a DQN's Q-values diverge.
    pub done: bool,
}

/// Fixed-capacity ring buffer.
#[derive(Debug)]
pub struct ReplayBuffer {
    entries: Vec<Transition>,
    capacity: usize,
    next: usize,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity.min(1024)),
            capacity: capacity.max(1),
            next: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn push(&mut self, transition: Transition) {
        if self.entries.len() < self.capacity {
            self.entries.push(transition);
        } else {
            self.entries[self.next] = transition;
            self.next = (self.next + 1) % self.capacity;
        }
    }

    pub fn get(&self, index: usize) -> Option<&Transition> {
        self.entries.get(index)
    }

    /// Sample `count` indices with replacement.
    ///
    /// With replacement because the buffer is large relative to a batch, so
    /// collisions are rare, and rejection sampling would make the cost depend
    /// on buffer occupancy.
    pub fn sample_indices(&self, count: usize, rng: &mut ChaCha8Rng, out: &mut Vec<usize>) {
        out.clear();
        if self.entries.is_empty() {
            return;
        }
        for _ in 0..count {
            out.push(rng.random_range(0..self.entries.len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;

    fn transition(reward: f32) -> Transition {
        Transition {
            observation: vec![0.0; 4],
            action: 0,
            reward,
            next_observation: vec![0.0; 4],
            done: false,
        }
    }

    #[test]
    fn grows_until_capacity_then_overwrites_oldest_first() {
        let mut buffer = ReplayBuffer::new(3);
        for i in 0..5 {
            buffer.push(transition(i as f32));
        }
        assert_eq!(buffer.len(), 3);
        let rewards: Vec<f32> = (0..3).map(|i| buffer.get(i).unwrap().reward).collect();
        assert!(rewards.contains(&4.0), "most recent transition was lost");
        assert!(
            !rewards.contains(&0.0),
            "oldest transition should have been evicted"
        );
    }

    #[test]
    fn zero_capacity_is_treated_as_one_rather_than_dividing_by_zero() {
        let mut buffer = ReplayBuffer::new(0);
        buffer.push(transition(1.0));
        buffer.push(transition(2.0));
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn sampling_an_empty_buffer_yields_nothing() {
        let mut out = vec![99];
        ReplayBuffer::new(4).sample_indices(8, &mut ChaCha8Rng::seed_from_u64(0), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn sampling_is_reproducible_and_in_range() {
        let mut buffer = ReplayBuffer::new(16);
        for i in 0..16 {
            buffer.push(transition(i as f32));
        }
        let draw = || {
            let mut out = Vec::new();
            buffer.sample_indices(32, &mut ChaCha8Rng::seed_from_u64(5), &mut out);
            out
        };
        let sample = draw();
        assert_eq!(sample.len(), 32);
        assert!(sample.iter().all(|&i| i < 16));
        assert_eq!(sample, draw());
    }
}

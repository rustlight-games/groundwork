//! Learned behaviour.
//!
//! Units decide what to do with a Deep Q-Network: given an encoded view of the
//! battlefield, the network scores every available action and the unit takes
//! the best one. Training happens out of process in `tools/bw_train`; the game
//! only ever runs inference.
//!
//! ## The float boundary
//!
//! Observations are `f32`, which may look like a violation of the no-floats
//! rule the simulation lives under. It is not, because the flow is one-way: the
//! simulation produces an observation, the network consumes it and returns a
//! *discrete action index*, and the simulation acts on that integer. No float
//! ever re-enters simulation state, so no float can perturb it. Two machines
//! whose networks disagree in the last bit would at worst pick a different
//! action — and since the action is an integer, the resulting battle is still
//! internally consistent and still reproducible from its own recorded actions.
//!
//! ## Versioning
//!
//! [`OBS_VERSION`] and [`ActionSpace`] together form a contract between the
//! trainer and the game. A model trained against one encoding and run against
//! another produces confident nonsense — the single most common way to lose a
//! week on a reinforcement learning project. [`ModelManifest`] records both
//! alongside the weights and refuses to load on a mismatch.

#![forbid(unsafe_code)]
// Burn's backend generics nest deeply enough to exceed the default limit.
#![recursion_limit = "256"]

pub mod action;
pub mod manifest;
pub mod net;
pub mod obs;
pub mod policy;
pub mod replay;

#[cfg(feature = "train")]
pub mod dqn;

pub use action::{Action, ActionIndex, ActionSpace};
pub use manifest::{ManifestError, ModelManifest};
pub use net::{DqnNet, DqnNetConfig};
pub use obs::{OBS_VERSION, ObsBatch, ObservationEncoder};
pub use policy::{EpsilonGreedy, Policy, ScriptedPolicy};
pub use replay::{ReplayBuffer, Transition};

/// The backend the game uses for inference.
pub type InferenceBackend = burn::backend::NdArray<f32>;

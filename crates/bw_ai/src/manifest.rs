//! Model metadata.
//!
//! Weights on disk are meaningless without the encoding they were trained
//! against. Loading a model whose observation layout has since changed does not
//! fail loudly — the tensor shapes still line up, the network still runs, and
//! it confidently returns rubbish. That failure mode costs days, so every
//! checkpoint carries a manifest and loading verifies it.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::action::ActionSpace;
use crate::net::DqnNetConfig;
use crate::obs::{OBS_LEN, OBS_VERSION};

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("could not read manifest {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse manifest {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: Box<ron::error::SpannedError>,
    },

    #[error(
        "model was trained against observation version {found} but this build uses {expected}; \
         retrain or check out the matching revision"
    )]
    ObsVersion { found: u32, expected: u32 },

    #[error("model expects an observation of {found} values but this build produces {expected}")]
    ObsLength { found: usize, expected: usize },

    #[error("model has {found} actions but this build's action space has {expected}")]
    ActionCount { found: usize, expected: usize },
}

/// What a checkpoint was trained against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    pub obs_version: u32,
    pub obs_len: usize,
    pub action_count: usize,
    pub net: DqnNetConfig,
    /// Environment steps of training behind these weights.
    pub trained_steps: u64,
    /// Free-form note, e.g. which encounter set this was trained on.
    #[serde(default)]
    pub notes: String,
}

impl ModelManifest {
    /// A manifest describing the current build.
    pub fn current(net: DqnNetConfig, trained_steps: u64) -> Self {
        Self {
            obs_version: OBS_VERSION,
            obs_len: OBS_LEN,
            action_count: ActionSpace::SIZE,
            net,
            trained_steps,
            notes: String::new(),
        }
    }

    /// Check this manifest against the running build.
    pub fn verify(&self) -> Result<(), ManifestError> {
        if self.obs_version != OBS_VERSION {
            return Err(ManifestError::ObsVersion {
                found: self.obs_version,
                expected: OBS_VERSION,
            });
        }
        if self.obs_len != OBS_LEN {
            return Err(ManifestError::ObsLength {
                found: self.obs_len,
                expected: OBS_LEN,
            });
        }
        if self.action_count != ActionSpace::SIZE {
            return Err(ManifestError::ActionCount {
                found: self.action_count,
                expected: ActionSpace::SIZE,
            });
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .expect("manifest is always serialisable");
        std::fs::write(path, text).map_err(|source| ManifestError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    /// Load and verify in one step, so an unverified manifest cannot be used by
    /// accident.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let manifest: Self = ron::from_str(&text).map_err(|source| ManifestError::Parse {
            path: path.display().to_string(),
            source: Box::new(source),
        })?;
        manifest.verify()?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ModelManifest {
        ModelManifest::current(DqnNetConfig::new(OBS_LEN, ActionSpace::SIZE), 1_000)
    }

    #[test]
    fn a_current_manifest_verifies() {
        assert!(manifest().verify().is_ok());
    }

    #[test]
    fn a_stale_observation_version_is_rejected() {
        let mut m = manifest();
        m.obs_version = OBS_VERSION + 1;
        let err = m.verify().unwrap_err().to_string();
        assert!(err.contains("retrain"), "{err}");
    }

    #[test]
    fn a_mismatched_observation_length_is_rejected() {
        let mut m = manifest();
        m.obs_len = OBS_LEN + 1;
        assert!(matches!(m.verify(), Err(ManifestError::ObsLength { .. })));
    }

    #[test]
    fn a_mismatched_action_count_is_rejected() {
        let mut m = manifest();
        m.action_count = 3;
        assert!(matches!(m.verify(), Err(ManifestError::ActionCount { .. })));
    }

    #[test]
    fn round_trips_through_ron() {
        let original = manifest();
        let text = ron::ser::to_string(&original).unwrap();
        assert_eq!(ron::from_str::<ModelManifest>(&text).unwrap(), original);
    }
}

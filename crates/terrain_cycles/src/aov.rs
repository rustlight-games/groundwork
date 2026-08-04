//! What to ask the path tracer for besides a picture.
//!
//! ## Named now, implemented as they are needed
//!
//! Most of these are not wired up. That is deliberate rather than aspirational:
//! the *names* are the compatibility surface, and a dataset manifest that
//! records `"direct_diffuse"` today has to mean the same thing when the channel
//! is added. Deciding the vocabulary once, before there are shards on disk
//! referring to it, is much cheaper than renaming a channel that a trained model
//! has already learned the statistics of.
//!
//! ## Configuration, not plumbing
//!
//! The renderer this replaces carried ten channels by hand through its own
//! resolve loop, and each one was a chance for the recorded value to drift from
//! the value actually used. Cycles produces these by configuration — it already
//! knows what its own direct diffuse was — so the failure mode disappears rather
//! than being managed.
//!
//! ## Cryptomatte is the one worth explaining
//!
//! A per-object matte gives the neural renderer something no other channel does:
//! *which blade* a pixel belongs to. A network trained without it has to infer
//! object boundaries from colour, and the boundaries it infers are the ones that
//! are easy to see — which is precisely the set that a cheap renderer already
//! gets right.

use terrain_core::digest::{Digest, Digestible};

/// One output the path tracer is asked to produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum OutputPass {
    /// The picture. Always produced.
    Beauty,
    /// Surface colour before lighting.
    Albedo,
    /// World-space normals.
    Normal,
    /// Distance from the camera, in metres.
    Depth,
    /// Light that arrived straight from the sun.
    DirectDiffuse,
    /// Light that bounced. The half a cheap renderer cannot approximate, and
    /// therefore the half worth learning.
    IndirectDiffuse,
    /// How much of the sun reaches each point.
    Shadow,
    /// How much of the sky does.
    AmbientOcclusion,
    /// Which material owns each pixel.
    MaterialId,
    /// Ground height, from the scene rather than from the trace.
    Elevation,
    /// Which population each pixel belongs to.
    PopulationId,
    /// Per-object mattes.
    Cryptomatte,
}

impl OutputPass {
    /// The stable name this pass is written and recorded under.
    ///
    /// Snake case, and fixed. A manifest on disk refers to these, so renaming
    /// one silently invalidates every shard that mentions it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Beauty => "beauty",
            Self::Albedo => "albedo",
            Self::Normal => "normal",
            Self::Depth => "depth",
            Self::DirectDiffuse => "direct_diffuse",
            Self::IndirectDiffuse => "indirect_diffuse",
            Self::Shadow => "shadow",
            Self::AmbientOcclusion => "ambient_occlusion",
            Self::MaterialId => "material_id",
            Self::Elevation => "elevation",
            Self::PopulationId => "population_id",
            Self::Cryptomatte => "cryptomatte",
        }
    }

    /// Whether this build can actually produce it.
    ///
    /// Stated rather than assumed, so that asking for an unimplemented pass is
    /// an error at the request rather than a channel that silently does not
    /// appear in the output directory. A dataset job that quietly produced nine
    /// channels where ten were asked for is a corpus with a hole in it that
    /// nothing reports.
    pub const fn is_implemented(self) -> bool {
        matches!(
            self,
            Self::Beauty
                | Self::Albedo
                | Self::Normal
                | Self::Depth
                | Self::DirectDiffuse
                | Self::IndirectDiffuse
                | Self::AmbientOcclusion
                | Self::Cryptomatte
        )
    }

    /// Every pass, in a stable order.
    pub const ALL: [Self; 12] = [
        Self::Beauty,
        Self::Albedo,
        Self::Normal,
        Self::Depth,
        Self::DirectDiffuse,
        Self::IndirectDiffuse,
        Self::Shadow,
        Self::AmbientOcclusion,
        Self::MaterialId,
        Self::Elevation,
        Self::PopulationId,
        Self::Cryptomatte,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pass| pass.name() == name)
    }
}

/// The passes one render is asked for.
///
/// Ordered and deduplicated on the way in, so that two requests naming the same
/// passes in different orders produce the same manifest and therefore the same
/// digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputRequest {
    passes: Vec<OutputPass>,
}

impl OutputRequest {
    /// The picture alone.
    pub fn beauty() -> Self {
        Self {
            passes: vec![OutputPass::Beauty],
        }
    }

    /// Everything this build can produce.
    pub fn everything_implemented() -> Self {
        Self::from_passes(OutputPass::ALL.into_iter().filter(|p| p.is_implemented()))
    }

    pub fn from_passes(passes: impl IntoIterator<Item = OutputPass>) -> Self {
        let mut passes: Vec<OutputPass> = passes.into_iter().collect();
        // Beauty is always produced, whether or not it was asked for: every
        // other pass is a supplement to a picture, and a request without one is
        // far more likely to be an omission than an intention.
        if !passes.contains(&OutputPass::Beauty) {
            passes.push(OutputPass::Beauty);
        }
        passes.sort();
        passes.dedup();
        Self { passes }
    }

    pub fn contains(&self, pass: OutputPass) -> bool {
        self.passes.contains(&pass)
    }

    pub fn passes(&self) -> &[OutputPass] {
        &self.passes
    }

    pub fn len(&self) -> usize {
        self.passes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    /// The passes that were asked for and cannot be produced.
    pub fn unsupported(&self) -> Vec<OutputPass> {
        self.passes
            .iter()
            .copied()
            .filter(|pass| !pass.is_implemented())
            .collect()
    }

    /// The names, for a manifest.
    pub fn names(&self) -> Vec<&'static str> {
        self.passes.iter().map(|pass| pass.name()).collect()
    }
}

impl Digestible for OutputRequest {
    fn absorb(&self, digest: &mut Digest) {
        digest.slice(&self.passes, |d, pass| {
            d.str(pass.name());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pass_has_a_distinct_stable_name() {
        // A manifest on disk refers to these, so a collision or a rename
        // silently invalidates every shard that mentions one.
        let mut seen: Vec<&str> = Vec::new();
        for pass in OutputPass::ALL {
            let name = pass.name();
            assert!(!seen.contains(&name), "{name} is used twice");
            assert_eq!(OutputPass::from_name(name), Some(pass));
            seen.push(name);
        }
        assert_eq!(OutputPass::from_name("not_a_pass"), None);
    }

    #[test]
    fn a_request_always_produces_a_picture() {
        // Every other pass is a supplement to a beauty render, and a request
        // without one is far more likely to be an omission than an intention.
        let request = OutputRequest::from_passes([OutputPass::Depth]);
        assert!(request.contains(OutputPass::Beauty));
        assert!(request.contains(OutputPass::Depth));
    }

    #[test]
    fn a_request_is_ordered_and_deduplicated() {
        // So two requests naming the same passes in different orders produce
        // the same manifest and therefore the same digest.
        let forward =
            OutputRequest::from_passes([OutputPass::Depth, OutputPass::Albedo, OutputPass::Depth]);
        let backward =
            OutputRequest::from_passes([OutputPass::Albedo, OutputPass::Depth, OutputPass::Albedo]);
        assert_eq!(forward, backward);
        assert_eq!(forward.len(), 3);
        assert_eq!(
            forward.fingerprint("outputs"),
            backward.fingerprint("outputs")
        );
    }

    #[test]
    fn asking_for_an_unimplemented_pass_is_visible() {
        // A dataset job that quietly produced nine channels where ten were
        // asked for is a corpus with a hole in it that nothing reports.
        let request = OutputRequest::from_passes([OutputPass::Elevation, OutputPass::Albedo]);
        assert_eq!(request.unsupported(), vec![OutputPass::Elevation]);
        assert!(
            OutputRequest::everything_implemented()
                .unsupported()
                .is_empty()
        );
    }

    #[test]
    fn the_implemented_set_is_not_everything_and_says_so() {
        // The honest status. If this ever came back equal, either every pass
        // landed or somebody flipped a flag without writing the channel.
        let implemented = OutputPass::ALL
            .iter()
            .filter(|p| p.is_implemented())
            .count();
        assert!(implemented < OutputPass::ALL.len());
        assert!(implemented >= 8);
    }

    #[test]
    fn a_beauty_only_request_is_one_pass() {
        assert_eq!(OutputRequest::beauty().len(), 1);
        assert_eq!(OutputRequest::beauty().names(), ["beauty"]);
    }
}

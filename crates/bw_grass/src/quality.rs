//! How hard the renderer is allowed to work.
//!
//! The grass used to have one quality, because it had one job: be finished
//! before the camera arrives. It now has two jobs that want opposite things.
//! The game still wants a page in tens of milliseconds. The training corpus
//! wants the best picture this design can produce and does not care what it
//! costs, because nothing is waiting on it.
//!
//! Those are not settings on a slider, they are different products, so they are
//! an enum. A tier is chosen once, at the top, and every decision that scales
//! with budget reads it rather than consulting a constant.
//!
//! ## What a tier may and may not change
//!
//! It may change **how finely a thing is measured** — supersampling, shadow map
//! density, how many directions occlusion is gathered from, how many segments a
//! blade is walked in.
//!
//! It may not change **what grows where**. Placement, shape and material are
//! pure functions of world coordinates and the seed, and a tier that moved a
//! blade would make the preview a picture of a different meadow — which would
//! also destroy the one property the neural training depends on, that the cheap
//! render and the expensive render are two photographs of one scene.
//!
//! [`GrassRenderQuality::forks`] looks like a counter-example and is not: a fork
//! that cannot be resolved collapses into the notched silhouette it would have
//! averaged to anyway, which is a filtering decision about a blade that exists
//! either way.

/// Which product a bake is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrassRenderQuality {
    /// The game. Streamed on a background thread, has to keep up.
    #[default]
    Preview,
    /// Bulk training targets. Everything that matters, nothing that does not.
    Dataset,
    /// The best this renderer can do, for masters and for judging the others.
    Reference,
}

impl GrassRenderQuality {
    /// Linear supersampling factor the page is composited at.
    ///
    /// Three is what the look was tuned at and is kept exactly there for
    /// [`GrassRenderQuality::Preview`], so the cheap tier stays the picture that
    /// was accepted. Four is the point where a blade edge has sixteen levels of
    /// coverage rather than nine, which is roughly where a thin ribbon stops
    /// crawling when the ground slides under the sampling grid.
    ///
    /// Not five or six. Cost is quadratic and the returns past four are visible
    /// only on the thinnest marks in the vocabulary, which is exactly the
    /// population deliberately kept dim.
    #[inline]
    pub const fn supersample(self) -> usize {
        match self {
            Self::Preview => 3,
            Self::Dataset | Self::Reference => 4,
        }
    }

    /// Light-space texels per final page pixel, or zero for no cast shadows.
    ///
    /// Above one on purpose. Grass shadows are thin, and a shadow map at the
    /// resolution of the thing receiving it aliases into a dotted line — the
    /// blade is a pixel wide and the shadow test either hits it or does not.
    #[inline]
    pub const fn shadow_density(self) -> f32 {
        match self {
            Self::Preview => 0.0,
            Self::Dataset => 3.0,
            Self::Reference => 4.0,
        }
    }

    /// How many directions the sun is sampled over its disc.
    ///
    /// One is a point sun and a hard edge. Four is enough for the penumbra to
    /// read as soft rather than as four overlapping hard shadows, given the
    /// filter that follows it. Reference spends more because a soft shadow with
    /// too few samples does not look noisy, it looks *banded*, and banding is
    /// the one artefact a neural renderer will faithfully learn.
    #[inline]
    pub const fn sun_samples(self) -> usize {
        match self {
            Self::Preview => 1,
            Self::Dataset => 4,
            Self::Reference => 12,
        }
    }

    /// Horizon directions ambient occlusion is gathered over.
    #[inline]
    pub const fn ao_directions(self) -> usize {
        match self {
            Self::Preview => 0,
            Self::Dataset => 8,
            Self::Reference => 16,
        }
    }

    /// Whether a blade may split at the tip.
    ///
    /// Off in preview because a fork is a few pixels of structure at the
    /// authoring scale and none at all once the page is minified for the game,
    /// so drawing it costs the streaming tier time it does not have to buy
    /// something nobody sees.
    #[inline]
    pub const fn forks(self) -> bool {
        matches!(self, Self::Dataset | Self::Reference)
    }

    /// Whether a blade is given a real cross-section rather than a flat ribbon.
    #[inline]
    pub const fn cross_section(self) -> bool {
        matches!(self, Self::Dataset | Self::Reference)
    }

    /// Ribs per supersampled pixel of blade length.
    ///
    /// Half a pixel apart leaves no gap at any angle, which is what the
    /// rasteriser has always used. Reference walks finer because the twist term
    /// makes the surface normal rotate along the blade, and a normal sampled
    /// every half pixel on a blade that turns ninety degrees over its length is
    /// a visibly faceted highlight.
    #[inline]
    pub const fn ribs_per_pixel(self) -> f32 {
        match self {
            Self::Preview | Self::Dataset => 2.0,
            Self::Reference => 3.0,
        }
    }

    /// A short stable name, for filenames and benchmark ids.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Dataset => "dataset",
            Self::Reference => "reference",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIERS: [GrassRenderQuality; 3] = [
        GrassRenderQuality::Preview,
        GrassRenderQuality::Dataset,
        GrassRenderQuality::Reference,
    ];

    #[test]
    fn preview_is_the_tier_the_look_was_tuned_at() {
        // Three, and it has to stay three: the accepted snapshot baseline was
        // taken through it, so moving it silently invalidates every comparison
        // the optimisation suite has ever made.
        assert_eq!(GrassRenderQuality::Preview.supersample(), 3);
    }

    #[test]
    fn every_budget_climbs_with_the_tier() {
        for pair in TIERS.windows(2) {
            let (low, high) = (pair[0], pair[1]);
            assert!(low.supersample() <= high.supersample());
            assert!(low.shadow_density() <= high.shadow_density());
            assert!(low.sun_samples() <= high.sun_samples());
            assert!(low.ao_directions() <= high.ao_directions());
            assert!(low.ribs_per_pixel() <= high.ribs_per_pixel());
        }
    }

    #[test]
    fn only_the_cheap_tier_goes_without_shadows() {
        assert_eq!(GrassRenderQuality::Preview.shadow_density(), 0.0);
        for tier in [GrassRenderQuality::Dataset, GrassRenderQuality::Reference] {
            assert!(
                tier.shadow_density() > 1.0,
                "{tier:?} cannot resolve a blade"
            );
        }
    }

    #[test]
    fn every_tier_has_its_own_name() {
        let mut names: Vec<&str> = TIERS.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TIERS.len());
    }
}

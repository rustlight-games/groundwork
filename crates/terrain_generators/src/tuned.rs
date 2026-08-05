//! Which path owns a population, and what a document may say about it.
//!
//! ## The problem this module exists to name
//!
//! Two generators can grow grass here. The tuned one — `placement`, `stroke`,
//! `style` — is the quality bar and the only thing the production image has ever
//! contained. The generic one — `families` — emits ribbons and curves into a
//! `TerrainScene`, and nothing draws them.
//!
//! That was survivable while the scene was discarded. The moment the compiled
//! scene reaches Cycles, it stops being survivable: `families` still holds a
//! grass, a fine grass and a thatch recipe, so wiring the scene in draws a
//! *second*, lower-quality canopy over the tuned one. Density doubles, quality
//! halves, and the regression reads as "more detail" to anyone who did not know
//! what to look for.
//!
//! So every recipe now declares who renders it:
//!
//! ```text
//! Tuned(pass)   the document controls an existing tuned pass;
//!               this recipe's generic marks are never emitted
//! Secondary     this recipe's geometry is what the hybrid Cycles path draws
//! Deferred      compiled for diagnostics only; reported, never silently dropped
//! ```
//!
//! The declaration is on the recipe rather than in a table in the compiler,
//! because a table is a second place to forget: a new family added without a
//! render class fails to compile here, and a new family added without a table
//! entry would quietly default to whatever the table's fallback was.
//!
//! ## Why `Deferred` is a class and not an omission
//!
//! `population.dirt_clods` describes coarse soil structure, and so does the
//! ground profile's aggregate relief band. Rendering both counts one physical
//! signal twice. The honest state is "declared, understood, not drawn yet, and
//! here is the note saying so" — which is a class. Deleting the recipe would
//! lose the authored intent; drawing it would double the relief.

use std::fmt;

/// One of the tuned generator's four planting passes.
///
/// These are not new. They are the passes `placement::plant` has always run, in
/// this order, and naming them is what lets a document address one of them
/// without the others.
///
/// Ordered and hashable because a `TunedPopulationSet` is keyed by pass and its
/// iteration order reaches a fingerprint. A `HashMap` here would make the
/// compiled control set depend on hash seed, which is exactly the class of
/// non-determinism this framework is built to refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TunedPass {
    /// The dark mat, laid down first to be buried.
    Thatch,
    /// The closed canopy the statement tufts stand in.
    Fine,
    /// The statement tufts: clumps of blades sharing a lean and a family.
    Tuft,
    /// Broadleaf clusters.
    Broadleaf,
}

impl TunedPass {
    /// Every pass, in planting order.
    ///
    /// Planting order rather than alphabetical, because that is the order the
    /// reports read in and the order a reviewer thinks in: mat, canopy, tufts,
    /// leaves.
    pub const ALL: [Self; 4] = [Self::Thatch, Self::Fine, Self::Tuft, Self::Broadleaf];

    pub fn name(self) -> &'static str {
        match self {
            Self::Thatch => "thatch",
            Self::Fine => "fine",
            Self::Tuft => "tuft",
            Self::Broadleaf => "broadleaf",
        }
    }

    /// The density the tuned style asks for when nothing modulates it.
    ///
    /// The denominator of a document's density request: an author writing
    /// `density: 50` for tufts is asking for exactly what the style already
    /// does, so the pass factor is one and the picture does not move.
    ///
    /// Taken from `GrassStyle`'s defaults rather than invented. A number here
    /// that disagreed with the style would silently rescale every meadow the
    /// first time a document declared a tuned population.
    pub fn reference_density_per_m2(self) -> f64 {
        match self {
            Self::Thatch => 395.0,
            Self::Fine => 3800.0,
            Self::Tuft => 50.0,
            Self::Broadleaf => 4.0,
        }
    }
}

impl fmt::Display for TunedPass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Who draws what a recipe describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipeRenderClass {
    /// The document modulates an existing tuned pass. The recipe's own generic
    /// marks are never emitted into the production scene.
    Tuned(TunedPass),
    /// The recipe's geometry and instances are what the hybrid Cycles path
    /// draws.
    Secondary,
    /// Validated and reported, but not drawn. See the module note.
    Deferred,
}

impl RecipeRenderClass {
    /// Whether this class emits geometry into the secondary scene.
    ///
    /// The single question the compiler asks. Written as a method rather than a
    /// `matches!` at the call site so that adding a fourth class is a compile
    /// error here rather than a silent "not secondary" everywhere.
    pub fn emits_secondary(self) -> bool {
        match self {
            Self::Secondary => true,
            Self::Tuned(_) | Self::Deferred => false,
        }
    }

    /// The pass this class claims, if it claims one.
    pub fn tuned_pass(self) -> Option<TunedPass> {
        match self {
            Self::Tuned(pass) => Some(pass),
            Self::Secondary | Self::Deferred => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Tuned(_) => "tuned",
            Self::Secondary => "secondary",
            Self::Deferred => "deferred",
        }
    }
}

impl fmt::Display for RecipeRenderClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tuned(pass) => write!(f, "tuned({pass})"),
            Self::Secondary => f.write_str("secondary"),
            Self::Deferred => f.write_str("deferred"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pass_is_in_the_all_list_exactly_once() {
        // `ALL` drives report tables and control compilation, so a pass missing
        // from it is a pass a document silently cannot address.
        let mut seen = TunedPass::ALL.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), TunedPass::ALL.len());
    }

    #[test]
    fn reference_densities_are_positive_and_ordered_by_how_fine_the_pass_is() {
        // Not a style claim — a sanity check that the four numbers were copied
        // from the right place. Fine grass is the densest layer by an order of
        // magnitude and broadleaf is the sparsest; a transcription error would
        // almost certainly break that ordering.
        for pass in TunedPass::ALL {
            assert!(pass.reference_density_per_m2() > 0.0, "{pass}");
        }
        assert!(
            TunedPass::Fine.reference_density_per_m2()
                > TunedPass::Thatch.reference_density_per_m2()
        );
        assert!(
            TunedPass::Thatch.reference_density_per_m2()
                > TunedPass::Tuft.reference_density_per_m2()
        );
        assert!(
            TunedPass::Tuft.reference_density_per_m2()
                > TunedPass::Broadleaf.reference_density_per_m2()
        );
    }

    #[test]
    fn only_the_secondary_class_emits_into_the_scene() {
        assert!(RecipeRenderClass::Secondary.emits_secondary());
        assert!(!RecipeRenderClass::Deferred.emits_secondary());
        for pass in TunedPass::ALL {
            assert!(!RecipeRenderClass::Tuned(pass).emits_secondary());
        }
    }

    #[test]
    fn a_tuned_class_names_its_pass_and_the_others_name_none() {
        assert_eq!(
            RecipeRenderClass::Tuned(TunedPass::Fine).tuned_pass(),
            Some(TunedPass::Fine)
        );
        assert_eq!(RecipeRenderClass::Secondary.tuned_pass(), None);
        assert_eq!(RecipeRenderClass::Deferred.tuned_pass(), None);
    }
}

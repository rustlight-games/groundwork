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

/// One tuned pass, as a document can control it.
///
/// ## Why a spatial evaluator rather than a scalar
///
/// A tuned pass needs to be turned up in one part of a meadow and down in
/// another — that is what a modifier channel is *for*. A single number per pass
/// could only scale the whole plate, which is a global exposure control wearing
/// a population's name.
///
/// So a control is three spatial terms multiplied together: how readily this
/// pass takes the substrate under it, what its abundance channel says here, and
/// how the density the document asked for compares to what the style already
/// does. All three are one at a point the document says nothing about, which is
/// what keeps an unmodified meadow exactly as tuned.
#[derive(Clone, Debug, PartialEq)]
pub struct TunedPopulationControl {
    pub population: String,
    pub pass: TunedPass,
    /// How readily this pass takes each material, by index.
    pub material_affinity: Vec<(terrain_core::ids::MaterialIndex, f32)>,
    pub abundance_channel: Option<terrain_core::ids::ModifierIndex>,
    /// What the document asked for, per square metre.
    pub target_density_per_m2: f64,
    /// What the tuned style already does, per square metre.
    ///
    /// The denominator. A document asking for exactly this gets a factor of
    /// one and the picture does not move.
    pub reference_density_per_m2: f64,
}

impl TunedPopulationControl {
    /// How much the density request scales this pass.
    pub fn density_factor(&self) -> f32 {
        if self.reference_density_per_m2 <= 0.0 {
            return 1.0;
        }
        (self.target_density_per_m2 / self.reference_density_per_m2).max(0.0) as f32
    }

    /// How readily this pass takes an already-realised substrate.
    ///
    /// An empty affinity table means "anywhere", which is what a pass a
    /// document has not opinionated about should do.
    pub fn affinity_for(&self, substrate: &crate::transition::RealisedSubstrate) -> f32 {
        if self.material_affinity.is_empty() {
            return 1.0;
        }
        self.material_affinity
            .iter()
            .map(|(material, weight)| substrate.weight_of(*material) * weight)
            .sum::<f32>()
            .max(0.0)
    }
}

/// Every tuned pass a document controls.
///
/// Keyed by pass, and at most one control each — the tuned generator has one
/// pass identity, so two populations claiming it would leave the density
/// semantics ambiguous. The compiler reports that rather than picking.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TunedPopulationSet {
    controls: std::collections::BTreeMap<TunedPass, TunedPopulationControl>,
}

impl TunedPopulationSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a control, returning the population that already claimed its pass.
    pub fn insert(&mut self, control: TunedPopulationControl) -> Option<String> {
        let existing = self
            .controls
            .get(&control.pass)
            .map(|held| held.population.clone());
        if existing.is_none() {
            self.controls.insert(control.pass, control);
        }
        existing
    }

    pub fn get(&self, pass: TunedPass) -> Option<&TunedPopulationControl> {
        self.controls.get(&pass)
    }

    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }

    pub fn len(&self) -> usize {
        self.controls.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&TunedPass, &TunedPopulationControl)> {
        self.controls.iter()
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;
    use terrain_core::ids::MaterialIndex;

    fn control(pass: TunedPass, density: f64) -> TunedPopulationControl {
        TunedPopulationControl {
            population: format!("p_{pass}"),
            pass,
            material_affinity: Vec::new(),
            abundance_channel: None,
            target_density_per_m2: density,
            reference_density_per_m2: pass.reference_density_per_m2(),
        }
    }

    #[test]
    fn asking_for_exactly_what_the_style_does_changes_nothing() {
        // The property that lets a document name a tuned pass without moving
        // the picture. If this were not one, adding a population declaration to
        // an existing document would retune the whole meadow.
        for pass in TunedPass::ALL {
            let at_reference = control(pass, pass.reference_density_per_m2());
            assert!(
                (at_reference.density_factor() - 1.0).abs() < 1.0e-6,
                "{pass}"
            );
        }
    }

    #[test]
    fn the_density_factor_is_monotone_and_never_negative() {
        let pass = TunedPass::Tuft;
        let half = control(pass, pass.reference_density_per_m2() * 0.5);
        let double = control(pass, pass.reference_density_per_m2() * 2.0);
        assert!(half.density_factor() < 1.0);
        assert!(double.density_factor() > 1.0);
        // A negative density is nonsense an author can write, and zero rather
        // than a negative multiplier is the honest reading of it.
        let negative = control(pass, -5.0);
        assert_eq!(negative.density_factor(), 0.0);
    }

    #[test]
    fn an_empty_affinity_table_means_anywhere() {
        let control = control(TunedPass::Fine, 100.0);
        let substrate = crate::transition::RealisedSubstrate::pure(MaterialIndex(3));
        assert_eq!(control.affinity_for(&substrate), 1.0);
    }

    #[test]
    fn an_affinity_table_is_a_veto_on_ground_it_does_not_name() {
        let mut control = control(TunedPass::Fine, 100.0);
        control.material_affinity = vec![(MaterialIndex(0), 1.0)];
        assert_eq!(
            control.affinity_for(&crate::transition::RealisedSubstrate::pure(MaterialIndex(
                0
            ))),
            1.0
        );
        assert_eq!(
            control.affinity_for(&crate::transition::RealisedSubstrate::pure(MaterialIndex(
                1
            ))),
            0.0
        );
    }

    #[test]
    fn one_population_per_pass_and_the_first_one_keeps_it() {
        // The compiler reports the collision; this is the structure that makes
        // it detectable. First rather than last, so three claimants all report
        // against the same original.
        let mut set = TunedPopulationSet::new();
        assert_eq!(set.insert(control(TunedPass::Tuft, 50.0)), None);
        let clash = set.insert(TunedPopulationControl {
            population: "second".into(),
            ..control(TunedPass::Tuft, 90.0)
        });
        assert_eq!(clash.as_deref(), Some("p_tuft"));
        assert_eq!(set.len(), 1);
        assert_eq!(
            set.get(TunedPass::Tuft).map(|c| c.target_density_per_m2),
            Some(50.0)
        );
    }

    #[test]
    fn the_set_iterates_in_planting_order_rather_than_insertion_order() {
        // A `BTreeMap` keyed by pass, so a report reads mat, canopy, tufts,
        // leaves whichever order the document declared them in — and a
        // fingerprint over it does not depend on that order either.
        let mut set = TunedPopulationSet::new();
        for pass in [TunedPass::Broadleaf, TunedPass::Thatch, TunedPass::Tuft] {
            set.insert(control(pass, 10.0));
        }
        let order: Vec<TunedPass> = set.iter().map(|(pass, _)| *pass).collect();
        assert_eq!(
            order,
            vec![TunedPass::Thatch, TunedPass::Tuft, TunedPass::Broadleaf]
        );
    }
}

//! Which representation carries each piece of the ground's relief.
//!
//! ## The failure this replaces
//!
//! Until now the ground had one semantic evaluator and *two* procedural relief
//! implementations. Rust built the mesh-scale bands from addressed value noise
//! and a monotone aggregate transform; the Blender material rebuilt the
//! sub-mesh bands from Blender's own noise and a folded ridge function. They
//! were never the same surface. A band that crossed the geometry/bump threshold
//! — because the camera moved closer, or the mesh budget changed — changed its
//! phase, its spectrum, its morphology and its response to state.
//!
//! That violates the point of a representation ladder. Moving a band between
//! tiers is supposed to change *how* the ground is carried, never *what the
//! ground is*.
//!
//! ## One owner per contribution, recorded
//!
//! A plan assigns every band exactly one tier and says why. Nothing is drawn
//! twice, nothing is silently dropped, and the assignment is fingerprinted so a
//! render can be asked which ladder it was built against.
//!
//! ```text
//! Geometry     λ ≥ 4Δg          the mesh displaces it
//! Bump         λ < 4Δg, λ ≥ 2p  a Rust-authored height field bumps it
//! Microfacet   λ < 2p           it survives only as roughness
//! ```
//!
//! `Δg` is the ground mesh lattice; `p` is the traced world-space pixel. Both
//! come from the render, not from the profile — a profile cannot know how close
//! the camera is, which is exactly why the tier is a *plan* rather than a field
//! on the band.
//!
//! ## Why a budget reclassification is written down
//!
//! The bump field has to sample every band assigned to it at four samples per
//! wavelength or better. When a memory budget forces a coarser lattice, the
//! bands that no longer meet that are moved to microfacet — and the plan records
//! that it happened. A silently undersampled bump field looks like a band that
//! got quieter, which is indistinguishable from a band somebody deliberately
//! turned down.

use terrain_core::digest::{Digest, Fingerprint};
use terrain_core::ground_material::{GroundMaterialProfile, ReliefBand};

/// How many samples across a wavelength a lattice needs to carry a band.
///
/// Four. Two is Nyquist and produces a triangle wave whose peaks land wherever
/// the lattice happens to fall, so the same clod moves when the window moves.
/// The same constant governs the mesh and the bump field, because they are the
/// same question asked of two lattices.
pub const SAMPLES_PER_WAVELENGTH: f32 = 4.0;

/// Which representation carries a contribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReliefTier {
    /// Displaced mesh vertices.
    Geometry,
    /// A Rust-authored height field the shader bumps from.
    Bump,
    /// Below a pixel: it survives as roughness and nothing else.
    Microfacet,
}

impl ReliefTier {
    pub fn name(self) -> &'static str {
        match self {
            Self::Geometry => "geometry",
            Self::Bump => "bump",
            Self::Microfacet => "microfacet",
        }
    }
}

/// One band, and the tier that owns it.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedBand {
    pub profile: String,
    pub band_index: u16,
    pub wavelength_m: f32,
    pub amplitude_m: f32,
    pub tier: ReliefTier,
    /// Why this tier, in one line, so a report can be read without the code.
    pub reason: String,
    /// Whether a budget moved it here from a finer tier.
    pub reclassified: bool,
}

/// Every relief contribution in a render, with exactly one owner each.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundReliefPlan {
    /// The ground mesh lattice.
    pub geometry_spacing_m: f32,
    /// The bump field lattice, when there is a bump tier at all.
    pub bump_spacing_m: Option<f32>,
    /// One traced pixel, in world metres.
    pub traced_pixel_m: f32,
    pub bands: Vec<PlannedBand>,
    /// Whether a memory budget forced any band down a tier.
    pub budget_reclassified: bool,
    pub fingerprint: Fingerprint,
}

/// The most samples a bump plane may hold on one axis.
///
/// A nine-tile plate at a fifth of a millimetre is tens of millions of texels
/// per plane, which is gigabytes of float image before Blender has drawn
/// anything. The bound is on the lattice; the consequence is that the finest
/// bands are reclassified, and the plan says so.
pub const MAX_BUMP_SAMPLES_PER_AXIS: usize = 4096;

impl GroundReliefPlan {
    /// Assign every band of every profile to exactly one tier.
    ///
    /// `geometry_spacing_m` is the ground mesh lattice, `traced_pixel_m` is one
    /// rendered pixel in world metres, and `bump_samples_per_axis` is how many
    /// texels the bump plane may span — the memory budget.
    pub fn resolve<'a>(
        profiles: impl IntoIterator<Item = &'a GroundMaterialProfile>,
        geometry_spacing_m: f32,
        traced_pixel_m: f32,
        world_span_m: f32,
    ) -> Self {
        let geometry_cut = geometry_spacing_m * SAMPLES_PER_WAVELENGTH;
        let pixel_cut = traced_pixel_m * 2.0;

        let mut bands: Vec<PlannedBand> = Vec::new();
        for profile in profiles {
            for (index, band) in profile.structure.bands.iter().enumerate() {
                let (tier, reason) = if band.wavelength_m >= geometry_cut {
                    (
                        ReliefTier::Geometry,
                        format!(
                            "{:.4} m is at or above {geometry_cut:.4} m, which the mesh resolves",
                            band.wavelength_m
                        ),
                    )
                } else if band.wavelength_m >= pixel_cut {
                    (
                        ReliefTier::Bump,
                        format!(
                            "{:.4} m is below the mesh cut but at or above {pixel_cut:.4} m, \
                             which a pixel resolves",
                            band.wavelength_m
                        ),
                    )
                } else {
                    (
                        ReliefTier::Microfacet,
                        format!(
                            "{:.4} m is below {pixel_cut:.4} m, so it cannot be seen as shape",
                            band.wavelength_m
                        ),
                    )
                };
                bands.push(PlannedBand {
                    profile: profile.key.as_str().to_string(),
                    band_index: index as u16,
                    wavelength_m: band.wavelength_m,
                    amplitude_m: band.amplitude_m,
                    tier,
                    reason,
                    reclassified: false,
                });
            }
        }

        // The bump lattice has to carry its finest assigned band at four samples
        // per wavelength. Derived from the bands rather than chosen, then held
        // against the budget.
        let finest_bump = bands
            .iter()
            .filter(|band| band.tier == ReliefTier::Bump)
            .map(|band| band.wavelength_m)
            .fold(f32::INFINITY, f32::min);

        let mut bump_spacing = None;
        let mut reclassified = false;
        if finest_bump.is_finite() {
            let wanted = finest_bump / SAMPLES_PER_WAVELENGTH;
            let affordable = world_span_m / MAX_BUMP_SAMPLES_PER_AXIS as f32;
            let spacing = wanted.max(affordable);
            // Anything the affordable lattice can no longer carry goes down a
            // tier rather than being silently undersampled: an undersampled
            // bump field reads as a band somebody turned down.
            let carried = spacing * SAMPLES_PER_WAVELENGTH;
            for band in &mut bands {
                if band.tier == ReliefTier::Bump && band.wavelength_m < carried {
                    band.tier = ReliefTier::Microfacet;
                    band.reclassified = true;
                    band.reason = format!(
                        "{:.4} m needs a {:.5} m bump lattice; the budget allows {spacing:.5} m, \
                         so it drops to roughness",
                        band.wavelength_m,
                        band.wavelength_m / SAMPLES_PER_WAVELENGTH
                    );
                    reclassified = true;
                }
            }
            if bands.iter().any(|band| band.tier == ReliefTier::Bump) {
                bump_spacing = Some(spacing);
            }
        }

        let mut plan = Self {
            geometry_spacing_m,
            bump_spacing_m: bump_spacing,
            traced_pixel_m,
            bands,
            budget_reclassified: reclassified,
            fingerprint: Fingerprint::from_u128(0),
        };
        plan.fingerprint = plan.compute_fingerprint();
        plan
    }

    fn compute_fingerprint(&self) -> Fingerprint {
        let mut digest = Digest::for_domain("ground-relief-plan");
        digest
            .f32(self.geometry_spacing_m)
            .f32(self.bump_spacing_m.unwrap_or(0.0))
            .f32(self.traced_pixel_m);
        for band in &self.bands {
            digest
                .str(&band.profile)
                .u32(band.band_index as u32)
                .f32(band.wavelength_m)
                .f32(band.amplitude_m)
                .tag(band.tier as u8);
        }
        digest.finish()
    }

    /// The bands one profile carries in one tier, in declaration order.
    pub fn bands_in(&self, profile: &str, tier: ReliefTier) -> Vec<&PlannedBand> {
        self.bands
            .iter()
            .filter(|band| band.profile == profile && band.tier == tier)
            .collect()
    }

    /// How many contributions each tier owns.
    pub fn tier_counts(&self) -> [usize; 3] {
        let mut counts = [0usize; 3];
        for band in &self.bands {
            counts[band.tier as usize] += 1;
        }
        counts
    }

    /// Everything wrong with this plan.
    ///
    /// The invariants a package is refused for: a band in two tiers, or in
    /// none. Both are silent at every layer that carries them — a band drawn
    /// twice is a surface with double the relief it declared, and a band drawn
    /// nowhere is a surface that quietly lost a scale.
    pub fn problems(&self, expected_bands: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen: std::collections::BTreeSet<(&str, u16)> = std::collections::BTreeSet::new();
        for band in &self.bands {
            if !seen.insert((band.profile.as_str(), band.band_index)) {
                out.push(format!(
                    "{} band {} is assigned to more than one tier",
                    band.profile, band.band_index
                ));
            }
        }
        if self.bands.len() != expected_bands {
            out.push(format!(
                "the plan covers {} contributions and the profiles declare {expected_bands}",
                self.bands.len()
            ));
        }
        if self.bump_spacing_m.is_none()
            && self.bands.iter().any(|band| band.tier == ReliefTier::Bump)
        {
            out.push(
                "a band is assigned to the bump tier but the plan has no bump lattice".to_string(),
            );
        }
        if let Some(spacing) = self.bump_spacing_m {
            for band in self.bands.iter().filter(|b| b.tier == ReliefTier::Bump) {
                if band.wavelength_m < spacing * SAMPLES_PER_WAVELENGTH {
                    out.push(format!(
                        "{} band {} at {:.4} m is undersampled by a {spacing:.5} m bump lattice",
                        band.profile, band.band_index, band.wavelength_m
                    ));
                }
            }
        }
        out
    }

    /// A one-line-per-band table, for a report.
    pub fn to_table(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let counts = self.tier_counts();
        let _ = writeln!(
            out,
            "relief plan {} — mesh {:.4} m, bump {}, pixel {:.5} m",
            self.fingerprint.short(),
            self.geometry_spacing_m,
            self.bump_spacing_m
                .map(|s| format!("{s:.5} m"))
                .unwrap_or_else(|| "none".into()),
            self.traced_pixel_m
        );
        let _ = writeln!(
            out,
            "  {} geometry, {} bump, {} microfacet",
            counts[0], counts[1], counts[2]
        );
        for band in &self.bands {
            let _ = writeln!(
                out,
                "  {:<18} band {} at {:.4} m -> {:<10} {}",
                band.profile,
                band.band_index,
                band.wavelength_m,
                band.tier.name(),
                band.reason
            );
        }
        out
    }
}

/// The state scale one band survives.
///
/// Factored out so geometry, bump, cavity, unresolved slope and every debug AOV
/// call the same function. Two implementations of this would let a band flatten
/// under compaction in the mesh and not in the shader, which is the same class
/// of bug the tier ladder exists to remove — a band that is one thing when
/// displaced and another when bumped.
pub fn band_state_scale(band: &ReliefBand, compaction: f32, moisture: f32, flattening: f32) -> f32 {
    let packed = (1.0 - band.compaction_response * compaction).clamp(0.0, 1.0);
    let wet = (1.0 - flattening * moisture).clamp(0.0, 1.0);
    packed * wet
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::ground_material::AggregateShape;

    fn band(wavelength_m: f32, amplitude_m: f32) -> ReliefBand {
        ReliefBand {
            wavelength_m,
            amplitude_m,
            shape: AggregateShape::Rounded,
            compaction_response: 0.5,
            clustered: false,
        }
    }

    fn profile(bands: Vec<ReliefBand>) -> GroundMaterialProfile {
        let mut profile = crate::ground::tests_support::loam();
        profile.structure.bands = bands;
        profile
    }

    #[test]
    fn every_band_lands_in_exactly_one_tier() {
        // The invariant the whole plan exists for. A band in two tiers is a
        // surface with double the relief it declared; a band in none is a
        // surface that quietly lost a scale. Both are silent everywhere else.
        let profile = profile(vec![
            band(0.05, 0.016),
            band(0.014, 0.004),
            band(0.004, 0.001),
        ]);
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0007, 6.0);
        assert!(plan.problems(3).is_empty(), "{:?}", plan.problems(3));
        assert_eq!(plan.bands.len(), 3);
        let counts = plan.tier_counts();
        assert_eq!(counts.iter().sum::<usize>(), 3);
    }

    #[test]
    fn the_thresholds_are_the_ladder_they_claim_to_be() {
        // Mesh at 1 cm carries 4 cm and up; a 0.7 mm pixel carries 1.4 mm and
        // up as bump; anything finer is roughness only.
        let profile = profile(vec![
            band(0.05, 0.016),   // geometry
            band(0.014, 0.004),  // bump
            band(0.001, 0.0003), // microfacet
        ]);
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0007, 6.0);
        assert_eq!(plan.bands[0].tier, ReliefTier::Geometry);
        assert_eq!(plan.bands[1].tier, ReliefTier::Bump);
        assert_eq!(plan.bands[2].tier, ReliefTier::Microfacet);
    }

    #[test]
    fn a_band_exactly_on_a_threshold_takes_the_coarser_tier() {
        // `>=` rather than `>`, so a band sitting exactly at four samples per
        // wavelength is carried rather than dropped. The alternative makes the
        // ladder's behaviour depend on a float comparison at the boundary.
        let profile = profile(vec![band(0.04, 0.01)]);
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0007, 6.0);
        assert_eq!(plan.bands[0].tier, ReliefTier::Geometry);
    }

    #[test]
    fn the_bump_lattice_carries_its_finest_band_at_four_samples() {
        let profile = profile(vec![band(0.05, 0.016), band(0.008, 0.002)]);
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0005, 2.0);
        let spacing = plan.bump_spacing_m.expect("a bump band implies a lattice");
        assert!(
            spacing <= 0.008 / SAMPLES_PER_WAVELENGTH + 1.0e-9,
            "{spacing} does not carry an 8 mm band"
        );
        assert!(plan.problems(2).is_empty());
    }

    #[test]
    fn a_budget_that_cannot_carry_a_band_moves_it_down_and_says_so() {
        // The silent-undersampling failure, made loud. A bump field sampled
        // below four per wavelength reads as a band somebody turned down, which
        // is indistinguishable from a deliberate change.
        let profile = profile(vec![band(0.05, 0.016), band(0.002, 0.0005)]);
        // A hundred metres of ground: the affordable lattice is far coarser
        // than a 2 mm band needs.
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.02, 0.0002, 100.0);
        let fine = &plan.bands[1];
        assert_eq!(fine.tier, ReliefTier::Microfacet);
        assert!(fine.reclassified);
        assert!(plan.budget_reclassified);
        assert!(fine.reason.contains("budget"), "{}", fine.reason);
        assert!(plan.problems(2).is_empty());
    }

    #[test]
    fn a_plan_with_no_bump_band_has_no_bump_lattice() {
        // Rather than an unused lattice in the manifest, which a reader would
        // allocate a plane for.
        let profile = profile(vec![band(0.05, 0.016)]);
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0007, 6.0);
        assert_eq!(plan.bump_spacing_m, None);
        assert!(plan.problems(1).is_empty());
    }

    #[test]
    fn the_fingerprint_moves_with_every_decision_it_records() {
        let profile = profile(vec![band(0.05, 0.016), band(0.008, 0.002)]);
        let base = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0005, 2.0);

        // A finer mesh moves a band from bump to geometry, which is a different
        // ladder and must be a different fingerprint.
        let finer = GroundReliefPlan::resolve(std::iter::once(&profile), 0.002, 0.0005, 2.0);
        assert_ne!(base.fingerprint, finer.fingerprint);

        // A coarser pixel moves a band from bump to microfacet.
        let coarser = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.01, 2.0);
        assert_ne!(base.fingerprint, coarser.fingerprint);

        // And the same inputs give the same answer.
        assert_eq!(
            base.fingerprint,
            GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0005, 2.0).fingerprint
        );
    }

    #[test]
    fn a_plan_that_lost_a_band_is_reported() {
        let profile = profile(vec![band(0.05, 0.016)]);
        let plan = GroundReliefPlan::resolve(std::iter::once(&profile), 0.01, 0.0007, 6.0);
        let problems = plan.problems(2);
        assert!(
            problems.iter().any(|p| p.contains("declare 2")),
            "{problems:?}"
        );
    }

    #[test]
    fn state_scale_is_one_when_nothing_has_happened_to_the_ground() {
        let band = band(0.05, 0.016);
        assert_eq!(band_state_scale(&band, 0.0, 0.0, 0.45), 1.0);
        // And compaction bites by the band's own declared response.
        assert!((band_state_scale(&band, 1.0, 0.0, 0.45) - 0.5).abs() < 1.0e-6);
        // Saturation by the profile's flattening, independently.
        assert!((band_state_scale(&band, 0.0, 1.0, 0.45) - 0.55).abs() < 1.0e-6);
        // And the two multiply rather than either winning.
        assert!((band_state_scale(&band, 1.0, 1.0, 0.45) - 0.275).abs() < 1.0e-6);
    }

    #[test]
    fn state_scale_never_goes_negative() {
        // A response above one would otherwise invert the band, which reads as
        // clods becoming pits under a heavy boot.
        let mut band = band(0.05, 0.016);
        band.compaction_response = 2.0;
        assert_eq!(band_state_scale(&band, 1.0, 0.0, 0.0), 0.0);
        assert_eq!(band_state_scale(&band, 1.0, 1.0, 2.0), 0.0);
    }
}

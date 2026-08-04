//! What the terrain is at a point.
//!
//! ## A sample has a footprint, from version one
//!
//! [`SampleQuery`] carries the *area* a sample covers, not just its centre, and
//! that is here now rather than later because adding it later changes every
//! answer in the framework. A bake texel covers ground; a mask read at a
//! mathematical point aliases, a path edge read at a point has no width to
//! antialias against, and noise read at a point cannot know which octaves are
//! below the sampling rate. Retrofitting the footprint would move path widths,
//! mask filtering and every level-of-detail decision at once — which is not a
//! change anybody can review.
//!
//! Most callers pass [`SampleFootprint::Point`] and most sources ignore it.
//! That is fine. The cost of carrying it is a few bytes; the cost of not having
//! it is a migration.
//!
//! ## Material weights are normalised, and the type enforces it
//!
//! [`MaterialWeightSet`] cannot be constructed holding anything that breaks its
//! invariants. Every weight is finite, non-negative, sums to one, holds no
//! zeroes, and is ordered by material index.
//!
//! Each of those is load-bearing:
//!
//! - **Finite and non-negative**, because a weight is a proportion of ground and
//!   there is no such thing as minus a third of a square metre. A NaN weight
//!   propagates into every downstream blend and turns a whole page black.
//! - **Normalised**, so a consumer never has to ask whether it should divide by
//!   the sum. Two consumers that answer that differently is how a boundary ends
//!   up in two places.
//! - **Zero-pruned**, because iterating a set of twenty materials of which
//!   eighteen are zero is the inner loop of everything, and because "present
//!   with weight zero" and "absent" are the same thing and should not be two
//!   states.
//! - **Ordered**, so two samples of the same ground compare equal and digest
//!   equal regardless of the order the layers happened to contribute in.
//!
//! ## Modifiers are a dense array, materials are a sparse list
//!
//! Deliberately different shapes, for a reason that shows up at scale. A
//! document declares a handful of channels and *every point has a value for
//! every one of them* — a default is still a value. A document may declare many
//! materials and any given point is made of one or two. So modifiers are an
//! array indexed by channel, and materials are a pruned list.

use crate::coords::{WorldPoint, WorldVector};
use crate::ids::{MaterialIndex, ModifierIndex};

/// The area one sample covers.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SampleFootprint {
    /// A mathematical point. What a debug probe wants, and what most callers
    /// pass.
    #[default]
    Point,
    /// An ellipse, given by its two semi-axes in world metres.
    ///
    /// Two axes rather than a radius because a texel's footprint on the ground
    /// is not round under an isometric projection — it is stretched along one
    /// screen axis — and a circular approximation blurs across the path edge in
    /// one direction while aliasing along it in the other.
    Ellipse {
        axis_u: WorldVector,
        axis_v: WorldVector,
    },
}

impl SampleFootprint {
    /// A circular footprint of the given radius.
    pub fn circle(radius_m: f64) -> Self {
        Self::Ellipse {
            axis_u: WorldVector::new(radius_m, 0.0),
            axis_v: WorldVector::new(0.0, radius_m),
        }
    }

    /// A single number for a source that only wants to know "how big".
    ///
    /// The larger semi-axis, which is the conservative choice: a source using
    /// this to decide how many noise octaves are below the sampling rate should
    /// err toward dropping one rather than aliasing.
    pub fn radius_m(self) -> f64 {
        match self {
            Self::Point => 0.0,
            Self::Ellipse { axis_u, axis_v } => axis_u.length().max(axis_v.length()),
        }
    }

    pub fn is_point(self) -> bool {
        matches!(self, Self::Point)
    }
}

/// Which parts of a sample the caller actually wants.
///
/// A scatter deciding whether to place a flower needs material weights and one
/// modifier; it does not need microrelief gradients or feature geometry, and
/// computing them is not free. This is how it says so.
///
/// A hand-rolled bitset rather than a dependency: five flags do not justify one,
/// and the exhaustive constructors below are more readable at a call site than
/// an `or` chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleChannels(u8);

impl SampleChannels {
    pub const MATERIALS: Self = Self(1 << 0);
    pub const ELEVATION: Self = Self(1 << 1);
    pub const MICRORELIEF: Self = Self(1 << 2);
    pub const MODIFIERS: Self = Self(1 << 3);
    pub const FEATURES: Self = Self(1 << 4);

    /// Everything.
    pub const ALL: Self = Self(0b1_1111);

    /// What a population scatter needs: what the ground is, and how much of this
    /// thing grows on it.
    pub const SCATTER: Self = Self(Self::MATERIALS.0 | Self::MODIFIERS.0);

    /// What a ground surface needs.
    pub const SURFACE: Self =
        Self(Self::MATERIALS.0 | Self::ELEVATION.0 | Self::MICRORELIEF.0 | Self::MODIFIERS.0);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl Default for SampleChannels {
    fn default() -> Self {
        Self::ALL
    }
}

/// What to sample, where, and how much of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleQuery {
    pub position: WorldPoint,
    pub footprint: SampleFootprint,
    pub channels: SampleChannels,
}

impl SampleQuery {
    /// Everything, at a point.
    pub fn at(position: WorldPoint) -> Self {
        Self {
            position,
            footprint: SampleFootprint::Point,
            channels: SampleChannels::ALL,
        }
    }

    pub fn with_footprint(mut self, footprint: SampleFootprint) -> Self {
        self.footprint = footprint;
        self
    }

    pub fn with_channels(mut self, channels: SampleChannels) -> Self {
        self.channels = channels;
        self
    }
}

/// How much of one material is here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialWeight {
    pub material: MaterialIndex,
    pub weight: f32,
}

/// Everything this ground is made of, normalised.
///
/// Construct through [`MaterialWeightSet::from_scores`], which is the only way
/// in and which enforces every invariant in the module note.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MaterialWeightSet {
    weights: Vec<MaterialWeight>,
}

/// Below this, a material is treated as absent.
///
/// Not zero, and the difference matters. Scores arrive from smooth masks, so a
/// material's weight goes to zero *asymptotically* rather than reaching it — and
/// without a floor, a boundary carries a long tail of materials at weights of
/// `1e-9`, each of which costs a full evaluation in every consumer and none of
/// which is visible. A thousandth is well below what any renderer can show and
/// well above the noise.
pub const WEIGHT_EPSILON: f32 = 1.0e-3;

impl MaterialWeightSet {
    /// The empty set: ground made of nothing.
    ///
    /// Reachable, and validation refuses documents that produce it — but a
    /// sampler still has to return *something* for a point outside every layer's
    /// mask, and a caller has to be able to ask whether that happened.
    pub const fn empty() -> Self {
        Self {
            weights: Vec::new(),
        }
    }

    /// Normalise a list of scores into a weight set.
    ///
    /// Scores are what layers accumulate: unbounded, unordered, and free to be
    /// anything an author wrote. This is the one place they become weights, and
    /// everything the type promises is established here rather than trusted.
    ///
    /// Non-finite and negative scores are dropped rather than clamped. Clamping
    /// a NaN to zero would silently turn a broken layer into an absent one;
    /// dropping it means the material is missing, which is the same visible
    /// outcome and is at least honest about the arithmetic. Validation is what
    /// catches the cause.
    pub fn from_scores(scores: impl IntoIterator<Item = (MaterialIndex, f32)>) -> Self {
        let mut weights: Vec<MaterialWeight> = scores
            .into_iter()
            .filter(|(_, score)| score.is_finite() && *score > 0.0)
            .map(|(material, weight)| MaterialWeight { material, weight })
            .collect();

        let total: f32 = weights.iter().map(|w| w.weight).sum();
        if !(total.is_finite() && total > 0.0) {
            return Self::empty();
        }
        for weight in &mut weights {
            weight.weight /= total;
        }

        // Prune, then renormalise what is left. Pruning first and renormalising
        // second is the order that matters: the other way round leaves the sum
        // slightly under one by exactly the weight that was dropped, and a
        // consumer that trusts the sum then darkens every boundary.
        weights.retain(|w| w.weight >= WEIGHT_EPSILON);
        let kept: f32 = weights.iter().map(|w| w.weight).sum();
        if !(kept.is_finite() && kept > 0.0) {
            return Self::empty();
        }
        for weight in &mut weights {
            weight.weight /= kept;
        }

        // Ordered by index, so two samples of the same ground compare and digest
        // equal whatever order the layers contributed in.
        weights.sort_by_key(|w| w.material);
        Self { weights }
    }

    /// One material, at full strength.
    pub fn solid(material: MaterialIndex) -> Self {
        Self {
            weights: vec![MaterialWeight {
                material,
                weight: 1.0,
            }],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    pub fn len(&self) -> usize {
        self.weights.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = MaterialWeight> + '_ {
        self.weights.iter().copied()
    }

    pub fn as_slice(&self) -> &[MaterialWeight] {
        &self.weights
    }

    /// How much of one material is here, zero if absent.
    pub fn weight_of(&self, material: MaterialIndex) -> f32 {
        self.weights
            .binary_search_by_key(&material, |w| w.material)
            .map(|index| self.weights[index].weight)
            .unwrap_or(0.0)
    }

    /// The material with the largest weight.
    ///
    /// Ties break toward the lower index, which is arbitrary and *stable* — and
    /// stable is the whole requirement. A dominant material that flickered
    /// between two equal claimants would put a boundary in a different place
    /// every time the ground was resampled.
    pub fn dominant(&self) -> Option<MaterialWeight> {
        self.weights.iter().copied().reduce(|best, next| {
            if next.weight > best.weight {
                next
            } else {
                best
            }
        })
    }

    /// The two strongest materials, for a boundary treatment.
    pub fn dominant_pair(&self) -> Option<(MaterialWeight, MaterialWeight)> {
        if self.weights.len() < 2 {
            return None;
        }
        let mut sorted = self.weights.clone();
        // By weight descending, then by index, so the pair is stable under ties.
        sorted.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.material.cmp(&b.material))
        });
        Some((sorted[0], sorted[1]))
    }

    /// How mixed this ground is, `0..1`.
    ///
    /// Zero where one material owns the ground, rising toward one where two
    /// share it evenly. What a transition recipe keys on, and what a debug view
    /// draws to show where the boundaries actually are as opposed to where the
    /// author thinks they are.
    pub fn blend(&self) -> f32 {
        match self.dominant() {
            None => 0.0,
            Some(dominant) => (1.0 - dominant.weight) * 2.0,
        }
        .clamp(0.0, 1.0)
    }
}

/// Fine displacement, and which way it slopes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MicroreliefSample {
    /// Displacement from the elevation surface, in metres.
    pub displacement_m: f32,
    /// The displacement's gradient, per metre, as `(du, dv)`.
    ///
    /// Carried rather than differenced by the consumer, because differencing
    /// costs two extra samples and because a source that knows its own
    /// derivative analytically gives a better answer than three point samples
    /// ever will.
    pub gradient: [f32; 2],
}

/// Every declared channel's value here.
///
/// A dense array indexed by [`ModifierIndex`]. See the module note for why this
/// is a different shape from the material weights.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ModifierSet {
    values: Vec<f32>,
}

impl ModifierSet {
    /// Every channel at the given defaults.
    pub fn from_defaults(defaults: &[f32]) -> Self {
        Self {
            values: defaults.to_vec(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// One channel's value.
    ///
    /// Returns `None` for an index this document does not have, rather than
    /// panicking or returning zero: an index from another prepared terrain is a
    /// caller error worth surfacing, and zero would be a plausible-looking
    /// answer that means "suppressed" for most channels.
    pub fn get(&self, channel: ModifierIndex) -> Option<f32> {
        self.values.get(channel.index()).copied()
    }

    /// One channel's value, or a stated fallback.
    pub fn get_or(&self, channel: ModifierIndex, fallback: f32) -> f32 {
        self.get(channel).unwrap_or(fallback)
    }

    pub fn set(&mut self, channel: ModifierIndex, value: f32) {
        if let Some(slot) = self.values.get_mut(channel.index()) {
            *slot = value;
        }
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.values
    }
}

/// What kind of junction a feature makes here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum JunctionClass {
    /// Not near a junction.
    #[default]
    None,
    /// The feature stops here.
    End,
    /// Two features meet at a T.
    Tee,
    /// Two features cross.
    Cross,
    /// One feature turns sharply.
    Bend,
}

/// Which feature this is near.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeatureId(pub u32);

/// Where this point sits relative to the nearest authored feature.
///
/// **Reserved, and mostly unread today.** It is here because the things it
/// enables all need the same information and none of them can compute it
/// afterwards: ruts aligned to a path, grass leaning away from a track, stones
/// following a boundary, a path that varies in width along its length, a dirt
/// fringe that is wider on the outside of a bend. Each of those is a small
/// recipe on top of this and an architectural change without it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureContext {
    pub feature_id: FeatureId,
    /// Distance to the feature's centreline. Negative inside a closed shape.
    pub signed_distance_m: f32,
    /// Unit direction the feature runs in here.
    pub tangent: [f32; 2],
    /// Unit direction away from the feature's centreline.
    pub normal: [f32; 2],
    /// How far along the feature the nearest point is, in metres from its start.
    ///
    /// What lets anything vary *along* a path rather than only across it.
    pub along_feature_m: f32,
    pub junction: JunctionClass,
}

/// Everything the terrain is at one point.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TerrainSample {
    pub material_weights: MaterialWeightSet,
    pub elevation_m: f32,
    pub microrelief: MicroreliefSample,
    pub modifiers: ModifierSet,
    /// The nearest authored feature, if any is in reach.
    pub feature_context: Option<FeatureContext>,
}

impl TerrainSample {
    /// The surface height: elevation plus fine displacement.
    ///
    /// The two are stored apart because they are filtered differently and mean
    /// different things — see `MicroreliefLayer` — and this is the one place
    /// that wants them added, so it is written down once.
    pub fn surface_height_m(&self) -> f32 {
        self.elevation_m + self.microrelief.displacement_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(index: u16) -> MaterialIndex {
        MaterialIndex(index)
    }

    #[test]
    fn weights_normalise_to_one() {
        let set = MaterialWeightSet::from_scores([(material(0), 3.0), (material(1), 1.0)]);
        let total: f32 = set.iter().map(|w| w.weight).sum();
        assert!((total - 1.0).abs() < 1.0e-6, "{total}");
        assert!((set.weight_of(material(0)) - 0.75).abs() < 1.0e-6);
        assert!((set.weight_of(material(1)) - 0.25).abs() < 1.0e-6);
    }

    #[test]
    fn weights_stay_normalised_after_pruning() {
        // The order that matters. Prune first and renormalise second, or the sum
        // comes back under one by exactly the weight dropped — and a consumer
        // that trusts the sum darkens every boundary by that much.
        let set = MaterialWeightSet::from_scores([
            (material(0), 1000.0),
            (material(1), 0.1),
            (material(2), 0.01),
        ]);
        let total: f32 = set.iter().map(|w| w.weight).sum();
        assert!((total - 1.0).abs() < 1.0e-6, "{total} after pruning");
    }

    #[test]
    fn a_vanishing_material_is_dropped_rather_than_carried() {
        // Smooth masks approach zero asymptotically, so without a floor every
        // boundary carries a tail of materials at 1e-9 — each costing a full
        // evaluation in every consumer, none of them visible.
        let set = MaterialWeightSet::from_scores([(material(0), 1.0), (material(1), 1.0e-9)]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.weight_of(material(1)), 0.0);
    }

    #[test]
    fn weights_are_ordered_by_material_however_they_arrived() {
        // So two samples of the same ground compare and digest equal whatever
        // order the layers contributed in.
        let forward = MaterialWeightSet::from_scores([(material(0), 1.0), (material(3), 1.0)]);
        let backward = MaterialWeightSet::from_scores([(material(3), 1.0), (material(0), 1.0)]);
        assert_eq!(forward, backward);
        assert_eq!(
            forward
                .as_slice()
                .iter()
                .map(|w| w.material)
                .collect::<Vec<_>>(),
            [material(0), material(3)]
        );
    }

    #[test]
    fn a_broken_score_cannot_reach_a_weight() {
        // A NaN weight propagates into every downstream blend and turns a whole
        // page black, so it is dropped at the boundary rather than clamped.
        let set = MaterialWeightSet::from_scores([
            (material(0), 1.0),
            (material(1), f32::NAN),
            (material(2), -3.0),
            (material(3), f32::INFINITY),
        ]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.weight_of(material(0)), 1.0);
        assert!(set.iter().all(|w| w.weight.is_finite() && w.weight > 0.0));
    }

    #[test]
    fn ground_made_of_nothing_is_representable_and_empty() {
        assert!(MaterialWeightSet::from_scores([]).is_empty());
        assert!(MaterialWeightSet::from_scores([(material(0), 0.0)]).is_empty());
        assert!(MaterialWeightSet::from_scores([(material(0), f32::NAN)]).is_empty());
        assert_eq!(MaterialWeightSet::empty().dominant(), None);
        assert_eq!(MaterialWeightSet::empty().blend(), 0.0);
    }

    #[test]
    fn the_dominant_material_is_stable_under_a_tie() {
        // A boundary that moved every time the ground was resampled would be
        // worse than a boundary in the wrong place.
        let set = MaterialWeightSet::from_scores([(material(2), 1.0), (material(5), 1.0)]);
        let first = set.dominant().expect("two materials");
        for _ in 0..8 {
            assert_eq!(set.dominant(), Some(first));
        }
        assert_eq!(first.material, material(2), "the tie did not break low");
    }

    #[test]
    fn blend_reads_zero_on_solid_ground_and_one_on_an_even_mix() {
        let solid = MaterialWeightSet::solid(material(0));
        assert_eq!(solid.blend(), 0.0);
        let even = MaterialWeightSet::from_scores([(material(0), 1.0), (material(1), 1.0)]);
        assert!((even.blend() - 1.0).abs() < 1.0e-6, "{}", even.blend());
        let lopsided = MaterialWeightSet::from_scores([(material(0), 3.0), (material(1), 1.0)]);
        assert!((lopsided.blend() - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn a_dominant_pair_needs_two_materials() {
        assert_eq!(MaterialWeightSet::solid(material(0)).dominant_pair(), None);
        let mixed = MaterialWeightSet::from_scores([
            (material(0), 1.0),
            (material(1), 5.0),
            (material(2), 3.0),
        ]);
        let (first, second) = mixed.dominant_pair().expect("three materials");
        assert_eq!(first.material, material(1));
        assert_eq!(second.material, material(2));
    }

    #[test]
    fn a_footprint_reports_its_own_size() {
        assert_eq!(SampleFootprint::Point.radius_m(), 0.0);
        assert!(SampleFootprint::Point.is_point());
        assert_eq!(SampleFootprint::circle(0.5).radius_m(), 0.5);
        // The larger axis, so a source dropping octaves errs toward dropping one
        // rather than aliasing.
        let stretched = SampleFootprint::Ellipse {
            axis_u: WorldVector::new(0.2, 0.0),
            axis_v: WorldVector::new(0.0, 0.8),
        };
        assert_eq!(stretched.radius_m(), 0.8);
    }

    #[test]
    fn channel_sets_compose_and_answer_what_they_hold() {
        assert!(SampleChannels::ALL.contains(SampleChannels::MATERIALS));
        assert!(SampleChannels::ALL.contains(SampleChannels::FEATURES));
        assert!(SampleChannels::SCATTER.contains(SampleChannels::MATERIALS));
        assert!(!SampleChannels::SCATTER.contains(SampleChannels::FEATURES));
        assert!(
            SampleChannels::MATERIALS
                .union(SampleChannels::FEATURES)
                .contains(SampleChannels::FEATURES)
        );
        assert!(SampleChannels::SURFACE.contains(SampleChannels::ELEVATION));
    }

    #[test]
    fn a_modifier_index_from_another_document_reports_rather_than_lying() {
        // Zero would be a plausible-looking answer that reads as "suppressed"
        // for most channels.
        let modifiers = ModifierSet::from_defaults(&[1.0, 0.08]);
        assert_eq!(modifiers.get(ModifierIndex(0)), Some(1.0));
        assert_eq!(modifiers.get(ModifierIndex(1)), Some(0.08));
        assert_eq!(modifiers.get(ModifierIndex(9)), None);
        assert_eq!(modifiers.get_or(ModifierIndex(9), 1.0), 1.0);
    }

    #[test]
    fn setting_a_channel_out_of_range_does_nothing_rather_than_growing_the_set() {
        let mut modifiers = ModifierSet::from_defaults(&[1.0]);
        modifiers.set(ModifierIndex(4), 0.5);
        assert_eq!(modifiers.len(), 1);
        assert_eq!(modifiers.get(ModifierIndex(0)), Some(1.0));
    }

    #[test]
    fn surface_height_adds_the_two_channels_that_mean_different_things() {
        let sample = TerrainSample {
            elevation_m: 2.0,
            microrelief: MicroreliefSample {
                displacement_m: -0.06,
                gradient: [0.0, 0.0],
            },
            ..TerrainSample::default()
        };
        assert!((sample.surface_height_m() - 1.94).abs() < 1.0e-6);
    }
}

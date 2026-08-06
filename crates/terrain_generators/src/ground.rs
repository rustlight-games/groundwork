//! What the ground is, at a point, once.
//!
//! Every consumer of ground — the mesh that carries its relief, the shader that
//! colours it, the overlay that decides how much grass grows on it, the scatter
//! that puts stones on it, the corpus that conditions a neural renderer on it —
//! asks the same questions, and before this module they each answered them
//! separately. That is not merely duplication: it is the specific bug where the
//! grass thins in one place and the ground changes colour a centimetre away,
//! because two callers realised the same ragged boundary through two calls with
//! two different rounding histories.
//!
//! So there is one evaluator. [`GroundEvaluator::sample`] realises the substrate
//! **once**, resolves the state channels once, and hands back everything anyone
//! needs. Two consumers asking about the same world point get the same answer by
//! construction rather than by agreement.
//!
//! ## What replaced what
//!
//! The thing this supersedes was a pair of numbers: `earth`, meaning "one minus
//! however much of this is vegetated", and `wetness`. Both were computed by
//! hardcoded recipes with the clod scales written into the function body.
//!
//! "Not vegetated" is not a material identity. It cannot distinguish loam from
//! sand from clay from gravel, so every exposed surface in the world had to be
//! the same brown. The replacement carries the realised weight of *each*
//! substrate, and the profile bound to each one says what that substrate looks
//! like and how it responds.
//!
//! ## Which relief is geometry is decided here, not in the profile
//!
//! A [`ReliefBand`] declares a wavelength and an amplitude and says nothing
//! about whether it should be displaced or bumped, because that depends on the
//! sampling rate — which the profile cannot know. [`BandSplit`] takes a lattice
//! spacing and cuts the bands in two: everything the lattice resolves is
//! geometry, everything below it is the shader's.
//!
//! The cut is at four samples per wavelength. Two is the theoretical limit and
//! produces a triangle wave with a phase that shifts as the lattice moves;
//! four keeps the shape and, more importantly, keeps it *stable* — which matters
//! because the same ground is sampled by overlapping windows that must agree.

use std::sync::Arc;

use glam::Vec2;
use terrain_core::coords::WorldPoint;
use terrain_core::document::ModifierRole;
use terrain_core::ground_material::{AggregateShape, GroundMaterialProfile, ReliefBand};
use terrain_core::ids::{MaterialIndex, ModifierIndex};
use terrain_core::prepare::PreparedTerrain;
use terrain_scene::field::TerrainFieldStack;

use crate::rng::{Stream, fbm, value_noise};
use crate::transition::{RealisedSubstrate, TransitionProfile, realise};

/// How many samples across a wavelength a lattice needs before a band is
/// carried as geometry rather than as a bump.
///
/// Two is Nyquist and is not enough: a two-sample sine is a triangle whose peaks
/// land wherever the lattice happens to fall, so the same clod moves when the
/// window moves. Four holds its shape and its phase.
pub const SAMPLES_PER_WAVELENGTH: f32 = 4.0;

/// The condition the ground is in, as opposed to what it is made of.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GroundState {
    pub moisture: f32,
    pub compaction: f32,
    pub disturbance: f32,
    pub loose_material: f32,
    pub desiccation: f32,
    /// Derived, not declared: how the surface curves here. Negative is a hollow.
    pub curvature: f32,
    /// How much upstream drains through this point.
    pub flow: f32,
    /// How much sky it can see.
    pub exposure: f32,
}

/// Everything a consumer needs to know about one point of ground.
#[derive(Clone, Debug)]
pub struct GroundSample {
    /// The realised substrate weights — ragged, and shared with ownership.
    pub substrates: RealisedSubstrate,
    pub state: GroundState,
    /// How much this point supports plants, `0..1`.
    ///
    /// The weighted sum of each substrate's vegetation affinity. Never inferred
    /// from a material's name.
    pub vegetation_support: f32,
    /// Relief the mesh should carry, in metres.
    pub displacement_m: f32,
    /// How deep in its own relief this point sits, `0..1`.
    ///
    /// Zero on a crest, one at the bottom of a hollow. The single most important
    /// channel for making ground read as ground, and the one this system did not
    /// have for its first working render.
    ///
    /// ## The dark in soil is shadow, not pigment
    ///
    /// A photograph of bare earth beside grass spans **twenty times** between
    /// its darkest fifth-percentile and its brightest — and the dark end is
    /// nearly neutral in hue, because a crevice deep enough to be that dark is
    /// lit by sky rather than by sun. That is not a colour the soil has. It is
    /// occlusion between crumbs.
    ///
    /// A shader whose tone comes from noise *uncorrelated with its height* can
    /// reproduce neither. Widening the palette only makes louder mottling, and
    /// the surface reads as painted paper however much relief the mesh carries,
    /// because the shading and the form disagree about where the low ground is.
    ///
    /// So this comes out of the same band sum that makes the displacement. One
    /// field, two consumers — the same rule the whole evaluator exists for.
    pub cavity: f32,
    /// How wet the surface reads, `0..1`.
    pub wet_film: f32,
}

/// Which relief bands a given lattice can carry.
///
/// Computed once per export from the profiles actually in play, and reported,
/// because "the mesh silently stopped carrying the clods" is exactly the kind of
/// change that looks like a shader regression.
#[derive(Clone, Debug, PartialEq)]
pub struct BandSplit {
    pub spacing_m: f32,
    /// Bands the mesh carries, per profile key.
    pub geometry: Vec<(String, Vec<ReliefBand>)>,
    /// Bands left to the shader, per profile key.
    pub shader: Vec<(String, Vec<ReliefBand>)>,
    /// Every band, per profile key, in declaration order.
    ///
    /// Kept so a caller can ask "does the mesh carry band three" and get an
    /// answer in terms of the *declaration* index — which is what the band basis
    /// is addressed by, and therefore the only index that means anything
    /// outside this struct.
    pub all: Vec<(String, Vec<ReliefBand>)>,
}

impl BandSplit {
    /// Split every profile's bands at `spacing_m`.
    pub fn resolve<'a>(
        profiles: impl IntoIterator<Item = &'a GroundMaterialProfile>,
        spacing_m: f32,
    ) -> Self {
        let cut = spacing_m * SAMPLES_PER_WAVELENGTH;
        let mut geometry = Vec::new();
        let mut shader = Vec::new();
        let mut all = Vec::new();
        for profile in profiles {
            let key = profile.key.as_str().to_string();
            let (mesh, bump): (Vec<_>, Vec<_>) = profile
                .structure
                .bands
                .iter()
                .copied()
                .partition(|band| band.wavelength_m >= cut);
            all.push((key.clone(), profile.structure.bands.clone()));
            geometry.push((key.clone(), mesh));
            shader.push((key, bump));
        }
        Self {
            spacing_m,
            geometry,
            shader,
            all,
        }
    }

    /// The spacing at which every profile gets the relief it asked for.
    ///
    /// The reason this is derived rather than a constant: a document of hardpan
    /// and beach sand needs a finer lattice than one of turned farm soil, and a
    /// single number chosen for one of them aliases the other.
    ///
    /// A profile that declares a [`mesh_floor_m`] is asking for a *hierarchy* on
    /// the mesh rather than one band, and the lattice is sized to its floor. One
    /// that does not gets what this always did: fine enough for its coarsest band
    /// and no finer.
    ///
    /// [`mesh_floor_m`]: terrain_core::ground_material::GroundStructure::mesh_floor_m
    pub fn spacing_for<'a>(
        profiles: impl IntoIterator<Item = &'a GroundMaterialProfile>,
    ) -> Option<f32> {
        profiles
            .into_iter()
            .filter_map(|profile| {
                let coarsest = profile
                    .coarsest_band()
                    // A band with no amplitude has no shape to alias.
                    .filter(|band| band.amplitude_m > 0.0)?;
                // Never coarser than the coarsest band needs: a floor declared
                // above it would put the soil's own shape below the lattice.
                Some(
                    profile
                        .structure
                        .mesh_floor_m
                        .unwrap_or(coarsest.wavelength_m)
                        .min(coarsest.wavelength_m),
                )
            })
            .map(|wavelength| wavelength / SAMPLES_PER_WAVELENGTH)
            .fold(None, |best: Option<f32>, step| {
                Some(best.map_or(step, |b| b.min(step)))
            })
    }

    /// Whether the mesh carries one profile's band, by *declaration* index.
    ///
    /// Declaration index rather than position in the filtered list, because the
    /// band basis is addressed by it — see `GroundEvaluator::relief_of`. Matched
    /// on wavelength, which is unique within a profile because validation
    /// requires bands to be strictly descending.
    pub fn carries(&self, key: &str, declaration_index: usize) -> bool {
        let Some(band) = self
            .all
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, bands)| bands.get(declaration_index))
        else {
            return false;
        };
        self.geometry
            .iter()
            .find(|(k, _)| k == key)
            .is_some_and(|(_, carried)| {
                carried
                    .iter()
                    .any(|mesh| mesh.wavelength_m == band.wavelength_m)
            })
    }

    fn bands_for(&self, key: &str) -> &[ReliefBand] {
        self.geometry
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, bands)| bands.as_slice())
            .unwrap_or(&[])
    }
}

/// One ground, evaluated.
pub struct GroundEvaluator {
    fields: Arc<TerrainFieldStack>,
    transition: TransitionProfile,
    root_seed: u64,
    /// Profile and affinity per material index, dense so the inner loop indexes.
    materials: Vec<MaterialEntry>,
    split: BandSplit,
    roles: Roles,
    /// A laboratory: state held constant, and one material everywhere.
    ///
    /// A benchmark that wants to measure "the ground at compaction 0.75" cannot
    /// get there through a document: compaction is a modifier channel, and
    /// authoring one document per sweep point would make the sweep a test of the
    /// loader. So this exists, and it is `None` on every evaluator the compiler
    /// builds — [`GroundEvaluator::new`] cannot set it, and only
    /// [`GroundEvaluator::for_benchmark`] can. A production document has no way
    /// to reach it even by accident.
    ///
    /// It forces the *substrate* as well, and that half is not optional: a flat
    /// field stack carries no material weights, so a laboratory relying on the
    /// document's substrate would measure a ground made of nothing and report
    /// zero relief for every band it declared.
    laboratory: Option<GroundState>,
}

struct MaterialEntry {
    profile: Option<Arc<GroundMaterialProfile>>,
    affinity: f32,
    /// The profile key, so band lookup does not walk an `Arc` every sample.
    key: String,
    /// This soil's simulated surface — the mesh-scale relief.
    ///
    /// One per material rather than one per document, because two soils on the
    /// same plate settle differently and must not share a grid.
    surface: Option<Arc<crate::soil::SoilSurface>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Roles {
    moisture: Option<ModifierIndex>,
    compaction: Option<ModifierIndex>,
    disturbance: Option<ModifierIndex>,
    loose: Option<ModifierIndex>,
    desiccation: Option<ModifierIndex>,
    water_supply: Option<ModifierIndex>,
    vegetation_density: Option<ModifierIndex>,
    dead_litter: Option<ModifierIndex>,
}

impl std::fmt::Debug for GroundEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroundEvaluator")
            .field("materials", &self.materials.len())
            .field("spacing_m", &self.split.spacing_m)
            .finish()
    }
}

impl GroundEvaluator {
    /// Build an evaluator for a prepared terrain and a realised field stack.
    ///
    /// `spacing_m` is the lattice the ground will be sampled on, and it decides
    /// the band split. Pass [`BandSplit::spacing_for`] over the same profiles to
    /// let the materials choose it.
    pub fn new(
        terrain: &PreparedTerrain,
        fields: Arc<TerrainFieldStack>,
        transition: TransitionProfile,
        spacing_m: f32,
    ) -> Self {
        let root_seed = terrain.root_seed().bits();
        let materials: Vec<MaterialEntry> = (0..terrain.materials().len())
            .map(|index| {
                let index = MaterialIndex(index as u16);
                let profile = terrain.material_profile(index).cloned();
                MaterialEntry {
                    key: profile
                        .as_ref()
                        .map(|p| p.key.as_str().to_string())
                        .unwrap_or_default(),
                    affinity: terrain.vegetation_affinity(index),
                    surface: profile.as_ref().map(|p| {
                        Arc::new(crate::soil::SoilSurface::new(
                            Arc::clone(p),
                            root_seed ^ 0x_501_1_5_1_2,
                        ))
                    }),
                    profile,
                }
            })
            .collect();
        let split = BandSplit::resolve(
            materials.iter().filter_map(|m| m.profile.as_deref()),
            spacing_m,
        );
        Self {
            fields,
            transition,
            root_seed,
            materials,
            split,
            laboratory: None,
            roles: Roles {
                moisture: terrain.role_channel(ModifierRole::SoilMoisture),
                compaction: terrain.role_channel(ModifierRole::SoilCompaction),
                disturbance: terrain.role_channel(ModifierRole::SoilDisturbance),
                loose: terrain.role_channel(ModifierRole::LooseMaterial),
                desiccation: terrain.role_channel(ModifierRole::Desiccation),
                water_supply: terrain.role_channel(ModifierRole::WaterSupply),
                vegetation_density: terrain.role_channel(ModifierRole::VegetationDensity),
                dead_litter: terrain.role_channel(ModifierRole::DeadLitter),
            },
        }
    }

    /// An evaluator over bare fields: no materials, no profiles, no roles.
    ///
    /// What every answer degenerates to when a document names no ground
    /// profiles — empty substrate, default state, zero relief, full vegetation
    /// support — expressed directly rather than reached through a
    /// `PreparedTerrain` that has nothing to contribute.
    ///
    /// Its real use is fixtures. A recipe test wants to check that a stem comes
    /// out the right length, and making it construct a document, prepare it and
    /// resolve a profile library first would mean every geometry test also
    /// tested the loader.
    pub fn bare(
        fields: Arc<TerrainFieldStack>,
        transition: TransitionProfile,
        root_seed: u64,
    ) -> Self {
        Self {
            fields,
            transition,
            root_seed,
            materials: Vec::new(),
            split: BandSplit::resolve(std::iter::empty(), 0.04),
            roles: Roles::default(),
            laboratory: None,
        }
    }

    /// An evaluator over explicit profiles, at a state held constant.
    ///
    /// The laboratory constructor. A benchmark isolating one relief band needs a
    /// profile carrying exactly that band and a compaction it can sweep, and
    /// neither is expressible as a document without making the measurement a
    /// test of the loader as well.
    ///
    /// Deliberately separate from [`GroundEvaluator::new`] rather than a flag on
    /// it. A forced state reaching a production render would silently override
    /// every authored moisture channel in the document, and the picture would
    /// look plausible.
    pub fn for_benchmark(
        fields: Arc<TerrainFieldStack>,
        transition: TransitionProfile,
        root_seed: u64,
        profiles: Vec<Arc<GroundMaterialProfile>>,
        spacing_m: f32,
        compaction: f32,
        moisture: f32,
    ) -> Self {
        let materials: Vec<MaterialEntry> = profiles
            .into_iter()
            .map(|profile| MaterialEntry {
                key: profile.key.as_str().to_string(),
                affinity: profile.vegetation_affinity,
                surface: Some(Arc::new(crate::soil::SoilSurface::new(
                    Arc::clone(&profile),
                    root_seed ^ 0x_501_1_5_1_2,
                ))),
                profile: Some(profile),
            })
            .collect();
        let split = BandSplit::resolve(
            materials.iter().filter_map(|m| m.profile.as_deref()),
            spacing_m,
        );
        Self {
            fields,
            transition,
            root_seed,
            materials,
            split,
            roles: Roles::default(),
            laboratory: Some(GroundState {
                compaction: compaction.clamp(0.0, 1.0),
                moisture: moisture.clamp(0.0, 1.0),
                ..GroundState::default()
            }),
        }
    }

    /// The document's vegetation-density channel here, or one.
    ///
    /// Separate from [`vegetation_support`](Self::vegetation_support) because
    /// they answer different questions: support is what the *ground* allows, and
    /// this is what the *author* asked for on top of it. A track suppresses
    /// growth for a metre either side of a material band that is half that wide,
    /// and collapsing the two would make that impossible to say.
    pub fn abundance(&self, world: Vec2) -> f32 {
        match self.roles.vegetation_density {
            None => 1.0,
            Some(channel) => self
                .fields
                .modifier(
                    channel,
                    WorldPoint::new(world.x as f64, world.y as f64),
                    1.0,
                )
                .max(0.0),
        }
    }

    /// How dead the bottom of the sward is here, `0..1`.
    ///
    /// **Zero when no channel claims the role**, and that default is the whole
    /// contract: a document that says nothing about litter renders the sward
    /// exactly as the reference art tuned it, so every pinned fixture in this
    /// workspace holds. See [`ModifierRole::DeadLitter`].
    ///
    /// Separate from abundance and from bareness because it is a different
    /// question from either. Litter is not less grass — the mat is as thick as
    /// ever — and it is not exposed earth, because a dead mat covers the ground
    /// as completely as a live one. It is the *age* of what is down there.
    pub fn dead_litter(&self, world: Vec2) -> f32 {
        match self.roles.dead_litter {
            None => 0.0,
            Some(channel) => self
                .fields
                .modifier(
                    channel,
                    WorldPoint::new(world.x as f64, world.y as f64),
                    0.0,
                )
                .clamp(0.0, 1.0),
        }
    }

    /// The profile key each material resolved to, in material-index order.
    ///
    /// What an exporter needs to map a realised weight onto a weight plane
    /// without a string comparison per sample.
    pub fn material_profile_keys(&self) -> Vec<Option<String>> {
        self.materials
            .iter()
            .map(|entry| entry.profile.as_ref().map(|p| p.key.as_str().to_string()))
            .collect()
    }

    pub fn band_split(&self) -> &BandSplit {
        &self.split
    }

    /// Every distinct profile in play, in material-index order of first use.
    /// The profile of one material, if it declared one.
    ///
    /// What a recipe needs to place loose material at the density and size the
    /// *soil* says, rather than at a constant a document repeated. A sand bar
    /// carries 220 fragments a square metre and a meadow floor 40, and those
    /// numbers are already authored in the profiles — see `GroundScatter`.
    pub fn profile_of(&self, material: MaterialIndex) -> Option<&Arc<GroundMaterialProfile>> {
        self.materials
            .get(material.0 as usize)
            .and_then(|entry| entry.profile.as_ref())
    }

    pub fn profiles(&self) -> Vec<&Arc<GroundMaterialProfile>> {
        let mut seen: Vec<&Arc<GroundMaterialProfile>> = Vec::new();
        for entry in &self.materials {
            let Some(profile) = &entry.profile else {
                continue;
            };
            if !seen.iter().any(|known| known.key == profile.key) {
                seen.push(profile);
            }
        }
        seen
    }

    /// The realised substrate at a point.
    ///
    /// One call, shared by everything. See the module note: two callers
    /// realising the same boundary separately is how a track's colour ends up a
    /// centimetre away from where its grass thinned.
    pub fn substrates(&self, world: Vec2) -> RealisedSubstrate {
        // A laboratory is one material everywhere: there is no document, so
        // there are no authored weights to realise. Returned before the field
        // read rather than after, because the field would answer "nothing".
        if self.laboratory.is_some() {
            return RealisedSubstrate::pure(MaterialIndex(0));
        }
        let at = WorldPoint::new(world.x as f64, world.y as f64);
        let mut weights: Vec<(MaterialIndex, f32)> = Vec::new();
        self.fields.substrate_weights_into(at, &mut weights);
        realise(
            weights.iter().copied(),
            at,
            &self.transition,
            self.root_seed,
        )
    }

    /// How much of this point supports plants, `0..1`.
    ///
    /// The cheap path, for the grass overlay: it needs this and nothing else,
    /// several million times.
    pub fn vegetation_support(&self, world: Vec2) -> f32 {
        let realised = self.substrates(world);
        if realised.is_empty() {
            // Ground nothing has claimed. Grass grows, because that is what the
            // laboratory meadow is and every existing measurement assumes it.
            return 1.0;
        }
        self.support_of(&realised)
    }

    /// How much an already-realised substrate supports plants, `0..1`.
    ///
    /// The same answer as [`vegetation_support`](Self::vegetation_support)
    /// without realising the boundary a second time, for a caller that already
    /// has the weights.
    pub fn support_for(&self, realised: &RealisedSubstrate) -> f32 {
        if realised.is_empty() {
            return 1.0;
        }
        self.support_of(realised)
    }

    fn support_of(&self, realised: &RealisedSubstrate) -> f32 {
        realised
            .iter()
            .map(|(material, weight)| weight * self.affinity(material))
            .sum::<f32>()
            .clamp(0.0, 1.0)
    }

    /// One material's vegetation affinity, `0..1`.
    ///
    /// One where the material is unknown, which is the honest answer: an
    /// evaluator with no material table is a laboratory, and a laboratory's
    /// ground grows things.
    pub fn material_affinity(&self, material: MaterialIndex) -> f32 {
        self.affinity(material)
    }

    fn affinity(&self, material: MaterialIndex) -> f32 {
        self.materials
            .get(material.index())
            .map_or(1.0, |entry| entry.affinity)
    }

    /// The state channels and derived fields at a point.
    pub fn state(&self, world: Vec2) -> GroundState {
        // A laboratory holds the state constant so a sweep varies one thing.
        // `None` on every evaluator the compiler builds — see `forced_state`.
        if let Some(forced) = self.laboratory {
            return forced;
        }
        let at = WorldPoint::new(world.x as f64, world.y as f64);
        let read = |channel: Option<ModifierIndex>, fallback: f32| match channel {
            None => fallback,
            Some(channel) => self.fields.modifier(channel, at, fallback).clamp(0.0, 1.0),
        };

        let curvature = self
            .fields
            .derived
            .curvature
            .as_ref()
            .map(|plane| plane.sample(&self.fields.grid, at))
            .unwrap_or(0.0);
        let flow = self
            .fields
            .derived
            .flow_accumulation
            .as_ref()
            .map(|plane| plane.sample(&self.fields.grid, at))
            .unwrap_or(0.0);
        let exposure = self
            .fields
            .derived
            .exposure
            .as_ref()
            .map(|plane| plane.sample(&self.fields.grid, at))
            .unwrap_or(1.0);

        GroundState {
            moisture: self.moisture(at, read(self.roles.moisture, 0.0), flow),
            compaction: read(self.roles.compaction, 0.0),
            disturbance: read(self.roles.disturbance, 0.0),
            loose_material: read(self.roles.loose, 0.0),
            desiccation: read(self.roles.desiccation, 0.0),
            curvature,
            flow,
            exposure,
        }
    }

    /// How wet the ground is, from what the document supplies and where it goes.
    ///
    /// Flow **redistributes** the declared supply rather than creating water.
    /// The rule this replaced was `max(declared, collected)`, which let a hollow
    /// in a desert fill up from nothing: an accumulation field says how much of
    /// the surrounding area drains through a point, not how much rain fell on
    /// it, and treating it as a source is how a document that declared its
    /// ground bone dry rendered a puddle.
    ///
    /// Where a document declares no supply at all, flow still concentrates the
    /// small amount any ground carries — otherwise a laboratory meadow with no
    /// moisture channel would have perfectly uniform ground, which is the one
    /// thing real ground never is.
    ///
    /// ## The concentration is deliberately modest
    ///
    /// It was strong enough to nearly double the supply for one build, and a
    /// track whose document declared `0.45` came out of the exporter at `0.82`
    /// — saturated, everywhere, on ground the author had described as damp. The
    /// visible consequence was not a wetness problem: saturation flattens
    /// relief, so the *clods disappeared*, and the track rendered as paper.
    ///
    /// The rule that avoids this is that flow moves water **within** the range
    /// the author asked for. A point that collects everything upstream reaches
    /// the supply plus a third of the headroom above it; a point that sheds
    /// keeps rather less than the supply. Neither end escapes what the document
    /// said.
    fn moisture(&self, at: WorldPoint, declared: f32, flow: f32) -> f32 {
        // ## An authored saturation is not a supply
        //
        // These are two different roles and the code used to run both through
        // the same redistribution. `WaterSupply` is defined as how much water
        // *arrives* before any is moved, so concentrating and shedding it is
        // exactly right. `SoilMoisture` is defined as how wet the ground *is*,
        // which is the answer rather than the input — and shedding a fifth of
        // it meant a document that declared saturated ground got 0.8 on flat
        // terrain, with no way to reach 1.0 anywhere, because the shed is only
        // returned through flow concentration and this world is flat.
        //
        // Measured before this: `wet_hollow` authored 1.0 across its hollow and
        // not one sample in fourteen thousand exceeded 0.8. Every wet response
        // downstream — the palette lerp, the roughness collapse, the relief
        // slump, the film — ran at four fifths of the strength the document
        // asked for, and the ceiling was invisible from any of them.
        //
        // So redistribution now applies to a supply and not to a state.
        if self.roles.water_supply.is_none() {
            return declared.clamp(0.0, 1.0);
        }
        let supply = declared.max(
            self.fields
                .modifier(self.roles.water_supply.expect("checked"), at, 0.0)
                .clamp(0.0, 1.0),
        );
        // Saturating, so doubling the catchment does not double the wetness.
        let concentration = (flow / (flow + 0.6)).clamp(0.0, 1.0);
        // Centred on the supply: hollows gain a third of the headroom above it,
        // crowns give up a fifth of what is there.
        let headroom = (1.0 - supply) * 0.33;
        let shed = supply * 0.20;
        (supply - shed + (headroom + shed) * concentration).clamp(0.0, 1.0)
    }

    /// Everything, at one point.
    pub fn sample(&self, world: Vec2) -> GroundSample {
        let substrates = self.substrates(world);
        let state = self.state(world);
        let vegetation_support = if substrates.is_empty() {
            1.0
        } else {
            self.support_of(&substrates)
        };
        let relief = self.relief_of(&substrates, &state, world);
        GroundSample {
            displacement_m: relief.displacement_m,
            cavity: relief.cavity,
            wet_film: self.wet_film_of(&substrates, &state),
            substrates,
            state,
            vegetation_support,
        }
    }

    /// The height a thing standing on this ground actually rests at, metres.
    ///
    /// ```text
    /// final_surface_z = authored elevation
    ///                 + authored microrelief      (both inside surface_height)
    ///                 + profile geometry displacement
    /// ```
    ///
    /// The one number a stone, a stem or a leaf should be registered to, and the
    /// reason it exists as a named method rather than being spelled out at each
    /// call site: the ground mesh Cycles renders is displaced by the third term,
    /// and anything placed against only the first two floats or sinks by however
    /// much relief the profile happened to have there. Centimetres, which is
    /// exactly the scale at which a pebble stops touching the ground.
    ///
    /// Deliberately geometry only. Shader bump is not in here and must not be: a
    /// stone rests on a surface, not on a normal perturbation, and adding the
    /// bump would put objects at a height no mesh in the scene has.
    pub fn final_surface_z_m(&self, world: Vec2) -> f32 {
        let at = WorldPoint::new(world.x as f64, world.y as f64);
        self.fields.surface_height(at) + self.displacement(world)
    }

    /// The height the rendered *mesh* has here, rather than the analytic one.
    ///
    /// ## Why these are different, and why the difference floats a flower
    ///
    /// The ground mesh samples [`final_surface_z_m`](Self::final_surface_z_m) at
    /// lattice vertices and draws flat triangles between them. Between two
    /// vertices the mesh is the *chord*; the analytic surface is the curve. Over
    /// a crest the curve is above the chord, by up to about half the band
    /// amplitude — eight millimetres on the shipped loam, which is more than a
    /// flower stem is thick.
    ///
    /// A stem rooted at the analytic height therefore stands a visible gap above
    /// the mesh whenever it lands between vertices near a crest, and the gap is
    /// worst exactly where the ground is most interesting. Nothing reports it:
    /// from the placement side the root is on the ground, and from the mesh side
    /// there is simply a stem nearby.
    ///
    /// So anything that *rests on* the ground registers to this instead. It
    /// reproduces the mesh's own bilinear interpolation over the same
    /// lattice — snap, sample four corners, blend — so a root registered to it
    /// cannot float or sink however coarse the mesh gets.
    ///
    /// The mesh's own lattice spacing is derived from the profiles in play by
    /// [`BandSplit::spacing_for`], and the exporter derives it the same way, so
    /// the two agree without either having to be told.
    pub fn mesh_surface_z_m(&self, world: Vec2, spacing_m: f32) -> f32 {
        // `!(x > 0)` rather than `x <= 0`: they are different predicates and a
        // NaN spacing must take this branch. See `terrain_scene::field`.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(spacing_m > 0.0) {
            return self.final_surface_z_m(world);
        }
        // The same global anchoring the exporter uses: a lattice snapped to
        // multiples of the spacing rather than to the window's own corner, so
        // two windows over the same ground sample the same vertices.
        let snap = |value: f32| (value / spacing_m).floor() * spacing_m;
        let (u0, v0) = (snap(world.x), snap(world.y));
        let (fu, fv) = (
            ((world.x - u0) / spacing_m).clamp(0.0, 1.0),
            ((world.y - v0) / spacing_m).clamp(0.0, 1.0),
        );
        let corner = |du: f32, dv: f32| {
            self.final_surface_z_m(Vec2::new(u0 + du * spacing_m, v0 + dv * spacing_m))
        };
        let low = corner(0.0, 0.0) + (corner(1.0, 0.0) - corner(0.0, 0.0)) * fu;
        let high = corner(0.0, 1.0) + (corner(1.0, 1.0) - corner(0.0, 1.0)) * fu;
        low + (high - low) * fv
    }

    /// The lattice the ground mesh will be built on, from the soils in play.
    pub fn mesh_spacing_m(&self) -> f32 {
        self.split.spacing_m
    }

    /// The field stack this evaluator reads.
    ///
    /// Exposed so a caller holding the evaluator does not need to carry the
    /// stack beside it and risk the two being different objects.
    pub fn fields(&self) -> &Arc<TerrainFieldStack> {
        &self.fields
    }

    /// The sub-mesh relief at a point, in metres.
    ///
    /// The bands a [`GroundReliefPlan`] assigned to the bump tier, evaluated
    /// through the *same* basis, phase, aggregate transform, clustering and
    /// state response the mesh uses. That sameness is the point: before this
    /// existed, the Blender material rebuilt these bands from its own noise with
    /// a non-monotone ridge function, so the mesh and the shader were two
    /// different surfaces that happened to be adjacent.
    ///
    /// Exported as a float plane rather than evaluated per shading point,
    /// because a path tracer asking Rust a question per sample is not a
    /// pipeline.
    pub fn bump_height_m(&self, world: Vec2, plan: &crate::relief::GroundReliefPlan) -> f32 {
        self.tiered_height(world, plan, crate::relief::ReliefTier::Bump)
    }

    /// The RMS slope the microfacet tier leaves unresolved.
    ///
    /// A band below a pixel cannot be seen as shape, but it is still there: it
    /// scatters light, and a surface that lost it reads as polished. What
    /// survives is the *slope variance*, which is what a microfacet roughness
    /// is a model of.
    ///
    /// Amplitude shrinks linearly with state and slope variance shrinks
    /// quadratically, so the per-band contribution is `(q·A/λ)²` summed and
    /// rooted. The constant relating a band's amplitude and wavelength to its
    /// RMS slope is `2π/√2` for a sinusoid; a noise band of the same scale is
    /// shallower, and the factor here is the conservative sinusoidal one until
    /// the calibration laboratory measures the real number.
    pub fn micro_slope_rms(&self, world: Vec2, plan: &crate::relief::GroundReliefPlan) -> f32 {
        use crate::relief::ReliefTier;
        let substrates = self.substrates(world);
        if substrates.is_empty() {
            return 0.0;
        }
        let state = self.state(world);
        let mut variance = 0.0f32;
        for (material, weight) in substrates.iter() {
            if weight <= 0.0 {
                continue;
            }
            let Some(entry) = self.materials.get(material.index()) else {
                continue;
            };
            let Some(profile) = &entry.profile else {
                continue;
            };
            let cluster = self.cluster(profile, world);
            for planned in plan.bands_in(&entry.key, ReliefTier::Microfacet) {
                let Some(band) = profile.structure.bands.get(planned.band_index as usize) else {
                    continue;
                };
                let scale = profile.band_scale(band, state.compaction, state.moisture);
                let clustered = if band.clustered { cluster } else { 1.0 };
                let slope = std::f32::consts::TAU * band.amplitude_m
                    / (band.wavelength_m * std::f32::consts::SQRT_2);
                // Linear in amplitude, so quadratic in variance.
                let q = scale * clustered;
                variance += weight * (q * slope) * (q * slope);
            }
        }
        variance.max(0.0).sqrt()
    }

    /// Relief from one tier of a plan, blended by realised weight.
    fn tiered_height(
        &self,
        world: Vec2,
        plan: &crate::relief::GroundReliefPlan,
        tier: crate::relief::ReliefTier,
    ) -> f32 {
        let substrates = self.substrates(world);
        if substrates.is_empty() {
            return 0.0;
        }
        let state = self.state(world);
        let mut total = 0.0;
        for (material, weight) in substrates.iter() {
            if weight <= 0.0 {
                continue;
            }
            let Some(entry) = self.materials.get(material.index()) else {
                continue;
            };
            let Some(profile) = &entry.profile else {
                continue;
            };
            let cluster = self.cluster(profile, world);
            // What kind of ground this patch is, as opposed to how deep — see
            // `shape`. One value per point and per soil, so every band of a
            // fracture surface breaks the same way here and they read as one
            // surface rather than three overlaid textures.
            let shift = centre_shift(profile, self.root_seed, world);
            let mut height = 0.0;
            for planned in plan.bands_in(&entry.key, tier) {
                let index = planned.band_index as usize;
                let Some(band) = profile.structure.bands.get(index) else {
                    continue;
                };
                let scale = profile.band_scale(band, state.compaction, state.moisture);
                let clustered = if band.clustered { cluster } else { 1.0 };
                height += self.band_height(band, index, world, shift) * scale * clustered;
            }
            total += height * weight;
        }
        total
    }

    /// The mesh relief at a point, in metres.
    ///
    /// Blended by realised weight, so a boundary between cloddy loam and smooth
    /// clay is a smooth ramp in relief rather than a step — and so neither
    /// material is a special case.
    pub fn displacement(&self, world: Vec2) -> f32 {
        let substrates = self.substrates(world);
        if substrates.is_empty() {
            return 0.0;
        }
        self.relief_of(&substrates, &self.state(world), world)
            .displacement_m
    }

    fn relief_of(
        &self,
        substrates: &RealisedSubstrate,
        state: &GroundState,
        world: Vec2,
    ) -> Relief {
        let mut total = 0.0;
        // Relief measured against how much this material *could* have, so a
        // smooth clay and a cloddy loam both report their own hollows rather
        // than the loam reporting all of them.
        let mut cavity = 0.0;
        for (material, weight) in substrates.iter() {
            if weight <= 0.0 {
                continue;
            }
            let Some(entry) = self.materials.get(material.index()) else {
                continue;
            };
            let Some(profile) = &entry.profile else {
                continue;
            };
            // ## The mesh-scale relief is simulated, not sampled
            //
            // Everything the mesh can carry comes from `soil`: aggregate packets
            // laid down, slumped toward a moisture-dependent angle of repose,
            // pressed into, and washed over. See that module for why no shaping
            // of a noise field can produce this — the short version is that noise
            // is statistically uniform and real ground has a history.
            //
            // The bands are still what the *shader* draws below a cell, and they
            // are still what the profile declares, so a soil's authored structure
            // is unchanged: this replaces how the coarse end of that structure is
            // realised, not what an author writes.
            let mut height = match &entry.surface {
                Some(surface) => surface.height(world, state.moisture, state.compaction),
                None => 0.0,
            };
            if let Some(ripples) = &profile.ripples {
                height += ripple_height(ripples, world, state, self.root_seed);
            }
            if let Some(cracks) = &profile.cracks {
                height -= crack_depth(cracks, profile, world, state, self.root_seed);
            }
            // How deep in its own relief this point sits, against how much this
            // soil has to give. The simulation is zero-mean about its own datum,
            // so the reach is the amplitude the soil declared for the scales the
            // mesh carries — which is what the simulation was sized from.
            let reach: f32 = self
                .split
                .bands_for(&entry.key)
                .iter()
                .map(|band| band.amplitude_m)
                .sum();
            if reach > 0.0 {
                cavity += (0.5 - height / reach).clamp(0.0, 1.0) * weight;
            }
            total += height * weight;
        }
        Relief {
            displacement_m: total,
            cavity: cavity.clamp(0.0, 1.0),
        }
    }

    /// One band's contribution, in metres.
    ///
    /// ## One octave, and that is the whole point of a band list
    ///
    /// This summed two octaves for one render, and the result was speckle
    /// wherever the ground was loose. The reason is worth writing down because
    /// it is easy to walk back into: a two-octave sum at a declared wavelength
    /// of five centimetres contains content at **two and a half**, so the
    /// lattice that was sized to carry the band aliases half of what the band
    /// actually holds.
    ///
    /// A band is a scale. The multi-scale structure comes from the *list* of
    /// bands, not from octaves inside one of them — and that is what makes the
    /// declared wavelength true, which is what lets the exporter decide where
    /// each band should be drawn.
    ///
    /// ## Two turned copies of one scale, not two octaves of two
    ///
    /// Value noise is built on an axis-aligned lattice and one octave of it
    /// *shows* that lattice — a plate of it reads as quilting, in squares, lined
    /// up with the world axes. The usual cure is more octaves, which is exactly
    /// what cannot be used here.
    ///
    /// So the band is the mean of two samples of the **same frequency** on
    /// frames turned against each other. The declared wavelength stays true, so
    /// the exporter's split still means something, and neither lattice survives
    /// the other. Averaging two fields narrows the distribution, hence the lift
    /// back afterwards.
    ///
    /// Both turns are non-zero. Leaving the first at zero degrees is what put a
    /// visible grid of squares across a whole plate for one render: every other
    /// band was turned and the coarsest one — the one carrying the shape — was
    /// still square to the world.
    fn band_height(&self, band: &ReliefBand, index: usize, world: Vec2, shift: f32) -> f32 {
        let frequency = 1.0 / band.wavelength_m;
        let seed = self.root_seed ^ (0x9E37_79B9_u64.wrapping_mul(index as u64 + 1));
        // Golden-angle turns, so no two bands in any profile share a direction
        // and no band is square to the world.
        let mut sum = 0.0;
        for turn in 0..2 {
            let angle = 2.399_963_2 * (index as f32 * 2.0 + turn as f32 + 1.0);
            let (sin, cos) = angle.sin_cos();
            let offset = 37.0 * (index * 2 + turn) as f32;
            let x = (world.x * cos - world.y * sin) * frequency + offset;
            let y = (world.x * sin + world.y * cos) * frequency - offset;
            sum += value_noise(seed, Stream::GroundRelief, x, y);
        }
        // Two averaged uniform fields have about seven tenths of one field's
        // spread, so the amplitude in the profile would otherwise mean less than
        // it says.
        let raw = ((sum * 0.5 - 0.5) * 1.41 + 0.5).clamp(0.0, 1.0);
        // The lattice decides how sharp the wall is allowed to be. A fragment
        // drawn with an edge the mesh cannot resolve is a staircase that moves
        // when the window does — see `shape`.
        let floor = flank_floor(band.wavelength_m, self.split.spacing_m);
        shape(raw, band.shape, floor, shift) * band.amplitude_m
    }

    /// How cloddy this patch is, `0..1`.
    fn cluster(&self, profile: &GroundMaterialProfile, world: Vec2) -> f32 {
        let strength = profile.structure.cluster_strength;
        if strength <= 0.0 {
            return 1.0;
        }
        let frequency = 1.0 / profile.structure.cluster_wavelength_m;
        let raw = fbm(
            self.root_seed ^ 0x00C1_0575,
            Stream::GroundCluster,
            world.x * frequency,
            world.y * frequency,
            2,
        );
        1.0 - strength * (1.0 - smoothstep(0.30, 0.70, raw))
    }

    fn wet_film_of(&self, substrates: &RealisedSubstrate, state: &GroundState) -> f32 {
        if substrates.is_empty() {
            return 0.0;
        }
        // A film forms where the ground cannot take any more water, so a
        // saturated surface reads as wet before a puddle is anywhere near deep
        // enough to be geometry. Concave ground holds it; a crown sheds it.
        //
        // ## Both ends were short of one
        //
        // `held` divided by 0.45 so that full saturation gave one — but the
        // supply never reached one, because `moisture` shed a fifth of every
        // authored value. And the curvature term topped out at 0.35 + 0.65 on a
        // perfect hollow, so *flat* saturated ground reached 0.62 of a film at
        // best. Multiplied together, the wettest flat ground the pipeline could
        // produce carried a coat weight of about 0.48, which at a water film's
        // 2% normal reflectance is a one-percent highlight: present in the
        // buffer, invisible in the picture.
        //
        // The shed is gone (see `moisture`), the threshold is lower, and flat
        // ground now reaches nine tenths of a film at saturation. A hollow
        // still gets more, because it genuinely holds more.
        let held = (state.moisture - 0.42).max(0.0) / 0.58;
        let hollow = smoothstep(-0.35, 0.15, -state.curvature);
        (held * (0.72 + 0.28 * hollow)).clamp(0.0, 1.0)
    }
}

/// What one point's relief is, and how deep in it that point sits.
struct Relief {
    displacement_m: f32,
    cavity: f32,
}

/// Centre a `0..1` noise value on zero, with the band's shape applied.
///
/// ## Why this is a skew and not a fold
///
/// The obvious way to make a noise field look ridged is to fold it about its
/// mean — `1 - 2|c|`, then square. It is what ridged fractal noise does, it is
/// one line, and on terrain at kilometre scale it gives convincing crests.
///
/// On soil it produces **worms**. A fold is not monotonic: a noise value well
/// above the mean and one well below it map to the same height, so the crests
/// end up tracing the field's *mid-level contour* — and the mid-level set of a
/// smooth field is a family of closed curves. The result is a maze of even-width
/// squiggles, and no ground anywhere looks like that.
///
/// Everything below is monotonic by construction, which is what a contour cannot
/// survive. A test asserts it directly.
///
/// ## Why a power curve was not enough either
///
/// The skew that replaced the fold — `u^gamma`, re-centred — is monotone and it
/// does narrow the crests. What it cannot do is make a **flank**, and a flank is
/// the entire visual difference between soil and dunes.
///
/// The reason is a bound rather than a tuning failure. A power curve multiplies
/// the field's own gradient by at most `gamma`, and the field's gradient is set
/// by the band: a wavelength `L` carrying a peak-to-trough of `D` has a maximum
/// slope near `pi*D/L`. Compacted loam's clod band declared 17 mm over 50 mm and
/// arrived, after compaction and saturation and clustering, at **5.3 mm over
/// 50 mm — a slope of 19 degrees**. Measured off an exported `ground.bin`, bare
/// river sand came back at 2.0 mm peak-to-peak, mean slope 5.6, maximum 16.3.
///
/// The sun is at 35 degrees. A face has to tilt past **55** before it falls into
/// its own shadow, and not one vertex of bare ground in the scene did. So the
/// ground could not cast a single shadow on itself at any moisture, any
/// compaction, anywhere — and every scrap of apparent structure in a render of it
/// was pigment. That is why the soil read as a painted card: it *was* one.
///
/// ## A fracture surface, not a plane with lumps on it
///
/// So the transform is flat at *both* ends and steep in the middle:
///
/// ```text
///   below the wall   the floor of a void between fragments
///   the wall         the steep side of a fragment
///   above it         the top of a fragment
/// ```
///
/// The wall is where the steepness comes from, and it is steep because it is
/// *narrow*: compressing the transition into a fraction `wall` of the field's
/// range multiplies the gradient there by `1.5 / wall`, independent of the
/// band's amplitude. That is the point — a six-centimetre clod standing two
/// centimetres proud has near-vertical sides, which no amplitude-to-wavelength
/// ratio can express and a narrow wall expresses exactly.
///
/// **Flat at both ends** is what the version before this got wrong. It put two
/// thirds of the field on a single floor and raised rounded domes out of it,
/// which is spheres on a plane, and looked it — the render was called a
/// nineties video game and that was the right word for it. Two levels with a
/// wall between them is a *break*; three bands of that summed is the
/// many-levelled irregular surface broken ground actually is, because the levels
/// multiply rather than repeat.
///
/// It also fixes a measured shortfall the dome caused. With most of the ground
/// on one floor the only surfaces facing the sun were dome tops, so a render had
/// a handful of very bright specular glints and a great deal of dark: its
/// ninetieth percentile came in at 0.098 against the reference photograph's
/// 0.167. Plates present broad lit faces, and broad lit faces are where a
/// photograph of soil keeps its light.
///
/// A smoothstep for the wall, because a fragment's edge is chipped and its foot
/// is buried in loose material — neither end is a knife edge. Its integral is a
/// half, so the mean is `1 - centre` exactly whatever the wall width, which
/// keeps the band zero-mean and stops an author retuning the shape from moving
/// the ground under it.
///
/// ## The wall cannot be narrower than the lattice
///
/// A transition compressed into a tenth of the field's range occupies about a
/// tenth of half a wavelength on the ground. Drawn on a lattice that cannot
/// resolve it, that is not a crisp edge — it is a staircase, and it moves when
/// the window moves. So the caller passes the finest wall the lattice can carry
/// and the shape widens to it. A close view gets fragments with edges; a wide
/// view gets soft undulation of the same field, in the same places, at the same
/// height. The same argument this file makes everywhere else about drawing a
/// thing only where it can be seen.
///
/// ## And the ground must not be the same everywhere
///
/// `centre_shift` is the other half of what made this read as a video game, and
/// it is about no single clod. Procedural noise is **statistically uniform**:
/// every patch has the same distribution as every other patch, so a metre of it
/// reads as one texture tiled, however well that texture is tuned. Real ground
/// has history — a stretch broken up by a hoof, a swept smooth patch, a corner
/// gone to fine crumb — and no two hand's-widths of it match.
///
/// So the wall's position slides with a slow field. Where it sits low the
/// fragments merge into broad raised sheets; where it sits high the surface is
/// mostly void with islands standing in it. Same band, same wavelength, same
/// seed — different ground.
fn shape(raw: f32, shape: AggregateShape, wall_floor: f32, centre_shift: f32) -> f32 {
    let u = raw.clamp(0.0, 1.0);
    let (centre, wall) = shape.profile();
    let centre = (centre + centre_shift).clamp(0.12, 0.88);
    // Never wider than the room either side, or the wall runs off the end of the
    // range and the band flattens to a ramp.
    let room = 2.0 * centre.min(1.0 - centre);
    let wall = wall.max(wall_floor).clamp(1.0e-4, room);
    let s = ((u - (centre - 0.5 * wall)) / wall).clamp(0.0, 1.0);
    let plate = s * s * (3.0 - 2.0 * s);
    plate - (1.0 - centre)
}

/// The finest wall a lattice of `spacing_m` can carry, for a band of
/// `wavelength_m`, as a fraction of the noise's range.
///
/// A value-noise field of wavelength `L` traverses its range over roughly `L/2`,
/// so a wall occupying a fraction `f` of the range is about `f * L/2` wide on
/// the ground. Asking for at least two and a half cells across it — below that a
/// transition is a step, and a step aliases — gives `f >= 5 * spacing / L`.
pub fn flank_floor(wavelength_m: f32, spacing_m: f32) -> f32 {
    if wavelength_m <= 0.0 {
        return 1.0;
    }
    (5.0 * spacing_m / wavelength_m).clamp(0.0, 1.0)
}

/// How far this patch's wall has slid from the shape's declared position.
///
/// A slow field, and deliberately slower than the cluster mask it sits beside:
/// clustering says how *deep* the relief is here, this says what *kind* it is,
/// and the two varying together would collapse into one signal that only makes
/// the ground alternately bumpy and smooth. See `shape`'s last section for what
/// this is for.
fn centre_shift(profile: &GroundMaterialProfile, root_seed: u64, world: Vec2) -> f32 {
    let frequency = 1.0 / (profile.structure.cluster_wavelength_m.max(1.0e-3) * 2.6);
    let raw = fbm(
        root_seed ^ 0x5E17_0C7E,
        Stream::GroundCluster,
        world.x * frequency + 311.0,
        world.y * frequency - 197.0,
        2,
    );
    // A fifth of the range either way. Enough that a patch of merged sheets and
    // a patch of scattered islands are plainly different ground; not so much
    // that either end stops being the material the profile declared.
    (raw - 0.5) * 0.40
}

/// Wind ripples, in metres.
///
/// Three things stop this being parallel sine waves, which is the single most
/// recognisable procedural failure there is:
///
/// - **Meander.** The phase is displaced by a low-frequency field, so
///   wavefronts wander the way real ones do.
/// - **Asymmetry.** Real ripples have a long windward face and a short lee one.
///   Skewing the phase before the sine produces that without a second texture.
/// - **Patchiness.** A bar is not rippled evenly. Some of it was worked over
///   recently and some was washed flat, so the amplitude itself varies over a
///   scale of a metre or two.
///
/// ## The meander's *scale* is not its size
///
/// The first version derived the meander field's frequency from `meander_m` —
/// the distance it displaces by — which conflates how far a wavefront wanders
/// with how quickly it changes its mind. At the shipped sand's settings that
/// put the meander field's own wavelength at ten centimetres against a
/// seven-and-a-half-centimetre ripple, so the displacement was *faster* than
/// the thing it was displacing: every wavefront wobbled inside a single cycle
/// and none of them curved. What rendered was corduroy.
///
/// The scale a wavefront wanders on is set by the ripple itself: real ones hold
/// their line for a handful of wavelengths and then bend. So the field's
/// frequency comes from `wavelength_m`, and `meander_m` says only how far.
///
/// Saturation flattens all of it, because wet sand does not ripple — the water
/// holds the grains where they are.
fn ripple_height(
    ripples: &terrain_core::ground_material::RippleProfile,
    world: Vec2,
    state: &GroundState,
    root_seed: u64,
) -> f32 {
    let amplitude = ripples.amplitude_m * (1.0 - ripples.wetness_suppression * state.moisture);
    if amplitude <= 0.0 {
        return 0.0;
    }
    let (sin, cos) = ripples.direction_rad.sin_cos();
    // Distance along the wind, which is the axis the pattern varies on.
    let along = world.x * cos + world.y * sin;
    let across = -world.x * sin + world.y * cos;

    // Six wavelengths, which is about how far a real wavefront holds its line
    // before it bends. Tied to the ripple rather than to the displacement — see
    // the note above for what happens when it is not.
    let meander = if ripples.meander_m > 0.0 {
        let frequency = 1.0 / (ripples.wavelength_m.max(f32::MIN_POSITIVE) * 6.0);
        (fbm(
            root_seed ^ 0x_21_99_1E_00,
            Stream::Ripple,
            along * frequency,
            across * frequency,
            3,
        ) - 0.5)
            * ripples.meander_m
            * 2.0
    } else {
        0.0
    };

    // ## Short crests, which is the whole difference between a ripple and a weave
    //
    // A ripple field drawn as a continuous wave is a woven fabric, and no
    // amount of meander or amplitude variation rescues it — three renders were
    // spent proving that, and the profile that declared them was left with
    // `ripples: None` for a while as a result.
    //
    // What a real ripple field has that a sine does not is **termination**.
    // Crests run for a few times their own wavelength, die out, and a new one
    // starts offset from it; between them the surface is smooth. That is a
    // property of the crest's *length along itself*, so the field that carries
    // it has to be anisotropic — long across the wave and short along it —
    // which is what the two different frequencies below are.
    //
    // Coverage lands near half: about that much of a bar is actively rippled
    // and the rest is smooth between. `smoothstep` rather than a threshold, so
    // a crest fades out along its length instead of stopping at a line.
    let segment = fbm(
        root_seed ^ 0x_21_99_1E_5A,
        Stream::Ripple,
        // Along the wave: slow, so a run of crests shares a fate.
        along * 0.9,
        // Across it: fast, so neighbouring crests do not.
        across * 2.6,
        2,
    );
    let crested_here = smoothstep(0.38, 0.62, segment);

    // And a broad term on top, so the bar has reworked stretches and smoothed
    // ones at a scale above the individual crest.
    let patchiness = crested_here
        * (0.45
            + 0.55
                * fbm(
                    root_seed ^ 0x_21_99_1E_A7,
                    Stream::Ripple,
                    world.x * 0.5,
                    world.y * 0.5,
                    2,
                ));

    let phase =
        (along + meander) * std::f32::consts::TAU / ripples.wavelength_m.max(f32::MIN_POSITIVE);
    // Skewing the phase by its own sine gives one face a longer run-up than the
    // other, which is the whole of the windward/lee asymmetry.
    let skewed = phase + ripples.asymmetry * phase.sin();
    let wave = skewed.sin();
    // Sharpen the crests without touching the troughs.
    let sharp = ripples.crest_sharpness;
    let crested = wave * (1.0 - sharp) + wave.abs().powf(0.55) * wave.signum() * sharp;
    crested * amplitude * patchiness * 0.5
}

/// How deep the desiccation network cuts here, in metres.
///
/// A capability times an occasion: the profile says whether this material can
/// crack, and the state says whether it has. Wet ground, disturbed ground and
/// ground with nothing holding it together do not crack, and each of those is a
/// veto rather than a term — a product, so any one of them closes the network
/// completely.
fn crack_depth(
    cracks: &terrain_core::ground_material::CrackProfile,
    profile: &GroundMaterialProfile,
    world: Vec2,
    state: &GroundState,
    root_seed: u64,
) -> f32 {
    if state.moisture >= cracks.moisture_ceiling {
        return 0.0;
    }
    let dryness = 1.0 - state.moisture / cracks.moisture_ceiling.max(f32::MIN_POSITIVE);
    let opportunity = dryness
        * state.desiccation.max(dryness * 0.5)
        * (1.0 - state.disturbance)
        * profile.structure.cohesion;
    if opportunity <= 0.0 {
        return 0.0;
    }

    let primary = crack_network(root_seed, world, cracks.polygon_m, 0);
    let mut mask = crack_mask(primary, cracks.width_m / cracks.polygon_m);
    if cracks.secondary > 0.0 {
        let fine = crack_network(root_seed, world, cracks.polygon_m * 0.38, 1);
        // Secondary branches stop *against* the primaries rather than crossing
        // them, so the finer network is suppressed wherever a primary already
        // opened. A network without that reads as two Voronoi diagrams laid on
        // top of each other, which is exactly what it would be.
        let fine_mask = crack_mask(fine, cracks.width_m * 0.55 / (cracks.polygon_m * 0.38));
        mask = mask.max(fine_mask * cracks.secondary * (1.0 - mask));
    }
    mask * cracks.depth_m * opportunity
}

/// Distance to the nearest cell boundary of a jittered lattice, `0..1`.
///
/// A Worley F2−F1, which is the standard way to get a polygon network. Written
/// here rather than reached for from the noise sources because it needs to be
/// addressed by world cell — a network whose cells moved when the sampling
/// window moved would break the seam guarantee everything else holds to.
fn crack_network(root_seed: u64, world: Vec2, polygon_m: f32, tier: u64) -> f32 {
    let scale = 1.0 / polygon_m.max(f32::MIN_POSITIVE);
    let p = Vec2::new(world.x * scale, world.y * scale);
    let base = Vec2::new(p.x.floor(), p.y.floor());
    let seed = root_seed ^ (0x0C_2ACC_u64 << (tier * 8));

    let (mut first, mut second) = (f32::INFINITY, f32::INFINITY);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cell = base + Vec2::new(dx as f32, dy as f32);
            let jx = value_noise(seed, Stream::Crack, cell.x + 0.5, cell.y + 0.5);
            let jy = value_noise(seed, Stream::Crack, cell.x + 13.7, cell.y - 7.1);
            let site = cell + Vec2::new(jx, jy);
            let distance = (site - p).length();
            if distance < first {
                second = first;
                first = distance;
            } else if distance < second {
                second = distance;
            }
        }
    }
    // The gap between the two nearest sites is zero exactly on a cell wall and
    // grows toward the middle of a polygon, which is the network itself.
    (second - first).clamp(0.0, 1.0)
}

/// Turn a distance-to-wall field into an open crack.
///
/// Two thresholds rather than one: a narrow dark centre inside a broader shallow
/// depression. A single threshold gives a trench with vertical walls, which
/// catches no light and reads as a painted line.
fn crack_mask(distance: f32, width: f32) -> f32 {
    let centre = 1.0 - smoothstep(0.0, width.max(1e-4), distance);
    let shoulder = 1.0 - smoothstep(0.0, (width * 3.2).max(1e-4), distance);
    (centre * 0.7 + shoulder * 0.3).clamp(0.0, 1.0)
}

fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    if high <= low {
        return if x < low { 0.0 } else { 1.0 };
    }
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::ground_material::*;

    fn band(wavelength: f32, amplitude: f32) -> ReliefBand {
        ReliefBand {
            wavelength_m: wavelength,
            amplitude_m: amplitude,
            shape: AggregateShape::Rounded,
            compaction_response: 0.5,
            clustered: false,
        }
    }

    fn profile(bands: Vec<ReliefBand>) -> GroundMaterialProfile {
        GroundMaterialProfile {
            key: terrain_core::ids::GroundProfileKey::new("test_soil").unwrap(),
            shader: terrain_core::ids::AppearanceKey::new("surface.ground").unwrap(),
            display_name: "Test soil".into(),
            optics: GroundOptics {
                dry_palette: Palette {
                    low: [0.05, 0.03, 0.02],
                    mid: [0.08, 0.05, 0.03],
                    high: [0.15, 0.10, 0.06],
                },
                wet: WetResponse {
                    wet_mid: [0.03, 0.015, 0.007],
                    roughness_wet: 0.2,
                    saturation_flattening: 0.45,
                    film_ior: 1.333,
                },
                roughness_dry: Span::new(0.8, 0.96),
                ior: 1.5,
                region_wavelength_m: 2.0,
                region_strength: 0.5,
                patch_wavelength_m: 0.25,
                patch_strength: 0.9,
                grain_strength: 0.55,
            },
            structure: GroundStructure {
                bands,
                cluster_wavelength_m: 0.8,
                cluster_strength: 0.5,
                cohesion: 0.6,
                mesh_floor_m: None,
            },
            ripples: None,
            cracks: None,
            scatter: GroundScatter {
                grit_per_m2: 90.0,
                pebble_per_m2: 0.5,
                fragment_radius_m: Span::new(0.004, 0.02),
            },
            vegetation_affinity: 0.0,
        }
    }

    #[test]
    fn the_lattice_decides_which_bands_are_geometry() {
        let soil = profile(vec![band(0.05, 0.055), band(0.008, 0.009)]);
        // Fine enough for the clods and not for the crumb.
        let split = BandSplit::resolve([&soil], 0.0125);
        assert_eq!(split.geometry[0].1.len(), 1);
        assert_eq!(split.shader[0].1.len(), 1);
        assert_eq!(split.geometry[0].1[0].wavelength_m, 0.05);
    }

    #[test]
    fn a_coarse_lattice_hands_every_band_to_the_shader() {
        // Not a failure — a correct answer. A band the mesh cannot carry has to
        // go somewhere, and aliasing it into the mesh is the one option that
        // produces a different and worse-looking band.
        let soil = profile(vec![band(0.05, 0.055), band(0.008, 0.009)]);
        let split = BandSplit::resolve([&soil], 0.1);
        assert!(split.geometry[0].1.is_empty());
        assert_eq!(split.shader[0].1.len(), 2);
    }

    #[test]
    fn the_spacing_follows_the_finest_material_in_play() {
        let cloddy = profile(vec![band(0.09, 0.085)]);
        let smooth = profile(vec![band(0.03, 0.006)]);
        assert_eq!(BandSplit::spacing_for([&cloddy]), Some(0.09 / 4.0));
        // Two soils together need the finer of the two lattices, or the smooth
        // one aliases while the cloddy one looks fine.
        assert_eq!(BandSplit::spacing_for([&cloddy, &smooth]), Some(0.03 / 4.0));
    }

    #[test]
    fn a_flat_band_does_not_drag_the_lattice_finer() {
        // A profile can declare a band at zero amplitude to say "this scale is
        // deliberately absent". Sizing the lattice for it would cost vertices
        // to resolve a shape that is not there.
        let flat = profile(vec![band(0.002, 0.0)]);
        assert_eq!(BandSplit::spacing_for([&flat]), None);
    }

    #[test]
    fn every_shape_is_monotonic_in_its_input() {
        // The whole of the worm bug in one property. A fold — `1 - 2|c|`, which
        // is what ridged fractal noise does — maps a value well above the mean
        // and one well below it to the same height, so the crests trace the
        // field's mid-level contour, and the mid-level set of a smooth field is
        // a family of closed curves. The render came back as a maze of squiggles.
        //
        // Monotonic means a higher noise value is always a higher point, which
        // is exactly what a contour cannot survive.
        for shape_kind in [
            AggregateShape::Rounded,
            AggregateShape::RoundedRidged,
            AggregateShape::Angular,
        ] {
            let mut last = f32::NEG_INFINITY;
            for i in 0..=2000 {
                let value = shape(i as f32 / 2000.0, shape_kind, 0.0, 0.0);
                assert!(
                    value >= last,
                    "{shape_kind:?} fell from {last} to {value} at {i}"
                );
                last = value;
            }
        }
    }

    #[test]
    fn a_fractured_band_puts_its_surface_on_two_levels() {
        // The property that separates broken ground from bumps on a plane, and
        // the reason a render of the previous shape was called a nineties video
        // game. That one put two thirds of the field on a single floor and raised
        // rounded domes out of it, so the only thing facing the sun was a dome
        // top: a handful of specular glints and a lot of dark.
        //
        // A fracture surface is flat at *both* ends. Most of it sits on a
        // fragment top or on the floor of a void, and a narrow wall runs between,
        // which is what presents broad lit faces to a low sun.
        let count = 4000;
        let near_a_level = |kind: AggregateShape| {
            let (centre, _) = kind.profile();
            (0..count)
                .filter(|i| {
                    let v = shape(*i as f32 / count as f32, kind, 0.0, 0.0);
                    // Within a tenth of the range of either level.
                    v < -(1.0 - centre) + 0.10 || v > centre - 0.10
                })
                .count()
        };
        for kind in [AggregateShape::RoundedRidged, AggregateShape::Angular] {
            let flat = near_a_level(kind);
            assert!(
                flat > count * 6 / 10,
                "{kind:?} put only {flat} of {count} on a level"
            );
        }
        // And a weathered surface is the one that does not, which is what
        // weathering is: the wall worn out until the two levels meet.
        let worn = near_a_level(AggregateShape::Rounded);
        assert!(
            worn < count * 6 / 10,
            "Rounded is behaving like a fracture: {worn} of {count}"
        );
    }

    #[test]
    fn the_ground_is_not_the_same_everywhere() {
        // Procedural noise is statistically uniform — every patch has the same
        // distribution as every other — so a metre of it reads as one texture
        // tiled however well that texture is tuned. Real ground has history, and
        // no two hand's-widths of it match.
        //
        // The wall's position slides with a slow field, so a patch of merged
        // sheets and a patch of scattered islands are plainly different ground.
        let sheets = shape(0.5, AggregateShape::RoundedRidged, 0.0, -0.18);
        let islands = shape(0.5, AggregateShape::RoundedRidged, 0.0, 0.18);
        assert!(
            sheets - islands > 0.25,
            "the same noise value gave {sheets} and {islands}; the ground is uniform"
        );

        // And the field is slower than the cluster mask beside it, so "how deep"
        // and "what kind" stay two signals rather than collapsing into one.
        let profile = profile(vec![band(0.06, 0.02)]);
        let mut seen: Vec<f32> = (0..40)
            .map(|i| centre_shift(&profile, 99, Vec2::new(i as f32 * 0.35, 0.0)))
            .collect();
        seen.sort_by(f32::total_cmp);
        let spread = seen[seen.len() - 1] - seen[0];
        assert!(spread > 0.10, "the shift barely moves across 14 m: {spread}");
    }

    #[test]
    fn every_shape_averages_zero() {
        // Otherwise raising a soil's shape factor lifts the whole surface, and
        // an author tuning the look of the clods moves the ground under them.
        let count = 8000;
        for kind in [
            AggregateShape::Rounded,
            AggregateShape::RoundedRidged,
            AggregateShape::Angular,
        ] {
            let mean: f32 = (0..count)
                .map(|i| shape(i as f32 / count as f32, kind, 0.0, 0.0))
                .sum::<f32>()
                / count as f32;
            assert!(mean.abs() < 0.01, "{kind:?} averages {mean}");
        }
    }

    #[test]
    fn a_clod_has_a_flank_a_gradient_cannot_reach() {
        // The defect this transform exists for, stated as the property that
        // fixes it. A power curve on the raw value multiplies the field's own
        // gradient by its exponent and no more, so a band declaring 17 mm over
        // 50 mm could never present a face steeper than about 19 degrees — and
        // the sun is at 35, so no bare ground in the scene cast a shadow on
        // itself at any moisture, anywhere.
        //
        // Compressing the rise into a fraction of the range multiplies the
        // gradient by `(pi/2) / flank` instead, which does not depend on the
        // amplitude at all. That is the whole difference between soil and dunes.
        let step = 1.0e-4;
        let steepest = |kind| {
            (0..10_000)
                .map(|i| {
                    let u = i as f32 / 10_000.0;
                    (shape(u + step, kind, 0.0, 0.0) - shape(u, kind, 0.0, 0.0)) / step
                })
                .fold(0.0f32, f32::max)
        };
        // Four and a half times the raw field's own slope, for a clod.
        let ridged = steepest(AggregateShape::RoundedRidged);
        assert!(ridged > 4.0, "RoundedRidged peaks at {ridged}");
        // And weathered ground is gentler than turned ground, which is what
        // the two shapes are for.
        let rounded = steepest(AggregateShape::Rounded);
        assert!(
            rounded < ridged,
            "Rounded {rounded} is not gentler than RoundedRidged {ridged}"
        );
    }

    #[test]
    fn a_lattice_that_cannot_resolve_a_flank_widens_it() {
        // A rise drawn narrower than the thing sampling it is not a crisp edge,
        // it is a staircase whose steps move when the window moves — and the
        // same ground is sampled by overlapping windows that have to agree.
        //
        // So the shape gives the steepness back rather than aliasing, and a wide
        // view gets soft lumps of the same field in the same places instead of
        // clods that flicker.
        let sharp = shape(0.7, AggregateShape::RoundedRidged, 0.0, 0.0);
        let blunt = shape(0.7, AggregateShape::RoundedRidged, 0.9, 0.0);
        assert!(
            blunt < sharp,
            "a floored flank did not lower the crest: {blunt} against {sharp}"
        );

        // Coarse lattice, coarse flank. Four samples across a wavelength — the
        // threshold for carrying a band at all — leaves no room for an edge.
        assert!(flank_floor(0.06, 0.015) >= 1.0);
        // Fifteen across it does, which is what a soil declaring a mesh floor
        // is buying.
        assert!(flank_floor(0.06, 0.004) < 0.34);
    }

    #[test]
    fn cracks_close_when_the_ground_is_wet() {
        let soil = profile(vec![band(0.05, 0.02)]);
        let cracks = CrackProfile {
            polygon_m: 0.15,
            width_m: 0.008,
            depth_m: 0.03,
            secondary: 0.4,
            curl_m: 0.002,
            moisture_ceiling: 0.35,
        };
        let dry = GroundState {
            moisture: 0.02,
            desiccation: 1.0,
            ..GroundState::default()
        };
        let wet = GroundState {
            moisture: 0.60,
            desiccation: 1.0,
            ..GroundState::default()
        };
        // Somewhere on a crack, found by sampling rather than asserted blind.
        let deepest = (0..400)
            .map(|i| {
                let world = Vec2::new(i as f32 * 0.004, 0.31);
                crack_depth(&cracks, &soil, world, &dry, 7)
            })
            .fold(0.0f32, f32::max);
        assert!(deepest > 0.004, "nothing cracked: {deepest}");
        for i in 0..400 {
            let world = Vec2::new(i as f32 * 0.004, 0.31);
            assert_eq!(crack_depth(&cracks, &soil, world, &wet, 7), 0.0);
        }
    }

    #[test]
    fn cracks_close_where_the_ground_has_been_churned() {
        let soil = profile(vec![band(0.05, 0.02)]);
        let cracks = CrackProfile {
            polygon_m: 0.15,
            width_m: 0.008,
            depth_m: 0.03,
            secondary: 0.4,
            curl_m: 0.002,
            moisture_ceiling: 0.35,
        };
        let still = GroundState {
            moisture: 0.02,
            desiccation: 1.0,
            ..GroundState::default()
        };
        let churned = GroundState {
            disturbance: 1.0,
            ..still
        };
        for i in 0..400 {
            let world = Vec2::new(i as f32 * 0.004, 0.31);
            assert_eq!(crack_depth(&cracks, &soil, world, &churned, 7), 0.0);
            let _ = crack_depth(&cracks, &soil, world, &still, 7);
        }
    }

    #[test]
    fn ripples_flatten_as_the_sand_wets() {
        let ripples = RippleProfile {
            wavelength_m: 0.14,
            amplitude_m: 0.006,
            direction_rad: 0.4,
            crest_sharpness: 0.35,
            asymmetry: 0.15,
            meander_m: 0.9,
            wetness_suppression: 0.85,
        };
        let dry = GroundState::default();
        let wet = GroundState {
            moisture: 1.0,
            ..GroundState::default()
        };
        let energy = |state: &GroundState| {
            (0..500)
                .map(|i| {
                    let world = Vec2::new(i as f32 * 0.01, 0.7);
                    ripple_height(&ripples, world, state, 11).abs()
                })
                .sum::<f32>()
        };
        let (dry, wet) = (energy(&dry), energy(&wet));
        assert!(dry > 0.0);
        assert!(wet < dry * 0.2, "dry {dry}, wet {wet}");
    }

    #[test]
    fn ripples_run_across_the_wind_rather_than_along_it() {
        // The one property that makes them read as ripples. Walking along the
        // wind should cross crest after crest; walking across it should not.
        let ripples = RippleProfile {
            wavelength_m: 0.14,
            amplitude_m: 0.006,
            direction_rad: 0.0,
            crest_sharpness: 0.0,
            asymmetry: 0.0,
            meander_m: 0.0,
            wetness_suppression: 0.0,
        };
        let state = GroundState::default();
        let variation = |step: Vec2| {
            (0..200)
                .map(|i| ripple_height(&ripples, step * i as f32, &state, 3))
                .collect::<Vec<_>>()
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .sum::<f32>()
        };
        let along_wind = variation(Vec2::new(0.01, 0.0));
        let across_wind = variation(Vec2::new(0.0, 0.01));
        assert!(
            across_wind < along_wind * 0.01,
            "{across_wind} vs {along_wind}"
        );
    }
}

/// A profile for tests in this crate that need one.
///
/// Not `#[cfg(test)]`: `relief`'s tests are in a sibling module, and a
/// `cfg(test)` item in `ground` is invisible to them. Kept `doc(hidden)` so it
/// does not appear in the crate's surface.
#[doc(hidden)]
pub mod tests_support {
    use terrain_core::ground_material::*;
    use terrain_core::ids::{AppearanceKey, GroundProfileKey};

    /// The shipped loam's measurements, for a fixture.
    pub fn loam() -> GroundMaterialProfile {
        GroundMaterialProfile {
            key: GroundProfileKey::new("fixture_loam").expect("valid"),
            shader: AppearanceKey::new("surface.ground").expect("valid"),
            display_name: "Fixture loam".into(),
            optics: GroundOptics {
                dry_palette: Palette {
                    low: [0.0090, 0.0079, 0.0074],
                    mid: [0.0545, 0.0343, 0.0222],
                    high: [0.2420, 0.1550, 0.0905],
                },
                wet: WetResponse {
                    wet_mid: [0.0210, 0.0116, 0.0058],
                    roughness_wet: 0.22,
                    saturation_flattening: 0.45,
                    film_ior: 1.33,
                },
                roughness_dry: Span {
                    low: 0.62,
                    high: 0.88,
                },
                ior: 1.5,
                region_wavelength_m: 1.6,
                region_strength: 0.35,
                patch_wavelength_m: 0.28,
                patch_strength: 0.22,
                grain_strength: 0.12,
            },
            structure: GroundStructure {
                bands: Vec::new(),
                cohesion: 0.55,
                cluster_wavelength_m: 0.42,
                cluster_strength: 0.35,
                mesh_floor_m: None,
            },
            scatter: GroundScatter {
                grit_per_m2: 0.0,
                pebble_per_m2: 0.0,
                fragment_radius_m: Span {
                    low: 0.002,
                    high: 0.008,
                },
            },
            ripples: None,
            cracks: None,
            vegetation_affinity: 0.0,
        }
    }
}

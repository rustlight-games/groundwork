//! What a ground material is made of, and how it responds.
//!
//! A [`MaterialDef`](crate::document::MaterialDef) in a terrain document is a
//! *semantic identity* — "this ground is compacted dirt" — and deliberately says
//! nothing about colour or relief. This module is the other half: the physical
//! description a renderer needs to draw that identity, held in its own versioned
//! asset so that twenty documents can share one soil and one edit retunes all of
//! them.
//!
//! ## Material, state, disturbance
//!
//! Three different things, and conflating any two of them is what produces a
//! parameter set nobody can reason about:
//!
//! > **Material** says what particles are present. **State** says their current
//! > condition. **Disturbance** says what happened to them.
//!
//! | Question | Where the answer lives |
//! | --- | --- |
//! | Is this loam, clay, beach sand or volcanic dust? | this profile |
//! | Is it wet, compacted, dry or loose? | a modifier channel with a [`ModifierRole`](crate::document::ModifierRole) |
//! | Was it churned by wheels, or worn smooth by feet? | a disturbance channel, and the feature frames |
//! | Does water collect here? | derived hydrology, and the moisture channel |
//!
//! That rule settles the question this system kept tripping over. **Mud is not a
//! material.** Mud is loam whose moisture is high, and churned mud is loam whose
//! moisture is high and whose disturbance is high and whose compaction has been
//! broken up. A document that wants mud raises the state channels on a soil; it
//! does not swap in an unrelated brown substrate whose relationship to the soil
//! beside it nothing can express.
//!
//! Materials stay separate when the *composition* genuinely differs: loam beside
//! beach sand, clay beside peat, pale desert sand beside dark organic soil. Those
//! are different particles, not different weather.
//!
//! ## Everything here is measured, in metres, in linear light
//!
//! Colours are linear RGB, not sRGB — they are handed to a path tracer, not to a
//! screen. Lengths are metres. A wavelength is the distance between one feature
//! and the next; an amplitude is peak to trough, so a field of clods declaring
//! `0.017` stands ±8.5 mm — which, on a five-centimetre wavelength, is a soil
//! aggregate standing about a third of its own width.
//!
//! Stating the units in the type is not decoration. The first version of this
//! carried a "chunkiness" slider whose relationship to a centimetre was one
//! magic constant in a shader, and every attempt to tune it was a guess.
//!
//! ## Why a three-stop palette rather than one colour
//!
//! One base colour plus procedural variation always ends up as a hue with noise
//! multiplied into it, and multiplication can only darken toward black. Real
//! earth varies in hue as well as value — its dry crests are not merely brighter
//! than its damp hollows, they are less saturated and warmer — so the palette
//! names three points on the range and the field interpolates between them.
//!
//! Three, not a general gradient, because three is what an author can hold in
//! their head and what a measurement of a photograph actually supports: a
//! shadowed value, a median, and a highlight.

use crate::ids::{AppearanceKey, GroundProfileKey};

/// A linear-RGB colour. Not sRGB, and never displayed without a transform.
pub type LinearRgb = [f32; 3];

/// An inclusive range of a quantity that varies across a surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub low: f32,
    pub high: f32,
}

impl Span {
    pub const fn new(low: f32, high: f32) -> Self {
        Self { low, high }
    }

    pub fn is_valid(self) -> bool {
        self.low.is_finite() && self.high.is_finite() && self.low <= self.high
    }

    pub fn lerp(self, t: f32) -> f32 {
        self.low + (self.high - self.low) * t.clamp(0.0, 1.0)
    }
}

/// The three points a ground's dry colour is interpolated between.
///
/// `low` is what it looks like in its own shadow and in its hollows, `high` is a
/// dry crest catching the sun, `mid` is the median — the value a photograph of
/// the material actually measures at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub low: LinearRgb,
    pub mid: LinearRgb,
    pub high: LinearRgb,
}

impl Palette {
    /// The colour at `t` in `0..1`, through the mid stop.
    pub fn sample(&self, t: f32) -> LinearRgb {
        let t = t.clamp(0.0, 1.0);
        let (a, b, u) = if t < 0.5 {
            (self.low, self.mid, t * 2.0)
        } else {
            (self.mid, self.high, (t - 0.5) * 2.0)
        };
        [
            a[0] + (b[0] - a[0]) * u,
            a[1] + (b[1] - a[1]) * u,
            a[2] + (b[2] - a[2]) * u,
        ]
    }

    fn is_valid(&self) -> bool {
        [self.low, self.mid, self.high]
            .iter()
            .all(|c| c.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0))
    }
}

/// What water does to this ground.
///
/// Wet earth is emphatically **not** dry earth turned down, and a shader that
/// only darkens produces ground that reads as being in shadow. Water fills the
/// pores, so the air-soil boundary that scattered light diffusely becomes a
/// water-soil boundary: internal scattering falls, absorption rises, and the
/// outer surface becomes a smooth water-air interface.
///
/// Three things follow, and the third is the one that sells it:
///
/// ```text
/// albedo      darkens toward its own square
/// hue         warms — the film absorbs blue and green harder than red
/// roughness   collapses, and does so before the darkening is even noticeable
/// ```
///
/// ## The author writes two colours, not three gain factors
///
/// The obvious parameterisation is a per-channel multiplier on the square, and
/// it is unusable: the multiplier that keeps a dark meadow floor in range is
/// four times the one that keeps a mid loam in range, so the number means
/// nothing on its own and cannot be compared between two soils.
///
/// So the author writes [`wet_mid`](WetResponse::wet_mid) — *what this soil's
/// median tone becomes when it is soaked* — which is a colour they can measure
/// off a photograph of wet ground. The square law is fitted through it. The
/// darkening still bites hardest on the brightest parts, which is the whole
/// visual point, and the number in the file is one an author can argue about.
///
/// There is no subsurface scattering here and there should not be. Production
/// mud shaders do not use it — mud is a rough dark dielectric under a glossy
/// coat, and scattering inside a medium that absorbs this hard buys nothing for
/// the cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WetResponse {
    /// What the palette's `mid` stop becomes at full saturation.
    pub wet_mid: LinearRgb,
    /// Roughness at full saturation.
    pub roughness_wet: f32,
    /// How much of the relief water fills in, `0..1`.
    ///
    /// Water finds the small cavities first, so a wet surface is smoother than
    /// its dry self at grain scale long before it is smoother at clod scale.
    pub saturation_flattening: f32,
    /// Index of refraction of the surface film. Water is 1.333.
    pub film_ior: f32,
}

/// How this ground returns light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundOptics {
    pub dry_palette: Palette,
    pub wet: WetResponse,
    /// Roughness across the dry range, driven by the same field as the palette.
    pub roughness_dry: Span,
    /// Index of refraction of the substrate itself. Silicate soils sit near 1.5.
    pub ior: f32,
    /// The broad tonal sweep: which clearing this is, in metres per cycle.
    pub region_wavelength_m: f32,
    /// How far that sweep moves the palette, `0..1`.
    pub region_strength: f32,
    /// Scuffs and patches, the size of a footfall.
    pub patch_wavelength_m: f32,
    pub patch_strength: f32,
    /// A darkening rather than a second colour, at grain scale.
    pub grain_strength: f32,
}

/// The shape family a material's aggregates take.
///
/// Not decoration: a rounded clod and an angular fragment catch a low sun
/// completely differently, and a field of one reads as soil where a field of the
/// other reads as gravel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AggregateShape {
    /// Soft lumps. Wet or organic soil, weathered ground.
    #[default]
    Rounded,
    /// Rounded, with a broken ridge along the top. Freshly turned soil.
    RoundedRidged,
    /// Fractured, with flat faces and hard edges. Dry clay, hardpan, scree.
    Angular,
}

impl AggregateShape {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rounded => "Rounded",
            Self::RoundedRidged => "RoundedRidged",
            Self::Angular => "Angular",
        }
    }

    /// How hard the ridge transform bites, `0..1`.
    ///
    /// Zero leaves the noise alone and reads as soft lumps; one folds it about
    /// its mean and squares the result, which is what puts a crease along the
    /// top of a clod and a flat between clods.
    pub fn ridge(self) -> f32 {
        match self {
            Self::Rounded => 0.0,
            Self::RoundedRidged => 0.55,
            Self::Angular => 1.0,
        }
    }
}

/// One scale of relief.
///
/// Bare ground has several and a single noise frequency reads as sandpaper at
/// any magnification. The bands a soil actually has, measured:
///
/// ```text
/// aggregates   2 - 8 cm
/// crumb        2 - 15 mm
/// grain        0.5 - 2 mm
/// ```
///
/// ## Which of these is geometry is not written here
///
/// It is tempting to declare "clods are mesh, grain is bump" and be done. That
/// declaration is wrong as often as it is right, because it depends on something
/// the profile cannot know: **how finely the ground is being sampled**. A band
/// carried in the mesh at three samples per wavelength is not carried, it is
/// aliased, and it comes out as a different and worse-looking band.
///
/// So a profile declares its bands and the exporter splits them. Anything the
/// lattice resolves becomes displaced geometry; everything below becomes shader
/// bump. Each band is drawn exactly once, by whichever half can actually draw
/// it, and lowering the sampling rate moves a band across the line rather than
/// dropping it.
///
/// The split matters because a bump cannot make a lump occlude its own shadow,
/// and the shadows the clods throw are most of what makes bare ground read as
/// ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReliefBand {
    /// Distance from one feature to the next.
    pub wavelength_m: f32,
    /// Peak to trough.
    pub amplitude_m: f32,
    pub shape: AggregateShape,
    /// How much full compaction flattens this band, `0..1`.
    ///
    /// Per band because compaction does not act evenly across scales: a wheel
    /// presses clods flat and barely touches the grain between them.
    pub compaction_response: f32,
    /// Whether the cluster mask can suppress this band.
    ///
    /// Coarse bands should be; fine ones should not. Grain that came and went
    /// with the clods would read as patches of polish.
    pub clustered: bool,
}

/// The relief this ground carries, and what breaks it up.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundStructure {
    /// Relief bands, strictly coarsest first.
    pub bands: Vec<ReliefBand>,
    /// How large the calm-and-cloddy patches are.
    ///
    /// Uniform clods everywhere read as gravel. Real ground has smooth
    /// compacted stretches next to broken ones, and this is the scale of that
    /// alternation.
    pub cluster_wavelength_m: f32,
    /// How completely the clustering can suppress a clustered band, `0..1`.
    pub cluster_strength: f32,
    /// How readily this material holds a mark, `0..1`.
    ///
    /// Cohesion is what separates a rut from a trough: plastic clay keeps the
    /// shape it was pushed into, and dry sand collapses back to its angle of
    /// repose. Also gates cracking, which needs a material that can hold a wall
    /// open.
    pub cohesion: f32,
}

/// Directional relief left by wind on a granular surface.
///
/// What makes sand sand. Isotropic noise at ripple amplitude reads as a rough
/// plane; the coherence over distance is the entire signal, and it is the one
/// thing a noise texture cannot supply on its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RippleProfile {
    /// Crest to crest.
    pub wavelength_m: f32,
    /// Peak to trough.
    pub amplitude_m: f32,
    /// Which way the wind blew, in radians anticlockwise from +u.
    pub direction_rad: f32,
    /// How pointed the crests are, `0..1`. Zero is a sine, one is a sharp ridge.
    pub crest_sharpness: f32,
    /// Windward-lee asymmetry, `-1..1`. Positive steepens the lee face.
    pub asymmetry: f32,
    /// How far the wavefronts wander, in metres.
    ///
    /// Zero gives parallel sine waves, which is the single most recognisable
    /// procedural failure there is.
    pub meander_m: f32,
    /// How completely saturation flattens the ripples, `0..1`.
    ///
    /// Wet sand does not ripple: the water holds the grains where they are.
    pub wetness_suppression: f32,
}

/// Desiccation cracking.
///
/// A capability, not an event. The profile says whether this material *can*
/// crack — which is a question about clay content and cohesion — and the
/// [`ModifierRole::Desiccation`](crate::document::ModifierRole) channel says
/// whether it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrackProfile {
    /// Across one polygon of the network.
    pub polygon_m: f32,
    /// How wide a primary crack opens.
    pub width_m: f32,
    /// How deep it cuts.
    pub depth_m: f32,
    /// Relative scale of the secondary network inside each polygon, `0..1`.
    ///
    /// A single-scale network is a Voronoi diagram and looks like one. Real
    /// cracking has a hierarchy: wide primaries, then finer branches that stop
    /// against them.
    pub secondary: f32,
    /// How much the polygon shoulders curl up, in metres.
    pub curl_m: f32,
    /// Above this moisture nothing cracks, `0..1`.
    pub moisture_ceiling: f32,
}

/// What lies loose on top.
///
/// Densities are per square metre of ground that is *fully* this material; the
/// realised weight scales them, so a half-and-half boundary gets half of each
/// rather than a doubled stripe of both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundScatter {
    pub grit_per_m2: f32,
    pub pebble_per_m2: f32,
    /// Radius range of the scattered fragments.
    pub fragment_radius_m: Span,
}

/// How much this ground supports plants, before any document says otherwise.
///
/// A default rather than a rule: the same beach sand supports dune grass on one
/// map and nothing on another, so a material may override it. What this replaces
/// is worse than either — deciding whether a material grows grass by looking for
/// the substring `dirt` in its key.
pub type VegetationAffinity = f32;

/// One ground material, completely described.
#[derive(Clone, Debug, PartialEq)]
pub struct GroundMaterialProfile {
    pub key: GroundProfileKey,
    /// Which renderer-side implementation can draw it.
    ///
    /// Separate from the profile because it answers a different question. The
    /// profile says what the ground is made of; this says which shader graph
    /// knows how to read that. Nearly everything is `surface.ground`.
    pub shader: AppearanceKey,
    pub display_name: String,
    pub optics: GroundOptics,
    pub structure: GroundStructure,
    pub ripples: Option<RippleProfile>,
    pub cracks: Option<CrackProfile>,
    pub scatter: GroundScatter,
    pub vegetation_affinity: VegetationAffinity,
}

/// Why a profile could not be used.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileProblem {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ProfileProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl GroundMaterialProfile {
    /// Every problem with this profile, in one pass.
    ///
    /// Collected rather than short-circuited, for the same reason document
    /// validation collects: an author who mistyped three wavelengths should be
    /// told about three, not told about one and sent round again.
    pub fn problems(&self) -> Vec<ProfileProblem> {
        let mut found = Vec::new();
        let mut check = |ok: bool, field: &str, message: &str| {
            if !ok {
                found.push(ProfileProblem {
                    field: field.to_string(),
                    message: message.to_string(),
                });
            }
        };

        let o = &self.optics;
        check(
            o.dry_palette.is_valid(),
            "optics.dry_palette",
            "colours must be finite and within 0..1 — these are linear RGB, not sRGB",
        );
        check(
            o.roughness_dry.is_valid() && o.roughness_dry.low >= 0.0 && o.roughness_dry.high <= 1.0,
            "optics.roughness_dry",
            "must be an ascending range within 0..1",
        );
        check(
            (0.0..=1.0).contains(&o.wet.roughness_wet),
            "optics.wet.roughness_wet",
            "must be within 0..1",
        );
        check(
            (0.0..=1.0).contains(&o.wet.saturation_flattening),
            "optics.wet.saturation_flattening",
            "must be within 0..1",
        );
        check(
            o.wet
                .wet_mid
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0),
            "optics.wet.wet_mid",
            "must be finite and within 0..1 — these are linear RGB, not sRGB",
        );
        check(
            o.wet
                .wet_mid
                .iter()
                .zip(o.dry_palette.mid)
                .all(|(wet, dry)| *wet <= dry + 1e-6),
            "optics.wet.wet_mid",
            "must not be brighter than the dry mid stop — wetting darkens",
        );
        check(
            o.wet.film_ior >= 1.0 && o.wet.film_ior <= 3.0,
            "optics.wet.film_ior",
            "must be within 1..3 — water is 1.333",
        );
        check(
            o.ior >= 1.0 && o.ior <= 3.0,
            "optics.ior",
            "must be within 1..3 — silicate soils sit near 1.5",
        );
        for (name, value) in [
            ("optics.region_wavelength_m", o.region_wavelength_m),
            ("optics.patch_wavelength_m", o.patch_wavelength_m),
        ] {
            check(value > 0.0 && value.is_finite(), name, "must be positive");
        }
        for (name, value) in [
            ("optics.region_strength", o.region_strength),
            ("optics.patch_strength", o.patch_strength),
            ("optics.grain_strength", o.grain_strength),
        ] {
            check((0.0..=1.0).contains(&value), name, "must be within 0..1");
        }

        let s = &self.structure;
        check(
            !s.bands.is_empty(),
            "structure.bands",
            "must declare at least one relief band — ground with no relief at any \
             scale is a plane",
        );
        check(
            s.cluster_wavelength_m > 0.0 && s.cluster_wavelength_m.is_finite(),
            "structure.cluster_wavelength_m",
            "must be positive",
        );
        for (name, value) in [
            ("structure.cluster_strength", s.cluster_strength),
            ("structure.cohesion", s.cohesion),
        ] {
            check((0.0..=1.0).contains(&value), name, "must be within 0..1");
        }
        for (index, band) in s.bands.iter().enumerate() {
            check(
                band.wavelength_m > 0.0 && band.wavelength_m.is_finite(),
                &format!("structure.bands[{index}].wavelength_m"),
                "must be positive",
            );
            check(
                band.amplitude_m.is_finite() && band.amplitude_m >= 0.0,
                &format!("structure.bands[{index}].amplitude_m"),
                "must be finite and non-negative",
            );
            check(
                (0.0..=1.0).contains(&band.compaction_response),
                &format!("structure.bands[{index}].compaction_response"),
                "must be within 0..1",
            );
            // A lump as tall as it is wide is not a clod, it is a spike, and a
            // field of them reads as speckle rather than as ground. Soil
            // aggregates stand about a third of their width; loose turned earth
            // reaches a half. Refusing above three quarters catches an amplitude
            // that was written for a differently-scaled noise, which is exactly
            // the mistake that produced a plate of black dots.
            check(
                band.amplitude_m <= band.wavelength_m * 0.75,
                &format!("structure.bands[{index}].amplitude_m"),
                "must be at most three quarters of the wavelength — taller than \
                 that is a spike field, not ground",
            );
            // Bands have to stay separated or they are one band with extra
            // parameters, and the exporter's coarse-to-fine split has no
            // defensible place to cut.
            if index > 0 {
                check(
                    band.wavelength_m < s.bands[index - 1].wavelength_m,
                    &format!("structure.bands[{index}].wavelength_m"),
                    "must be finer than the band before it — bands are declared \
                     coarsest first",
                );
            }
        }

        if let Some(r) = &self.ripples {
            check(
                r.wavelength_m > 0.0 && r.wavelength_m.is_finite(),
                "ripples.wavelength_m",
                "must be positive",
            );
            check(
                r.amplitude_m.is_finite() && r.amplitude_m >= 0.0,
                "ripples.amplitude_m",
                "must be finite and non-negative",
            );
            check(
                r.direction_rad.is_finite(),
                "ripples.direction_rad",
                "must be finite",
            );
            check(
                (0.0..=1.0).contains(&r.crest_sharpness),
                "ripples.crest_sharpness",
                "must be within 0..1",
            );
            check(
                (-1.0..=1.0).contains(&r.asymmetry),
                "ripples.asymmetry",
                "must be within -1..1",
            );
            check(
                r.meander_m.is_finite() && r.meander_m >= 0.0,
                "ripples.meander_m",
                "must be finite and non-negative",
            );
            check(
                (0.0..=1.0).contains(&r.wetness_suppression),
                "ripples.wetness_suppression",
                "must be within 0..1",
            );
        }

        if let Some(c) = &self.cracks {
            check(
                c.polygon_m > 0.0 && c.polygon_m.is_finite(),
                "cracks.polygon_m",
                "must be positive",
            );
            check(
                c.width_m > 0.0 && c.width_m < c.polygon_m,
                "cracks.width_m",
                "must be positive and narrower than a polygon",
            );
            check(
                c.depth_m.is_finite() && c.depth_m >= 0.0,
                "cracks.depth_m",
                "must be finite and non-negative",
            );
            check(
                (0.0..=1.0).contains(&c.secondary),
                "cracks.secondary",
                "must be within 0..1",
            );
            check(
                c.curl_m.is_finite() && c.curl_m >= 0.0,
                "cracks.curl_m",
                "must be finite and non-negative",
            );
            check(
                (0.0..=1.0).contains(&c.moisture_ceiling),
                "cracks.moisture_ceiling",
                "must be within 0..1",
            );
            // Cracking is a cohesion phenomenon. A material that cannot hold a
            // wall open cannot hold a crack, and declaring one on dry sand is
            // an authoring mistake worth reporting rather than rendering.
            check(
                self.structure.cohesion >= 0.25,
                "cracks",
                "declared on a material with cohesion below 0.25 — loose material \
                 slumps instead of cracking",
            );
        }

        for (name, value) in [
            ("scatter.grit_per_m2", self.scatter.grit_per_m2),
            ("scatter.pebble_per_m2", self.scatter.pebble_per_m2),
        ] {
            check(
                value.is_finite() && value >= 0.0,
                name,
                "must be finite and non-negative",
            );
        }
        check(
            self.scatter.fragment_radius_m.is_valid() && self.scatter.fragment_radius_m.low > 0.0,
            "scatter.fragment_radius_m",
            "must be an ascending range of positive radii",
        );
        check(
            (0.0..=1.0).contains(&self.vegetation_affinity),
            "vegetation_affinity",
            "must be within 0..1",
        );

        found
    }

    /// The dry colour at a point on this material's tonal range.
    pub fn dry_colour(&self, tone: f32) -> LinearRgb {
        self.optics.dry_palette.sample(tone)
    }

    /// The per-channel factor that carries the dry mid stop to the wet one.
    ///
    /// `wet = dry² × gain`, fitted so that `mid² × gain == wet_mid`. A channel
    /// whose dry mid is zero has no curve to fit and stays at zero.
    pub fn wet_gain(&self) -> LinearRgb {
        let mid = self.optics.dry_palette.mid;
        let target = self.optics.wet.wet_mid;
        let mut gain = [0.0; 3];
        for channel in 0..3 {
            let square = mid[channel] * mid[channel];
            gain[channel] = if square > 0.0 {
                target[channel] / square
            } else {
                0.0
            };
        }
        gain
    }

    /// The albedo at a tonal position and a wetness.
    ///
    /// The same maths the Cycles graph runs, kept here so a test can assert on
    /// it and a diagnostic can print it without starting Blender.
    pub fn albedo(&self, tone: f32, wetness: f32) -> LinearRgb {
        let dry = self.dry_colour(tone);
        let wetness = wetness.clamp(0.0, 1.0);
        let gain = self.wet_gain();
        let mut out = [0.0; 3];
        for channel in 0..3 {
            let wet = (dry[channel] * dry[channel] * gain[channel]).min(dry[channel]);
            out[channel] = dry[channel] + (wet - dry[channel]) * wetness;
        }
        out
    }

    /// The roughness at a tonal position and a wetness.
    pub fn roughness(&self, tone: f32, wetness: f32) -> f32 {
        let dry = self.optics.roughness_dry.lerp(tone);
        let wet = self.optics.wet.roughness_wet;
        dry + (wet - dry) * wetness.clamp(0.0, 1.0)
    }

    /// How much of one band survives this state, `0..1`.
    ///
    /// Compaction presses it flat; saturation smooths it over. Multiplied
    /// because they are independent — a wet packed track is smoother than
    /// either a wet loose one or a dry packed one.
    pub fn band_scale(&self, band: &ReliefBand, compaction: f32, moisture: f32) -> f32 {
        let packed = 1.0 - band.compaction_response * compaction.clamp(0.0, 1.0);
        let sodden = 1.0 - self.optics.wet.saturation_flattening * moisture.clamp(0.0, 1.0);
        (packed * sodden).clamp(0.0, 1.0)
    }

    /// The coarsest band this ground has, which is what sets the sampling rate a
    /// mesh needs to carry any of it.
    pub fn coarsest_band(&self) -> Option<&ReliefBand> {
        self.structure.bands.first()
    }
}

/// A profile the caller has already read off disk, keyed by asset path.
///
/// Owned rather than borrowed because it outlives the load, and a map rather
/// than a list because a document names profiles by path and several materials
/// may name the same one.
///
/// The parsing lives in `terrain_format`, which is where the versioned files
/// live; this crate holds the meaning and depends on nothing but `serde`.
#[derive(Clone, Debug, Default)]
pub struct GroundProfileLibrary {
    entries: std::collections::BTreeMap<String, std::sync::Arc<GroundMaterialProfile>>,
}

impl GroundProfileLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, profile: GroundMaterialProfile) {
        self.entries.insert(path.into(), std::sync::Arc::new(profile));
    }

    pub fn with(mut self, path: impl Into<String>, profile: GroundMaterialProfile) -> Self {
        self.insert(path, profile);
        self
    }

    pub fn get(&self, path: &str) -> Option<&std::sync::Arc<GroundMaterialProfile>> {
        self.entries.get(path)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &std::sync::Arc<GroundMaterialProfile>)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> GroundMaterialProfile {
        GroundMaterialProfile {
            key: GroundProfileKey::new("compacted_loam").unwrap(),
            shader: AppearanceKey::new("surface.ground").unwrap(),
            display_name: "Compacted loam".into(),
            optics: GroundOptics {
                dry_palette: Palette {
                    low: [0.0500, 0.0315, 0.0180],
                    mid: [0.0840, 0.0530, 0.0300],
                    high: [0.1550, 0.0977, 0.0558],
                },
                wet: WetResponse {
                    wet_mid: [0.0294, 0.0154, 0.0069],
                    roughness_wet: 0.20,
                    saturation_flattening: 0.45,
                    film_ior: 1.333,
                },
                roughness_dry: Span::new(0.82, 0.96),
                ior: 1.48,
                region_wavelength_m: 2.0,
                region_strength: 0.5,
                patch_wavelength_m: 0.25,
                patch_strength: 0.9,
                grain_strength: 0.55,
            },
            structure: GroundStructure {
                bands: vec![
                    ReliefBand {
                        wavelength_m: 0.05,
                        amplitude_m: 0.017,
                        shape: AggregateShape::RoundedRidged,
                        compaction_response: 0.85,
                        clustered: true,
                    },
                    ReliefBand {
                        wavelength_m: 0.008,
                        amplitude_m: 0.003,
                        shape: AggregateShape::Rounded,
                        compaction_response: 0.60,
                        clustered: true,
                    },
                    ReliefBand {
                        wavelength_m: 0.0012,
                        amplitude_m: 0.0006,
                        shape: AggregateShape::Rounded,
                        compaction_response: 0.30,
                        clustered: false,
                    },
                ],
                cluster_wavelength_m: 0.8,
                cluster_strength: 0.5,
                cohesion: 0.62,
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
    fn a_measured_profile_has_no_problems() {
        assert_eq!(profile().problems(), Vec::new());
    }

    #[test]
    fn the_palette_passes_through_its_middle_stop() {
        let p = profile();
        assert_eq!(p.dry_colour(0.5), p.optics.dry_palette.mid);
        assert_eq!(p.dry_colour(0.0), p.optics.dry_palette.low);
        assert_eq!(p.dry_colour(1.0), p.optics.dry_palette.high);
    }

    #[test]
    fn wetting_darkens_and_warms() {
        let p = profile();
        let dry = p.albedo(0.5, 0.0);
        let wet = p.albedo(0.5, 1.0);
        // Darker in every channel.
        for channel in 0..3 {
            assert!(
                wet[channel] < dry[channel],
                "channel {channel}: {} is not below {}",
                wet[channel],
                dry[channel]
            );
        }
        // And warmer: red survives better than blue, so the ratio rises.
        assert!(wet[0] / wet[2] > dry[0] / dry[2]);
    }

    #[test]
    fn wetting_collapses_roughness() {
        let p = profile();
        assert!(p.roughness(0.5, 0.0) > 0.8);
        assert!(p.roughness(0.5, 1.0) < 0.25);
    }

    #[test]
    fn the_wet_curve_passes_through_the_declared_wet_mid() {
        let p = profile();
        let soaked = p.albedo(0.5, 1.0);
        for channel in 0..3 {
            assert!(
                (soaked[channel] - p.optics.wet.wet_mid[channel]).abs() < 1e-6,
                "channel {channel}: {} is not {}",
                soaked[channel],
                p.optics.wet.wet_mid[channel]
            );
        }
    }

    #[test]
    fn wetting_deepens_the_ground_rather_than_dimming_it() {
        // The whole reason for a square law rather than a multiply. A multiply
        // scales every tone by the same factor, which is exactly what shadow
        // does — so wet ground shaded that way reads as dry ground in shadow.
        //
        // The square widens the tonal range instead: the crest gives up more
        // absolute light than the hollow, and the ratio between them grows. That
        // deepening is what the eye reads as wet.
        let p = profile();
        let hollow = (p.albedo(0.0, 0.0)[0], p.albedo(0.0, 1.0)[0]);
        let crest = (p.albedo(1.0, 0.0)[0], p.albedo(1.0, 1.0)[0]);
        assert!(
            crest.0 - crest.1 > hollow.0 - hollow.1,
            "the crest should lose more light than the hollow"
        );
        assert!(
            crest.1 / hollow.1 > crest.0 / hollow.0,
            "the tonal range should widen, not merely shift down"
        );
    }

    #[test]
    fn packing_and_soaking_both_flatten_and_compound() {
        let p = profile();
        let band = &p.structure.bands[0];
        let loose_dry = p.band_scale(band, 0.0, 0.0);
        let packed_dry = p.band_scale(band, 1.0, 0.0);
        let loose_wet = p.band_scale(band, 0.0, 1.0);
        let packed_wet = p.band_scale(band, 1.0, 1.0);
        assert_eq!(loose_dry, 1.0);
        assert!(packed_dry < loose_dry);
        assert!(loose_wet < loose_dry);
        assert!(packed_wet < packed_dry && packed_wet < loose_wet);
    }

    #[test]
    fn compaction_flattens_the_clods_harder_than_the_grain() {
        // Physical, not cosmetic: a wheel presses clods flat and barely touches
        // the grain between them. A single flattening factor cannot say that,
        // which is why the response lives on the band.
        let p = profile();
        let clods = p.band_scale(&p.structure.bands[0], 1.0, 0.0);
        let grain = p.band_scale(&p.structure.bands[2], 1.0, 0.0);
        assert!(clods < grain, "{clods} >= {grain}");
    }

    #[test]
    fn bands_declared_out_of_order_are_refused() {
        let mut p = profile();
        p.structure.bands[1].wavelength_m = 0.5;
        let problems = p.problems();
        assert!(
            problems
                .iter()
                .any(|p| p.field == "structure.bands[1].wavelength_m"),
            "{problems:?}"
        );
    }

    #[test]
    fn cracking_loose_material_is_refused() {
        let mut p = profile();
        p.structure.cohesion = 0.05;
        p.cracks = Some(CrackProfile {
            polygon_m: 0.15,
            width_m: 0.008,
            depth_m: 0.02,
            secondary: 0.4,
            curl_m: 0.002,
            moisture_ceiling: 0.35,
        });
        assert!(p.problems().iter().any(|p| p.field == "cracks"));
    }

    #[test]
    fn every_problem_is_reported_in_one_pass() {
        let mut p = profile();
        p.optics.ior = 9.0;
        p.optics.region_strength = 4.0;
        p.structure.cohesion = -1.0;
        assert_eq!(p.problems().len(), 3);
    }
}

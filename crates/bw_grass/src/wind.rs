//! Wind.
//!
//! Wind is a *forcing* term, never a displacement. The distinction matters: set
//! grass orientation directly from a flow field and it stops looking like grass
//! and starts looking like a vector-field visualisation, or seaweed — there is
//! no inertia, no overshoot, and no spring-back, because the blade is simply
//! wearing the field. Here wind pushes on the [`field`](crate::field) solver,
//! which owns the elasticity, and recovery falls out of the blade's own
//! dynamics.
//!
//! ## What produces the motion
//!
//! Three layers, none of them a fluid simulation:
//!
//! - **Mean flow.** A constant direction and speed.
//! - **Curl noise.** A sum of sinusoidal stream-function terms, differentiated
//!   analytically. Taking the curl of a potential makes the result
//!   divergence-free by construction, so the turbulence swirls instead of
//!   producing sources and sinks that suck grass into a point.
//! - **Gust fronts.** Travelling bands, which is what gives a field the
//!   readable waves that cross it rather than an even shimmer.
//!
//! ## Cauchy-number saturation
//!
//! Doubling the wind does not double the lean. A real blade *reconfigures* — it
//! bends over, presents less of itself to the flow, and sheds drag. The Cauchy
//! number is the standard dimensionless comparison of fluid forcing against
//! elastic restoration, and running it through a saturating curve reproduces
//! the behaviour that matters: a quadratic-ish response at low speed flattening
//! into a ceiling at high speed. Without it a strong gust drives every blade to
//! the angular cap at once, and the whole field snaps flat like one sheet.

use bevy::prelude::*;

/// Largest lean the wind alone will ask for, in radians.
///
/// Wind bends grass a long way but does not flatten it; that is what
/// [trampling](crate::disturbance) is for.
pub const MAX_WIND_ANGLE: f32 = 70.0 * std::f32::consts::PI / 180.0;

/// Cauchy number at which the response is half of [`MAX_WIND_ANGLE`].
const CAUCHY_HALF: f32 = 0.31;

/// Converts world units into the dimensionless Cauchy number.
const CAUCHY_SCALE: f32 = 1.0;

/// Smallest exposure a fully aligned blade retains.
///
/// Never zero: a blade laid flat along the wind still has thickness, and an
/// exposure of exactly zero leaves it bent with no force holding it there,
/// which looks like the simulation froze.
const MIN_EXPOSURE: f32 = 0.12;

/// Sinusoidal terms of the turbulence stream function.
///
/// Each is `(wavenumber x, wavenumber y, temporal rate, amplitude)`. The
/// wavelengths are deliberately not harmonically related, so the pattern does
/// not visibly repeat over the length of a battle.
const CURL_OCTAVES: [(f32, f32, f32, f32); 3] = [
    (0.091, 0.063, 0.51, 1.00),
    (-0.207, 0.171, 0.83, 0.42),
    (0.431, -0.317, 1.44, 0.17),
];

/// Global wind state.
#[derive(Resource, Clone, Copy, Debug)]
pub struct WindField {
    /// Normalised direction the wind blows toward.
    pub direction: Vec2,
    /// Mean speed in metres per second.
    pub speed: f32,
    /// Peak speed added by curl-noise turbulence.
    pub turbulence: f32,
    /// Peak speed added at the centre of a gust front.
    pub gust_strength: f32,
    /// How fast gust fronts travel downwind, in metres per second.
    ///
    /// Faster than the mean flow, as real gust fronts are: the pattern moves
    /// through the air rather than with it.
    pub gust_speed: f32,
    /// Standard deviation of a gust front, in metres.
    pub gust_width: f32,
    /// Distance between successive fronts, in metres.
    pub gust_spacing: f32,
    /// Seconds since the field started. Advanced by [`advance_wind`].
    pub time: f32,
}

impl Default for WindField {
    fn default() -> Self {
        Self {
            direction: Vec2::new(1.0, 0.35).normalize(),
            speed: 3.4,
            turbulence: 1.3,
            gust_strength: 5.0,
            gust_speed: 7.0,
            gust_width: 2.6,
            gust_spacing: 17.0,
            time: 0.0,
        }
    }
}

/// What the wind asks a patch of grass to do.
#[derive(Clone, Copy, Debug)]
pub struct WindResponse {
    /// Equilibrium bend-angle vector: direction of lean, magnitude in radians.
    pub target: Vec2,
    /// How hard the wind holds the grass there, in 0..=1. Feeds the stiffness
    /// of the wind's target spring, so a still day pushes weakly and a gale
    /// pushes hard — rather than both pulling equally toward different angles.
    pub strength: f32,
}

impl WindField {
    /// Wind velocity at a world position, in metres per second.
    pub fn velocity_at(&self, position: Vec2) -> Vec2 {
        let direction = normalize_or(self.direction, Vec2::X);
        direction * self.speed + self.turbulent(position) + self.gust(position, direction)
    }

    /// Divergence-free turbulence: the curl of a sinusoidal stream function.
    ///
    /// For a 2D potential `psi`, the curl is `(dpsi/dy, -dpsi/dx)`. Each term
    /// of `psi` is `A sin(k . x + nu t)`, so its curl is `A cos(k . x + nu t)`
    /// times `(k_y, -k_x)` — exact, and cheaper than differencing a noise
    /// texture.
    fn turbulent(&self, position: Vec2) -> Vec2 {
        let mut velocity = Vec2::ZERO;
        for (kx, ky, rate, amplitude) in CURL_OCTAVES {
            let phase = position.x * kx + position.y * ky + self.time * rate;
            velocity += Vec2::new(ky, -kx) * (amplitude * phase.cos());
        }
        // The octave wavenumbers set the curl's scale; normalising by their sum
        // keeps `turbulence` meaning "metres per second" rather than "metres
        // per second times whatever the octaves happen to add up to".
        velocity * (self.turbulence / CURL_NORMALISATION)
    }

    /// Travelling gust fronts.
    ///
    /// Two trains at different spacings and speeds. One alone is a metronome —
    /// visibly periodic within a few seconds — and two incommensurate ones read
    /// as weather.
    fn gust(&self, position: Vec2, direction: Vec2) -> Vec2 {
        if self.gust_spacing <= f32::EPSILON || self.gust_width <= f32::EPSILON {
            return Vec2::ZERO;
        }
        let along = position.dot(direction);
        let across = position.perp_dot(direction);

        let primary = front_envelope(
            along - self.gust_speed * self.time,
            self.gust_spacing,
            self.gust_width,
        );
        // Slightly turned, so fronts sweep across the field at an angle rather
        // than arriving as perfectly parallel stripes.
        let secondary = front_envelope(
            along * 0.87 + across * 0.31 - self.gust_speed * 0.62 * self.time,
            self.gust_spacing * 1.618,
            self.gust_width * 1.4,
        );

        direction * (self.gust_strength * (primary + 0.55 * secondary))
    }

    /// The equilibrium lean a given wind velocity asks of a given blade.
    ///
    /// `current_bend` is where the blade already is, which is what allows
    /// reconfiguration: a blade that has already laid over presents less of
    /// itself to the flow and is pushed less hard.
    pub fn lean_target(
        &self,
        velocity: Vec2,
        blade_length: f32,
        relative_stiffness: f32,
        current_bend: Vec2,
    ) -> WindResponse {
        let speed = velocity.length();
        if speed <= f32::EPSILON || blade_length <= f32::EPSILON {
            return WindResponse {
                target: Vec2::ZERO,
                strength: 0.0,
            };
        }
        let flow = velocity / speed;

        // Cauchy number: fluid forcing over elastic restoration, reduced by how
        // much of itself the blade still presents to the flow.
        let cauchy = CAUCHY_SCALE * speed * speed * blade_length.powi(3)
            / relative_stiffness.max(1e-4)
            * exposure(current_bend, flow);

        let saturation = cauchy / (cauchy + CAUCHY_HALF);
        WindResponse {
            target: flow * (MAX_WIND_ANGLE * saturation),
            strength: saturation,
        }
    }
}

/// Sine of the angle between a bent blade's tangent and the horizontal flow.
///
/// The blade tangent lifts out of the ground plane, so this is a genuinely
/// three-dimensional quantity: an upright blade is fully exposed even though
/// its horizontal bend is zero, and a blade laid flat *along* the wind is
/// barely exposed at all. Computing it from horizontal bend alone would get the
/// upright case exactly backwards.
fn exposure(bend: Vec2, flow: Vec2) -> f32 {
    let angle = bend.length();
    let lean = normalize_or(bend, flow);
    let tangent = Vec3::new(lean.x * angle.sin(), lean.y * angle.sin(), angle.cos());
    let along = tangent.dot(flow.extend(0.0));
    let across = (1.0 - along * along).max(0.0).sqrt();
    MIN_EXPOSURE + (1.0 - MIN_EXPOSURE) * across
}

/// A periodic train of Gaussian fronts, peaking at one.
fn front_envelope(travel: f32, spacing: f32, width: f32) -> f32 {
    // Distance to the nearest front, which turns an infinite sum of Gaussians
    // into a single exponential.
    let offset = travel.rem_euclid(spacing) - spacing * 0.5;
    (-(offset * offset) / (2.0 * width * width)).exp()
}

/// Upper bound on the unnormalised curl sum, so `turbulence` reads in m/s.
const CURL_NORMALISATION: f32 = {
    let mut total = 0.0;
    let mut i = 0;
    while i < CURL_OCTAVES.len() {
        let (kx, ky, _, amplitude) = CURL_OCTAVES[i];
        // No sqrt in const context on stable, so bound the vector length by its
        // larger component times root two. Erring high only makes turbulence
        // slightly conservative, which is the safe direction.
        let x = if kx < 0.0 { -kx } else { kx };
        let y = if ky < 0.0 { -ky } else { ky };
        let biggest = if x > y { x } else { y };
        total += amplitude * biggest * 1.4143;
        i += 1;
    }
    total
};

fn normalize_or(v: Vec2, fallback: Vec2) -> Vec2 {
    let length = v.length();
    if length > 1e-6 { v / length } else { fallback }
}

/// Advance the wind clock.
pub fn advance_wind(time: Res<Time>, mut wind: ResMut<WindField>) {
    // Wrapped at an hour rather than at TAU. The gust trains are periodic in
    // distance, not in phase, so wrapping at TAU would teleport the fronts;
    // wrapping far out keeps f32 precision adequate with no visible jump.
    wind.time = (wind.time + time.delta_secs()) % 3600.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> WindField {
        WindField::default()
    }

    #[test]
    fn sampling_is_pure() {
        let wind = field();
        let p = Vec2::new(3.0, 4.0);
        assert_eq!(wind.velocity_at(p), wind.velocity_at(p));
    }

    #[test]
    fn wind_varies_across_space() {
        // Uniform wind makes the whole field lean as one, which reads as a
        // sliding texture rather than as moving grass.
        let wind = field();
        let samples: Vec<Vec2> = (0..12)
            .map(|i| wind.velocity_at(Vec2::new(i as f32 * 2.7, i as f32 * -1.3)))
            .collect();
        let first = samples[0];
        assert!(
            samples.iter().any(|v| v.distance(first) > 0.3),
            "wind must not be spatially uniform"
        );
    }

    #[test]
    fn wind_varies_over_time() {
        let mut wind = field();
        let p = Vec2::new(1.0, 1.0);
        let before = wind.velocity_at(p);
        wind.time = 2.3;
        assert!(wind.velocity_at(p).distance(before) > 0.1);
    }

    #[test]
    fn turbulence_is_divergence_free() {
        // The property that makes curl noise worth the trouble: with sources or
        // sinks, grass would visibly converge on or flee from fixed points.
        let wind = WindField {
            speed: 0.0,
            gust_strength: 0.0,
            ..field()
        };
        let h = 0.01;
        for probe in [
            Vec2::new(0.0, 0.0),
            Vec2::new(5.5, -2.5),
            Vec2::new(-9.0, 7.0),
        ] {
            let dx = (wind.velocity_at(probe + Vec2::new(h, 0.0)).x
                - wind.velocity_at(probe - Vec2::new(h, 0.0)).x)
                / (2.0 * h);
            let dy = (wind.velocity_at(probe + Vec2::new(0.0, h)).y
                - wind.velocity_at(probe - Vec2::new(0.0, h)).y)
                / (2.0 * h);
            assert!(
                (dx + dy).abs() < 1e-2,
                "divergence at {probe:?} was {}",
                dx + dy
            );
        }
    }

    #[test]
    fn gusts_travel_downwind() {
        // A front sampled now should reappear further downwind a second later.
        let wind = WindField {
            speed: 0.0,
            turbulence: 0.0,
            ..field()
        };
        let direction = wind.direction.normalize();
        let now = wind.velocity_at(Vec2::ZERO).length();

        let later = WindField { time: 1.0, ..wind };
        let moved = later.velocity_at(direction * wind.gust_speed).length();
        assert!(
            (now - moved).abs() < 0.2,
            "front should have travelled gust_speed metres: {now} vs {moved}"
        );
    }

    #[test]
    fn lean_saturates_rather_than_growing_without_bound() {
        // Doubling a gale must not double the lean, or every blade pins to the
        // angular cap at once and the field moves like one rigid sheet.
        let wind = field();
        let lean_of = |speed: f32| {
            wind.lean_target(Vec2::X * speed, 0.24, 1.0, Vec2::ZERO)
                .target
                .length()
        };
        let moderate = lean_of(4.0);
        let strong = lean_of(8.0);

        assert!(strong > moderate, "more wind should still mean more lean");
        assert!(
            strong - moderate < moderate,
            "but with diminishing returns: {moderate} -> {strong}"
        );
        assert!(lean_of(40.0) <= MAX_WIND_ANGLE);
    }

    #[test]
    fn a_moderate_breeze_gives_a_believable_lean() {
        // Roughly twenty degrees at a walking-pace breeze. This is the number
        // that decides whether a calm day looks calm.
        let wind = field();
        let lean = wind
            .lean_target(Vec2::X * 3.0, 0.24, 1.0, Vec2::ZERO)
            .target
            .length()
            .to_degrees();
        assert!((12.0..32.0).contains(&lean), "{lean} degrees");
    }

    #[test]
    fn an_already_flattened_blade_catches_less_wind() {
        // Reconfiguration. Without it the response grows without limit and
        // grass laid flat keeps accelerating downwind.
        let wind = field();
        let flow = Vec2::X * 6.0;
        let upright = wind
            .lean_target(flow, 0.24, 1.0, Vec2::ZERO)
            .target
            .length();
        let laid_over = wind
            .lean_target(flow, 0.24, 1.0, Vec2::X * 1.4)
            .target
            .length();
        assert!(
            laid_over < upright,
            "aligned blade should catch less: {laid_over} vs {upright}"
        );
    }

    #[test]
    fn stiffer_grass_leans_less() {
        let wind = field();
        let flow = Vec2::X * 5.0;
        let soft = wind
            .lean_target(flow, 0.24, 0.5, Vec2::ZERO)
            .target
            .length();
        let stiff = wind
            .lean_target(flow, 0.24, 2.0, Vec2::ZERO)
            .target
            .length();
        assert!(stiff < soft);
    }

    #[test]
    fn longer_grass_leans_more() {
        let wind = field();
        let flow = Vec2::X * 5.0;
        let short = wind
            .lean_target(flow, 0.12, 1.0, Vec2::ZERO)
            .target
            .length();
        let tall = wind
            .lean_target(flow, 0.35, 1.0, Vec2::ZERO)
            .target
            .length();
        assert!(tall > short);
    }

    #[test]
    fn still_air_asks_for_nothing() {
        let wind = field();
        let response = wind.lean_target(Vec2::ZERO, 0.24, 1.0, Vec2::ZERO);
        assert_eq!(response.target, Vec2::ZERO);
        assert_eq!(response.strength, 0.0);
    }

    #[test]
    fn degenerate_settings_do_not_divide_by_zero() {
        let wind = WindField {
            direction: Vec2::ZERO,
            gust_spacing: 0.0,
            gust_width: 0.0,
            ..field()
        };
        assert!(wind.velocity_at(Vec2::new(1.0, 1.0)).is_finite());
        let response = wind.lean_target(Vec2::X, 0.0, 0.0, Vec2::ZERO);
        assert!(response.target.is_finite());
    }
}

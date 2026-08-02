//! Trampling and blasts.
//!
//! Everything that pushes grass around, turned into contact samples the
//! [field](crate::field) can integrate. Two shapes cover the cases that matter:
//! a swept capsule for anything that moves through the grass, and an expanding
//! ring for anything that goes off in it.
//!
//! ## Why the capsule is swept
//!
//! Stamping a body at its current position leaves gaps. A unit crossing at four
//! metres per second covers sixty-six millimetres per step, which is half a
//! cell, and a sprinting one covers several — so the trail comes out as a line
//! of dots rather than a track. Stamping the *segment* from the previous
//! position to the current one costs the same and cannot skip ground, however
//! fast the thing is going.
//!
//! ## Why a blast is an impulse, not a target
//!
//! A contact target says "be bent this far while I am here". That is right for
//! a foot resting on grass and wrong for an explosion, which is over long
//! before the next step. An explosion instead *kicks* the angular velocity and
//! then lets go, so the grass flies outward, overshoots, and springs back under
//! its own dynamics. The ring also lays down compaction and a radial axis
//! behind itself, which is what leaves the scorched-looking star pattern.

use bevy::prelude::*;

use crate::field::GrassField;
use crate::noise::fbm;

/// Bend a firm contact asks for, in radians. Not quite flat — a body pushes
/// grass aside as much as it presses it down.
const MAX_CONTACT_ANGLE: f32 = 80.0 * std::f32::consts::PI / 180.0;

/// Sharpness of a footprint's edge. Above one, the influence falls off quickly
/// and the print keeps a recognisable shape instead of blurring into a halo.
const FOOTPRINT_POWER: f32 = 1.6;

/// Speed at which a body's push is fully aligned with its travel, in m/s.
///
/// Below it the push turns radial, which is what a body standing still does:
/// splay the grass outward rather than sweep it in a direction it is not going.
const VELOCITY_BLEND_SPEED: f32 = 1.2;

/// How much of a moving body's push follows its travel, at full speed.
const VELOCITY_SHARE: f32 = 0.7;

/// Contact area pressure that counts as heavy, in pascals-ish. Arbitrary units
/// chosen so a footfall lands near one; only ratios matter.
const REFERENCE_PRESSURE: f32 = 9_000.0;

/// Speed that counts as fast, in m/s.
const REFERENCE_SPEED: f32 = 4.0;

/// Severity accrued merely by being touched, before mass or speed.
const BASE_SEVERITY: f32 = 0.35;
const PRESSURE_SEVERITY: f32 = 1.1;
const VELOCITY_SEVERITY: f32 = 0.55;

/// Something that pushes grass as it moves.
///
/// Carries its own previous position rather than reading a velocity component,
/// so that anything at all — a unit, a projectile, a rolling boulder, a cursor
/// — can disturb grass by writing two positions.
#[derive(Component, Clone, Copy, Debug)]
pub struct GrassInteractor {
    /// Where the body was at the previous stamp.
    pub previous: Vec2,
    /// Where it is now.
    pub current: Vec2,
    /// Radius of full influence, in metres.
    pub radius: f32,
    /// Distance over which influence fades to nothing beyond `radius`.
    pub falloff: f32,
    /// Mass in kilograms. Drives how much lasting damage is done, not how far
    /// the grass bends — even something light physically displaces a blade it
    /// walks through.
    pub mass: f32,
}

impl Default for GrassInteractor {
    fn default() -> Self {
        Self {
            previous: Vec2::ZERO,
            current: Vec2::ZERO,
            radius: 0.32,
            falloff: 0.28,
            mass: 70.0,
        }
    }
}

impl GrassInteractor {
    /// Move the body, keeping the segment it swept.
    pub fn move_to(&mut self, position: Vec2) {
        self.previous = self.current;
        self.current = position;
    }
}

/// An expanding ring of force.
#[derive(Clone, Copy, Debug)]
pub struct Shockwave {
    pub origin: Vec2,
    /// Seconds since it went off.
    pub age: f32,
    /// How fast the front travels, in m/s.
    pub speed: f32,
    /// Standard deviation of the front, in metres.
    pub width: f32,
    /// Peak angular kick at the front.
    pub strength: f32,
    /// Where the ring gives out, in metres.
    pub max_radius: f32,
    /// Seed for the raggedness and swirl of this particular blast.
    ///
    /// Per-blast rather than global, so two blasts in the same place do not
    /// produce the same shape.
    pub seed: u32,
}

/// How far the front's radius wanders, as a fraction of it.
const RAGGEDNESS: f32 = 0.30;

/// Lobes around the ring. Low, so the front undulates rather than crinkles.
const RAGGED_LOBES: f32 = 2.4;

/// How far the kick twists away from straight out, in radians.
const SWIRL: f32 = 1.5;

/// Cycles per metre of the field that does the twisting.
const SWIRL_METRES: f32 = 0.55;

impl Default for Shockwave {
    fn default() -> Self {
        Self {
            origin: Vec2::ZERO,
            age: 0.0,
            seed: 0x51A5_5EED,
            // Comfortably faster than a person, slow enough to watch cross the
            // field. Much quicker and it is a flicker rather than a wave.
            speed: 7.0,
            width: 0.75,
            strength: 62.0,
            max_radius: 11.0,
        }
    }
}

impl Shockwave {
    /// Radius of the front right now.
    pub fn radius(&self) -> f32 {
        self.age * self.speed
    }

    /// Whether the ring has run its course.
    pub fn finished(&self) -> bool {
        self.radius() > self.max_radius
    }

    /// How much punch is left.
    ///
    /// Falls off with radius because the same energy is spread around an
    /// ever-longer circumference — the reason a real blast weakens with
    /// distance rather than stopping at a boundary.
    pub fn intensity(&self) -> f32 {
        let radius = self.radius();
        if radius >= self.max_radius {
            return 0.0;
        }
        // Geometric spreading, but gently. The physically honest `1/r` leaves a
        // blast invisible within a couple of metres of going off, which reads
        // as the effect failing rather than as distance.
        let spread = 1.0 / (1.0 + radius * 0.16);
        let fade = 1.0 - (radius / self.max_radius).powi(2);
        spread * fade.max(0.0)
    }
}

/// Blasts currently crossing the field.
#[derive(Resource, Default, Debug)]
pub struct GrassEvents {
    shockwaves: Vec<Shockwave>,
}

impl GrassEvents {
    /// Set off a blast at a world position.
    pub fn shockwave(&mut self, origin: Vec2) {
        // Seeded from where it went off, so two blasts in different places look
        // different and one replayed in the same place looks the same.
        let seed = crate::noise::hash_2d(
            (origin.x * 64.0) as i32,
            (origin.y * 64.0) as i32,
            0x9E37_79B9,
        );
        self.shockwaves.push(Shockwave {
            origin,
            seed,
            ..Default::default()
        });
    }

    pub fn push(&mut self, wave: Shockwave) {
        self.shockwaves.push(wave);
    }

    pub fn active(&self) -> &[Shockwave] {
        &self.shockwaves
    }

    pub fn len(&self) -> usize {
        self.shockwaves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shockwaves.is_empty()
    }

    /// Age every blast and drop the spent ones.
    pub fn advance(&mut self, delta_seconds: f32) {
        for wave in &mut self.shockwaves {
            wave.age += delta_seconds;
        }
        self.shockwaves.retain(|wave| !wave.finished());
    }
}

/// Stamp a body's swept capsule into the field.
pub fn stamp_interactor(field: &mut GrassField, body: &GrassInteractor, dt: f32) {
    let reach = body.radius + body.falloff;
    let min = body.previous.min(body.current) - Vec2::splat(reach);
    let max = body.previous.max(body.current) + Vec2::splat(reach);
    let Some((x0, y0, x1, y1)) = field.cell_range(min, max) else {
        return;
    };

    let travel = body.current - body.previous;
    let distance = travel.length();
    let speed = if dt > 0.0 { distance / dt } else { 0.0 };
    let heading = if distance > 1e-6 {
        travel / distance
    } else {
        Vec2::ZERO
    };
    let velocity_share = VELOCITY_SHARE * (speed / VELOCITY_BLEND_SPEED).clamp(0.0, 1.0);

    let area = std::f32::consts::PI * body.radius * body.radius;
    let pressure = body.mass * 9.81 / (area * REFERENCE_PRESSURE).max(1e-6);
    let relative_speed = speed / REFERENCE_SPEED;

    for y in y0..=y1 {
        for x in x0..=x1 {
            let point = field.cell_center(x, y);
            let closest = closest_point_on_segment(point, body.previous, body.current);
            let offset = point - closest;
            let range = offset.length();

            let weight = falloff_weight(range, body.radius, body.falloff);
            if weight <= 0.0 {
                continue;
            }

            let outward = if range > 1e-6 {
                offset / range
            } else {
                // Directly under the body. Push along travel if it is moving,
                // and pick a stable arbitrary direction if it is not, rather
                // than producing a zero direction that would poison the axis
                // accumulator with a meaningless value.
                if heading == Vec2::ZERO {
                    Vec2::X
                } else {
                    heading
                }
            };
            let direction = (outward.lerp(heading, velocity_share))
                .try_normalize()
                .unwrap_or(outward);

            let severity = weight
                * (BASE_SEVERITY
                    + PRESSURE_SEVERITY * pressure
                    + VELOCITY_SEVERITY * relative_speed * relative_speed);

            field.accumulate_contact(
                x,
                y,
                direction * (MAX_CONTACT_ANGLE * weight),
                direction,
                weight,
                severity,
            );
        }
    }
}

/// Stamp an expanding ring into the field.
pub fn stamp_shockwave(field: &mut GrassField, wave: &Shockwave) {
    let intensity = wave.intensity();
    if intensity <= 1e-4 {
        return;
    }
    let radius = wave.radius();
    // Three standard deviations captures essentially all of the front, and
    // bounding the stamp this way keeps the cost proportional to the ring's
    // circumference rather than to the disc it has swept.
    let reach = radius + wave.width * 3.0;
    let Some((x0, y0, x1, y1)) = field.cell_range(
        wave.origin - Vec2::splat(reach),
        wave.origin + Vec2::splat(reach),
    ) else {
        return;
    };

    let inner = (radius - wave.width * 3.0).max(0.0);
    let variance = 2.0 * wave.width * wave.width;

    for y in y0..=y1 {
        for x in x0..=x1 {
            let point = field.cell_center(x, y);
            let offset = point - wave.origin;
            let range = offset.length();
            if range < inner {
                continue;
            }

            // The front is ragged, not a circle.
            //
            // A perfectly circular ring reads as a drop landing in water, which
            // is the one thing a blast in grass should not look like. Real
            // fronts break up: they run further where the grass is thin and
            // stall where it is thick. Perturbing the radius by a smooth
            // angular field costs one noise sample and turns the ring into
            // something that happened *in* the grass.
            let angle = offset.y.atan2(offset.x);
            let ragged = radius
                * (1.0
                    + RAGGEDNESS
                        * (fbm(
                            angle.cos() * RAGGED_LOBES,
                            angle.sin() * RAGGED_LOBES,
                            wave.seed,
                            2,
                        ) - 0.5));

            let front = range - ragged;
            let envelope = (-(front * front) / variance).exp();
            if envelope <= 1e-3 {
                continue;
            }

            let outward = if range > 1e-6 {
                offset / range
            } else {
                Vec2::X
            };
            // Not purely outward. A blast throws grass out *and* stirs it, so
            // the kick is swirled by the same angular field — without this the
            // whole ring lies down in perfect radial symmetry, which is the
            // other half of the water-drop look.
            let swirl = SWIRL
                * (fbm(
                    point.x * SWIRL_METRES,
                    point.y * SWIRL_METRES,
                    wave.seed ^ 0x51DE_9A7C,
                    2,
                ) - 0.5);
            let (sin, cos) = swirl.sin_cos();
            let outward = Vec2::new(
                outward.x * cos - outward.y * sin,
                outward.x * sin + outward.y * cos,
            );

            let push = envelope * intensity;

            // The kick, which throws the grass outward.
            field.add_impulse(x, y, outward * (wave.strength * push));
            // And the mark it leaves: a radial axis, which is what makes the
            // aftermath read as a star rather than a smudge.
            let weight = (push * 0.85).min(1.0);
            field.accumulate_contact(
                x,
                y,
                outward * (MAX_CONTACT_ANGLE * weight),
                outward,
                weight,
                weight * 4.5,
            );
        }
    }
}

/// Influence of a capsule at a given distance from its axis.
fn falloff_weight(distance: f32, radius: f32, falloff: f32) -> f32 {
    if distance <= radius {
        return 1.0;
    }
    if falloff <= f32::EPSILON || distance >= radius + falloff {
        return 0.0;
    }
    let t = (distance - radius) / falloff;
    (1.0 - crate::noise::smoothstep(t)).powf(FOOTPRINT_POWER)
}

fn closest_point_on_segment(point: Vec2, from: Vec2, to: Vec2) -> Vec2 {
    let along = to - from;
    let length_squared = along.length_squared();
    if length_squared <= 1e-12 {
        return from;
    }
    let t = ((point - from).dot(along) / length_squared).clamp(0.0, 1.0);
    from + along * t
}

/// Age the blasts each frame.
pub fn advance_events(time: Res<Time>, mut events: ResMut<GrassEvents>) {
    events.advance(time.delta_secs());
}

/// Write every disturbance into the field, before it steps.
pub fn stamp_disturbances(
    time: Res<Time>,
    events: Res<GrassEvents>,
    bodies: Query<&GrassInteractor>,
    mut field: ResMut<GrassField>,
) {
    let dt = time.delta_secs().max(1e-4);
    for body in &bodies {
        stamp_interactor(&mut field, body, dt);
    }
    for wave in events.active() {
        stamp_shockwave(&mut field, wave);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wind::WindField;

    fn still_field() -> GrassField {
        let mut field = GrassField::new(64, 0.15, 1);
        field.make_uniform(0.24, 1.0);
        field
    }

    fn calm() -> WindField {
        WindField {
            speed: 0.0,
            turbulence: 0.0,
            gust_strength: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn a_body_bends_the_grass_it_stands_in() {
        let mut field = still_field();
        let body = GrassInteractor {
            previous: Vec2::ZERO,
            current: Vec2::ZERO,
            ..Default::default()
        };
        for _ in 0..10 {
            stamp_interactor(&mut field, &body, 1.0 / 60.0);
            field.step(1.0 / 60.0, &calm());
        }
        // Sampled off the exact centre. Directly beneath a stationary body the
        // grass is splayed outward in every direction at once, so its *average*
        // direction is genuinely zero — the grass is flattened, not upright,
        // and asserting on the mean bend there would be measuring a
        // cancellation rather than a lack of force. `dose` is what says the
        // centre was touched.
        assert!(
            field.dose_at(Vec2::ZERO) > 0.0,
            "the centre must register contact"
        );
        assert!(
            field.bend_at(Vec2::new(0.2, 0.0)).length() > 0.3,
            "grass under a body should be well bent, was {}",
            field.bend_at(Vec2::new(0.2, 0.0)).length()
        );
    }

    #[test]
    fn grass_well_clear_of_a_body_is_untouched() {
        let mut field = still_field();
        let body = GrassInteractor::default();
        for _ in 0..10 {
            stamp_interactor(&mut field, &body, 1.0 / 60.0);
            field.step(1.0 / 60.0, &calm());
        }
        assert!(field.bend_at(Vec2::new(3.0, 3.0)).length() < 0.01);
    }

    #[test]
    fn a_fast_body_leaves_a_continuous_trail() {
        // The reason the capsule is swept. At this speed the body moves several
        // cells per step, and stamping only its current position would leave
        // untouched gaps between the prints.
        let mut field = still_field();
        let dt = 1.0 / 60.0;
        let mut body = GrassInteractor {
            previous: Vec2::new(-2.0, 0.0),
            current: Vec2::new(-2.0, 0.0),
            ..Default::default()
        };
        for step in 0..40 {
            let x = -2.0 + step as f32 * 0.1;
            body.move_to(Vec2::new(x, 0.0));
            stamp_interactor(&mut field, &body, dt);
            field.step(dt, &calm());
        }

        // Sample along the middle of the path; every point should have
        // registered contact. Measured on dose rather than compaction: dose
        // responds the instant a cell is touched, whereas compaction is
        // deliberately slow, so a gap in compaction after one quick pass would
        // mean "not crushed yet" rather than "never touched".
        let mut untouched = 0;
        for i in 0..30 {
            let x = -1.9 + i as f32 * 0.05;
            if field.dose_at(Vec2::new(x, 0.0)) <= 0.0 {
                untouched += 1;
            }
        }
        assert_eq!(untouched, 0, "{untouched} gaps along a swept path");
    }

    #[test]
    fn a_moving_body_pushes_grass_along_its_travel() {
        let mut field = still_field();
        let dt = 1.0 / 60.0;
        let mut body = GrassInteractor {
            previous: Vec2::new(-1.0, 0.0),
            current: Vec2::new(-1.0, 0.0),
            ..Default::default()
        };
        for step in 0..20 {
            body.move_to(Vec2::new(-1.0 + step as f32 * 0.08, 0.0));
            stamp_interactor(&mut field, &body, dt);
            field.step(dt, &calm());
        }
        // Grass just behind the body should lean the way the body went.
        let bend = field.bend_at(body.current - Vec2::new(0.15, 0.0));
        assert!(bend.x > 0.0, "expected an eastward lean, got {bend:?}");
    }

    #[test]
    fn a_stationary_body_splays_grass_outward() {
        let mut field = still_field();
        let body = GrassInteractor {
            radius: 0.4,
            ..Default::default()
        };
        for _ in 0..20 {
            stamp_interactor(&mut field, &body, 1.0 / 60.0);
            field.step(1.0 / 60.0, &calm());
        }
        // Opposite sides should lean in opposite directions.
        let east = field.bend_at(Vec2::new(0.45, 0.0));
        let west = field.bend_at(Vec2::new(-0.45, 0.0));
        assert!(east.x > 0.0 && west.x < 0.0, "{east:?} / {west:?}");
    }

    #[test]
    fn a_heavier_body_leaves_a_deeper_mark() {
        let run = |mass: f32| {
            let mut field = still_field();
            let body = GrassInteractor {
                mass,
                ..Default::default()
            };
            for _ in 0..60 {
                stamp_interactor(&mut field, &body, 1.0 / 60.0);
                field.step(1.0 / 60.0, &calm());
            }
            field.compaction_at(Vec2::ZERO)
        };
        assert!(run(400.0) > run(40.0));
    }

    #[test]
    fn a_shockwave_front_moves_outward_over_time() {
        let mut wave = Shockwave::default();
        let early = wave.radius();
        wave.age = 0.5;
        assert!(wave.radius() > early);
        assert!((wave.radius() - 0.5 * wave.speed).abs() < 1e-5);
    }

    #[test]
    fn a_shockwave_weakens_as_it_spreads() {
        let near = Shockwave {
            age: 0.1,
            ..Default::default()
        };
        let far = Shockwave {
            age: 0.9,
            ..Default::default()
        };
        assert!(far.intensity() < near.intensity());
    }

    #[test]
    fn a_shockwave_expires() {
        let wave = Shockwave {
            age: 100.0,
            ..Default::default()
        };
        assert!(wave.finished());
        assert_eq!(wave.intensity(), 0.0);
    }

    #[test]
    fn a_shockwave_throws_grass_radially_outward() {
        let mut field = still_field();
        let dt = 1.0 / 60.0;
        let mut wave = Shockwave {
            origin: Vec2::ZERO,
            width: 0.4,
            ..Default::default()
        };
        // Step until the front has passed a probe a metre out.
        for _ in 0..20 {
            stamp_shockwave(&mut field, &wave);
            field.step(dt, &calm());
            wave.age += dt;
        }

        let east = field.bend_at(Vec2::new(1.0, 0.0));
        let north = field.bend_at(Vec2::new(0.0, 1.0));
        assert!(east.x > 0.05, "east probe should lean east: {east:?}");
        assert!(north.y > 0.05, "north probe should lean north: {north:?}");
    }

    #[test]
    fn a_shockwave_has_no_directional_bias() {
        // The blast is deliberately *not* rotationally symmetric — a perfect
        // ring reads as a drop landing in water, which is the one thing an
        // explosion in grass must not look like. So the property worth guarding
        // is weaker and more useful: the raggedness must be *unbiased*. It may
        // run further in one place than another, but averaged around the ring
        // it must not favour a direction, because a systematic bias is the
        // giveaway that something is being simulated in projected space rather
        // than in the world.
        let mut field = still_field();
        let dt = 1.0 / 60.0;
        let mut wave = Shockwave {
            width: 0.4,
            ..Default::default()
        };
        for _ in 0..24 {
            stamp_shockwave(&mut field, &wave);
            field.step(dt, &calm());
            wave.age += dt;
        }

        // Sampled densely around the ring rather than at four points, so local
        // raggedness averages out and only a real bias survives.
        let samples = 64;
        let mut sum = Vec2::ZERO;
        let mut total = 0.0;
        for i in 0..samples {
            let angle = i as f32 / samples as f32 * std::f32::consts::TAU;
            let at = Vec2::new(angle.cos(), angle.sin()) * 1.2;
            let magnitude = field.bend_at(at).length();
            sum += Vec2::new(angle.cos(), angle.sin()) * magnitude;
            total += magnitude;
        }

        assert!(
            total / samples as f32 > 0.02,
            "the blast should have reached the ring"
        );
        // The magnitude-weighted centroid of the ring should sit on the origin.
        let bias = sum.length() / total;
        assert!(bias < 0.12, "the blast leans one way: bias {bias}");
    }

    #[test]
    fn a_shockwave_front_is_ragged() {
        // The other half of the same decision. If this ever measures perfectly
        // even, the raggedness has been tuned or refactored away and the blast
        // is a water drop again.
        let mut field = still_field();
        let dt = 1.0 / 60.0;
        let mut wave = Shockwave {
            width: 0.4,
            ..Default::default()
        };
        for _ in 0..24 {
            stamp_shockwave(&mut field, &wave);
            field.step(dt, &calm());
            wave.age += dt;
        }

        let samples = 64;
        let magnitudes: Vec<f32> = (0..samples)
            .map(|i| {
                let angle = i as f32 / samples as f32 * std::f32::consts::TAU;
                field
                    .bend_at(Vec2::new(angle.cos(), angle.sin()) * 1.2)
                    .length()
            })
            .collect();
        let mean = magnitudes.iter().sum::<f32>() / samples as f32;
        let spread = (magnitudes
            .iter()
            .map(|m| (m - mean) * (m - mean))
            .sum::<f32>()
            / samples as f32)
            .sqrt();
        assert!(
            spread / mean.max(1e-6) > 0.05,
            "the front is perfectly even"
        );
    }

    #[test]
    fn events_expire_on_their_own() {
        let mut events = GrassEvents::default();
        events.shockwave(Vec2::ZERO);
        assert_eq!(events.len(), 1);
        events.advance(10.0);
        assert!(events.is_empty());
    }

    #[test]
    fn a_body_outside_the_field_is_ignored_not_a_panic() {
        let mut field = still_field();
        let body = GrassInteractor {
            previous: Vec2::splat(9_999.0),
            current: Vec2::splat(9_999.0),
            ..Default::default()
        };
        stamp_interactor(&mut field, &body, 1.0 / 60.0);
        stamp_shockwave(
            &mut field,
            &Shockwave {
                origin: Vec2::splat(-9_999.0),
                age: 0.2,
                ..Default::default()
            },
        );
        assert_eq!(field.max_bend(), 0.0);
    }

    #[test]
    fn the_capsule_falloff_is_monotonic() {
        let mut previous = f32::MAX;
        for i in 0..40 {
            let distance = i as f32 * 0.03;
            let weight = falloff_weight(distance, 0.3, 0.3);
            assert!(weight <= previous + 1e-6, "weight rose at {distance}");
            previous = weight;
        }
        assert_eq!(falloff_weight(1.0, 0.3, 0.3), 0.0);
        assert_eq!(falloff_weight(0.0, 0.3, 0.3), 1.0);
    }

    #[test]
    fn closest_point_handles_a_degenerate_segment() {
        let at = Vec2::new(2.0, 2.0);
        assert_eq!(closest_point_on_segment(Vec2::ZERO, at, at), at);
    }
}

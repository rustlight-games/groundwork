//! Trampling.
//!
//! A low-resolution field that units write into as they move and that grass
//! reads to bend away from them. Low resolution on purpose: it is uploaded
//! every frame, and grass bending is a soft effect where a coarse field is
//! indistinguishable from a fine one.
//!
//! Decays over time so an army leaves a wake that recovers behind it, which is
//! most of what sells the effect.

use bevy::prelude::*;

/// Cells per edge of the disturbance field.
pub const DISTURBANCE_RESOLUTION: usize = 64;

/// How much of the disturbance remains after one second.
const DECAY_PER_SECOND: f32 = 0.4;

/// Where the grass is currently pushed down.
#[derive(Resource, Clone, Debug)]
pub struct DisturbanceMap {
    values: Vec<f32>,
    /// World-space extent the field covers, centred on the origin.
    pub world_size: f32,
}

impl Default for DisturbanceMap {
    fn default() -> Self {
        Self {
            values: vec![0.0; DISTURBANCE_RESOLUTION * DISTURBANCE_RESOLUTION],
            world_size: 128.0,
        }
    }
}

impl DisturbanceMap {
    fn index_of(&self, position: Vec2) -> Option<usize> {
        let half = self.world_size * 0.5;
        if position.x < -half || position.x >= half || position.y < -half || position.y >= half {
            return None;
        }
        let scale = DISTURBANCE_RESOLUTION as f32 / self.world_size;
        let x = ((position.x + half) * scale) as usize;
        let y = ((position.y + half) * scale) as usize;
        Some(
            y.min(DISTURBANCE_RESOLUTION - 1) * DISTURBANCE_RESOLUTION
                + x.min(DISTURBANCE_RESOLUTION - 1),
        )
    }

    /// Record a unit standing at `position`.
    ///
    /// Saturating rather than accumulating: a tight formation would otherwise
    /// drive one cell far past the visual maximum and take much longer to
    /// recover than the grass around it.
    pub fn disturb(&mut self, position: Vec2, amount: f32) {
        if let Some(i) = self.index_of(position) {
            self.values[i] = (self.values[i] + amount).clamp(0.0, 1.0);
        }
    }

    pub fn sample(&self, position: Vec2) -> f32 {
        self.index_of(position).map_or(0.0, |i| self.values[i])
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Fade everything toward zero.
    pub fn decay(&mut self, delta_seconds: f32) {
        let retained = DECAY_PER_SECOND.powf(delta_seconds.max(0.0));
        for value in &mut self.values {
            *value *= retained;
            if *value < 0.001 {
                *value = 0.0;
            }
        }
    }
}

pub fn decay_disturbance(time: Res<Time>, mut map: ResMut<DisturbanceMap>) {
    map.decay(time.delta_secs());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disturbing_shows_up_when_sampled() {
        let mut map = DisturbanceMap::default();
        map.disturb(Vec2::new(4.0, 4.0), 0.8);
        assert!(map.sample(Vec2::new(4.0, 4.0)) > 0.5);
        assert_eq!(map.sample(Vec2::new(-40.0, -40.0)), 0.0);
    }

    #[test]
    fn disturbance_saturates_rather_than_accumulating() {
        let mut map = DisturbanceMap::default();
        for _ in 0..50 {
            map.disturb(Vec2::ZERO, 0.5);
        }
        assert_eq!(map.sample(Vec2::ZERO), 1.0);
    }

    #[test]
    fn outside_the_field_is_ignored_not_a_panic() {
        let mut map = DisturbanceMap::default();
        map.disturb(Vec2::new(9_999.0, -9_999.0), 1.0);
        assert_eq!(map.sample(Vec2::new(9_999.0, -9_999.0)), 0.0);
    }

    #[test]
    fn decay_returns_grass_to_upright() {
        let mut map = DisturbanceMap::default();
        map.disturb(Vec2::ZERO, 1.0);
        map.decay(10.0);
        assert_eq!(map.sample(Vec2::ZERO), 0.0);
    }

    #[test]
    fn decay_is_gradual_rather_than_immediate() {
        let mut map = DisturbanceMap::default();
        map.disturb(Vec2::ZERO, 1.0);
        map.decay(0.1);
        let after = map.sample(Vec2::ZERO);
        assert!(after > 0.5 && after < 1.0, "{after}");
    }
}

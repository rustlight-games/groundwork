//! Wind.
//!
//! Stored as a direction, a strength and a phase, and sampled analytically in
//! the vertex shader rather than from a scrolling texture. Analytic wind costs
//! no bandwidth and no memory, tiles perfectly, and can be evaluated at any
//! position — which matters because blades are placed procedurally and do not
//! sit on a texel grid.

use bevy::prelude::*;

/// Global wind state.
#[derive(Resource, Clone, Copy, Debug)]
pub struct WindField {
    /// Normalised direction the wind blows toward.
    pub direction: Vec2,
    /// How far a blade tip displaces, in world units.
    pub strength: f32,
    /// Advances with time; the shader's input to its wave function.
    pub phase: f32,
    /// Spatial frequency of the gust pattern.
    pub wavelength: f32,
    /// How quickly the phase advances.
    pub speed: f32,
}

impl Default for WindField {
    fn default() -> Self {
        Self {
            direction: Vec2::new(1.0, 0.25).normalize(),
            strength: 0.35,
            phase: 0.0,
            wavelength: 6.0,
            speed: 1.2,
        }
    }
}

impl WindField {
    /// Blade displacement at a world position.
    ///
    /// A reference implementation of what the shader will do, kept in Rust so
    /// the behaviour can be tested and benchmarked without a GPU.
    pub fn sample(&self, position: Vec2) -> Vec2 {
        if self.wavelength <= f32::EPSILON {
            return Vec2::ZERO;
        }
        let along = position.dot(self.direction) / self.wavelength;
        // Two waves at different rates, so gusts do not visibly repeat.
        let wave = (along + self.phase).sin() * 0.7 + (along * 0.37 - self.phase * 1.3).sin() * 0.3;
        self.direction * (wave * self.strength)
    }
}

/// Advance the wind phase.
pub fn advance_wind(time: Res<Time>, mut wind: ResMut<WindField>) {
    let delta = time.delta_secs() * wind.speed;
    // Wrapped, so the phase cannot grow until f32 precision makes the motion
    // visibly judder after a long session.
    wind.phase = (wind.phase + delta) % std::f32::consts::TAU;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displacement_is_bounded_by_strength() {
        let wind = WindField::default();
        for i in 0..500 {
            let p = Vec2::new(i as f32 * 0.7, i as f32 * -0.3);
            assert!(wind.sample(p).length() <= wind.strength * 1.001);
        }
    }

    #[test]
    fn sampling_is_pure() {
        let wind = WindField::default();
        let p = Vec2::new(3.0, 4.0);
        assert_eq!(wind.sample(p), wind.sample(p));
    }

    #[test]
    fn wind_varies_across_space() {
        // Uniform wind would make the whole field lean as one, which reads as a
        // sliding texture rather than as moving grass.
        let wind = WindField::default();
        let a = wind.sample(Vec2::ZERO);
        let b = wind.sample(Vec2::new(wind.wavelength * 1.5, 0.0));
        assert_ne!(a, b);
    }

    #[test]
    fn a_degenerate_wavelength_does_not_divide_by_zero() {
        let wind = WindField {
            wavelength: 0.0,
            ..Default::default()
        };
        assert_eq!(wind.sample(Vec2::new(1.0, 1.0)), Vec2::ZERO);
    }
}

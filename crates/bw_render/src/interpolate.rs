//! Smoothing simulation ticks into frames.

use bevy::prelude::*;
use bw_core::Vec2Fx;

/// Where a unit was at the end of the previous tick.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PreviousPosition(pub Vec2Fx);

/// How far through the current tick the renderer is, 0..=1.
///
/// Owned by whoever drives the simulation, because only they know how much real
/// time has accumulated toward the next tick.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct RenderInterpolation {
    pub alpha: f32,
}

impl RenderInterpolation {
    /// Clamped, because a frame longer than a tick would otherwise extrapolate
    /// units past where they have actually been — which looks like a rubber-band
    /// snap the moment the simulation catches up.
    pub fn set(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }
}

pub struct InterpolationPlugin;

impl Plugin for InterpolationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderInterpolation>();
    }
}

/// Blend between two simulation positions for display.
pub fn interpolate(previous: Vec2Fx, current: Vec2Fx, alpha: f32) -> Vec2 {
    let [px, py] = previous.to_f32_array();
    let [cx, cy] = current.to_f32_array();
    let t = alpha.clamp(0.0, 1.0);
    Vec2::new(px + (cx - px) * t, py + (cy - py) * t)
}

/// Sprite depth from world position.
///
/// Lower on the screen draws in front, which is the convention for a 2D game
/// viewed from above and slightly behind. Scaled down hard so that y-ordering
/// never fights with explicit layer offsets.
pub fn depth_for(y: f32) -> f32 {
    -y * 0.001
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_zero_and_one_hit_the_endpoints() {
        let a = Vec2Fx::from_ints(0, 0);
        let b = Vec2Fx::from_ints(10, 20);
        assert_eq!(interpolate(a, b, 0.0), Vec2::new(0.0, 0.0));
        assert_eq!(interpolate(a, b, 1.0), Vec2::new(10.0, 20.0));
    }

    #[test]
    fn alpha_is_clamped_rather_than_extrapolating() {
        // An overlong frame must not fling units past their real position.
        let a = Vec2Fx::from_ints(0, 0);
        let b = Vec2Fx::from_ints(10, 0);
        assert_eq!(interpolate(a, b, 5.0), Vec2::new(10.0, 0.0));
        assert_eq!(interpolate(a, b, -3.0), Vec2::new(0.0, 0.0));
    }

    #[test]
    fn halfway_is_the_midpoint() {
        let mid = interpolate(Vec2Fx::from_ints(0, 0), Vec2Fx::from_ints(4, 8), 0.5);
        assert_eq!(mid, Vec2::new(2.0, 4.0));
    }

    #[test]
    fn lower_on_screen_draws_in_front() {
        assert!(depth_for(-5.0) > depth_for(5.0));
    }

    #[test]
    fn the_resource_clamps_on_set() {
        let mut interpolation = RenderInterpolation::default();
        interpolation.set(2.5);
        assert_eq!(interpolation.alpha, 1.0);
        interpolation.set(-1.0);
        assert_eq!(interpolation.alpha, 0.0);
    }
}

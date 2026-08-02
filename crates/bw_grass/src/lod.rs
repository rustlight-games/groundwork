//! Level of detail.

/// Detail tiers for a grass chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrassLod {
    /// Every blade, full animation.
    Full,
    /// Roughly a third of the blades.
    Reduced,
    /// A flat textured quad. No blades, no per-blade wind.
    Billboard,
    /// Not drawn.
    Culled,
}

impl GrassLod {
    /// Fraction of the full blade budget this tier draws.
    pub fn blade_fraction(self) -> f32 {
        match self {
            GrassLod::Full => 1.0,
            GrassLod::Reduced => 0.34,
            GrassLod::Billboard | GrassLod::Culled => 0.0,
        }
    }

    pub fn draws_blades(self) -> bool {
        self.blade_fraction() > 0.0
    }
}

/// Pick a tier from how far a chunk is from the camera.
///
/// Thresholds are in world units. They are deliberately generous — popping is
/// far more noticeable than a slightly higher blade count, and the cost of
/// being one tier too detailed is small.
pub fn lod_for_distance(distance: f32, view_height: f32) -> GrassLod {
    // Scaled by zoom, so zooming out does not push everything to Billboard and
    // flatten the whole field at once.
    let scale = (view_height / 40.0).max(0.25);
    match distance / scale {
        d if d < 45.0 => GrassLod::Full,
        d if d < 90.0 => GrassLod::Reduced,
        d if d < 200.0 => GrassLod::Billboard,
        _ => GrassLod::Culled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_falls_off_monotonically_with_distance() {
        let tiers: Vec<GrassLod> = [0.0, 50.0, 100.0, 500.0]
            .iter()
            .map(|&d| lod_for_distance(d, 40.0))
            .collect();
        assert_eq!(
            tiers,
            [
                GrassLod::Full,
                GrassLod::Reduced,
                GrassLod::Billboard,
                GrassLod::Culled
            ]
        );
        assert!(tiers.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn zooming_out_keeps_nearby_chunks_detailed() {
        // Without zoom scaling, zooming out would flatten the entire field in
        // one step, which is exactly when the player is looking at all of it.
        let close = 60.0;
        assert_eq!(lod_for_distance(close, 40.0), GrassLod::Reduced);
        assert_eq!(lod_for_distance(close, 160.0), GrassLod::Full);
    }

    #[test]
    fn blade_fractions_are_ordered_and_bounded() {
        assert_eq!(GrassLod::Full.blade_fraction(), 1.0);
        assert!(GrassLod::Reduced.blade_fraction() < 1.0);
        assert_eq!(GrassLod::Culled.blade_fraction(), 0.0);
        assert!(!GrassLod::Billboard.draws_blades());
        assert!(GrassLod::Full.draws_blades());
    }
}

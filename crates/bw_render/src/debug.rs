//! Debug overlays.
//!
//! Flow-field arrows, navigation costs, target lines. Behind a resource toggle
//! rather than a compile-time feature so it can be turned on in a running game
//! when something looks wrong, which is when you actually want it.

use bevy::prelude::*;

/// Which overlays are currently drawn.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct DebugDraw {
    pub flow_field: bool,
    pub nav_costs: bool,
    pub targets: bool,
    pub unit_radii: bool,
}

impl DebugDraw {
    pub fn any(&self) -> bool {
        self.flow_field || self.nav_costs || self.targets || self.unit_radii
    }
}

pub struct DebugDrawPlugin;

impl Plugin for DebugDrawPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugDraw>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_drawn_by_default() {
        assert!(!DebugDraw::default().any());
    }

    #[test]
    fn any_reports_a_single_enabled_overlay() {
        assert!(
            DebugDraw {
                targets: true,
                ..Default::default()
            }
            .any()
        );
    }
}

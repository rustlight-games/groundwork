//! Everything that grows on a page, as a value.
//!
//! This is the change the whole reference renderer is built on, and it is worth
//! being explicit about why a list of marks deserves its own module.
//!
//! The baker used to decide a blade existed and rasterise it in the same
//! statement. That is the right shape for one camera pass and the wrong shape
//! for everything that comes after it, because the same geometry now has to be
//! drawn more than once:
//!
//! - **From the sun**, into a light-space depth buffer, so blades can shadow the
//!   ground and each other.
//! - **From the camera**, into the surface, so they can be shaded.
//! - **Twice at different budgets**, so a cheap render and an expensive one can
//!   be paired as the input and the target of a neural renderer.
//!
//! Every one of those needs the second pass to see *exactly* what the first one
//! saw. Regenerating the scene would nearly work — placement is deterministic —
//! and "nearly" is the problem: it doubles the expensive half of the bake, and
//! it leaves a standing invitation for the two generators to drift apart after
//! some later edit, with the symptom being shadows that do not quite belong to
//! the blades casting them.
//!
//! So the scene is built once, held, and rendered.
//!
//! ## It is cheap to hold
//!
//! A page carries something like ten thousand marks and a mark is a hundred
//! bytes of parameters, so a scene is a megabyte or so — against a surface that
//! is thirty. Holding geometry is not what costs; holding *vertices* would be,
//! which is why a mark stays a description and is tessellated at raster time.

use crate::field::WorldField;
use crate::page::Page;
use crate::placement::{self, Bed};
use crate::stroke::Stroke;
use crate::style::GrassParams;

/// Every mark that can touch one page, in draw order.
pub struct GrassScene {
    /// The page this scene was grown for, including its scale.
    pub page: Page,
    /// The marks, in the order the depth compositor wants them.
    ///
    /// Order is not correctness here — the surface resolves by depth, not by
    /// arrival — but it is not arbitrary either. The mat goes down first because
    /// its job is to be *buried*, and a buried mark contributes occlusion where
    /// one that wins its pixel does not.
    pub marks: Vec<Stroke>,
}

impl GrassScene {
    /// Grow everything this page can show.
    pub fn build(page: Page, field: &WorldField, params: &GrassParams) -> Self {
        let mut marks = Vec::new();
        placement::plant(
            &mut marks,
            &Bed {
                page: &page,
                field,
                params,
            },
        );
        Self { page, marks }
    }

    /// How many marks the page holds.
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// The tallest any mark's root sits, in world metres.
    ///
    /// Roots only — a bound on the canopy needs the arc as well, and
    /// [`GrassScene::canopy_ceiling`] is the one that answers that question.
    pub fn highest_root(&self) -> f32 {
        self.marks.iter().fold(0.0f32, |high, m| high.max(m.root.z))
    }

    /// An upper bound on how high anything in this scene stands, world metres.
    ///
    /// Conservative: a mark cannot reach higher above its root than its own arc
    /// length, whatever it does on the way. The shadow pass needs this to size
    /// its light-space volume, and needs it to be a genuine bound rather than an
    /// estimate — a caster clipped out of the depth map is a shadow that simply
    /// is not there.
    pub fn canopy_ceiling(&self) -> f32 {
        self.marks
            .iter()
            .fold(0.0f32, |high, m| high.max(m.root.z + m.length.abs()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    #[test]
    fn a_scene_grows_something() {
        let page = Page::new(Vec2::new(-64.0, -64.0), 96, 96);
        let field = WorldField::lit_by(7, GrassParams::default().light);
        let scene = GrassScene::build(page, &field, &GrassParams::default());
        assert!(!scene.is_empty(), "a page grew nothing at all");
        // The mat alone is nearly four hundred marks to the square metre, and a
        // 96-pixel page at the authoring scale is a square metre of ground.
        assert!(scene.len() > 500, "only {} marks", scene.len());
    }

    #[test]
    fn the_same_page_grows_the_same_scene_twice() {
        // The property everything downstream leans on: the shadow pass and the
        // camera pass see one scene because a scene is built once, and the
        // paired low and high renders are two photographs of the same meadow.
        let page = Page::new(Vec2::new(128.0, -32.0), 64, 64);
        let params = GrassParams::default();
        let field = WorldField::lit_by(params.seed, params.light);
        let first = GrassScene::build(page, &field, &params);
        let second = GrassScene::build(page, &field, &params);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.marks.iter().zip(&second.marks) {
            assert_eq!(a.root.to_array(), b.root.to_array());
            assert_eq!(a.length.to_bits(), b.length.to_bits());
            assert_eq!(a.bend.to_bits(), b.bend.to_bits());
        }
    }

    #[test]
    fn the_canopy_ceiling_bounds_every_mark() {
        let page = Page::new(Vec2::ZERO, 96, 96);
        let params = GrassParams::default();
        let field = WorldField::lit_by(params.seed, params.light);
        let scene = GrassScene::build(page, &field, &params);
        let ceiling = scene.canopy_ceiling();
        assert!(ceiling > 0.0);
        for mark in &scene.marks {
            assert!(
                mark.root.z + mark.length.abs() <= ceiling + 1.0e-6,
                "a mark reaches past the ceiling the shadow volume is sized from"
            );
        }
    }
}

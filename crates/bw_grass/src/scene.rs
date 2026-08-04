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

use glam::{Vec2, Vec3};

use crate::bake::{BakeParams, Page};
use crate::field::WorldField;
use crate::placement::{self, Bed};
use crate::stroke::{Painter, Stroke};

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
    pub fn build(page: Page, field: &WorldField, params: &BakeParams) -> Self {
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

    /// Rasterise the scene into a surface through `painter`.
    pub fn draw(&self, painter: &mut Painter) {
        for mark in &self.marks {
            painter.draw(mark);
        }
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

/// Several output pages baked as one piece of ground.
///
/// The runtime still consumes 256-pixel pages, and nothing about that changes.
/// What changes is that the offline renderer stops baking them one at a time.
///
/// Three things want a region rather than a page. A cast shadow crosses page
/// boundaries, and a shadow map built per page has to guard for casters it will
/// then throw away — build it once over four pages and the guard is paid for
/// once instead of four times. Patch and tuft structure spans page edges, so a
/// training crop taken from a region carries genuine neighbourhood context
/// rather than context that stops at a border. And the fixed per-page costs —
/// the field, the lattice, the guard band's own area — amortise.
///
/// The region is baked as one large [`Page`] and cut afterwards, which is what
/// makes the output identical to a page bake wherever it can be: the world is
/// sampled on the same world-anchored lattice either way, and the only remaining
/// difference is that a neighbourhood read near a page edge is cropped in one
/// path and complete in the other.
#[derive(Clone, Copy, Debug)]
pub struct BakeRegion {
    /// Cache-pixel corner of the region's first page.
    pub origin: Vec2,
    /// Output pages across and down.
    pub pages: (usize, usize),
    /// Side of one output page, in cache pixels.
    pub page_pixels: usize,
    /// Cache pixels per world metre.
    pub px_per_metre: f32,
}

impl BakeRegion {
    /// A square region of `side × side` pages at the authoring scale.
    pub fn square(origin: Vec2, side: usize, page_pixels: usize) -> Self {
        Self {
            origin,
            pages: (side.max(1), side.max(1)),
            page_pixels,
            px_per_metre: crate::iso::PX_PER_METRE,
        }
    }

    /// The whole region as one page.
    pub fn whole(&self) -> Page {
        Page {
            origin: self.origin,
            width: self.pages.0 * self.page_pixels,
            height: self.pages.1 * self.page_pixels,
            px_per_metre: self.px_per_metre,
        }
    }

    /// One output page of the region.
    pub fn tile(&self, x: usize, y: usize) -> Page {
        Page {
            origin: self.origin
                + Vec2::new((x * self.page_pixels) as f32, (y * self.page_pixels) as f32),
            width: self.page_pixels,
            height: self.page_pixels,
            px_per_metre: self.px_per_metre,
        }
    }

    /// How many output pages the region holds.
    pub fn count(&self) -> usize {
        self.pages.0 * self.pages.1
    }

    /// Bake the whole region, correctly, and hand back the finished plate.
    ///
    /// Goes through [`crate::bake::bake_padded`], so every neighbourhood-reading
    /// shading term sees the ground that is actually there rather than whatever
    /// part of it fell inside the rectangle. The pad is a perimeter cost against
    /// an area of pages, which is the whole reason to bake a region rather than
    /// a page: one 256-pixel page padded for correctness costs three and a half
    /// times itself, a four-by-four region costs half again.
    ///
    /// Crop the result with [`BakeRegion::crop`] to get pages the runtime cache
    /// can hold.
    pub fn bake(&self, params: &crate::bake::BakeParams) -> Vec<Vec3> {
        crate::bake::bake_padded(self.whole(), params)
    }

    /// Cut one output page out of a finished region plate.
    pub fn crop(&self, plate: &[Vec3], x: usize, y: usize) -> Vec<Vec3> {
        let whole = self.whole();
        let side = self.page_pixels;
        let mut page = Vec::with_capacity(side * side);
        for row in 0..side {
            let start = (y * side + row) * whole.width + x * side;
            page.extend_from_slice(&plate[start..start + side]);
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_grows_something() {
        let page = Page::new(Vec2::new(-64.0, -64.0), 96, 96);
        let field = WorldField::lit_by(7, BakeParams::default().light);
        let scene = GrassScene::build(page, &field, &BakeParams::default());
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
        let params = BakeParams::default();
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
        let params = BakeParams::default();
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

    #[test]
    fn a_region_tiles_its_own_pages_exactly() {
        let region = BakeRegion::square(Vec2::new(-256.0, -256.0), 2, 128);
        let whole = region.whole();
        assert_eq!(whole.width, 256);
        assert_eq!(whole.height, 256);
        assert_eq!(region.count(), 4);
        // Every tile's corner has to land on the region grid, or a crop takes
        // the wrong pixels.
        for y in 0..region.pages.1 {
            for x in 0..region.pages.0 {
                let tile = region.tile(x, y);
                let offset = tile.origin - region.origin;
                assert_eq!(offset.x, (x * region.page_pixels) as f32);
                assert_eq!(offset.y, (y * region.page_pixels) as f32);
            }
        }
    }

    #[test]
    fn cropping_a_region_takes_the_right_pixels() {
        // A plate whose every pixel encodes its own coordinate, so a crop that
        // is off by a row or a column cannot pass.
        let region = BakeRegion::square(Vec2::ZERO, 2, 4);
        let whole = region.whole();
        let plate: Vec<Vec3> = (0..whole.width * whole.height)
            .map(|i| {
                let (x, y) = (i % whole.width, i / whole.width);
                Vec3::new(x as f32, y as f32, 0.0)
            })
            .collect();
        for ty in 0..2 {
            for tx in 0..2 {
                let page = region.crop(&plate, tx, ty);
                assert_eq!(page.len(), 16);
                for row in 0..4 {
                    for column in 0..4 {
                        let pixel = page[row * 4 + column];
                        assert_eq!(pixel.x, (tx * 4 + column) as f32);
                        assert_eq!(pixel.y, (ty * 4 + row) as f32);
                    }
                }
            }
        }
    }
}

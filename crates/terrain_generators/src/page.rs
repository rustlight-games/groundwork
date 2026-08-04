//! The region a bake covers, and the scale it covers it at.
//!
//! Lifted out of `bake.rs`, and the lift is what makes a crate boundary
//! possible. A page is not a rasteriser concern — it is *where and how finely*
//! terrain is being generated, which is the first thing a generator needs to
//! know. While it lived inside the baker, every module that placed a blade had
//! to depend on the module that drew one.

use glam::{Vec2, Vec3};

use crate::iso;

/// A rectangle of already-projected screen, in cache pixels.
#[derive(Clone, Copy, Debug)]
pub struct Page {
    /// Cache-pixel position of the top-left corner.
    pub origin: Vec2,
    pub width: usize,
    pub height: usize,
    /// How many of this page's pixels one world metre spans.
    ///
    /// [`iso::PX_PER_METRE`] for a page baked at the authoring scale, and less
    /// for one baked for a camera that will not show that much. See
    /// [`Page::detail`].
    pub px_per_metre: f32,
}

impl Page {
    /// A page at the scale the art is authored at.
    pub const fn new(origin: Vec2, width: usize, height: usize) -> Self {
        Self {
            origin,
            width,
            height,
            px_per_metre: iso::PX_PER_METRE,
        }
    }

    /// A page baked at a fraction of the authoring scale.
    ///
    /// `detail` is that fraction: one is the full authoring scale, a quarter
    /// bakes a page that covers sixteen times the ground for the same number of
    /// pixels. **The origin is in this page's own cache pixels**, not in
    /// reference ones, so a page and its neighbour at the same detail tile the
    /// same way they always did.
    pub fn at_detail(origin: Vec2, width: usize, height: usize, detail: f32) -> Self {
        Self {
            origin,
            width,
            height,
            px_per_metre: iso::PX_PER_METRE * detail.max(1.0e-3),
        }
    }

    /// This page's scale as a fraction of the authoring scale.
    ///
    /// The number every length in the art has to be multiplied by. The art is
    /// authored in cache pixels — a blade is 1.6 of them wide, a mound's relief
    /// reaches 17 of them, the guard band is 140 — and every one of those is a
    /// statement about how large a thing is *relative to a metre of ground*. Bake
    /// at a quarter scale without carrying this through and the field shrinks
    /// while its brush marks do not, which is the difference between distant
    /// grass and a page of bristles.
    ///
    /// Two families of number are deliberately **not** scaled by it. Lengths
    /// already expressed in metres — blade length, tuft radius, mound spacing —
    /// scale themselves, because the projection does it for them. And canopy
    /// *height* is kept in reference pixels throughout, so that every shading
    /// term keyed on how tall the grass stands means the same thing at every
    /// detail level; only the distances those terms reach *across* the page are
    /// scaled.
    #[inline]
    pub fn detail(&self) -> f32 {
        self.px_per_metre / iso::PX_PER_METRE
    }

    /// A length authored in reference cache pixels, at this page's scale.
    #[inline]
    pub fn px(&self, reference: f32) -> f32 {
        reference * self.detail()
    }

    /// A blur or search radius authored in reference cache pixels, at this
    /// page's scale — never below one, since a radius of nought is the identity
    /// and would silently delete the shading term rather than coarsen it.
    #[inline]
    pub fn radius(&self, reference: usize) -> usize {
        ((reference as f32 * self.detail()).round() as usize).max(1)
    }

    /// This page's ground, as a world point.
    #[inline]
    pub fn ground_at(&self, pixel: Vec2) -> Vec2 {
        iso::from_cache_ground_at(self.origin + pixel, self.px_per_metre)
    }

    /// A world point as a page pixel, at final resolution.
    ///
    /// The inverse of [`Page::ground_at`] for points on the ground plane, and
    /// the projection for points above it. Placement needs this and used to
    /// reach through a [`Painter`] for it, which meant deciding *where* a blade
    /// goes required a mutable borrow of the surface it would eventually be
    /// drawn on — the coupling that made a scene impossible to build without
    /// also drawing it.
    #[inline]
    pub fn to_pixel(&self, world: Vec3) -> Vec2 {
        iso::to_cache_at(world, self.px_per_metre) - self.origin
    }

    /// A page baked at exactly the scale a camera will show it at.
    ///
    /// The whole point of [`Page::at_detail`], expressed as the call a renderer
    /// actually wants to make. `view_height` is world metres visible
    /// vertically — `terrain_bench::scenarios::RTS_VIEW_M` — and `screen` is the
    /// window; [`iso::view_pixels`] turns the pair into the scale the ground is
    /// presented at, and this bakes there instead of baking at the authoring
    /// scale and throwing the difference away.
    ///
    /// `origin` is in this page's own pixels, so a streaming grid steps by whole
    /// page widths exactly as it does at full detail. What changes is how much
    /// world one page covers: at the fifty-five-metre camera this game ships at,
    /// a page holds about twenty-four times the ground it used to, which is
    /// twenty-four times fewer pages and twenty-four times fewer draw calls for
    /// the same screen.
    ///
    /// Clamped at one: a camera close enough to magnify the ground past the
    /// authoring scale should bake at the authoring scale and be filtered up,
    /// never invent detail the art does not contain.
    pub fn for_view(
        origin: Vec2,
        width: usize,
        height: usize,
        view_height: f32,
        screen: (usize, usize),
    ) -> Self {
        let (_, _, scale) = iso::view_pixels(view_height, screen);
        Self::at_detail(origin, width, height, scale.min(1.0))
    }
}

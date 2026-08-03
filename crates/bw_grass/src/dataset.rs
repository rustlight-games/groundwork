//! Paired renders, for training something to do this faster.
//!
//! The renderer's expensive path now costs seconds a page. That is fine for
//! masters and useless for a game, so the plan is to learn it: feed a network a
//! cheap render plus the structure the cheap render cannot see, and have it
//! produce the expensive one.
//!
//! Which puts one requirement above all the others.
//!
//! ## The pair must be one meadow
//!
//! ```text
//!   wrong                              right
//!   ─────                              ─────
//!   cheap  → generate scene A          generate scene ────┬─→ render cheap
//!   costly → generate scene B                             └─→ render costly
//! ```
//!
//! Generating twice looks safe, because placement is a pure function of world
//! coordinates and both runs would agree. It is not the agreement that matters;
//! it is that nothing can *later* make them disagree. A quality tier that
//! skipped a fork, a step count that moved a rib, an optimisation that reordered
//! a draw — any of those turns the pair into two photographs of two different
//! fields, and a network trained on that learns to hallucinate rather than to
//! reconstruct. The failure is silent: the loss simply stops falling, and no
//! image in the corpus looks wrong.
//!
//! So [`Pair::bake`] builds one [`GrassScene`] and renders it twice. That is
//! also why [`crate::quality::GrassRenderQuality`] is forbidden from changing
//! what grows where — the tier is allowed to decide how finely something is
//! measured and never whether it exists.
//!
//! ## What travels with the target
//!
//! The expensive path decides a great deal from hashes the cheap input cannot
//! see. Whether this broad blade forked, which way its face turned, how much
//! canopy is stacked behind it — none of that is in a low-resolution plate, and
//! a network given only pixels has no choice but to average over the
//! possibilities. Averaged forks are soft tips and averaged occlusion is a flat
//! interior, which are precisely the failures this renderer was built to remove.
//! [`crate::bake::Passes`] is the structure, exported beside the picture.

use bevy::prelude::*;

use crate::bake::{BakeParams, Macro, Page, Passes, cast_shadows, lay_floor, resolve_passes};
use crate::field::WorldField;
use crate::quality::GrassRenderQuality;
use crate::scene::GrassScene;
use crate::stroke::Painter;
use crate::surface::Surface;

/// One rendering of a scene, at one budget.
pub struct Render {
    /// Final-resolution linear colour.
    pub colour: Vec<Vec3>,
    /// What the renderer knew while producing it.
    pub passes: Passes,
    pub width: usize,
    pub height: usize,
}

/// A cheap render and an expensive one, of the same ground.
pub struct Pair {
    pub input: Render,
    pub target: Render,
    /// How many marks the shared scene held. Worth recording: a crop grown from
    /// a nearly empty scene is not a hard example, it is a useless one.
    pub marks: usize,
}

impl Pair {
    /// Build one scene and render it at both budgets.
    pub fn bake(page: Page, params: &BakeParams, input: GrassRenderQuality) -> Self {
        let field = WorldField::lit_by(params.seed, params.light);
        let lattice = Macro::build(&page, &field);
        // Once. See the module note for why this is the whole point.
        let scene = GrassScene::build(page, &field, params);

        let render = |quality: GrassRenderQuality| {
            let params = BakeParams { quality, ..*params };
            let mut surface =
                Surface::at_supersample(page.width, page.height, quality.supersample());
            lay_floor(&mut surface, &page, &field, &lattice);
            {
                let mut painter =
                    Painter::at_scale(&mut surface, page.origin, params.light, page.px_per_metre)
                        .with_ribs_per_pixel(quality.ribs_per_pixel());
                scene.draw(&mut painter);
            }
            let shadows = cast_shadows(&scene, &params);
            let mut passes = Passes::default();
            let colour = resolve_passes(
                &surface,
                &page,
                &lattice,
                &params,
                shadows.as_deref(),
                Some(&mut passes),
            );
            Render {
                colour,
                passes,
                width: page.width,
                height: page.height,
            }
        };

        Self {
            input: render(input),
            target: render(params.quality),
            marks: scene.len(),
        }
    }

    /// Cut the middle out of both renders.
    ///
    /// Training crops are taken from the centre of a larger bake rather than
    /// baked at their own size, and the reason is the same one that made
    /// `bake_padded` necessary: every neighbourhood-reading term — occlusion,
    /// the relief comparison, the shadows themselves — is wrong near an edge.
    /// A corpus of crops baked at their own size teaches the network that page
    /// borders exist, and it will faithfully reproduce them.
    pub fn crop(&self, margin: usize) -> (Vec<Vec3>, Vec<Vec3>, usize, usize) {
        let (w, h) = (self.input.width, self.input.height);
        let margin = margin.min(w / 2).min(h / 2);
        let (cw, ch) = (w - margin * 2, h - margin * 2);
        let cut = |source: &[Vec3]| {
            let mut out = Vec::with_capacity(cw * ch);
            for row in 0..ch {
                let start = (row + margin) * w + margin;
                out.extend_from_slice(&source[start..start + cw]);
            }
            out
        };
        (cut(&self.input.colour), cut(&self.target.colour), cw, ch)
    }
}

/// Everything needed to reproduce a shard, recorded beside it.
///
/// A training corpus that cannot say what produced it is a corpus that can only
/// be regenerated by guessing, and a renderer under active development will not
/// produce the same bytes next month. The point of this is not provenance for
/// its own sake — it is that a model whose targets came from two renderer
/// versions has learned the average of two looks, and nothing in the loss curve
/// says so.
#[derive(Clone, Debug)]
pub struct ShardMetadata {
    /// The renderer's own version, from the crate.
    pub renderer: &'static str,
    /// Which tier produced the target.
    pub target_quality: &'static str,
    /// Which produced the input.
    pub input_quality: &'static str,
    pub seed: u64,
    /// Cache-pixel origin of the page.
    pub origin: (f32, f32),
    pub page_pixels: (usize, usize),
    pub px_per_metre: f32,
    /// The key light, in world space, and its height above the ground.
    pub sun: (f32, f32, f32),
    pub sun_elevation_degrees: f32,
    pub sun_radius: f32,
    pub marks: usize,
}

impl ShardMetadata {
    pub fn of(page: &Page, params: &BakeParams, input: GrassRenderQuality, marks: usize) -> Self {
        let sun = crate::iso::image_to_world(params.light).normalize_or(Vec3::Z);
        Self {
            renderer: env!("CARGO_PKG_VERSION"),
            target_quality: params.quality.name(),
            input_quality: input.name(),
            seed: params.seed,
            origin: (page.origin.x, page.origin.y),
            page_pixels: (page.width, page.height),
            px_per_metre: page.px_per_metre,
            sun: (sun.x, sun.y, sun.z),
            sun_elevation_degrees: crate::iso::elevation_of(sun).to_degrees(),
            sun_radius: params.sun_radius,
            marks,
        }
    }

    /// As RON, which is what the rest of this repository stores data in.
    pub fn to_ron(&self) -> String {
        format!(
            "(\n    renderer: \"{}\",\n    target_quality: \"{}\",\n    \
             input_quality: \"{}\",\n    seed: {},\n    origin: ({}, {}),\n    \
             page_pixels: ({}, {}),\n    px_per_metre: {},\n    \
             sun: ({}, {}, {}),\n    sun_elevation_degrees: {},\n    \
             sun_radius: {},\n    marks: {},\n)\n",
            self.renderer,
            self.target_quality,
            self.input_quality,
            self.seed,
            self.origin.0,
            self.origin.1,
            self.page_pixels.0,
            self.page_pixels.1,
            self.px_per_metre,
            self.sun.0,
            self.sun.1,
            self.sun.2,
            self.sun_elevation_degrees,
            self.sun_radius,
            self.marks,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_is_two_renders_of_one_meadow() {
        // The property everything else here rests on. Not "the two look alike" —
        // they should not, that is the whole point — but that the geometry under
        // them is the same, which is what makes the expensive one a *target* for
        // the cheap one rather than a different picture of a different field.
        let page = Page::new(Vec2::new(-32.0, -32.0), 64, 64);
        let params = BakeParams {
            quality: GrassRenderQuality::Dataset,
            ..default()
        };
        let pair = Pair::bake(page, &params, GrassRenderQuality::Preview);
        assert!(pair.marks > 100, "the scene grew {} marks", pair.marks);
        assert_eq!(pair.input.colour.len(), 64 * 64);
        assert_eq!(pair.target.colour.len(), 64 * 64);

        // The canopy is the geometry, seen from the camera. If the two renders
        // were of different meadows this is where it would show, and it would
        // show far more loudly than in the colour.
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (low, high) = (
            mean(&pair.input.passes.height),
            mean(&pair.target.passes.height),
        );
        assert!(
            (low - high).abs() < high * 0.06 + 0.5,
            "the two renders disagree about the canopy: {low:.2} against {high:.2}"
        );

        // And they are genuinely different pictures, or the pair teaches nothing.
        let difference = pair
            .input
            .colour
            .iter()
            .zip(&pair.target.colour)
            .map(|(a, b)| (*a - *b).length() as f64)
            .sum::<f64>()
            / pair.input.colour.len() as f64;
        assert!(
            difference > 0.005,
            "the cheap and costly renders differ by {difference:.5} — there is \
             nothing here to learn"
        );
    }

    #[test]
    fn every_pass_is_filled_and_finite() {
        // A channel of zeroes or NaNs trains a network to produce zeroes or
        // NaNs, and neither shows up as anything but a loss that will not fall.
        let page = Page::new(Vec2::ZERO, 48, 48);
        let params = BakeParams {
            quality: GrassRenderQuality::Dataset,
            ..default()
        };
        let pair = Pair::bake(page, &params, GrassRenderQuality::Preview);
        let passes = &pair.target.passes;
        for (name, channel) in passes.scalars() {
            assert_eq!(channel.len(), 48 * 48, "{name} is the wrong size");
            assert!(
                channel.iter().all(|v| v.is_finite()),
                "{name} holds a non-finite value"
            );
            let spread = channel.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - channel.iter().cloned().fold(f32::INFINITY, f32::min);
            assert!(spread > 0.0, "{name} is constant across the whole page");
        }
        for (name, channel) in passes.vectors() {
            assert_eq!(channel.len(), 48 * 48, "{name} is the wrong size");
            assert!(
                channel.iter().all(|v| (v.length() - 1.0).abs() < 0.05),
                "{name} holds a normal that is not unit length"
            );
        }
    }

    #[test]
    fn a_crop_takes_the_middle_of_both() {
        let page = Page::new(Vec2::ZERO, 64, 64);
        let params = BakeParams {
            quality: GrassRenderQuality::Preview,
            ..default()
        };
        let pair = Pair::bake(page, &params, GrassRenderQuality::Preview);
        let (input, target, w, h) = pair.crop(8);
        assert_eq!((w, h), (48, 48));
        assert_eq!(input.len(), 48 * 48);
        assert_eq!(target.len(), 48 * 48);
        // The crop's top-left is the page's (8, 8).
        assert_eq!(input[0], pair.input.colour[8 * 64 + 8]);
        assert_eq!(target[0], pair.target.colour[8 * 64 + 8]);
        // And a margin larger than the page does not panic.
        let (_, _, w, _) = pair.crop(999);
        assert_eq!(w, 0);
    }

    #[test]
    fn metadata_records_where_the_sun_actually_was() {
        // In world degrees, not in whatever the image vector's `z` happens to
        // be. A corpus whose metadata says 35° and whose targets were lit at 55°
        // is worse than one with no metadata at all.
        let page = Page::new(Vec2::ZERO, 32, 32);
        let params = BakeParams::default();
        let meta = ShardMetadata::of(&page, &params, GrassRenderQuality::Preview, 1234);
        assert!(
            (meta.sun_elevation_degrees - crate::lab::DEFAULT_ELEVATION.to_degrees()).abs() < 0.5,
            "metadata says {}°",
            meta.sun_elevation_degrees
        );
        let ron = meta.to_ron();
        assert!(ron.starts_with('('), "{ron}");
        assert!(ron.contains("marks: 1234"));
        assert!(ron.contains("sun_elevation_degrees:"));
    }
}

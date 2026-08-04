//! A digest of the meadow a page grew, taken before anything draws it.
//!
//! This exists for one job, and it is a job the rest of the measurement suite
//! cannot do. The snapshot compares finished pixels and the critique compares
//! bands of colour; both answer questions about the *picture*, and both are
//! therefore hostage to every renderer, every palette and every shading term
//! between the placement code and the plate. During an architecture migration
//! that is precisely the wrong instrument. Code is about to move between crates
//! by the thousand lines, and the question worth asking after each move is not
//! "does it still look right" but **"is it the same meadow"**.
//!
//! So this hashes the scene: where every mark is, what shape it is, what it is
//! made of, and the ground underneath it. Nothing about light, nothing about
//! rasterisation, nothing about the order files happen to be compiled in. Two
//! fingerprints that match mean the generator survived the move intact, and they
//! mean it in a fraction of a second with no Blender and no reference art.
//!
//! ## What is deliberately not in it
//!
//! Pointers, capacities, and anything with a `Debug` string in it. A digest that
//! moves when a `Vec` reallocates is a digest nobody trusts, and one that moves
//! when a doc comment is reworded is worse — it trains its readers to accept a
//! new baseline without looking, which is the failure mode the whole benchmark
//! culture here exists to prevent.
//!
//! ## Exact for marks, quantised for ground
//!
//! Every parameter of a mark is hashed by its exact bit pattern, because a mark
//! is *chosen* — the generator picked that length and that bend, and if a
//! refactor changes them by one unit in the last place then something has been
//! reordered and that is worth knowing about.
//!
//! Ground heights are hashed to the millimetre instead. The height field is
//! sampled through a good deal of trigonometry, and the last bit of a value that
//! has been through six transcendental functions is not a decision anybody made.
//! A millimetre over a mound whose relief runs to a quarter of a metre is a
//! quarter of a percent, so a real change to the field cannot hide under it.

use std::fmt;

use glam::Vec2;

use crate::field::WorldField;
use crate::geometry::TipProfile;
use crate::page::Page;
use crate::scene::GrassScene;
use crate::stroke::Stroke;

/// Bumped when the generator is *meant* to produce a different meadow.
///
/// The point of a version in the digest is that it makes the two kinds of change
/// distinguishable. A fingerprint mismatch with the version unchanged is a
/// regression; a mismatch with the version bumped in the same commit is a
/// decision, and the commit message is where the reason for it lives.
pub const GENERATOR_VERSION: u32 = 1;

/// Ground samples along each axis of a page, for the ground half of the digest.
///
/// Odd, so the lattice includes the page's centre as well as its corners. The
/// grid is page-relative rather than world-relative because a fingerprint is
/// always about one page — sampling a fixed world lattice would make two pages
/// of different sizes read the same ground at different densities.
const GROUND_LATTICE: usize = 33;

/// Height quantisation, in steps per metre.
const HEIGHT_STEPS: f32 = 1000.0;

/// A 128-bit digest of a generated scene.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneFingerprint(u128);

impl SceneFingerprint {
    pub const fn from_u128(bits: u128) -> Self {
        Self(bits)
    }

    pub const fn to_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for SceneFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Debug for SceneFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SceneFingerprint({self})")
    }
}

impl std::str::FromStr for SceneFingerprint {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        u128::from_str_radix(text.trim().trim_start_matches("0x"), 16).map(Self)
    }
}

/// An order-sensitive accumulator, pinned so that a fingerprint written down
/// today still means the same thing after the crate it lives in is renamed.
///
/// FNV-1a's structure over 64-bit words rather than bytes: xor the word into the
/// low half, multiply the whole 128 bits by a prime with a high set bit. The
/// multiply is what carries the low half's information upward, so a change to
/// the very first mark still moves the top byte of the result.
///
/// Not cryptographic and not trying to be. It is compared against a value in a
/// file in this repository, by a test, on a machine that also holds the code
/// that produced it; the threat is a silent reordering, not an adversary.
#[derive(Clone, Copy)]
pub struct Digest {
    state: u128,
}

/// FNV-1a's 128-bit offset basis.
const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;

/// FNV-1a's 128-bit prime, `2^88 + 0x13B`.
const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

impl Default for Digest {
    fn default() -> Self {
        Self::new()
    }
}

impl Digest {
    pub const fn new() -> Self {
        Self {
            state: OFFSET_BASIS,
        }
    }

    /// Absorb one 64-bit word.
    #[inline]
    pub fn u64(&mut self, word: u64) -> &mut Self {
        self.state ^= word as u128;
        self.state = self.state.wrapping_mul(PRIME);
        self
    }

    #[inline]
    pub fn u32(&mut self, word: u32) -> &mut Self {
        self.u64(word as u64)
    }

    /// Absorb a length or an index.
    ///
    /// Widened to 64 bits so that a digest taken on a 32-bit target matches one
    /// taken on a 64-bit one.
    #[inline]
    pub fn usize(&mut self, word: usize) -> &mut Self {
        self.u64(word as u64)
    }

    /// Absorb an `f32` by its exact bit pattern.
    ///
    /// Two values are canonicalised on the way in. Negative zero becomes zero,
    /// because `-0.0 == 0.0` everywhere else in the renderer and a digest that
    /// disagreed with `==` would be reporting a difference nothing can observe.
    /// Every NaN becomes one NaN, because the payload bits of a NaN are not a
    /// decision the generator made.
    #[inline]
    pub fn f32(&mut self, value: f32) -> &mut Self {
        let bits = if value == 0.0 {
            0
        } else if value.is_nan() {
            0x7fc0_0000
        } else {
            value.to_bits()
        };
        self.u32(bits)
    }

    /// Absorb a real number to a fixed number of steps per unit.
    ///
    /// For quantities that fall out of a long chain of transcendental functions,
    /// where the last bit is arithmetic noise rather than an authored value.
    #[inline]
    pub fn quantised(&mut self, value: f32, steps: f32) -> &mut Self {
        self.u64((value * steps).round() as i64 as u64)
    }

    /// Absorb an enum discriminant or a structural marker.
    ///
    /// Separate from [`Digest::u32`] only for readability at the call site, but
    /// the readability matters: a tag is what stops two differently-shaped
    /// values with the same field bits digesting identically.
    #[inline]
    pub fn tag(&mut self, tag: u8) -> &mut Self {
        self.u64(tag as u64)
    }

    pub fn finish(&self) -> SceneFingerprint {
        SceneFingerprint(self.state)
    }
}

/// Absorb the page a scene grew on: where it is, how big, and at what scale.
pub fn page(digest: &mut Digest, page: &Page) {
    digest
        .f32(page.origin.x)
        .f32(page.origin.y)
        .usize(page.width)
        .usize(page.height)
        .f32(page.px_per_metre);
}

/// Absorb one mark, exactly.
///
/// Every field of [`Stroke`] appears here, and that is a maintenance contract
/// rather than an accident: a parameter added to the vocabulary and not added
/// here is a parameter the migration cannot prove it preserved.
pub fn mark(digest: &mut Digest, mark: &Stroke) {
    // Where it grows.
    digest.f32(mark.root.x).f32(mark.root.y).f32(mark.root.z);

    // The centreline: everything that decides the shape of the arc.
    digest
        .f32(mark.azimuth)
        .f32(mark.length)
        .f32(mark.bend)
        .f32(mark.curl)
        .f32(mark.sway)
        .f32(mark.kink)
        .f32(mark.kink_at)
        .f32(mark.kink_turn);

    // The cross-section: how wide it is and what shape that width is.
    digest
        .f32(mark.width)
        .f32(mark.tip_width)
        .tag(mark.profile as u8)
        .f32(mark.twist)
        .f32(mark.ridge);
    tip(digest, mark.tip);

    // What it is made of, and the intrinsic attributes a material reads.
    digest
        .f32(mark.maturity)
        .tag(mark.tone as u8)
        .f32(mark.base_light)
        .f32(mark.tip_light)
        .f32(mark.glint)
        .f32(mark.side_light)
        .f32(mark.under)
        .f32(mark.depth_bias);
}

/// Absorb a tip, tagged so that two variants cannot collide through their
/// payloads.
fn tip(digest: &mut Digest, tip: TipProfile) {
    match tip {
        TipProfile::Pointed => {
            digest.tag(0);
        }
        TipProfile::Notched { depth } => {
            digest.tag(1).f32(depth);
        }
        TipProfile::Forked {
            split_at,
            opening,
            long,
            short,
        } => {
            digest
                .tag(2)
                .f32(split_at)
                .f32(opening)
                .f32(long)
                .f32(short);
        }
    }
}

/// Absorb every mark, in the order the scene holds them.
///
/// The order is part of the digest and is meant to be. It is the painter order —
/// mat first, so it is buried — and a refactor that produced the same marks in a
/// different sequence would produce a different picture wherever the depth test
/// ties.
pub fn marks(digest: &mut Digest, marks: &[Stroke]) {
    digest.usize(marks.len());
    for one in marks {
        mark(digest, one);
    }
}

/// Absorb the ground under a page, on a fixed lattice, to the millimetre.
///
/// Sampled rather than stored, because the scene does not hold a ground grid —
/// the Cycles export builds one and the rasteriser reads the field per pixel.
/// What matters for the migration is that both of them would be reading the same
/// surface afterwards as before, and a lattice over the page proves that without
/// committing the digest to either one's grid.
pub fn ground(digest: &mut Digest, page: &Page, field: &WorldField) {
    digest.usize(GROUND_LATTICE).usize(GROUND_LATTICE);
    let last = (GROUND_LATTICE - 1) as f32;
    for row in 0..GROUND_LATTICE {
        for column in 0..GROUND_LATTICE {
            let pixel = Vec2::new(
                page.width as f32 * column as f32 / last,
                page.height as f32 * row as f32 / last,
            );
            let height = field.sample(page.ground_at(pixel)).height;
            digest.quantised(height, HEIGHT_STEPS);
        }
    }
}

impl GrassScene {
    /// The digest of this scene: the page, every mark, and the ground beneath.
    ///
    /// `seed` is taken rather than read off the field because a field does not
    /// expose its own seed, and the seed is the one input that decides everything
    /// here — leaving it out would let two worlds share a fingerprint.
    pub fn fingerprint(&self, seed: u64, field: &WorldField) -> SceneFingerprint {
        let mut digest = Digest::new();
        digest.u32(GENERATOR_VERSION).u64(seed);
        page(&mut digest, &self.page);
        marks(&mut digest, &self.marks);
        ground(&mut digest, &self.page, field);
        digest.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::BakeParams;
    use std::str::FromStr;

    fn scene_at(origin: Vec2, side: usize, seed: u64) -> (GrassScene, WorldField, u64) {
        let params = BakeParams {
            seed,
            ..BakeParams::default()
        };
        let field = WorldField::lit_by(params.seed, params.light);
        let page = Page::new(origin, side, side);
        let scene = GrassScene::build(page, &field, &params.grass());
        (scene, field, params.seed)
    }

    #[test]
    fn the_same_scene_fingerprints_the_same_way_twice() {
        let (scene, field, seed) = scene_at(Vec2::new(-64.0, -64.0), 64, 7);
        assert_eq!(
            scene.fingerprint(seed, &field),
            scene.fingerprint(seed, &field)
        );
    }

    #[test]
    fn rebuilding_a_page_reproduces_its_fingerprint() {
        // The property the whole migration leans on. Placement is a pure
        // function of world coordinates, so a page grown twice is the same
        // meadow — and this is the cheap, total statement of that, against the
        // per-field spot checks in `scene`.
        let (first, field, seed) = scene_at(Vec2::new(128.0, -32.0), 64, 11);
        let (second, _, _) = scene_at(Vec2::new(128.0, -32.0), 64, 11);
        assert_eq!(
            first.fingerprint(seed, &field),
            second.fingerprint(seed, &field)
        );
    }

    #[test]
    fn a_different_seed_is_a_different_meadow() {
        let (a, a_field, a_seed) = scene_at(Vec2::ZERO, 48, 1);
        let (b, b_field, b_seed) = scene_at(Vec2::ZERO, 48, 2);
        assert_ne!(
            a.fingerprint(a_seed, &a_field),
            b.fingerprint(b_seed, &b_field)
        );
    }

    #[test]
    fn a_different_place_is_a_different_meadow() {
        let (a, field, seed) = scene_at(Vec2::ZERO, 48, 5);
        let (b, _, _) = scene_at(Vec2::new(2048.0, 1024.0), 48, 5);
        assert_ne!(a.fingerprint(seed, &field), b.fingerprint(seed, &field));
    }

    #[test]
    fn the_seed_reaches_the_digest_even_when_the_scene_does_not_change() {
        // Guards the one input that is passed in rather than derived. Were the
        // seed dropped from the digest, this would be the only test that
        // noticed.
        let (scene, field, _) = scene_at(Vec2::ZERO, 32, 3);
        assert_ne!(scene.fingerprint(3, &field), scene.fingerprint(4, &field));
    }

    #[test]
    fn moving_one_mark_moves_the_fingerprint() {
        let (mut scene, field, seed) = scene_at(Vec2::ZERO, 48, 9);
        let before = scene.fingerprint(seed, &field);
        // A tenth of a millimetre, on one mark out of thousands.
        scene.marks[0].root.x += 0.0001;
        assert_ne!(before, scene.fingerprint(seed, &field));
    }

    #[test]
    fn reordering_the_marks_moves_the_fingerprint() {
        let (mut scene, field, seed) = scene_at(Vec2::ZERO, 48, 9);
        let before = scene.fingerprint(seed, &field);
        let last = scene.marks.len() - 1;
        scene.marks.swap(0, last);
        assert_ne!(before, scene.fingerprint(seed, &field));
    }

    #[test]
    fn every_stroke_parameter_reaches_the_digest() {
        // The maintenance contract, enforced. A field added to `Stroke` and not
        // added to `mark` would leave the migration unable to prove it preserved
        // that field, and this is what says so at the moment it happens.
        let base = Stroke::default();
        let mut digest = Digest::new();
        mark(&mut digest, &base);
        let reference = digest.finish();

        /// One named nudge to a single parameter.
        type Nudge = (&'static str, fn(&mut Stroke));

        let mutations: [Nudge; 24] = [
            ("root.x", |s| s.root.x += 1.0),
            ("root.y", |s| s.root.y += 1.0),
            ("root.z", |s| s.root.z += 1.0),
            ("azimuth", |s| s.azimuth += 1.0),
            ("length", |s| s.length += 1.0),
            ("bend", |s| s.bend += 1.0),
            ("curl", |s| s.curl += 1.0),
            ("sway", |s| s.sway += 1.0),
            ("kink", |s| s.kink += 1.0),
            ("kink_at", |s| s.kink_at += 1.0),
            ("kink_turn", |s| s.kink_turn += 1.0),
            ("width", |s| s.width += 1.0),
            ("tip_width", |s| s.tip_width += 1.0),
            ("profile", |s| s.profile = crate::stroke::Profile::Oval),
            ("twist", |s| s.twist += 1.0),
            ("ridge", |s| s.ridge += 1.0),
            ("tip", |s| s.tip = TipProfile::Notched { depth: 0.2 }),
            ("maturity", |s| s.maturity += 1.0),
            ("tone", |s| s.tone = crate::tone::Tone::Dry),
            ("base_light", |s| s.base_light += 1.0),
            ("tip_light", |s| s.tip_light += 1.0),
            ("glint", |s| s.glint += 1.0),
            ("side_light", |s| s.side_light += 1.0),
            ("under", |s| s.under += 1.0),
        ];
        for (name, mutate) in mutations {
            let mut moved = base;
            mutate(&mut moved);
            let mut digest = Digest::new();
            mark(&mut digest, &moved);
            assert_ne!(
                reference,
                digest.finish(),
                "{name} does not reach the digest"
            );
        }

        // `depth_bias` last, because the default is already zero and adding to
        // it is the same shape as the rest only by luck.
        let mut biased = base;
        biased.depth_bias = 0.5;
        let mut digest = Digest::new();
        mark(&mut digest, &biased);
        assert_ne!(
            reference,
            digest.finish(),
            "depth_bias does not reach the digest"
        );
    }

    #[test]
    fn the_three_tips_are_told_apart() {
        // Tagged variants, so a `Notched { depth: 0.0 }` cannot digest as a
        // `Pointed` merely by having no interesting payload.
        let tips = [
            TipProfile::Pointed,
            TipProfile::Notched { depth: 0.0 },
            TipProfile::Forked {
                split_at: 0.0,
                opening: 0.0,
                long: 0.0,
                short: 0.0,
            },
        ];
        let digests: Vec<_> = tips
            .iter()
            .map(|&t| {
                let mut digest = Digest::new();
                tip(&mut digest, t);
                digest.finish()
            })
            .collect();
        assert_ne!(digests[0], digests[1]);
        assert_ne!(digests[1], digests[2]);
        assert_ne!(digests[0], digests[2]);
    }

    #[test]
    fn negative_zero_digests_as_zero() {
        // `-0.0 == 0.0` everywhere else in the renderer, and a digest that
        // disagreed with `==` would report a difference nothing can observe.
        let mut zero = Digest::new();
        zero.f32(0.0);
        let mut negative = Digest::new();
        negative.f32(-0.0);
        assert_eq!(zero.finish(), negative.finish());
    }

    #[test]
    fn the_digest_is_order_sensitive() {
        let mut forward = Digest::new();
        forward.u64(1).u64(2);
        let mut backward = Digest::new();
        backward.u64(2).u64(1);
        assert_ne!(forward.finish(), backward.finish());
    }

    #[test]
    fn the_meadow_survives_every_change_to_how_it_is_drawn() {
        // The property the `BakeParams` split exists to make visible, and the
        // one that was true and invisible while the two halves were one struct.
        //
        // Twenty-three of the parameters decide the *picture* — the fake
        // occlusion, the macro lighting, the colour grade — and not one of them
        // moves a blade. So a plate can be re-shaded without regenerating a
        // scene, and a training pair's two halves can be lit differently while
        // remaining the same meadow. An invisible property is one somebody
        // breaks; this is the test that notices.
        let base = BakeParams::default();
        let field = WorldField::lit_by(base.seed, base.light);
        let page = Page::new(Vec2::new(-48.0, -48.0), 48, 48);
        let reference =
            GrassScene::build(page, &field, &base.grass()).fingerprint(base.seed, &field);

        type Nudge = (&'static str, fn(&mut crate::bake::PreviewRasterStyle));
        let nudges: [Nudge; 12] = [
            ("form_light", |r| r.form_light += 0.5),
            ("mound_light", |r| r.mound_light += 0.5),
            ("elevation_light", |r| r.elevation_light += 0.5),
            ("crown_light", |r| r.crown_light += 0.5),
            ("ambient_occlusion", |r| r.ambient_occlusion += 0.5),
            ("interior", |r| r.interior += 0.5),
            ("canopy_relief", |r| r.canopy_relief += 0.5),
            ("shadow", |r| r.shadow += 0.5),
            ("shade_depth", |r| r.shade_depth += 0.5),
            ("sky_fill", |r| r.sky_fill += 0.5),
            ("transmission", |r| r.transmission += 0.5),
            ("glaze", |r| r.glaze += 0.5),
        ];
        for (name, nudge) in nudges {
            let mut params = base;
            nudge(&mut params.raster);
            let moved =
                GrassScene::build(page, &field, &params.grass()).fingerprint(params.seed, &field);
            assert_eq!(
                reference, moved,
                "changing `{name}` moved the meadow — it belongs in GrassStyle, \
                 not PreviewRasterStyle"
            );
        }
    }

    #[test]
    fn the_meadow_does_move_when_the_style_changes() {
        // The other half of the claim. A split that put everything in the raster
        // half would pass the test above and mean nothing.
        let base = BakeParams::default();
        let field = WorldField::lit_by(base.seed, base.light);
        // A full-size page: the tiller vocabulary that reads blade_bend does not
        // grow on half a square metre, so a smaller one would pass this test by
        // never reaching the code it is checking.
        let page = Page::new(Vec2::new(-48.0, -48.0), 96, 96);
        let reference =
            GrassScene::build(page, &field, &base.grass()).fingerprint(base.seed, &field);

        // `blade_bend` is deliberately absent, and the reason is a finding
        // rather than an omission — see `blade_bend_reaches_nothing` below.
        type Nudge = (&'static str, fn(&mut crate::style::GrassStyle));
        let nudges: [Nudge; 5] = [
            ("tufts", |s| s.tufts *= 1.5),
            ("fine", |s| s.fine *= 1.5),
            ("blade_length", |s| s.blade_length.1 *= 1.5),
            ("blade_width", |s| s.blade_width.1 *= 1.5),
            ("thatch", |s| s.thatch *= 1.5),
        ];
        for (name, nudge) in nudges {
            let mut params = base;
            nudge(&mut params.style);
            let moved =
                GrassScene::build(page, &field, &params.grass()).fingerprint(params.seed, &field);
            assert_ne!(
                reference, moved,
                "changing `{name}` did not move the meadow"
            );
        }
    }

    /// `blade_bend` is a dead parameter, and this asserts the gap rather than
    /// the fix.
    ///
    /// It is read in exactly one place — `Mark::shape`, which builds the base
    /// stroke for a tiller — and `Mark::shape` is never called. So the authored
    /// bend range `(0.35, 1.40)` reaches nothing: set it to `(5.0, 9.0)` and not
    /// one of nine thousand marks moves.
    ///
    /// Left alone on purpose. Wiring it up would change the meadow, which is a
    /// deliberate look change and not something to smuggle into a migration
    /// whose entire claim is that it moved nothing. This is written down so the
    /// next person to find it is told it is known, and so that whoever fixes it
    /// is told to delete a test rather than left wondering whether the
    /// parameter was meant to be inert.
    ///
    /// The same shape of defect as the rock palette's `luminance_spread`, which
    /// reads the same value for all ten seeds.
    #[test]
    fn blade_bend_reaches_nothing() {
        let base = BakeParams::default();
        let field = WorldField::lit_by(base.seed, base.light);
        let page = Page::new(Vec2::new(-48.0, -48.0), 96, 96);
        let reference =
            GrassScene::build(page, &field, &base.grass()).fingerprint(base.seed, &field);

        let mut absurd = base;
        absurd.style.blade_bend = (5.0, 9.0);
        assert_eq!(
            reference,
            GrassScene::build(page, &field, &absurd.grass()).fingerprint(absurd.seed, &field),
            "blade_bend now reaches the meadow — delete this test and add it to \
             the list in `the_meadow_does_move_when_the_style_changes`"
        );
    }

    #[test]
    fn a_fingerprint_survives_a_round_trip_through_text() {
        let (scene, field, seed) = scene_at(Vec2::ZERO, 32, 13);
        let printed = scene.fingerprint(seed, &field);
        assert_eq!(
            SceneFingerprint::from_str(&printed.to_string()),
            Ok(printed)
        );
        assert_eq!(printed.to_string().len(), 32);
    }

    #[test]
    fn ground_quantisation_ignores_a_micron_and_catches_a_centimetre() {
        let mut coarse = Digest::new();
        coarse.quantised(0.1, HEIGHT_STEPS);
        let mut nudged = Digest::new();
        nudged.quantised(0.1 + 1.0e-6, HEIGHT_STEPS);
        assert_eq!(coarse.finish(), nudged.finish());

        let mut moved = Digest::new();
        moved.quantised(0.11, HEIGHT_STEPS);
        assert_ne!(coarse.finish(), moved.finish());
    }
}

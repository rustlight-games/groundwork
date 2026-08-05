//! Measuring a nine-tile render: the subject, and the joins.
//!
//! Two questions that the existing instruments cannot answer, because both are
//! about the *layout* rather than about a rectangle of ground.
//!
//! **Is the subject any good?** A nine-tile plate is nine ninths context and one
//! ninth subject, so a metric over the whole frame is eight parts set dressing.
//! A change that improved the middle tile and did nothing else would move such a
//! number by a ninth of its real size, which is inside the noise. So every
//! measurement here is taken twice: once over the layout, once weighted by the
//! subject mask, and the pair is what says whether a change is about the subject
//! or about everything.
//!
//! **Are the internal joins invisible?** They are supposed to be — the ground is
//! continuous across them and the tiles are a semantic division, not a
//! generation boundary. But "supposed to be" is exactly the kind of claim that
//! stops being true without anything failing. The measurement is deliberately
//! *relative*: the difference across a tile join, against the difference across
//! an arbitrary parallel line in the same picture. Grass is high-frequency, so
//! the absolute number is large and meaningless; the ratio is the whole content.
//! One is invisible. Two is a line.
//!
//! ## Pinned, never random
//!
//! `./render` picks a fresh world every time, and that is right for looking at
//! pictures and wrong for measuring them. Every scenario here names
//! its seed and its centre tile, and the rule is the one
//! [`crate::scenarios::SCENARIOS`] obeys: **append only**.

use glam::Vec3;
use terrain_scene::frame::{IsoFrameOptions, ResolvedIsoFrame, resolve_render_sample};
use terrain_scene::layout::{TileLayoutPreset, TileRole, WorldTileCoord};
use terrain_scene::overlay;
use terrain_scene::pixel::RenderImage;
use terrain_scene::projection::Projection;

/// One pinned nine-tile render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IsoScenario {
    /// The stable, dotted name a measurement is recorded under.
    pub name: &'static str,
    /// The world seed, written the way it is printed.
    pub seed: u64,
    /// The subject tile. Named rather than derived, so a change to the
    /// derivation cannot silently move every measurement to different ground.
    pub centre_tile: WorldTileCoord,
    pub tile_side_m: f64,
    pub output_size: [u32; 2],
    pub fill: f64,
}

impl IsoScenario {
    /// The frame this scenario resolves to.
    pub fn frame(self) -> ResolvedIsoFrame {
        resolve_render_sample(
            TileLayoutPreset::Nine,
            self.tile_side_m,
            terrain_scene::frame::RenderIdentity::new(self.seed, self.centre_tile),
            Projection::DIMETRIC_2_1,
            IsoFrameOptions {
                output_size: self.output_size,
                fill: self.fill,
                subject_position: [0.5, 0.5],
                halo_m: terrain_scene::frame::DEFAULT_HALO_M,
            },
        )
        .expect("a pinned scenario is well formed")
        .frame
    }
}

/// The pinned nine-tile scenarios. **Append only.**
///
/// Small, deliberately. These run in a test suite rather than in an overnight
/// job, and a 480×270 plate holds a whole layout at a scale where a seam is
/// still several pixels wide.
pub const ISO_SCENARIOS: [IsoScenario; 3] = [
    IsoScenario {
        // The base case: the world origin, the shipping tile size.
        name: "iso_nine.origin",
        seed: 7,
        centre_tile: WorldTileCoord::new(0, 0),
        tile_side_m: 2.0,
        output_size: [480, 270],
        fill: 0.90,
    },
    IsoScenario {
        // A long way from the origin and negative on both axes, which is where a
        // single-precision cache pixel and a floored division both stop being
        // obviously right.
        name: "iso_nine.far_negative",
        seed: 0x5a17_e33b_0c9d_2f14,
        centre_tile: WorldTileCoord::new(-1829, -1410),
        tile_side_m: 2.0,
        output_size: [480, 270],
        fill: 0.90,
    },
    IsoScenario {
        // Four-metre tiles: half the detail per metre, and the framing the
        // layout was first worked out at.
        name: "iso_nine.coarse_tiles",
        seed: 11,
        centre_tile: WorldTileCoord::new(563, 124),
        tile_side_m: 4.0,
        output_size: [480, 270],
        fill: 0.90,
    },
];

/// Find a pinned scenario by name.
pub fn iso_scenario(name: &str) -> Option<IsoScenario> {
    ISO_SCENARIOS.into_iter().find(|s| s.name == name)
}

/// What a nine-tile plate came to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IsoReport {
    /// Fraction of the plate that is picture rather than background.
    pub coverage: f64,
    /// Fraction of the *subject tile* that is picture.
    ///
    /// One, or very near it. Anything else means the silhouette has a hole in
    /// the middle of the frame, which is a bug rather than a look.
    pub subject_coverage: f64,
    /// Mean luminance over the layout, and over the subject alone.
    pub luminance: f64,
    pub subject_luminance: f64,
    /// Mean absolute neighbour difference — how much fine structure there is.
    pub detail: f64,
    pub subject_detail: f64,
    /// How visible the internal tile joins are. See [`join_visibility`].
    pub join_visibility: f64,
}

/// Measure a baked layout.
pub fn measure(frame: &ResolvedIsoFrame, image: &RenderImage) -> IsoReport {
    let subject = overlay::subject_mask(frame);
    let layout = overlay::layout_mask(frame);

    let weighted = |mask: &[f32], value: &dyn Fn(usize) -> f64| {
        let mut total = 0.0;
        let mut weight = 0.0;
        for (index, m) in mask.iter().enumerate() {
            let m = *m as f64;
            if m <= 0.0 {
                continue;
            }
            total += value(index) * m;
            weight += m;
        }
        if weight > 0.0 { total / weight } else { 0.0 }
    };

    let luma = |index: usize| luminance(image.colour[index]) as f64;
    let alpha = |index: usize| image.alpha[index] as f64;
    let detail = neighbour_difference(image);

    IsoReport {
        coverage: image.coverage() as f64,
        subject_coverage: weighted(&subject, &alpha),
        luminance: weighted(&layout, &luma),
        subject_luminance: weighted(&subject, &luma),
        detail: weighted(&layout, &|index| detail[index] as f64),
        subject_detail: weighted(&subject, &|index| detail[index] as f64),
        join_visibility: join_visibility(frame, image),
    }
}

/// Rec. 709 luminance.
fn luminance(colour: Vec3) -> f32 {
    colour.x * 0.2126 + colour.y * 0.7152 + colour.z * 0.0722
}

/// Mean absolute difference to the right and lower neighbour, per pixel.
///
/// The detail-energy measure the rest of the suite uses, computed here rather
/// than borrowed because it has to be maskable: the whole point is comparing the
/// subject against its surroundings.
fn neighbour_difference(image: &RenderImage) -> Vec<f32> {
    let (w, h) = (image.width, image.height);
    let mut detail = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let here = luminance(image.colour[y * w + x]);
            let right = luminance(image.colour[y * w + (x + 1).min(w - 1)]);
            let below = luminance(image.colour[(y + 1).min(h - 1) * w + x]);
            detail[y * w + x] = ((here - right).abs() + (here - below).abs()) * 0.5;
        }
    }
    detail
}

/// How far inside a tile the bands either side of a join are sampled.
const JOIN_BAND_PX: i64 = 3;

/// How visible the internal tile joins are, against the picture's own noise.
///
/// One means a join is indistinguishable from an arbitrary line drawn in the
/// same place; two means the step across it is twice what the grass does on its
/// own, which is a visible line.
///
/// Relative rather than absolute, and that is the whole design. Grass is
/// high-frequency: the mean difference between two neighbouring bands of it is
/// large whether or not there is a seam, so an absolute threshold would either
/// pass everything or fail everything depending on how lush the meadow was. The
/// control is the same measurement taken a few pixels away from the join, where
/// there is certainly nothing.
pub fn join_visibility(frame: &ResolvedIsoFrame, image: &RenderImage) -> f64 {
    let mut at_join = 0.0;
    let mut control = 0.0;
    let mut samples = 0usize;

    // Every edge of every tile that is shared with another tile in the layout.
    // Walked from the subject outward, because the subject's four edges are the
    // four joins a reader actually looks at.
    for tile in &frame.tile_polygons_px {
        if tile.role != TileRole::Subject {
            continue;
        }
        for index in 0..4 {
            let a = tile.corners_px[index];
            let b = tile.corners_px[(index + 1) % 4];
            let (step_a, step_b, step_control) = edge_bands(a, b);
            let length = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() as usize;
            for t in 1..length.max(2) {
                let f = t as f32 / length.max(2) as f32;
                let on = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
                let (Some(inside), Some(outside), Some(further)) = (
                    sample(image, on, step_a),
                    sample(image, on, step_b),
                    sample(image, on, step_control),
                ) else {
                    continue;
                };
                at_join += (inside - outside).abs() as f64;
                control += (outside - further).abs() as f64;
                samples += 1;
            }
        }
    }

    if samples == 0 || control <= 0.0 {
        return 0.0;
    }
    (at_join / samples as f64) / (control / samples as f64)
}

/// The three offsets a join is sampled at: inside, outside, and one further out.
fn edge_bands(a: [f32; 2], b: [f32; 2]) -> ([f32; 2], [f32; 2], [f32; 2]) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = (dx * dx + dy * dy).sqrt().max(1.0e-6);
    // The edge's own normal, in pixels.
    let normal = [-dy / length, dx / length];
    let band = JOIN_BAND_PX as f32;
    (
        [normal[0] * band, normal[1] * band],
        [-normal[0] * band, -normal[1] * band],
        [-normal[0] * band * 3.0, -normal[1] * band * 3.0],
    )
}

/// Luminance at a point, or `None` off the plate.
fn sample(image: &RenderImage, at: [f32; 2], offset: [f32; 2]) -> Option<f32> {
    let x = (at[0] + offset[0]).round() as i64;
    let y = (at[1] + offset[1]).round() as i64;
    if x < 0 || y < 0 || x >= image.width as i64 || y >= image.height as i64 {
        return None;
    }
    let index = y as usize * image.width + x as usize;
    // Background is not a measurement. A join whose control band fell off the
    // diamond would report the difference between grass and nothing.
    (image.alpha[index] > 0.99).then(|| luminance(image.colour[index]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plate shaped like a real render but filled with independent noise.
    ///
    /// There is no renderer in this crate, so a unit test cannot ask "is the
    /// seam invisible in an actual render" — that question is answered by
    /// `terrain compile` and read off a real Cycles plate, per AGENTS.md
    /// invariant 8. What a synthetic plate *can* prove is that the measurement
    /// code — [`measure`], [`join_visibility`] — behaves correctly: coverage
    /// matches the mask it was built from, and a field with no seam by
    /// construction (independent per-pixel noise has no structure at a tile
    /// boundary) measures as one.
    ///
    /// `subject_lift` raises the subject tile above its surroundings. Zero is
    /// the uniform field most tests want; a non-zero value is what proves the
    /// subject-weighted metrics actually read the subject rather than
    /// reporting the layout figure twice.
    fn synthetic_image(frame: &ResolvedIsoFrame, seed: u64, subject_lift: f32) -> RenderImage {
        let (width, height) = (frame.output_size[0] as usize, frame.output_size[1] as usize);
        let mask = overlay::layout_mask(frame);
        let subject = overlay::subject_mask(frame);
        let mut colour = Vec::with_capacity(width * height);
        // Never zero: an all-zero state is the xorshift's one fixed point, and
        // a fixture that silently produced a constant field would pass the
        // tests below for the wrong reason.
        let mut state = (seed ^ 0x9E37_79B9_7F4A_7C15) | 1;
        for lit in subject.iter().take(width * height) {
            // A cheap xorshift, enough to give the field texture without
            // pulling in a dependency for a test fixture.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = ((state >> 40) as u32 & 0xFF) as f32 / 255.0;
            colour.push(Vec3::splat(0.3 + value * 0.4 + subject_lift * lit));
        }
        RenderImage {
            colour,
            alpha: mask,
            width,
            height,
        }
    }

    #[test]
    fn every_iso_scenario_has_a_distinct_dotted_name() {
        let mut seen: Vec<&str> = Vec::new();
        for scenario in ISO_SCENARIOS {
            assert!(!seen.contains(&scenario.name), "{} twice", scenario.name);
            assert!(scenario.name.contains('.'), "{}", scenario.name);
            assert_eq!(scenario.name, scenario.name.to_lowercase());
            seen.push(scenario.name);
        }
        assert_eq!(
            iso_scenario("iso_nine.origin").map(|s| s.name),
            Some("iso_nine.origin")
        );
        assert_eq!(iso_scenario("iso_nine.nowhere"), None);
    }

    #[test]
    fn every_iso_scenario_names_its_own_ground() {
        // The measurement suite must never be random. `./render` picks a
        // fresh world every time and that is right for looking at pictures; a
        // benchmark that did it would have no history.
        for scenario in ISO_SCENARIOS {
            let frame = scenario.frame();
            assert_eq!(frame.layout.subject(), scenario.centre_tile);
            assert_eq!(frame.layout.len(), 9);
            assert_eq!(frame.output_size, scenario.output_size);
            // And resolving twice gives the same frame.
            assert_eq!(scenario.frame().cache_origin, frame.cache_origin);
        }
    }

    #[test]
    fn the_subject_tile_is_completely_covered() {
        // The one number here that has a right answer. The subject is the middle
        // of the frame and entirely inside the layout, so a coverage below one
        // is a hole in the silhouette rather than a look.
        let scenario = iso_scenario("iso_nine.origin").expect("pinned");
        let frame = scenario.frame();
        let image = synthetic_image(&frame, scenario.seed, 0.0);
        let report = measure(&frame, &image);
        assert!(
            report.subject_coverage > 0.999,
            "the subject is {:.3} covered",
            report.subject_coverage
        );
        // And the plate as a whole is mostly background: a diamond is half its
        // bounding box, and the layout fills ninety percent of the frame.
        assert!(
            (0.30..0.50).contains(&report.coverage),
            "the plate is {:.2} covered",
            report.coverage
        );
    }

    #[test]
    fn the_internal_joins_are_invisible() {
        // What this proves without a renderer: the ratio-based measurement
        // reports "no seam" for a field with no seam by construction. The claim
        // that an *actual* render has no seam is proven separately, against
        // `terrain compile` output — see AGENTS.md invariant 8.
        for scenario in ISO_SCENARIOS {
            let frame = scenario.frame();
            let image = synthetic_image(&frame, scenario.seed, 0.0);
            let visibility = join_visibility(&frame, &image);
            assert!(
                (0.5..1.5).contains(&visibility),
                "{}: the joins measure {visibility:.2} against the picture's own noise",
                scenario.name
            );
        }
    }

    #[test]
    fn the_subject_is_measured_apart_from_its_surroundings() {
        // A metric over the whole frame is eight parts set dressing. The pair is
        // what says whether a change is about the subject or about everything.
        let scenario = iso_scenario("iso_nine.origin").expect("pinned");
        let frame = scenario.frame();
        let image = synthetic_image(&frame, scenario.seed, 0.0);
        let report = measure(&frame, &image);

        assert!(report.detail > 0.0 && report.subject_detail > 0.0);
        assert!(report.luminance > 0.0 && report.subject_luminance > 0.0);
        // The subject is the same field as its neighbours — deliberately, since
        // a context tile that differed systematically is what a neural renderer
        // would learn instead of grass. So the two must land close together.
        assert!(
            (report.subject_luminance / report.luminance - 1.0).abs() < 0.25,
            "subject {:.3} against layout {:.3}",
            report.subject_luminance,
            report.luminance
        );
        assert!(
            (report.subject_detail / report.detail - 1.0).abs() < 0.35,
            "subject detail {:.4} against layout {:.4}",
            report.subject_detail,
            report.detail
        );
    }

    #[test]
    fn the_subject_figure_reads_the_subject_and_not_the_layout() {
        // The other half of the claim above, and the one a uniform field
        // cannot make: `the_subject_is_measured_apart_from_its_surroundings`
        // asserts the two numbers land *close*, which would also hold if
        // `subject_luminance` accidentally reported the layout figure twice.
        //
        // So lift the subject tile deliberately and check the subject figure
        // follows it further than the layout figure does. The subject is one
        // ninth of the diamond, so a lift of L moves the layout mean by about
        // L/9 and the subject mean by about L — an eightfold difference that
        // no amount of noise can produce by accident.
        let scenario = iso_scenario("iso_nine.origin").expect("pinned");
        let frame = scenario.frame();
        const LIFT: f32 = 0.2;

        let flat = measure(&frame, &synthetic_image(&frame, scenario.seed, 0.0));
        let lifted = measure(&frame, &synthetic_image(&frame, scenario.seed, LIFT));

        let subject_moved = lifted.subject_luminance - flat.subject_luminance;
        let layout_moved = lifted.luminance - flat.luminance;
        assert!(
            subject_moved > layout_moved * 4.0,
            "lifting the subject moved the subject figure by {subject_moved:.4} \
             and the layout figure by {layout_moved:.4} — the subject-weighted \
             metric is not reading the subject"
        );
        // And the layout figure did move, since the subject is inside it. A
        // subject mask that had drifted off the diamond entirely would leave
        // this at zero.
        assert!(
            layout_moved > 0.0,
            "the layout figure did not notice the subject at all"
        );
    }

    #[test]
    fn a_coarser_tile_holds_less_detail_per_metre() {
        // The reason two metres is the default. Four-metre tiles put the same
        // nine-tile diamond over four times the ground, so every blade is half
        // the width in pixels and the canopy averages toward a wash.
        let fine = iso_scenario("iso_nine.origin").expect("pinned");
        let coarse = iso_scenario("iso_nine.coarse_tiles").expect("pinned");
        assert!(coarse.tile_side_m > fine.tile_side_m);
        assert!(coarse.frame().pixels_per_metre < fine.frame().pixels_per_metre);
    }

    #[test]
    fn the_neighbours_grow_into_the_subject() {
        // The whole reason the eight context tiles exist, and it is not
        // decoration: a subject tile whose grass stopped at its own edge would
        // have four hard boundaries through the middle of the frame. One
        // continuous scene means marks rooted in a neighbour lean across.
        use terrain_generators::field::WorldField;
        use terrain_generators::iso;
        use terrain_generators::page::Page;
        use terrain_generators::scene::GrassScene;
        use terrain_generators::style::GrassParams;
        use terrain_scene::layout::TileRole;

        let scenario = iso_scenario("iso_nine.origin").expect("pinned");
        let frame = scenario.frame();
        let params = GrassParams {
            seed: scenario.seed,
            ..GrassParams::default()
        };
        let page = Page::at_detail(
            glam::Vec2::new(frame.cache_origin[0], frame.cache_origin[1]),
            frame.output_size[0] as usize,
            frame.output_size[1] as usize,
            frame.pixels_per_metre / iso::PX_PER_METRE,
        );
        let field = WorldField::lit_by(params.seed, params.light);
        let scene = GrassScene::build(page, &field, &params);
        let subject = frame.subject_polygon();

        let mut crossing = 0usize;
        for mark in &scene.marks {
            let root =
                terrain_core::coords::WorldPoint::new(mark.root.x as f64, mark.root.y as f64);
            if frame.layout.role_at(root) != Some(TileRole::Context) {
                continue;
            }
            // Where the mark's tip lands, which is what actually crosses the
            // join: a blade leans, and the projection lifts its tip up the
            // screen as well as along.
            let tip = page.to_pixel(mark.root + glam::Vec3::Z * mark.length);
            if subject.contains_px(tip.x, tip.y) {
                crossing += 1;
            }
        }
        assert!(
            crossing > 20,
            "only {crossing} marks rooted in a neighbour reach the subject tile"
        );
    }

    #[test]
    fn a_join_measurement_ignores_the_background() {
        // A control band that fell off the diamond would report the difference
        // between grass and nothing, which is enormous and meaningless.
        let scenario = iso_scenario("iso_nine.origin").expect("pinned");
        let frame = scenario.frame();
        let mut image = synthetic_image(&frame, scenario.seed, 0.0);
        image.alpha.fill(0.0);
        assert_eq!(
            join_visibility(&frame, &image),
            0.0,
            "a fully transparent plate reported a seam"
        );
    }
}

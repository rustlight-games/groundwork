//! One square metre of grass, with named things in it.
//!
//! Tuning the field by looking at the field is how this kind of work stalls. A
//! meadow shows blade shape, tuft organisation, palette, occlusion, shadow bias
//! and broad field variation all at once and all interacting, so a change that
//! improves one and damages two reads as "hmm, different" and gets kept.
//!
//! So the first thing built for a look change is this: a small plate with a
//! *known* population, laid out in a grid, where each cell answers one question.
//! A fork that does not read as one blade separating is obvious in cell four and
//! invisible in a meadow. A key light rotated ninety degrees has to swap which
//! side of every blade is lit, and a plate with eight upright twisted blades in
//! it says whether it did in one glance.
//!
//! ```sh
//! cargo run --release -p terrain_bench --example grass_lab
//! cargo run --release -p terrain_bench --example grass_lab -- --azimuth 90
//! cargo run --release -p terrain_bench --example grass_lab -- --quality preview
//! ```
//!
//! ## The layout is fixed on purpose
//!
//! Cells are indexed, not placed by taste, and the order is append-only for the
//! same reason [`terrain_generators::fixtures::PLACES`] is: a plate photographed last week
//! and one photographed today have to be pictures of the same experiment. Adding
//! a fixture goes on the end.

use glam::{Vec2, Vec3};

use terrain_bake::bake::{BakeParams, Macro, lay_floor};
use terrain_bake::painter::Painter;
use terrain_generators::field::WorldField;
use terrain_generators::geometry::TipProfile;
use terrain_generators::iso;
use terrain_generators::page::Page;
use terrain_generators::quality::GrassRenderQuality;
use terrain_generators::rng::{Draw, Stream};
use terrain_generators::stroke::{Profile, Stroke};
pub use terrain_generators::sun::{DEFAULT_ELEVATION, Key};
use terrain_generators::tone::Tone;

use terrain_bake::surface::Surface;

/// One question the plate answers.
///
/// Append only. See the module note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    /// Nothing. The floor, so the ground shader can be judged on its own.
    Bare,
    /// A single upright blade, unbent, as plain as the vocabulary gets.
    LoneBlade,
    /// Eight blades fanned around a point at rising twist. The cell that says
    /// whether rotating the key swaps the lit side.
    TwistFan,
    /// One broad mature blade with a split tip.
    ForkedBlade,
    /// Two blades crossing at a shallow angle, one in front of the other. Reads
    /// on contact shadowing and on whether the depth test separates them.
    Crossing,
    /// A tight tuft of fine blades.
    FineTuft,
    /// A loose tuft of broad mature blades — the population that forks.
    BroadTuft,
    /// A patch of bare earth with a fringe around it.
    DirtOpening,
    /// The low mat with nothing standing above it.
    LowMat,
    /// A blade rooted outside the cell, leaning in. Reads on the guard band.
    RootedOutside,
    /// A blade laid almost flat along the ground, curling back.
    Lodged,
    /// Three blades of rising width at the same length, for the width profile.
    WidthLadder,
}

impl Fixture {
    /// Every fixture, in plate order.
    pub const ALL: [Fixture; 12] = [
        Fixture::Bare,
        Fixture::LoneBlade,
        Fixture::TwistFan,
        Fixture::ForkedBlade,
        Fixture::Crossing,
        Fixture::FineTuft,
        Fixture::BroadTuft,
        Fixture::DirtOpening,
        Fixture::LowMat,
        Fixture::RootedOutside,
        Fixture::Lodged,
        Fixture::WidthLadder,
    ];

    /// A short stable name, for captions and filenames.
    pub const fn name(self) -> &'static str {
        match self {
            Fixture::Bare => "bare",
            Fixture::LoneBlade => "lone-blade",
            Fixture::TwistFan => "twist-fan",
            Fixture::ForkedBlade => "forked-blade",
            Fixture::Crossing => "crossing",
            Fixture::FineTuft => "fine-tuft",
            Fixture::BroadTuft => "broad-tuft",
            Fixture::DirtOpening => "dirt-opening",
            Fixture::LowMat => "low-mat",
            Fixture::RootedOutside => "rooted-outside",
            Fixture::Lodged => "lodged",
            Fixture::WidthLadder => "width-ladder",
        }
    }
}

/// How many cells across the plate is laid out.
pub const COLUMNS: usize = 4;

/// **Screen** metres one cell of the plate covers.
///
/// Screen, not world, and the difference is not pedantry. The projection is
/// area-preserving but not distance-preserving — a step of half a metre straight
/// across the screen is `0.5 / √2` of a metre of ground, because the world axes
/// run along the screen diagonals. The plate is laid out in the space it is
/// looked at in, so this is the unit that keeps the grid square.
pub const CELL_METRES: f32 = 0.5;

/// How far down its cell a fixture is rooted, as a fraction.
///
/// Not a half. Grass grows *up the screen*, so a fixture rooted in the middle of
/// its cell spends its whole length in the cell above and the top row runs off
/// the plate. Rooting low leaves the cell to hold what grows out of it, which is
/// the thing being looked at.
const ROOT_DOWN_CELL: f32 = 0.82;

/// Extra height above the top row, in cells.
///
/// The tallest thing on the plate is a third of a metre of blade, and a plate
/// exactly `rows` tall crops it. Cheaper than making every cell taller, which
/// would spread the fixtures apart for no gain.
const TOP_MARGIN_CELLS: f32 = 0.45;

/// The plate as a whole.
#[derive(Clone, Copy, Debug)]
pub struct Lab {
    pub seed: u64,
    pub key: Key,
    pub quality: GrassRenderQuality,
    /// Cache pixels per world metre. Independent of the page grid, because the
    /// plate is not a page — it is a photograph of an experiment, and it wants
    /// to be readable rather than to tile.
    pub px_per_metre: f32,
}

impl Default for Lab {
    fn default() -> Self {
        Self {
            seed: 0x1ab_0000,
            key: Key::default(),
            quality: GrassRenderQuality::Reference,
            // Twice the authoring scale. The plate exists to be looked at
            // closely, and every judgement it is for — does the fork read as one
            // blade separating, does the twist move the highlight — is about
            // features a few pixels across at the scale the game shows.
            px_per_metre: iso::PX_PER_METRE * 2.0,
        }
    }
}

impl Lab {
    /// Rows the plate needs to hold every fixture.
    pub const fn rows() -> usize {
        Fixture::ALL.len().div_ceil(COLUMNS)
    }

    /// The plate's size in final pixels.
    pub fn size(&self) -> (usize, usize) {
        let width = (COLUMNS as f32 * CELL_METRES * self.px_per_metre) as usize;
        let height =
            ((Self::rows() as f32 + TOP_MARGIN_CELLS) * CELL_METRES * self.px_per_metre) as usize;
        (width, height)
    }

    /// Where a cell's centre sits in world coordinates.
    ///
    /// The plate is laid out in *screen* rows and columns and then unprojected,
    /// so the picture is a tidy grid even though the ground under it is a
    /// parallelogram. A grid laid out in world coordinates would come back as a
    /// diamond with fixtures running off the corners.
    pub fn cell_centre(&self, index: usize) -> Vec2 {
        let (column, row) = (index % COLUMNS, index / COLUMNS);
        let pixel = Vec2::new(
            (column as f32 + 0.5) * CELL_METRES * self.px_per_metre,
            (row as f32 + ROOT_DOWN_CELL + TOP_MARGIN_CELLS) * CELL_METRES * self.px_per_metre,
        );
        iso::from_cache_ground_at(self.origin() + pixel, self.px_per_metre)
    }

    /// The plate's top-left corner, in its own cache pixels.
    pub fn origin(&self) -> Vec2 {
        let (width, height) = self.size();
        Vec2::new(-(width as f32) * 0.5, -(height as f32) * 0.5)
    }
}

/// The page a lab plate is baked as.
///
/// A real [`Page`], not a special case, so the plate goes through the floor
/// pass, the macro lattice and the resolve every shipping page does. A
/// laboratory whose pixels came out of a different pipeline would certify a
/// renderer nobody runs.
pub fn lab_page(lab: &Lab) -> Page {
    let (width, height) = lab.size();
    Page {
        origin: lab.origin(),
        width,
        height,
        px_per_metre: lab.px_per_metre,
    }
}

/// Bake the whole plate: floor, fixtures, resolve.
pub fn bake_lab(lab: &Lab, params: &BakeParams) -> Vec<Vec3> {
    let params = BakeParams {
        seed: lab.seed,
        light: lab.key.direction(),
        quality: lab.quality,
        ..params.clone()
    };
    let page = lab_page(lab);
    let field = WorldField::lit_by(params.seed, params.light);
    let lattice = Macro::build(&page, &field);
    let mut surface = Surface::at_supersample(page.width, page.height, lab.quality.supersample());

    // The fixtures go into a scene rather than straight onto the surface, so
    // the plate is lit and shadowed by exactly the path a page is. A laboratory
    // that skipped the shadow pass would certify a renderer nobody runs.
    let mut scene = terrain_generators::scene::GrassScene {
        page,
        marks: Vec::new(),
    };
    for (index, fixture) in Fixture::ALL.iter().enumerate() {
        plant_fixture(&mut scene.marks, lab, *fixture, lab.cell_centre(index));
    }

    lay_floor(&mut surface, &page, &field, &lattice);
    {
        let mut painter =
            Painter::at_scale(&mut surface, page.origin, params.light, page.px_per_metre)
                .with_ribs_per_pixel(lab.quality.ribs_per_pixel());
        painter.draw_scene(&scene);
    }
    let shadows = terrain_bake::bake::cast_shadows(&scene, &params);
    terrain_bake::bake::resolve_lit(&surface, &page, &lattice, &params, shadows.as_deref())
}

/// Grow one fixture onto a painter.
///
/// Deliberately takes the same [`Painter`] the field does, and builds the same
/// [`Stroke`] values. A laboratory that drew through its own path would be a
/// laboratory for a renderer nobody ships.
pub fn plant_fixture(marks: &mut Vec<Stroke>, lab: &Lab, fixture: Fixture, centre: Vec2) {
    let mut draw = Draw::from_seed(lab.seed ^ (fixture as u64) << 32);
    let blade = |draw: &mut Draw| Stroke {
        length: draw.range(0.16, 0.24),
        bend: draw.range(0.35, 0.7),
        width: draw.range(0.7, 1.6),
        twist: draw.signed() * 0.8,
        tip_light: 0.42,
        side_light: 0.34,
        ..Default::default()
    };

    match fixture {
        Fixture::Bare => {}

        Fixture::LoneBlade => {
            marks.push(Stroke {
                root: centre.extend(0.0),
                azimuth: 0.0,
                length: 0.26,
                bend: 0.45,
                width: 1.8,
                tip_light: 0.42,
                glint: 0.85,
                ..Default::default()
            });
        }

        Fixture::TwistFan => {
            // Eight blades around the compass, all the same shape, at rising
            // twist. Whatever differs between them across the cell is the
            // lighting talking, and that is the whole point of the fixture: turn
            // the key and the lit edge has to walk round with it.
            for step in 0..8 {
                let azimuth = step as f32 / 8.0 * std::f32::consts::TAU;
                marks.push(Stroke {
                    root: (centre + Vec2::from_angle(azimuth) * 0.03).extend(0.0),
                    azimuth,
                    length: 0.22,
                    bend: 0.55,
                    width: 1.7,
                    twist: step as f32 / 8.0 * std::f32::consts::PI,
                    tip_light: 0.42,
                    side_light: 0.42,
                    ..Default::default()
                });
            }
        }

        Fixture::ForkedBlade => {
            // Three, at rising split depth, so the cell says both whether a fork
            // reads as one blade separating and where along the blade it stops
            // doing so.
            for (step, split_at) in [0.74f32, 0.82, 0.90].into_iter().enumerate() {
                marks.push(Stroke {
                    root: (centre + Vec2::new(step as f32 * 0.08 - 0.08, 0.0)).extend(0.0),
                    azimuth: 0.2,
                    length: 0.30,
                    bend: 0.45,
                    width: 1.9,
                    tip_width: 0.4,
                    twist: 0.4,
                    tip: TipProfile::Forked {
                        split_at,
                        opening: 0.26,
                        long: (1.0 - split_at) + 0.05,
                        short: (1.0 - split_at) * 0.55,
                    },
                    tip_light: 0.42,
                    side_light: 0.42,
                    glint: 0.85,
                    ..Default::default()
                });
            }
        }

        Fixture::Crossing => {
            for (offset, azimuth) in [(-0.05f32, 0.6f32), (0.05, -0.6)] {
                marks.push(Stroke {
                    root: (centre + Vec2::new(offset, 0.0)).extend(0.0),
                    azimuth,
                    length: 0.26,
                    bend: 0.9,
                    width: 1.9,
                    tip_light: 0.42,
                    ..Default::default()
                });
            }
        }

        Fixture::FineTuft => {
            let heading = 0.4;
            for _ in 0..24 {
                let angle = draw.range(0.0, std::f32::consts::TAU);
                let offset = Vec2::from_angle(angle) * 0.05 * draw.unit().sqrt();
                let mut stroke = blade(&mut draw);
                stroke.root = (centre + offset).extend(0.0);
                stroke.azimuth = heading + draw.signed() * 0.9;
                stroke.width *= 0.7;
                stroke.length *= 0.8;
                marks.push(stroke);
            }
        }

        Fixture::BroadTuft => {
            let heading = -0.5;
            for _ in 0..18 {
                let angle = draw.range(0.0, std::f32::consts::TAU);
                let offset = Vec2::from_angle(angle) * 0.09 * draw.unit().sqrt();
                let mut stroke = blade(&mut draw);
                stroke.root = (centre + offset).extend(0.0);
                stroke.azimuth = heading + draw.signed() * 1.2;
                stroke.width *= 1.5;
                stroke.length *= 1.25;
                marks.push(stroke);
            }
        }

        Fixture::DirtOpening => {
            // The strokes only; the soil itself is the floor pass's business and
            // the plate lays it under this cell.
            for _ in 0..10 {
                let angle = draw.range(0.0, std::f32::consts::TAU);
                let offset = Vec2::from_angle(angle) * 0.18 * draw.unit().sqrt().max(0.55);
                let mut stroke = blade(&mut draw);
                stroke.root = (centre + offset).extend(0.0);
                stroke.azimuth = angle;
                stroke.bend += 0.5;
                stroke.length *= 0.6;
                stroke.tone = if draw.chance(0.3) {
                    Tone::Dry
                } else {
                    Tone::Grass
                };
                marks.push(stroke);
            }
        }

        Fixture::LowMat => {
            for _ in 0..90 {
                let offset = Vec2::new(draw.signed(), draw.signed()) * 0.2;
                marks.push(Stroke {
                    root: (centre + offset).extend(0.0),
                    azimuth: draw.range(0.0, std::f32::consts::TAU),
                    length: draw.range(0.05, 0.11),
                    bend: draw.range(1.1, 1.9),
                    curl: draw.range(0.0, 1.2),
                    width: draw.range(0.9, 1.5),
                    tone: Tone::Thatch,
                    base_light: 0.5,
                    tip_light: 0.14,
                    ..Default::default()
                });
            }
        }

        Fixture::RootedOutside => {
            // Rooted a third of a metre below the cell and leaning up into it.
            // Reads on whether the guard band admits it at all.
            for step in 0..5 {
                let across = (step as f32 - 2.0) * 0.06;
                marks.push(Stroke {
                    root: (centre + Vec2::new(across, 0.0) + Vec2::splat(0.22)).extend(0.0),
                    azimuth: std::f32::consts::PI * 1.25,
                    length: 0.34,
                    bend: 0.5,
                    width: 1.8,
                    tip_light: 0.42,
                    ..Default::default()
                });
            }
        }

        Fixture::Lodged => {
            for step in 0..6 {
                let along = (step as f32 - 2.5) * 0.05;
                marks.push(Stroke {
                    root: (centre + Vec2::new(along, -along)).extend(0.0),
                    azimuth: 0.9,
                    length: 0.33,
                    bend: 1.75,
                    curl: 1.1,
                    sway: 1.4,
                    width: 1.6,
                    tip_light: 0.3,
                    ..Default::default()
                });
            }
        }

        Fixture::WidthLadder => {
            for step in 0..3 {
                let across = (step as f32 - 1.0) * 0.11;
                marks.push(Stroke {
                    root: (centre + Vec2::new(across, -across)).extend(0.0),
                    azimuth: 0.0,
                    length: 0.26,
                    bend: 0.4,
                    width: 0.9 + step as f32 * 1.1,
                    tip_width: 0.3 + step as f32 * 0.2,
                    profile: if step == 2 {
                        Profile::Stem
                    } else {
                        Profile::Tapered
                    },
                    tip_light: 0.42,
                    ..Default::default()
                });
            }
        }
    }
    // Silence the unused-stream lint on fixtures that never draw randomly.
    let _ = Stream::Shape;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_has_its_own_name() {
        let mut names: Vec<&str> = Fixture::ALL.iter().map(|f| f.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Fixture::ALL.len(), "two fixtures share a name");
    }

    #[test]
    fn the_plate_holds_every_fixture() {
        let lab = Lab::default();
        assert!(Lab::rows() * COLUMNS >= Fixture::ALL.len());
        let (width, height) = lab.size();
        assert!(width > 0 && height > 0);
    }

    #[test]
    fn the_key_turns_without_changing_its_height() {
        // The property the whole rotation sweep rests on: turning the compass
        // must move the light around the sky, not raise or lower it. A rotation
        // that also changed elevation would make "the lit side followed the key"
        // impossible to tell from "the light got brighter".
        //
        // Measured in the **world**, which is the point. The same assertion
        // against the image vector's `z` passes trivially and means nothing,
        // because image `+Z` points at the viewer rather than at the sky.
        for step in 0..8 {
            let key = Key {
                azimuth: step as f32 / 8.0 * std::f32::consts::TAU,
                elevation: DEFAULT_ELEVATION,
            };
            assert!((key.direction().length() - 1.0).abs() < 1.0e-5);
            let elevation = iso::elevation_of(key.world());
            assert!(
                (elevation - DEFAULT_ELEVATION).abs() < 1.0e-3,
                "bearing {step} put the sun {}° above the ground, not {}°",
                elevation.to_degrees(),
                DEFAULT_ELEVATION.to_degrees()
            );
        }
    }

    #[test]
    fn the_key_sits_where_it_says_it_does() {
        // The bug this arithmetic exists to prevent, pinned. Building the image
        // vector as `(plane · cos θ, sin θ)` puts a "35° sun" at nearly 55° of
        // real elevation, and the shadow guard band — which is sized from one
        // over the tangent — comes out a third short of what the field casts.
        for degrees in [15.0f32, 25.0, 35.0, 45.0] {
            let key = Key {
                azimuth: 0.0,
                elevation: degrees.to_radians(),
            };
            let measured = iso::elevation_of(key.world()).to_degrees();
            assert!(
                (measured - degrees).abs() < 0.05,
                "a {degrees}° key sits at {measured}°"
            );
        }
    }

    #[test]
    fn an_unreachable_sun_clamps_to_the_highest_the_bearing_allows() {
        // With the screen bearing pinned, a sun that would have to be both high
        // and down-screen does not exist. At the field's own bearing the ceiling
        // is a little under 54°, and the important thing is that asking for more
        // gives a real light at that height rather than a vector that is
        // silently not a unit, not on the bearing, or not a number at all — all
        // three of which an earlier arithmetic managed.
        let ceiling = Key {
            azimuth: 0.0,
            elevation: std::f32::consts::FRAC_PI_2,
        };
        let reached = iso::elevation_of(ceiling.world()).to_degrees();
        assert!(
            (50.0..56.0).contains(&reached),
            "the bearing's ceiling came out at {reached}°"
        );
        for degrees in [60.0f32, 75.0, 89.0] {
            let key = Key {
                azimuth: 0.0,
                elevation: degrees.to_radians(),
            };
            let direction = key.direction();
            assert!(direction.is_finite(), "{degrees}° gave {direction:?}");
            assert!((direction.length() - 1.0).abs() < 1.0e-4);
            let measured = iso::elevation_of(key.world()).to_degrees();
            assert!(
                (measured - reached).abs() < 0.5,
                "{degrees}° clamped to {measured}° rather than the {reached}° \
                 ceiling"
            );
        }
        // And the tier the renderer actually ships at is comfortably inside it.
        assert!(DEFAULT_ELEVATION.to_degrees() < reached);
    }

    #[test]
    fn a_quarter_turn_moves_the_light_a_quarter_of_the_way_round() {
        let start = Key::default().plane();
        let quarter = Key {
            azimuth: std::f32::consts::FRAC_PI_2,
            ..Key::default()
        }
        .plane();
        assert!(
            start.dot(quarter).abs() < 1.0e-4,
            "a quarter turn left the light {start:?} → {quarter:?}"
        );
    }

    #[test]
    fn the_default_key_is_the_field_s_own_bearing() {
        // Zero bearing has to mean "where the sun already is", or every plate
        // taken at the default would be lit from somewhere the meadow is not —
        // and the mound field, which shades its own domes, would disagree with
        // every mark's under-stroke about where that was.
        let direction = Key::default().direction();
        let plane = Vec2::new(direction.x, direction.y).normalize();
        assert!(plane.distance(terrain_generators::field::LIGHT_PLANE) < 1.0e-4);
        // The bearing survives every elevation, which is what lets the sun be
        // lowered without recomposing the picture.
        for degrees in [25.0f32, 35.0, 55.0] {
            let key = Key {
                azimuth: 0.0,
                elevation: degrees.to_radians(),
            };
            let plane = Vec2::new(key.direction().x, key.direction().y).normalize();
            assert!(
                plane.distance(terrain_generators::field::LIGHT_PLANE) < 1.0e-3,
                "a {degrees}° sun moved the screen bearing to {plane:?}"
            );
        }
    }

    #[test]
    fn the_supported_sun_is_the_one_the_guard_band_is_sized_for() {
        // Thirty-five degrees, stated once. `bake` derives its shadow guard from
        // the light it is given, and this is the lowest that guard is proven
        // against, so a plate lit lower than this is outside the tested range.
        assert!((DEFAULT_ELEVATION.to_degrees() - 35.0).abs() < 1.0e-4);
    }

    #[test]
    fn cells_do_not_overlap_on_the_plate() {
        // Measured in the plate's own pixels rather than in metres of ground,
        // because the plate is a picture. Two cells a comfortable distance apart
        // in world coordinates can still land on top of each other on screen —
        // the world axes run along the screen diagonals, so a world step of one
        // metre covers anywhere between zero and two metres of screen depending
        // which way it points.
        let lab = Lab::default();
        let on_plate =
            |index: usize| iso::to_cache_at(lab.cell_centre(index).extend(0.0), lab.px_per_metre);
        let side = CELL_METRES * lab.px_per_metre;
        for a in 0..Fixture::ALL.len() {
            for b in (a + 1)..Fixture::ALL.len() {
                let apart = (on_plate(a) - on_plate(b)).abs();
                assert!(
                    apart.x > side * 0.9 || apart.y > side * 0.9,
                    "cells {a} and {b} are {apart:?} px apart on a {side:.0} px grid"
                );
            }
        }
    }

    #[test]
    fn the_plate_is_laid_out_in_a_square_grid() {
        // The layout goes through an unprojection and back, and it is easy to
        // write one that comes out as a diamond with fixtures off the corners.
        let lab = Lab::default();
        let on_plate =
            |index: usize| iso::to_cache_at(lab.cell_centre(index).extend(0.0), lab.px_per_metre);
        let side = CELL_METRES * lab.px_per_metre;
        // Along a row, only x moves; down a column, only y.
        let across = on_plate(1) - on_plate(0);
        assert!(
            (across.x - side).abs() < 0.5 && across.y.abs() < 0.5,
            "{across:?}"
        );
        let down = on_plate(COLUMNS) - on_plate(0);
        assert!(
            (down.y - side).abs() < 0.5 && down.x.abs() < 0.5,
            "{down:?}"
        );
    }
}

//! The world tiles a render is *about*.
//!
//! A render used to be an arbitrary rectangle of projected ground: an origin in
//! cache pixels, a scale, and whatever happened to fall inside. That is the
//! right shape for a laboratory plate and the wrong shape for a game, because a
//! game's ground is made of tiles and one of them is the subject. Nothing in a
//! rectangle says which part of the picture the render was *for*.
//!
//! So a layout is a named arrangement of square world tiles with exactly one
//! subject. Nine of them, three by three, subject in the middle, and the eight
//! neighbours are set dressing — they exist so the subject has grass leaning
//! into it, shadows falling across it, and a colour field that does not stop at
//! its edge.
//!
//! ## What a tile is not
//!
//! Four words in this repository mean four different things and three of them
//! were already taken:
//!
//! - A **world tile** is one semantic square of terrain. This module.
//! - A **plate** is one finished image. See `terrain_cycles::plate`.
//! - A **page** is a rectangular unit of runtime cache. See
//!   `terrain_generators::page::Page`.
//! - A **trace tile** is a slice of a plate small enough for Blender to hold.
//!   An implementation detail of getting one plate traced.
//!
//! A nine-tile layout is *not* nine pages, and it is *not* a trace-tile split.
//! It is nine regions of one continuous world, rendered by one camera, from one
//! scene. That last part is the whole design: the tiles are a spatial and
//! semantic division, never a generation boundary. Nine independently generated
//! scenes composited together would have a visible join at every internal edge
//! and no shadow would ever cross one.
//!
//! ## Why coordinates and not a count
//!
//! `tile_count: usize` would be the obvious model and it says nothing. Nine is
//! three by three; twenty-seven could be three by nine, or three layers of nine,
//! or a hexagonal ring. A layout is therefore an explicit list of coordinates,
//! and a preset is a function that produces one. Adding a shape later changes
//! this module and nothing downstream — the resolver, the camera and the
//! renderers all read the coordinate list.
//!
//! ## The tile grid is not the terrain's grid
//!
//! This lives in `terrain_scene`, not in `terrain_core`, and deliberately. The
//! terrain is a continuous function of world position with no preferred origin
//! and no preferred resolution; giving it a tile size would make two documents
//! authored at different scales disagree about where anything is. A layout is a
//! *rendering and composition request* laid over that continuous function, which
//! is why it sits with the projection and the scene rather than with the
//! sampler.

use terrain_core::coords::{WorldPoint, WorldRect};
use terrain_core::digest::{Digest, Digestible};

/// Which square of the world, in whole tiles.
///
/// Integers, so a tile has an exact identity that survives being written to a
/// manifest and typed back in. A render that could only be reproduced from a
/// float pair would be a render nobody reproduces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct WorldTileCoord {
    pub u: i64,
    pub v: i64,
}

impl WorldTileCoord {
    pub const ORIGIN: Self = Self { u: 0, v: 0 };

    pub const fn new(u: i64, v: i64) -> Self {
        Self { u, v }
    }

    /// This tile, offset by whole tiles.
    pub const fn offset(self, du: i64, dv: i64) -> Self {
        Self {
            u: self.u + du,
            v: self.v + dv,
        }
    }
}

impl std::fmt::Display for WorldTileCoord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{}", self.u, self.v)
    }
}

impl std::str::FromStr for WorldTileCoord {
    type Err = String;

    /// Parse `U,V`.
    ///
    /// Negative coordinates are the common case rather than an edge case — a
    /// random centre tile is negative on each axis half the time — so this is
    /// tested against them rather than against the happy path.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (u, v) = text
            .split_once(',')
            .ok_or_else(|| format!("a tile coordinate is `U,V`, not `{text}`"))?;
        let parse = |part: &str, axis: char| {
            part.trim()
                .parse::<i64>()
                .map_err(|_| format!("the {axis} of `{text}` is not a whole number"))
        };
        Ok(Self::new(parse(u, 'U')?, parse(v, 'V')?))
    }
}

impl Digestible for WorldTileCoord {
    fn absorb(&self, digest: &mut Digest) {
        digest.i64(self.u).i64(self.v);
    }
}

/// What a tile is in the picture.
///
/// Metadata, and only metadata. Both roles are generated identically, at the
/// same density and the same quality — see the module note. A context tile that
/// were thinner, blurrier or darker than the subject would put a systematic
/// difference one tile away from the middle of every frame, which is precisely
/// the sort of artefact a neural renderer learns instead of learning grass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileRole {
    /// The tile the render is about.
    Subject,
    /// Set dressing: present so the subject has a neighbourhood.
    Context,
}

impl TileRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Context => "context",
        }
    }
}

/// One tile of a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldTile {
    pub coord: WorldTileCoord,
    pub role: TileRole,
}

/// The arrangements a render can ask for by name.
///
/// One, so far, and that is honest rather than unfinished: twenty-seven tiles
/// is not a number, it is a shape nobody has chosen yet. When one is chosen it
/// arrives here as a variant and the rest of the pipeline does not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TileLayoutPreset {
    /// Three by three, subject in the middle.
    #[default]
    Nine,
}

impl TileLayoutPreset {
    pub fn name(self) -> &'static str {
        match self {
            Self::Nine => "nine",
        }
    }

    /// Build the layout, around a subject.
    pub fn layout(
        self,
        subject: WorldTileCoord,
        tile_side_m: f64,
    ) -> Result<IsoTileLayout, LayoutError> {
        match self {
            Self::Nine => IsoTileLayout::nine(subject, tile_side_m),
        }
    }
}

impl std::str::FromStr for TileLayoutPreset {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "nine" | "9" | "3x3" => Ok(Self::Nine),
            other => Err(format!("unknown layout `{other}`; expected nine")),
        }
    }
}

/// What is wrong with a layout that will not build.
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutError {
    /// A tile side that is zero, negative, or not a number.
    TileSide(f64),
    /// No tiles at all.
    Empty,
    /// The same coordinate twice.
    ///
    /// Refused rather than deduplicated: a duplicate means the caller's geometry
    /// is wrong, and quietly rendering eight tiles when nine were asked for is
    /// the kind of thing nobody notices until the manifest is read months later.
    Duplicate(WorldTileCoord),
    /// Not exactly one subject.
    Subjects(usize),
    /// The named subject is not one of the tiles.
    SubjectMissing(WorldTileCoord),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TileSide(side) => write!(f, "a tile side of {side} m is not a length"),
            Self::Empty => write!(f, "a layout with no tiles renders nothing"),
            Self::Duplicate(coord) => write!(f, "tile {coord} appears twice"),
            Self::Subjects(count) => write!(
                f,
                "a layout needs exactly one subject tile, and this one has {count}"
            ),
            Self::SubjectMissing(coord) => {
                write!(f, "the subject {coord} is not one of the layout's tiles")
            }
        }
    }
}

impl std::error::Error for LayoutError {}

/// A named arrangement of square world tiles, with one subject.
///
/// The fields are private and the constructor validates, because two of the
/// invariants are load-bearing rather than tidy: [`IsoTileLayout::subject_bounds`]
/// would have no answer without exactly one subject, and a duplicated
/// coordinate would make the tile-outline overlay draw one edge twice at double
/// weight, which reads as a deliberate emphasis.
#[derive(Clone, Debug, PartialEq)]
pub struct IsoTileLayout {
    tile_side_m: f64,
    tiles: Vec<WorldTile>,
    subject: WorldTileCoord,
}

impl IsoTileLayout {
    /// A layout from an explicit tile list.
    pub fn new(
        subject: WorldTileCoord,
        tile_side_m: f64,
        tiles: Vec<WorldTile>,
    ) -> Result<Self, LayoutError> {
        if !tile_side_m.is_finite() || tile_side_m <= 0.0 {
            return Err(LayoutError::TileSide(tile_side_m));
        }
        if tiles.is_empty() {
            return Err(LayoutError::Empty);
        }
        for (index, tile) in tiles.iter().enumerate() {
            if tiles[..index].iter().any(|seen| seen.coord == tile.coord) {
                return Err(LayoutError::Duplicate(tile.coord));
            }
        }
        let subjects = tiles
            .iter()
            .filter(|tile| tile.role == TileRole::Subject)
            .count();
        if subjects != 1 {
            return Err(LayoutError::Subjects(subjects));
        }
        if !tiles
            .iter()
            .any(|tile| tile.coord == subject && tile.role == TileRole::Subject)
        {
            return Err(LayoutError::SubjectMissing(subject));
        }
        Ok(Self {
            tile_side_m,
            tiles,
            subject,
        })
    }

    /// Three by three, subject in the middle.
    ///
    /// The order the tiles come out in is row-major from `-1,-1`, and it is
    /// stable because the debug overlay draws in it and the manifest lists it.
    pub fn nine(subject: WorldTileCoord, tile_side_m: f64) -> Result<Self, LayoutError> {
        let mut tiles = Vec::with_capacity(9);
        for dv in -1..=1 {
            for du in -1..=1 {
                tiles.push(WorldTile {
                    coord: subject.offset(du, dv),
                    role: if du == 0 && dv == 0 {
                        TileRole::Subject
                    } else {
                        TileRole::Context
                    },
                });
            }
        }
        Self::new(subject, tile_side_m, tiles)
    }

    pub fn tile_side_m(&self) -> f64 {
        self.tile_side_m
    }

    pub fn tiles(&self) -> &[WorldTile] {
        &self.tiles
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn subject(&self) -> WorldTileCoord {
        self.subject
    }

    /// The half-open world rectangle a tile occupies.
    ///
    /// Half-open, like every other rectangle here: the tile at `u` owns
    /// `[u·S, (u+1)·S)`, so two neighbours meet exactly and no point belongs to
    /// both. A closed rectangle would put the join in both tiles, and any
    /// per-tile tally — marks rooted inside, bare-ground share — would
    /// double-count a line of ground down every internal edge.
    pub fn tile_bounds(&self, coord: WorldTileCoord) -> WorldRect {
        let side = self.tile_side_m;
        WorldRect::new(
            WorldPoint::new(coord.u as f64 * side, coord.v as f64 * side),
            WorldPoint::new((coord.u + 1) as f64 * side, (coord.v + 1) as f64 * side),
        )
    }

    /// Everything the render shows, as one rectangle.
    ///
    /// The union of the tile rectangles. For the nine-tile preset that is
    /// exactly `3S × 3S`, but this is computed rather than assumed so a layout
    /// that is not a filled square still frames correctly.
    pub fn visible_bounds(&self) -> WorldRect {
        let mut bounds = self.tile_bounds(self.tiles[0].coord);
        for tile in &self.tiles[1..] {
            bounds = bounds.union(self.tile_bounds(tile.coord));
        }
        bounds
    }

    /// The subject tile's own rectangle.
    pub fn subject_bounds(&self) -> WorldRect {
        self.tile_bounds(self.subject)
    }

    /// The middle of the subject tile.
    ///
    /// What the camera is aimed at. For a symmetric layout it is also the middle
    /// of [`IsoTileLayout::visible_bounds`], and a test pins that — but the two
    /// are different questions and only this one stays right when the layout
    /// stops being symmetric.
    pub fn subject_centre(&self) -> WorldPoint {
        self.subject_bounds().centre()
    }

    /// Which tile a world point falls in, whether or not the layout holds it.
    ///
    /// Floored division, so a point at `-0.5 m` with a 4-metre tile is in tile
    /// `-1` rather than in tile `0`. Truncating toward zero would make the two
    /// tiles either side of the origin share one index and every world be
    /// wrong near its own centre.
    pub fn tile_at(&self, point: WorldPoint) -> WorldTileCoord {
        let side = self.tile_side_m;
        WorldTileCoord::new(
            (point.u_m / side).floor() as i64,
            (point.v_m / side).floor() as i64,
        )
    }

    /// The role of the tile a point falls in, or `None` outside the layout.
    pub fn role_at(&self, point: WorldPoint) -> Option<TileRole> {
        let coord = self.tile_at(point);
        self.tiles
            .iter()
            .find(|tile| tile.coord == coord)
            .map(|tile| tile.role)
    }
}

impl Digestible for IsoTileLayout {
    fn absorb(&self, digest: &mut Digest) {
        digest.f64(self.tile_side_m);
        self.subject.absorb(digest);
        digest.slice(&self.tiles, |d, tile| {
            tile.coord.absorb(d);
            d.u32(tile.role as u32);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nine() -> IsoTileLayout {
        IsoTileLayout::nine(WorldTileCoord::new(-713, 284), 4.0).expect("well formed")
    }

    #[test]
    fn the_nine_preset_is_nine_unique_tiles_around_one_subject() {
        let layout = nine();
        assert_eq!(layout.len(), 9);
        let mut coords: Vec<_> = layout.tiles().iter().map(|tile| tile.coord).collect();
        coords.sort();
        coords.dedup();
        assert_eq!(coords.len(), 9, "a coordinate appeared twice");
        assert_eq!(
            layout
                .tiles()
                .iter()
                .filter(|tile| tile.role == TileRole::Subject)
                .count(),
            1
        );
        assert_eq!(layout.subject(), WorldTileCoord::new(-713, 284));
    }

    #[test]
    fn the_nine_preset_is_exactly_the_offsets_minus_one_to_one() {
        let subject = WorldTileCoord::new(5, -2);
        let layout = IsoTileLayout::nine(subject, 4.0).expect("well formed");
        for dv in -1..=1 {
            for du in -1..=1 {
                let wanted = subject.offset(du, dv);
                let tile = layout
                    .tiles()
                    .iter()
                    .find(|tile| tile.coord == wanted)
                    .unwrap_or_else(|| panic!("{wanted} is missing"));
                let expected = if du == 0 && dv == 0 {
                    TileRole::Subject
                } else {
                    TileRole::Context
                };
                assert_eq!(tile.role, expected, "{wanted}");
            }
        }
    }

    #[test]
    fn the_visible_bounds_are_three_tiles_on_each_axis() {
        let layout = nine();
        let bounds = layout.visible_bounds();
        assert!((bounds.width_m() - 12.0).abs() < 1.0e-9);
        assert!((bounds.height_m() - 12.0).abs() < 1.0e-9);
        assert!((layout.subject_bounds().width_m() - 4.0).abs() < 1.0e-9);
    }

    #[test]
    fn neighbouring_tiles_meet_exactly() {
        // The property the whole silhouette rests on. A gap shows as a hairline
        // of background down the middle of the picture; an overlap double-counts
        // a line of ground in every per-tile measurement.
        let layout = nine();
        let left = layout.tile_bounds(WorldTileCoord::new(-714, 284));
        let middle = layout.tile_bounds(WorldTileCoord::new(-713, 284));
        assert_eq!(left.max.u_m, middle.min.u_m);
        assert_eq!(left.min.v_m, middle.min.v_m);
        assert_eq!(left.max.v_m, middle.max.v_m);
    }

    #[test]
    fn the_subject_sits_in_the_middle_of_a_symmetric_layout() {
        // True of the nine preset and not of layouts in general, which is why
        // the camera is aimed at the subject's centre rather than the union's.
        let layout = nine();
        let subject = layout.subject_centre();
        let union = layout.visible_bounds().centre();
        assert!((subject.u_m - union.u_m).abs() < 1.0e-9);
        assert!((subject.v_m - union.v_m).abs() < 1.0e-9);
    }

    #[test]
    fn a_tile_is_half_open_and_floors() {
        // Truncating toward zero would make the two tiles either side of the
        // origin share an index, and every world would be wrong near its centre.
        let layout = IsoTileLayout::nine(WorldTileCoord::ORIGIN, 4.0).expect("well formed");
        assert_eq!(
            layout.tile_at(WorldPoint::new(0.0, 0.0)),
            WorldTileCoord::new(0, 0)
        );
        assert_eq!(
            layout.tile_at(WorldPoint::new(-0.5, -0.5)),
            WorldTileCoord::new(-1, -1)
        );
        assert_eq!(
            layout.tile_at(WorldPoint::new(3.999, 3.999)),
            WorldTileCoord::new(0, 0)
        );
        assert_eq!(
            layout.tile_at(WorldPoint::new(4.0, 4.0)),
            WorldTileCoord::new(1, 1)
        );
    }

    #[test]
    fn a_point_reports_the_role_of_the_tile_it_is_in() {
        let layout = IsoTileLayout::nine(WorldTileCoord::ORIGIN, 4.0).expect("well formed");
        assert_eq!(
            layout.role_at(WorldPoint::new(2.0, 2.0)),
            Some(TileRole::Subject)
        );
        assert_eq!(
            layout.role_at(WorldPoint::new(-2.0, 2.0)),
            Some(TileRole::Context)
        );
        // Outside the nine, and honestly so rather than clamped to the nearest.
        assert_eq!(layout.role_at(WorldPoint::new(-9.0, 0.0)), None);
    }

    #[test]
    fn a_malformed_layout_is_refused_rather_than_repaired() {
        let subject = WorldTileCoord::ORIGIN;
        let one = |role| {
            vec![WorldTile {
                coord: subject,
                role,
            }]
        };
        assert_eq!(
            IsoTileLayout::nine(subject, 0.0),
            Err(LayoutError::TileSide(0.0))
        );
        // Compared by shape rather than by value: a NaN is not equal to itself,
        // so `assert_eq!` on the error would fail on the very case it is meant
        // to be checking.
        assert!(matches!(
            IsoTileLayout::nine(subject, f64::NAN),
            Err(LayoutError::TileSide(side)) if side.is_nan()
        ));
        assert_eq!(
            IsoTileLayout::new(subject, 4.0, Vec::new()),
            Err(LayoutError::Empty)
        );
        assert_eq!(
            IsoTileLayout::new(subject, 4.0, one(TileRole::Context)),
            Err(LayoutError::Subjects(0))
        );
        assert_eq!(
            IsoTileLayout::new(
                subject,
                4.0,
                vec![
                    WorldTile {
                        coord: subject,
                        role: TileRole::Subject
                    },
                    WorldTile {
                        coord: subject,
                        role: TileRole::Context
                    },
                ]
            ),
            Err(LayoutError::Duplicate(subject))
        );
        assert_eq!(
            IsoTileLayout::new(WorldTileCoord::new(9, 9), 4.0, one(TileRole::Subject)),
            Err(LayoutError::SubjectMissing(WorldTileCoord::new(9, 9)))
        );
    }

    #[test]
    fn a_tile_coordinate_survives_being_written_down_and_typed_back() {
        // The reproduction path. A centre tile that could not round-trip through
        // a manifest and a command line would make every random render a
        // one-off.
        for coord in [
            WorldTileCoord::new(0, 0),
            WorldTileCoord::new(-713, 284),
            WorldTileCoord::new(2047, -2048),
        ] {
            assert_eq!(coord.to_string().parse::<WorldTileCoord>(), Ok(coord));
        }
        assert!("-713, 284".parse::<WorldTileCoord>().is_ok());
        assert!("713".parse::<WorldTileCoord>().is_err());
        assert!("a,b".parse::<WorldTileCoord>().is_err());
    }

    #[test]
    fn a_preset_parses_from_the_name_it_prints() {
        assert_eq!(
            TileLayoutPreset::Nine.name().parse(),
            Ok(TileLayoutPreset::Nine)
        );
        assert!("twenty-seven".parse::<TileLayoutPreset>().is_err());
    }

    #[test]
    fn every_part_of_a_layout_reaches_its_digest() {
        let fingerprint = |layout: &IsoTileLayout| {
            let mut digest = Digest::for_domain("test");
            layout.absorb(&mut digest);
            digest.finish()
        };
        let base = nine();
        assert_ne!(
            fingerprint(&base),
            fingerprint(&IsoTileLayout::nine(WorldTileCoord::new(-713, 285), 4.0).unwrap()),
            "the subject did not reach the digest"
        );
        assert_ne!(
            fingerprint(&base),
            fingerprint(&IsoTileLayout::nine(WorldTileCoord::new(-713, 284), 5.0).unwrap()),
            "the tile side did not reach the digest"
        );
    }
}

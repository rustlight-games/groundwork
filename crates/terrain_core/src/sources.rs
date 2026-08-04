//! The fields a document can name, made evaluable.
//!
//! Two built-ins so far, and they are the two that unlock the layer model:
//! noise, which needs no asset, and spline distance, which is what a path is.
//!
//! ## Noise is a function of world position, not of texel
//!
//! Frequency is per metre. Bake the same ground at half the resolution and the
//! noise is the same noise, larger in texels and identical on the ground. Any
//! other convention makes a level-of-detail change a *content* change, and then
//! the wide view and the close-up are different worlds.
//!
//! It also reads the sample footprint, and drops octaves whose wavelength falls
//! below it. That is not an optimisation — an octave finer than the sampling
//! rate cannot be filtered, only sampled, so it turns into noise that crawls
//! when the ground moves under the grid. Dropping it is the only correct
//! response.
//!
//! ## A spline is a polyline, and that is on purpose
//!
//! Not a Bézier, not a Catmull-Rom. Distance to a curve has no closed form and
//! is solved by subdivision anyway, so a curve buys an authoring convenience at
//! the cost of every consumer having to agree about the subdivision — and two
//! consumers that subdivide differently put the path edge in two places.
//!
//! A polyline is subdivided once, by the author's tool, and every consumer
//! measures against the same segments. The authoring convenience is real and
//! belongs in whatever draws the spline, not in what samples it.

use crate::coords::{WorldPoint, WorldRect};
use crate::registry::ScalarField;
use crate::sample::SampleFootprint;
use crate::seed::{key_hash, mix};

// ---------------------------------------------------------------------------
// Noise
// ---------------------------------------------------------------------------

/// Value noise over a world-space lattice.
///
/// Value rather than gradient noise, and the reason is reproducibility rather
/// than quality: a value lattice is one hash per corner and a smoothstep, so it
/// is exactly specifiable in a sentence and cannot drift between an
/// implementation here and one in a shader. Perlin's gradients are better
/// looking and take a table that both sides have to agree about.
pub struct NoiseField {
    /// Lattice cells per metre.
    frequency_per_m: f64,
    octaves: u32,
    lacunarity: f64,
    gain: f64,
    /// The stream's hash, mixed into every lattice corner.
    seed: u64,
    /// Whether the output is remapped from `-1..1` to `0..1`.
    unsigned: bool,
}

impl NoiseField {
    pub fn new(
        frequency_per_m: f64,
        octaves: u32,
        lacunarity: f64,
        gain: f64,
        stream: &str,
        root_seed: u64,
    ) -> Self {
        Self {
            frequency_per_m: if frequency_per_m.is_finite() && frequency_per_m > 0.0 {
                frequency_per_m
            } else {
                1.0
            },
            octaves: octaves.clamp(1, 12),
            lacunarity: if lacunarity.is_finite() && lacunarity > 1.0 {
                lacunarity
            } else {
                2.0
            },
            gain: if gain.is_finite() && gain > 0.0 && gain < 1.0 {
                gain
            } else {
                0.5
            },
            seed: mix(root_seed ^ key_hash(stream)),
            unsigned: false,
        }
    }

    /// Remap the output to `0..1`.
    pub fn unsigned(mut self) -> Self {
        self.unsigned = true;
        self
    }

    /// One lattice corner's value, in `-1..1`.
    fn corner(&self, x: i64, y: i64) -> f64 {
        let mut state = mix(self.seed ^ x as u64);
        state = mix(state ^ y as u64);
        // The top 53 bits, scaled to `-1..1`.
        (state >> 11) as f64 * (2.0 / (1u64 << 53) as f64) - 1.0
    }

    /// One octave, bilinearly interpolated with a smoothstep.
    fn octave(&self, u: f64, v: f64) -> f64 {
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);

        // Smoothstep, so the lattice does not show as a grid of creases. A
        // linear interpolation has a discontinuous derivative at every cell
        // boundary, and the eye finds those immediately.
        let sx = fx * fx * (3.0 - 2.0 * fx);
        let sy = fy * fy * (3.0 - 2.0 * fy);

        let a = self.corner(x0, y0);
        let b = self.corner(x0 + 1, y0);
        let c = self.corner(x0, y0 + 1);
        let d = self.corner(x0 + 1, y0 + 1);
        let top = a + (b - a) * sx;
        let bottom = c + (d - c) * sx;
        top + (bottom - top) * sy
    }
}

impl ScalarField for NoiseField {
    fn value_at(&self, point: WorldPoint, footprint: SampleFootprint) -> f32 {
        // Below this wavelength an octave cannot be filtered, only sampled, so
        // it crawls when the ground moves under the grid. Two samples per
        // wavelength is the Nyquist limit; the factor of two is the margin that
        // stops an octave right at the limit beating against the grid.
        let radius = footprint.radius_m();
        let mut frequency = self.frequency_per_m;
        let mut amplitude = 1.0;
        let mut total = 0.0;
        let mut normaliser = 0.0;

        for _ in 0..self.octaves {
            let wavelength = 1.0 / frequency;
            if radius > 0.0 && wavelength < radius * 4.0 {
                break;
            }
            total += self.octave(point.u_m * frequency, point.v_m * frequency) * amplitude;
            normaliser += amplitude;
            frequency *= self.lacunarity;
            amplitude *= self.gain;
        }

        if normaliser <= 0.0 {
            // Every octave was below the sampling rate. The honest answer is the
            // field's mean rather than nothing.
            return if self.unsigned { 0.5 } else { 0.0 };
        }
        let value = total / normaliser;
        (if self.unsigned {
            value * 0.5 + 0.5
        } else {
            value
        }) as f32
    }

    fn describe(&self) -> String {
        format!(
            "noise {:.4}/m, {} octaves",
            self.frequency_per_m, self.octaves
        )
    }
}

// ---------------------------------------------------------------------------
// Splines
// ---------------------------------------------------------------------------

/// An authored polyline.
///
/// See the module note for why this is a polyline rather than a curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spline {
    pub points: Vec<WorldPoint>,
    pub closed: bool,
}

/// Why a spline asset could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplineError {
    /// Fewer than two points, so there is no segment to measure against.
    TooShort,
    /// A line that is neither a comment, a directive, nor a coordinate pair.
    BadLine { line: usize, text: String },
    /// A coordinate that is not a finite number.
    BadNumber { line: usize, text: String },
}

impl std::fmt::Display for SplineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "a spline needs at least two points"),
            Self::BadLine { line, text } => {
                write!(
                    f,
                    "line {line}: {text:?} is not `u v`, `closed`, or a comment"
                )
            }
            Self::BadNumber { line, text } => {
                write!(f, "line {line}: {text:?} is not a finite number")
            }
        }
    }
}

impl std::error::Error for SplineError {}

impl Spline {
    /// Read a spline from its text form.
    ///
    /// One `u v` pair per line, `#` for a comment, `closed` on its own line to
    /// join the ends. Deliberately not RON.
    ///
    /// A path is dozens to hundreds of points and is produced by a tool rather
    /// than typed. A line-based format diffs one point per line — so a version
    /// control history shows *which part of the path moved* — where a RON array
    /// reflows and shows the whole thing. It also keeps this parser inside
    /// `terrain_core`, which takes no deserialiser, so sampling a spline does not
    /// drag a format crate into every consumer.
    pub fn parse(text: &str) -> Result<Self, SplineError> {
        let mut points = Vec::new();
        let mut closed = false;
        for (index, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line == "closed" {
                closed = true;
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(u), Some(v)) = (parts.next(), parts.next()) else {
                return Err(SplineError::BadLine {
                    line: index + 1,
                    text: line.to_string(),
                });
            };
            if parts.next().is_some() {
                return Err(SplineError::BadLine {
                    line: index + 1,
                    text: line.to_string(),
                });
            }
            let parse = |t: &str| -> Result<f64, SplineError> {
                t.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| SplineError::BadNumber {
                        line: index + 1,
                        text: t.to_string(),
                    })
            };
            points.push(WorldPoint::new(parse(u)?, parse(v)?));
        }
        if points.len() < 2 {
            return Err(SplineError::TooShort);
        }
        Ok(Self { points, closed })
    }

    /// Segments, as index pairs.
    pub fn segments(&self) -> impl Iterator<Item = (WorldPoint, WorldPoint)> + '_ {
        let last = if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        };
        (0..last).map(move |i| (self.points[i], self.points[(i + 1) % self.points.len()]))
    }

    /// The rectangle the spline occupies.
    pub fn bounds(&self) -> WorldRect {
        let mut min = self.points[0];
        let mut max = self.points[0];
        for point in &self.points {
            min = WorldPoint::new(min.u_m.min(point.u_m), min.v_m.min(point.v_m));
            max = WorldPoint::new(max.u_m.max(point.u_m), max.v_m.max(point.v_m));
        }
        WorldRect { min, max }
    }

    /// Distance from a point to the nearest segment.
    pub fn distance_to(&self, point: WorldPoint) -> f64 {
        let mut nearest = f64::INFINITY;
        for (a, b) in self.segments() {
            nearest = nearest.min(distance_to_segment(point, a, b));
            if nearest == 0.0 {
                break;
            }
        }
        nearest
    }
}

/// Distance from a point to a line segment.
fn distance_to_segment(point: WorldPoint, a: WorldPoint, b: WorldPoint) -> f64 {
    let (dx, dy) = (b.u_m - a.u_m, b.v_m - a.v_m);
    let length_squared = dx * dx + dy * dy;
    if length_squared <= 0.0 {
        return point.distance(a);
    }
    // Projected onto the segment and clamped to it, so the nearest point on a
    // segment past its own end is the end rather than a point on the infinite
    // line — which is what makes a corner behave like a corner.
    let t =
        (((point.u_m - a.u_m) * dx + (point.v_m - a.v_m) * dy) / length_squared).clamp(0.0, 1.0);
    point.distance(WorldPoint::new(a.u_m + dx * t, a.v_m + dy * t))
}

/// Distance to a spline, as a field.
///
/// Reports the distance in metres, clamped to `max_distance_m`. Layers turn that
/// into a mask through a [`crate::document::Profile`] — a `SmoothBand` is how a
/// path gets its width and its soft edge — which is why this returns a raw
/// distance rather than something already shaped.
pub struct SplineDistanceField {
    spline: Spline,
    max_distance_m: f64,
    /// The spline's own bounds, grown by the maximum distance.
    reach_bounds: WorldRect,
}

impl SplineDistanceField {
    pub fn new(spline: Spline, max_distance_m: f64) -> Self {
        let max_distance_m = if max_distance_m.is_finite() && max_distance_m > 0.0 {
            max_distance_m
        } else {
            1.0
        };
        let reach_bounds = spline.bounds().expanded(max_distance_m);
        Self {
            spline,
            max_distance_m,
            reach_bounds,
        }
    }

    pub fn spline(&self) -> &Spline {
        &self.spline
    }
}

impl ScalarField for SplineDistanceField {
    fn value_at(&self, point: WorldPoint, _footprint: SampleFootprint) -> f32 {
        // The bounding-box reject. A path crosses a small fraction of any large
        // region, so most samples are answered without touching a segment — and
        // a page far from the path costs one comparison per texel rather than
        // one per texel per segment.
        if !self.reach_bounds.contains(point) {
            return self.max_distance_m as f32;
        }
        self.spline.distance_to(point).min(self.max_distance_m) as f32
    }

    fn reach_m(&self) -> f64 {
        self.max_distance_m
    }

    fn describe(&self) -> String {
        format!(
            "spline, {} points, reaching {:.2} m",
            self.spline.points.len(),
            self.max_distance_m
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise() -> NoiseField {
        NoiseField::new(0.1, 4, 2.0, 0.5, "meadow", 0x8df7_82f9_5ce1_a4d4)
    }

    #[test]
    fn noise_is_a_pure_function_of_world_position() {
        let field = noise();
        let point = WorldPoint::new(3.25, -7.5);
        let first = field.value_at(point, SampleFootprint::Point);
        for _ in 0..4 {
            assert_eq!(field.value_at(point, SampleFootprint::Point), first);
        }
    }

    #[test]
    fn noise_stays_inside_its_range() {
        let field = noise();
        let unsigned = NoiseField::new(0.1, 4, 2.0, 0.5, "meadow", 7).unsigned();
        for i in 0..2000 {
            let point = WorldPoint::new(i as f64 * 0.37 - 300.0, i as f64 * -0.91 + 120.0);
            let signed = field.value_at(point, SampleFootprint::Point);
            assert!((-1.0..=1.0).contains(&signed), "{signed}");
            let zero_one = unsigned.value_at(point, SampleFootprint::Point);
            assert!((0.0..=1.0).contains(&zero_one), "{zero_one}");
        }
    }

    #[test]
    fn noise_actually_varies() {
        // A field that returned a constant would pass every other test here.
        let field = noise();
        let values: Vec<f32> = (0..64)
            .map(|i| field.value_at(WorldPoint::new(i as f64 * 2.5, 0.0), SampleFootprint::Point))
            .collect();
        let low = values.iter().cloned().fold(f32::INFINITY, f32::min);
        let high = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(high - low > 0.4, "the field spans only {}", high - low);
    }

    #[test]
    fn noise_is_continuous_across_a_lattice_boundary() {
        // A linear interpolation has a discontinuous derivative at every cell
        // boundary, and the eye finds those immediately as a grid of creases.
        let field = noise();
        // The lattice is at 0.1 per metre, so a boundary sits at every 10 m.
        let step = 0.001;
        let mut worst = 0.0f32;
        for i in -3..3 {
            let boundary = i as f64 * 10.0;
            let before = field.value_at(
                WorldPoint::new(boundary - step, 1.0),
                SampleFootprint::Point,
            );
            let after = field.value_at(
                WorldPoint::new(boundary + step, 1.0),
                SampleFootprint::Point,
            );
            worst = worst.max((before - after).abs());
        }
        assert!(worst < 0.01, "a {worst} jump across a lattice boundary");
    }

    #[test]
    fn a_coarse_footprint_drops_the_octaves_it_cannot_resolve() {
        // An octave finer than the sampling rate cannot be filtered, only
        // sampled, so it crawls when the ground moves under the grid.
        let field = NoiseField::new(0.05, 8, 2.0, 0.5, "meadow", 11);
        let point = WorldPoint::new(12.0, -5.0);
        let sharp = field.value_at(point, SampleFootprint::Point);
        let blurred = field.value_at(point, SampleFootprint::circle(20.0));
        assert_ne!(sharp, blurred, "the footprint changed nothing");

        // And a footprint coarser than every octave gives the field's mean
        // rather than nothing.
        let hopeless = field.value_at(point, SampleFootprint::circle(10_000.0));
        assert_eq!(hopeless, 0.0);
        assert_eq!(
            NoiseField::new(0.05, 8, 2.0, 0.5, "m", 11)
                .unsigned()
                .value_at(point, SampleFootprint::circle(10_000.0)),
            0.5
        );
    }

    #[test]
    fn two_streams_are_independent() {
        let a = NoiseField::new(0.1, 4, 2.0, 0.5, "meadow", 7);
        let b = NoiseField::new(0.1, 4, 2.0, 0.5, "rocks", 7);
        let differing = (0..64)
            .filter(|i| {
                let p = WorldPoint::new(*i as f64 * 3.0, 0.0);
                a.value_at(p, SampleFootprint::Point) != b.value_at(p, SampleFootprint::Point)
            })
            .count();
        assert!(differing > 60, "only {differing} of 64 samples differ");
    }

    #[test]
    fn a_degenerate_noise_configuration_does_not_produce_nonsense() {
        // Reached from authored data, which validation reports separately.
        for field in [
            NoiseField::new(0.0, 4, 2.0, 0.5, "m", 1),
            NoiseField::new(f64::NAN, 4, 2.0, 0.5, "m", 1),
            NoiseField::new(0.1, 0, 2.0, 0.5, "m", 1),
            NoiseField::new(0.1, 4, 0.5, 0.5, "m", 1),
            NoiseField::new(0.1, 4, 2.0, 2.0, "m", 1),
        ] {
            let value = field.value_at(WorldPoint::new(1.0, 2.0), SampleFootprint::Point);
            assert!(
                value.is_finite() && (-1.0..=1.0).contains(&value),
                "{value}"
            );
        }
    }

    fn spline() -> Spline {
        Spline::parse("# a straight path\n0 0\n10 0\n10 10\n").expect("parses")
    }

    #[test]
    fn a_spline_parses_from_its_line_form() {
        let spline = spline();
        assert_eq!(spline.points.len(), 3);
        assert_eq!(spline.points[0], WorldPoint::new(0.0, 0.0));
        assert_eq!(spline.points[2], WorldPoint::new(10.0, 10.0));
        assert!(!spline.closed);
        assert_eq!(spline.segments().count(), 2);
    }

    #[test]
    fn a_closed_spline_joins_its_ends() {
        let closed = Spline::parse("0 0\n10 0\n10 10\nclosed\n").expect("parses");
        assert!(closed.closed);
        assert_eq!(closed.segments().count(), 3);
        // Standing at the midpoint of the closing segment is on the spline.
        assert!(closed.distance_to(WorldPoint::new(5.0, 5.0)) < 1.0e-9);
    }

    #[test]
    fn a_malformed_spline_says_which_line() {
        // An author fixing a path wants the line, not "invalid input".
        assert_eq!(
            Spline::parse("0 0\nnot a point\n"),
            Err(SplineError::BadLine {
                line: 2,
                text: "not a point".into()
            })
        );
        assert_eq!(
            Spline::parse("0 0\n1 nope\n"),
            Err(SplineError::BadNumber {
                line: 2,
                text: "nope".into()
            })
        );
        assert_eq!(Spline::parse("0 0\n"), Err(SplineError::TooShort));
        assert_eq!(
            Spline::parse("# nothing but a comment\n"),
            Err(SplineError::TooShort)
        );
        assert_eq!(
            Spline::parse("0 0\n1 2 3\n"),
            Err(SplineError::BadLine {
                line: 2,
                text: "1 2 3".into()
            })
        );
    }

    #[test]
    fn distance_is_measured_to_the_segment_rather_than_the_line() {
        // What makes a corner behave like a corner: past a segment's end, the
        // nearest point is the end, not a point on the infinite line.
        let spline = Spline::parse("0 0\n10 0\n").expect("parses");
        // Alongside the segment.
        assert!((spline.distance_to(WorldPoint::new(5.0, 3.0)) - 3.0).abs() < 1.0e-9);
        // Past its end: five metres beyond, so five metres away.
        assert!((spline.distance_to(WorldPoint::new(15.0, 0.0)) - 5.0).abs() < 1.0e-9);
        // Diagonally past its end.
        let expected = (25.0f64 + 9.0).sqrt();
        assert!((spline.distance_to(WorldPoint::new(15.0, 3.0)) - expected).abs() < 1.0e-9);
    }

    #[test]
    fn a_point_on_the_spline_is_at_zero() {
        let spline = spline();
        for point in &spline.points {
            assert!(spline.distance_to(*point) < 1.0e-9);
        }
        assert!(spline.distance_to(WorldPoint::new(5.0, 0.0)) < 1.0e-9);
    }

    #[test]
    fn the_field_clamps_to_its_own_reach() {
        // Past the maximum the source reports its maximum and stops being
        // interesting — which is also what bounds the index.
        let field = SplineDistanceField::new(spline(), 5.0);
        assert_eq!(field.reach_m(), 5.0);
        assert_eq!(
            field.value_at(WorldPoint::new(1000.0, 1000.0), SampleFootprint::Point),
            5.0
        );
        assert!(field.value_at(WorldPoint::new(5.0, 2.0), SampleFootprint::Point) - 2.0 < 1.0e-5);
    }

    #[test]
    fn a_point_far_outside_is_answered_without_touching_a_segment() {
        // The bounding-box reject. A page far from the path costs one comparison
        // per texel rather than one per texel per segment.
        let mut points = String::new();
        for i in 0..500 {
            points.push_str(&format!("{i} 0\n"));
        }
        let field = SplineDistanceField::new(Spline::parse(&points).expect("parses"), 4.0);
        // Well outside the grown bounds.
        assert_eq!(
            field.value_at(WorldPoint::new(-1000.0, -1000.0), SampleFootprint::Point),
            4.0
        );
        // And inside, it still measures.
        assert!(field.value_at(WorldPoint::new(250.0, 1.0), SampleFootprint::Point) < 4.0);
    }

    #[test]
    fn a_degenerate_reach_does_not_produce_an_empty_index() {
        // Zero is not "no falloff", it is "no index", which is why validation
        // refuses it — and why this clamps rather than trusting.
        let field = SplineDistanceField::new(spline(), 0.0);
        assert!(field.reach_m() > 0.0);
    }
}

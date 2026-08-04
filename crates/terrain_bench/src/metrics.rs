//! Aesthetic metrics.
//!
//! These score generated output on properties that correlate with looking
//! right. They are proxies, not judges — a rock that scores well can still be
//! ugly. What they reliably catch is *drift*: the generator that slowly starts
//! producing spikier rocks, or scatter that starts clumping, over the weeks
//! between the times anyone looks closely at it.
//!
//! Each returns a value in a documented range with a documented direction, so a
//! baseline comparison can be automatic rather than requiring interpretation.

/// A point in the plane, in whatever unit the caller measures in.
///
/// Plain `f64` where this was fixed point. The fixed-point types existed so a
/// battle would produce bit-identical results on any machine; there is no battle
/// and nothing here feeds a simulation, so what is left is a shape-scoring
/// routine paying a conversion at every arithmetic site for a guarantee nobody
/// needs. Every function below already computed in `f64` internally — the fixed
/// point reached the boundary and stopped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// Isoperimetric quotient: `4 * pi * area / perimeter^2`.
///
/// 1.0 is a perfect circle; a long spiky shape tends toward 0. For rocks the
/// useful band is roughly 0.6 to 0.9 — below that the silhouette is getting
/// spindly, above it the rock is a featureless blob.
///
/// Returns 0 for degenerate input rather than dividing by zero.
pub fn compactness(area: f64, perimeter: f64) -> f64 {
    if perimeter <= 0.0 || area <= 0.0 {
        return 0.0;
    }
    (4.0 * std::f64::consts::PI * area) / (perimeter * perimeter)
}

/// Ratio of a polygon's area to its convex hull's area, in 0..=1.
///
/// 1.0 means fully convex. Rocks want a little concavity for interest but not
/// much: below about 0.85 the silhouette starts reading as broken debris rather
/// than a solid stone.
pub fn convexity(outline: &[Point]) -> f64 {
    if outline.len() < 3 {
        return 0.0;
    }
    let hull = convex_hull(outline);
    let hull_area = polygon_area(&hull).abs();
    if hull_area <= 0.0 {
        return 0.0;
    }
    (polygon_area(outline).abs() / hull_area).clamp(0.0, 1.0)
}

/// How evenly a scatter is spread, in 0..=1.
///
/// The ratio of the smallest nearest-neighbour distance to the mean
/// nearest-neighbour distance. Uniform random points produce clusters and score
/// low; a well-spaced blue-noise distribution scores high. This is the number
/// that tells you a forest looks scattered rather than clumped.
///
/// Returns 0 for fewer than two points.
pub fn blue_noise_score(points: &[Point]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let nearest: Vec<f64> = points
        .iter()
        .map(|&p| {
            points
                .iter()
                .filter(|&&q| q != p)
                .map(|&q| p.distance(q))
                .fold(f64::MAX, f64::min)
        })
        .collect();

    let mean = nearest.iter().sum::<f64>() / nearest.len() as f64;
    if mean <= 0.0 {
        return 0.0;
    }
    let min = nearest.iter().cloned().fold(f64::MAX, f64::min);
    (min / mean).clamp(0.0, 1.0)
}

/// Spread between the lightest and darkest tone of a palette, in 0..=1.
///
/// Too low and a rock reads as a flat blob with no form; too high and it looks
/// like cut glass. Around 0.3 to 0.6 is the useful band for stone.
pub fn luminance_spread(colors: &[[u8; 3]]) -> f64 {
    if colors.is_empty() {
        return 0.0;
    }
    // Rec. 601 luma, which is close enough to perceived brightness for this.
    let luma =
        |c: &[u8; 3]| (0.299 * c[0] as f64 + 0.587 * c[1] as f64 + 0.114 * c[2] as f64) / 255.0;
    let values: Vec<f64> = colors.iter().map(luma).collect();
    let max = values.iter().cloned().fold(f64::MIN, f64::max);
    let min = values.iter().cloned().fold(f64::MAX, f64::min);
    (max - min).clamp(0.0, 1.0)
}

/// How much a set of generated shapes differ from one another, in 0..=1.
///
/// Coefficient of variation of their areas, clamped. Near 0 means the generator
/// is producing the same shape every time with a different seed — which is a
/// real and easy failure to introduce, and invisible in any correctness test.
pub fn silhouette_variety(areas: &[f64]) -> f64 {
    if areas.len() < 2 {
        return 0.0;
    }
    let mean = areas.iter().sum::<f64>() / areas.len() as f64;
    if mean.abs() <= f64::EPSILON {
        return 0.0;
    }
    let variance = areas.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / areas.len() as f64;
    (variance.sqrt() / mean.abs()).clamp(0.0, 1.0)
}

/// Shoelace area; positive when counter-clockwise.
fn polygon_area(points: &[Point]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    sum / 2.0
}

/// Andrew's monotone chain.
fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut sorted: Vec<Point> = points.to_vec();
    // `total_cmp` rather than `partial_cmp`, so the sort is a total order even
    // if a NaN reaches it. A hull built from a partial order silently loses
    // points instead of reporting anything.
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x).then_with(|| a.y.total_cmp(&b.y)));
    sorted.dedup();
    if sorted.len() < 3 {
        return sorted;
    }

    let cross =
        |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

    // Two separate passes. A single pass over `points ++ reversed(points)`
    // looks tidier but is wrong: the upper chain can pop vertices belonging to
    // the lower one, because nothing stops it unwinding past the join.
    let build = |sequence: &mut dyn Iterator<Item = Point>| -> Vec<Point> {
        let mut chain: Vec<Point> = Vec::new();
        for p in sequence {
            while chain.len() >= 2
                && cross(chain[chain.len() - 2], chain[chain.len() - 1], p) <= 0.0
            {
                chain.pop();
            }
            chain.push(p);
        }
        // The last point is the first of the other chain.
        chain.pop();
        chain
    };

    let mut lower = build(&mut sorted.iter().copied());
    let upper = build(&mut sorted.iter().rev().copied());
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(points: &[(i32, i32)]) -> Vec<Point> {
        points
            .iter()
            .map(|&(x, y)| Point::new(x as f64, y as f64))
            .collect()
    }

    #[test]
    fn compactness_peaks_at_a_circle() {
        // A circle of radius 1: area pi, perimeter 2pi, quotient exactly 1.
        let circle = compactness(std::f64::consts::PI, std::f64::consts::TAU);
        assert!((circle - 1.0).abs() < 1e-9, "{circle}");
        // A 1x100 sliver should score far lower than a square of equal area.
        assert!(compactness(100.0, 202.0) < compactness(100.0, 40.0));
    }

    #[test]
    fn compactness_handles_degenerate_input() {
        assert_eq!(compactness(0.0, 0.0), 0.0);
        assert_eq!(compactness(-1.0, 10.0), 0.0);
    }

    #[test]
    fn convexity_is_one_for_a_convex_polygon() {
        let square = polygon(&[(0, 0), (4, 0), (4, 4), (0, 4)]);
        assert!(
            (convexity(&square) - 1.0).abs() < 1e-6,
            "{}",
            convexity(&square)
        );
    }

    #[test]
    fn convexity_drops_for_a_concave_polygon() {
        // An arrowhead: same hull as the square, much less area.
        let arrow = polygon(&[(0, 0), (4, 0), (2, 2), (4, 4), (0, 4)]);
        let score = convexity(&arrow);
        assert!(score < 0.95 && score > 0.0, "{score}");
    }

    #[test]
    fn convexity_handles_degenerate_input() {
        assert_eq!(convexity(&[]), 0.0);
        assert_eq!(convexity(&polygon(&[(0, 0), (1, 1)])), 0.0);
    }

    #[test]
    fn evenly_spaced_points_beat_clustered_ones() {
        let even: Vec<Point> = (0..5)
            .flat_map(|y| (0..5).map(move |x| Point::new((x * 3) as f64, (y * 3) as f64)))
            .collect();
        let mut clustered = even.clone();
        // Drop a point almost on top of an existing one.
        clustered.push(Point::new(0.1, 0.0));
        assert!(
            blue_noise_score(&even) > blue_noise_score(&clustered),
            "even {} should beat clustered {}",
            blue_noise_score(&even),
            blue_noise_score(&clustered)
        );
    }

    #[test]
    fn blue_noise_needs_at_least_two_points() {
        assert_eq!(blue_noise_score(&[]), 0.0);
        assert_eq!(blue_noise_score(&polygon(&[(0, 0)])), 0.0);
    }

    #[test]
    fn luminance_spread_separates_flat_from_contrasty_palettes() {
        let flat = [[120, 120, 120], [122, 122, 122]];
        let contrasty = [[20, 20, 20], [230, 230, 230]];
        assert!(luminance_spread(&flat) < 0.05);
        assert!(luminance_spread(&contrasty) > 0.7);
        assert_eq!(luminance_spread(&[]), 0.0);
    }

    #[test]
    fn variety_is_zero_when_every_shape_is_identical() {
        // The failure this exists to catch: a generator that ignores its seed.
        assert_eq!(silhouette_variety(&[10.0; 8]), 0.0);
        assert!(silhouette_variety(&[5.0, 10.0, 20.0, 40.0]) > 0.2);
        assert_eq!(silhouette_variety(&[1.0]), 0.0);
    }

    #[test]
    fn convex_hull_of_a_square_with_an_interior_point_ignores_the_interior() {
        let points = polygon(&[(0, 0), (4, 0), (4, 4), (0, 4), (2, 2)]);
        assert_eq!(convex_hull(&points).len(), 4);
    }
}

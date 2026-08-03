//! Hashed randomness, one stream per decision.
//!
//! Everything the baker places is a pure function of world coordinates, which is
//! what makes two neighbouring pages agree along their shared edge without ever
//! talking to each other. A sequential generator cannot do that — its output
//! depends on how many draws came before — so every draw here is a hash of
//! *where* and *what for*.
//!
//! The "what for" half matters more than it looks. Reusing one stream for clump
//! density and clump colour correlates them, and correlated randomness is the
//! fastest way to make procedural vegetation read as artificial: every dense
//! patch is also the same shade, and the eye finds the rule immediately. Each
//! [`Stream`] variant is an independent field over the same world.

/// What a random draw is *for*.
///
/// Adding a decision means adding a variant, never borrowing one. Values are
/// arbitrary but fixed — changing one reshuffles that field across the whole
/// world, which is a deliberate act, not a tidy-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Stream {
    /// Where the mounds sit, and how big they are.
    Mound = 0x01,
    /// Broad colour drift, independent of mound placement.
    Tint = 0x02,
    /// Where the bare ground shows through.
    Dirt = 0x03,
    /// Blade placement within a cell.
    Blade = 0x04,
    /// Blade shape: lean, curl, width, length.
    Shape = 0x05,
    /// The dark mat under everything.
    Thatch = 0x06,
    /// Broadleaf clusters.
    Leaf = 0x07,
    /// Tall sparse accents.
    Tuft = 0x08,
    /// Soil mottling.
    Soil = 0x09,
    /// Which clump family a patch of ground grows.
    Family = 0x0a,
    /// Per-blade brightness jitter.
    Shade = 0x0b,
    /// Animation phase, later.
    Phase = 0x0c,
    /// How finely a patch of ground is described.
    Detail = 0x0d,
    /// Which way the field runs: ridge orientation and blade heading.
    Flow = 0x0e,
    /// Regional hue drift, independent of how bright the region is.
    Hue = 0x0f,
    /// Which tuft group a bunch belongs to, and where the group's crown sits.
    TuftGroup = 0x10,
    /// How old and how established a patch of ground is.
    Maturity = 0x11,
    /// How damp it is. Drives density, root darkness and hue together.
    Moisture = 0x12,
    /// How much sky a patch of canopy can see.
    Exposure = 0x13,
    /// How far a blade's face rotates about its own axis.
    Twist = 0x14,
    /// Whether a blade's tip splits at all.
    Fork = 0x15,
    /// Where it splits, how far it opens, and how unequal the halves are.
    ForkGeometry = 0x16,
    /// How pronounced a blade's midrib is.
    Ridge = 0x17,
    /// How much darker a blade's underside runs.
    Underside = 0x18,
    /// The fine grass layer that closes the canopy under the statement tufts.
    Fine = 0x19,
    /// Shoot bundles within a tuft — several related blades from one root.
    Tiller = 0x1a,
}

/// Mix a 64-bit value until its bits are independent.
///
/// `splitmix64`'s finaliser. Cheap, and good enough that adjacent integer inputs
/// produce uncorrelated outputs — which is the entire requirement here, since
/// every input is a grid coordinate one step from its neighbour.
#[inline]
pub const fn scramble(z: u64) -> u64 {
    mix(z)
}

/// See [`scramble`], which is this under a name callers are allowed to use.
#[inline]
const fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Hash a grid cell on a stream into a seed.
#[inline]
pub const fn cell(seed: u64, stream: Stream, x: i32, y: i32) -> u64 {
    let packed = ((x as i64 as u64) << 32) ^ (y as i64 as u64 & 0xffff_ffff);
    mix(seed ^ mix(packed ^ ((stream as u64) << 56)))
}

/// A short sequence of draws from one hashed seed.
///
/// Cheap to make and cheap to advance, so the usual shape is: hash a cell into
/// one of these, pull the handful of numbers that describe the thing living
/// there, and drop it.
#[derive(Clone, Copy, Debug)]
pub struct Draw(u64);

impl Draw {
    /// Start drawing for `stream` at grid cell `(x, y)`.
    #[inline]
    pub const fn at(seed: u64, stream: Stream, x: i32, y: i32) -> Self {
        Self(cell(seed, stream, x, y))
    }

    /// Start drawing from an already-hashed value, for per-object sequences.
    #[inline]
    pub const fn from_seed(seed: u64) -> Self {
        Self(mix(seed))
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0)
    }

    /// A uniform value in `[0, 1)`.
    #[inline]
    pub fn unit(&mut self) -> f32 {
        // Top 24 bits: every value representable in an f32 mantissa, no bias.
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// A uniform value in `[low, high)`.
    #[inline]
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }

    /// A uniform value in `[-1, 1)`.
    #[inline]
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// Roughly normal, mean zero, standard deviation one.
    ///
    /// Three uniforms rather than a Box–Muller pair: no transcendentals, and the
    /// tails are clipped, which is what you want for shape parameters that must
    /// not occasionally produce a blade ten times too long.
    #[inline]
    pub fn normal(&mut self) -> f32 {
        (self.unit() + self.unit() + self.unit() - 1.5) * 1.1547
    }

    /// True with probability `p`.
    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    /// An index in `0..n`.
    #[inline]
    pub fn index(&mut self, n: usize) -> usize {
        ((self.unit() * n as f32) as usize).min(n.saturating_sub(1))
    }

    /// The raw state, for seeding a nested sequence.
    #[inline]
    pub const fn seed(&self) -> u64 {
        self.0
    }
}

/// Smooth 2D value noise on a unit grid.
///
/// Value rather than gradient noise because the baker never needs the derivative
/// and value noise is half the cost; its axis-aligned bias is invisible once
/// it is only ever used to modulate something else.
pub fn value_noise(seed: u64, stream: Stream, x: f32, y: f32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (ix, iy) = (ix as i32, iy as i32);
    // Quintic: zero first *and* second derivative at the cell edges, so a slow
    // gradient across a noise cell boundary does not show as a crease.
    let sx = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let sy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);

    let corner =
        |dx: i32, dy: i32| (cell(seed, stream, ix + dx, iy + dy) >> 40) as f32 / 16_777_216.0;
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// Summed value noise, `octaves` of it, each half the amplitude and twice the
/// frequency of the last.
pub fn fbm(seed: u64, stream: Stream, x: f32, y: f32, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut frequency = 1.0;
    for octave in 0..octaves {
        // Rotate each octave so the axis-aligned lattices do not stack up into a
        // visible grid.
        let (rx, ry) = match octave % 3 {
            0 => (x, y),
            1 => (x * 0.8 + y * 0.6 + 31.4, y * 0.8 - x * 0.6 - 17.2),
            _ => (x * 0.6 - y * 0.8 - 7.7, y * 0.6 + x * 0.8 + 51.3),
        };
        sum += value_noise(seed, stream, rx * frequency, ry * frequency) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    sum / total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_hashes_the_same_way_every_time() {
        assert_eq!(cell(7, Stream::Blade, 3, -4), cell(7, Stream::Blade, 3, -4));
    }

    #[test]
    fn neighbouring_cells_are_uncorrelated() {
        // The property the whole scheme rests on. A hash that let neighbours
        // agree would show up as visible rows of identical clumps.
        let mut agreements = 0;
        for x in 0..64 {
            for y in 0..64 {
                let a = Draw::at(1, Stream::Blade, x, y).unit();
                let b = Draw::at(1, Stream::Blade, x + 1, y).unit();
                if (a - b).abs() < 0.02 {
                    agreements += 1;
                }
            }
        }
        // Two independent uniforms land within 0.02 about 4% of the time.
        assert!(
            agreements < 64 * 64 / 10,
            "{agreements} of 4096 neighbours agreed"
        );
    }

    #[test]
    fn streams_are_independent() {
        let a = Draw::at(1, Stream::Blade, 5, 5).unit();
        let b = Draw::at(1, Stream::Tint, 5, 5).unit();
        assert!((a - b).abs() > 1e-6);
    }

    #[test]
    fn draws_stay_in_range() {
        let mut draw = Draw::at(9, Stream::Shape, 0, 0);
        for _ in 0..10_000 {
            let unit = draw.unit();
            assert!((0.0..1.0).contains(&unit));
            assert!((-1.0..1.0).contains(&draw.signed()));
            assert!(draw.normal().abs() < 2.0);
            assert!(draw.index(7) < 7);
        }
    }

    #[test]
    fn value_noise_is_continuous_across_cell_boundaries() {
        let left = value_noise(3, Stream::Tint, 4.0 - 1e-4, 2.3);
        let right = value_noise(3, Stream::Tint, 4.0 + 1e-4, 2.3);
        assert!((left - right).abs() < 1e-3, "{left} vs {right}");
    }

    #[test]
    fn noise_spans_most_of_the_unit_interval() {
        let (mut low, mut high) = (1.0f32, 0.0f32);
        for i in 0..2000 {
            let v = fbm(5, Stream::Soil, i as f32 * 0.37, i as f32 * 0.11, 4);
            low = low.min(v);
            high = high.max(v);
        }
        assert!(low < 0.3 && high > 0.7, "{low}..{high}");
    }
}

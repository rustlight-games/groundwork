//! Hashing and value noise.
//!
//! Used for two jobs that both need "random, but the same every time": the
//! smoothly varying terrain properties a [field](crate::field) starts from, and
//! the per-blade variation that stops a chunk of grass looking stamped from one
//! mould.
//!
//! Everything here is a pure function of its inputs. Nothing carries generator
//! state, which is what lets a chunk be rebuilt years later — or on another
//! machine — and come out identical, and what lets blades be placed in parallel
//! without a shared generator serialising them.
//!
//! This is presentation-side randomness and deliberately does not use
//! `bw_core::rng`. Blade placement must never influence a battle, so it draws
//! from somewhere the simulation cannot reach.

/// A 32-bit integer hash.
///
/// Two rounds of xor-shift and multiply. Cheap, and good enough that adjacent
/// inputs decorrelate — which is the property that matters, since the caller is
/// almost always hashing neighbouring integer coordinates.
pub fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Hash a lattice coordinate and a seed together.
pub fn hash_2d(x: i32, y: i32, seed: u32) -> u32 {
    // Odd multipliers so the two axes cannot alias onto each other, which would
    // make the noise visibly diagonal.
    let mixed = (x as u32).wrapping_mul(0x9e37_79b9)
        ^ (y as u32).wrapping_mul(0x85eb_ca6b)
        ^ seed.wrapping_mul(0xc2b2_ae35);
    hash_u32(mixed)
}

/// A hash mapped into `0.0..1.0`.
pub fn unit_from_hash(hash: u32) -> f32 {
    // Top 24 bits: f32 has 24 bits of mantissa, so using more would be a lie.
    (hash >> 8) as f32 / (1u32 << 24) as f32
}

/// A uniform random value in `0.0..1.0` for a lattice cell.
pub fn rand_2d(x: i32, y: i32, seed: u32) -> f32 {
    unit_from_hash(hash_2d(x, y, seed))
}

/// Smooth value noise in `0.0..1.0`, with a lattice spacing of one.
///
/// Bilinear interpolation of hashed lattice values through a smoothstep, so the
/// result is continuous and has a continuous first derivative. That second part
/// matters: value noise interpolated linearly has creases along the lattice,
/// and grass properties that jump at a crease produce a visible grid.
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let ix = x0 as i32;
    let iy = y0 as i32;

    let fx = smoothstep(x - x0);
    let fy = smoothstep(y - y0);

    let c00 = rand_2d(ix, iy, seed);
    let c10 = rand_2d(ix + 1, iy, seed);
    let c01 = rand_2d(ix, iy + 1, seed);
    let c11 = rand_2d(ix + 1, iy + 1, seed);

    let bottom = c00 + (c10 - c00) * fx;
    let top = c01 + (c11 - c01) * fx;
    bottom + (top - bottom) * fy
}

/// Summed octaves of [`value_noise`], normalised back into `0.0..1.0`.
pub fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut normalisation = 0.0;
    let mut frequency = 1.0;

    for octave in 0..octaves.max(1) {
        // A different seed per octave, or every octave would be the same
        // pattern at a different scale and the sum would show its structure.
        total += value_noise(x * frequency, y * frequency, seed ^ (octave * 0x9e37)) * amplitude;
        normalisation += amplitude;
        amplitude *= 0.5;
        // Not exactly two, so octave lattices do not line up and reinforce.
        frequency *= 2.031;
    }
    total / normalisation
}

/// The classic `3t^2 - 2t^3` ease, clamped.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smoothstep between two edges, tolerating `low >= high`.
pub fn smoothstep_between(low: f32, high: f32, value: f32) -> f32 {
    if high - low <= f32::EPSILON {
        return if value >= high { 1.0 } else { 0.0 };
    }
    smoothstep((value - low) / (high - low))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_is_stable() {
        // If this ever changes, every generated field and every blade layout
        // changes with it. That is allowed, but it should be a deliberate act.
        assert_eq!(hash_u32(0), 0);
        assert_eq!(hash_u32(1), hash_u32(1));
        assert_ne!(hash_u32(1), hash_u32(2));
    }

    #[test]
    fn adjacent_coordinates_decorrelate() {
        // The failure this guards: a hash where neighbouring cells return
        // similar values produces visible diagonal banding rather than noise.
        let mut differences = 0;
        for i in 0..64 {
            let a = rand_2d(i, 0, 7);
            let b = rand_2d(i + 1, 0, 7);
            if (a - b).abs() > 0.2 {
                differences += 1;
            }
        }
        assert!(
            differences > 40,
            "only {differences}/64 neighbours differed"
        );
    }

    #[test]
    fn the_two_axes_are_not_interchangeable() {
        // A hash that mixes x and y symmetrically makes the noise mirror about
        // the diagonal, which is extremely visible on a large field.
        assert_ne!(rand_2d(3, 8, 1), rand_2d(8, 3, 1));
    }

    #[test]
    fn seeds_produce_different_fields() {
        let a: Vec<f32> = (0..32).map(|i| rand_2d(i, i, 1)).collect();
        let b: Vec<f32> = (0..32).map(|i| rand_2d(i, i, 2)).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn values_stay_in_range() {
        for i in 0..200 {
            let x = i as f32 * 0.37 - 30.0;
            let y = i as f32 * -0.19 + 11.0;
            let v = value_noise(x, y, 5);
            assert!((0.0..=1.0).contains(&v), "{v}");
            let f = fbm(x, y, 5, 3);
            assert!((0.0..=1.0).contains(&f), "{f}");
        }
    }

    #[test]
    fn noise_is_continuous_across_lattice_lines() {
        // A discontinuity here becomes a visible grid in grass height.
        let epsilon = 1e-3;
        for lattice in -3..4 {
            let at = lattice as f32;
            let before = value_noise(at - epsilon, 0.5, 9);
            let after = value_noise(at + epsilon, 0.5, 9);
            assert!(
                (before - after).abs() < 0.01,
                "jump of {} at x = {at}",
                (before - after).abs()
            );
        }
    }

    #[test]
    fn noise_actually_varies() {
        let samples: Vec<f32> = (0..50)
            .map(|i| value_noise(i as f32 * 0.7, 0.0, 3))
            .collect();
        let min = samples.iter().cloned().fold(f32::MAX, f32::min);
        let max = samples.iter().cloned().fold(f32::MIN, f32::max);
        assert!(max - min > 0.4, "range was only {}", max - min);
    }

    #[test]
    fn noise_is_pure() {
        assert_eq!(value_noise(1.25, -3.5, 4), value_noise(1.25, -3.5, 4));
        assert_eq!(fbm(1.25, -3.5, 4, 3), fbm(1.25, -3.5, 4, 3));
    }

    #[test]
    fn smoothstep_is_flat_at_both_ends() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(-5.0), 0.0);
        assert_eq!(smoothstep(5.0), 1.0);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn smoothstep_between_handles_a_degenerate_range() {
        assert_eq!(smoothstep_between(1.0, 1.0, 0.5), 0.0);
        assert_eq!(smoothstep_between(1.0, 1.0, 1.5), 1.0);
        assert_eq!(smoothstep_between(0.0, 1.0, 0.5), 0.5);
    }
}

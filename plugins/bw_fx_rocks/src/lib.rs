//! Procedural 2D rock artwork.
//!
//! Rocks and terrain are the two generated parts of the landscape. Rocks earn
//! it because the game needs a great many of them, at many sizes and in many
//! silhouettes, and hand-drawn variants would either run out or start visibly
//! repeating. Trees and other props stay as authored sprites — they are more
//! recognisable, fewer are needed, and procedural foliage is a much harder
//! problem than procedural stone.
//!
//! Output is geometry rather than pixels, which is what lets one generator
//! serve three consumers: the renderer rasterises the outline and facets, the
//! simulation can take the outline as a collider, and `bw_bench` scores the
//! silhouette without rendering anything.

#![forbid(unsafe_code)]

use bw_content::registry::{Facet, GeneratorRegistry, RockGenerator, RockPalette, RockShape};
use bw_content::{ContentError, ContentResult, Params};
use bw_core::{Real, Vec2Fx, real_from_int, sin_cos};
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Register every rock generator.
pub fn register_generators(registry: &mut GeneratorRegistry) {
    registry.add_rock(Boulder);
}

/// A chunky, angular boulder.
///
/// A superellipse gives the broad silhouette — rounder than a circle at the
/// sides, flatter on top, which reads as "sat there a long time" — and radial
/// noise breaks up the regularity. Facets are wedges from the centre, shaded by
/// how much they face the light.
///
/// Parameters: `radius` (number, default 1.0), `sides` (integer, default 14),
/// `jaggedness` (0..1, default 0.25), `squareness` (0.5..4, default 2.4).
pub struct Boulder;

impl RockGenerator for Boulder {
    fn key(&self) -> &'static str {
        "boulder"
    }

    fn validate(&self, params: &Params) -> ContentResult<()> {
        if params.contains("sides") {
            let sides = params.int("boulder", "sides")?;
            if !(5..=64).contains(&sides) {
                return Err(ContentError::Invalid {
                    context: "boulder".into(),
                    message: format!("sides must be between 5 and 64, found {sides}"),
                });
            }
        }
        if params.contains("jaggedness") {
            let j = params.real("boulder", "jaggedness")?.to_num::<f64>();
            if !(0.0..=1.0).contains(&j) {
                return Err(ContentError::Invalid {
                    context: "boulder".into(),
                    message: format!("jaggedness must be between 0 and 1, found {j}"),
                });
            }
        }
        Ok(())
    }

    fn generate(&self, params: &Params, rng: &mut ChaCha8Rng) -> RockShape {
        let radius = params
            .real_or("boulder", "radius", real_from_int(1))
            .unwrap_or(one());
        let sides = params
            .int_or("boulder", "sides", 14)
            .unwrap_or(14)
            .clamp(5, 64) as usize;
        let jaggedness = params
            .real_or("boulder", "jaggedness", Real::from_num(0.25))
            .unwrap_or(quarter());
        let squareness = params
            .real_or("boulder", "squareness", Real::from_num(2.4))
            .unwrap_or(two_four());

        let outline = superellipse(sides, radius, squareness, jaggedness, rng);
        let facets = wedge_facets(&outline, rng);

        RockShape {
            outline,
            facets,
            palette: palette(rng),
        }
    }
}

fn one() -> Real {
    real_from_int(1)
}
fn quarter() -> Real {
    Real::from_num(0.25)
}
fn two_four() -> Real {
    Real::from_num(2.4)
}

/// Points around a superellipse, displaced radially by noise.
fn superellipse(
    sides: usize,
    radius: Real,
    squareness: Real,
    jaggedness: Real,
    rng: &mut ChaCha8Rng,
) -> Vec<Vec2Fx> {
    let exponent = squareness.to_num::<f64>().clamp(0.5, 4.0);
    let tau = std::f64::consts::TAU;

    (0..sides)
        .map(|i| {
            let angle = tau * (i as f64) / (sides as f64);
            let (sin, cos) = sin_cos(Real::from_num(angle));
            let (c, s) = (cos.to_num::<f64>(), sin.to_num::<f64>());

            // Superellipse radius at this angle.
            let denominator = c.abs().powf(exponent) + (s.abs() * 1.25).powf(exponent);
            let shape = if denominator <= f64::EPSILON {
                1.0
            } else {
                denominator.powf(-1.0 / exponent)
            };

            // Radial noise, symmetric about zero so the rock does not drift
            // systematically larger or smaller than the requested radius.
            let noise = rng.random_range(-1.0..=1.0f64) * jaggedness.to_num::<f64>();
            let r = radius.to_num::<f64>() * shape * (1.0 + noise);

            Vec2Fx::new(Real::from_num(r * c), Real::from_num(r * s))
        })
        .collect()
}

/// Triangular wedges from the centre to each outline edge.
fn wedge_facets(outline: &[Vec2Fx], rng: &mut ChaCha8Rng) -> Vec<Facet> {
    if outline.len() < 3 {
        return Vec::new();
    }
    // Light from the upper left, which is the convention the sprites use.
    let light = Vec2Fx::new(Real::from_num(-0.7), Real::from_num(0.7));

    (0..outline.len())
        .map(|i| {
            let a = outline[i];
            let b = outline[(i + 1) % outline.len()];
            let facing = ((a + b) / real_from_int(2)).normalize_or_zero();
            let alignment = facing.dot(light).to_num::<f64>().clamp(-1.0, 1.0);
            // Map -1..1 onto a shade range, then jitter so facets do not band.
            let jitter = rng.random_range(-12i32..=12);
            let shade = (128.0 + alignment * 90.0) as i32 + jitter;
            Facet {
                polygon: vec![Vec2Fx::ZERO, a, b],
                shade: shade.clamp(0, 255) as u8,
            }
        })
        .collect()
}

fn palette(rng: &mut ChaCha8Rng) -> RockPalette {
    // One hue drift applied to all three tones, so a rock reads as one material
    // rather than three unrelated colours.
    let drift = rng.random_range(-18i32..=18);
    let shift = |c: [u8; 3]| {
        [
            (c[0] as i32 + drift).clamp(0, 255) as u8,
            (c[1] as i32 + drift).clamp(0, 255) as u8,
            (c[2] as i32 + drift / 2).clamp(0, 255) as u8,
        ]
    };
    let base = RockPalette::default();
    RockPalette {
        base: shift(base.base),
        light: shift(base.light),
        shadow: shift(base.shadow),
    }
}

#[cfg(test)]
mod tests {
    use bw_content::Value;
    use rand::SeedableRng;

    use super::*;

    fn generate(seed: u64, params: &Params) -> RockShape {
        Boulder.generate(params, &mut ChaCha8Rng::seed_from_u64(seed))
    }

    #[test]
    fn the_same_seed_produces_the_same_rock() {
        assert_eq!(generate(1, &Params::new()), generate(1, &Params::new()));
    }

    #[test]
    fn different_seeds_produce_different_rocks() {
        assert_ne!(generate(1, &Params::new()), generate(2, &Params::new()));
    }

    #[test]
    fn outline_is_counter_clockwise_and_encloses_area() {
        // The renderer and the collider both assume this winding.
        let rock = generate(3, &Params::new());
        assert!(
            rock.signed_area() > 0.0,
            "outline is clockwise or degenerate"
        );
        assert!(rock.perimeter() > 0.0);
    }

    #[test]
    fn every_edge_gets_a_facet() {
        let rock = generate(4, &Params::new());
        assert_eq!(rock.facets.len(), rock.outline.len());
        assert!(rock.facets.iter().all(|f| f.polygon.len() == 3));
    }

    #[test]
    fn side_count_is_honoured_and_clamped() {
        let mut params = Params::new();
        params.insert("sides", Value::Int(9));
        assert_eq!(generate(5, &params).outline.len(), 9);

        // Out of range in content is rejected, but a value that slips through
        // still clamps rather than producing a degenerate shape.
        params.insert("sides", Value::Int(2));
        assert_eq!(generate(5, &params).outline.len(), 5);
    }

    #[test]
    fn radius_scales_the_rock() {
        let mut small = Params::new();
        small.insert("radius", Value::Num(1.0));
        small.insert("jaggedness", Value::Num(0.0));
        let mut large = Params::new();
        large.insert("radius", Value::Num(4.0));
        large.insert("jaggedness", Value::Num(0.0));
        assert!(generate(6, &large).signed_area() > generate(6, &small).signed_area() * 4.0);
    }

    #[test]
    fn zero_jaggedness_gives_a_smooth_silhouette() {
        // Radii should all be equal for the same angle band when there is no
        // noise, which is what makes jaggedness a meaningful dial.
        let mut params = Params::new();
        params.insert("jaggedness", Value::Num(0.0));
        params.insert("sides", Value::Int(32));
        let rock = generate(7, &params);
        let radii: Vec<f64> = rock
            .outline
            .iter()
            .map(|p| p.length().to_num::<f64>())
            .collect();
        let max = radii.iter().cloned().fold(f64::MIN, f64::max);
        let min = radii.iter().cloned().fold(f64::MAX, f64::min);
        // A superellipse is not a circle, so some variation is expected — but
        // far less than a jagged rock would show.
        assert!(max / min < 1.6, "smooth rock varied by {:.2}x", max / min);
    }

    #[test]
    fn validation_rejects_out_of_range_parameters() {
        let mut params = Params::new();
        params.insert("sides", Value::Int(2));
        assert!(Boulder.validate(&params).is_err());

        let mut params = Params::new();
        params.insert("jaggedness", Value::Num(9.0));
        assert!(Boulder.validate(&params).is_err());

        assert!(Boulder.validate(&Params::new()).is_ok());
    }

    #[test]
    fn register_adds_the_generator() {
        let mut registry = GeneratorRegistry::new();
        register_generators(&mut registry);
        assert!(registry.rock("boulder").is_some());
    }
}

//! Repeated geometry, placed rather than described.
//!
//! ## When a prototype beats a mark
//!
//! A [`crate::mark::SceneMark`] is *described* — a renderer builds its geometry
//! from parameters, so every one can differ from every other for free. That is
//! the right trade for a hundred thousand blades of grass, where the variation
//! is the whole point and no two are alike.
//!
//! It is the wrong trade for a rock. A rock's geometry is expensive to build and
//! its silhouette is what makes it recognisable, so the useful thing is to build
//! a handful of them and place each one many times with a different rotation and
//! scale. That is what a prototype is: geometry built once, referred to by
//! index, and instanced.
//!
//! The line is roughly: **describe what is varied and cheap, instance what is
//! distinctive and expensive.** Grass, thatch and stems are marks. Rocks, flower
//! heads, broad leaves and debris are instances.
//!
//! ## The scene holds no geometry
//!
//! A prototype here is a *reference* — an index into a table the renderer
//! resolves — not a mesh. The scene stays a description throughout, which is
//! what lets it be fingerprinted, serialised and handed across a process
//! boundary without carrying megabytes of vertices that each renderer would
//! rebuild anyway.

use terrain_core::digest::{Digest, Digestible};

use crate::mark::{Aabb3, MarkAttributes, MarkId, PainterOrder, SceneMaterialIndex};
use crate::projection::ScenePoint;

/// Which prototype in the scene's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrototypeIndex(pub u16);

/// One placement of a prototype.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrototypeInstance {
    pub stable_id: MarkId,
    pub order: PainterOrder,
    pub prototype: PrototypeIndex,
    pub material: SceneMaterialIndex,
    /// Where it sits.
    pub position: ScenePoint,
    /// Turn about the vertical axis, radians.
    pub yaw_rad: f32,
    /// Tilt away from vertical, radians.
    ///
    /// Small, usually. A rock lying at an angle reads as *settled*; every rock
    /// perfectly level reads as placed, which is the tell that gives away a
    /// scatter faster than any spacing artefact.
    pub tilt_rad: f32,
    /// Which way it tilts, radians.
    pub tilt_azimuth_rad: f32,
    /// Non-uniform scale, so one prototype can be several shapes.
    pub scale: [f32; 3],
    pub attributes: MarkAttributes,
    pub bounds: Aabb3,
}

impl PrototypeInstance {
    /// The largest of the three scale factors.
    ///
    /// What a bound is computed from when only the prototype's own radius is
    /// known.
    pub fn max_scale(&self) -> f32 {
        self.scale[0]
            .abs()
            .max(self.scale[1].abs())
            .max(self.scale[2].abs())
    }
}

impl Digestible for PrototypeInstance {
    fn absorb(&self, digest: &mut Digest) {
        digest
            .u64(self.stable_id.0)
            .u64(self.order.bits())
            .u32(self.prototype.0 as u32)
            .u32(self.material.0 as u32)
            .f64(self.position.u_m)
            .f64(self.position.v_m)
            .f64(self.position.z_m)
            .f32(self.yaw_rad)
            .f32(self.tilt_rad)
            .f32(self.tilt_azimuth_rad)
            .f32(self.scale[0])
            .f32(self.scale[1])
            .f32(self.scale[2])
            .f32(self.attributes.maturity)
            .f32(self.attributes.moisture)
            .f32(self.attributes.exposure)
            .f32(self.attributes.tint)
            .f32(self.attributes.variation);
    }
}

/// What a prototype is, without being the geometry.
///
/// The recipe key and its parameters, so a renderer can build the mesh itself
/// and two renderers building the same prototype get the same shape. Carrying
/// the mesh instead would make the scene hundreds of megabytes and would fix the
/// tessellation at whatever the builder chose — which a path tracer and a
/// rasteriser do not agree about.
#[derive(Clone, Debug, PartialEq)]
pub struct PrototypeBinding {
    /// The recipe that builds it: `prototype.granite_boulder`.
    pub recipe: terrain_core::ids::RecipeKey,
    /// The seed this particular prototype was built from.
    ///
    /// A prototype is one *shape*, and a population using six shapes registers
    /// six prototypes with six seeds. This is what distinguishes them.
    pub seed: u64,
    pub parameters: terrain_core::document::ParameterObject,
    /// A bound on the prototype's own geometry, before instancing, in metres.
    ///
    /// Carried so an instance's world bound can be computed without building
    /// the mesh — which the scene builder must be able to do, since it does not
    /// build meshes.
    pub radius_m: f32,
}

impl Digestible for PrototypeBinding {
    fn absorb(&self, digest: &mut Digest) {
        digest.str(self.recipe.as_str()).u64(self.seed);
        self.parameters.absorb(digest);
        digest.f32(self.radius_m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mark::Stratum;
    use terrain_core::document::ParameterObject;
    use terrain_core::ids::RecipeKey;

    fn instance(id: u64) -> PrototypeInstance {
        PrototypeInstance {
            stable_id: MarkId(id),
            order: PainterOrder::new(Stratum::Canopy, 0.0, 0, MarkId(id)),
            prototype: PrototypeIndex(0),
            material: SceneMaterialIndex(0),
            position: ScenePoint::default(),
            yaw_rad: 0.0,
            tilt_rad: 0.0,
            tilt_azimuth_rad: 0.0,
            scale: [1.0, 1.0, 1.0],
            attributes: MarkAttributes::default(),
            bounds: Aabb3::around(ScenePoint::default(), 0.5),
        }
    }

    #[test]
    fn every_placement_parameter_reaches_the_digest() {
        let base = instance(1);
        let reference = base.fingerprint("instance");

        type Nudge = (&'static str, fn(&mut PrototypeInstance));
        let nudges: [Nudge; 9] = [
            ("prototype", |i| i.prototype = PrototypeIndex(1)),
            ("material", |i| i.material = SceneMaterialIndex(1)),
            ("position.u", |i| i.position.u_m += 1.0),
            ("position.z", |i| i.position.z_m += 1.0),
            ("yaw", |i| i.yaw_rad += 1.0),
            ("tilt", |i| i.tilt_rad += 1.0),
            ("tilt_azimuth", |i| i.tilt_azimuth_rad += 1.0),
            ("scale.x", |i| i.scale[0] += 1.0),
            ("scale.z", |i| i.scale[2] += 1.0),
        ];
        for (name, nudge) in nudges {
            let mut moved = base;
            nudge(&mut moved);
            assert_ne!(
                reference,
                moved.fingerprint("instance"),
                "{name} does not reach the digest"
            );
        }
    }

    #[test]
    fn a_prototype_is_a_recipe_and_a_seed_rather_than_a_mesh() {
        // Two prototypes from one recipe are two shapes, and that is what a
        // population with six rock shapes registers.
        let binding = |seed: u64| PrototypeBinding {
            recipe: RecipeKey::new("prototype.granite_boulder").expect("valid"),
            seed,
            parameters: ParameterObject::new(),
            radius_m: 0.4,
        };
        assert_ne!(
            binding(1).fingerprint("prototype"),
            binding(2).fingerprint("prototype")
        );
        assert_eq!(
            binding(1).fingerprint("prototype"),
            binding(1).fingerprint("prototype")
        );
    }

    #[test]
    fn the_largest_scale_is_what_a_bound_grows_by() {
        let mut stretched = instance(1);
        stretched.scale = [0.5, 2.5, 1.0];
        assert_eq!(stretched.max_scale(), 2.5);
        // Including a mirrored axis, which is still that much geometry.
        stretched.scale = [-3.0, 1.0, 1.0];
        assert_eq!(stretched.max_scale(), 3.0);
    }
}

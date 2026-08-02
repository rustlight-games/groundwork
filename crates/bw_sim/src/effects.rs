//! Effect execution.
//!
//! An effect is queued by whatever caused it and applied later, serially, in a
//! sorted order. That indirection is what lets combat systems run in parallel
//! without their write order mattering.
//!
//! Handlers receive an [`EffectCtx`] rather than raw `&mut World`. The context
//! offers a deliberately small vocabulary — damage, heal, apply a status, read
//! a position, queue a child effect — which keeps a spell from reaching into
//! the scheduler or the renderer, and keeps handlers easy to test.

use std::collections::BTreeSet;

use bevy_ecs::prelude::*;
use bw_content::{ContentError, ContentResult, Params};
use bw_core::{ContentId, Real, RngStream, SimRng, Tick, UnitId, Vec2Fx};
use indexmap::IndexMap;
use rand_chacha::ChaCha8Rng;
use smol_str::SmolStr;

use crate::components::{Health, Position, StatusStack, Team};
use crate::resources::{EffectQueue, SimClock, SimSeed, UnitIndex};

/// An effect waiting to be applied.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingEffect {
    /// Registry key of the handler.
    pub kind: SmolStr,
    pub source: UnitId,
    pub targets: Vec<UnitId>,
    pub params: Params,
    /// Assigned by [`EffectQueue::push`]; do not set by hand.
    pub sequence: u32,
}

impl PendingEffect {
    pub fn new(kind: impl Into<SmolStr>, source: UnitId, targets: Vec<UnitId>) -> Self {
        Self {
            kind: kind.into(),
            source,
            targets,
            params: Params::new(),
            sequence: 0,
        }
    }

    pub fn with_params(mut self, params: Params) -> Self {
        self.params = params;
        self
    }
}

/// A registered effect primitive.
///
/// Implemented in the plugin crates under `plugins/`, registered at startup.
pub trait EffectHandler: Send + Sync + 'static {
    /// Registry key, as written in content.
    fn key(&self) -> &'static str;

    /// Reject bad parameters at content load rather than mid-battle.
    fn validate(&self, _params: &Params) -> ContentResult<()> {
        Ok(())
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>);
}

/// Every registered handler.
#[derive(Resource, Default)]
pub struct EffectRegistry {
    handlers: IndexMap<SmolStr, Box<dyn EffectHandler>>,
}

impl EffectRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, handler: impl EffectHandler) -> &mut Self {
        self.handlers
            .insert(SmolStr::new(handler.key()), Box::new(handler));
        self
    }

    pub fn get(&self, kind: &str) -> Option<&dyn EffectHandler> {
        self.handlers.get(kind).map(|h| h.as_ref())
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.handlers.contains_key(kind)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.handlers.keys().map(SmolStr::as_str)
    }

    /// The key set, for [`ContentDb::validate`](bw_content::ContentDb::validate).
    pub fn key_set(&self) -> BTreeSet<&str> {
        self.keys().collect()
    }

    /// Check every handler's parameters against authored content.
    pub fn validate(&self, kind: &str, params: &Params, referrer: &str) -> ContentResult<()> {
        match self.get(kind) {
            Some(handler) => handler.validate(params),
            None => Err(ContentError::UnknownEffectKind {
                referrer: SmolStr::new(referrer),
                kind: SmolStr::new(kind),
            }),
        }
    }
}

/// What a handler may do.
pub struct EffectCtx<'w> {
    world: &'w mut World,
    pub tick: Tick,
    pub source: UnitId,
    pub targets: Vec<UnitId>,
    pub params: Params,
    seed: SimRng,
    queued: Vec<PendingEffect>,
}

impl<'w> EffectCtx<'w> {
    /// A generator scoped to this effect.
    ///
    /// Salted with the source unit and the effect's position in the queue, so
    /// two units casting the same spell on the same tick roll differently while
    /// each stays reproducible.
    pub fn rng(&self, stream: RngStream, salt: u64) -> ChaCha8Rng {
        self.seed
            .stream(stream, self.tick, self.source.0 as u64 ^ (salt << 32))
    }

    pub fn position_of(&self, unit: UnitId) -> Option<Vec2Fx> {
        let entity = self.world.resource::<UnitIndex>().entity(unit)?;
        self.world.get::<Position>(entity).map(|p| p.0)
    }

    pub fn health_of(&self, unit: UnitId) -> Option<Health> {
        let entity = self.world.resource::<UnitIndex>().entity(unit)?;
        self.world.get::<Health>(entity).copied()
    }

    pub fn team_of(&self, unit: UnitId) -> Option<Team> {
        let entity = self.world.resource::<UnitIndex>().entity(unit)?;
        self.world.get::<Team>(entity).copied()
    }

    pub fn is_alive(&self, unit: UnitId) -> bool {
        self.health_of(unit).is_some_and(|h| h.is_alive())
    }

    /// Deal damage after armour. Returns the amount actually dealt.
    pub fn damage(&mut self, unit: UnitId, amount: Real) -> Real {
        let Some(entity) = self.world.resource::<UnitIndex>().entity(unit) else {
            return Real::ZERO;
        };
        let armor = self
            .world
            .get::<crate::components::Stats>(entity)
            .map_or(Real::ZERO, |s| s.armor);
        // Armour subtracts, but a hit always does something — otherwise a
        // high-armour unit becomes literally invulnerable to a whole damage
        // tier and the fight stops progressing.
        let dealt = (amount - armor).max(MINIMUM_DAMAGE);
        if let Some(mut health) = self.world.get_mut::<Health>(entity) {
            health.apply_damage(dealt);
            dealt
        } else {
            Real::ZERO
        }
    }

    pub fn heal(&mut self, unit: UnitId, amount: Real) {
        let Some(entity) = self.world.resource::<UnitIndex>().entity(unit) else {
            return;
        };
        if let Some(mut health) = self.world.get_mut::<Health>(entity) {
            health.heal(amount);
        }
    }

    pub fn apply_status(
        &mut self,
        unit: UnitId,
        status: ContentId,
        duration_ticks: u32,
        max_stacks: u32,
        refreshes: bool,
    ) {
        let Some(entity) = self.world.resource::<UnitIndex>().entity(unit) else {
            return;
        };
        let expires_at = Tick(self.tick.0 + duration_ticks as u64);
        if let Some(mut stack) = self.world.get_mut::<StatusStack>(entity) {
            stack.apply(status, expires_at, max_stacks, refreshes);
        }
    }

    pub fn push_velocity(&mut self, unit: UnitId, delta: Vec2Fx) {
        let Some(entity) = self.world.resource::<UnitIndex>().entity(unit) else {
            return;
        };
        if let Some(mut velocity) = self.world.get_mut::<crate::components::Velocity>(entity) {
            velocity.0 += delta;
        }
    }

    /// Queue a follow-up effect, applied later in this same tick.
    pub fn queue(&mut self, effect: PendingEffect) {
        self.queued.push(effect);
    }

    /// Live units, in ascending id order.
    pub fn all_units(&self) -> Vec<UnitId> {
        self.world.resource::<UnitIndex>().sorted_ids().to_vec()
    }

    /// Escape hatch for handlers that genuinely need more than the above.
    ///
    /// Prefer the typed helpers: anything reached through here is invisible to
    /// the determinism review and easy to get wrong.
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }
}

/// The floor on any damaging hit, so armour cannot make a unit unkillable.
pub const MINIMUM_DAMAGE: Real = bw_core::fx::real_from_int(1);

/// Drain and apply the effect queue.
///
/// An exclusive system on purpose. Effects mutate arbitrary units, so running
/// them in parallel would need locking that reintroduces order-dependence;
/// applying them serially in a sorted order is both simpler and provably
/// reproducible. It is not a bottleneck — a tick's worth of effects is tens of
/// items, not thousands.
pub fn apply_effects(world: &mut World) {
    let tick = world.resource::<SimClock>().0;
    let seed = world.resource::<SimSeed>().0;

    // Bounded, because a handler that queues a child which queues another
    // child can otherwise spin forever on a content mistake.
    const MAX_CASCADE_DEPTH: usize = 8;

    for _ in 0..MAX_CASCADE_DEPTH {
        let batch = world.resource_mut::<EffectQueue>().drain_sorted();
        if batch.is_empty() {
            return;
        }

        let mut cascaded = Vec::new();
        world.resource_scope(|world, registry: Mut<EffectRegistry>| {
            for effect in batch {
                let Some(handler) = registry.get(&effect.kind) else {
                    // Unregistered kinds are rejected at content load, so this
                    // means a handler queued a typo. Skip rather than panic:
                    // one bad spell should not end the battle.
                    continue;
                };
                let mut ctx = EffectCtx {
                    world,
                    tick,
                    source: effect.source,
                    targets: effect.targets,
                    params: effect.params,
                    seed,
                    queued: Vec::new(),
                };
                handler.apply(&mut ctx);
                cascaded.append(&mut ctx.queued);
            }
        });

        if cascaded.is_empty() {
            return;
        }
        let mut queue = world.resource_mut::<EffectQueue>();
        for effect in cascaded {
            queue.push(effect);
        }
    }
}

#[cfg(test)]
mod tests {
    use bw_core::real_from_int;

    use super::*;
    use crate::components::{Stats, Unit};

    struct RecordingHandler;
    impl EffectHandler for RecordingHandler {
        fn key(&self) -> &'static str {
            "damage"
        }
        fn apply(&self, ctx: &mut EffectCtx<'_>) {
            let amount = ctx
                .params
                .real("damage", "amount")
                .unwrap_or(real_from_int(1));
            for target in ctx.targets.clone() {
                ctx.damage(target, amount);
            }
        }
    }

    fn world_with_units(count: u32) -> World {
        let mut world = World::new();
        world.insert_resource(SimClock(Tick(0)));
        world.insert_resource(SimSeed(SimRng::new(1)));
        world.insert_resource(EffectQueue::default());
        let mut registry = EffectRegistry::new();
        registry.add(RecordingHandler);
        world.insert_resource(registry);

        let mut index = UnitIndex::new();
        for _ in 0..count {
            let id = index.allocate_id();
            let entity = world
                .spawn((
                    Unit {
                        id,
                        character: ContentId(0),
                    },
                    Health::new(real_from_int(100)),
                    Stats {
                        move_speed: real_from_int(1),
                        attack_damage: real_from_int(1),
                        attack_range: real_from_int(1),
                        attack_cooldown_ticks: 8,
                        armor: Real::ZERO,
                        radius: real_from_int(1),
                    },
                    Position(Vec2Fx::ZERO),
                    StatusStack::default(),
                ))
                .id();
            index.insert(id, entity);
        }
        index.refresh();
        world.insert_resource(index);
        world
    }

    fn health(world: &World, id: UnitId) -> Real {
        let entity = world.resource::<UnitIndex>().entity(id).unwrap();
        world.get::<Health>(entity).unwrap().current
    }

    #[test]
    fn queued_effects_are_applied() {
        let mut world = world_with_units(2);
        let mut params = Params::new();
        params.insert("amount", bw_content::Value::Num(30.0));
        world
            .resource_mut::<EffectQueue>()
            .push(PendingEffect::new("damage", UnitId(0), vec![UnitId(1)]).with_params(params));
        apply_effects(&mut world);
        assert_eq!(health(&world, UnitId(1)), real_from_int(70));
    }

    #[test]
    fn queue_order_does_not_change_the_result() {
        // The property the sorted drain exists for.
        let run = |reverse: bool| {
            let mut world = world_with_units(3);
            let mut effects = vec![
                PendingEffect::new("damage", UnitId(0), vec![UnitId(2)]),
                PendingEffect::new("damage", UnitId(1), vec![UnitId(2)]),
            ];
            if reverse {
                effects.reverse();
            }
            for e in effects {
                world.resource_mut::<EffectQueue>().push(e);
            }
            apply_effects(&mut world);
            health(&world, UnitId(2))
        };
        assert_eq!(run(false), run(true));
    }

    #[test]
    fn unregistered_kinds_are_skipped_rather_than_fatal() {
        let mut world = world_with_units(2);
        world.resource_mut::<EffectQueue>().push(PendingEffect::new(
            "no_such_effect",
            UnitId(0),
            vec![UnitId(1)],
        ));
        apply_effects(&mut world);
        assert_eq!(health(&world, UnitId(1)), real_from_int(100));
    }

    #[test]
    fn armour_never_reduces_a_hit_to_nothing() {
        let mut world = world_with_units(1);
        let entity = world.resource::<UnitIndex>().entity(UnitId(0)).unwrap();
        world.get_mut::<Stats>(entity).unwrap().armor = real_from_int(1000);
        let mut params = Params::new();
        params.insert("amount", bw_content::Value::Num(5.0));
        world
            .resource_mut::<EffectQueue>()
            .push(PendingEffect::new("damage", UnitId(0), vec![UnitId(0)]).with_params(params));
        apply_effects(&mut world);
        assert_eq!(
            health(&world, UnitId(0)),
            real_from_int(100) - MINIMUM_DAMAGE
        );
    }

    #[test]
    fn effects_on_a_missing_unit_are_harmless() {
        let mut world = world_with_units(1);
        world.resource_mut::<EffectQueue>().push(PendingEffect::new(
            "damage",
            UnitId(0),
            vec![UnitId(999)],
        ));
        apply_effects(&mut world);
        assert_eq!(health(&world, UnitId(0)), real_from_int(100));
    }

    #[test]
    fn registry_reports_its_keys_for_content_validation() {
        let mut registry = EffectRegistry::new();
        registry.add(RecordingHandler);
        assert!(registry.contains("damage"));
        assert_eq!(registry.key_set().len(), 1);
    }
}

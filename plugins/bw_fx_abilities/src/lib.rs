//! Spell and ability primitives.
//!
//! Every handler here is a small, composable verb. Abilities are built from
//! them in RON rather than in Rust — see `bw_content::effect`. The set is meant
//! to stay small: if a new spell needs a new primitive, that is a signal the
//! primitive is worth having, but most new spells should need none.
//!
//! Skeleton: `damage`, `heal`, `apply_status` and `sequence` are implemented as
//! worked examples of the shape. `knockback`, `projectile`, `chain`, `aura` and
//! `summon` are the obvious next ones and follow the same pattern.

#![forbid(unsafe_code)]

use bw_content::{ContentResult, Params};
use bw_core::{ContentId, Real, RngStream};
use bw_sim::effects::{EffectCtx, EffectHandler, EffectRegistry};

/// Register every ability primitive.
///
/// Called once at startup by both the game and the trainer.
pub fn register(registry: &mut EffectRegistry) {
    registry
        .add(Damage)
        .add(Heal)
        .add(ApplyStatus)
        .add(Sequence);
}

/// Deal damage to every target.
///
/// Parameters: `amount` (number, required), `variance` (number, optional
/// fraction of `amount`).
pub struct Damage;

impl EffectHandler for Damage {
    fn key(&self) -> &'static str {
        "damage"
    }

    fn validate(&self, params: &Params) -> ContentResult<()> {
        params.real("damage", "amount")?;
        Ok(())
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>) {
        let Ok(base) = ctx.params.real("damage", "amount") else {
            return;
        };
        let variance = ctx
            .params
            .real_or("damage", "variance", Real::ZERO)
            .unwrap_or(Real::ZERO);

        for (index, target) in ctx.targets.clone().into_iter().enumerate() {
            let amount = if variance == Real::ZERO {
                base
            } else {
                // Salted by target index so two targets of one cast roll
                // independently while the whole cast stays reproducible.
                use rand::Rng;
                let mut rng = ctx.rng(RngStream::Damage, index as u64);
                let roll = rng.random_range(0..=1000i32);
                let swing = base * variance * Real::from_num(roll) / Real::from_num(1000);
                base - (base * variance / Real::from_num(2)) + swing
            };
            ctx.damage(target, amount.max(Real::ZERO));
        }
    }
}

/// Restore health to every target.
///
/// Parameters: `amount` (number, required).
pub struct Heal;

impl EffectHandler for Heal {
    fn key(&self) -> &'static str {
        "heal"
    }

    fn validate(&self, params: &Params) -> ContentResult<()> {
        params.real("heal", "amount")?;
        Ok(())
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>) {
        let Ok(amount) = ctx.params.real("heal", "amount") else {
            return;
        };
        for target in ctx.targets.clone() {
            ctx.heal(target, amount);
        }
    }
}

/// Attach a status to every target.
///
/// Parameters: `status` (integer content id, required), `duration_ticks`
/// (integer, required), `max_stacks` (integer, default 1), `refreshes` (bool,
/// default true).
pub struct ApplyStatus;

impl EffectHandler for ApplyStatus {
    fn key(&self) -> &'static str {
        "apply_status"
    }

    fn validate(&self, params: &Params) -> ContentResult<()> {
        params.int("apply_status", "status")?;
        params.int("apply_status", "duration_ticks")?;
        Ok(())
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>) {
        let (Ok(status), Ok(duration)) = (
            ctx.params.int("apply_status", "status"),
            ctx.params.int("apply_status", "duration_ticks"),
        ) else {
            return;
        };
        let max_stacks = ctx
            .params
            .int_or("apply_status", "max_stacks", 1)
            .unwrap_or(1);
        let refreshes = ctx
            .params
            .bool_or("apply_status", "refreshes", true)
            .unwrap_or(true);

        for target in ctx.targets.clone() {
            ctx.apply_status(
                target,
                ContentId(status.max(0) as u32),
                duration.max(0) as u32,
                max_stacks.max(1) as u32,
                refreshes,
            );
        }
    }
}

/// Does nothing on its own; exists so an ability can hold several children.
///
/// The effect tree needs a way to say "all of these", and giving that its own
/// key keeps the walker uniform rather than special-casing the root.
pub struct Sequence;

impl EffectHandler for Sequence {
    fn key(&self) -> &'static str {
        "sequence"
    }

    fn apply(&self, _ctx: &mut EffectCtx<'_>) {}
}

#[cfg(test)]
mod tests {
    use bw_content::{TerrainMap, Value};
    use bw_core::{Grid, GridDims, TeamId, UnitId, Vec2Fx, real_from_int};
    use bw_sim::components::{Health, Stats};
    use bw_sim::effects::PendingEffect;
    use bw_sim::resources::UnitIndex;
    use bw_sim::{BattleConfig, BattleSim, EffectQueue, SpawnStats, UnitSpawn};

    use super::*;

    fn sim() -> BattleSim {
        let mut registry = EffectRegistry::new();
        register(&mut registry);
        let grid = Grid::centered(GridDims::new(16, 16), real_from_int(1));
        let mut sim = BattleSim::new(BattleConfig::default(), TerrainMap::new(grid), registry);
        for i in 0..2 {
            sim.spawn_unit(UnitSpawn {
                character: ContentId(0),
                team: if i == 0 {
                    TeamId::PLAYER
                } else {
                    TeamId::ENEMY
                },
                // Far apart, so basic attacks do not interfere with the test.
                position: Vec2Fx::from_ints(i * 12, 0),
                stats: SpawnStats {
                    max_health: real_from_int(100),
                    combat: Stats {
                        move_speed: Real::ZERO,
                        attack_damage: real_from_int(1),
                        attack_range: real_from_int(1),
                        attack_cooldown_ticks: 1000,
                        armor: Real::ZERO,
                        radius: Real::from_num(0.5),
                    },
                },
                abilities: vec![],
            });
        }
        sim
    }

    fn health(sim: &BattleSim, id: UnitId) -> Real {
        let index = sim.world().resource::<UnitIndex>();
        let entity = index.entity(id).unwrap();
        sim.world().get::<Health>(entity).unwrap().current
    }

    fn queue(sim: &mut BattleSim, kind: &str, params: Params) {
        sim.world_mut()
            .resource_mut::<EffectQueue>()
            .push(PendingEffect::new(kind, UnitId(0), vec![UnitId(1)]).with_params(params));
    }

    #[test]
    fn damage_reduces_health_by_the_authored_amount() {
        let mut sim = sim();
        let mut params = Params::new();
        params.insert("amount", Value::Num(25.0));
        queue(&mut sim, "damage", params);
        sim.step();
        assert_eq!(health(&sim, UnitId(1)), real_from_int(75));
    }

    #[test]
    fn heal_restores_but_does_not_overheal() {
        let mut sim = sim();
        let mut damage = Params::new();
        damage.insert("amount", Value::Num(40.0));
        queue(&mut sim, "damage", damage);
        sim.step();

        let mut heal = Params::new();
        heal.insert("amount", Value::Num(1000.0));
        queue(&mut sim, "heal", heal);
        sim.step();
        assert_eq!(health(&sim, UnitId(1)), real_from_int(100));
    }

    #[test]
    fn damage_variance_is_reproducible_and_centred() {
        let run = || {
            let mut sim = sim();
            let mut params = Params::new();
            params.insert("amount", Value::Num(50.0));
            params.insert("variance", Value::Num(0.5));
            queue(&mut sim, "damage", params);
            sim.step();
            health(&sim, UnitId(1))
        };
        let first = run();
        assert_eq!(first, run(), "variance must be reproducible");
        let dealt = real_from_int(100) - first;
        assert!(
            dealt >= real_from_int(24) && dealt <= real_from_int(76),
            "dealt {dealt}"
        );
    }

    #[test]
    fn validation_rejects_content_missing_a_required_parameter() {
        assert!(Damage.validate(&Params::new()).is_err());
        assert!(Heal.validate(&Params::new()).is_err());
        assert!(ApplyStatus.validate(&Params::new()).is_err());
        // Sequence takes no parameters, so an empty bag is fine.
        assert!(Sequence.validate(&Params::new()).is_ok());
    }

    #[test]
    fn register_adds_every_primitive() {
        let mut registry = EffectRegistry::new();
        register(&mut registry);
        for key in ["damage", "heal", "apply_status", "sequence"] {
            assert!(registry.contains(key), "{key} was not registered");
        }
    }
}

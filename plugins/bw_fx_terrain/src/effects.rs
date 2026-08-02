//! Terrain effects.
//!
//! Spells that change the ground rather than the units standing on it. These
//! write into the [`Battlefield`]'s cost field, which invalidates cached flow
//! fields — the reason [`Battlefield::add_cost`] exists rather than letting
//! callers touch the cost field directly.

use bw_content::{ContentResult, Params};
use bw_core::GridPos;
use bw_sim::effects::{EffectCtx, EffectHandler, EffectRegistry};
use bw_sim::resources::Battlefield;

/// Register every terrain effect.
pub fn register_effects(registry: &mut EffectRegistry) {
    registry.add(Mud);
}

/// Slow the ground in a radius around the caster.
///
/// Parameters: `radius` (number, required, in cells), `extra_cost` (integer,
/// default 512).
pub struct Mud;

impl EffectHandler for Mud {
    fn key(&self) -> &'static str {
        "terrain_mud"
    }

    fn validate(&self, params: &Params) -> ContentResult<()> {
        params.real("terrain_mud", "radius")?;
        Ok(())
    }

    fn apply(&self, ctx: &mut EffectCtx<'_>) {
        let Ok(radius) = ctx.params.real("terrain_mud", "radius") else {
            return;
        };
        let extra = ctx
            .params
            .int_or("terrain_mud", "extra_cost", 512)
            .unwrap_or(512);
        let Some(origin) = ctx.position_of(ctx.source) else {
            return;
        };

        let world = ctx.world_mut();
        let mut battlefield = world.resource_mut::<Battlefield>();
        let grid = *battlefield.costs.grid();
        let center = grid.world_to_cell(origin);
        let span = radius.to_num::<i32>().max(0);

        let mut affected = Vec::new();
        for dy in -span..=span {
            for dx in -span..=span {
                let cell = GridPos::new(center.x + dx, center.y + dy);
                if dx * dx + dy * dy <= span * span && grid.contains(cell) {
                    affected.push(cell);
                }
            }
        }
        for cell in affected {
            battlefield
                .costs
                .add_cost(cell, extra.clamp(0, u16::MAX as i64) as u16);
        }
        // One invalidation for the whole patch rather than one per cell.
        battlefield.flow.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use bw_content::{TerrainMap, Value};
    use bw_core::{ContentId, Grid, GridDims, Real, TeamId, UnitId, Vec2Fx, real_from_int};
    use bw_sim::components::Stats;
    use bw_sim::effects::PendingEffect;
    use bw_sim::{BattleConfig, BattleSim, EffectQueue, SpawnStats, UnitSpawn};

    use super::*;

    fn sim() -> BattleSim {
        let mut registry = EffectRegistry::new();
        register_effects(&mut registry);
        let grid = Grid::centered(GridDims::new(32, 32), real_from_int(1));
        let mut sim = BattleSim::new(BattleConfig::default(), TerrainMap::new(grid), registry);
        sim.spawn_unit(UnitSpawn {
            character: ContentId(0),
            team: TeamId::PLAYER,
            position: Vec2Fx::ZERO,
            stats: SpawnStats {
                max_health: real_from_int(10),
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
        sim
    }

    fn cast_mud(sim: &mut BattleSim, radius: f64) {
        let mut params = Params::new();
        params.insert("radius", Value::Num(radius));
        sim.world_mut()
            .resource_mut::<EffectQueue>()
            .push(PendingEffect::new("terrain_mud", UnitId(0), vec![]).with_params(params));
        sim.step();
    }

    #[test]
    fn mud_raises_the_cost_under_the_caster() {
        let mut sim = sim();
        let before = {
            let battlefield = sim.world().resource::<Battlefield>();
            let cell = battlefield.costs.grid().world_to_cell(Vec2Fx::ZERO);
            battlefield.costs.cost(cell)
        };
        cast_mud(&mut sim, 3.0);
        let after = {
            let battlefield = sim.world().resource::<Battlefield>();
            let cell = battlefield.costs.grid().world_to_cell(Vec2Fx::ZERO);
            battlefield.costs.cost(cell)
        };
        assert!(after > before, "{after} should exceed {before}");
    }

    #[test]
    fn mud_leaves_distant_ground_alone() {
        let mut sim = sim();
        cast_mud(&mut sim, 2.0);
        let battlefield = sim.world().resource::<Battlefield>();
        let far = battlefield
            .costs
            .grid()
            .world_to_cell(Vec2Fx::from_ints(14, 14));
        assert_eq!(
            battlefield.costs.cost(far),
            bw_content::terrain::NORMAL_COST
        );
    }

    #[test]
    fn casting_mud_invalidates_cached_paths() {
        // A stale flow field would route units straight through the mud.
        let mut sim = sim();
        {
            let battlefield = &mut *sim.world_mut().resource_mut::<Battlefield>();
            let Battlefield { costs, flow, .. } = battlefield;
            flow.get_or_build(costs, GridPos::new(4, 4));
            assert_eq!(flow.len(), 1);
        }
        cast_mud(&mut sim, 3.0);
        assert!(sim.world().resource::<Battlefield>().flow.is_empty());
    }

    #[test]
    fn validation_requires_a_radius() {
        assert!(Mud.validate(&Params::new()).is_err());
    }
}

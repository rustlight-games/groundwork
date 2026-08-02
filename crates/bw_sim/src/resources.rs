//! Simulation resources.

use bevy_ecs::prelude::*;
use bw_content::TerrainMap;
use bw_core::{SimRng, Tick, UnitId};
use bw_nav::{CostField, FlowFieldCache};
use indexmap::IndexMap;

/// Current tick.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimClock(pub Tick);

/// Root randomness for this battle.
#[derive(Resource, Clone, Copy, Debug)]
pub struct SimSeed(pub SimRng);

/// The battlefield: terrain, derived costs, and cached flow fields.
#[derive(Resource)]
pub struct Battlefield {
    pub terrain: TerrainMap,
    pub costs: CostField,
    pub flow: FlowFieldCache,
}

impl Battlefield {
    pub fn new(terrain: TerrainMap) -> Self {
        let costs = CostField::from_terrain(&terrain);
        Self {
            terrain,
            costs,
            flow: FlowFieldCache::new(),
        }
    }

    /// Apply a cost change and drop cached fields.
    ///
    /// Always paired, because a stale flow field routes units through mud they
    /// should now be avoiding — and the failure is silent.
    pub fn add_cost(&mut self, pos: bw_core::GridPos, extra: u16) {
        self.costs.add_cost(pos, extra);
        self.flow.invalidate_all();
    }

    pub fn set_blocked(&mut self, pos: bw_core::GridPos, blocked: bool) {
        self.costs.set_blocked(pos, blocked);
        self.flow.invalidate_all();
    }
}

/// Maps [`UnitId`] to `Entity`, and holds the canonical iteration order.
///
/// Systems that must be deterministic iterate [`sorted_ids`] and look entities
/// up here, rather than iterating a `Query`. Query iteration order follows
/// archetype and table layout, which shifts the moment a component is added or
/// removed — a unit gaining a status would quietly reorder the whole battle.
///
/// [`sorted_ids`]: UnitIndex::sorted_ids
#[derive(Resource, Default, Debug)]
pub struct UnitIndex {
    by_id: IndexMap<UnitId, Entity>,
    sorted: Vec<UnitId>,
    dirty: bool,
    next_id: u32,
}

impl UnitIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the next id. Never reused, so ids stay stable in replays.
    pub fn allocate_id(&mut self) -> UnitId {
        let id = UnitId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, id: UnitId, entity: Entity) {
        self.by_id.insert(id, entity);
        self.dirty = true;
    }

    pub fn remove(&mut self, id: UnitId) {
        self.by_id.shift_remove(&id);
        self.dirty = true;
    }

    pub fn entity(&self, id: UnitId) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Rebuild the sorted order. Called once per tick, before any system that
    /// depends on ordering.
    pub fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.sorted.clear();
        self.sorted.extend(self.by_id.keys().copied());
        self.sorted.sort_unstable();
        self.dirty = false;
    }

    /// Live unit ids in ascending order.
    ///
    /// Only valid after [`refresh`]. Debug builds assert rather than silently
    /// handing back a stale order.
    ///
    /// [`refresh`]: UnitIndex::refresh
    pub fn sorted_ids(&self) -> &[UnitId] {
        debug_assert!(!self.dirty, "UnitIndex::refresh must run before sorted_ids");
        &self.sorted
    }
}

/// Effects waiting to be applied.
///
/// Producers push from anywhere; the queue is sorted and drained once per tick
/// by a single exclusive system. That is what makes effect resolution
/// order-independent: it does not matter which system queued what first.
#[derive(Resource, Default)]
pub struct EffectQueue {
    pub pending: Vec<crate::effects::PendingEffect>,
    next_sequence: u32,
}

impl EffectQueue {
    pub fn push(&mut self, mut effect: crate::effects::PendingEffect) {
        effect.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.pending.push(effect);
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Take everything pending, in a deterministic order.
    ///
    /// Sorted by source unit, then by the order each source queued them. The
    /// sequence number alone would be enough for a serial producer, but systems
    /// may run in parallel, so the source id has to lead.
    pub fn drain_sorted(&mut self) -> Vec<crate::effects::PendingEffect> {
        let mut taken = std::mem::take(&mut self.pending);
        taken.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then_with(|| a.sequence.cmp(&b.sequence))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_allocated_without_reuse() {
        let mut index = UnitIndex::new();
        let a = index.allocate_id();
        let b = index.allocate_id();
        assert_eq!((a, b), (UnitId(0), UnitId(1)));
        index.remove(a);
        assert_eq!(index.allocate_id(), UnitId(2), "ids must never be recycled");
    }

    #[test]
    fn sorted_order_is_ascending_regardless_of_insertion_order() {
        let mut index = UnitIndex::new();
        let mut world = World::new();
        for id in [7u32, 2, 9, 1] {
            let e = world.spawn_empty().id();
            index.insert(UnitId(id), e);
        }
        index.refresh();
        assert_eq!(
            index.sorted_ids(),
            &[UnitId(1), UnitId(2), UnitId(7), UnitId(9)]
        );
    }

    #[test]
    fn removal_is_reflected_after_refresh() {
        let mut index = UnitIndex::new();
        let mut world = World::new();
        for id in 0..3u32 {
            let e = world.spawn_empty().id();
            index.insert(UnitId(id), e);
        }
        index.remove(UnitId(1));
        index.refresh();
        assert_eq!(index.sorted_ids(), &[UnitId(0), UnitId(2)]);
    }
}

//! Stable identifiers.
//!
//! Simulation code orders and hashes by [`UnitId`], never by `bevy_ecs::Entity`.
//! Entity ids depend on allocation and recycling order, which is an
//! implementation detail of the ECS rather than a property of the battle. A
//! `UnitId` is assigned in spawn order and never reused, so it is safe to sort
//! by and safe to write into a replay.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// A unit within a single battle. Unique for the battle's lifetime.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct UnitId(pub u32);

/// Which side a unit fights for.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TeamId(pub u8);

impl TeamId {
    pub const PLAYER: Self = Self(0);
    pub const ENEMY: Self = Self(1);

    pub fn is_hostile_to(self, other: Self) -> bool {
        self != other
    }
}

/// An interned reference to a content definition.
///
/// Content is authored with string keys, which are pleasant to write and awful
/// to compare in a hot loop. Interning resolves each key once at load time to a
/// dense integer that is cheap to copy, compare, and use as an array index.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ContentId(pub u32);

impl ContentId {
    /// Sentinel for "no content", so callers do not need `Option` everywhere.
    pub const NONE: Self = Self(u32::MAX);

    pub fn is_none(self) -> bool {
        self == Self::NONE
    }

    pub fn index(self) -> Option<usize> {
        (!self.is_none()).then_some(self.0 as usize)
    }
}

/// Bidirectional map between content keys and [`ContentId`]s.
///
/// Insertion order is the id order, and `IndexMap` preserves it, so iterating
/// an interner is deterministic. Loading content in a fixed order therefore
/// produces the same ids every run — which matters because ids end up in the
/// observation vectors fed to the network.
#[derive(Clone, Debug, Default)]
pub struct Interner {
    keys: IndexMap<SmolStr, ContentId>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `key`, returning its existing id if already present.
    pub fn intern(&mut self, key: impl Into<SmolStr>) -> ContentId {
        let key = key.into();
        if let Some(&id) = self.keys.get(&key) {
            return id;
        }
        let id = ContentId(self.keys.len() as u32);
        self.keys.insert(key, id);
        id
    }

    pub fn get(&self, key: &str) -> Option<ContentId> {
        self.keys.get(key).copied()
    }

    /// The key an id was interned from.
    pub fn resolve(&self, id: ContentId) -> Option<&str> {
        self.keys.get_index(id.index()?).map(|(k, _)| k.as_str())
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Iterate in id order.
    pub fn iter(&self) -> impl Iterator<Item = (ContentId, &str)> {
        self.keys.iter().map(|(k, &id)| (id, k.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable_and_idempotent() {
        let mut i = Interner::new();
        let a = i.intern("goblin");
        let b = i.intern("ogre");
        assert_eq!(i.intern("goblin"), a);
        assert_eq!(a, ContentId(0));
        assert_eq!(b, ContentId(1));
        assert_eq!(i.resolve(a), Some("goblin"));
    }

    #[test]
    fn same_insertion_order_gives_same_ids() {
        let build = || {
            let mut i = Interner::new();
            for k in ["a", "b", "c"] {
                i.intern(k);
            }
            i.iter()
                .map(|(id, k)| (id, k.to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn none_sentinel_resolves_to_nothing() {
        let i = Interner::new();
        assert!(ContentId::NONE.is_none());
        assert_eq!(i.resolve(ContentId::NONE), None);
    }

    #[test]
    fn teams_are_hostile_across_sides_only() {
        assert!(TeamId::PLAYER.is_hostile_to(TeamId::ENEMY));
        assert!(!TeamId::PLAYER.is_hostile_to(TeamId::PLAYER));
    }
}

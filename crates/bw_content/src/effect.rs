//! Composable effect specifications.
//!
//! An ability is a tree. Each node names a registered primitive by string key,
//! optionally selects targets, carries parameters, and may have children that
//! run against the targets it selected. A fireball, a chain lightning, and a
//! healing aura differ only in how those nodes are arranged.
//!
//! ```ron
//! EffectSpec(
//!     kind: "sequence",
//!     children: [
//!         EffectSpec(
//!             kind: "damage",
//!             targeting: Some(Targeting(
//!                 shape: Circle(radius: 3.0),
//!                 filter: Enemies,
//!                 sort: Nearest,
//!                 max_targets: 8,
//!             )),
//!             params: {"amount": Num(24.0), "school": Text("fire")},
//!         ),
//!     ],
//! )
//! ```
//!
//! Separating [`Targeting`] from the payload is what keeps the primitive count
//! low: one `damage` handler serves a cone, a chain, and a single-target nuke,
//! because the shape is data rather than code.

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::params::Params;

/// One node of an ability's effect tree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EffectSpec {
    /// Registry key of the handler that runs this node.
    pub kind: SmolStr,

    /// Who this node applies to. `None` means "whatever the parent selected",
    /// which is how a child inherits its parent's targets.
    #[serde(default)]
    pub targeting: Option<Targeting>,

    #[serde(default)]
    pub params: Params,

    #[serde(default)]
    pub children: Vec<EffectSpec>,
}

impl EffectSpec {
    pub fn new(kind: impl Into<SmolStr>) -> Self {
        Self {
            kind: kind.into(),
            ..Default::default()
        }
    }

    /// Every node in the tree, parents before children.
    pub fn walk(&self) -> impl Iterator<Item = &EffectSpec> {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            let node = stack.pop()?;
            // Push in reverse so siblings come out in authored order.
            stack.extend(node.children.iter().rev());
            Some(node)
        })
    }

    /// Longest root-to-leaf depth, used to reject pathological content.
    pub fn depth(&self) -> usize {
        1 + self.children.iter().map(Self::depth).max().unwrap_or(0)
    }
}

/// How a node picks the units it affects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Targeting {
    pub shape: TargetShape,
    pub filter: TargetFilter,
    #[serde(default)]
    pub sort: TargetSort,
    /// Zero means unlimited.
    #[serde(default)]
    pub max_targets: u32,
}

impl Default for Targeting {
    fn default() -> Self {
        Self {
            shape: TargetShape::SelfOnly,
            filter: TargetFilter::Any,
            sort: TargetSort::Nearest,
            max_targets: 1,
        }
    }
}

/// The region a node reaches. Distances are in world units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TargetShape {
    SelfOnly,
    Circle {
        radius: f64,
    },
    Cone {
        radius: f64,
        arc_degrees: f64,
    },
    Line {
        length: f64,
        width: f64,
    },
    /// Everything on the battlefield, for global effects.
    Everywhere,
}

/// Which side of the fight a node cares about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetFilter {
    Enemies,
    Allies,
    /// Allies excluding the caster — the usual choice for support abilities.
    OtherAllies,
    #[default]
    Any,
}

/// Which candidates win when there are more than `max_targets`.
///
/// Ties are always broken by ascending `UnitId`. Without that rule two units at
/// identical distance could be ordered differently between runs, and a battle
/// would diverge from an apparently harmless change in spawn order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetSort {
    #[default]
    Nearest,
    Farthest,
    LowestHealth,
    HighestHealth,
    /// Lowest `UnitId` first. Stable and cheap, for effects that do not care.
    Arbitrary,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Value;

    fn tree() -> EffectSpec {
        let mut root = EffectSpec::new("sequence");
        let mut hit = EffectSpec::new("damage");
        hit.params.insert("amount", Value::Num(24.0));
        hit.children.push(EffectSpec::new("apply_status"));
        root.children.push(hit);
        root.children.push(EffectSpec::new("knockback"));
        root
    }

    #[test]
    fn walk_visits_every_node_parents_first() {
        let kinds: Vec<_> = tree().walk().map(|n| n.kind.to_string()).collect();
        assert_eq!(kinds, ["sequence", "damage", "apply_status", "knockback"]);
    }

    #[test]
    fn depth_counts_the_longest_branch() {
        assert_eq!(tree().depth(), 3);
        assert_eq!(EffectSpec::new("damage").depth(), 1);
    }

    #[test]
    fn round_trips_through_ron() {
        let original = tree();
        let text = ron::ser::to_string(&original).unwrap();
        let parsed: EffectSpec = ron::from_str(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn absent_optional_fields_default() {
        let spec: EffectSpec = ron::from_str(r#"(kind: "damage")"#).unwrap();
        assert_eq!(spec.kind, "damage");
        assert!(spec.targeting.is_none());
        assert!(spec.children.is_empty());
        assert!(spec.params.is_empty());
    }
}

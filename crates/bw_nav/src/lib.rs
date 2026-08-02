//! Navigation.
//!
//! Auto-battler crowds converge on a small number of destinations: an enemy
//! line, a choke point, an objective. Running A* per unit re-solves almost the
//! same search hundreds of times per tick and scales badly exactly when the
//! battle gets interesting.
//!
//! Flow fields invert that. One Dijkstra sweep from the goal produces an
//! [`IntegrationField`] of distances over the whole map, which collapses into a
//! [`FlowField`] holding one direction per cell. After that, a unit's pathing
//! query is a single array lookup, and the cost of adding another hundred units
//! is a hundred array lookups. The sweep is shared, cached, and only redone
//! when the goal moves or the terrain changes.
//!
//! Everything is integer arithmetic, and every tie is broken by a fixed rule,
//! so two runs from the same seed produce the same paths.

#![forbid(unsafe_code)]

pub mod avoidance;
pub mod cost;
pub mod field;

pub use avoidance::{SpatialHash, separation};
pub use cost::CostField;
pub use field::{FlowField, FlowFieldCache, IntegrationField, UNREACHABLE};

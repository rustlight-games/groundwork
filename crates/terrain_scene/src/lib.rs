//! The scene: what to render, built once, consumed by every renderer.
//!
//! ```text
//! PreparedTerrain  ──sample──►  TerrainScene  ──┬──►  Cycles
//!                                               ├──►  cheap rasteriser
//!                                               ├──►  debug plates
//!                                               └──►  dataset pairs
//! ```
//!
//! This is the boundary the whole framework is arranged around, and the reason
//! is one requirement that sounds modest and is not: **a training pair must be
//! one meadow**. If the cheap render and the expensive render come from two
//! generation passes, a network trained on them learns to hallucinate rather
//! than to reconstruct, and nothing says so — the loss simply stops falling and
//! no image in the corpus looks wrong.
//!
//! Generating twice would nearly work. Placement is a pure function of world
//! position, so two passes would agree today. It is not today's agreement that
//! matters; it is that nothing can *later* make them disagree. A quality tier
//! that skips a fork, a rib count that moves a vertex, an optimisation that
//! reorders a draw — any of those silently turns one meadow into two.
//!
//! ## What lives here
//!
//! - [`projection`] — how ground becomes screen. A contract between four
//!   renderers rather than one renderer's detail.
//! - [`ground`] — the terrain, sampled onto an edge-anchored lattice so that
//!   independently built neighbours agree along their join.
//! - [`mark`] — ribbons, curves, analytic shapes and stamps, plus the total
//!   painter order that decides what draws over what.
//! - [`instance`] — prototypes, for geometry that is distinctive and expensive
//!   rather than varied and cheap.
//! - [`layout`] — the square world tiles a render is *about*, and which one of
//!   them is the subject. A composition request laid over continuous terrain,
//!   never a generation boundary.
//! - [`scene`] — the whole thing, and the builder that produces it.
//!
//! ## What may not live here
//!
//! No Bevy entity ids or handles. No Blender objects. No GPU bind groups. No
//! Python-side offsets. The moment one appears, this stops being the thing every
//! renderer consumes and becomes one renderer's input the others adapt from.
//!
//! Nothing here knows about light, either. A mark carries how *old* it is and
//! how *wet* its ground is — intrinsic properties of the plant — and not how
//! much it catches the current sun. That separation is what lets a scene survive
//! a lighting change without being regenerated.

#![forbid(unsafe_code)]

pub mod ground;
pub mod instance;
pub mod layout;
pub mod mark;
pub mod projection;
pub mod scene;

pub use ground::{GroundMaterialChannel, GroundModifierChannel, GroundSurface};
pub use instance::{PrototypeBinding, PrototypeIndex, PrototypeInstance};
pub use layout::{
    IsoTileLayout, LayoutError, TileLayoutPreset, TileRole, WorldTile, WorldTileCoord,
};
pub use mark::{
    Aabb3, AnalyticMark, CurveMark, MarkAttributes, MarkId, PainterOrder, RibbonGeometry,
    RibbonMark, SceneMark, SceneMaterialBinding, SceneMaterialIndex, StampMark, Stratum, TipShape,
    WidthProfile,
};
pub use projection::{Projection, ScenePoint, ScreenPoint, ScreenRect};
pub use scene::{SCENE_DIGEST_DOMAIN, SceneBuilder, SceneRequest, StampBinding, TerrainScene};

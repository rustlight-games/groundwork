//! Which family of colour a mark belongs to.
//!
//! Lifted out of `palette.rs`, and the lift is the point rather than the tidying.
//! A tone is an **intrinsic property of a plant** — this is a blade, that is
//! thatch, that is a broad leaf — and the generator decides it while placing the
//! mark. The *ramps* a tone shades through are a rasteriser's business entirely,
//! and a path tracer never looks at them.
//!
//! While the two lived in one module, deciding what a mark was made of required
//! depending on a table of measured colours. The generator now names a family and
//! nothing more; `palette` maps families to paint, and `terrain_scene` maps them
//! to appearance keys.

/// Which ramp a pixel is shaded through.
///
/// A small closed set on purpose: every material in the field is one of these,
/// and a pixel that cannot say which one it is has no business being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Tone {
    /// Bare earth: olive-brown, never reddish.
    Soil = 0,
    /// The dark mat under the canopy.
    Thatch = 1,
    /// Ordinary blades — most of the field.
    Grass = 2,
    /// Broadleaf clusters, a shade cooler and flatter than blades.
    Leaf = 3,
    /// Dry stems and the odd bleached tuft.
    Dry = 4,
}

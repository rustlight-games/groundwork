// Placeholder. The grass renderer currently draws chunks as flat quads from
// Rust (see crates/bw_grass); this file is where the instanced-blade shader
// will live.
//
// The plan, in the order it should be built:
//   1. Per-blade instance data: root position, height, phase offset, tint.
//   2. Vertex-stage sway from the analytic wind field. WindField::sample in
//      crates/bw_grass/src/wind.rs is the reference implementation, kept in
//      Rust so it can be tested and benchmarked without a GPU.
//   3. Bend away from the disturbance texture so an army leaves a wake.
//   4. LOD: blade count from GrassLod, fading to a flat billboard at distance.

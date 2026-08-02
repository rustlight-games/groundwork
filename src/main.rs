//! Backseat Warlord.
//!
//! A thin entry point. Everything lives in the workspace crates under
//! `crates/`, `plugins/`, and `tools/` — see `docs/ARCHITECTURE.md`.

fn main() {
    bw_app::build_app().run();
}

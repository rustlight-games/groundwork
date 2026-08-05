//! Capture the compiler that built this crate.
//!
//! The report schema names `rustc` as a required field of a machine identity,
//! and it has to be the compiler that produced *this* binary rather than
//! whatever `rustc` happens to be first on the path when the benchmark runs —
//! those differ on any machine with a toolchain override, which is most of them.
//!
//! `RUSTC` is set by Cargo to the exact compiler it is using, so asking that one
//! for its version is the only reading that cannot drift.

fn main() {
    println!("cargo:rerun-if-env-changed=RUSTC");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|text| !text.is_empty());
    if let Some(version) = version {
        println!("cargo:rustc-env=TERRAIN_BENCH_RUSTC={version}");
    }
}

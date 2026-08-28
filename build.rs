//! Embed a build identifier (git short hash + dirty marker) so every
//! aitrace process can state exactly which code it runs. This makes the
//! "which binary is under test" acceptance check mechanical: MCP
//! `initialize`, `--version`, and `daemon status` all expose it.

use std::process::Command;

fn main() {
    let build = build_hash();
    println!("cargo:rustc-env=AITRACE_BUILD_HASH={build}");
}

/// `<short-hash>[-dirty]`, or `unknown` when git is unavailable.
fn build_hash() -> String {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if hash.is_empty() {
        return "unknown".to_string();
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty { format!("{hash}-dirty") } else { hash }
}

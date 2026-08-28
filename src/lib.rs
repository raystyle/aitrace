#![allow(clippy::collapsible_if)]

/// Build identifier (`git short hash` plus `-dirty` when the worktree has
/// uncommitted changes), injected by `build.rs`.
pub fn build_hash() -> &'static str {
    env!("AITRACE_BUILD_HASH")
}

pub mod analysis;
pub mod auto_detect;
pub mod checkpoint;
pub mod claude_log;
pub mod config;
pub mod daemon;
pub mod event;
pub mod export;
pub mod hook;
pub mod import;
pub mod install;
pub mod ipc;
pub mod mcp;
pub mod project;
pub mod recorder;
pub mod restore;
pub mod session;
pub mod snapshot;
pub mod watcher;

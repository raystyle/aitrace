//! Resolve the project directory even when the process was started from
//! `target/debug` (a common way to launch the just-built exe).

use std::path::{Path, PathBuf};

/// Walk up from `path` to the Cargo package root.
///
/// `aitrace` launched as `target\debug\aitrace.exe` with no args used to
/// treat `target\debug` as the project, so the daemon wrote
/// `target\debug\.aitrace\` and `--daemon-child` kept locking the linker
/// output. If `path` sits under `target/{debug,release}`, jump to the
/// directory that contains `Cargo.toml`.
pub fn workspace_root(path: &Path) -> PathBuf {
    // Do not canonicalize: on Windows that injects a `\\?\` prefix and
    // AF_UNIX sockets at `\\?\C:\...\.aitrace\daemon.sock` miss the
    // daemon listening on `C:\...\.aitrace\daemon.sock`.
    let stripped = strip_cargo_target_dir(path);
    if stripped != path && stripped.join("Cargo.toml").is_file() {
        return stripped;
    }
    path.to_path_buf()
}

fn strip_cargo_target_dir(dir: &Path) -> PathBuf {
    let mut parts: Vec<_> = dir.components().collect();
    let last = parts
        .last()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");
    if matches!(last, "deps" | "examples" | "incremental") {
        parts.pop();
    }
    let last = parts
        .last()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");
    if !matches!(last, "debug" | "release") {
        return dir.to_path_buf();
    }
    parts.pop();
    let last = parts
        .last()
        .and_then(|c| c.as_os_str().to_str())
        .unwrap_or("");
    if last != "target" {
        return dir.to_path_buf();
    }
    parts.pop();
    parts.iter().collect()
}

/// Directories that may hold a leaked `.aitrace/daemon.pid` for this package.
pub fn daemon_pid_search_roots(project: &Path) -> Vec<PathBuf> {
    let root = workspace_root(project);
    vec![
        root.clone(),
        root.join("target").join("debug"),
        root.join("target").join("release"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn walks_up_from_target_debug() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let debug = tmp.path().join("target").join("debug");
        fs::create_dir_all(&debug).unwrap();
        let got = workspace_root(&debug);
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn walks_up_from_target_debug_deps() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let deps = tmp.path().join("target").join("debug").join("deps");
        fs::create_dir_all(&deps).unwrap();
        let got = workspace_root(&deps);
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn leaves_plain_project_alone() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let got = workspace_root(tmp.path());
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn pid_search_includes_target_debug() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let roots = daemon_pid_search_roots(tmp.path());
        assert!(
            roots.iter().any(|p| p.ends_with("debug")),
            "roots: {roots:?}"
        );
    }
}

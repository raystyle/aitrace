use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Directory for the project-local aitrace binary: `<project>/.aitrace/bin/`.
pub fn bin_dir(project: &Path) -> PathBuf {
    project.join(".aitrace").join("bin")
}

/// File name of the installed binary. Always `aitrace.exe` so project
/// `.claude/settings.json` and `.mcp.json` can use one path on Windows
/// (exec form requires a real `.exe`) and Unix.
pub fn bin_name() -> &'static str {
    "aitrace.exe"
}

/// `<project>/.aitrace/bin/aitrace.exe`
pub fn bin_path(project: &Path) -> PathBuf {
    bin_dir(project).join(bin_name())
}

/// Copy the running executable into `<project>/.aitrace/bin/aitrace.exe`.
///
/// Logs, the daemon socket, and this binary all stay under the project
/// directory. If the destination file is locked because a process is still
/// running from it (on Windows: a live MCP server or hook-send), rename it
/// out of the way first -- Windows forbids overwriting a running executable
/// but allows renaming it, and the old process keeps its handle. Only if the
/// rename also fails do we keep the existing file.
pub fn install_project_bin(project: &Path) -> Result<PathBuf> {
    let dest = bin_path(project);
    let dir = bin_dir(project);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;

    let src = std::env::current_exe().context("resolve current executable")?;
    if paths_equal(&src, &dest) {
        return Ok(dest);
    }

    if dest.exists() && std::fs::copy(&src, &dest).is_err() {
        // Locked destination: move the running exe aside so the copy can
        // land at the canonical path. A stale `.old` from a previous update
        // is replaced.
        let aside = unique_old_path(&dest);
        let _ = std::fs::remove_file(&aside);
        if std::fs::rename(&dest, &aside).is_err() || std::fs::copy(&src, &dest).is_err() {
            // Rename failed too (e.g. aside is locked as well): restore the
            // original name if we moved it, and keep the old binary.
            if !dest.exists() && aside.exists() {
                let _ = std::fs::rename(&aside, &dest);
            }
            return Ok(dest);
        }
    }
    if !dest.exists() {
        std::fs::copy(&src, &dest).with_context(|| format!("copy {:?} -> {:?}", src, dest))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)
            .with_context(|| format!("stat {:?}", dest))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).with_context(|| format!("chmod {:?}", dest))?;
    }

    Ok(dest)
}

fn unique_old_path(dest: &Path) -> PathBuf {
    let base = dest.with_extension("exe.old");
    if std::fs::remove_file(&base).is_ok() || !base.exists() {
        return base;
    }
    dest.with_extension(format!("exe.old.{}", std::process::id()))
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(aa), Ok(bb)) => aa == bb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_writes_under_project_aitrace_bin() {
        let tmp = tempdir().unwrap();
        let dest = install_project_bin(tmp.path()).unwrap();
        assert!(dest.exists());
        assert_eq!(dest.file_name().unwrap(), "aitrace.exe");
        assert!(dest.starts_with(tmp.path().join(".aitrace").join("bin")));
    }
}

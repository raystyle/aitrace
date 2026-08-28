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
/// directory. If the destination is already running (Windows file lock),
/// keep the existing file.
pub fn install_project_bin(project: &Path) -> Result<PathBuf> {
    let dest = bin_path(project);
    let dir = bin_dir(project);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;

    let src = std::env::current_exe().context("resolve current executable")?;
    if paths_equal(&src, &dest) {
        return Ok(dest);
    }

    if dest.exists() && std::fs::copy(&src, &dest).is_err() {
        return Ok(dest);
    }
    if !dest.exists() {
        std::fs::copy(&src, &dest)
            .with_context(|| format!("copy {:?} -> {:?}", src, dest))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)
            .with_context(|| format!("stat {:?}", dest))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)
            .with_context(|| format!("chmod {:?}", dest))?;
    }

    Ok(dest)
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

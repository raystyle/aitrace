use anyhow::{Context, Result};
use std::path::Path;

/// Write a PID file containing the daemon's PID and session ID.
///
/// Format: two lines -- PID on line 1, session ID on line 2.
pub fn write_pid_file(path: &Path, pid: i32, session_id: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for PID file {:?}", path))?;
    }
    let content = format!("{}\n{}\n", pid, session_id);
    std::fs::write(path, content).with_context(|| format!("write PID file {:?}", path))?;
    Ok(())
}

/// Read a PID file and return `(pid, session_id)`.
pub fn read_pid_file(path: &Path) -> Result<(i32, String)> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read PID file {:?}", path))?;
    let mut lines = content.lines();
    let pid_str = lines.next().context("PID file is empty")?;
    let pid: i32 = pid_str
        .trim()
        .parse()
        .with_context(|| format!("invalid PID value: {:?}", pid_str))?;
    let session_id = lines
        .next()
        .context("PID file missing session ID")?
        .trim()
        .to_string();
    Ok((pid, session_id))
}

/// Check whether a process with the given PID is still alive.
pub fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    is_process_alive_impl(pid)
}

#[cfg(unix)]
fn is_process_alive_impl(pid: i32) -> bool {
    // `kill(pid, 0)` sends no signal; EPERM means the process exists.
    let ret = unsafe { libc::kill(pid, 0) };
    if ret == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn is_process_alive_impl(pid: i32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code == STILL_ACTIVE as u32
    }
}

/// Best-effort stop of a process that did not exit after a graceful socket stop.
pub fn terminate(pid: i32) {
    if pid <= 0 {
        return;
    }
    terminate_impl(pid);
}

#[cfg(unix)]
fn terminate_impl(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

#[cfg(windows)]
fn terminate_impl(pid: i32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
        if handle.is_null() {
            return;
        }
        let _ = TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}

/// Clean up stale daemon artifacts (PID file and socket) left by a crashed daemon.
///
/// If the PID file exists and the recorded process is dead, both the PID file
/// and the socket file are removed. If the process is still alive, this is a
/// no-op (the daemon is running).
pub fn cleanup_stale(pid_path: &Path, sock_path: &Path) -> Result<()> {
    if !pid_path.exists() {
        return Ok(());
    }

    let (pid, _session_id) = read_pid_file(pid_path)?;

    if is_process_alive(pid) {
        // Process is still running -- nothing to clean up.
        return Ok(());
    }

    tracing::warn!(
        "cleaning up stale PID file from crashed daemon (PID {})",
        pid
    );

    if pid_path.exists() {
        std::fs::remove_file(pid_path)
            .with_context(|| format!("remove stale PID file {:?}", pid_path))?;
    }

    let _ = std::fs::remove_file(sock_path);

    Ok(())
}

// ---- unit tests ---------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_and_read_pid_file() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("daemon.pid");

        write_pid_file(&pid_path, 12345, "20260324-120000-abcd").unwrap();
        let (pid, session_id) = read_pid_file(&pid_path).unwrap();

        assert_eq!(pid, 12345);
        assert_eq!(session_id, "20260324-120000-abcd");
    }

    #[test]
    fn read_nonexistent_pid_file_errors() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("nonexistent.pid");
        assert!(read_pid_file(&pid_path).is_err());
    }

    #[test]
    fn is_process_alive_self() {
        // Our own process must be alive.
        let pid = std::process::id() as i32;
        assert!(is_process_alive(pid));
    }

    #[test]
    fn is_process_alive_bogus() {
        // PID 0 is the kernel on most Unix systems and not a real user process,
        // but a very large PID is almost certainly unused.
        // Use a PID that is extremely unlikely to exist.
        assert!(!is_process_alive(4_000_000));
    }

    #[test]
    fn cleanup_stale_removes_files_for_dead_process() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("daemon.pid");
        let sock_path = dir.path().join("daemon.sock");

        // Write a PID file referencing a dead process.
        write_pid_file(&pid_path, 4_000_000, "dead-session").unwrap();
        std::fs::write(&sock_path, "socket placeholder").unwrap();

        assert!(pid_path.exists());
        assert!(sock_path.exists());

        cleanup_stale(&pid_path, &sock_path).unwrap();

        assert!(!pid_path.exists());
        assert!(!sock_path.exists());
    }

    #[test]
    fn cleanup_stale_noop_when_no_pid_file() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("daemon.pid");
        let sock_path = dir.path().join("daemon.sock");

        // Should be a no-op, not an error.
        cleanup_stale(&pid_path, &sock_path).unwrap();
    }

    #[test]
    fn cleanup_stale_noop_when_process_alive() {
        let dir = tempdir().unwrap();
        let pid_path = dir.path().join("daemon.pid");
        let sock_path = dir.path().join("daemon.sock");

        // Write a PID file referencing our own (alive) process.
        let our_pid = std::process::id() as i32;
        write_pid_file(&pid_path, our_pid, "live-session").unwrap();
        std::fs::write(&sock_path, "socket placeholder").unwrap();

        cleanup_stale(&pid_path, &sock_path).unwrap();

        // Files should still exist because the process is alive.
        assert!(pid_path.exists());
        assert!(sock_path.exists());
    }
}

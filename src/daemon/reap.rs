//! Kill leftover `--daemon-child` processes and stale pid files.
//!
//! Windows DETACHED_PROCESS daemons survive the CLI that started them. If
//! that CLI's cwd was `target\debug`, `daemon stop` at the repo root used
//! to miss them and `cargo build` then hit os error 5.

use std::path::Path;

use anyhow::Result;

use crate::project::daemon_pid_search_roots;

use super::pid;
use super::{pid_path, stop_one_daemon};

/// Stop every daemon pid file that belongs to this package (repo root and
/// leaked `target/{debug,release}/.aitrace`), then terminate leftover
/// `--daemon-child` processes whose image lives under the package.
///
/// Does **not** kill `aitrace mcp` (the IDE's MCP server).
pub fn reap(project_path: &Path) -> Result<ReapReport> {
    let roots = daemon_pid_search_roots(project_path);
    let mut report = ReapReport::default();

    for root in &roots {
        let pid_file = pid_path(root);
        if !pid_file.exists() {
            continue;
        }
        match stop_one_daemon(root) {
            Ok(()) => report.stopped_pid_files.push(pid_file),
            Err(e) => report.errors.push(format!("{}: {e}", pid_file.display())),
        }
    }

    report
        .killed
        .extend(kill_stray_daemon_children(project_path)?);
    report.removed_old = remove_unlocked_old_bins(project_path);

    Ok(report)
}

#[derive(Default, Debug)]
pub struct ReapReport {
    pub stopped_pid_files: Vec<std::path::PathBuf>,
    pub killed: Vec<(u32, String)>,
    pub removed_old: Vec<std::path::PathBuf>,
    pub errors: Vec<String>,
}

impl ReapReport {
    pub fn is_empty(&self) -> bool {
        self.stopped_pid_files.is_empty() && self.killed.is_empty() && self.removed_old.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        for p in &self.stopped_pid_files {
            lines.push(format!("stopped pid file {}", p.display()));
        }
        for (pid, cmd) in &self.killed {
            lines.push(format!("killed pid {pid} ({cmd})"));
        }
        for p in &self.removed_old {
            lines.push(format!("removed {}", p.display()));
        }
        for e in &self.errors {
            lines.push(format!("warning: {e}"));
        }
        if lines.is_empty() {
            "no stray daemons".to_string()
        } else {
            lines.join("\n")
        }
    }
}

fn remove_unlocked_old_bins(project_path: &Path) -> Vec<std::path::PathBuf> {
    let dir = crate::install::bin_dir(project_path);
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return removed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.contains(".old") {
            continue;
        }
        let path = entry.path();
        if std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

#[cfg(windows)]
fn kill_stray_daemon_children(project_path: &Path) -> Result<Vec<(u32, String)>> {
    let root = crate::project::workspace_root(project_path);
    let root_s = root.to_string_lossy().to_lowercase().replace('/', "\\");
    let self_pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let mut killed = Vec::new();

    for (pid, image, cmd) in list_aitrace_processes() {
        if pid == self_pid {
            continue;
        }
        let cmd_l = cmd.to_lowercase();
        if !cmd_l.contains("--daemon-child") {
            continue;
        }
        let image_l = image.to_lowercase().replace('/', "\\");
        let under_project = image_l.contains(&root_s) || cmd_l.contains(&root_s);
        if !under_project {
            continue;
        }
        pid::terminate(pid as i32);
        killed.push((pid, cmd));
    }
    Ok(killed)
}

#[cfg(not(windows))]
fn kill_stray_daemon_children(_project_path: &Path) -> Result<Vec<(u32, String)>> {
    Ok(Vec::new())
}

#[cfg(windows)]
fn list_aitrace_processes() -> Vec<(u32, String, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut out = Vec::new();
        if Process32FirstW(snap, &mut pe) != 0 {
            loop {
                let exe = wchar_to_string(&pe.szExeFile);
                if exe.to_lowercase().starts_with("aitrace") {
                    let pid = pe.th32ProcessID;
                    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                    if !handle.is_null() {
                        let mut buf = [0u16; 32768];
                        let mut len = buf.len() as u32;
                        let image = if QueryFullProcessImageNameW(
                            handle,
                            PROCESS_NAME_WIN32,
                            buf.as_mut_ptr(),
                            &mut len,
                        ) != 0
                        {
                            String::from_utf16_lossy(&buf[..len as usize])
                        } else {
                            String::new()
                        };
                        let cmd = process_command_line(handle).unwrap_or_else(|| image.clone());
                        CloseHandle(handle);
                        out.push((pid, image, cmd));
                    }
                }
                if Process32NextW(snap, &mut pe) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        out
    }
}

#[cfg(windows)]
fn wchar_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(windows)]
fn process_command_line(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            process_handle: windows_sys::Win32::Foundation::HANDLE,
            process_information_class: u32,
            process_information: *mut core::ffi::c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    unsafe {
        let mut needed: u32 = 0;
        NtQueryInformationProcess(
            handle,
            PROCESS_COMMAND_LINE_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let st = NtQueryInformationProcess(
            handle,
            PROCESS_COMMAND_LINE_INFORMATION,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        );
        if st < 0 {
            return None;
        }
        if buf.len() < std::mem::size_of::<UnicodeString>() {
            return None;
        }
        let us = buf.as_ptr().cast::<UnicodeString>().read_unaligned();
        if us.buffer.is_null() || us.length == 0 {
            return None;
        }
        let nchars = (us.length as usize) / 2;
        let slice = std::slice::from_raw_parts(us.buffer, nchars);
        Some(String::from_utf16_lossy(slice))
    }
}

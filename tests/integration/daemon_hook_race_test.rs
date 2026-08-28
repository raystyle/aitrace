//! End-to-end regression test for the watcher-event / PostToolUse-hook race.
//!
//! Claude Code fires PostToolUse *after* the file write is on disk, so the
//! hook message usually reaches the daemon after the watcher event for the
//! same edit. The daemon must wait briefly (HOOK_GRACE) for the in-flight
//! hook instead of recording the edit with null enrichment.
//!
//! This test reproduces the ordering with real binaries: it starts the
//! daemon, writes a file, then sends a Claude-shaped hook payload through
//! `aitrace hook-send` 100 ms later, and asserts the recorded edit carries
//! the agent label and operation id.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_aitrace")
}

fn wait_for(pid_file: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pid_file.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("daemon pid file did not appear at {:?}", pid_file);
}

/// Read the single session's edits.jsonl and return the parsed events.
fn recorded_edits(project: &Path) -> Vec<Value> {
    let sessions_dir = project.join(".aitrace").join("sessions");
    let session_dir = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .next()
        .expect("one session")
        .unwrap()
        .path();
    let log = std::fs::read_to_string(session_dir.join("edits.jsonl")).unwrap_or_default();
    log.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn wait_for_edit(project: &Path, timeout: Duration) -> Vec<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let edits = recorded_edits(project);
        if edits.iter().any(|e| {
            e["file"]
                .as_str()
                .unwrap_or("")
                .replace('\\', "/")
                .contains("lib.rs")
        }) {
            return edits;
        }
        if Instant::now() > deadline {
            panic!(
                "edit for src/lib.rs was not recorded in time; log: {:?}",
                edits
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hook_arriving_after_watcher_event_still_enriches_the_edit() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("src")).unwrap();

    // 1. Start the daemon main loop directly (as start_daemon's child does).
    let mut daemon: Child = Command::new(bin())
        .arg("--daemon-child")
        .arg(project.to_str().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon child");
    wait_for(
        &project.join(".aitrace").join("daemon.pid"),
        Duration::from_secs(5),
    );

    // 2. Write a file -- the watcher event is now in flight.
    let src = project.join("src").join("lib.rs");
    std::fs::write(&src, "fn answer() -> u32 { 42 }\n").unwrap();

    // 3. Send the hook 100 ms later, mirroring PostToolUse latency, through
    //    the real hook-send path (session_id extraction and all).
    std::thread::sleep(Duration::from_millis(100));
    let payload = serde_json::json!({
        "session_id": "race-sess",
        "tool_use_id": "call_race1",
        "tool_name": "Edit",
        "tool_input": { "file_path": src.to_string_lossy() },
    });
    let mut hook = Command::new(bin())
        .arg("hook-send")
        .arg("--project")
        .arg(project.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hook-send");
    hook.stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&payload).unwrap().as_bytes())
        .unwrap();
    let status = hook.wait().unwrap();
    assert!(status.success(), "hook-send failed: {:?}", status);

    // 4. The edit must be recorded with the hook's enrichment despite the
    //    hook arriving after the watcher event.
    let edits = wait_for_edit(project, Duration::from_secs(5));
    let edit = edits
        .iter()
        .find(|e| {
            e["file"]
                .as_str()
                .unwrap_or("")
                .replace('\\', "/")
                .contains("lib.rs")
        })
        .expect("edit for src/lib.rs");
    assert_eq!(
        edit["agent_label"].as_str(),
        Some("claude-code-1"),
        "agent label missing; edit: {}",
        edit
    );
    assert_eq!(
        edit["operation_id"].as_str(),
        Some("race-sess:call_race1"),
        "operation id missing; edit: {}",
        edit
    );

    // 5. Shut the daemon down cleanly.
    let _ = Command::new(bin())
        .arg("daemon")
        .arg("stop")
        .current_dir(project)
        .output();
    let _ = daemon.wait();
}

#[test]
fn transcript_intents_are_attached_to_edits() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("src")).unwrap();

    // A minimal transcript mirroring Claude Code's real shape: an assistant
    // text entry, then a tool_use entry whose parent is that text, then a
    // last-prompt marker. The tool-use line is on disk before the "tool" runs.
    let transcript = project.join("transcript.jsonl");
    let mut t = String::new();
    t.push_str(r#"{"uuid":"u1","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"wire the intent index into the daemon"}]}}"#);
    t.push('\n');
    t.push_str(r#"{"uuid":"u2","parentUuid":"u1","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call_intent1","name":"Edit","input":{}}]}}"#);
    t.push('\n');
    t.push_str(
        r#"{"type":"last-prompt","lastPrompt":"implement the intent feature","leafUuid":"u2"}"#,
    );
    t.push('\n');
    std::fs::write(&transcript, t).unwrap();

    let mut daemon: Child = Command::new(bin())
        .arg("--daemon-child")
        .arg(project.to_str().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon child");
    wait_for(
        &project.join(".aitrace").join("daemon.pid"),
        Duration::from_secs(5),
    );

    let src = project.join("src").join("lib.rs");
    std::fs::write(&src, "pub fn ping() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let payload = serde_json::json!({
        "session_id": "intent-sess",
        "tool_use_id": "call_intent1",
        "tool_name": "Edit",
        "tool_input": { "file_path": src.to_string_lossy() },
        "transcript_path": transcript.to_string_lossy(),
    });
    let mut hook = Command::new(bin())
        .arg("hook-send")
        .arg("--project")
        .arg(project.to_str().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hook-send");
    hook.stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&payload).unwrap().as_bytes())
        .unwrap();
    let status = hook.wait().unwrap();
    assert!(status.success(), "hook-send failed: {:?}", status);

    let edits = wait_for_edit(project, Duration::from_secs(5));
    let edit = edits
        .iter()
        .find(|e| {
            e["file"]
                .as_str()
                .unwrap_or("")
                .replace('\\', "/")
                .contains("lib.rs")
        })
        .expect("edit for src/lib.rs");
    assert_eq!(
        edit["operation_intent"].as_str(),
        Some("wire the intent index into the daemon"),
        "operation intent should come from the transcript text; edit: {}",
        edit
    );
    assert_eq!(
        edit["intent"].as_str(),
        Some("implement the intent feature"),
        "user intent should come from last-prompt; edit: {}",
        edit
    );

    let _ = Command::new(bin())
        .arg("daemon")
        .arg("stop")
        .current_dir(project)
        .output();
    let _ = daemon.wait();
}

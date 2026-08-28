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

/// Kills the daemon child even when the test panics, so a failing
/// assertion cannot leave a process locking `target\debug\aitrace.exe`
/// (a lesson from mutation-testing this suite).
struct DaemonGuard(Child);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_daemon(project: &Path) -> DaemonGuard {
    let child: Child = Command::new(bin())
        .arg("--daemon-child")
        .arg(project.to_str().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon child");
    DaemonGuard(child)
}

fn send_hook(project: &Path, payload: &serde_json::Value) {
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
        .write_all(serde_json::to_string(payload).unwrap().as_bytes())
        .unwrap();
    let status = hook.wait().unwrap();
    assert!(status.success(), "hook-send failed: {:?}", status);
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

/// Read the single session's edits.jsonl and return the parsed events,
/// deduplicated by id keeping the last record (backfill corrections replace
/// the original line; read_all consumers behave the same way).
fn recorded_edits(project: &Path) -> Vec<Value> {
    let sessions_dir = project.join(".aitrace").join("sessions");
    let session_dir = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .next()
        .expect("one session")
        .unwrap()
        .path();
    let log = std::fs::read_to_string(session_dir.join("edits.jsonl")).unwrap_or_default();
    let mut events: Vec<Value> = Vec::new();
    let mut positions: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for line in log.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(ev) = serde_json::from_str::<Value>(line) {
            let id = ev["id"].as_u64().unwrap_or(0);
            match positions.get(&id) {
                Some(&pos) => events[pos] = ev,
                None => {
                    positions.insert(id, events.len());
                    events.push(ev);
                }
            }
        }
    }
    events
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

/// Wait until an edit for `file_part` satisfies `check`, returning the
/// matching (deduplicated) edit list.
fn wait_for_edit_where(
    project: &Path,
    file_part: &str,
    timeout: Duration,
    check: impl Fn(&Value) -> bool,
) -> Vec<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let edits = recorded_edits(project);
        if let Some(e) = find_edit_opt(&edits, file_part)
            && check(e)
        {
            return edits;
        }
        if Instant::now() > deadline {
            panic!("edit for {file_part} did not satisfy the condition in time; log: {edits:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn find_edit_opt<'a>(edits: &'a [Value], file_part: &str) -> Option<&'a Value> {
    edits.iter().find(|e| {
        e["file"]
            .as_str()
            .unwrap_or("")
            .replace('\\', "/")
            .contains(file_part)
    })
}

fn find_edit<'a>(edits: &'a [Value], file_part: &str) -> &'a Value {
    find_edit_opt(edits, file_part)
        .unwrap_or_else(|| panic!("edit for {file_part} not found in {edits:?}"))
}

fn stop_daemon(project: &Path) {
    let _ = Command::new(bin())
        .arg("daemon")
        .arg("stop")
        .current_dir(project)
        .output();
}

#[test]
fn hook_arriving_after_watcher_event_still_enriches_the_edit() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("src")).unwrap();

    let _daemon = spawn_daemon(project);
    wait_for(
        &project.join(".aitrace").join("daemon.pid"),
        Duration::from_secs(5),
    );

    // Write a file -- the watcher event is now in flight -- then send the
    // hook 100 ms later, mirroring PostToolUse latency, through the real
    // hook-send path.
    let src = project.join("src").join("lib.rs");
    std::fs::write(&src, "fn answer() -> u32 { 42 }\n").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    send_hook(
        project,
        &serde_json::json!({
            "session_id": "race-sess",
            "tool_use_id": "call_race1",
            "tool_name": "Edit",
            "tool_input": { "file_path": src.to_string_lossy() },
        }),
    );

    // The edit must be recorded with the hook's enrichment despite the hook
    // arriving after the watcher event.
    let edits = wait_for_edit(project, Duration::from_secs(5));
    let edit = find_edit(&edits, "lib.rs");
    assert_eq!(
        edit["agent_label"].as_str(),
        Some("claude-code-1"),
        "edit: {edit}"
    );
    assert_eq!(
        edit["operation_id"].as_str(),
        Some("race-sess:call_race1"),
        "edit: {edit}"
    );

    stop_daemon(project);
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

    let _daemon = spawn_daemon(project);
    wait_for(
        &project.join(".aitrace").join("daemon.pid"),
        Duration::from_secs(5),
    );

    let src = project.join("src").join("lib.rs");
    std::fs::write(&src, "pub fn ping() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    send_hook(
        project,
        &serde_json::json!({
            "session_id": "intent-sess",
            "tool_use_id": "call_intent1",
            "tool_name": "Edit",
            "tool_input": { "file_path": src.to_string_lossy() },
            "transcript_path": transcript.to_string_lossy(),
        }),
    );

    let edits = wait_for_edit(project, Duration::from_secs(5));
    let edit = find_edit(&edits, "lib.rs");
    assert_eq!(
        edit["operation_intent"].as_str(),
        Some("wire the intent index into the daemon"),
        "operation intent should come from the transcript text; edit: {edit}"
    );
    assert_eq!(
        edit["intent"].as_str(),
        Some("implement the intent feature"),
        "user intent should come from last-prompt; edit: {edit}"
    );

    stop_daemon(project);
}

/// The real-world regression from the five-principles walkthrough: Claude
/// Code appends the assistant parent text AFTER the tool_use transcript
/// line. The edit is first recorded with a null intent and must be
/// backfilled once the parent lands and a later hook triggers a refresh.
#[test]
fn late_transcript_parent_backfills_the_recorded_intent() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("src")).unwrap();

    // Transcript initially contains ONLY the tool_use line; its parent uuid
    // dangles until we append the text entry below.
    let transcript = project.join("transcript.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            r#"{"uuid":"t2","parentUuid":"t1","type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"call_late1","name":"Edit","input":{}}]}}"#,
            "\n"
        ),
    )
    .unwrap();

    let _daemon = spawn_daemon(project);
    wait_for(
        &project.join(".aitrace").join("daemon.pid"),
        Duration::from_secs(5),
    );

    // First edit: parent text absent, intent cannot resolve yet.
    let src = project.join("src").join("lib.rs");
    std::fs::write(&src, "pub fn one() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    send_hook(
        project,
        &serde_json::json!({
            "session_id": "late-sess",
            "tool_use_id": "call_late1",
            "tool_name": "Edit",
            "tool_input": { "file_path": src.to_string_lossy() },
            "transcript_path": transcript.to_string_lossy(),
        }),
    );
    let edits = wait_for_edit(project, Duration::from_secs(5));
    let edit = find_edit(&edits, "lib.rs");
    assert_eq!(edit["operation_intent"], Value::Null, "edit: {edit}");

    // The parent text lands seconds later, then any later hook (here: a
    // second edit) triggers a transcript refresh + backfill pass.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        f.write_all(
            concat!(
                r#"{"uuid":"t1","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the late arriving plan"}]}}"#,
                "\n"
            )
            .as_bytes(),
        )
        .unwrap();
    }
    let src2 = project.join("src").join("mod.rs");
    std::fs::write(&src2, "pub fn two() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    send_hook(
        project,
        &serde_json::json!({
            "session_id": "late-sess",
            "tool_use_id": "call_late2",
            "tool_name": "Edit",
            "tool_input": { "file_path": src2.to_string_lossy() },
            "transcript_path": transcript.to_string_lossy(),
        }),
    );

    // The first edit's intent must be corrected in place (dedup by id).
    let edits = wait_for_edit_where(project, "lib.rs", Duration::from_secs(5), |e| {
        e["operation_intent"].is_string()
    });
    let edit = find_edit(&edits, "lib.rs");
    assert_eq!(
        edit["operation_intent"].as_str(),
        Some("the late arriving plan"),
        "backfilled intent; edit: {edit}"
    );
    let lib_entries = edits
        .iter()
        .filter(|e| {
            e["file"]
                .as_str()
                .unwrap_or("")
                .replace('\\', "/")
                .contains("lib.rs")
        })
        .count();
    assert_eq!(lib_entries, 1, "correction must not duplicate entries");

    stop_daemon(project);
}

/// `subscribe_edits`: a socket client that subscribes to the daemon's
/// session receives a JSON notification for each recorded edit.
#[test]
fn subscribe_edits_streams_recorded_edits() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join("src")).unwrap();

    let _daemon = spawn_daemon(project);
    let pid_file = project.join(".aitrace").join("daemon.pid");
    wait_for(&pid_file, Duration::from_secs(5));
    let (pid, session_id) = {
        let raw = std::fs::read_to_string(&pid_file).unwrap();
        let mut lines = raw.lines();
        let pid: i32 = lines.next().unwrap().trim().parse().unwrap();
        let session_id = lines.next().unwrap().trim().to_string();
        (pid, session_id)
    };
    assert!(pid > 0, "pid file payload: {session_id}");

    // Subscribe over the project socket, then trigger a recorded edit.
    use std::io::BufRead;
    let sock = project.join(".aitrace").join("daemon.sock");
    let mut stream = aitrace::ipc::connect(&sock).expect("connect to daemon socket");
    writeln!(
        stream,
        r#"{{"type":"subscribe","session_id":"{session_id}"}}"#
    )
    .unwrap();
    stream.flush().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout on subscription stream");

    let src = project.join("src").join("sub.rs");
    std::fs::write(&src, "pub fn watched() {}\n").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    send_hook(
        project,
        &serde_json::json!({
            "session_id": "sub-sess",
            "tool_name": "Write",
            "tool_input": { "file_path": src.to_string_lossy() },
        }),
    );

    // One notification line must arrive for the recorded edit.
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("notification line");
    let notification: Value = serde_json::from_str(line.trim()).expect("notification JSON");
    assert_eq!(
        notification["type"], "edit_notification",
        "got: {notification}"
    );
    assert!(
        notification["event"]["file"]
            .as_str()
            .unwrap_or("")
            .replace('\\', "/")
            .contains("sub.rs"),
        "notification event file; got: {notification}"
    );

    stop_daemon(project);
}

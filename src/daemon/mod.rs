pub mod agent_registry;
pub mod correlation;
pub mod hook_listener;
pub mod intent_index;
pub mod pid;

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::config::Config;
use crate::ipc;
use crate::recorder::{Enrichment, Recorder};
use crate::session::SessionManager;
use crate::watcher::fs_watcher::FsWatcher;

use agent_registry::AgentRegistry;
use correlation::Correlator;
use hook_listener::SocketMessage;

/// Standard filesystem locations for daemon artifacts, relative to
/// `<project>/.aitrace/`.
fn pid_path(project_path: &std::path::Path) -> PathBuf {
    project_path.join(".aitrace").join("daemon.pid")
}

/// Point tracing at `<project>/.aitrace/daemon.log` (append, DEBUG level).
/// Rotates the previous log away once it passes 1 MB (one generation kept).
///
/// Best effort: a logging failure must never stop the recorder.
fn init_logging(vt_dir: &std::path::Path) -> Result<()> {
    let log_path = vt_dir.join("daemon.log");
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 1024 * 1024 {
            let _ = std::fs::rename(&log_path, vt_dir.join("daemon.log.1"));
        }
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {:?}", log_path))?;
    tracing_subscriber::fmt()
        .with_writer(std::sync::Arc::new(file))
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .try_init()
        .map_err(|e| anyhow::anyhow!("init tracing: {e}"))?;
    tracing::info!("daemon logging to {}", log_path.display());
    Ok(())
}

/// First `n` **characters** of `s` for log lines.
///
/// Byte slicing panics when the cut lands inside a multi-byte character --
/// which for this project's Chinese intents is the common case, and a panic
/// inside a log macro kills the daemon.
fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// One pending intent backfill: the recorded event plus the session log it
/// belongs to. Corrections must be appended to their **origin** session's
/// edits.jsonl -- event ids are per-session, so writing them into a newer
/// session's log would collide with that session's own ids.
#[derive(serde::Serialize, serde::Deserialize)]
struct BackfillItem {
    session_dir: PathBuf,
    event: crate::event::EditEvent,
}

/// Persist the pending intent-backfill queue in the current session
/// directory so a daemon restart keeps waiting-for-late-parents work alive.
fn save_backfill(session_dir: &std::path::Path, queue: &[BackfillItem]) {
    let path = session_dir.join("backfill.json");
    if queue.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Ok(json) = serde_json::to_string(queue) {
        let _ = std::fs::write(&path, json);
    }
}

/// Load the queue left behind by the most recent prior session (sessions
/// without one are skipped), then take ownership of the file.
fn load_backfill(sessions_dir: &std::path::Path, current_id: &str) -> Vec<BackfillItem> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(sessions_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir() && p.file_name().and_then(|n| n.to_str()) != Some(current_id)
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs.reverse();
    for dir in dirs {
        let path = dir.join("backfill.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(items) = serde_json::from_str::<Vec<BackfillItem>>(&text) {
                let _ = std::fs::remove_file(&path);
                return items;
            }
        }
    }
    Vec::new()
}

fn sock_path(project_path: &std::path::Path) -> PathBuf {
    project_path.join(".aitrace").join("daemon.sock")
}

/// Run the daemon process.
///
/// This is the entry point called by `--daemon-child`. It:
/// 1. Creates a new session.
/// 2. Writes the PID file.
/// 3. Starts the file watcher and socket listener.
/// 4. Enters the main loop processing file changes and socket messages.
/// 5. Cleans up on shutdown.
pub fn run_daemon(project_path: PathBuf, config: Config) -> Result<()> {
    let vt_dir = project_path.join(".aitrace");
    std::fs::create_dir_all(&vt_dir)?;
    if let Err(e) = crate::install::install_project_bin(&project_path) {
        tracing::warn!("project bin install: {e}");
    }

    // Per-session debug log next to edits.jsonl: the daemon child has no
    // console, so without this it runs silent and debugging tests means
    // dissecting raw JSON dumps.
    let _ = init_logging(&vt_dir);

    // 1. Create a new session.
    let sessions_dir = vt_dir.join("sessions");
    let session_mgr = SessionManager::new(sessions_dir.clone());
    let session = session_mgr.create()?;

    let pid_file = pid_path(&project_path);
    let sock_file = sock_path(&project_path);
    let my_pid = std::process::id() as i32;

    // 2. Write PID file so the parent process (and future CLI commands) can
    //    discover this daemon.
    pid::write_pid_file(&pid_file, my_pid, &session.id)?;
    tracing::info!("daemon session {} starting (pid {my_pid})", session.id);

    // 3. Create the recorder, inheriting the baseline from the most recent
    //    session that actually recorded edits (immediately preceding
    //    sessions may have crashed or recorded nothing). Known files then
    //    record as `modify` (not `create`) after a daemon restart and their
    //    prior content stays diffable.
    let prev_session_dir = session_mgr.latest_prior_session_with_edits(&session.id);
    let recorder = Recorder::with_baseline(
        project_path.clone(),
        session.dir.clone(),
        prev_session_dir.as_deref(),
    );
    tracing::info!(
        "baseline: {} known files inherited from {:?}",
        recorder.current_file_hashes().len(),
        prev_session_dir
    );

    // 4. Start file watcher.
    let (fs_tx, fs_rx) = mpsc::channel::<PathBuf>();
    let mut watcher = FsWatcher::with_ignore(
        project_path.clone(),
        fs_tx,
        config.watch.debounce_ms,
        config.watch.ignore.clone(),
    )?;
    watcher.start()?;

    // 5. Start socket listener thread.
    let (sock_tx, sock_rx) = mpsc::channel::<SocketMessage>();
    let sock_file_clone = sock_file.clone();
    let _listener_thread = std::thread::spawn(move || {
        if let Err(e) = hook_listener::listen(&sock_file_clone, sock_tx) {
            tracing::error!("socket listener error: {}", e);
        }
    });

    // 5b. If this project already has `.claude/`, keep a local PostToolUse
    // hook in settings.local.json unless committed settings.json already
    // defines the aitrace handler.
    let claude_dir = project_path.join(".claude");
    if claude_dir.is_dir() {
        if let Err(e) = crate::hook::registration::register_hook(&claude_dir, &project_path) {
            tracing::warn!("claude hook registration: {e}");
        }
    }

    // 6. Create correlator, agent registry, and transcript intent index.
    let mut correlator = Correlator::new();
    let mut agent_registry = AgentRegistry::new();
    let mut intent_index = intent_index::IntentIndex::new();
    // Edits recorded with a missing operation intent, awaiting a transcript
    // parent that lands late (Claude Code writes assistant entries lazily).
    // Persisted across daemon restarts; corrections go back to the origin
    // session's log.
    let mut intent_backfill: Vec<BackfillItem> = load_backfill(&sessions_dir, &session.id);
    if !intent_backfill.is_empty() {
        tracing::info!(
            "restored {} pending intent backfills from previous runs",
            intent_backfill.len()
        );
    }

    // Channel for sending EditEvents (Recorder requires one, but the daemon
    // doesn't consume them through the channel -- it uses them inline).
    let (event_tx, _event_rx) = mpsc::channel();

    // We need mutable access to recorder in the loop.
    let mut recorder = recorder;
    let mut edit_count: u64 = 0;
    let mut subscribers: Vec<ipc::UnixStream> = Vec::new();

    // 7. Main loop.
    loop {
        let mut should_stop = false;

        // 7a. Drain socket messages.
        {
            let mut state = LoopState {
                correlator: &mut correlator,
                agent_registry: &mut agent_registry,
                subscribers: &mut subscribers,
                intent_index: &mut intent_index,
            };
            while let Ok(msg) = sock_rx.try_recv() {
                if handle_socket_message(
                    msg,
                    &mut state,
                    &session,
                    my_pid,
                    edit_count,
                    &project_path,
                ) {
                    should_stop = true;
                }
            }
        }

        if should_stop {
            break;
        }

        // 7a'. Intent backfill pass: transcript refreshes during the socket
        // drain may have resolved intents for edits that were recorded
        // before their parent text landed. Corrected copies are appended to
        // their ORIGIN session's log (ids are per-session); read_all
        // deduplicates by id keeping the last record.
        if !intent_backfill.is_empty() {
            let mut still_pending = Vec::new();
            for mut item in intent_backfill.drain(..) {
                let mut resolved = false;
                if let Some(op) = item.event.operation_id.as_deref() {
                    if let Some(tool_use_id) = intent_index::tool_use_id_from_operation(op) {
                        if let Some(intent) = intent_index.operation_intent(tool_use_id) {
                            item.event.operation_intent = Some(intent);
                            if item.event.intent.is_none() {
                                item.event.intent = intent_index.user_prompt();
                            }
                            let origin_log = crate::snapshot::edit_log::EditLog::new(
                                item.session_dir.join("edits.jsonl"),
                            );
                            resolved = origin_log.append(&item.event).is_ok();
                            if resolved {
                                tracing::info!(
                                    "backfill: edit #{} {} intent={:?}",
                                    item.event.id,
                                    item.event.file,
                                    item.event.operation_intent.as_deref().map(|s| head(s, 60))
                                );
                            }
                        }
                    }
                }
                if !resolved {
                    still_pending.push(item);
                }
            }
            intent_backfill = still_pending;
            // Bound the queue: entries whose parents never land age out.
            if intent_backfill.len() > 32 {
                let drop_n = intent_backfill.len() - 32;
                tracing::debug!("backfill queue over cap, aging out {drop_n} entries");
                intent_backfill.drain(0..drop_n);
            }
            save_backfill(&session.dir, &intent_backfill);
        }

        // 7b. Drain file changes.
        while let Ok(abs_path) = fs_rx.try_recv() {
            // Directory events (a created folder, mostly) carry no content
            // and never match a hook -- skip instead of burning a grace
            // window on them. Non-existent paths fall through: they are how
            // deletions reach the recorder.
            if abs_path.exists() && abs_path.is_dir() {
                continue;
            }

            // Compute relative path for correlation lookup.
            let rel_path = abs_path
                .strip_prefix(&project_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| abs_path.to_string_lossy().to_string());

            // Build enrichment: restore takes precedence over hook.
            let rel_key = correlation::correlation_key(&rel_path, &project_path);
            let enrichment = if let Some(restore_id) = correlator.pop_restore(&rel_path) {
                // Restore wins -- discard any hook enrichment for this file.
                let _ = correlator.pop_enrichment(&rel_key);
                Some(Enrichment {
                    restore_id: Some(restore_id),
                    ..Default::default()
                })
            } else {
                let mut hook = correlator.pop_enrichment(&rel_key);
                if hook.is_none() {
                    // PostToolUse fires after the file write is already on
                    // disk, so the hook message is frequently still in flight
                    // when the watcher event arrives. Wait briefly for it
                    // instead of recording the edit without enrichment.
                    tracing::debug!("watcher event for {rel_path}: no hook yet, entering grace");
                    let deadline = std::time::Instant::now() + HOOK_GRACE;
                    while std::time::Instant::now() < deadline {
                        {
                            let mut state = LoopState {
                                correlator: &mut correlator,
                                agent_registry: &mut agent_registry,
                                subscribers: &mut subscribers,
                                intent_index: &mut intent_index,
                            };
                            while let Ok(msg) = sock_rx.try_recv() {
                                if handle_socket_message(
                                    msg,
                                    &mut state,
                                    &session,
                                    my_pid,
                                    edit_count,
                                    &project_path,
                                ) {
                                    should_stop = true;
                                }
                            }
                        }
                        if let Some(h) = correlator.pop_enrichment(&rel_key) {
                            hook = Some(h);
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    if hook.is_some() {
                        tracing::debug!("watcher event for {rel_path}: hook arrived in grace");
                    } else {
                        tracing::debug!("watcher event for {rel_path}: grace expired hookless");
                    }
                }
                if should_stop {
                    // Daemon is shutting down; drop this event.
                    break;
                }
                hook.map(|mut hook| {
                    // Final refresh: the transcript parent may have landed
                    // during the grace window. Lookups walk live, so this
                    // resolves intents the hook-time refresh could not.
                    if let Some(tp) = hook.transcript_path.as_deref() {
                        intent_index.refresh(std::path::Path::new(tp));
                        if hook.intent.is_none() {
                            if let Some(tool_use_id) =
                                intent_index::tool_use_id_from_operation(&hook.operation_id)
                            {
                                hook.intent = intent_index.operation_intent(tool_use_id);
                            }
                        }
                        if hook.user_intent.is_none() {
                            hook.user_intent = intent_index.user_prompt();
                        }
                    }
                    let label = agent_registry
                        .get(&hook.agent_id)
                        .map(|info| info.agent_label.clone());
                    Enrichment {
                        agent_id: Some(hook.agent_id.clone()),
                        agent_label: label,
                        operation_id: Some(hook.operation_id),
                        operation_intent: hook.intent,
                        intent: hook.user_intent,
                        tool_name: Some(hook.tool_name),
                        restore_id: None,
                    }
                })
            };

            match recorder.process_file_change(&abs_path, &event_tx, enrichment.as_ref()) {
                Ok(Some(result)) => {
                    edit_count += 1;
                    tracing::info!(
                        "edit #{} {:?} {} +{}/-{} op={:?} intent={:?}{}",
                        result.event.id,
                        result.event.kind,
                        result.event.file,
                        result.event.lines_added,
                        result.event.lines_removed,
                        result.event.operation_id,
                        result
                            .event
                            .operation_intent
                            .as_deref()
                            .map(|s| head(s, 60)),
                        if result.event.operation_id.is_some()
                            && result.event.operation_intent.is_none()
                        {
                            " (queued for backfill)"
                        } else {
                            ""
                        }
                    );
                    // Increment agent edit count if enrichment came from a hook.
                    if let Some(ref enrich) = enrichment {
                        if let Some(ref agent_id) = enrich.agent_id {
                            let ts = Utc::now().timestamp_millis();
                            agent_registry.increment_edit_count(agent_id, ts);
                        }
                    }
                    // Still-missing operation intent: the transcript parent
                    // may land late; queue for the backfill pass.
                    if result.event.operation_id.is_some()
                        && result.event.operation_intent.is_none()
                    {
                        intent_backfill.push(BackfillItem {
                            session_dir: session.dir.clone(),
                            event: result.event.clone(),
                        });
                        save_backfill(&session.dir, &intent_backfill);
                    }

                    // Broadcast to subscribers.
                    if !subscribers.is_empty() {
                        let notification = serde_json::json!({
                            "type": "edit_notification",
                            "event": result.event,
                        });
                        let msg = format!(
                            "{}\n",
                            serde_json::to_string(&notification).unwrap_or_default()
                        );
                        subscribers.retain(|s| {
                            // Try to write; remove subscriber if write fails
                            (&*s).write_all(msg.as_bytes()).is_ok()
                        });
                    }
                }
                Ok(None) => {
                    // No actual change detected -- skip.
                }
                Err(e) => {
                    tracing::warn!("error processing file change {:?}: {}", abs_path, e);
                }
            }
        }

        // 7c. Cleanup stale enrichments (5 second threshold).
        correlator.cleanup_stale(5_000);

        // 7d. Sleep 50ms before next iteration.
        std::thread::sleep(Duration::from_millis(50));
    }

    // Shutdown: update session metadata with final agent list.
    let meta_path = session.dir.join("meta.json");
    if let Ok(content) = std::fs::read_to_string(&meta_path) {
        if let Ok(mut meta) = serde_json::from_str::<crate::session::SessionMeta>(&content) {
            meta.agents = agent_registry.to_vec();
            if let Ok(json) = serde_json::to_string_pretty(&meta) {
                let _ = std::fs::write(&meta_path, json);
            }
        }
    }

    // Cleanup PID file and socket.
    let _ = std::fs::remove_file(&pid_file);
    let _ = std::fs::remove_file(&sock_file);

    Ok(())
}

/// How long a file-change event waits for an in-flight PostToolUse hook
/// before recording the edit without enrichment. The hook has to spawn a
/// process, connect, and write, so it typically lands tens of milliseconds
/// after the watcher event for the same edit.
const HOOK_GRACE: Duration = Duration::from_millis(250);

/// Mutable state shared by the socket-message handlers in the main loop.
struct LoopState<'a> {
    correlator: &'a mut Correlator,
    agent_registry: &'a mut AgentRegistry,
    subscribers: &'a mut Vec<ipc::UnixStream>,
    intent_index: &'a mut intent_index::IntentIndex,
}

/// Handle one socket message.
///
/// Shared by the main socket drain and the hook-grace wait inside the
/// file-change drain, so a hook (or any other message) arriving mid-wait is
/// processed exactly as in the main loop. Returns true when the daemon
/// should stop.
fn handle_socket_message(
    msg: SocketMessage,
    state: &mut LoopState<'_>,
    session: &crate::session::Session,
    my_pid: i32,
    edit_count: u64,
    project_path: &std::path::Path,
) -> bool {
    match msg {
        SocketMessage::Hook(mut payload, file) => {
            // Register or update the agent.
            let ts = Utc::now().timestamp_millis();
            state
                .agent_registry
                .register_or_update(&payload.agent_id, "claude-code", ts);
            if payload.is_error {
                // Failed tool call: no file change will follow, so count it
                // instead of queueing an enrichment that would never match.
                state
                    .agent_registry
                    .increment_failed_attempts(&payload.agent_id, ts);
                tracing::debug!(
                    "hook failed tool call: agent {} op {} tool {} ({} total failures)",
                    payload.agent_id,
                    payload.operation_id,
                    payload.tool_name,
                    state
                        .agent_registry
                        .get(&payload.agent_id)
                        .map(|i| i.failed_attempts)
                        .unwrap_or(0)
                );
                return false;
            }
            // Resolve intents from the transcript: the tool-use entry is
            // written before the tool runs, so the index sees it by now.
            if let Some(tp) = payload.transcript_path.as_deref() {
                state.intent_index.refresh(std::path::Path::new(tp));
                if payload.intent.is_none() {
                    if let Some(tool_use_id) =
                        intent_index::tool_use_id_from_operation(&payload.operation_id)
                    {
                        payload.intent = state.intent_index.operation_intent(tool_use_id);
                    }
                }
                if payload.user_intent.is_none() {
                    payload.user_intent = state.intent_index.user_prompt();
                }
            }
            tracing::debug!(
                "hook: agent {} op {} file {} intent={:?} user={:?}",
                payload.agent_id,
                payload.operation_id,
                file,
                payload.intent.as_deref().map(|s| head(s, 60)),
                payload.user_intent.as_deref().map(|s| head(s, 40))
            );
            // Push enrichment for correlation. Hooks report absolute
            // paths while watcher events are project-relative, so both
            // sides are normalized to the same canonical key.
            let key = correlation::correlation_key(&file, project_path);
            state.correlator.push_enrichment(&key, payload);
        }

        SocketMessage::RestoreStart { restore_id, files } => {
            state.correlator.register_restore(restore_id, &files);
        }

        SocketMessage::RestoreEnd { restore_id } => {
            state.correlator.clear_restore(restore_id);
        }

        SocketMessage::StatusQuery(mut stream) => {
            let agents = state.agent_registry.to_vec();
            let uptime_secs = {
                // Calculate from session start time.
                let meta_path = session.dir.join("meta.json");
                if let Ok(content) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_json::from_str::<crate::session::SessionMeta>(&content)
                    {
                        (Utc::now().timestamp_millis() - meta.started_at).max(0) / 1000
                    } else {
                        0
                    }
                } else {
                    0
                }
            };

            let status = serde_json::json!({
                "type": "status",
                "pid": my_pid,
                "session_id": session.id,
                "version": env!("CARGO_PKG_VERSION"),
                "build_hash": crate::build_hash(),
                "uptime_secs": uptime_secs,
                "edit_count": edit_count,
                "agents": agents,
            });

            let _ = writeln!(stream, "{}", status);
        }

        SocketMessage::Subscribe { session_id, stream } => {
            if session_id == session.id {
                if let Some(s) = stream {
                    state.subscribers.push(s);
                }
            }
        }

        SocketMessage::Stop => {
            tracing::info!("stop requested");
            return true;
        }
    }
    false
}

/// Start the daemon as a detached child process.
///
/// Returns `Ok(pid, session_id)` on success after confirming the daemon wrote
/// its PID file.
pub fn start_daemon(project_path: &std::path::Path) -> Result<(i32, String)> {
    let pid_file = pid_path(project_path);
    let sock_file = sock_path(project_path);

    // Check for an already-running daemon.
    if pid_file.exists() {
        let (existing_pid, existing_session) = pid::read_pid_file(&pid_file)?;
        if pid::is_process_alive(existing_pid) {
            anyhow::bail!(
                "daemon already running (PID {}, session {})",
                existing_pid,
                existing_session
            );
        }
        // Stale PID file from a crashed daemon.
        pid::cleanup_stale(&pid_file, &sock_file)?;
    }

    // Spawn the daemon as a child process.
    let exe = std::env::current_exe().context("resolve current executable path")?;
    let project_str = project_path
        .to_str()
        .context("project path is not valid UTF-8")?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--daemon-child")
        .arg(project_str)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Prefer breaking out of the parent Job Object so the daemon survives
        // when the spawning CLI (CI, agent wrappers) is torn down. If the job
        // forbids breakaway, retry without that flag.
        cmd.creation_flags(
            CREATE_NEW_PROCESS_GROUP
                | DETACHED_PROCESS
                | CREATE_NO_WINDOW
                | CREATE_BREAKAWAY_FROM_JOB,
        );
    }
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
                const DETACHED_PROCESS: u32 = 0x0000_0008;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW);
            }
            cmd.spawn().context("spawn daemon child process")?
        }
    };

    // We don't wait on the child -- it runs independently.
    drop(child);

    // Poll for the PID file to appear (up to 3 seconds, 50ms intervals).
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if pid_file.exists() {
            match pid::read_pid_file(&pid_file) {
                Ok((pid, session_id)) => return Ok((pid, session_id)),
                Err(_) => {
                    // File exists but not fully written yet.
                }
            }
        }

        if std::time::Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for daemon to start (3 seconds)");
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop a running daemon by sending a stop command over the local socket.
///
/// Falls back to SIGTERM (Unix) or TerminateProcess (Windows) after 5 seconds.
pub fn stop_daemon(project_path: &std::path::Path) -> Result<()> {
    let pid_file = pid_path(project_path);
    let sock_file = sock_path(project_path);

    if !pid_file.exists() {
        anyhow::bail!("no daemon running (PID file not found)");
    }

    let (daemon_pid, _session_id) = pid::read_pid_file(&pid_file)?;

    if !pid::is_process_alive(daemon_pid) {
        // Daemon already dead -- clean up.
        pid::cleanup_stale(&pid_file, &sock_file)?;
        println!("daemon was not running (cleaned up stale PID file)");
        return Ok(());
    }

    if let Ok(mut stream) = ipc::connect(&sock_file) {
        let _ = writeln!(stream, r#"{{"type":"control","command":"stop"}}"#);
    }

    // Poll for the PID file to disappear (up to 5 seconds).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if !pid_file.exists() || !pid::is_process_alive(daemon_pid) {
            // Clean up any leftover files.
            let _ = std::fs::remove_file(&pid_file);
            let _ = std::fs::remove_file(&sock_file);
            return Ok(());
        }

        if std::time::Instant::now() >= deadline {
            break;
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    tracing::warn!(
        "daemon (PID {}) did not stop gracefully, terminating",
        daemon_pid
    );
    pid::terminate(daemon_pid);

    // Wait a bit more for the process to die.
    std::thread::sleep(Duration::from_millis(500));
    let _ = std::fs::remove_file(&pid_file);
    let _ = std::fs::remove_file(&sock_file);

    Ok(())
}

/// Query the status of a running daemon.
pub fn daemon_status(project_path: &std::path::Path) -> Result<String> {
    let pid_file = pid_path(project_path);
    let sock_file = sock_path(project_path);

    if !pid_file.exists() {
        anyhow::bail!("no daemon running (PID file not found)");
    }

    let (daemon_pid, session_id) = pid::read_pid_file(&pid_file)?;

    if !pid::is_process_alive(daemon_pid) {
        pid::cleanup_stale(&pid_file, &sock_file)?;
        anyhow::bail!("daemon was not running (cleaned up stale PID file)");
    }

    if let Ok(stream) = ipc::connect(&sock_file) {
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
        let mut stream = stream;
        let _ = writeln!(stream, r#"{{"type":"control","command":"status"}}"#);

        let mut response = String::new();
        if std::io::BufRead::read_line(&mut std::io::BufReader::new(&stream), &mut response).is_ok()
            && !response.trim().is_empty()
        {
            return Ok(response.trim().to_string());
        }
    }

    // Fallback: return basic info from PID file.
    Ok(format!(
        r#"{{"type":"status","pid":{},"session_id":"{}"}}"#,
        daemon_pid, session_id
    ))
}

#[cfg(test)]
mod log_tests {
    use super::head;

    #[test]
    fn head_cuts_on_character_boundaries() {
        // Regression: byte slicing at 60 landed mid-character in Chinese
        // intents and crashed the daemon inside its own log macro.
        let chinese = "提交".repeat(50);
        assert_eq!(head(&chinese, 60).chars().count(), 60);
        assert_eq!(head("plain ascii", 5), "plain");
        assert_eq!(head("short", 100), "short");
    }
}

#[cfg(test)]
mod backfill_tests {
    use super::*;

    fn sample_event(id: u64) -> crate::event::EditEvent {
        crate::event::EditEvent {
            id,
            ts: 1_700_000_000_000,
            file: "src/a.rs".to_string(),
            kind: crate::event::EditKind::Modify,
            patch: String::new(),
            before_hash: None,
            after_hash: "h".to_string(),
            intent: None,
            tool: None,
            lines_added: 1,
            lines_removed: 0,
            agent_id: Some("s".to_string()),
            agent_label: Some("claude-code-1".to_string()),
            operation_id: Some("s:call_x".to_string()),
            operation_intent: None,
            tool_name: Some("Edit".to_string()),
            restore_id: None,
        }
    }

    #[test]
    fn backfill_queue_survives_save_load_handoff() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let old_session = sessions.join("20260828-070000-000001");
        std::fs::create_dir_all(&old_session).unwrap();

        save_backfill(
            &old_session,
            &[BackfillItem {
                session_dir: old_session.clone(),
                event: sample_event(7),
            }],
        );
        assert!(old_session.join("backfill.json").exists());

        // A restarted daemon picks the queue up and takes file ownership.
        let loaded = load_backfill(&sessions, "20260828-080000-000002");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].event.id, 7);
        assert_eq!(loaded[0].session_dir, old_session);
        assert!(
            !old_session.join("backfill.json").exists(),
            "loading must take ownership of the queue file"
        );

        // Saving an empty queue removes the file entirely.
        let other = sessions.join("20260828-090000-000003");
        std::fs::create_dir_all(&other).unwrap();
        save_backfill(
            &other,
            &[BackfillItem {
                session_dir: other.clone(),
                event: sample_event(8),
            }],
        );
        save_backfill(&other, &[]);
        assert!(!other.join("backfill.json").exists());
    }
}

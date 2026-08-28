// Console subsystem on purpose. `windows_subsystem = "windows"` makes pwsh/cmd
// return to the prompt immediately, so the shell and the process share one
// console. Daemon/hook/MCP flashes are prevented at spawn time with
// CREATE_NO_WINDOW, not by hiding the PE subsystem.

use clap::Parser;
use std::path::PathBuf;

use aitrace::import::claude::{import_session, list_sessions};
use aitrace::restore::RestoreEngine;
use aitrace::session::SessionManager;
use aitrace::snapshot::edit_log::EditLog;
use aitrace::snapshot::store::SnapshotStore;

#[derive(Parser)]
#[command(
    name = "aitrace",
    about = "Trace, replay, and rewind AI coding edits",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("AITRACE_BUILD_HASH"), ")")
)]
struct Cli {
    /// Project directory to watch (defaults to current directory)
    path: Option<String>,

    /// Internal: run as daemon child process (do not use directly)
    #[arg(long, hide = true)]
    daemon_child: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Print a session's timeline as text
    Replay { session_id: String },
    /// List past sessions
    Sessions,
    /// Create default config
    Init,
    /// Import a past Claude Code session for replay
    Import {
        /// Session ID or path to JSONL file (lists available sessions if omitted)
        session: Option<String>,
    },
    /// Restore a file to a prior state
    Restore {
        /// File path to restore (relative to project root)
        file: String,
        /// Edit ID to restore to (from edits.jsonl)
        #[arg(long)]
        edit_id: u64,
    },
    /// Export a session to external formats (Agent Trace JSON, git notes)
    Export {
        /// Output format
        #[arg(long, value_enum)]
        format: aitrace::export::ExportFormat,
        /// Session ID (from `aitrace sessions`)
        session_id: String,
        /// Output file path (default: stdout for agent-trace, git note on HEAD for git-notes)
        #[arg(long)]
        output: Option<String>,
    },
    /// Start MCP server (stdio JSON-RPC for AI coding assistants)
    Mcp,
    /// Manage the background recorder daemon
    Daemon {
        #[command(subcommand)]
        command: DaemonCommands,
    },
    /// Internal: forward a Claude Code hook payload to the daemon
    #[command(hide = true)]
    HookSend {
        /// Project directory (defaults to cwd)
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(clap::Subcommand)]
enum DaemonCommands {
    /// Start the background recorder
    Start,
    /// Stop the background recorder
    Stop,
    /// Kill leftover --daemon-child processes (not MCP) and stale pid files
    Reap,
    /// Show daemon status
    Status,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Black box for crashes: every panic is appended to
    // <project>/.aitrace/panic.log with a backtrace before the default
    // hook runs, so even a fast-exiting failure leaves a trace.
    {
        let panic_log = resolve_path(cli.path.as_deref())?
            .join(".aitrace")
            .join("panic.log");
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&panic_log)
            {
                let _ = writeln!(
                    f,
                    "{} {info}\n{}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                    std::backtrace::Backtrace::force_capture()
                );
            }
            prev_hook(info);
        }));
    }

    // ── Daemon child mode ────────────────────────────────────────────────────
    // When spawned with --daemon-child, run the daemon main loop directly.
    if cli.daemon_child {
        let project_path = resolve_path(cli.path.as_deref())?;
        let config = load_config_or_default(&project_path);
        return aitrace::daemon::run_daemon(project_path, config);
    }

    match cli.command {
        // ── Daemon subcommands ───────────────────────────────────────────────
        Some(Commands::Daemon { command }) => {
            let project_path = resolve_path(cli.path.as_deref())?;

            match command {
                DaemonCommands::Start => match aitrace::daemon::start_daemon(&project_path) {
                    Ok((pid, session_id)) => {
                        println!("daemon started (PID {}, session {})", pid, session_id);
                    }
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                },

                DaemonCommands::Stop => match aitrace::daemon::stop_daemon(&project_path) {
                    Ok(()) => {
                        println!("daemon stopped");
                    }
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                },

                DaemonCommands::Reap => match aitrace::daemon::reap::reap(&project_path) {
                    Ok(report) => {
                        println!("{}", report.summary());
                    }
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(1);
                    }
                },

                DaemonCommands::Status => {
                    match aitrace::daemon::daemon_status(&project_path) {
                        Ok(status_json) => {
                            // Pretty-print the status.
                            if let Ok(value) =
                                serde_json::from_str::<serde_json::Value>(&status_json)
                            {
                                let pid = value.get("pid").and_then(|v| v.as_i64()).unwrap_or(0);
                                let session = value
                                    .get("session_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let uptime = value
                                    .get("uptime_secs")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0);
                                let edits = value
                                    .get("edit_count")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let agents = value
                                    .get("agents")
                                    .and_then(|v| v.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);

                                println!("pid:       {}", pid);
                                println!("session:   {}", session);
                                println!(
                                    "version:   {} ({})",
                                    value.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
                                    value
                                        .get("build_hash")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("?")
                                );
                                println!("uptime:    {}s", uptime);
                                println!("edits:     {}", edits);
                                println!("agents:    {}", agents);
                            } else {
                                println!("{}", status_json);
                            }
                        }
                        Err(e) => {
                            eprintln!("error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }

        // ── Init: write auto-detected config ──────────────────────────────────
        Some(Commands::Init) => {
            let project_path = resolve_path(cli.path.as_deref())?;
            let vt_dir = project_path.join(".aitrace");
            std::fs::create_dir_all(&vt_dir)?;
            match aitrace::install::install_project_bin(&project_path) {
                Ok(bin) => println!("installed project bin {}", bin.display()),
                Err(e) => eprintln!("hint: could not install project bin: {e}"),
            }
            let config_path = vt_dir.join("config.toml");

            if config_path.exists() {
                println!("config already exists at {}", config_path.display());
            } else {
                let config = aitrace::auto_detect::auto_detect_config(&project_path);
                let toml_str = toml::to_string_pretty(&config)?;
                let header = "# aitrace configuration (auto-generated)\n# https://github.com/raystyle/aitrace\n# Generated by: aitrace init\n\n";
                std::fs::write(&config_path, format!("{header}{toml_str}"))?;

                // Print what was detected
                let const_count = config.watchdog.constants.len();
                let dep_count = config.blast_radius.manual.len();
                println!("wrote config to {}", config_path.display());
                if const_count > 0 {
                    println!("  detected {} watchdog constants", const_count);
                }
                if dep_count > 0 {
                    println!("  detected {} dependency mappings", dep_count);
                }
                if const_count == 0 && dep_count == 0 {
                    println!("  no auto-detectable patterns found (edit config manually)");
                }
            }

            // Suggest adding .aitrace/ to .gitignore
            let gitignore = project_path.join(".gitignore");
            if gitignore.exists() {
                let content = std::fs::read_to_string(&gitignore).unwrap_or_default();
                if !content.contains(".aitrace") {
                    println!("hint: add .aitrace/ to your .gitignore");
                }
            } else {
                println!("hint: add .aitrace/ to your .gitignore");
            }

            let claude_dir = project_path.join(".claude");
            if claude_dir.is_dir() {
                match aitrace::hook::registration::register_hook(&claude_dir, &project_path) {
                    Ok(()) => {}
                    Err(e) => eprintln!("hint: could not register Claude Code hook: {e}"),
                }
            }

            // Detect agents
            let agents = aitrace::import::detect::detect_agents(&project_path);
            if agents.is_empty() {
                println!("  no AI agents detected (start an agent and run init again)");
            } else {
                println!("  detected agents:");
                for agent in &agents {
                    println!("    - {} ({})", agent.name, agent.log_path.display());
                }
            }

            // Configure git notes.rewriteRef for git-notes export compatibility
            let notes_configured = std::process::Command::new("git")
                .args(["config", "--get", "notes.rewriteRef"])
                .current_dir(&project_path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if !notes_configured {
                let _ = std::process::Command::new("git")
                    .args(["config", "notes.rewriteRef", "refs/notes/commits"])
                    .current_dir(&project_path)
                    .output();
                println!("  configured git notes.rewriteRef for export compatibility");
            }
        }

        // ── Sessions: list past sessions ───────────────────────────────────────
        Some(Commands::Sessions) => {
            let project_path = resolve_path(cli.path.as_deref())?;
            let sessions_dir = project_path.join(".aitrace").join("sessions");
            let manager = SessionManager::new(sessions_dir);
            let sessions = manager.list()?;

            if sessions.is_empty() {
                println!("no sessions found");
            } else {
                println!("{:<30}  {:<20}  {:<8}  mode", "id", "started_at", "agents");
                println!("{}", "-".repeat(72));
                for meta in sessions {
                    let dt = chrono::DateTime::from_timestamp_millis(meta.started_at)
                        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                        .unwrap_or_else(|| meta.started_at.to_string());
                    let agent_count = meta.agents.len();
                    println!(
                        "{:<30}  {:<20}  {:<8}  {:?}",
                        meta.id, dt, agent_count, meta.mode
                    );
                }
            }
        }

        // ── Replay: load session and replay in TUI ─────────────────────────────
        Some(Commands::Replay { session_id }) => {
            let project_path = resolve_path(cli.path.as_deref())?;
            let sessions_dir = project_path.join(".aitrace").join("sessions");
            let manager = SessionManager::new(sessions_dir);

            // Load edit log for the session.
            let session_dir = manager.sessions_dir.join(&session_id);
            let edit_log_path = session_dir.join("edits.jsonl");

            if !edit_log_path.exists() {
                anyhow::bail!("no edit log found for session {}", session_id);
            }

            let edits = EditLog::read_all(&edit_log_path)?;

            println!("session {} — {} edits", session_id, edits.len());
            println!("{:<5}  {:<9}  {:<6}  {:<6}  file", "#", "time", "+", "-");
            println!("{}", "-".repeat(72));
            for e in &edits {
                let time = chrono::DateTime::from_timestamp_millis(e.ts)
                    .map(|d| d.format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| e.ts.to_string());
                println!(
                    "{:<5}  {:<9}  {:<6}  {:<6}  {}",
                    e.id, time, e.lines_added, e.lines_removed, e.file
                );
                if let Some(intent) = &e.operation_intent {
                    println!("      op:  {}", intent);
                }
                if let Some(intent) = &e.intent {
                    println!("      ask: {}", intent);
                }
            }
            let added: u32 = edits.iter().map(|e| e.lines_added).sum();
            let removed: u32 = edits.iter().map(|e| e.lines_removed).sum();
            println!("{}", "-".repeat(72));
            println!("total: +{added} / -{removed} across {} edits", edits.len());
        }

        // ── Import: import a past Claude Code session ─────────────────────────
        Some(Commands::Import { session }) => {
            let project_path = resolve_path(cli.path.as_deref())?;

            match session {
                None => {
                    // List available sessions
                    let sessions = list_sessions(&project_path)?;
                    if sessions.is_empty() {
                        println!("no Claude Code sessions found for this project");
                    } else {
                        println!("{:<40}  {:<22}  edits", "id", "started_at");
                        println!("{}", "-".repeat(70));
                        for s in sessions {
                            let dt = chrono::DateTime::from_timestamp_millis(s.started_at)
                                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                                .unwrap_or_else(|| s.started_at.to_string());
                            println!("{:<40}  {:<22}  {}", s.id, dt, s.edit_count);
                        }
                    }
                }
                Some(session_arg) => {
                    // Resolve the JSONL path
                    let jsonl_path = if session_arg.ends_with(".jsonl") {
                        PathBuf::from(&session_arg)
                    } else {
                        // Treat as UUID -- look it up under ~/.claude/projects/
                        let home = dirs::home_dir()
                            .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
                        let converted = project_path.to_string_lossy().replace('/', "-");
                        home.join(".claude")
                            .join("projects")
                            .join(&converted)
                            .join(format!("{}.jsonl", session_arg))
                    };

                    if !jsonl_path.exists() {
                        anyhow::bail!("session file not found: {}", jsonl_path.display());
                    }

                    let edits = import_session(&jsonl_path, &project_path)?;

                    println!(
                        "imported {} edits from {}",
                        edits.len(),
                        jsonl_path.display()
                    );
                    println!("inspect with: aitrace replay <session>");
                }
            }
        }

        // ── Restore: headless file restore to a prior edit ────────────────────
        Some(Commands::Restore { file, edit_id }) => {
            let project_path = resolve_path(cli.path.as_deref())?;
            let vt_dir = project_path.join(".aitrace");

            // Find the active (or most recent) session directory.
            let session_dir = {
                let pid_path = vt_dir.join("daemon.pid");
                if pid_path.exists() {
                    // Daemon is (or was) running -- read session ID from PID file.
                    match aitrace::daemon::pid::read_pid_file(&pid_path) {
                        Ok((_pid, session_id)) => {
                            let dir = vt_dir.join("sessions").join(&session_id);
                            if dir.exists() {
                                dir
                            } else {
                                find_most_recent_session(&vt_dir)?
                            }
                        }
                        Err(_) => find_most_recent_session(&vt_dir)?,
                    }
                } else {
                    find_most_recent_session(&vt_dir)?
                }
            };

            let edit_log_path = session_dir.join("edits.jsonl");
            if !edit_log_path.exists() {
                anyhow::bail!(
                    "no edit log found in session dir: {}",
                    session_dir.display()
                );
            }

            let edits = EditLog::read_all(&edit_log_path)?;
            let target = edits
                .iter()
                .find(|e| e.id == edit_id)
                .ok_or_else(|| anyhow::anyhow!("edit id {} not found in session", edit_id))?;

            // We restore to the before_hash of the target edit (state before that edit).
            let hash = target.before_hash.as_deref().ok_or_else(|| {
                anyhow::anyhow!("edit {} has no before_hash -- cannot restore", edit_id)
            })?;

            let store_dir = session_dir.join("snapshots");
            let store = SnapshotStore::new(store_dir);
            let engine = RestoreEngine::new(project_path.clone(), store);

            engine.restore_file(&file, hash)?;
            println!("restored {} to state before edit {}", file, edit_id);
        }

        // ── Export: export a session to external formats ─────────────────────
        Some(Commands::Export {
            format,
            session_id,
            output,
        }) => {
            let project_path = resolve_path(cli.path.as_deref())?;
            let sessions_dir = project_path.join(".aitrace").join("sessions");
            let edit_log_path = sessions_dir.join(&session_id).join("edits.jsonl");
            if !edit_log_path.exists() {
                anyhow::bail!("no edit log found for session {}", session_id);
            }
            let edits = EditLog::read_all(&edit_log_path)?;
            match format {
                aitrace::export::ExportFormat::AgentTrace => {
                    let output_path = output.as_deref().map(std::path::Path::new);
                    aitrace::export::agent_trace::export_agent_trace_to_path(
                        &edits,
                        &session_id,
                        output_path,
                    )?;
                }
                aitrace::export::ExportFormat::GitNotes => {
                    aitrace::export::git_notes::export_git_notes(
                        &edits,
                        &project_path,
                        output.as_deref(),
                    )?;
                }
            }
        }

        // ── MCP: start stdio JSON-RPC server ─────────────────────────────────
        Some(Commands::Mcp) => {
            let project_path = resolve_path(cli.path.as_deref())?;
            aitrace::mcp::run_mcp_server(project_path)?;
        }

        Some(Commands::HookSend { project }) => {
            let project_path = resolve_path(project.as_deref())?;
            aitrace::hook::send::send_to_daemon(&project_path)?;
        }

        // ── Default: headless status summary ───────────────────────────────────
        // aitrace is a daemon + CLI + MCP by design; there is no TUI.
        None => {
            let project_path = resolve_path(cli.path.as_deref())?;

            match aitrace::daemon::daemon_status(&project_path) {
                Ok(status_json) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&status_json) {
                        println!(
                            "daemon: pid {} · {} · {} ({}) · {} edits · {} agents",
                            v.get("pid").and_then(|x| x.as_i64()).unwrap_or(0),
                            v.get("session_id").and_then(|x| x.as_str()).unwrap_or("?"),
                            v.get("version").and_then(|x| x.as_str()).unwrap_or("?"),
                            v.get("build_hash").and_then(|x| x.as_str()).unwrap_or("?"),
                            v.get("edit_count").and_then(|x| x.as_u64()).unwrap_or(0),
                            v.get("agents")
                                .and_then(|x| x.as_array())
                                .map(|a| a.len())
                                .unwrap_or(0),
                        );
                    } else {
                        println!("daemon: running (unparsable status)");
                    }
                }
                Err(_) => println!("daemon: not running -- start with `aitrace daemon start`"),
            }

            let sessions_dir = project_path.join(".aitrace").join("sessions");
            let sessions = SessionManager::new(sessions_dir).list().unwrap_or_default();
            println!("sessions: {} recorded", sessions.len());
            println!(
                "inspect:  aitrace sessions · aitrace replay <id> · aitrace mcp (stdio JSON-RPC)"
            );
        }
    }

    Ok(())
}

/// Resolve the project path from an optional CLI argument (defaults to cwd).
fn resolve_path(arg: Option<&str>) -> anyhow::Result<PathBuf> {
    let raw = match arg {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir()?,
    };
    Ok(aitrace::project::workspace_root(&raw))
}

/// Load config from `.aitrace/config.toml`, falling back to defaults.
fn load_config_or_default(project_path: &std::path::Path) -> aitrace::config::Config {
    let config_path = project_path.join(".aitrace").join("config.toml");
    aitrace::config::Config::load(&config_path).unwrap_or_default()
}

/// Find the most recently modified session directory under `vt_dir/sessions/`.
fn find_most_recent_session(vt_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let sessions_dir = vt_dir.join("sessions");
    if !sessions_dir.exists() {
        anyhow::bail!("no sessions directory found at {}", sessions_dir.display());
    }

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join("meta.json");
        if !meta_path.exists() {
            continue;
        }
        let modified = entry
            .metadata()?
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, path));
        }
    }

    best.map(|(_, p)| p)
        .ok_or_else(|| anyhow::anyhow!("no sessions found under {}", sessions_dir.display()))
}

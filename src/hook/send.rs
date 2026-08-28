//! Forward a Claude Code hook payload to the local daemon socket.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use crate::ipc;

/// Read hook JSON from stdin (or env fallback) and write one line to `daemon.sock`.
///
/// Returns `Ok` if the daemon is not running so Claude Code is not blocked.
pub fn send_to_daemon(project_path: &Path) -> Result<()> {
    let sock = project_path.join(".aitrace").join("daemon.sock");
    let mut stream = match ipc::connect(&sock) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    let line = payload_line();
    writeln!(stream, "{line}")?;
    Ok(())
}

fn payload_line() -> String {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let trimmed = raw.trim();

    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return build_payload(&v);
    }

    json!({
        "type": "hook",
        "agent_id": std::process::id().to_string(),
        "operation_id": "unknown",
        "tool_name": std::env::var("TOOL_NAME").unwrap_or_default(),
        "file": "",
    })
    .to_string()
}

/// Build a daemon hook message from a Claude Code PostToolUse payload.
fn build_payload(v: &Value) -> String {
    // Already-shaped daemon messages pass through untouched.
    if v.get("type").and_then(|t| t.as_str()) == Some("hook") {
        return v.to_string();
    }
    let tool_name = v.get("tool_name").and_then(|x| x.as_str()).unwrap_or("");
    let file = v
        .get("tool_input")
        .and_then(|t| t.get("file_path").or_else(|| t.get("path")))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    // Claude Code's PostToolUse payload carries a stable `session_id`.
    // Use it as the agent identity so every edit from one session shares
    // one agent label, instead of the hook-send process's per-invocation
    // PID (which registers a new agent on every tool call).
    let session_id = v.get("session_id").and_then(|x| x.as_str()).unwrap_or("");
    let agent_id = if session_id.is_empty() {
        std::process::id().to_string()
    } else {
        session_id.to_string()
    };
    // Claude Code reports no per-operation id; prefer a tool-use id when
    // present, otherwise fall back to the session so edits can still be
    // grouped by it.
    let operation_id = match (
        v.get("tool_use_id").and_then(|x| x.as_str()),
        session_id.is_empty(),
    ) {
        (Some(tool_use_id), false) => format!("{session_id}:{tool_use_id}"),
        _ => agent_id.clone(),
    };
    // The transcript backs intent resolution in the daemon; is_error marks
    // failed tool calls, which the daemon counts instead of enriching.
    let transcript_path = v.get("transcript_path").and_then(|x| x.as_str());
    let is_error = v
        .get("tool_response")
        .and_then(|r| r.get("is_error"))
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    json!({
        "type": "hook",
        "agent_id": agent_id,
        "operation_id": operation_id,
        "tool_name": tool_name,
        "file": file,
        "transcript_path": transcript_path,
        "is_error": is_error,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_from_claude_hook_json() {
        let input = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/main.rs"}}"#;
        let v: Value = serde_json::from_str(input).unwrap();
        let tool_name = v.get("tool_name").and_then(|x| x.as_str()).unwrap();
        let file = v["tool_input"]["file_path"].as_str().unwrap();
        assert_eq!(tool_name, "Edit");
        assert_eq!(file, "src/main.rs");
    }

    #[test]
    fn session_id_becomes_agent_and_operation_id() {
        let v: Value = serde_json::from_str(
            r#"{"session_id":"abc-123","tool_name":"Edit","tool_input":{"file_path":"src/main.rs"}}"#,
        )
        .unwrap();
        let out: Value = serde_json::from_str(&build_payload(&v)).unwrap();
        assert_eq!(out["agent_id"], "abc-123");
        assert_eq!(out["operation_id"], "abc-123");
        assert_eq!(out["tool_name"], "Edit");
        assert_eq!(out["file"], "src/main.rs");
    }

    #[test]
    fn tool_use_id_refines_operation_id() {
        let v: Value = serde_json::from_str(
            r#"{"session_id":"abc-123","tool_use_id":"toolu_9","tool_name":"Write","tool_input":{"file_path":"a.rs"}}"#,
        )
        .unwrap();
        let out: Value = serde_json::from_str(&build_payload(&v)).unwrap();
        assert_eq!(out["agent_id"], "abc-123");
        assert_eq!(out["operation_id"], "abc-123:toolu_9");
    }

    #[test]
    fn missing_session_id_falls_back_to_pid() {
        let v: Value =
            serde_json::from_str(r#"{"tool_name":"Edit","tool_input":{"file_path":"a.rs"}}"#)
                .unwrap();
        let out: Value = serde_json::from_str(&build_payload(&v)).unwrap();
        assert!(!out["agent_id"].as_str().unwrap().is_empty());
        assert_eq!(out["agent_id"], out["operation_id"]);
    }

    #[test]
    fn pre_shaped_hook_message_passes_through() {
        let v: Value = serde_json::from_str(
            r#"{"type":"hook","agent_id":"a","operation_id":"o","tool_name":"Edit","file":"a.rs"}"#,
        )
        .unwrap();
        let out = build_payload(&v);
        assert_eq!(out, v.to_string());
    }
}

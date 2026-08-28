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
        if v.get("type").and_then(|t| t.as_str()) == Some("hook") {
            return trimmed.to_string();
        }
        let tool_name = v.get("tool_name").and_then(|x| x.as_str()).unwrap_or("");
        let file = v
            .get("tool_input")
            .and_then(|t| t.get("file_path").or_else(|| t.get("path")))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        return json!({
            "type": "hook",
            "agent_id": std::process::id().to_string(),
            "operation_id": "unknown",
            "tool_name": tool_name,
            "file": file,
        })
        .to_string();
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
}

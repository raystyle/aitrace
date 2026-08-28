use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::ipc;

use anyhow::{Context, Result};

use super::transport::StdioWriter;
use super::types::JsonRpcNotification;

pub fn handle_subscribe(
    arguments: &serde_json::Value,
    project_path: &Path,
    _sessions_dir: &Path,
    writer: &mut StdioWriter,
) -> Result<serde_json::Value> {
    let session_id = arguments
        .get("session_id")
        .and_then(|v| v.as_str())
        .context("missing session_id")?;

    let sock_path = project_path.join(".aitrace").join("daemon.sock");
    let mut stream = match ipc::connect(&sock_path) {
        Ok(s) => s,
        Err(_) => anyhow::bail!("daemon is not active — start with `aitrace daemon start`"),
    };

    // Send subscribe message
    let subscribe_msg = serde_json::json!({
        "type": "subscribe",
        "session_id": session_id,
    });
    writeln!(stream, "{}", serde_json::to_string(&subscribe_msg)?)?;
    stream.flush()?;

    // Read notifications and forward as MCP notifications
    let reader = BufReader::new(stream);
    let mut events_forwarded: u64 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if value.get("type").and_then(|v| v.as_str()) == Some("edit_notification") {
            let notification = JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "notifications/tools/edit_event".to_string(),
                params: Some(value["event"].clone()),
            };

            if writer.write_notification(&notification).is_err() {
                break; // MCP client disconnected
            }

            events_forwarded += 1;
        }
    }

    Ok(serde_json::json!({
        "status": "subscription_ended",
        "events_forwarded": events_forwarded,
    }))
}

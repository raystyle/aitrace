use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

const AITRACE_DESCRIPTION: &str = "aitrace edit tracking";
const POST_TOOL_USE: &str = "PostToolUse";
const MATCHER: &str = "Write|Edit";

/// Register an aitrace hook in `.claude/settings.local.json`.
///
/// Skips writing a local hook when the committed `.claude/settings.json`
/// already contains an aitrace PostToolUse handler (project-scoped config).
/// Otherwise reads or creates `settings.local.json`, removes any existing
/// aitrace hook, then appends an official-schema `PostToolUse` exec-form
/// hook that runs `aitrace hook-send`.
pub fn register_hook(claude_dir: &Path, project_path: &Path) -> Result<()> {
    let shared_path = claude_dir.join("settings.json");
    if let Some(shared) = read_json_if_exists(&shared_path)? {
        if settings_has_aitrace_hook(&shared) {
            return Ok(());
        }
    }

    let settings_path = claude_dir.join("settings.local.json");
    let mut settings = read_json_if_exists(&settings_path)?.unwrap_or_else(|| json!({}));
    ensure_post_tool_use_array(&mut settings);
    upsert_aitrace_hook(&mut settings, project_path)?;
    write_settings(claude_dir, &settings_path, &settings)
}

/// Remove all aitrace hooks from `.claude/settings.local.json`.
pub fn unregister_hook(claude_dir: &Path) -> Result<()> {
    let settings_path = claude_dir.join("settings.local.json");

    if !settings_path.exists() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(&settings_path)
        .with_context(|| format!("read {:?}", settings_path))?;
    let mut settings: Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {:?}", settings_path))?;

    remove_aitrace_hooks(&mut settings);

    let json_str = serde_json::to_string_pretty(&settings).context("serialize settings")?;
    std::fs::write(&settings_path, json_str)
        .with_context(|| format!("write {:?}", settings_path))?;

    Ok(())
}

fn read_json_if_exists(path: &Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {:?}", path))?;
    let value = serde_json::from_str(&raw).with_context(|| format!("parse {:?}", path))?;
    Ok(Some(value))
}

fn write_settings(claude_dir: &Path, settings_path: &Path, settings: &Value) -> Result<()> {
    std::fs::create_dir_all(claude_dir).with_context(|| format!("create dir {:?}", claude_dir))?;
    let json_str = serde_json::to_string_pretty(settings).context("serialize settings")?;
    std::fs::write(settings_path, json_str)
        .with_context(|| format!("write {:?}", settings_path))?;
    Ok(())
}

/// Official schema: `hooks` is an object keyed by event name.
/// Migrate a legacy top-level array into that object.
fn ensure_post_tool_use_array(settings: &mut Value) {
    let legacy_groups = match settings.get("hooks") {
        Some(Value::Array(arr)) => Some(arr.clone()),
        _ => None,
    };

    if !settings
        .get("hooks")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        settings["hooks"] = json!({});
    }

    if let Some(groups) = legacy_groups {
        let dest = settings["hooks"]
            .as_object_mut()
            .expect("hooks object just set");
        dest.entry(POST_TOOL_USE).or_insert_with(|| json!([]));
        if let Some(Value::Array(existing)) = dest.get_mut(POST_TOOL_USE) {
            existing.extend(groups);
        }
    }

    if !settings["hooks"]
        .get(POST_TOOL_USE)
        .map(|v| v.is_array())
        .unwrap_or(false)
    {
        settings["hooks"][POST_TOOL_USE] = json!([]);
    }
}

fn upsert_aitrace_hook(settings: &mut Value, project_path: &Path) -> Result<()> {
    ensure_post_tool_use_array(settings);
    let groups = settings["hooks"][POST_TOOL_USE]
        .as_array_mut()
        .expect("PostToolUse array");
    groups.retain(|entry| !entry_has_aitrace_description(entry));

    let exe = crate::install::bin_path(project_path);
    groups.push(json!({
        "matcher": MATCHER,
        "hooks": [
            {
                "type": "command",
                "command": exe.to_string_lossy(),
                "args": ["hook-send", "--project", project_path.to_string_lossy()],
                "description": AITRACE_DESCRIPTION,
                "timeout": 10
            }
        ]
    }));
    Ok(())
}

fn remove_aitrace_hooks(settings: &mut Value) {
    match settings.get_mut("hooks") {
        Some(Value::Array(hooks)) => {
            hooks.retain(|entry| !entry_has_aitrace_description(entry));
        }
        Some(Value::Object(map)) => {
            if let Some(Value::Array(groups)) = map.get_mut(POST_TOOL_USE) {
                groups.retain(|entry| !entry_has_aitrace_description(entry));
            }
        }
        _ => {}
    }
}

fn settings_has_aitrace_hook(settings: &Value) -> bool {
    match settings.get("hooks") {
        Some(Value::Array(hooks)) => hooks.iter().any(entry_has_aitrace_description),
        Some(Value::Object(map)) => map.values().any(|event| {
            event
                .as_array()
                .map(|groups| groups.iter().any(entry_has_aitrace_description))
                .unwrap_or(false)
        }),
        _ => false,
    }
}

/// Return `true` if a matcher group is an aitrace hook (description or hook-send).
fn entry_has_aitrace_description(entry: &Value) -> bool {
    let Some(arr) = entry.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    for hook in arr {
        if let Some(desc) = hook.get("description").and_then(Value::as_str) {
            if desc.contains("aitrace") {
                return true;
            }
        }
        if hook
            .get("args")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            == Some("hook-send")
        {
            return true;
        }
        if hook
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|c| c.contains("aitrace"))
        {
            return true;
        }
    }
    false
}

// ─── unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_register_creates_settings_file() {
        let tmp = tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        let project_path = tmp.path();
        register_hook(&claude_dir, project_path).unwrap();
        assert!(claude_dir.join("settings.local.json").exists());
    }

    #[test]
    fn test_register_uses_official_post_tool_use_schema() {
        let tmp = tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        let project_path = tmp.path();
        register_hook(&claude_dir, project_path).unwrap();

        let raw = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        let settings: Value = serde_json::from_str(&raw).unwrap();
        let group = &settings["hooks"]["PostToolUse"][0];
        assert_eq!(group["matcher"], MATCHER);
        let hook = &group["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(hook["args"][0], "hook-send");
        assert_eq!(hook["args"][1], "--project");
        assert_eq!(hook["args"][2], project_path.to_string_lossy().as_ref());
        let command = hook["command"].as_str().unwrap();
        assert!(
            command
                .replace('\\', "/")
                .ends_with(".aitrace/bin/aitrace.exe")
        );
    }

    #[test]
    fn test_register_skips_local_when_shared_settings_have_hook() {
        let tmp = tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let shared = json!({
            "hooks": {
                "PostToolUse": [{
                    "matcher": MATCHER,
                    "hooks": [{
                        "type": "command",
                        "command": "aitrace",
                        "args": ["hook-send", "--project", "${CLAUDE_PROJECT_DIR}"],
                        "description": AITRACE_DESCRIPTION
                    }]
                }]
            }
        });
        std::fs::write(
            claude_dir.join("settings.json"),
            serde_json::to_string_pretty(&shared).unwrap(),
        )
        .unwrap();

        register_hook(&claude_dir, tmp.path()).unwrap();
        assert!(
            !claude_dir.join("settings.local.json").exists(),
            "must not write a duplicate local hook"
        );
    }

    #[test]
    fn test_unregister_removes_aitrace_entry() {
        let tmp = tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        let project_path = tmp.path();
        register_hook(&claude_dir, project_path).unwrap();
        unregister_hook(&claude_dir).unwrap();

        let raw = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        assert!(!raw.contains("aitrace"));
    }

    #[test]
    fn test_migrates_legacy_hooks_array() {
        let tmp = tempdir().unwrap();
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let legacy = json!({
            "hooks": [{
                "matcher": "PostToolUse",
                "hooks": [{
                    "type": "command",
                    "command": "old",
                    "description": "other hook"
                }]
            }]
        });
        std::fs::write(
            claude_dir.join("settings.local.json"),
            serde_json::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        register_hook(&claude_dir, tmp.path()).unwrap();
        let raw = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
        let settings: Value = serde_json::from_str(&raw).unwrap();
        assert!(settings["hooks"].is_object());
        assert_eq!(
            settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
            2
        );
    }
}

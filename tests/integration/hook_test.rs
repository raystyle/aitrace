use aitrace::hook::registration::{register_hook, unregister_hook};
use tempfile::tempdir;

#[test]
fn test_register_hook_creates_settings() {
    let tmp = tempdir().unwrap();
    let claude_dir = tmp.path().join(".claude");
    let project_path = tmp.path();

    register_hook(&claude_dir, project_path).expect("register hook");

    let settings_path = claude_dir.join("settings.local.json");
    assert!(
        settings_path.exists(),
        "settings.local.json should be created"
    );

    let contents = std::fs::read_to_string(&settings_path).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&contents).unwrap();
    let group = &settings["hooks"]["PostToolUse"][0];
    assert_eq!(group["matcher"], "Write|Edit");
    let hook = &group["hooks"][0];
    assert_eq!(hook["args"][0], "hook-send");
    assert_eq!(hook["args"][2], project_path.to_string_lossy().as_ref());
    let command = hook["command"].as_str().unwrap();
    assert!(
        command.contains(".aitrace") && command.contains("aitrace.exe"),
        "hook command should be the project-local bin, got {command}"
    );
}

#[test]
fn test_unregister_hook_removes_entry() {
    let tmp = tempdir().unwrap();
    let claude_dir = tmp.path().join(".claude");
    let project_path = tmp.path();

    register_hook(&claude_dir, project_path).expect("register hook");
    unregister_hook(&claude_dir).expect("unregister hook");

    let settings_path = claude_dir.join("settings.local.json");
    assert!(settings_path.exists(), "settings file should still exist");

    let contents = std::fs::read_to_string(&settings_path).unwrap();
    assert!(
        !contents.contains("aitrace"),
        "settings should not contain 'aitrace' after unregistration"
    );
}

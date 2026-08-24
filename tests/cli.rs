use std::process::Command;

fn wut() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wut"))
}

#[test]
fn help_exposes_the_small_scriptable_interface() {
    let output = wut().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("wut [OPTIONS] [QUESTION...]"));
    assert!(stdout.contains("wut agents"));
    assert!(stdout.contains("wut sessions"));
    assert!(stdout.contains("wut config"));
    assert!(!stdout.contains("--upgrade"));
}

#[test]
fn unknown_options_fail_without_running_an_agent() {
    let output = wut().arg("--definitely-not-real").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("wut:"));
}

#[test]
fn agents_json_is_machine_readable_and_has_cursor_and_grok() {
    let output = wut().args(["agents", "--json"]).output().unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let agents = value.as_array().unwrap();
    assert!(agents.iter().any(|agent| agent["id"] == "cursor"));
    assert!(agents.iter().any(|agent| agent["id"] == "grok"));
}

#[test]
fn config_show_uses_wut_paths_and_cursor_default() {
    let root = std::env::temp_dir().join(format!("wut-cli-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let output = wut()
        .args(["config", "show", "--json"])
        .env("HOME", &root)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("WUT_CONFIG")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["agent"], "cursor");
    assert!(
        value["path"]
            .as_str()
            .unwrap()
            .contains("/.config/wut/config.json")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn session_json_lists_summaries_without_native_ids_or_transcripts() {
    let root = std::env::temp_dir().join(format!("wut-session-test-{}", std::process::id()));
    let sessions = root.join("state/sessions");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("cursor-test.json"),
        br#"{
          "version": 1,
          "id": "cursor-test",
          "agent": "cursor",
          "native_session_id": "private-provider-id",
          "cwd": "/tmp/project",
          "updated_at": 42,
          "settings": {"model": "grok-fast"},
          "turns": [{"user": "secret question", "assistant": "secret answer"}]
        }"#,
    )
    .unwrap();

    let output = wut()
        .args(["sessions", "--json"])
        .env("HOME", &root)
        .env("WUT_STATE_DIR", root.join("state"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let summary = &value.as_array().unwrap()[0];
    assert_eq!(summary["id"], "cursor-test");
    assert_eq!(summary["turn_count"], 1);
    assert!(summary.get("native_session_id").is_none());
    assert!(summary.get("turns").is_none());
    let _ = std::fs::remove_dir_all(root);
}

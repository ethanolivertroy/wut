use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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

#[test]
fn explicit_session_rejects_a_mismatched_filename_and_embedded_id() {
    let root =
        std::env::temp_dir().join(format!("wut-session-mismatch-test-{}", std::process::id()));
    let sessions = root.join("state/sessions");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("cursor-requested.json"),
        br#"{
          "version": 1,
          "id": "cursor-other",
          "agent": "cursor",
          "native_session_id": "private-other",
          "cwd": "/tmp/project",
          "updated_at": 42,
          "settings": {},
          "turns": []
        }"#,
    )
    .unwrap();

    let output = wut()
        .args(["--session", "cursor-requested", "next"])
        .env("HOME", &root)
        .env("WUT_STATE_DIR", root.join("state"))
        .env("WUT_CURSOR_BIN", root.join("does-not-exist"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("unknown session 'cursor-requested'")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn continuation_loads_only_the_selected_full_transcript() {
    let root = std::env::temp_dir().join(format!("wut-continue-test-{}", std::process::id()));
    let sessions = root.join("state/sessions");
    let project = root.join("project");
    let provider = root.join("fake-cursor");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        sessions.join("cursor-selected.json"),
        format!(
            r#"{{
              "version": 1,
              "id": "cursor-selected",
              "agent": "cursor",
              "native_session_id": "private-selected",
              "cwd": "{}",
              "updated_at": 42,
              "settings": {{}},
              "turns": [{{"user": "first", "assistant": "answer"}}]
            }}"#,
            project.display()
        ),
    )
    .unwrap();
    std::fs::write(
        sessions.join("cursor-unselected.json"),
        format!(
            r#"{{
              "version": 1,
              "id": "cursor-unselected",
              "agent": "cursor",
              "native_session_id": "private-unselected",
              "cwd": "{}",
              "updated_at": 1,
              "settings": {{}},
              "turns": [{{"future": "unselected transcript shape"}}]
            }}"#,
            project.display()
        ),
    )
    .unwrap();
    std::fs::write(
        &provider,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' '{\"type\":\"assistant\",\"timestamp_ms\":1,",
            "\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"next answer\"}]}}'\n",
            "printf '%s\\n' '{\"type\":\"result\",\"session_id\":\"private-selected\",",
            "\"is_error\":false}'\n"
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&provider, permissions).unwrap();

    let output = wut()
        .args(["-c", "next"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("WUT_STATE_DIR", root.join("state"))
        .env("WUT_CURSOR_BIN", &provider)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "next answer\n");

    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sessions.join("cursor-selected.json")).unwrap())
            .unwrap();
    assert_eq!(saved["turns"].as_array().unwrap().len(), 2);
    assert_eq!(saved["turns"][1]["assistant"], "next answer");
    let _ = std::fs::remove_dir_all(root);
}

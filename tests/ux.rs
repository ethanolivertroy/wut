use std::process::Command;

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Stdio;

fn wut() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wut"));
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "WUT_CONFIG",
        "WUT_STATE_DIR",
        "WUT_CURSOR_BIN",
        "WUT_GROK_BIN",
        "WUT_CODEX_BIN",
        "WUT_CLAUDE_BIN",
        "WUT_PI_BIN",
        "WUT_OPENCODE_BIN",
        "ASK_CURSOR_BIN",
        "ASK_GROK_BIN",
        "ASK_CODEX_BIN",
        "ASK_CLAUDE_BIN",
        "ASK_PI_BIN",
        "ASK_OPENCODE_BIN",
    ] {
        command.env_remove(key);
    }
    command
}

#[cfg(unix)]
fn executable(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
#[test]
fn fresh_wut_ignores_stale_ask_state_and_runs_cursor_without_setup() {
    let root = std::env::temp_dir().join(format!("wut-first-principles-{}", std::process::id()));
    let config_home = root.join("config");
    let state_home = root.join("state");
    let project = root.join("project");
    let legacy = config_home.join("ask/config.json");
    let cursor = root.join("cursor-agent");
    let codex = root.join("codex");
    let cursor_args = root.join("cursor-args");
    let codex_ran = root.join("codex-ran");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        &legacy,
        br#"{
          "version": 2,
          "agent": "codex",
          "instructions": "concise",
          "agents": {}
        }"#,
    )
    .unwrap();
    executable(
        &cursor,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$@\" > \"$WUT_TEST_CURSOR_ARGS\"\n",
            "printf '%s\\n' '{\"type\":\"assistant\",\"timestamp_ms\":1,",
            "\"message\":{\"content\":[{\"type\":\"text\",",
            "\"text\":\"Kubernetes answer\"}]}}'\n",
            "printf '%s\\n' '{\"type\":\"result\",\"session_id\":\"cursor-native\",",
            "\"is_error\":false}'\n"
        ),
    );
    executable(
        &codex,
        "#!/bin/sh\nprintf ran > \"$WUT_TEST_CODEX_RAN\"\nexit 1\n",
    );

    let output = wut()
        .args(["what", "is", "kubernetes"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_STATE_HOME", &state_home)
        .env_remove("WUT_CONFIG")
        .env("WUT_CURSOR_BIN", &cursor)
        .env("WUT_CODEX_BIN", &codex)
        .env("WUT_TEST_CURSOR_ARGS", &cursor_args)
        .env("WUT_TEST_CODEX_RAN", &codex_ran)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Kubernetes answer\n"
    );
    assert!(!codex_ran.exists(), "stale ask state selected Codex");
    let args = std::fs::read_to_string(cursor_args).unwrap();
    assert!(args.lines().any(|arg| arg.contains("what is kubernetes")));

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn provider_failure_is_concise_even_when_the_provider_is_noisy() {
    let root = std::env::temp_dir().join(format!("wut-noisy-provider-{}", std::process::id()));
    let project = root.join("project");
    let cursor = root.join("cursor-agent");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&project).unwrap();
    executable(
        &cursor,
        concat!(
            "#!/bin/sh\n",
            "i=0\n",
            "while [ \"$i\" -lt 5000 ]; do\n",
            "  printf 'MCP authentication failure %s: missing or invalid token\\n' \"$i\" >&2\n",
            "  i=$((i + 1))\n",
            "done\n",
            "printf 'FINAL_AUTH_DIAGNOSTIC: authenticate Cursor\\n' >&2\n",
            "exit 1\n"
        ),
    );

    let output = wut()
        .args(["what", "is", "kubernetes"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("WUT_CONFIG", root.join("config.json"))
        .env("WUT_STATE_DIR", root.join("state"))
        .env("WUT_CURSOR_BIN", &cursor)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Cursor failed"));
    assert!(stderr.contains("provider stderr truncated"));
    assert!(stderr.contains("FINAL_AUTH_DIAGNOSTIC"));
    assert!(
        stderr.len() < 1_500,
        "provider failure was {} bytes",
        stderr.len()
    );
    assert!(
        !root.join("state/sessions").exists(),
        "failed provider created session state"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn continuation_never_reopens_an_ask_session_implicitly() {
    let root = std::env::temp_dir().join(format!("wut-stale-session-{}", std::process::id()));
    let state_home = root.join("state");
    let project = root.join("project");
    let legacy_sessions = state_home.join("ask/sessions");
    let cursor = root.join("cursor-agent");
    let cursor_ran = root.join("cursor-ran");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&legacy_sessions).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let project = std::fs::canonicalize(project).unwrap();
    std::fs::write(
        legacy_sessions.join("old.json"),
        format!(
            r#"{{
              "agent": "cursor",
              "harness_session_id": "old-native-session",
              "cwd": "{}",
              "updated_at": 42,
              "settings": null,
              "turns": [{{"user": "old", "assistant": "old answer"}}]
            }}"#,
            project.display()
        ),
    )
    .unwrap();
    executable(
        &cursor,
        "#!/bin/sh\nprintf ran > \"$WUT_TEST_CURSOR_RAN\"\nexit 1\n",
    );

    let output = wut()
        .args(["-c", "next"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("XDG_STATE_HOME", &state_home)
        .env("WUT_CONFIG", root.join("config.json"))
        .env("WUT_CURSOR_BIN", &cursor)
        .env("WUT_TEST_CURSOR_RAN", &cursor_ran)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no saved wut sessions for this directory"));
    assert!(!cursor_ran.exists(), "wut resumed an ask session");
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn legacy_ask_environment_cannot_redirect_wut() {
    let root = std::env::temp_dir().join(format!("wut-stale-env-{}", std::process::id()));
    let project = root.join("project");
    let bin = root.join("bin");
    let cursor = bin.join("cursor-agent");
    let legacy_cursor = root.join("legacy-cursor");
    let legacy_ran = root.join("legacy-ran");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    executable(
        &cursor,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' '{\"type\":\"assistant\",\"timestamp_ms\":1,",
            "\"message\":{\"content\":[{\"type\":\"text\",",
            "\"text\":\"independent\"}]}}'\n",
            "printf '%s\\n' '{\"type\":\"result\",\"session_id\":\"cursor-native\",",
            "\"is_error\":false}'\n"
        ),
    );
    executable(
        &legacy_cursor,
        "#!/bin/sh\nprintf ran > \"$WUT_TEST_LEGACY_RAN\"\nexit 1\n",
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = wut()
        .arg("question")
        .current_dir(&project)
        .env("HOME", &root)
        .env("PATH", path)
        .env("WUT_CONFIG", root.join("config.json"))
        .env("WUT_STATE_DIR", root.join("state"))
        .env_remove("WUT_CURSOR_BIN")
        .env("ASK_CURSOR_BIN", &legacy_cursor)
        .env("WUT_TEST_LEGACY_RAN", &legacy_ran)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "independent\n");
    assert!(!legacy_ran.exists(), "ASK_CURSOR_BIN redirected wut");

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn plain_session_reuses_the_native_provider_session() {
    let root = std::env::temp_dir().join(format!("wut-plain-session-{}", std::process::id()));
    let project = root.join("project");
    let cursor = root.join("cursor-agent");
    let cursor_args = root.join("cursor-args");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&project).unwrap();
    executable(
        &cursor,
        concat!(
            "#!/bin/sh\n",
            "printf 'BEGIN\\n' >> \"$WUT_TEST_CURSOR_ARGS\"\n",
            "printf '%s\\n' \"$@\" >> \"$WUT_TEST_CURSOR_ARGS\"\n",
            "printf '%s\\n' '{\"type\":\"assistant\",\"timestamp_ms\":1,",
            "\"message\":{\"content\":[{\"type\":\"text\",",
            "\"text\":\"answer\"}]}}'\n",
            "printf '%s\\n' '{\"type\":\"result\",\"session_id\":\"cursor-native\",",
            "\"is_error\":false}'\n"
        ),
    );

    let mut child = wut()
        .current_dir(&project)
        .env("HOME", &root)
        .env("WUT_CONFIG", root.join("config.json"))
        .env("WUT_STATE_DIR", root.join("state"))
        .env("WUT_CURSOR_BIN", &cursor)
        .env("WUT_TEST_CURSOR_ARGS", &cursor_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"first question\nsecond question\n/quit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "answer\nanswer\n"
    );
    let args = std::fs::read_to_string(cursor_args).unwrap();
    assert_eq!(args.matches("BEGIN\n").count(), 2);
    assert!(args.contains("first question"));
    assert!(args.contains("second question"));
    assert!(args.contains("--resume"));
    assert!(args.contains("cursor-native"));
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn bare_relative_wut_config_path_is_writable() {
    let root = std::env::temp_dir().join(format!("wut-relative-config-{}", std::process::id()));
    let project = root.join("project");
    let config = project.join("wut.json");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&project).unwrap();
    let output = wut()
        .args(["config", "set", "agent", "cursor"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("WUT_CONFIG", "wut.json")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(config.exists());
    assert_eq!(
        std::fs::metadata(config).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn nested_relative_wut_config_path_is_writable() {
    let root =
        std::env::temp_dir().join(format!("wut-nested-relative-config-{}", std::process::id()));
    let project = root.join("project");
    let config = project.join("config/wut.json");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&project).unwrap();
    let output = wut()
        .args(["config", "set", "agent", "cursor"])
        .current_dir(&project)
        .env("HOME", &root)
        .env("WUT_CONFIG", "config/wut.json")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::metadata(config.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(config).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn model_listing_failure_is_concise_when_the_provider_is_noisy() {
    let root = std::env::temp_dir().join(format!("wut-noisy-models-{}", std::process::id()));
    let fake_cursor = root.join("cursor-agent");

    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    executable(
        &fake_cursor,
        r#"#!/bin/sh
i=0
while [ "$i" -lt 200 ]; do
  printf 'MODEL_DIAGNOSTIC_%03d_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n' "$i" >&2
  i=$((i + 1))
done
exit 9
"#,
    );

    let output = wut()
        .args(["models", "cursor"])
        .env("HOME", &root)
        .env("WUT_CONFIG", root.join("missing.json"))
        .env("WUT_CURSOR_BIN", &fake_cursor)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Cursor failed"));
    assert!(stderr.contains("provider stderr truncated"));
    assert!(
        stderr.len() < 1_500,
        "model-list failure was {} bytes",
        stderr.len()
    );

    let _ = std::fs::remove_dir_all(root);
}

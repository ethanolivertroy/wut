#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

// Each fixture writes fake agent scripts that wut then executes. Running
// fixtures on parallel test threads races those script writes against
// fork/exec in sibling tests, which intermittently breaks the spawned
// agents (ETXTBSY on Linux, transient early exits elsewhere). Holding a
// process-wide lock for the fixture's lifetime keeps every subprocess
// test serial; the whole suite still finishes in well under a second.
static SERIAL: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    cursor: PathBuf,
    codex: PathBuf,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("wut-product-test-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let cursor = root.join("fake-cursor");
        let codex = root.join("fake-codex");
        Self {
            root,
            cursor,
            codex,
            _serial: serial,
        }
    }

    fn write_cursor(&self, body: &str) {
        fs::write(&self.cursor, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&self.cursor, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn write_codex(&self) {
        fs::write(&self.codex, successful_codex()).unwrap();
        fs::set_permissions(&self.codex, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn configure_cursor(&self) {
        write(
            self.root.join("config/wut/config.json"),
            r#"{
  "version": 2,
  "agent": "cursor",
  "instructions": "concise",
  "agents": {"cursor": {"model": null, "reasoning": null}}
}"#,
        );
    }

    fn write_gh_release(&self, tag: &str) {
        let gh = self.root.join("gh");
        fs::write(
            &gh,
            format!(
                "#!/bin/sh\nset -eu\n[ \"$1\" = api ]\nprintf '%s\\n' '{{\"tag_name\":\"{tag}\"}}'\n"
            ),
        )
        .unwrap();
        fs::set_permissions(gh, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wut"));
        command
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin",
                    self.root.display()
                ),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("WUT_CODEX_BIN", &self.codex)
            .env("WUT_CURSOR_BIN", &self.cursor)
            .env("WUT_NO_UPDATE_CHECK", "1");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn successful_codex() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import os
import sys

def emit(message):
    print(json.dumps(message, separators=(",", ":")), flush=True)

for line in sys.stdin:
    request = json.loads(line)
    log = os.environ.get("WUT_TEST_MESSAGES")
    if log:
        with open(log, "a", encoding="utf-8") as output:
            output.write(json.dumps(request, separators=(",", ":")) + "\n")

    method = request.get("method")
    request_id = request.get("id")
    if method == "initialize":
        emit({"id": request_id, "result": {}})
    elif method == "model/list":
        emit({
            "id": request_id,
            "result": {
                "data": [{
                    "model": "gpt-5.3-codex-spark",
                    "displayName": "GPT-5.3 Codex Spark",
                    "description": "Fast",
                    "isDefault": False,
                    "supportedReasoningEfforts": [],
                    "defaultReasoningEffort": "low"
                }],
                "nextCursor": None
            }
        })
    elif method in ("thread/start", "thread/resume"):
        thread_id = request.get("params", {}).get("threadId", "codex-thread-1")
        emit({"id": request_id, "result": {"thread": {"id": thread_id}}})
    elif method == "turn/start":
        emit({"id": request_id, "result": {}})
        if os.environ.get("WUT_TEST_CODEX_MODE") == "fail":
            emit({"method": "error", "params": {"error": {"message": "rate limited"}}})
            emit({"method": "turn/completed", "params": {"turn": {"status": "failed"}}})
        else:
            emit({"method": "item/agentMessage/delta", "params": {"delta": "hello"}})
            emit({"method": "item/completed", "params": {"item": {"type": "agentMessage", "text": "hello"}}})
            emit({"method": "turn/completed", "params": {"turn": {"status": "completed"}}})
"#
}

fn successful_cursor() -> &'static str {
    r#"
printf '%s\n' "$@" > "$WUT_TEST_ARGS"
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]},"session_id":"cursor-session-1","timestamp_ms":1}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"hello","session_id":"cursor-session-1"}'
"#
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn cli_identity_is_only_wut() {
    let fixture = Fixture::new();
    let help = fixture.run(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("  wut [QUESTION...]"));
    assert!(!help.contains("  ask [QUESTION...]"));

    let error = fixture.run(&["--definitely-invalid"]);
    assert!(!error.status.success());
    assert!(
        String::from_utf8(error.stderr)
            .unwrap()
            .starts_with("wut: ")
    );

    let version = fixture.run(&["--version"]);
    assert!(version.status.success());
    assert_eq!(String::from_utf8(version.stdout).unwrap(), "0.3.0\n");
}

#[test]
fn empty_home_never_resolves_private_paths_relative_to_the_project() {
    let fixture = Fixture::new();
    let project = fixture.root.join("project");
    fs::create_dir(&project).unwrap();
    let output = fixture
        .command()
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .env("HOME", "")
        .current_dir(&project)
        .arg("hello")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HOME is not set"));
    assert!(!project.join(".config").exists());
    assert!(!project.join(".local").exists());
}

#[test]
fn empty_home_never_resolves_session_state_relative_to_the_project() {
    let fixture = Fixture::new();
    fixture.write_codex();
    let project = fixture.root.join("project");
    fs::create_dir(&project).unwrap();
    let output = fixture
        .command()
        .env_remove("XDG_STATE_HOME")
        .env("HOME", "")
        .current_dir(&project)
        .arg("hello")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HOME is not set"));
    assert!(!project.join(".local").exists());
}

#[test]
fn upgrade_discovers_private_release_through_gh() {
    let fixture = Fixture::new();
    fixture.write_gh_release("v0.3.0");
    let output = fixture.run(&["--upgrade"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "wut is already up to date (v0.3.0)\n"
    );
}

#[test]
fn update_worker_persists_private_release_metadata_through_gh() {
    let fixture = Fixture::new();
    fixture.write_gh_release("v999.0.0");
    let output = fixture.run(&["--internal-update-check"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cache = fixture.root.join("cache/wut/update.json");
    assert!(fs::read_to_string(&cache).unwrap().contains("999.0.0"));
    assert_eq!(
        fs::metadata(cache.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(cache).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn fresh_run_uses_codex_spark_and_private_canonical_state() {
    let fixture = Fixture::new();
    fixture.write_codex();
    let messages_path = fixture.root.join("provider-messages");
    let output = fixture
        .command()
        .env("WUT_TEST_MESSAGES", &messages_path)
        .args(["what", "is", "this?"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "hello\n");
    let messages = fs::read_to_string(messages_path).unwrap();
    assert!(messages.contains(r#""name":"wut""#));
    assert!(messages.contains(r#""sandbox":"read-only""#));
    assert!(messages.contains(r#""model":"gpt-5.3-codex-spark""#));
    assert!(messages.contains(r#""text":"what is this?""#));

    let session_dir = fixture.root.join("state/wut/sessions");
    let sessions = fs::read_dir(&session_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        fs::metadata(&session_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        sessions[0].metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn failed_turn_does_not_create_session_state() {
    let fixture = Fixture::new();
    fixture.write_codex();
    let output = fixture
        .command()
        .env("WUT_TEST_CODEX_MODE", "fail")
        .arg("fail")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.root.join("state/wut/sessions").exists());
}

#[test]
fn provider_diagnostics_are_bounded_and_keep_the_final_error() {
    let fixture = Fixture::new();
    fixture.configure_cursor();
    fixture.write_cursor(
        r#"
i=0
while [ "$i" -lt 5000 ]; do
  printf 'diagnostic-%04d-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n' "$i" >&2
  i=$((i + 1))
done
printf 'FINAL_AUTH_DIAGNOSTIC\n' >&2
exit 1
"#,
    );
    let output = fixture.run(&["fail"]);
    assert!(!output.status.success());
    assert!(output.stderr.len() <= 1536, "{} bytes", output.stderr.len());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("provider stderr truncated"));
    assert!(stderr.contains("FINAL_AUTH_DIAGNOSTIC"));
}

#[test]
fn prompt_accepts_dash_prefixed_words_after_the_question_starts() {
    let fixture = Fixture::new();
    fixture.write_codex();
    let messages_path = fixture.root.join("provider-messages");
    let output = fixture
        .command()
        .env("WUT_TEST_MESSAGES", &messages_path)
        .args(["compare", "-O2", "and", "-O3"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(messages_path)
            .unwrap()
            .contains(r#""text":"compare -O2 and -O3""#)
    );
}

#[test]
fn canonical_environment_wins_over_legacy_environment() {
    let fixture = Fixture::new();
    fixture.write_codex();
    let output = fixture
        .command()
        .env("ASK_CODEX_BIN", "/definitely/not/the-selected-provider")
        .arg("hello")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn explicit_wut_paths_override_xdg_locations() {
    let fixture = Fixture::new();
    fixture.write_cursor(successful_cursor());
    let config = fixture.root.join("explicit/config.json");
    let state_root = fixture.root.join("explicit/state");
    write(
        &config,
        r#"{
  "version": 2,
  "agent": "cursor",
  "instructions": "concise",
  "agents": {"cursor": {"model": null, "reasoning": null}}
}"#,
    );
    write(fixture.root.join("config/wut/config.json"), "not-json");
    let output = fixture
        .command()
        .env("WUT_CONFIG", &config)
        .env("WUT_STATE_DIR", &state_root)
        .env("WUT_TEST_ARGS", fixture.root.join("provider-args"))
        .arg("hello")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_dir(state_root.join("sessions")).unwrap().count(),
        1
    );
}

#[test]
fn legacy_provider_environment_is_a_read_only_fallback() {
    let fixture = Fixture::new();
    fixture.write_codex();
    let output = fixture
        .command()
        .env_remove("WUT_CODEX_BIN")
        .env("ASK_CODEX_BIN", &fixture.codex)
        .arg("hello")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn legacy_ask_config_and_sessions_migrate_to_wut_paths() {
    let fixture = Fixture::new();
    fixture.write_cursor(successful_cursor());
    write(
        fixture.root.join("config/ask/config.json"),
        r#"{
  "version": 2,
  "agent": "cursor",
  "instructions": "concise",
  "agents": {"cursor": {"model": null, "reasoning": null}}
}"#,
    );
    write(
        fixture.root.join("state/ask/sessions/legacy.json"),
        &format!(
            r#"{{
  "version": 2,
  "agent": "cursor",
  "harness_session_id": "legacy-native",
  "cwd": "{}",
  "updated_at": 42,
  "settings": {{"model": null, "reasoning": null}},
  "turns": [{{"user": "legacy question", "assistant": "legacy answer"}}]
}}"#,
            fixture.root.display()
        ),
    );

    let args_path = fixture.root.join("provider-args");
    let turn = fixture
        .command()
        .current_dir(&fixture.root)
        .env("WUT_TEST_ARGS", args_path)
        .arg("new question")
        .output()
        .unwrap();
    assert!(
        turn.status.success(),
        "{}",
        String::from_utf8_lossy(&turn.stderr)
    );

    let output = fixture
        .command()
        .current_dir(&fixture.root)
        .arg("--sessions")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout).unwrap().contains("1 turn"));
    assert!(fixture.root.join("config/wut/config.json").exists());
    assert!(fixture.root.join("state/wut/sessions").exists());
    assert!(fixture.root.join("config/ask/config.json").exists());
    assert!(fixture.root.join("state/ask/sessions/legacy.json").exists());
}

#[test]
fn previous_wut_v02_config_and_sessions_still_load() {
    let fixture = Fixture::new();
    write(
        fixture.root.join("config/wut/config.json"),
        r#"{
  "version": 1,
  "agent": "cursor",
  "instructions": {"custom": "Use evidence."},
  "agents": {"cursor": {"model": null, "reasoning": null}}
}"#,
    );
    write(
        fixture.root.join("state/wut/sessions/cursor-deadbeef.json"),
        &format!(
            r#"{{
  "version": 1,
  "id": "cursor-deadbeef",
  "agent": "cursor",
  "native_session_id": "native-v02",
  "cwd": "{}",
  "updated_at": 42,
  "settings": {{"model": null, "reasoning": null}},
  "turns": [{{"user": "why?", "assistant": "because"}}]
}}"#,
            fixture.root.display()
        ),
    );

    let output = fixture
        .command()
        .current_dir(&fixture.root)
        .arg("--sessions")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout).unwrap().contains("1 turn"));
}

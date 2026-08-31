use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::storage;

#[derive(Debug, Eq, PartialEq)]
pub struct Session {
    pub agent: String,
    pub harness_session_id: String,
    pub cwd: String,
    pub updated_at: u64,
    pub settings: Option<SessionSettings>,
    pub turns: Vec<Turn>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSettings {
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Turn {
    pub user: String,
    pub assistant: String,
}

impl Session {
    pub fn new(agent: &str, harness_session_id: String, cwd: &Path) -> Self {
        Self {
            agent: agent.to_owned(),
            harness_session_id,
            cwd: cwd.to_string_lossy().into_owned(),
            updated_at: now(),
            settings: None,
            turns: Vec::new(),
        }
    }

    pub fn add_turn(&mut self, user: &str, assistant: String) {
        self.turns.push(Turn {
            user: user.to_owned(),
            assistant,
        });
        self.updated_at = now();
    }

    fn to_json(&self) -> Value {
        json!({
            "version": 2,
            "agent": self.agent,
            "harness_session_id": self.harness_session_id,
            "cwd": self.cwd,
            "updated_at": self.updated_at,
            "settings": self.settings.as_ref().map(|settings| json!({
                "model": settings.model,
                "reasoning": settings.reasoning,
            })),
            "turns": self.turns.iter().map(|turn| json!({
                "user": turn.user,
                "assistant": turn.assistant,
            })).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Result<Self> {
        let version = value.get("version").and_then(Value::as_u64).unwrap_or(1);
        if version > 2 {
            return Err(Error::new(
                format!("session version {version} requires a newer version of wut"),
                "run 'wut --upgrade'",
            ));
        }
        if version == 0 {
            return Err(invalid_session("session version 0 is not supported"));
        }
        let string = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid_session(format!("missing or invalid '{key}'")))
        };
        let turns = value["turns"]
            .as_array()
            .ok_or_else(|| invalid_session("missing or invalid 'turns'"))?
            .iter()
            .map(|turn| {
                Ok(Turn {
                    user: turn["user"]
                        .as_str()
                        .ok_or_else(|| invalid_session("turn is missing 'user'"))?
                        .to_owned(),
                    assistant: turn["assistant"]
                        .as_str()
                        .ok_or_else(|| invalid_session("turn is missing 'assistant'"))?
                        .to_owned(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let harness_session_id = value
            .get("harness_session_id")
            .or_else(|| value.get("native_session_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_session("missing or invalid 'harness_session_id'"))?
            .to_owned();
        let settings = value
            .get("settings")
            .filter(|settings| !settings.is_null())
            .map(|settings| {
                if !settings.is_object() {
                    return Err(invalid_session("missing or invalid 'settings'"));
                }
                Ok(SessionSettings {
                    model: optional_string(settings, "model")?,
                    reasoning: optional_string(settings, "reasoning")?,
                })
            })
            .transpose()?;

        Ok(Self {
            agent: string("agent")?,
            harness_session_id,
            cwd: string("cwd")?,
            updated_at: value["updated_at"]
                .as_u64()
                .ok_or_else(|| invalid_session("missing or invalid 'updated_at'"))?,
            settings,
            turns,
        })
    }
}

pub fn save(session: &Session) -> Result<()> {
    let directory = directory()?;
    save_to(&directory, session)
}

fn save_to(directory: &Path, session: &Session) -> Result<()> {
    let destination = session_path(directory, session)?;
    let bytes = session_bytes(session)?;
    storage::write_private(&destination, &bytes, "session")
}

fn import_session(directory: &Path, session: &Session) -> Result<bool> {
    let destination = session_path(directory, session)?;
    let bytes = session_bytes(session)?;
    storage::write_private_if_absent(&destination, &bytes, "session")
}

fn session_path(directory: &Path, session: &Session) -> Result<PathBuf> {
    Ok(directory.join(format!(
        "{}.json",
        file_key(&session.agent, &session.harness_session_id)?
    )))
}

fn session_bytes(session: &Session) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(&session.to_json())
        .map_err(|error| Error::internal(format!("could not encode session: {error}")))
}

pub fn delete(session: &Session) -> Result<()> {
    delete_from(&directory()?, session)
}

fn delete_from(directory: &Path, session: &Session) -> Result<()> {
    let path = directory.join(format!(
        "{}.json",
        file_key(&session.agent, &session.harness_session_id)?
    ));
    fs::remove_file(&path).map_err(|error| {
        Error::new(
            format!("could not delete session '{}': {error}", path.display()),
            "check its permissions and try again",
        )
    })
}

pub fn latest(cwd: &Path) -> Result<Session> {
    let (directory, legacy) = directories()?;
    migrate_legacy(&directory, legacy.as_deref())?;
    latest_from(&directory, &cwd.to_string_lossy())?.ok_or_else(|| {
        Error::new(
            "no saved sessions for this folder",
            "start one by running 'wut'",
        )
    })
}

fn latest_from(directory: &Path, cwd: &str) -> Result<Option<Session>> {
    if !directory.exists() {
        return Ok(None);
    }

    // Sessions are rewritten atomically on every save, so modification time
    // follows updated_at; walking files newest-first finds the latest match
    // without parsing every saved session.
    let mut candidates = Vec::new();
    for entry in session_entries(directory)? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        candidates.push((modified, path));
    }
    candidates.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));

    for (_, path) in candidates {
        let session = load_session(&path)?;
        if session.cwd == cwd {
            return Ok(Some(session));
        }
    }
    Ok(None)
}

pub fn load_all() -> Result<Vec<Session>> {
    let (directory, legacy) = directories()?;
    migrate_legacy(&directory, legacy.as_deref())?;
    load_from(&directory)
}

fn load_from(directory: &Path) -> Result<Vec<Session>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for entry in session_entries(directory)? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        sessions.push(load_session(&path)?);
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

fn session_entries(directory: &Path) -> Result<Vec<fs::DirEntry>> {
    let entries = fs::read_dir(directory).map_err(|error| {
        Error::new(
            format!(
                "could not read session directory '{}': {error}",
                directory.display()
            ),
            "check its permissions and try again",
        )
    })?;
    entries
        .map(|entry| {
            entry.map_err(|error| {
                Error::new(
                    format!("could not read a session entry: {error}"),
                    "check the session directory permissions and try again",
                )
            })
        })
        .collect()
}

fn load_session(path: &Path) -> Result<Session> {
    let bytes = fs::read(path).map_err(|error| {
        Error::new(
            format!("could not read '{}': {error}", path.display()),
            "check its permissions and try again",
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        Error::new(
            format!("could not parse '{}': {error}", path.display()),
            "remove this file and try again",
        )
    })?;
    Session::from_json(&value)
        .map_err(|error| error.context(format!("invalid session '{}'", path.display())))
}

fn load_legacy_from(directory: &Path) -> Result<Vec<Session>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        Error::new(
            format!(
                "could not read legacy session directory '{}': {error}",
                directory.display()
            ),
            "check its permissions and try again",
        )
    })?;
    let mut sessions = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let bytes = fs::read(path).ok()?;
            let value: Value = serde_json::from_slice(&bytes).ok()?;
            Session::from_json(&value).ok()
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

pub fn directory() -> Result<PathBuf> {
    Ok(directories()?.0)
}

fn directories() -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(path) = std::env::var_os("WUT_STATE_DIR").filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(path).join("sessions"), None));
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        let root = PathBuf::from(path);
        return Ok((root.join("wut/sessions"), Some(root.join("ask/sessions"))));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::new(
                "HOME is not set",
                "set XDG_STATE_HOME to a writable directory and try again",
            )
        })?;
    let root = PathBuf::from(home).join(".local/state");
    Ok((root.join("wut/sessions"), Some(root.join("ask/sessions"))))
}

fn migrate_legacy(canonical: &Path, legacy: Option<&Path>) -> Result<()> {
    let Some(legacy) = legacy.filter(|path| path.exists()) else {
        return Ok(());
    };
    let marker = canonical.join(".legacy-imported");
    if marker.exists() {
        return Ok(());
    }
    for session in load_legacy_from(legacy)? {
        import_session(canonical, &session)?;
    }
    storage::write_private(&marker, b"1\n", "session migration marker")
}

fn file_key(agent: &str, harness_session_id: &str) -> Result<String> {
    if agent.is_empty()
        || agent.len() > 64
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid_session("missing or invalid agent id"));
    }
    if harness_session_id.is_empty() || harness_session_id.len() > 4_096 {
        return Err(invalid_session("missing or invalid provider session id"));
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in harness_session_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("{agent}-{hash:016x}"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_session(format!(
            "missing or invalid 'settings.{key}'"
        ))),
    }
}

fn invalid_session(message: impl Into<String>) -> Error {
    Error::new(message, "remove the invalid session file and try again")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{
        Session, SessionSettings, Turn, delete_from, file_key, import_session, latest_from,
        load_from, migrate_legacy, save_to,
    };

    #[test]
    fn malformed_legacy_session_does_not_block_valid_canonical_state() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("wut-legacy-malformed-{unique}"));
        let canonical_directory = root.join("wut/sessions");
        let legacy_directory = root.join("ask/sessions");

        let cwd = std::env::temp_dir();
        let mut canonical = Session::new("codex", "canonical-id".into(), &cwd);
        canonical.add_turn("canonical", "keep me".into());
        save_to(&canonical_directory, &canonical).unwrap();

        let mut legacy = Session::new("cursor", "legacy-id".into(), &cwd);
        legacy.add_turn("legacy", "import me".into());
        save_to(&legacy_directory, &legacy).unwrap();
        fs::write(legacy_directory.join("malformed.json"), b"not-json").unwrap();

        migrate_legacy(&canonical_directory, Some(&legacy_directory)).unwrap();

        let loaded = load_from(&canonical_directory).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().any(|session| session.agent == "codex"));
        assert!(loaded.iter().any(|session| session.agent == "cursor"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_import_never_replaces_an_existing_canonical_session() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "wut-session-no-clobber-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();

        let mut canonical = Session::new("codex", "shared-native-id".into(), &directory);
        canonical.add_turn("canonical", "keep me".into());
        save_to(&directory, &canonical).unwrap();
        let mut legacy = Session::new("codex", "shared-native-id".into(), &directory);
        legacy.add_turn("legacy", "do not import".into());

        assert!(!import_session(&directory, &legacy).unwrap());
        let loaded = load_from(&directory).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].turns[0].user, "canonical");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn same_native_id_from_different_agents_keeps_both_sessions() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "wut-agent-session-collision-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();

        let codex = Session::new("codex", "shared-native-id".into(), &directory);
        let cursor = Session::new("cursor", "shared-native-id".into(), &directory);
        save_to(&directory, &codex).unwrap();
        save_to(&directory, &cursor).unwrap();

        let sessions = load_from(&directory).unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|session| session.agent == "codex"));
        assert!(sessions.iter().any(|session| session.agent == "cursor"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn agent_ids_cannot_escape_the_session_directory() {
        assert!(file_key("../codex", "native-id").is_err());
        assert!(file_key("codex/slash", "native-id").is_err());
    }

    #[test]
    fn deleting_a_session_removes_only_its_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("wut-delete-test-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();

        let deleted = Session::new("codex", "delete-me".into(), &directory);
        let kept = Session::new("codex", "keep-me".into(), &directory);
        let deleted_path = directory.join(format!(
            "{}.json",
            file_key(&deleted.agent, "delete-me").unwrap()
        ));
        let kept_path = directory.join(format!(
            "{}.json",
            file_key(&kept.agent, &kept.harness_session_id).unwrap()
        ));
        fs::write(&deleted_path, b"deleted").unwrap();
        fs::write(&kept_path, b"kept").unwrap();

        delete_from(&directory, &deleted).unwrap();

        assert!(!deleted_path.exists());
        assert!(kept_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn latest_prefers_the_newest_matching_session_without_older_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("wut-latest-test-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let cwd_a = Path::new("/tmp/wut-project-a");
        let cwd_b = Path::new("/tmp/wut-project-b");

        // Oldest file is malformed: the lazy walk must find newer matches
        // before ever reading it.
        fs::write(directory.join("malformed.json"), b"not-json").unwrap();
        thread::sleep(Duration::from_millis(20));
        save_to(&directory, &Session::new("codex", "a-old".into(), cwd_a)).unwrap();
        thread::sleep(Duration::from_millis(20));
        save_to(&directory, &Session::new("codex", "b-only".into(), cwd_b)).unwrap();
        thread::sleep(Duration::from_millis(20));
        save_to(&directory, &Session::new("codex", "a-new".into(), cwd_a)).unwrap();

        let found_a = latest_from(&directory, &cwd_a.to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(found_a.harness_session_id, "a-new");

        let found_b = latest_from(&directory, &cwd_b.to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(found_b.harness_session_id, "b-only");

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn latest_without_matching_sessions_is_none() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "wut-latest-none-test-{}-{unique}",
            std::process::id()
        ));

        assert_eq!(latest_from(&directory, "/tmp/missing").unwrap(), None);

        fs::create_dir(&directory).unwrap();
        save_to(
            &directory,
            &Session::new("codex", "other".into(), Path::new("/tmp/wut-other")),
        )
        .unwrap();
        assert_eq!(latest_from(&directory, "/tmp/missing").unwrap(), None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn session_round_trips_through_json() {
        let session = Session {
            agent: "codex".into(),
            harness_session_id: "full-id".into(),
            cwd: "/tmp/project".into(),
            updated_at: 42,
            settings: Some(SessionSettings {
                model: Some("gpt-test".into()),
                reasoning: Some("high".into()),
            }),
            turns: vec![Turn {
                user: "hello".into(),
                assistant: "hi".into(),
            }],
        };

        assert_eq!(Session::from_json(&session.to_json()).unwrap(), session);
    }
}

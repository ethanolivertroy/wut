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

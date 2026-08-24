use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::store;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Turn {
    pub user: String,
    pub assistant: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Session {
    pub version: u8,
    pub id: String,
    pub agent: String,
    pub native_session_id: String,
    pub cwd: String,
    pub updated_at: u64,
    pub settings: Settings,
    pub turns: Vec<Turn>,
}

impl Session {
    pub fn new(agent: &str, native_session_id: String, cwd: &Path, settings: Settings) -> Self {
        Self {
            version: 1,
            id: local_id(agent, &native_session_id),
            agent: agent.to_owned(),
            native_session_id,
            cwd: cwd.to_string_lossy().into_owned(),
            updated_at: now(),
            settings,
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
}

pub fn save(session: &Session) -> Result<()> {
    validate_local_id(&session.id)?;
    let path = store::session_dir()?.join(format!("{}.json", session.id));
    store::write_json(&path, session, "wut session")
}

pub fn load_all() -> Result<Vec<Session>> {
    let current = load_dir(&store::session_dir()?, false)?;
    let mut sessions = if current.is_empty() {
        load_dir(&store::legacy_session_dir()?, true)?
    } else {
        current
    };
    let mut seen = HashSet::new();
    sessions
        .retain(|session| seen.insert((session.agent.clone(), session.native_session_id.clone())));
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

pub fn latest(cwd: &Path) -> Result<Session> {
    let cwd = cwd.to_string_lossy();
    load_all()?
        .into_iter()
        .filter(|session| session.cwd == cwd)
        .max_by_key(|session| session.updated_at)
        .ok_or_else(|| {
            Error::new("no saved wut sessions for this directory")
                .hint("start one with 'wut QUESTION' or list all with 'wut sessions'")
        })
}

pub fn find(id: &str) -> Result<Session> {
    load_all()?
        .into_iter()
        .find(|session| session.id == id)
        .ok_or_else(|| Error::new(format!("unknown session '{id}'")).hint("run 'wut sessions'"))
}

fn load_dir(directory: &Path, legacy: bool) -> Result<Vec<Session>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        Error::new(format!("could not read '{}': {error}", directory.display()))
    })?;
    let mut sessions = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Error::new(format!("could not read a session entry: {error}")))?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path)
            .map_err(|error| Error::new(format!("could not read '{}': {error}", path.display())))?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            Error::new(format!("could not parse '{}': {error}", path.display()))
        })?;
        let session = if legacy || value.get("native_session_id").is_none() {
            parse_legacy(&value)
        } else {
            serde_json::from_value::<Session>(value).map_err(|error| {
                Error::new(format!("invalid session '{}': {error}", path.display()))
            })
        }?;
        validate_local_id(&session.id)
            .map_err(|error| error.context(format!("invalid session '{}'", path.display())))?;
        sessions.push(session);
    }
    Ok(sessions)
}

fn parse_legacy(value: &Value) -> Result<Session> {
    let string = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::new(format!("legacy session is missing '{field}'")))
    };
    let agent = string("agent")?;
    let native_session_id = string("harness_session_id")?;
    let settings = value
        .get("settings")
        .filter(|value| !value.is_null())
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| Error::new(format!("legacy session has invalid settings: {error}")))?
        .unwrap_or_default();
    let turns = serde_json::from_value(value.get("turns").cloned().unwrap_or_default())
        .map_err(|error| Error::new(format!("legacy session has invalid turns: {error}")))?;
    Ok(Session {
        version: 1,
        id: local_id(&agent, &native_session_id),
        agent,
        native_session_id,
        cwd: string("cwd")?,
        updated_at: value
            .get("updated_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::new("legacy session is missing 'updated_at'"))?,
        settings,
        turns,
    })
}

fn local_id(agent: &str, native_session_id: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in native_session_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{agent}-{hash:016x}")
}

fn validate_local_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::new("session ID contains unsafe characters"));
    }
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn directory() -> Result<PathBuf> {
    store::session_dir()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Session, Settings, local_id, parse_legacy};

    #[test]
    fn local_ids_are_stable_and_do_not_expose_native_ids() {
        let first = local_id("cursor", "private-session-id");
        assert_eq!(first, local_id("cursor", "private-session-id"));
        assert!(first.starts_with("cursor-"));
        assert!(!first.contains("private-session-id"));
    }

    #[test]
    fn imports_ask_sessions() {
        let session = parse_legacy(&json!({
            "agent": "cursor",
            "harness_session_id": "native-1",
            "cwd": "/tmp/project",
            "updated_at": 42,
            "settings": {"model": "grok-fast", "reasoning": null},
            "turns": [{"user": "hello", "assistant": "hi"}]
        }))
        .unwrap();
        assert_eq!(session.native_session_id, "native-1");
        assert_eq!(session.settings.model.as_deref(), Some("grok-fast"));
        assert_eq!(session.turns.len(), 1);
    }

    #[test]
    fn new_sessions_capture_settings() {
        let session = Session::new(
            "grok",
            "native".into(),
            std::path::Path::new("/tmp"),
            Settings {
                model: Some("grok-4".into()),
                reasoning: Some("high".into()),
            },
        );
        assert_eq!(session.agent, "grok");
        assert_eq!(session.settings.reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn rejects_local_ids_that_could_escape_the_session_directory() {
        assert!(super::validate_local_id("cursor-deadbeef").is_ok());
        assert!(super::validate_local_id("../../config").is_err());
        assert!(super::validate_local_id("cursor/other").is_err());
        assert!(super::validate_local_id("").is_err());
    }
}

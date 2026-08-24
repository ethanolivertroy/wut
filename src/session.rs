use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::IgnoredAny};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Summary {
    pub id: String,
    pub agent: String,
    pub cwd: String,
    pub updated_at: u64,
    pub settings: Settings,
    pub turn_count: usize,
    native_session_id: String,
    path: PathBuf,
}

#[derive(Deserialize)]
struct StoredSummary {
    #[serde(default)]
    id: Option<String>,
    agent: String,
    #[serde(default)]
    native_session_id: Option<String>,
    cwd: String,
    updated_at: u64,
    #[serde(default)]
    settings: Option<Settings>,
    #[serde(default)]
    turns: Vec<IgnoredAny>,
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
    let mut sessions = load_dir(&store::session_dir()?)?;
    let mut seen = HashSet::new();
    sessions
        .retain(|session| seen.insert((session.agent.clone(), session.native_session_id.clone())));
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

pub fn load_summaries() -> Result<Vec<Summary>> {
    let mut sessions = load_summary_dir(&store::session_dir()?)?;
    let mut seen = HashSet::new();
    sessions
        .retain(|session| seen.insert((session.agent.clone(), session.native_session_id.clone())));
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    Ok(sessions)
}

pub fn latest(cwd: &Path) -> Result<Session> {
    let cwd = cwd.to_string_lossy();
    let summary = load_summaries()?
        .into_iter()
        .filter(|session| session.cwd == cwd)
        .max_by_key(|session| session.updated_at)
        .ok_or_else(|| {
            Error::new("no saved wut sessions for this directory")
                .hint("start one with 'wut QUESTION' or list all with 'wut sessions'")
        })?;
    load_path(&summary.path)
}

pub fn find(id: &str) -> Result<Session> {
    validate_local_id(id)?;
    let path = store::session_dir()?.join(format!("{id}.json"));
    if path.exists() {
        let session = load_path(&path)?;
        if session.id == id {
            return Ok(session);
        }
    }
    let summary = load_summaries()?
        .into_iter()
        .find(|session| session.id == id)
        .ok_or_else(|| Error::new(format!("unknown session '{id}'")).hint("run 'wut sessions'"))?;
    load_path(&summary.path)
}

fn load_dir(directory: &Path) -> Result<Vec<Session>> {
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
        sessions.push(load_path(&path)?);
    }
    Ok(sessions)
}

fn load_path(path: &Path) -> Result<Session> {
    let bytes = fs::read(path)
        .map_err(|error| Error::new(format!("could not read '{}': {error}", path.display())))?;
    let session: Session = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("invalid session '{}': {error}", path.display())))?;
    validate_local_id(&session.id)
        .map_err(|error| error.context(format!("invalid session '{}'", path.display())))?;
    Ok(session)
}

fn load_summary_dir(directory: &Path) -> Result<Vec<Summary>> {
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
        sessions.push(parse_summary(&bytes, path)?);
    }
    Ok(sessions)
}

fn parse_summary(bytes: &[u8], path: PathBuf) -> Result<Summary> {
    let stored: StoredSummary = serde_json::from_slice(bytes)
        .map_err(|error| Error::new(format!("invalid session '{}': {error}", path.display())))?;
    let native_session_id = stored.native_session_id.ok_or_else(|| {
        Error::new(format!(
            "invalid session '{}': missing native session ID",
            path.display()
        ))
    })?;
    let id = stored.id.ok_or_else(|| {
        Error::new(format!(
            "invalid session '{}': missing local session ID",
            path.display()
        ))
    })?;
    validate_local_id(&id)
        .map_err(|error| error.context(format!("invalid session '{}'", path.display())))?;
    Ok(Summary {
        id,
        agent: stored.agent,
        cwd: stored.cwd,
        updated_at: stored.updated_at,
        settings: stored.settings.unwrap_or_default(),
        turn_count: stored.turns.len(),
        native_session_id,
        path,
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
    use super::{Session, Settings, local_id, parse_summary};

    #[test]
    fn summaries_count_turns_without_loading_transcript_fields() {
        let summary = parse_summary(
            br#"{
                "id": "cursor-test",
                "agent": "cursor",
                "native_session_id": "native-1",
                "cwd": "/tmp/project",
                "updated_at": 42,
                "settings": {"model": "fast"},
                "turns": [
                    {"user": "secret question", "assistant": "secret answer"},
                    {"future": {"shape": "does not matter for a summary"}}
                ]
            }"#,
            std::path::PathBuf::from("/tmp/cursor-test.json"),
        )
        .unwrap();

        assert_eq!(summary.id, "cursor-test");
        assert_eq!(summary.turn_count, 2);
        assert_eq!(summary.settings.model.as_deref(), Some("fast"));
        assert_eq!(summary.native_session_id, "native-1");
    }

    #[test]
    fn loads_v01_wut_session_fixture() {
        let session: Session = serde_json::from_slice(
            br#"{
                "version": 1,
                "id": "cursor-deadbeef",
                "agent": "cursor",
                "native_session_id": "native-v01",
                "cwd": "/tmp/project",
                "updated_at": 42,
                "settings": {"model": "cursor-fast", "reasoning": null},
                "turns": [{"user": "why?", "assistant": "because"}]
            }"#,
        )
        .unwrap();

        assert_eq!(session.version, 1);
        assert_eq!(session.id, "cursor-deadbeef");
        assert_eq!(session.native_session_id, "native-v01");
        assert_eq!(session.turns.len(), 1);
    }

    #[test]
    fn local_ids_are_stable_and_do_not_expose_native_ids() {
        let first = local_id("cursor", "private-session-id");
        assert_eq!(first, local_id("cursor", "private-session-id"));
        assert!(first.starts_with("cursor-"));
        assert!(!first.contains("private-session-id"));
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

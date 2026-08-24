use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::store;

pub const DEFAULT_INSTRUCTIONS: &str =
    "Answer directly and concisely. Explain commands before suggesting them. Do not modify files.";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    pub version: u8,
    pub agent: String,
    pub instructions: Option<String>,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            agent: "cursor".into(),
            instructions: Some(DEFAULT_INSTRUCTIONS.into()),
            agents: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = path()?;
        if path.exists() {
            return load_path(&path);
        }
        Ok(Self::default())
    }

    pub fn save(&self) -> Result<()> {
        store::write_json(&path()?, self, "wut config")
    }

    pub fn settings(&self, agent: &str) -> AgentConfig {
        self.agents.get(agent).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "agent" => self.agent = required(value, "agent")?.to_owned(),
            "instructions" => {
                self.instructions = match value {
                    "none" | "default" => None,
                    "concise" => Some(DEFAULT_INSTRUCTIONS.into()),
                    value => Some(required(value, "instructions")?.to_owned()),
                };
            }
            _ => {
                let (agent, field) = key.split_once('.').ok_or_else(|| {
                    Error::usage(
                        "config keys are 'agent', 'instructions', '<agent>.model', or '<agent>.reasoning'",
                    )
                })?;
                let agent = required(agent, "agent")?;
                let setting = optional(value);
                let settings = self.agents.entry(agent.to_owned()).or_default();
                match field {
                    "model" => settings.model = setting,
                    "reasoning" => settings.reasoning = setting,
                    _ => {
                        return Err(Error::usage(format!("unknown config key '{key}'")));
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn path() -> Result<PathBuf> {
    store::config_path()
}

fn load_path(path: &PathBuf) -> Result<Config> {
    let bytes = fs::read(path)
        .map_err(|error| Error::new(format!("could not read '{}': {error}", path.display())))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("could not parse '{}': {error}", path.display())))?;
    from_value(&value)
        .map_err(|error| error.context(format!("invalid config '{}'", path.display())))
}

fn from_value(value: &Value) -> Result<Config> {
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::new("config is missing integer field 'version'"))?;
    if version != 1 {
        return Err(Error::new(format!(
            "unsupported wut config version {version}"
        )));
    }
    let mut config = Config {
        agent: value
            .get("agent")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("config is missing string field 'agent'"))?
            .to_owned(),
        instructions: parse_instructions(value.get("instructions"))?,
        ..Config::default()
    };

    if let Some(agents) = value.get("agents") {
        let agents = agents
            .as_object()
            .ok_or_else(|| Error::new("config field 'agents' must be an object"))?;
        for (id, value) in agents {
            let object = value.as_object().ok_or_else(|| {
                Error::new(format!("config field 'agents.{id}' must be an object"))
            })?;
            config.agents.insert(
                id.clone(),
                AgentConfig {
                    model: optional_string(object.get("model"), &format!("agents.{id}.model"))?,
                    reasoning: optional_string(
                        object.get("reasoning"),
                        &format!("agents.{id}.reasoning"),
                    )?,
                },
            );
        }
    }
    Ok(config)
}

fn parse_instructions(value: Option<&Value>) -> Result<Option<String>> {
    match value {
        None => Ok(Some(DEFAULT_INSTRUCTIONS.into())),
        Some(Value::Null) => Ok(None),
        Some(Value::String(mode)) if mode == "concise" => Ok(Some(DEFAULT_INSTRUCTIONS.into())),
        Some(Value::String(mode)) if mode == "agent_default" => Ok(None),
        Some(Value::String(instructions)) => Ok(Some(instructions.clone())),
        Some(Value::Object(object)) => object
            .get("custom")
            .and_then(Value::as_str)
            .map(|value| Some(value.to_owned()))
            .ok_or_else(|| {
                Error::new("config instructions object must contain string field 'custom'")
            }),
        Some(_) => Err(Error::new("config field 'instructions' is invalid")),
    }
}

fn optional_string(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(Error::new(format!(
            "config field '{field}' must be a string or null"
        ))),
    }
}

fn optional(value: &str) -> Option<String> {
    match value {
        "none" | "default" => None,
        _ => Some(value.to_owned()),
    }
}

fn required<'a>(value: &'a str, name: &str) -> Result<&'a str> {
    if value.trim().is_empty() {
        Err(Error::usage(format!("{name} must not be empty")))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Config, DEFAULT_INSTRUCTIONS, from_value};

    #[test]
    fn defaults_to_cursor_and_concise_answers() {
        let config = Config::default();
        assert_eq!(config.agent, "cursor");
        assert_eq!(config.instructions.as_deref(), Some(DEFAULT_INSTRUCTIONS));
    }

    #[test]
    fn loads_v01_wut_config_fixture() {
        let config = from_value(&json!({
            "version": 1,
            "agent": "codex",
            "instructions": {"custom": "Use evidence."},
            "agents": {
                "codex": {"model": "gpt-5", "reasoning": "high"}
            }
        }))
        .unwrap();

        assert_eq!(config.agent, "codex");
        assert_eq!(config.instructions.as_deref(), Some("Use evidence."));
        assert_eq!(config.settings("codex").model.as_deref(), Some("gpt-5"));
        assert_eq!(config.settings("codex").reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn rejects_foreign_config_versions() {
        let error = from_value(&json!({
            "version": 2,
            "agent": "cursor",
            "instructions": "concise",
            "agents": {}
        }))
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported wut config version 2")
        );
    }

    #[test]
    fn supports_scriptable_updates() {
        let mut config = Config::default();
        config.set("agent", "grok").unwrap();
        config.set("grok.model", "grok-4").unwrap();
        config.set("grok.reasoning", "high").unwrap();
        config.set("instructions", "none").unwrap();
        assert_eq!(config.agent, "grok");
        assert_eq!(config.settings("grok").model.as_deref(), Some("grok-4"));
        assert_eq!(config.settings("grok").reasoning.as_deref(), Some("high"));
        assert_eq!(config.instructions, None);
    }
}

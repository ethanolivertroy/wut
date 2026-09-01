use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::instructions::Instructions;
use crate::{harness, storage};

pub struct Config {
    pub agent: String,
    instructions: Instructions,
    agents: BTreeMap<String, AgentSettings>,
}

struct AgentSettings {
    model: Option<String>,
    reasoning: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let agents = harness::DEFINITIONS
            .iter()
            .map(|definition| {
                (
                    definition.id.to_owned(),
                    AgentSettings {
                        model: definition.default_model.map(str::to_owned),
                        reasoning: definition.default_reasoning.map(str::to_owned),
                    },
                )
            })
            .collect();
        Self {
            agent: "codex".into(),
            instructions: Instructions::default(),
            agents,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let (canonical, legacy) = paths()?;
        if canonical.exists() {
            return load_file(&canonical);
        }
        let Some(legacy) = legacy.filter(|legacy| legacy.exists()) else {
            return Ok(Self::default());
        };
        import_legacy(&canonical, &legacy)
    }

    pub fn save(&self) -> Result<()> {
        storage::write_private(&path()?, &self.bytes()?, "wut config")
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        let agents = self
            .agents
            .iter()
            .map(|(id, settings)| {
                (
                    id.clone(),
                    json!({
                        "model": settings.model,
                        "reasoning": settings.reasoning,
                    }),
                )
            })
            .collect::<Map<_, _>>();
        serde_json::to_vec_pretty(&json!({
            "version": 2,
            "agent": self.agent,
            "instructions": self.instructions.to_json(),
            "agents": agents,
        }))
        .map_err(|error| Error::internal(format!("could not encode wut config: {error}")))
    }

    pub fn model(&self, agent: &str) -> Option<&str> {
        match self.agents.get(agent) {
            Some(settings) => settings.model.as_deref(),
            None => harness::find(agent)?.default_model,
        }
    }

    pub fn set_model(&mut self, agent: &str, model: Option<String>) {
        self.agent_settings_mut(agent).model = model;
    }

    pub fn reasoning(&self, agent: &str) -> Option<&str> {
        match self.agents.get(agent) {
            Some(settings) => settings.reasoning.as_deref(),
            None => harness::find(agent)?.default_reasoning,
        }
    }

    pub fn set_reasoning(&mut self, agent: &str, reasoning: Option<String>) {
        self.agent_settings_mut(agent).reasoning = reasoning;
    }

    pub fn instructions(&self) -> &Instructions {
        &self.instructions
    }

    pub fn set_instructions(&mut self, instructions: Instructions) {
        self.instructions = instructions;
    }

    fn from_value(value: &Value) -> Result<Self> {
        let raw_agent = value["agent"]
            .as_str()
            .ok_or_else(|| invalid_config("config is missing 'agent'"))?;
        let agent = harness::find(raw_agent)
            .map_or(raw_agent, |definition| definition.id)
            .to_owned();

        let version = value.get("version").and_then(Value::as_u64).unwrap_or(1);
        if version > 2 {
            return Err(Error::new(
                format!("config version {version} requires a newer version of wut"),
                "run 'wut --upgrade'",
            ));
        }
        if version == 0 {
            return Err(Error::new(
                "config version 0 is not supported",
                "remove the config file, then run 'wut --settings'",
            ));
        }

        let values = value.get("agents").map(|values| {
            values
                .as_object()
                .ok_or_else(|| invalid_config("config has invalid 'agents'"))
        });
        let mut config = Self {
            agent,
            instructions: Instructions::from_json(value.get("instructions"), version)?,
            ..Self::default()
        };
        if let Some(values) = values.transpose()? {
            config.load_agent_settings(values)?;
        }
        Ok(config)
    }

    fn load_agent_settings(&mut self, values: &Map<String, Value>) -> Result<()> {
        for (raw_id, value) in values {
            let id = harness::find(raw_id)
                .map_or(raw_id.as_str(), |definition| definition.id)
                .to_owned();
            if !value.is_object() {
                return Err(invalid_config(format!(
                    "config has invalid 'agents.{raw_id}'"
                )));
            }
            self.agents.insert(
                id,
                AgentSettings {
                    model: optional_string(value, "model")?,
                    reasoning: optional_string(value, "reasoning")?,
                },
            );
        }
        Ok(())
    }

    fn agent_settings_mut(&mut self, agent: &str) -> &mut AgentSettings {
        self.agents.entry(agent.to_owned()).or_insert_with(|| {
            let definition = harness::find(agent);
            AgentSettings {
                model: definition
                    .and_then(|definition| definition.default_model.map(str::to_owned)),
                reasoning: definition
                    .and_then(|definition| definition.default_reasoning.map(str::to_owned)),
            }
        })
    }
}

fn load_file(path: &Path) -> Result<Config> {
    let bytes = fs::read(path).map_err(|error| {
        Error::new(
            format!("could not read '{}': {error}", path.display()),
            "check its permissions and try again",
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        Error::new(
            format!("could not parse '{}': {error}", path.display()),
            "fix or remove the file, then try again",
        )
    })?;
    Config::from_value(&value)
        .map_err(|error| error.context(format!("could not load '{}'", path.display())))
}

fn import_legacy(canonical: &Path, legacy: &Path) -> Result<Config> {
    let legacy_config = load_file(legacy)?;
    if storage::write_private_if_absent(canonical, &legacy_config.bytes()?, "wut config migration")?
    {
        Ok(legacy_config)
    } else {
        load_file(canonical)
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(invalid_config(format!("config has invalid '{key}'"))),
    }
}

fn path() -> Result<PathBuf> {
    Ok(paths()?.0)
}

fn paths() -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(path) = std::env::var_os("WUT_CONFIG").filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(path), None));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        let (canonical, legacy) = config_paths(Path::new(&path));
        return Ok((canonical, Some(legacy)));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Error::new(
                "HOME is not set",
                "set XDG_CONFIG_HOME to a writable directory and try again",
            )
        })?;
    let (canonical, legacy) = config_paths(&PathBuf::from(home).join(".config"));
    Ok((canonical, Some(legacy)))
}

fn config_paths(root: &Path) -> (PathBuf, PathBuf) {
    (root.join("wut/config.json"), root.join("ask/config.json"))
}

fn invalid_config(message: impl Into<String>) -> Error {
    Error::new(message, "fix or remove the config file, then try again")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{Config, config_paths, import_legacy};
    use crate::instructions::Instructions;

    #[test]
    fn canonical_config_path_is_wut_and_legacy_path_is_read_only_ask() {
        let (canonical, legacy) = config_paths(Path::new("/xdg"));

        assert_eq!(canonical, Path::new("/xdg/wut/config.json"));
        assert_eq!(legacy, Path::new("/xdg/ask/config.json"));
    }

    #[test]
    fn legacy_import_never_replaces_a_concurrent_canonical_config() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "wut-config-no-clobber-test-{}-{unique}",
            std::process::id()
        ));
        let canonical = root.join("wut/config.json");
        let legacy = root.join("ask/config.json");
        let canonical_bytes = br#"{"version":2,"agent":"cursor","instructions":"concise"}"#;
        fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&canonical, canonical_bytes).unwrap();
        fs::write(
            &legacy,
            br#"{"version":2,"agent":"codex","instructions":"concise"}"#,
        )
        .unwrap();

        let config = import_legacy(&canonical, &legacy).unwrap();
        assert_eq!(config.agent, "cursor");
        assert_eq!(fs::read(&canonical).unwrap(), canonical_bytes);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fresh_config_defaults_to_codex_fast() {
        let config = Config::default();
        assert_eq!(config.agent, "codex");
        assert_eq!(config.model("cursor"), None);
        assert_eq!(config.reasoning("cursor"), None);
        assert_eq!(config.model("codex"), Some("fast"));
        assert_eq!(config.reasoning("codex"), Some("low"));
        assert_eq!(config.model("cerebras"), Some("cerebras/gpt-oss-120b"));
        assert_eq!(config.reasoning("cerebras"), Some("medium"));
        assert_eq!(config.instructions(), &Instructions::Concise);
    }

    #[test]
    fn v1_without_agents_uses_provider_defaults() {
        let config = Config::from_value(&json!({
            "version": 1,
            "agent": "cursor",
            "instructions": null
        }))
        .unwrap();

        assert_eq!(config.agent, "cursor");
        assert_eq!(config.model("codex"), Some("fast"));
        assert_eq!(config.model("cursor"), None);
    }

    #[test]
    fn v1_preserves_agent_settings_and_resets_instruction_strings() {
        let config = Config::from_value(&json!({
            "version": 1,
            "agent": "pi",
            "instructions": "Use examples.",
            "agents": {
                "codex": {"model": "gpt-test", "reasoning": "high"},
                "pi": {"model": "fast", "reasoning": null}
            }
        }))
        .unwrap();

        assert_eq!(config.agent, "pi");
        assert_eq!(config.model("codex"), Some("gpt-test"));
        assert_eq!(config.reasoning("codex"), Some("high"));
        assert_eq!(config.model("pi"), Some("fast"));
        assert_eq!(config.reasoning("pi"), None);
        assert_eq!(config.instructions(), &Instructions::Concise);
    }

    #[test]
    fn v1_can_disable_instructions() {
        let disabled = Config::from_value(&json!({
            "version": 1,
            "agent": "codex",
            "instructions": null,
            "agents": {}
        }))
        .unwrap();

        assert_eq!(disabled.instructions(), &Instructions::AgentDefault);
    }

    #[test]
    fn v2_uses_explicit_instruction_modes() {
        let previous_default = Config::from_value(&json!({
            "version": 2,
            "agent": "codex",
            "instructions": "concise",
            "agents": {}
        }))
        .unwrap();
        let custom = Config::from_value(&json!({
            "version": 2,
            "agent": "codex",
            "instructions": { "custom": "Answer like a pirate." },
            "agents": {}
        }))
        .unwrap();

        assert_eq!(previous_default.instructions(), &Instructions::Concise);
        assert_eq!(
            custom.instructions(),
            &Instructions::Custom("Answer like a pirate.".into())
        );
    }
}

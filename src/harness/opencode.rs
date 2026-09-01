use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use serde_json::{Map, Value, json};

use super::{Harness, Model, ReasoningLevel, Response, RunOptions, bounded_output, capture_stderr};
use crate::error::{Error, Result};

const AGENT_ID: &str = "wut-read-only";
const CEREBRAS_PROVIDER_ID: &str = "cerebras";

#[derive(Clone, Copy, Eq, PartialEq)]
enum Profile {
    General,
    Cerebras,
}

pub(super) struct OpenCode {
    program: OsString,
    profile: Profile,
}

impl OpenCode {
    pub(super) fn new(program: OsString) -> Self {
        Self {
            program,
            profile: Profile::General,
        }
    }

    pub(super) fn cerebras(program: OsString) -> Self {
        Self {
            program,
            profile: Profile::Cerebras,
        }
    }

    fn command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Result<Command> {
        let existing = std::env::var("OPENCODE_CONFIG_CONTENT").ok();
        self.command_with_config(question, session_id, options, existing.as_deref())
    }

    fn command_with_config(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
        existing: Option<&str>,
    ) -> Result<Command> {
        let config = inline_config(existing, options.instructions, self.profile)?;
        let mut command = Command::new(&self.program);
        command
            .args(["--pure", "run", "--agent", AGENT_ID, "--format", "json"])
            .env("OPENCODE_CONFIG_CONTENT", config)
            .env("OPENCODE_AUTO_SHARE", "false")
            .stdin(Stdio::null());
        if let Some(session_id) = session_id {
            command.args(["--session", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if self.profile == Profile::Cerebras
            && let Some(reasoning) = options.reasoning
        {
            command.args(["--variant", reasoning]);
        }
        command.arg("--").arg(question);
        Ok(command)
    }
}

impl Harness for OpenCode {
    fn models(&mut self) -> Result<Vec<Model>> {
        if self.profile == Profile::Cerebras {
            return Ok(cerebras_models());
        }

        let mut command = Command::new(&self.program);
        command.args(["--pure", "models"]);
        let output = bounded_output(&mut command).map_err(start_error)?;
        if !output.status.success() {
            return Err(command_error(
                "could not list OpenCode models",
                &output.stderr,
            ));
        }
        let output = String::from_utf8(output.stdout).map_err(|_| {
            Error::agent(
                "opencode",
                "OpenCode returned a model list that was not valid UTF-8",
            )
        })?;
        parse_models(&output)
    }

    fn run(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let mut child = self
            .command(question, session_id, &options)?
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(start_error)?;
        let stdout = child.stdout.take().expect("piped stdout is available");
        let stderr = child.stderr.take().expect("piped stderr is available");
        let stderr_reader = std::thread::spawn(move || capture_stderr(stderr));

        let response = read_events(BufReader::new(stdout), on_delta);
        let status = child.wait().map_err(|error| {
            Error::new(
                format!("could not wait for OpenCode: {error}"),
                "restart wut and try again",
            )
        })?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| {
                Error::new(
                    "could not read OpenCode error output",
                    "restart wut and try again",
                )
            })?
            .into_detail();

        if !status.success() {
            if stderr.trim().is_empty() {
                return match response {
                    Err(error) => Err(error),
                    Ok(_) => Err(Error::agent(
                        "opencode",
                        format!("OpenCode exited with {status}"),
                    )),
                };
            }
            return Err(command_error("OpenCode failed", &stderr));
        }
        response
    }
}

fn inline_config(
    existing: Option<&str>,
    instructions: Option<&str>,
    profile: Profile,
) -> Result<String> {
    let mut config = match existing {
        Some(existing) => serde_json::from_str::<Value>(existing)
            .map_err(|error| {
                Error::new(
                    format!("could not parse OPENCODE_CONFIG_CONTENT: {error}"),
                    "fix or unset OPENCODE_CONFIG_CONTENT, then try again",
                )
            })?
            .as_object()
            .cloned()
            .ok_or_else(|| {
                Error::new(
                    "OPENCODE_CONFIG_CONTENT must contain a JSON object",
                    "fix or unset OPENCODE_CONFIG_CONTENT, then try again",
                )
            })?,
        None => Map::new(),
    };
    if profile == Profile::Cerebras {
        let providers = config
            .entry("provider")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                Error::new(
                    "OPENCODE_CONFIG_CONTENT has an invalid 'provider' value",
                    "fix or unset OPENCODE_CONFIG_CONTENT, then try again",
                )
            })?;
        providers.insert(CEREBRAS_PROVIDER_ID.into(), cerebras_provider());
    }

    let mut agent = json!({
        "description": "Read-only questions through wut",
        "mode": "primary",
        "permission": {
            "*": "deny",
            "read": {
                "*": "allow",
                "*.env": "deny",
                "*.env.*": "deny",
                "*.env.example": "allow"
            },
            "glob": "allow",
            "grep": "allow",
            "list": "allow",
            "webfetch": "allow",
            "websearch": "allow",
            "lsp": "allow"
        }
    });
    if let Some(instructions) = instructions {
        agent["prompt"] = Value::String(instructions.to_owned());
    }

    let agents = config
        .entry("agent")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            Error::new(
                "OPENCODE_CONFIG_CONTENT has an invalid 'agent' value",
                "fix or unset OPENCODE_CONFIG_CONTENT, then try again",
            )
        })?;
    agents.insert(AGENT_ID.into(), agent);
    config.insert("share".into(), Value::String("disabled".into()));
    serde_json::to_string(&config)
        .map_err(|error| Error::internal(format!("could not configure OpenCode: {error}")))
}

fn cerebras_provider() -> Value {
    json!({
        "npm": "@ai-sdk/cerebras",
        "name": "Cerebras",
        "options": {
            "baseURL": "https://api.cerebras.ai/v1"
        },
        "models": {
            "gpt-oss-120b": {
                "name": "GPT OSS 120B",
                "variants": {
                    "low": {"reasoningEffort": "low"},
                    "medium": {"reasoningEffort": "medium"},
                    "high": {"reasoningEffort": "high"}
                }
            },
            "gemma-4-31b": {
                "name": "Gemma 4 31B",
                "variants": {
                    "none": {"reasoningEffort": "none"},
                    "low": {"reasoningEffort": "low"},
                    "medium": {"reasoningEffort": "medium"},
                    "high": {"reasoningEffort": "high"}
                }
            }
        }
    })
}

fn cerebras_models() -> Vec<Model> {
    let level = |id: &str, description: &str| ReasoningLevel {
        id: id.to_owned(),
        description: description.to_owned(),
    };
    vec![
        Model {
            id: "cerebras/gpt-oss-120b".into(),
            name: "GPT OSS 120B".into(),
            description: "Cerebras public endpoint".into(),
            is_default: true,
            reasoning: vec![
                level("low", "Low reasoning"),
                level("medium", "Balanced reasoning"),
                level("high", "Deep reasoning"),
            ],
            default_reasoning: Some("medium".into()),
        },
        Model {
            id: "cerebras/gemma-4-31b".into(),
            name: "Gemma 4 31B".into(),
            description: "Cerebras public endpoint · vision capable".into(),
            is_default: false,
            reasoning: vec![
                level("none", "No reasoning"),
                level("low", "Reasoning enabled"),
                level("medium", "Reasoning enabled"),
                level("high", "Reasoning enabled"),
            ],
            default_reasoning: Some("none".into()),
        },
    ]
}

fn parse_models(output: &str) -> Result<Vec<Model>> {
    let models = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|id| {
            let (provider, model) = id
                .split_once('/')
                .filter(|(provider, model)| !provider.is_empty() && !model.is_empty())
                .ok_or_else(|| {
                    Error::agent(
                        "opencode",
                        format!("OpenCode returned an invalid model ID: {id}"),
                    )
                })?;
            Ok(Model {
                id: id.to_owned(),
                name: model.to_owned(),
                description: format!("{provider} provider"),
                is_default: false,
                reasoning: Vec::new(),
                default_reasoning: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if models.is_empty() {
        Err(Error::agent(
            "opencode",
            "OpenCode did not report any available models",
        ))
    } else {
        Ok(models)
    }
}

fn read_events(
    reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut session_id = None;
    let mut answer = String::new();
    let mut reported_error = None;

    for line in reader.lines() {
        let line = line.map_err(|error| {
            Error::agent(
                "opencode",
                format!("could not read OpenCode response: {error}"),
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(&line).map_err(|error| {
            Error::agent(
                "opencode",
                format!("could not parse OpenCode response: {error}"),
            )
        })?;
        if session_id.is_none() {
            session_id = event
                .get("sessionID")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        match event.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = event["part"]["text"].as_str()
                    && !text.is_empty()
                {
                    answer.push_str(text);
                    on_delta(text)?;
                }
            }
            Some("error") => {
                reported_error = event["error"]["data"]["message"]
                    .as_str()
                    .or_else(|| event["error"]["message"].as_str())
                    .or_else(|| event["error"]["name"].as_str())
                    .map(str::to_owned);
            }
            _ => {}
        }
    }

    if let Some(error) = reported_error {
        return Err(Error::agent(
            "opencode",
            format!("OpenCode reported an error: {error}"),
        ));
    }
    if answer.is_empty() {
        return Err(Error::agent(
            "opencode",
            "OpenCode completed without returning an answer",
        ));
    }
    Ok(Response {
        answer,
        session_id: session_id.ok_or_else(|| {
            Error::agent(
                "opencode",
                "OpenCode completed without returning a session ID",
            )
        })?,
    })
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "OpenCode is not installed or not on PATH",
            "install it, authenticate, then try again",
        )
    } else {
        Error::agent("opencode", format!("could not start OpenCode: {error}"))
    }
}

fn command_error(message: &str, stderr: &str) -> Error {
    let detail = stderr.trim();
    let message = if detail.is_empty() {
        message.to_owned()
    } else {
        format!("{message}: {detail}")
    };
    Error::agent("opencode", message)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::Value;

    use super::{OpenCode, Profile, cerebras_models, inline_config, parse_models, read_events};
    use crate::harness::RunOptions;

    #[test]
    fn configures_a_private_read_only_agent() {
        let config = inline_config(None, Some("Be concise."), Profile::General).unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        let agent = &config["agent"]["wut-read-only"];

        assert_eq!(config["share"], "disabled");
        assert_eq!(agent["prompt"], "Be concise.");
        assert_eq!(agent["permission"]["*"], "deny");
        assert_eq!(agent["permission"]["read"]["*"], "allow");
        assert_eq!(agent["permission"]["read"]["*.env"], "deny");
    }

    #[test]
    fn preserves_existing_inline_config() {
        let config = inline_config(
            Some(r#"{"provider":{"local":{"name":"Local"}}}"#),
            None,
            Profile::General,
        )
        .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();

        assert_eq!(config["provider"]["local"]["name"], "Local");
        assert_eq!(config["agent"]["wut-read-only"]["mode"], "primary");
    }

    #[test]
    fn cerebras_profile_defines_the_official_provider_without_copying_the_key() {
        let config = inline_config(None, None, Profile::Cerebras).unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        let provider = &config["provider"]["cerebras"];

        assert_eq!(provider["npm"], "@ai-sdk/cerebras");
        assert_eq!(provider["options"]["baseURL"], "https://api.cerebras.ai/v1");
        assert!(provider["options"].get("apiKey").is_none());
        assert_eq!(
            provider["models"]["gpt-oss-120b"]["variants"]["high"]["reasoningEffort"],
            "high"
        );
        assert_eq!(
            provider["models"]["gemma-4-31b"]["variants"]["none"]["reasoningEffort"],
            "none"
        );
    }

    #[test]
    fn cerebras_catalog_matches_the_public_models_and_reasoning_controls() {
        let models = cerebras_models();

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["cerebras/gpt-oss-120b", "cerebras/gemma-4-31b"]
        );
        assert!(models[0].is_default);
        assert_eq!(models[0].default_reasoning.as_deref(), Some("medium"));
        assert_eq!(
            models[0]
                .reasoning
                .iter()
                .map(|level| level.id.as_str())
                .collect::<Vec<_>>(),
            ["low", "medium", "high"]
        );
        assert_eq!(models[1].default_reasoning.as_deref(), Some("none"));
    }

    #[test]
    fn builds_a_session_command() {
        let harness = OpenCode {
            program: "opencode".into(),
            profile: Profile::General,
        };
        let command = harness
            .command_with_config(
                "hello",
                Some("session-1"),
                &RunOptions {
                    model: Some("anthropic/claude-test"),
                    reasoning: None,
                    instructions: None,
                },
                None,
            )
            .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();
        let auto_share = command
            .get_envs()
            .find(|(name, _)| *name == "OPENCODE_AUTO_SHARE")
            .and_then(|(_, value)| value)
            .unwrap();

        assert!(
            args.windows(2)
                .any(|args| args == ["--agent", "wut-read-only"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--session", "session-1"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--model", "anthropic/claude-test"])
        );
        assert_eq!(auto_share, "false");
    }

    #[test]
    fn dash_prefixed_question_follows_option_terminator() {
        let harness = OpenCode {
            program: "opencode".into(),
            profile: Profile::General,
        };
        let command = harness
            .command_with_config(
                "-why did this fail",
                None,
                &RunOptions {
                    model: None,
                    reasoning: None,
                    instructions: None,
                },
                None,
            )
            .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|args| args == ["--", "-why did this fail"])
        );
    }

    #[test]
    fn parses_models() {
        let models = parse_models("opencode/free\nanthropic/sonnet\n").unwrap();

        assert_eq!(models[0].id, "opencode/free");
        assert_eq!(models[1].name, "sonnet");
        assert_eq!(models[1].description, "anthropic provider");
    }

    #[test]
    fn streams_text_and_extracts_the_session() {
        let input = concat!(
            "{\"type\":\"step_start\",\"sessionID\":\"session-1\",\"part\":{}}\n",
            "{\"type\":\"text\",\"sessionID\":\"session-1\",\"part\":{\"text\":\"hello\"}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"session-1\",\"part\":{}}\n"
        );
        let mut streamed = String::new();
        let response = read_events(Cursor::new(input), &mut |text| {
            streamed.push_str(text);
            Ok(())
        })
        .unwrap();

        assert_eq!(streamed, "hello");
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "session-1");
    }

    #[test]
    fn surfaces_reported_errors() {
        let input = "{\"type\":\"error\",\"sessionID\":\"session-1\",\"error\":{\"data\":{\"message\":\"not authenticated\"}}}\n";
        let error = read_events(Cursor::new(input), &mut |_| Ok(())).unwrap_err();

        assert_eq!(
            error.message(),
            "OpenCode reported an error: not authenticated"
        );
    }
}

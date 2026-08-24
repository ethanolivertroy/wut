use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Map, Value, json};

use crate::error::{Error, Result};
use crate::protocol::{Invocation, Kind, bounded_output};

#[derive(Clone, Copy, Debug)]
pub struct Definition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default_program: &'static str,
    pub read_only: &'static str,
    env: &'static str,
    aliases: &'static [&'static str],
}

pub static DEFINITIONS: &[Definition] = &[
    Definition {
        id: "cursor",
        name: "Cursor",
        description: "Cursor Agent CLI",
        default_program: "cursor-agent",
        read_only: "ask mode",
        env: "WUT_CURSOR_BIN",
        aliases: &["cursor-agent"],
    },
    Definition {
        id: "grok",
        name: "Grok",
        description: "xAI Grok Build",
        default_program: "grok",
        read_only: "plan permission mode",
        env: "WUT_GROK_BIN",
        aliases: &["grok-cli"],
    },
    Definition {
        id: "codex",
        name: "Codex",
        description: "OpenAI Codex",
        default_program: "codex",
        read_only: "read-only sandbox",
        env: "WUT_CODEX_BIN",
        aliases: &[],
    },
    Definition {
        id: "claude",
        name: "Claude Code",
        description: "Anthropic Claude Code",
        default_program: "claude",
        read_only: "plan permission mode",
        env: "WUT_CLAUDE_BIN",
        aliases: &["claude-code"],
    },
    Definition {
        id: "pi",
        name: "Pi",
        description: "Pi coding agent",
        default_program: "pi",
        read_only: "read, grep, find, and ls only",
        env: "WUT_PI_BIN",
        aliases: &[],
    },
    Definition {
        id: "opencode",
        name: "OpenCode",
        description: "OpenCode coding agent",
        default_program: "opencode",
        read_only: "deny-by-default permissions",
        env: "WUT_OPENCODE_BIN",
        aliases: &["open-code"],
    },
];

pub struct Request<'a> {
    pub question: &'a str,
    pub session_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub reasoning: Option<&'a str>,
    pub instructions: Option<&'a str>,
}

impl Definition {
    pub fn program(&self) -> OsString {
        std::env::var_os(self.env).unwrap_or_else(|| self.default_program.into())
    }

    pub fn available(&self) -> bool {
        executable_available(&self.program())
    }
}

pub fn resolve(name: &str) -> Result<&'static Definition> {
    DEFINITIONS
        .iter()
        .find(|definition| definition.id == name || definition.aliases.contains(&name))
        .ok_or_else(|| {
            Error::usage(format!(
                "unknown agent '{name}' (choose from {})",
                DEFINITIONS
                    .iter()
                    .map(|definition| definition.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

pub fn invocation(agent: &str, request: &Request<'_>) -> Result<Invocation> {
    let definition = resolve(agent)?;
    let opencode_config = if definition.id == "opencode" {
        std::env::var("OPENCODE_CONFIG_CONTENT").ok()
    } else {
        None
    };
    invocation_for_definition(definition, request, opencode_config.as_deref())
}

#[cfg(test)]
fn invocation_with_opencode_config(
    agent: &str,
    request: &Request<'_>,
    opencode_config: Option<&str>,
) -> Result<Invocation> {
    invocation_for_definition(resolve(agent)?, request, opencode_config)
}

fn invocation_for_definition(
    definition: &Definition,
    request: &Request<'_>,
    opencode_config: Option<&str>,
) -> Result<Invocation> {
    let invocation = match definition.id {
        "cursor" => cursor(definition, request),
        "grok" => grok(definition, request),
        "codex" => codex(definition, request),
        "claude" => claude(definition, request),
        "pi" => pi(definition, request),
        "opencode" => opencode(definition, request, opencode_config)?,
        _ => unreachable!("all registered agents have an invocation"),
    };
    Ok(invocation)
}

pub fn models(agent: &str) -> Result<String> {
    let definition = resolve(agent)?;
    if definition.id == "claude" {
        return Ok("sonnet\nopus\nhaiku\n".into());
    }
    if definition.id == "codex" {
        return Err(
            Error::new("Codex does not expose a stable model-list command")
                .hint("pass any Codex model ID with '--model'"),
        );
    }

    let mut command = Command::new(definition.program());
    match definition.id {
        "cursor" | "grok" => {
            command.arg("models");
        }
        "pi" => {
            command.arg("--list-models");
        }
        "opencode" => {
            command.args(["--pure", "models"]);
        }
        _ => unreachable!(),
    }
    command.stdin(Stdio::null());
    let output = bounded_output(&mut command).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            start_error(definition, error)
        } else {
            Error::new(format!(
                "could not run {} model-list command: {error}",
                definition.name
            ))
        }
    })?;
    if !output.status.success() {
        return Err(command_error(definition, output.status, &output.stderr));
    }
    String::from_utf8(output.stdout).map_err(|_| {
        Error::new(format!(
            "{} returned a model list that was not valid UTF-8",
            definition.name
        ))
    })
}

fn cursor(definition: &Definition, request: &Request<'_>) -> Invocation {
    let mut invocation = Invocation::new(definition, Kind::Cursor);
    invocation.args([
        "-p",
        "--mode",
        "ask",
        "--trust",
        "--output-format",
        "stream-json",
        "--stream-partial-output",
    ]);
    if let Some(session) = request.session_id {
        invocation.args(["--resume", session]);
    }
    if let Some(model) = request.model {
        invocation.args(["--model", model]);
    }
    invocation.arg("--");
    invocation.arg(combined_prompt(request));
    invocation
}

fn grok(definition: &Definition, request: &Request<'_>) -> Invocation {
    let mut invocation = Invocation::new(definition, Kind::Grok);
    invocation.args([
        "-p",
        request.question,
        "--output-format",
        "streaming-json",
        "--permission-mode",
        "plan",
        "--no-auto-update",
    ]);
    if let Some(session) = request.session_id {
        invocation.args(["--resume", session]);
    }
    if let Some(model) = request.model {
        invocation.args(["--model", model]);
    }
    if let Some(reasoning) = request.reasoning {
        invocation.args(["--reasoning-effort", reasoning]);
    }
    if let Some(instructions) = request.instructions {
        invocation.args(["--rules", instructions]);
    }
    invocation
}

fn codex(definition: &Definition, request: &Request<'_>) -> Invocation {
    let mut invocation = Invocation::new(definition, Kind::Codex);
    invocation.args([
        "exec",
        "--json",
        "--sandbox",
        "read-only",
        "--skip-git-repo-check",
    ]);
    if let Some(model) = request.model {
        invocation.args(["--model", model]);
    }
    if let Some(reasoning) = request.reasoning {
        invocation.args(["--config", &format!("model_reasoning_effort={reasoning:?}")]);
    }
    if let Some(session) = request.session_id {
        invocation.args(["resume", session]);
    }
    invocation.arg("--");
    invocation.arg(combined_prompt(request));
    invocation
}

fn claude(definition: &Definition, request: &Request<'_>) -> Invocation {
    let mut invocation = Invocation::new(definition, Kind::Claude);
    invocation.args([
        "--print",
        "--verbose",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-mode",
        "plan",
    ]);
    if let Some(session) = request.session_id {
        invocation.args(["--resume", session]);
    }
    if let Some(model) = request.model {
        invocation.args(["--model", model]);
    }
    if let Some(reasoning) = request.reasoning {
        invocation.args(["--effort", reasoning]);
    }
    if let Some(instructions) = request.instructions {
        invocation.args(["--append-system-prompt", instructions]);
    }
    invocation.arg("--");
    invocation.arg(request.question);
    invocation
}

fn pi(definition: &Definition, request: &Request<'_>) -> Invocation {
    let mut invocation = Invocation::new(definition, Kind::Pi);
    invocation.args([
        "--mode",
        "json",
        "--print",
        "--tools",
        "read,grep,find,ls",
        "--no-extensions",
    ]);
    if let Some(session) = request.session_id {
        invocation.args(["--session", session]);
    }
    if let Some(model) = request.model {
        invocation.args(["--model", model]);
    }
    if let Some(reasoning) = request.reasoning {
        invocation.args(["--thinking", reasoning]);
    }
    if let Some(instructions) = request.instructions {
        invocation.args(["--append-system-prompt", instructions]);
    }
    invocation.arg("--");
    invocation.arg(request.question);
    invocation
}

fn opencode(
    definition: &Definition,
    request: &Request<'_>,
    existing_config: Option<&str>,
) -> Result<Invocation> {
    let mut invocation = Invocation::new(definition, Kind::OpenCode);
    invocation.args([
        "--pure",
        "run",
        "--agent",
        "wut-read-only",
        "--format",
        "json",
    ]);
    if let Some(session) = request.session_id {
        invocation.args(["--session", session]);
    }
    if let Some(model) = request.model {
        invocation.args(["--model", model]);
    }
    if let Some(reasoning) = request.reasoning {
        invocation.args(["--variant", reasoning]);
    }
    invocation.arg("--");
    invocation.arg(request.question);
    invocation.env(
        "OPENCODE_CONFIG_CONTENT",
        inline_opencode_config(existing_config, request.instructions)?,
    );
    invocation.env("OPENCODE_AUTO_SHARE", "false");
    Ok(invocation)
}

fn combined_prompt(request: &Request<'_>) -> String {
    match request.instructions {
        Some(instructions) if !instructions.is_empty() => {
            format!("{}\n\n{}", request.question, instructions)
        }
        _ => request.question.to_owned(),
    }
}

fn inline_opencode_config(
    existing_config: Option<&str>,
    instructions: Option<&str>,
) -> Result<String> {
    let mut config = match existing_config {
        Some(existing) => serde_json::from_str::<Value>(existing)
            .map_err(|error| Error::new(format!("invalid OPENCODE_CONFIG_CONTENT: {error}")))?
            .as_object()
            .cloned()
            .ok_or_else(|| Error::new("OPENCODE_CONFIG_CONTENT must be a JSON object"))?,
        None => Map::new(),
    };
    let mut agent = json!({
        "description": "Read-only questions through wut",
        "mode": "primary",
        "permission": {
            "*": "deny",
            "external_directory": "deny",
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
    config
        .entry("agent")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::new("OPENCODE_CONFIG_CONTENT field 'agent' must be an object"))?
        .insert("wut-read-only".into(), agent);
    config
        .entry("permission")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::new("OPENCODE_CONFIG_CONTENT field 'permission' must be an object"))?
        .insert("external_directory".into(), Value::String("deny".into()));
    config.insert("share".into(), Value::String("disabled".into()));
    serde_json::to_string(&config)
        .map_err(|error| Error::new(format!("could not configure OpenCode: {error}")))
}

fn executable_available(program: &OsStr) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return executable(path);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|directory| executable(&directory.join(path)))
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn start_error(definition: &Definition, error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(format!(
            "{} is not installed or '{}' is not on PATH",
            definition.name,
            definition.program().to_string_lossy()
        ))
        .hint(format!(
            "install and authenticate {}, then retry",
            definition.name
        ))
    } else {
        Error::new(format!("could not start {}: {error}", definition.name))
    }
}

fn command_error(definition: &Definition, status: std::process::ExitStatus, stderr: &str) -> Error {
    if stderr.trim().is_empty() {
        Error::new(format!("{} exited with {status}", definition.name))
    } else {
        Error::new(format!("{} failed: {}", definition.name, stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{Request, inline_opencode_config, invocation_with_opencode_config, resolve};

    fn request<'a>(session: Option<&'a str>) -> Request<'a> {
        request_with_question(session, "why?")
    }

    fn request_with_question<'a>(session: Option<&'a str>, question: &'a str) -> Request<'a> {
        Request {
            question,
            session_id: session,
            model: Some("model-1"),
            reasoning: Some("high"),
            instructions: Some("Be concise."),
        }
    }

    fn args(agent: &str, request: &Request<'_>) -> Vec<String> {
        invocation_with_opencode_config(agent, request, None)
            .unwrap()
            .args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    fn has_pair(args: &[String], left: &str, right: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == left && pair[1] == right)
    }

    #[test]
    fn cursor_and_grok_have_unambiguous_programs() {
        assert_eq!(resolve("cursor").unwrap().default_program, "cursor-agent");
        assert_eq!(resolve("grok").unwrap().default_program, "grok");
    }

    #[test]
    fn every_agent_command_is_natively_read_only() {
        let cursor = args("cursor", &request(Some("s")));
        assert!(has_pair(&cursor, "--mode", "ask"));
        assert!(has_pair(&cursor, "--resume", "s"));

        let grok = args("grok", &request(Some("s")));
        assert!(has_pair(&grok, "--permission-mode", "plan"));

        let codex = args("codex", &request(Some("s")));
        assert!(has_pair(&codex, "--sandbox", "read-only"));
        assert!(codex.iter().any(|arg| arg == "resume"));

        let claude = args("claude", &request(Some("s")));
        assert!(has_pair(&claude, "--permission-mode", "plan"));

        let pi = args("pi", &request(Some("s")));
        assert!(has_pair(&pi, "--tools", "read,grep,find,ls"));

        let opencode =
            invocation_with_opencode_config("opencode", &request(Some("s")), None).unwrap();
        let config = opencode
            .env
            .iter()
            .find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT")
            .unwrap()
            .1
            .to_string_lossy();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["agent"]["wut-read-only"]["permission"]["*"], "deny");
        assert_eq!(config["share"], "disabled");
    }

    #[test]
    fn opencode_blocks_secret_files() {
        let config: Value =
            serde_json::from_str(&inline_opencode_config(None, None).unwrap()).unwrap();
        assert_eq!(config["permission"]["external_directory"], "deny");
        assert_eq!(
            config["agent"]["wut-read-only"]["permission"]["external_directory"],
            "deny"
        );
        assert_eq!(
            config["agent"]["wut-read-only"]["permission"]["read"]["*.env"],
            "deny"
        );
    }

    #[test]
    fn positional_prompts_cannot_be_reparsed_as_provider_options() {
        for agent in ["cursor", "codex", "claude", "pi", "opencode"] {
            for session in [None, Some("session-1")] {
                let args = args(agent, &request_with_question(session, "--help"));
                let prompt = args.last().unwrap();
                assert!(prompt.starts_with("--help"), "{agent}: {args:?}");
                assert_eq!(args.get(args.len() - 2).map(String::as_str), Some("--"));
            }
        }

        let grok = args("grok", &request_with_question(None, "--help"));
        assert!(has_pair(&grok, "-p", "--help"));
    }
}

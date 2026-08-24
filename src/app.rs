use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

use serde_json::json;

use crate::agent::{self, Request};
use crate::cli::{self, Ask, Command, ConfigCommand};
use crate::config::{AgentConfig, Config};
use crate::error::{Error, Result};
use crate::session::{self, Session, Settings};

pub fn run(args: impl Iterator<Item = OsString>) -> Result<()> {
    match cli::parse(args)? {
        Command::Help => output(&[cli::HELP.as_bytes()]),
        Command::Version => output(&[env!("CARGO_PKG_VERSION").as_bytes(), b"\n"]),
        Command::Agents { json } => list_agents(json),
        Command::Models { agent } => list_models(agent.as_deref()),
        Command::Sessions { json } => list_sessions(json),
        Command::Config(command) => config(command),
        Command::Ask(options) => ask(options),
    }
}

fn list_agents(as_json: bool) -> Result<()> {
    if as_json {
        let value = agent::DEFINITIONS
            .iter()
            .map(|definition| {
                json!({
                    "id": definition.id,
                    "name": definition.name,
                    "description": definition.description,
                    "program": definition.program().to_string_lossy(),
                    "available": definition.available(),
                    "read_only": definition.read_only,
                })
            })
            .collect::<Vec<_>>();
        return json_output(&value);
    }

    let mut stdout = io::stdout().lock();
    for definition in agent::DEFINITIONS {
        writeln!(
            stdout,
            "{:<9} {:<7} {:<16} {}",
            definition.id,
            if definition.available() {
                "ready"
            } else {
                "missing"
            },
            definition.program().to_string_lossy(),
            definition.read_only
        )
        .map_err(output_error)?;
    }
    Ok(())
}

fn list_models(selected: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let agent = selected.unwrap_or(&config.agent);
    let models = agent::models(agent)?;
    output(&[models.as_bytes()])
}

fn list_sessions(as_json: bool) -> Result<()> {
    let sessions = session::load_all()?;
    if as_json {
        let summaries = sessions
            .iter()
            .map(|session| {
                json!({
                    "id": session.id,
                    "agent": session.agent,
                    "cwd": session.cwd,
                    "updated_at": session.updated_at,
                    "turn_count": session.turns.len(),
                    "model": session.settings.model,
                    "reasoning": session.settings.reasoning,
                })
            })
            .collect::<Vec<_>>();
        return json_output(&summaries);
    }
    if sessions.is_empty() {
        return output(&[b"No saved sessions.\n"]);
    }
    let mut stdout = io::stdout().lock();
    for session in sessions {
        writeln!(
            stdout,
            "{}\t{}\t{} turns\t{}",
            session.id,
            session.agent,
            session.turns.len(),
            session.cwd
        )
        .map_err(output_error)?;
    }
    Ok(())
}

fn config(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Path => {
            let path = crate::config::path()?;
            output(&[path.to_string_lossy().as_bytes(), b"\n"])
        }
        ConfigCommand::Show { json } => {
            let config = Config::load()?;
            let path = crate::config::path()?;
            if json {
                return json_output(&json!({
                    "path": path,
                    "version": config.version,
                    "agent": config.agent,
                    "instructions": config.instructions,
                    "agents": config.agents,
                }));
            }
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "path\t{}", path.display()).map_err(output_error)?;
            writeln!(stdout, "agent\t{}", config.agent).map_err(output_error)?;
            writeln!(
                stdout,
                "instructions\t{}",
                config.instructions.as_deref().unwrap_or("agent default")
            )
            .map_err(output_error)?;
            for (agent, settings) in config.agents {
                if let Some(model) = settings.model {
                    writeln!(stdout, "{agent}.model\t{model}").map_err(output_error)?;
                }
                if let Some(reasoning) = settings.reasoning {
                    writeln!(stdout, "{agent}.reasoning\t{reasoning}").map_err(output_error)?;
                }
            }
            Ok(())
        }
        ConfigCommand::Set { key, value } => {
            validate_config_key(&key, &value)?;
            let mut config = Config::load()?;
            config.set(&key, &value)?;
            config.save()?;
            output(&[
                b"updated ",
                crate::config::path()?.to_string_lossy().as_bytes(),
                b"\n",
            ])
        }
    }
}

fn validate_config_key(key: &str, value: &str) -> Result<()> {
    if key == "agent" {
        agent::resolve(value)?;
    } else if let Some((agent, field)) = key.split_once('.') {
        agent::resolve(agent)?;
        if !matches!(field, "model" | "reasoning") {
            return Err(Error::usage(format!("unknown config key '{key}'")));
        }
    }
    Ok(())
}

fn ask(options: Ask) -> Result<()> {
    let config = Config::load()?;
    let cwd = std::env::current_dir()
        .map_err(|error| Error::new(format!("could not read current directory: {error}")))?;
    let mut saved = match options.session.as_deref() {
        Some(id) => Some(session::find(id)?),
        None if options.continuation => Some(session::latest(&cwd)?),
        None => None,
    };

    let agent = match (&saved, options.agent.as_deref()) {
        (Some(saved), Some(selected)) if saved.agent != selected => {
            return Err(Error::usage(format!(
                "session '{}' belongs to agent '{}', not '{selected}'",
                saved.id, saved.agent
            )));
        }
        (Some(saved), _) => saved.agent.clone(),
        (None, Some(selected)) => selected.to_owned(),
        (None, None) => config.agent.clone(),
    };
    agent::resolve(&agent)?;

    let base = saved
        .as_ref()
        .map(|saved| AgentConfig {
            model: saved.settings.model.clone(),
            reasoning: saved.settings.reasoning.clone(),
        })
        .unwrap_or_else(|| config.settings(&agent));
    let settings = Settings {
        model: options.model.or(base.model),
        reasoning: options.reasoning.or(base.reasoning),
    };

    match options.question {
        Some(question) => turn(
            &agent,
            &settings,
            config.instructions.as_deref(),
            &question,
            &cwd,
            &mut saved,
        ),
        None => interactive(
            &agent,
            &settings,
            config.instructions.as_deref(),
            &cwd,
            &mut saved,
        ),
    }
}

fn interactive(
    agent: &str,
    settings: &Settings,
    instructions: Option<&str>,
    cwd: &Path,
    saved: &mut Option<Session>,
) -> Result<()> {
    let terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
    let mut input = String::new();
    loop {
        if terminal {
            eprint!("wut> ");
            io::stderr().flush().map_err(output_error)?;
        }
        input.clear();
        if io::stdin()
            .read_line(&mut input)
            .map_err(|error| Error::new(format!("could not read input: {error}")))?
            == 0
        {
            break;
        }
        let question = input.trim();
        if question.is_empty() {
            continue;
        }
        if matches!(question, "/quit" | "/exit") {
            break;
        }
        if question == "/help" {
            eprintln!("/quit or /exit ends the session; every other line is sent to {agent}.");
            continue;
        }
        turn(agent, settings, instructions, question, cwd, saved)?;
    }
    Ok(())
}

fn turn(
    agent: &str,
    settings: &Settings,
    instructions: Option<&str>,
    question: &str,
    cwd: &Path,
    saved: &mut Option<Session>,
) -> Result<()> {
    let request = Request {
        question,
        session_id: saved
            .as_ref()
            .map(|session| session.native_session_id.as_str()),
        model: settings.model.as_deref(),
        reasoning: settings.reasoning.as_deref(),
        instructions,
    };
    let invocation = agent::invocation(agent, &request)?;
    let mut stdout = io::stdout().lock();
    let response = crate::protocol::run(invocation, &mut |delta| {
        stdout.write_all(delta.as_bytes()).map_err(output_error)?;
        stdout.flush().map_err(output_error)
    })?;

    if response.streamed {
        if !response.answer.ends_with('\n') {
            stdout.write_all(b"\n").map_err(output_error)?;
        }
    } else {
        stdout
            .write_all(response.answer.as_bytes())
            .and_then(|()| stdout.write_all(b"\n"))
            .map_err(output_error)?;
    }
    stdout.flush().map_err(output_error)?;

    let session = saved.get_or_insert_with(|| {
        Session::new(agent, response.session_id.clone(), cwd, settings.clone())
    });
    session.native_session_id = response.session_id;
    session.settings = settings.clone();
    session.add_turn(question, response.answer);
    session::save(session)
}

fn json_output(value: &impl serde::Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::new(format!("could not encode JSON output: {error}")))?;
    bytes.push(b'\n');
    output(&[&bytes])
}

fn output(chunks: &[&[u8]]) -> Result<()> {
    let mut stdout = io::stdout().lock();
    for chunk in chunks {
        stdout.write_all(chunk).map_err(output_error)?;
    }
    stdout.flush().map_err(output_error)
}

fn output_error(error: io::Error) -> Error {
    Error::new(format!("could not write output: {error}"))
}

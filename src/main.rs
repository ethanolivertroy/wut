mod agent;
mod benchmark;
mod cancel;
mod cerebras;
mod cli;
mod clock;
mod config;
mod credentials;
mod diagram;
mod error;
mod instructions;
mod output;
mod prompt;
mod render;
mod response_format;
mod select;
mod settings_ui;
mod spinner;
mod state;
mod storage;
mod terminal;
mod tools;
mod triage;
#[cfg(test)]
mod triage_eval;
mod typesafe;
mod update_check;
mod upgrade;

use std::hash::{BuildHasher, Hasher, RandomState};
use std::process::ExitCode;

use error::{Error, Result};
use instructions::Instructions;
use output::{TurnOutput, write_stdout};

const SESSION_NAME_WIDTH: usize = 24;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error.print();
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli::Invocation {
        mode,
        model,
        reasoning,
    } = cli::parse(std::env::args_os().skip(1))?;
    let update_check = update_check::Check::start(update_checks_enabled(&mode));

    let result = (|| -> Result<()> {
        match mode {
            cli::Mode::Help => {
                write_stdout(&[cli::HELP.as_bytes()])?;
                Ok(())
            }
            cli::Mode::Version => {
                write_stdout(&[env!("CARGO_PKG_VERSION").as_bytes(), b"\n"])?;
                Ok(())
            }
            cli::Mode::Models => list_models(),
            cli::Mode::Benchmark => benchmark::run(model.as_deref(), reasoning.as_deref()),
            cli::Mode::Upgrade => {
                if let Some(message) = upgrade::run()? {
                    write_stdout(&[message.as_bytes(), b"\n"])?;
                }
                Ok(())
            }
            cli::Mode::UpdateCheck => {
                update_check::refresh();
                Ok(())
            }
            cli::Mode::OneShot(question) => {
                let config = config::Config::load()?;
                let settings = Settings::resolve(&config, None)
                    .with_overrides(model.as_deref(), reasoning.as_deref())?;
                let question = Question::with_piped_context(question)?;
                run_one_shot(&settings, config.instructions(), &question, None)
            }
            cli::Mode::Interactive => {
                let config = config::Config::load()?;
                let settings = Settings::resolve(&config, None)
                    .with_overrides(model.as_deref(), reasoning.as_deref())?;
                run_interactive(settings, None, config)
            }
            cli::Mode::Continue(words) => {
                let config = config::Config::load()?;
                let cwd = current_dir()?;
                let session = state::latest(&cwd)?;
                let settings = Settings::resolve(&config, Some(&session))
                    .with_overrides(model.as_deref(), reasoning.as_deref())?;
                if words.is_empty() {
                    run_interactive(settings, Some(session), config)
                } else {
                    let question = Question::with_piped_context(words.join(" "))?;
                    run_one_shot(&settings, config.instructions(), &question, Some(session))
                }
            }
            cli::Mode::Sessions => sessions(),
            cli::Mode::Settings => standalone_settings(),
            cli::Mode::SetKey(provider) => credentials::run(provider),
        }
    })();
    update_check.finish(result.is_ok());
    result
}

fn update_checks_enabled(mode: &cli::Mode) -> bool {
    use std::io::{self, IsTerminal};

    std::env::var_os("WUT_NO_UPDATE_CHECK").is_none()
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && !matches!(
            mode,
            cli::Mode::Help
                | cli::Mode::Version
                | cli::Mode::Upgrade
                | cli::Mode::UpdateCheck
                | cli::Mode::Models
                | cli::Mode::Benchmark
                | cli::Mode::SetKey(_)
        )
}

pub(crate) struct Settings {
    model: Option<String>,
    reasoning: Option<String>,
    response_color: config::ResponseColor,
}

impl Settings {
    fn with_overrides(mut self, model: Option<&str>, reasoning: Option<&str>) -> Result<Self> {
        if let Some(id) = model {
            let selected = cerebras::require_model(id)?;
            self.model = Some(selected.id.to_owned());
            // A one-off model choice gets that model's own defaults, rather
            // than an incompatible effort inherited from another model.
            self.reasoning = selected.default_reasoning.map(str::to_owned);
        }
        if let Some(level) = reasoning {
            cerebras::require_model(self.model.as_deref().unwrap_or(cerebras::default_model()))?
                .validate_reasoning(level)?;
            self.reasoning = Some(level.to_owned());
        }
        Ok(self)
    }

    fn resolve(config: &config::Config, session: Option<&state::Session>) -> Self {
        let saved = session.and_then(|session| session.settings.as_ref());
        let model = saved.map_or_else(
            || config.model().map(str::to_owned),
            |settings| settings.model.clone(),
        );
        let reasoning = saved.map_or_else(
            || config.reasoning().map(str::to_owned),
            |settings| settings.reasoning.clone(),
        );
        let (model, reasoning) = cerebras::migrate_archived_model(model, reasoning);
        Self {
            model,
            reasoning,
            response_color: config.response_color(),
        }
    }
}

fn list_models() -> Result<()> {
    let mut output = String::from("MODEL\tPROVIDER\tAPI KEY\tDEFAULT REASONING\n");
    for model in cerebras::MODELS {
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            model.id,
            model.provider.name(),
            model.provider.env_key(),
            model.default_reasoning.unwrap_or("none")
        ));
    }
    write_stdout(&[output.as_bytes()])
}

/// The most piped input wut sends. Anything longer is cut with a note.
const MAX_PIPED_BYTES: usize = 256 * 1_024;

/// A question as sent to the model and as saved in the session. They differ
/// when input was piped in: the model sees all of it, the session file only
/// records how much there was.
#[derive(Debug, Eq, PartialEq)]
struct Question {
    prompt: String,
    saved: String,
}

impl Question {
    fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            prompt: text.clone(),
            saved: text,
        }
    }

    /// `cat log | wut why did this fail`: the piped text becomes context for
    /// the question, sent as one turn.
    fn with_piped_context(question: String) -> Result<Self> {
        Ok(match piped_input()? {
            Some(input) => Self::with_context(question, &input),
            None => Self::plain(question),
        })
    }

    fn with_context(question: String, input: &str) -> Self {
        let fence = if input.contains("```") { "````" } else { "```" };
        Self {
            prompt: format!("{question}\n\n{fence}\n{}\n{fence}", input.trim_end()),
            saved: format!("{question} [+{} of piped input]", byte_count(input.len())),
        }
    }

    fn reasoning_to_save<'a>(&self, reasoning: Option<&'a str>) -> Option<&'a str> {
        // Reasoning can quote piped input, which session history omits.
        reasoning.filter(|_| self.prompt == self.saved)
    }
}

fn byte_count(bytes: usize) -> String {
    if bytes < 1_024 {
        format!("{bytes} bytes")
    } else {
        format!("{} KiB", bytes.div_ceil(1_024))
    }
}

/// Everything piped into wut, or `None` when stdin is a terminal or there is
/// nothing to read. Only a pipe or a redirected file counts, so a script that
/// leaves stdin open on a socket or a device never hangs waiting for EOF.
fn piped_input() -> Result<Option<String>> {
    use std::io::{IsTerminal, Read};

    let stdin = std::io::stdin();
    if stdin.is_terminal() || !stdin_is_pipe_or_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    stdin
        .lock()
        .take(MAX_PIPED_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            Error::new(
                format!("could not read piped input: {error}"),
                "check stdin and try again",
            )
        })?;
    let truncated = bytes.len() > MAX_PIPED_BYTES;
    bytes.truncate(MAX_PIPED_BYTES);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.trim().is_empty() {
        return Ok(None);
    }
    if truncated {
        text.push_str(&format!(
            "\n[piped input cut off after {}]",
            byte_count(MAX_PIPED_BYTES)
        ));
    }
    Ok(Some(text))
}

#[cfg(unix)]
fn stdin_is_pipe_or_file() -> bool {
    use std::os::fd::AsFd;
    use std::os::unix::fs::FileTypeExt;

    std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(std::fs::File::from)
        .and_then(|file| file.metadata())
        .is_ok_and(|metadata| metadata.file_type().is_fifo() || metadata.is_file())
}

#[cfg(not(unix))]
fn stdin_is_pipe_or_file() -> bool {
    true
}

#[derive(Debug, Eq, PartialEq)]
enum InteractiveCommand {
    Settings,
}

fn parse_interactive_command(input: &str) -> Result<InteractiveCommand> {
    let mut parts = input.split_whitespace();
    let command = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return Err(Error::new(
            format!("invalid command '{input}'"),
            "use '/settings' with no value",
        ));
    }
    match command {
        "/settings" => Ok(InteractiveCommand::Settings),
        _ => Err(Error::new(
            format!("unknown command '{command}'"),
            "use '/settings'",
        )),
    }
}

fn run_one_shot(
    settings: &Settings,
    instructions: &Instructions,
    question: &Question,
    mut saved: Option<state::Session>,
) -> Result<()> {
    use std::io::{self, IsTerminal};

    let decorate = io::stdout().is_terminal();
    let cwd = current_dir()?;
    let history = saved
        .as_ref()
        .map(|session| session.turns.as_slice())
        .unwrap_or(&[]);
    let mut agent = agent::Agent::new(
        settings.model.as_deref(),
        settings.reasoning.as_deref(),
        instructions,
        history,
    )?;
    run_turn(
        &mut agent,
        &question.prompt,
        &cwd,
        decorate,
        settings.response_color,
        &mut |answer, reasoning| {
            record_turn(
                &mut saved,
                settings,
                &cwd,
                &question.saved,
                answer.to_owned(),
                question.reasoning_to_save(reasoning),
            )
        },
    )?;
    Ok(())
}

fn run_interactive(
    mut settings: Settings,
    mut saved: Option<state::Session>,
    mut config: config::Config,
) -> Result<()> {
    use std::io::{self, IsTerminal};

    let terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
    let decorate = terminal && io::stdout().is_terminal();
    if let Some(session) = &saved
        && terminal
    {
        show_history(session, decorate, settings.response_color)?;
    }

    // A resumed session keeps working in its own folder, wherever wut was
    // started from.
    let cwd = match &saved {
        Some(session) => session_dir(session)?,
        None => current_dir()?,
    };
    let mut settings_cache = settings_ui::Cache;
    let mut agent = open_agent(&settings, config.instructions(), saved.as_ref())?;

    if !terminal {
        // Piped into a bare `wut`: everything piped is one question, and
        // never a command.
        let Some(input) = piped_input()? else {
            return Ok(());
        };
        let question = Question::plain(input.trim());
        return run_turn(
            &mut agent,
            &question.prompt,
            &cwd,
            decorate,
            settings.response_color,
            &mut |answer, reasoning| {
                record_turn(
                    &mut saved,
                    &settings,
                    &cwd,
                    &question.saved,
                    answer.to_owned(),
                    question.reasoning_to_save(reasoning),
                )
            },
        )
        .map(|_| ());
    }

    cancel::install();
    while let prompt::Input::Line(line) = prompt::Prompt::read()? {
        let question = line.trim();
        if question.is_empty() {
            continue;
        }
        if prompt::is_command(question) {
            match parse_interactive_command(question) {
                Ok(InteractiveCommand::Settings) => {
                    settings_ui::run_session(&mut settings, &mut config, &mut settings_cache)?;
                    settings.response_color = config.response_color();
                    if let Some(saved) = &mut saved {
                        saved.settings = Some(session_settings(&settings));
                        state::save(saved)?;
                    }
                    agent.reconfigure(open_agent(
                        &settings,
                        config.instructions(),
                        saved.as_ref(),
                    )?);
                }
                Err(error) => print_interactive_error(&error, true),
            }
            continue;
        }

        match run_turn(
            &mut agent,
            question,
            &cwd,
            decorate,
            settings.response_color,
            &mut |answer, reasoning| {
                record_turn(
                    &mut saved,
                    &settings,
                    &cwd,
                    question,
                    answer.to_owned(),
                    reasoning,
                )
            },
        ) {
            Ok(_) => {}
            Err(_) if cancel::requested() => {
                eprintln!("{}", dim("cancelled", decorate));
                eprintln!();
                continue;
            }
            Err(error) => {
                print_interactive_error(&error, true);
                continue;
            }
        }
        if decorate {
            eprintln!();
        }
    }

    Ok(())
}

fn open_agent(
    settings: &Settings,
    instructions: &Instructions,
    saved: Option<&state::Session>,
) -> Result<agent::Agent> {
    let history = saved.map(|session| session.turns.as_slice()).unwrap_or(&[]);
    agent::Agent::new(
        settings.model.as_deref(),
        settings.reasoning.as_deref(),
        instructions,
        history,
    )
}

fn standalone_settings() -> Result<()> {
    use std::io::{self, IsTerminal};

    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(Error::new(
            "--settings requires an interactive terminal",
            "rerun 'wut --settings' from a terminal",
        ));
    }
    let mut config = config::Config::load()?;
    let mut cache = settings_ui::Cache;
    settings_ui::run_defaults(&mut config, &mut cache)
}

fn run_turn(
    agent: &mut agent::Agent,
    question: &str,
    cwd: &std::path::Path,
    decorate: bool,
    response_color: config::ResponseColor,
    on_finish: &mut dyn FnMut(&str, Option<&str>) -> Result<()>,
) -> Result<String> {
    cancel::reset();
    let json_only = response_format::json_only(question);
    let output = std::cell::RefCell::new(Some(TurnOutput::new(
        decorate && !json_only,
        response_color,
    )));
    let spinner_enabled = decorate && spinner::enabled();
    let spinner = std::cell::RefCell::new(spinner::Spinner::start(spinner_enabled));
    let show_tools = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let answer = agent.ask_with_finish(
        question,
        cwd,
        &mut |delta| {
            spinner.borrow_mut().stop();
            output
                .borrow_mut()
                .as_mut()
                .expect("output is active")
                .push(delta)
        },
        // Every tool call is shown, so a reply never reads files or searches
        // the web out of sight.
        &mut |call| {
            spinner.borrow_mut().stop();
            // Whatever streamed before the first tool call of a round was
            // the model narrating its next step, so set it apart from the
            // answer. Later calls in the round find nothing left to dim.
            output
                .borrow_mut()
                .as_mut()
                .expect("output is active")
                .dim_round()?;
            if show_tools {
                use std::io::Write;
                let mut spinner = spinner.borrow_mut();
                writeln!(
                    std::io::stderr().lock(),
                    "{}",
                    dim(&format!("· {call}"), decorate)
                )
                .map_err(|error| Error::terminal("could not display tool call", error))?;
                *spinner = spinner::Spinner::start(spinner_enabled);
            }
            Ok(())
        },
        &mut |answer, reasoning| {
            spinner.borrow_mut().stop();
            let output = output.borrow_mut().take().expect("output is active");
            if decorate && json_only {
                output.finish(&response_format::terminal_json(answer))?;
            } else {
                output.finish(answer)?;
            }
            cancel::check()?;
            on_finish(answer, reasoning)
        },
    );
    spinner.borrow_mut().stop();
    if let Some(output) = output.into_inner() {
        // Finish partial output after an unsuccessful model request.
        let _ = output.finish("");
    }
    let notice = agent.take_notice();
    let result = answer;
    if let Some(notice) = notice {
        if decorate && result.is_ok() {
            eprintln!();
        }
        notice.print();
    }
    result
}

fn dim(text: &str, decorate: bool) -> String {
    if decorate {
        format!("\x1b[90m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn session_dir(session: &state::Session) -> Result<std::path::PathBuf> {
    let path = std::path::PathBuf::from(&session.cwd);
    if path.is_dir() {
        Ok(path)
    } else {
        Err(Error::new(
            format!("this session's folder no longer exists: {}", session.cwd),
            "start a new session with 'wut'",
        ))
    }
}

fn record_turn(
    saved: &mut Option<state::Session>,
    settings: &Settings,
    cwd: &std::path::Path,
    question: &str,
    answer: String,
    reasoning: Option<&str>,
) -> Result<()> {
    persist_turn(
        saved,
        || {
            let provider = cerebras::find_model(cerebras::resolve_model(settings.model.as_deref()))
                .map_or(cerebras::Provider::Cerebras, |model| model.provider);
            state::Session::new(provider.id(), new_session_id(), cwd)
        },
        state::Turn {
            user: question.to_owned(),
            assistant: answer,
            reasoning: reasoning.map(str::to_owned),
        },
        session_settings(settings),
        state::save,
    )
}

fn persist_turn(
    saved: &mut Option<state::Session>,
    new_session: impl FnOnce() -> state::Session,
    turn: state::Turn,
    settings: state::SessionSettings,
    save: impl FnOnce(&state::Session) -> Result<()>,
) -> Result<()> {
    let existed = saved.is_some();
    let mut session = saved.take().unwrap_or_else(new_session);
    let previous_settings = session.settings.replace(settings);
    let previous_time = session.updated_at;
    session.add_turn(&turn.user, turn.assistant, turn.reasoning);
    let result = save(&session);
    if result.is_err() {
        session.turns.pop();
        session.settings = previous_settings;
        session.updated_at = previous_time;
    }
    if existed || result.is_ok() {
        *saved = Some(session);
    }
    result
}

fn session_settings(settings: &Settings) -> state::SessionSettings {
    state::SessionSettings {
        model: settings.model.clone(),
        reasoning: settings.reasoning.clone(),
    }
}

fn new_session_id() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    format!("{:016x}", hasher.finish())
}

fn show_history(
    session: &state::Session,
    decorate: bool,
    response_color: config::ResponseColor,
) -> Result<()> {
    let elsewhere =
        std::env::current_dir().is_ok_and(|cwd| cwd.to_string_lossy() != session.cwd.as_str());
    eprintln!(
        "Continuing — {} turn{}{}",
        session.turns.len(),
        if session.turns.len() == 1 { "" } else { "s" },
        if elsewhere {
            format!(" in {}", session.cwd)
        } else {
            String::new()
        }
    );

    for turn in &session.turns {
        if decorate {
            prompt::write_submitted(&turn.user)?;
        } else {
            eprintln!("> {}", turn.user);
        }
        let mut output = TurnOutput::new(decorate, response_color);
        output.push(&turn.assistant)?;
        output.finish(&turn.assistant)?;
        if decorate {
            eprintln!();
        }
    }
    Ok(())
}

fn sessions() -> Result<()> {
    use std::io::{self, IsTerminal};

    let mut sessions = state::load_all()?;
    if sessions.is_empty() {
        write_stdout(&[b"No saved sessions.\n"])?;
        return Ok(());
    }

    if !(io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()) {
        return list_sessions(&sessions);
    }

    let (selected, deleted_all) = {
        let _screen = select::Screen::enter()?;
        let mut initial = 0;
        loop {
            let items = session_items(&sessions);
            match select::choose_deletable(
                "Sessions",
                "Choose a session to continue",
                &items,
                initial,
                SESSION_NAME_WIDTH,
            )? {
                select::DeletableChoice::Selected(index) => break (Some(index), false),
                select::DeletableChoice::Deleted(index) => {
                    state::delete(&sessions[index])?;
                    sessions.remove(index);
                    let Some(next) = selection_after_delete(index, sessions.len()) else {
                        break (None, true);
                    };
                    initial = next;
                }
                select::DeletableChoice::Cancelled => break (None, false),
            }
        }
    };
    if deleted_all {
        write_stdout(&[b"No saved sessions.\n"])?;
    }
    let Some(selected) = selected else {
        return Ok(());
    };

    let session = sessions.remove(selected);
    let config = config::Config::load()?;
    let settings = Settings::resolve(&config, Some(&session));
    run_interactive(settings, Some(session), config)
}

fn selection_after_delete(deleted: usize, remaining: usize) -> Option<usize> {
    (remaining > 0).then(|| deleted.min(remaining - 1))
}

fn session_items(sessions: &[state::Session]) -> Vec<select::Item> {
    sessions
        .iter()
        .map(|session| {
            let (label, detail) = session_item_text(session);
            select::Item::new(label, detail)
        })
        .collect()
}

fn session_item_text(session: &state::Session) -> (String, String) {
    let turns = session.turns.len();
    let folder = std::path::Path::new(&session.cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&session.cwd);
    (
        format!("{turns} turn{}", if turns == 1 { "" } else { "s" }),
        format!("{} — {}", folder, age(session.updated_at)),
    )
}

fn list_sessions(sessions: &[state::Session]) -> Result<()> {
    for session in sessions {
        let line = format!(
            "{:>3} turn{}  {:>8}  {}\n",
            session.turns.len(),
            if session.turns.len() == 1 { " " } else { "s" },
            age(session.updated_at),
            session.cwd
        );
        write_stdout(&[line.as_bytes()])?;
    }
    Ok(())
}

fn age(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn current_dir() -> Result<std::path::PathBuf> {
    std::env::current_dir().map_err(|error| {
        Error::new(
            format!("could not determine current directory: {error}"),
            "change to an existing directory and try again",
        )
    })
}

fn print_interactive_error(error: &Error, blank_line: bool) {
    error.print();
    if blank_line {
        eprintln!();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InteractiveCommand, Question, byte_count, parse_interactive_command, selection_after_delete,
    };
    use crate::prompt::is_command;
    use crate::state;

    #[test]
    fn failed_session_saves_restore_existing_sessions_and_leave_new_ones_unsaved() {
        for existed in [false, true] {
            let make_session =
                || state::Session::new("wut", "test".to_owned(), std::path::Path::new("/tmp"));
            let mut saved = existed.then(make_session);
            if let Some(session) = &mut saved {
                session.add_turn("old", "kept".to_owned(), None);
            }
            let before = format!("{saved:?}");
            let error = super::persist_turn(
                &mut saved,
                make_session,
                state::Turn {
                    user: "new".to_owned(),
                    assistant: "unsaved".to_owned(),
                    reasoning: None,
                },
                state::SessionSettings {
                    model: Some("gpt-oss-120b".to_owned()),
                    reasoning: None,
                },
                |_| Err(crate::error::Error::internal("disk full")),
            )
            .unwrap_err();
            assert!(error.message().contains("disk full"));
            assert_eq!(format!("{saved:?}"), before);
        }
    }

    #[test]
    fn model_override_uses_its_own_effort_without_changing_saved_defaults() {
        let mut config = crate::config::Config::default();
        config.set_reasoning(Some("high".to_owned()));
        let settings = super::Settings::resolve(&config, None)
            .with_overrides(Some("groq:openai/gpt-oss-20b"), None)
            .unwrap();
        assert_eq!(settings.reasoning.as_deref(), Some("low"));
        assert_eq!(settings.model.as_deref(), Some("groq:openai/gpt-oss-20b"));
        assert_eq!(config.reasoning(), Some("high"));
        assert_eq!(config.model(), Some("gpt-oss-120b"));
        assert!(
            super::Settings::resolve(&config, None)
                .with_overrides(Some("typo"), None)
                .is_err()
        );
        assert!(
            super::Settings::resolve(&config, None)
                .with_overrides(Some("groq:openai/gpt-oss-20b"), Some("none"))
                .is_err()
        );
    }

    #[test]
    fn only_known_commands_are_commands() {
        assert!(is_command("/settings"));
        assert!(is_command("/settings extra"));
        assert!(!is_command("/etc/hosts what are these perms?"));
        assert!(!is_command("/usr/bin/env -S"));
        assert!(!is_command("what is /settings"));
    }

    #[test]
    fn continued_gemma_sessions_migrate_without_reusing_unsupported_effort() {
        let config = crate::config::Config::default();
        let mut session =
            crate::state::Session::new("wut", "test".to_owned(), std::path::Path::new("/tmp"));
        session.settings = Some(crate::state::SessionSettings {
            model: Some("gemma-4-31b".to_owned()),
            reasoning: Some("none".to_owned()),
        });
        let settings = super::Settings::resolve(&config, Some(&session));
        assert_eq!(settings.model.as_deref(), Some("gpt-oss-120b"));
        assert_eq!(settings.reasoning.as_deref(), Some("low"));
        assert_eq!(
            session.settings.unwrap().model.as_deref(),
            Some("gemma-4-31b")
        );
    }

    #[test]
    fn piped_input_is_context_for_the_question() {
        let question = Question::with_context("why did this fail?".to_owned(), "error: boom\n");
        assert_eq!(
            question.prompt,
            "why did this fail?\n\n```\nerror: boom\n```"
        );
        assert_eq!(
            question.saved,
            "why did this fail? [+12 bytes of piped input]"
        );
        assert_eq!(question.reasoning_to_save(Some("error: boom")), None);
        assert_eq!(
            Question::plain("why?").reasoning_to_save(Some("thinking")),
            Some("thinking")
        );

        let fenced = Question::with_context("explain".to_owned(), "```rust\nfn x() {}\n```");
        assert!(fenced.prompt.starts_with("explain\n\n````\n```rust"));
        assert!(fenced.prompt.ends_with("```\n````"));

        assert_eq!(byte_count(2_048), "2 KiB");
        assert_eq!(byte_count(2_049), "3 KiB");
    }

    #[test]
    fn parses_interactive_commands() {
        assert_eq!(
            parse_interactive_command("/settings").unwrap(),
            InteractiveCommand::Settings
        );
        assert!(parse_interactive_command("/settings extra").is_err());
        assert!(parse_interactive_command("/unknown").is_err());
    }

    #[test]
    fn deletion_keeps_selection_in_bounds() {
        assert_eq!(selection_after_delete(1, 3), Some(1));
        assert_eq!(selection_after_delete(3, 3), Some(2));
        assert_eq!(selection_after_delete(0, 0), None);
    }
}

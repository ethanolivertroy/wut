mod agent;
mod cerebras;
mod cli;
mod config;
mod environment;
mod error;
mod instructions;
mod output;
mod prompt;
mod render;
mod select;
mod settings_ui;
mod spinner;
mod state;
mod storage;
mod terminal;
mod tools;
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
    let mode = cli::parse(std::env::args_os().skip(1))?;
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
                let settings = Settings::resolve(&config, None);
                run_one_shot(&settings, config.instructions(), &question, None)
            }
            cli::Mode::Interactive => {
                let config = config::Config::load()?;
                let settings = Settings::resolve(&config, None);
                run_interactive(settings, None, config)
            }
            cli::Mode::Continue(words) => {
                let config = config::Config::load()?;
                let cwd = current_dir()?;
                let session = state::latest(&cwd)?;
                let settings = Settings::resolve(&config, Some(&session));
                if words.is_empty() {
                    run_interactive(settings, Some(session), config)
                } else {
                    run_one_shot(
                        &settings,
                        config.instructions(),
                        &words.join(" "),
                        Some(session),
                    )
                }
            }
            cli::Mode::Sessions => sessions(),
            cli::Mode::Settings => standalone_settings(),
        }
    })();
    let notice = update_check.notice();
    if result.is_ok()
        && let Some(notice) = notice
    {
        eprintln!("{notice}");
    }
    result
}

fn update_checks_enabled(mode: &cli::Mode) -> bool {
    use std::io::{self, IsTerminal};

    environment::canonical_or_legacy(
        std::env::var_os("WUT_NO_UPDATE_CHECK"),
        std::env::var_os("ASK_NO_UPDATE_CHECK"),
    )
    .is_none()
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && io::stderr().is_terminal()
        && !matches!(
            mode,
            cli::Mode::Help | cli::Mode::Version | cli::Mode::Upgrade | cli::Mode::UpdateCheck
        )
}

pub(crate) struct Settings {
    model: Option<String>,
    reasoning: Option<String>,
    response_color: config::ResponseColor,
}

impl Settings {
    fn resolve(config: &config::Config, session: Option<&state::Session>) -> Self {
        let saved = session.and_then(|session| session.settings.as_ref());
        Self {
            model: saved.map_or_else(
                || config.model().map(str::to_owned),
                |settings| settings.model.clone(),
            ),
            reasoning: saved.map_or_else(
                || config.reasoning().map(str::to_owned),
                |settings| settings.reasoning.clone(),
            ),
            response_color: config.response_color(),
        }
    }
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
    question: &str,
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
    let answer = run_turn(
        &mut agent,
        question,
        &cwd,
        decorate,
        settings.response_color,
    )?;
    record_turn(&mut saved, settings, &cwd, question, answer)?;
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

    let cwd = current_dir()?;
    let mut settings_cache = settings_ui::Cache;
    let mut piped_line = String::new();
    let mut agent = open_agent(&settings, config.instructions(), saved.as_ref())?;

    loop {
        let line = if terminal {
            match prompt::Prompt::read()? {
                prompt::Input::Line(line) => line,
                prompt::Input::Eof => break,
            }
        } else {
            piped_line.clear();
            let bytes = io::stdin().read_line(&mut piped_line).map_err(|error| {
                Error::new(
                    format!("could not read input: {error}"),
                    "check stdin and try again",
                )
            })?;
            if bytes == 0 {
                break;
            }
            piped_line.clone()
        };

        let question = line.trim();
        if question.is_empty() {
            continue;
        }
        if question.starts_with('/') {
            match parse_interactive_command(question) {
                Ok(InteractiveCommand::Settings) if terminal => {
                    settings_ui::run_session(&mut settings, &mut config, &mut settings_cache)?;
                    settings.response_color = config.response_color();
                    if let Some(saved) = &mut saved {
                        saved.settings = Some(session_settings(&settings));
                        state::save(saved)?;
                    }
                    agent = open_agent(&settings, config.instructions(), saved.as_ref())?;
                }
                Ok(InteractiveCommand::Settings) => {
                    print_interactive_error(
                        &Error::new(
                            "/settings requires an interactive terminal",
                            "rerun wut in a terminal",
                        ),
                        terminal,
                    );
                }
                Err(error) => print_interactive_error(&error, terminal),
            }
            continue;
        }

        let answer = match run_turn(
            &mut agent,
            question,
            &cwd,
            decorate,
            settings.response_color,
        ) {
            Ok(answer) => answer,
            Err(error) if terminal => {
                print_interactive_error(&error, true);
                continue;
            }
            Err(error) => return Err(error),
        };
        if decorate {
            eprintln!();
        }
        record_turn(&mut saved, &settings, &cwd, question, answer)?;
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
) -> Result<String> {
    let mut output = TurnOutput::new(decorate, response_color);
    let mut spinner = spinner::Spinner::start(decorate && spinner::enabled());
    let answer = agent.ask(question, cwd, &mut |delta| {
        spinner.stop();
        output.push(delta)
    });
    spinner.stop();
    let answer = answer?;
    output.finish(&answer)?;
    Ok(answer)
}

fn record_turn(
    saved: &mut Option<state::Session>,
    settings: &Settings,
    cwd: &std::path::Path,
    question: &str,
    answer: String,
) -> Result<()> {
    let stored =
        saved.get_or_insert_with(|| state::Session::new("cerebras", new_session_id(), cwd));
    stored.settings = Some(session_settings(settings));
    stored.add_turn(question, answer);
    state::save(stored)
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
    eprintln!(
        "Continuing — {} turn{}",
        session.turns.len(),
        if session.turns.len() == 1 { "" } else { "s" }
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
    use super::{InteractiveCommand, parse_interactive_command, selection_after_delete};

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

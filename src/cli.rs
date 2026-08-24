use std::ffi::OsString;

use crate::error::{Error, Result};

pub const HELP: &str = "wut asks coding agents without letting them change your files.\n\n\
Usage:\n  wut [OPTIONS] [QUESTION...]\n  wut -c [QUESTION...]\n  wut --session ID [QUESTION...]\n  wut agents [--json]\n  wut models [AGENT]\n  wut sessions [--json]\n  wut config [show [--json] | path | set KEY VALUE]\n\n\
Options:\n  -a, --agent ID         choose an agent for a new session\n  -m, --model ID         override the model\n  -r, --reasoning LEVEL  override reasoning effort\n  -c, --continue         continue the latest session in this directory\n      --session ID       continue a saved wut session\n  -h, --help             print help\n  -V, --version          print version\n\n\
With no question, wut starts a plain interactive session. Use -- before a\nquestion that starts with a dash. Answers go to stdout; prompts and errors go\nto stderr.\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ask {
    pub question: Option<String>,
    pub continuation: bool,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigCommand {
    Show { json: bool },
    Path,
    Set { key: String, value: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Ask(Ask),
    Agents { json: bool },
    Models { agent: Option<String> },
    Sessions { json: bool },
    Config(ConfigCommand),
    Help,
    Version,
}

pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Command> {
    let args = args
        .map(|arg| {
            arg.into_string()
                .map_err(|_| Error::usage("arguments must be valid UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;

    match args.first().map(String::as_str) {
        Some("agents") => {
            return parse_json_only("agents", &args[1..], |json| Command::Agents { json });
        }
        Some("models") => return parse_models(&args[1..]),
        Some("sessions") => {
            return parse_json_only("sessions", &args[1..], |json| Command::Sessions { json });
        }
        Some("config") => return parse_config(&args[1..]),
        _ => {}
    }

    parse_ask(&args)
}

fn parse_json_only(
    name: &str,
    args: &[String],
    command: impl FnOnce(bool) -> Command,
) -> Result<Command> {
    match args {
        [] => Ok(command(false)),
        [flag] if flag == "--json" => Ok(command(true)),
        _ => Err(Error::usage(format!(
            "'{name}' accepts only the optional '--json' flag"
        ))),
    }
}

fn parse_models(args: &[String]) -> Result<Command> {
    match args {
        [] => Ok(Command::Models { agent: None }),
        [agent] if !agent.starts_with('-') => Ok(Command::Models {
            agent: Some(agent.clone()),
        }),
        _ => Err(Error::usage("usage: wut models [AGENT]")),
    }
}

fn parse_config(args: &[String]) -> Result<Command> {
    match args {
        [] => Ok(Command::Config(ConfigCommand::Show { json: false })),
        [single] if single == "show" => Ok(Command::Config(ConfigCommand::Show { json: false })),
        [single] if single == "--json" => Ok(Command::Config(ConfigCommand::Show { json: true })),
        [show, json] if show == "show" && json == "--json" => {
            Ok(Command::Config(ConfigCommand::Show { json: true }))
        }
        [single] if single == "path" => Ok(Command::Config(ConfigCommand::Path)),
        [set, key, value] if set == "set" => Ok(Command::Config(ConfigCommand::Set {
            key: key.clone(),
            value: value.clone(),
        })),
        _ => Err(Error::usage(
            "usage: wut config [show [--json] | path | set KEY VALUE]",
        )),
    }
}

fn parse_ask(args: &[String]) -> Result<Command> {
    let mut ask = Ask {
        question: None,
        continuation: false,
        session: None,
        agent: None,
        model: None,
        reasoning: None,
    };
    let mut words = Vec::new();
    let mut options = true;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if options && arg == "--" {
            options = false;
            index += 1;
            continue;
        }
        if options {
            match arg.as_str() {
                "-h" | "--help" => return Ok(Command::Help),
                "-V" | "--version" => return Ok(Command::Version),
                "-c" | "--continue" => {
                    if ask.continuation {
                        return Err(Error::usage("use '--continue' only once"));
                    }
                    ask.continuation = true;
                    index += 1;
                    continue;
                }
                "-a" | "--agent" => {
                    ask.agent = Some(take_value(args, &mut index, arg)?);
                    continue;
                }
                "-m" | "--model" => {
                    ask.model = Some(take_value(args, &mut index, arg)?);
                    continue;
                }
                "-r" | "--reasoning" => {
                    ask.reasoning = Some(take_value(args, &mut index, arg)?);
                    continue;
                }
                "--session" => {
                    ask.session = Some(take_value(args, &mut index, arg)?);
                    continue;
                }
                _ if arg.starts_with("--agent=") => ask.agent = split_value(arg, "--agent=")?,
                _ if arg.starts_with("--model=") => ask.model = split_value(arg, "--model=")?,
                _ if arg.starts_with("--reasoning=") => {
                    ask.reasoning = split_value(arg, "--reasoning=")?
                }
                _ if arg.starts_with("--session=") => ask.session = split_value(arg, "--session=")?,
                _ if arg.starts_with('-') => {
                    return Err(Error::usage(format!("unknown option '{arg}'")));
                }
                _ => options = false,
            }
        }
        words.push(arg.clone());
        index += 1;
    }

    if ask.continuation && ask.session.is_some() {
        return Err(Error::usage(
            "--continue and --session cannot be used together",
        ));
    }
    if !words.is_empty() {
        ask.question = Some(words.join(" "));
    }
    Ok(Command::Ask(ask))
}

fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String> {
    *index += 1;
    let value = args
        .get(*index)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::usage(format!("'{option}' requires a value")))?
        .clone();
    *index += 1;
    Ok(value)
}

fn split_value(arg: &str, prefix: &str) -> Result<Option<String>> {
    let value = &arg[prefix.len()..];
    if value.is_empty() {
        Err(Error::usage(format!(
            "'{}' requires a value",
            prefix.trim_end_matches('=')
        )))
    } else {
        Ok(Some(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{Ask, Command, ConfigCommand, parse};

    fn args(values: &[&str]) -> impl Iterator<Item = OsString> {
        values
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_one_shot_with_overrides() {
        assert_eq!(
            parse(args(&["-a", "cursor", "-m", "grok-fast", "why?"])).unwrap(),
            Command::Ask(Ask {
                question: Some("why?".into()),
                continuation: false,
                session: None,
                agent: Some("cursor".into()),
                model: Some("grok-fast".into()),
                reasoning: None,
            })
        );
    }

    #[test]
    fn parses_dash_prefixed_words_after_question_starts() {
        let command = parse(args(&["compare", "-O2", "and", "-O3"])).unwrap();
        let Command::Ask(ask) = command else {
            panic!("expected ask command");
        };
        assert_eq!(ask.question.as_deref(), Some("compare -O2 and -O3"));
    }

    #[test]
    fn parses_continue_and_explicit_session() {
        assert!(matches!(parse(args(&["-c"])).unwrap(), Command::Ask(_)));
        assert!(matches!(
            parse(args(&["--session", "cursor-deadbeef", "next"])).unwrap(),
            Command::Ask(_)
        ));
        assert!(parse(args(&["-c", "--session", "x"])).is_err());
    }

    #[test]
    fn parses_scriptable_commands() {
        assert_eq!(
            parse(args(&["config", "set", "agent", "grok"])).unwrap(),
            Command::Config(ConfigCommand::Set {
                key: "agent".into(),
                value: "grok".into(),
            })
        );
        assert_eq!(
            parse(args(&["sessions", "--json"])).unwrap(),
            Command::Sessions { json: true }
        );
    }
}

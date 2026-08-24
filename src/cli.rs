use std::ffi::OsString;

use crate::error::{Error, Result};

pub const HELP: &str = concat!(
    "Ask, then do. Follow up when you need to.\n",
    "\n",
    "Usage:\n",
    "  wut [QUESTION...]\n",
    "  wut -c [QUESTION...]\n",
    "  wut --sessions\n",
    "  wut --settings\n",
    "  wut --upgrade\n",
    "\n",
    "With no question, wut starts an interactive session.\n",
    "\n",
    "Options:\n",
    "  -c, --continue      Continue the latest session here\n",
    "  -s, --sessions      Choose a saved session to continue\n",
    "      --settings      Configure defaults and instructions\n",
    "      --upgrade       Upgrade to the latest release\n",
    "  -h, --help          Print help\n",
    "  -V, --version       Print version\n",
);

#[derive(Debug, Eq, PartialEq)]
pub enum Mode {
    Interactive,
    OneShot(String),
    Continue(Vec<String>),
    Sessions,
    Settings,
    Upgrade,
    UpdateCheck,
    Help,
    Version,
}

pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Mode> {
    let mut words = Vec::new();
    let mut options = true;
    let mut resume = false;
    let mut sessions = false;
    let mut settings = false;
    let mut upgrade = false;
    let mut update_check = false;
    for arg in args {
        let arg = arg
            .into_string()
            .map_err(|_| Error::usage("arguments must be valid UTF-8"))?;

        if options {
            match arg.as_str() {
                "--" => {
                    options = false;
                    continue;
                }
                "-h" | "--help" => {
                    return Ok(Mode::Help);
                }
                "-V" | "--version" => {
                    return Ok(Mode::Version);
                }
                "-c" | "--continue" => {
                    if resume {
                        return Err(Error::usage("use '--continue' only once"));
                    }
                    resume = true;
                    continue;
                }
                "-s" | "--sessions" => {
                    sessions = true;
                    continue;
                }
                "--settings" => {
                    settings = true;
                    continue;
                }
                "--upgrade" => {
                    if upgrade {
                        return Err(Error::usage("use '--upgrade' only once"));
                    }
                    upgrade = true;
                    continue;
                }
                "--internal-update-check" => {
                    if update_check {
                        return Err(Error::usage("use '--internal-update-check' only once"));
                    }
                    update_check = true;
                    continue;
                }
                _ if arg.starts_with('-') => {
                    return Err(Error::usage(format!("unknown option '{arg}'")));
                }
                _ => {}
            }
        }

        words.push(arg);
        options = false;
    }

    if sessions && resume {
        return Err(Error::usage(
            "--sessions cannot be combined with --continue",
        ));
    }
    if sessions && !words.is_empty() {
        return Err(Error::new(
            "--sessions does not accept a question",
            "run 'wut --sessions' by itself",
        ));
    }
    if settings && (sessions || resume || !words.is_empty()) {
        return Err(Error::new(
            "--settings cannot be combined with other arguments",
            "run 'wut --settings' by itself",
        ));
    }
    if upgrade && (settings || sessions || resume || !words.is_empty()) {
        return Err(Error::new(
            "--upgrade cannot be combined with other arguments",
            "run 'wut --upgrade' by itself",
        ));
    }
    if update_check && (upgrade || settings || sessions || resume || !words.is_empty()) {
        return Err(Error::usage(
            "--internal-update-check cannot be combined with other arguments",
        ));
    }
    let mode = if update_check {
        Mode::UpdateCheck
    } else if upgrade {
        Mode::Upgrade
    } else if settings {
        Mode::Settings
    } else if sessions {
        Mode::Sessions
    } else if resume {
        Mode::Continue(words)
    } else if words.is_empty() {
        Mode::Interactive
    } else {
        Mode::OneShot(words.join(" "))
    };

    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::{Mode, parse};
    use std::ffi::OsString;

    fn args(values: &[&str]) -> impl Iterator<Item = OsString> {
        values
            .iter()
            .map(|value| OsString::from(*value))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn help_uses_only_the_wut_command_identity() {
        assert!(super::HELP.contains("  wut [QUESTION...]"));
        assert!(super::HELP.contains("  wut --upgrade"));
        assert!(!super::HELP.contains("  ask "));
    }

    #[test]
    fn no_arguments_selects_interactive_mode() {
        assert_eq!(parse(args(&[])).unwrap(), Mode::Interactive);
    }

    #[test]
    fn words_are_joined_into_one_question() {
        assert_eq!(
            parse(args(&["why", "is", "this?"])).unwrap(),
            Mode::OneShot("why is this?".into()),
        );
    }

    #[test]
    fn separator_allows_question_starting_with_dash() {
        assert_eq!(
            parse(args(&["--", "--why?"])).unwrap(),
            Mode::OneShot("--why?".into()),
        );
    }

    #[test]
    fn parses_continue() {
        assert_eq!(parse(args(&["-c"])).unwrap(), Mode::Continue(Vec::new()));
    }

    #[test]
    fn parses_sessions_shorthand() {
        assert_eq!(parse(args(&["-s"])).unwrap(), Mode::Sessions);
    }

    #[test]
    fn parses_standalone_settings() {
        assert_eq!(parse(args(&["--settings"])).unwrap(), Mode::Settings);
        assert!(parse(args(&["-S"])).is_err());
        assert!(parse(args(&["--settings", "question"])).is_err());
    }

    #[test]
    fn parses_version_flags() {
        assert_eq!(parse(args(&["-V"])).unwrap(), Mode::Version);
        assert_eq!(parse(args(&["--version"])).unwrap(), Mode::Version);
    }

    #[test]
    fn internal_update_worker_must_be_standalone() {
        assert_eq!(
            parse(args(&["--internal-update-check"])).unwrap(),
            Mode::UpdateCheck
        );
        assert!(parse(args(&["--internal-update-check", "question"])).is_err());
        assert!(parse(args(&["--internal-update-check", "--upgrade"])).is_err());
    }

    #[test]
    fn upgrade_must_be_standalone() {
        assert_eq!(parse(args(&["--upgrade"])).unwrap(), Mode::Upgrade);
        assert!(parse(args(&["--upgrade", "now"])).is_err());
        assert!(parse(args(&["--upgrade", "-c"])).is_err());
        assert!(parse(args(&["--upgrade", "--settings"])).is_err());
        assert!(parse(args(&["--upgrade", "--upgrade"])).is_err());
        assert_eq!(
            parse(args(&["--", "--upgrade"])).unwrap(),
            Mode::OneShot("--upgrade".into())
        );
    }

    #[test]
    fn continue_keeps_follow_up_words() {
        assert_eq!(
            parse(args(&["-c", "what", "next?"])).unwrap(),
            Mode::Continue(vec!["what".into(), "next?".into()])
        );
    }

    #[test]
    fn continue_id_syntax_is_not_supported() {
        assert!(parse(args(&["--continue=abc"])).is_err());
    }

    #[test]
    fn history_flag_is_not_supported() {
        assert!(parse(args(&["-c", "--history"])).is_err());
    }

    #[test]
    fn unknown_option_is_an_error() {
        assert!(parse(args(&["--wat"])).is_err());
    }

    #[test]
    fn option_like_words_after_the_question_are_prompt_text() {
        assert_eq!(
            parse(args(&["hello", "--agent", "claude"])).unwrap(),
            Mode::OneShot("hello --agent claude".into())
        );
        assert_eq!(
            parse(args(&["hello", "--model=gpt-5.6-luna"])).unwrap(),
            Mode::OneShot("hello --model=gpt-5.6-luna".into())
        );
        assert_eq!(
            parse(args(&["hello", "--reasoning=low"])).unwrap(),
            Mode::OneShot("hello --reasoning=low".into())
        );
    }
}

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

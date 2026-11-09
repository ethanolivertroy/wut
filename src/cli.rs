use std::ffi::OsString;

use crate::cerebras::Provider;
use crate::error::{Error, Result};

pub const HELP: &str = concat!(
    "Ask, then do. Follow up when you need to.\n",
    "\n",
    "Usage:\n",
    "  wut [QUESTION...]\n",
    "  wut -c [QUESTION...]\n",
    "  wut --sessions\n",
    "  wut --settings\n",
    "  wut --set-key [cerebras|groq|ai-gateway]\n",
    "  wut --models\n",
    "  wut --benchmark [--model MODEL]\n",
    "  wut --upgrade\n",
    "\n",
    "With no question, wut starts an interactive session.\n",
    "\n",
    "Options:\n",
    "  -c, --continue      Continue the latest session here\n",
    "  -s, --sessions      Choose a saved session to continue\n",
    "  -m, --model MODEL   Use a model ID from --models\n",
    "      --reasoning LEVEL  Set this invocation's reasoning level\n",
    "      --models        List models and required API key variables\n",
    "      --benchmark     Measure first-text and total response time\n",
    "      --settings      Configure defaults and instructions\n",
    "      --set-key       Save an API key using hidden input\n",
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
    SetKey(Option<Provider>),
    Upgrade,
    UpdateCheck,
    Help,
    Version,
    Models,
    Benchmark,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Invocation {
    pub mode: Mode,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

impl Invocation {
    fn plain(mode: Mode) -> Self {
        Self {
            mode,
            model: None,
            reasoning: None,
        }
    }
}

pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Invocation> {
    let args: Vec<_> = args.collect();
    // Handle credential setup before ordinary option errors or early help
    // returns. A mistakenly supplied key must never be echoed in an error.
    if args
        .iter()
        .take_while(|arg| arg.as_os_str() != "--")
        .any(|arg| {
            arg == "--set-key"
                || arg
                    .to_str()
                    .is_some_and(|arg| arg.starts_with("--set-key="))
        })
    {
        return parse_set_key(&args);
    }
    let mut args = args.into_iter();
    let mut words = Vec::new();
    let mut options = true;
    let mut resume = false;
    let mut sessions = false;
    let mut settings = false;
    let mut upgrade = false;
    let mut update_check = false;
    let mut models = false;
    let mut benchmark = false;
    let mut model = None;
    let mut reasoning = None;
    while let Some(arg) = args.next() {
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
                    return Ok(Invocation::plain(Mode::Help));
                }
                "-V" | "--version" => {
                    return Ok(Invocation::plain(Mode::Version));
                }
                "--model" | "-m" | "--reasoning" => {
                    let flag = if arg == "--reasoning" {
                        "--reasoning"
                    } else {
                        "--model"
                    };
                    let value = args
                        .next()
                        .and_then(|value| value.into_string().ok())
                        .filter(|value| !value.is_empty() && !value.starts_with('-'))
                        .ok_or_else(|| Error::usage(format!("{flag} requires a value")))?;
                    set_option(
                        if flag == "--model" {
                            &mut model
                        } else {
                            &mut reasoning
                        },
                        value,
                        flag,
                    )?;
                    continue;
                }
                "--models" => {
                    models = true;
                    continue;
                }
                "--benchmark" => {
                    benchmark = true;
                    continue;
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
                _ if arg.starts_with("--model=") => {
                    set_option(&mut model, arg[8..].to_owned(), "--model")?;
                    continue;
                }
                _ if arg.starts_with("--reasoning=") => {
                    set_option(&mut reasoning, arg[12..].to_owned(), "--reasoning")?;
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

    let overrides = model.is_some() || reasoning.is_some();
    if models
        && (benchmark
            || settings
            || upgrade
            || update_check
            || sessions
            || resume
            || !words.is_empty()
            || overrides)
    {
        return Err(Error::usage("--models must be used by itself"));
    }
    if benchmark && (settings || upgrade || update_check || sessions || resume || !words.is_empty())
    {
        return Err(Error::usage(
            "--benchmark accepts only --model and --reasoning",
        ));
    }
    if overrides && (settings || upgrade || update_check || sessions) {
        return Err(Error::usage(
            "--model and --reasoning apply to a question, chat, continuation, or benchmark",
        ));
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
    let mode = if models {
        Mode::Models
    } else if benchmark {
        Mode::Benchmark
    } else if update_check {
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

    Ok(Invocation {
        mode,
        model,
        reasoning,
    })
}

fn parse_set_key(args: &[OsString]) -> Result<Invocation> {
    let invalid = || {
        Error::new(
            "--set-key accepts only an optional provider, with no other arguments",
            "run 'wut --set-key [cerebras|groq|ai-gateway]'; enter the key only at the hidden prompt",
        )
    };
    if args.first().is_none_or(|arg| arg != "--set-key") || args.len() > 2 {
        return Err(invalid());
    }
    let provider = match args.get(1).and_then(|arg| arg.to_str()) {
        None if args.len() == 1 => None,
        Some("cerebras") => Some(Provider::Cerebras),
        Some("groq") => Some(Provider::Groq),
        Some("ai-gateway") => Some(Provider::AiGateway),
        _ => return Err(invalid()),
    };
    Ok(Invocation::plain(Mode::SetKey(provider)))
}

fn set_option(slot: &mut Option<String>, value: String, flag: &str) -> Result<()> {
    if slot.is_some() || value.is_empty() {
        return Err(Error::usage(format!(
            "use {flag} once with a nonempty value"
        )));
    }
    *slot = Some(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Invocation, Mode, parse};
    use crate::cerebras::Provider;

    fn args(values: &[&str]) -> crate::error::Result<Invocation> {
        parse(values.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn model_and_reasoning_overrides_support_questions_and_continuations() {
        let parsed = args(&[
            "--model",
            "groq:openai/gpt-oss-20b",
            "--reasoning=low",
            "why",
            "is",
            "this?",
        ])
        .unwrap();
        assert_eq!(parsed.mode, Mode::OneShot("why is this?".to_owned()));
        assert_eq!(parsed.model.as_deref(), Some("groq:openai/gpt-oss-20b"));
        assert_eq!(parsed.reasoning.as_deref(), Some("low"));
        let parsed = args(&["-c", "--model=gpt-oss-120b", "again"]).unwrap();
        assert_eq!(parsed.mode, Mode::Continue(vec!["again".to_owned()]));
        assert_eq!(parsed.model.as_deref(), Some("gpt-oss-120b"));
        assert_eq!(
            args(&["-m", "gpt-oss-120b"]).unwrap().mode,
            Mode::Interactive
        );
    }

    #[test]
    fn model_listing_is_offline_and_benchmarks_accept_overrides() {
        assert_eq!(args(&["--models"]).unwrap().mode, Mode::Models);
        assert_eq!(
            args(&["--benchmark", "--model", "gpt-oss-120b"])
                .unwrap()
                .mode,
            Mode::Benchmark
        );
        for values in [
            vec!["--models", "question"],
            vec!["--benchmark", "question"],
            vec!["--models", "--benchmark"],
            vec!["--settings", "--model", "x"],
            vec!["--model"],
            vec!["--model="],
            vec!["--reasoning", "--models"],
            vec!["-m", "x", "--model", "y"],
        ] {
            assert!(args(&values).is_err(), "{values:?}");
        }
    }

    #[test]
    fn question_words_and_explicit_separator_preserve_literal_flags() {
        assert_eq!(
            args(&["explain", "--model", "please"]).unwrap().mode,
            Mode::OneShot("explain --model please".to_owned())
        );
        assert_eq!(
            args(&["--", "--models"]).unwrap().mode,
            Mode::OneShot("--models".to_owned())
        );
        assert_eq!(args(&[]).unwrap().mode, Mode::Interactive);
        assert_eq!(args(&["--help"]).unwrap().mode, Mode::Help);
    }

    #[test]
    fn set_key_accepts_each_provider_or_opens_the_chooser() {
        assert_eq!(
            args(&["--set-key"]).unwrap(),
            Invocation::plain(Mode::SetKey(None))
        );
        for (name, provider) in [
            ("cerebras", Provider::Cerebras),
            ("groq", Provider::Groq),
            ("ai-gateway", Provider::AiGateway),
        ] {
            assert_eq!(
                args(&["--set-key", name]).unwrap(),
                Invocation::plain(Mode::SetKey(Some(provider)))
            );
        }
    }

    #[test]
    fn set_key_rejects_combined_modes_and_overrides() {
        for other in [
            "--continue",
            "--sessions",
            "--settings",
            "--models",
            "--benchmark",
            "--upgrade",
            "--internal-update-check",
            "--help",
            "--version",
            "--set-key",
            "--model=gpt-oss-120b",
            "--reasoning=low",
            "question",
        ] {
            assert!(args(&["--set-key", other]).is_err());
            assert!(args(&[other, "--set-key"]).is_err());
            assert!(args(&["--set-key", "cerebras", other]).is_err());
        }
        assert!(args(&["--set-key", "--", "ai-gateway"]).is_err());
    }

    #[test]
    fn invalid_key_setup_arguments_are_never_echoed() {
        let secret = "sk-test-not-a-real-key";
        let inline = format!("--set-key={secret}");
        let unknown_flag = format!("--{secret}");
        for values in [
            vec!["--set-key", secret],
            vec!["--set-key", "cerebras", secret],
            vec!["--set-key", "groq", secret],
            vec!["--set-key", "ai-gateway", secret],
            vec![secret, "--set-key"],
            vec![inline.as_str()],
            vec!["--set-key", unknown_flag.as_str()],
            vec![unknown_flag.as_str(), "--set-key"],
            vec!["--help", "--set-key", secret],
            vec!["--model", secret, "--set-key"],
            vec!["--reasoning", secret, "--set-key"],
        ] {
            let error = args(&values).unwrap_err();
            assert!(!error.message().contains(secret));
            assert!(!error.help().contains(secret));
            assert!(error.help().contains("hidden prompt"));
        }
    }

    #[test]
    fn an_explicit_separator_keeps_key_setup_text_in_a_question() {
        assert_eq!(
            args(&["--", "--set-key", "meaning"]).unwrap().mode,
            Mode::OneShot("--set-key meaning".to_owned())
        );
    }
}

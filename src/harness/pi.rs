use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::fast_model;
use super::{Harness, Model, ReasoningLevel, Response, RunOptions, bounded_output, capture_stderr};
use crate::error::{Error, Result};

const READ_ONLY_TOOLS: &str = "read,grep,find,ls";
const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh"];

pub(super) struct Pi {
    program: OsString,
    alias: fast_model::Alias,
}

impl Pi {
    pub(super) fn new(program: OsString) -> Self {
        Self {
            program,
            alias: fast_model::Alias::new("pi"),
        }
    }

    fn command(
        program: &OsStr,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Command {
        let mut command = Command::new(program);
        command.args([
            "--mode",
            "json",
            "--print",
            "--tools",
            READ_ONLY_TOOLS,
            "--no-extensions",
        ]);
        if let Some(session_id) = session_id {
            command.args(["--session", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if let Some(reasoning) = options.reasoning {
            command.args(["--thinking", reasoning]);
        }
        if let Some(instructions) = options.instructions {
            command.args(["--append-system-prompt", instructions]);
        }
        command.arg("--").arg(question);
        command
    }
}

impl Harness for Pi {
    fn models(&mut self) -> Result<Vec<Model>> {
        let models = catalog(&self.program)?;
        let target = fastest_model(&models).cloned();
        Ok(fast_model::with_alias(models, target.as_ref()))
    }

    fn run(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let program = self.program.as_os_str();
        let run_once = |model: Option<&str>, on_delta: &mut dyn FnMut(&str) -> Result<()>| {
            let options = RunOptions { model, ..options };
            run_once(program, question, session_id, &options, on_delta)
        };
        if options.model != Some(fast_model::ALIAS) {
            return run_once(options.model, on_delta);
        }
        let refresh = || {
            let models = catalog(program)?;
            fastest_model(&models)
                .map(|model| model.id.clone())
                .ok_or_else(no_fast_provider)
        };
        self.alias.run(
            &refresh,
            &mut |model, on_delta| run_once(Some(model), on_delta),
            on_delta,
        )
    }
}

fn catalog(program: &OsStr) -> Result<Vec<Model>> {
    let mut command = Command::new(program);
    command.arg("--list-models");
    let output = bounded_output(&mut command).map_err(start_error)?;
    if !output.status.success() {
        return Err(command_error("could not list Pi models", &output.stderr));
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|_| Error::agent("pi", "Pi returned a model list that was not valid UTF-8"))?;
    parse_models(&output)
}

fn run_once(
    program: &OsStr,
    question: &str,
    session_id: Option<&str>,
    options: &RunOptions<'_>,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut child = Pi::command(program, question, session_id, options)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(start_error)?;
    let stdout = child.stdout.take().expect("piped stdout is available");
    let stderr = child.stderr.take().expect("piped stderr is available");
    let stderr_reader = std::thread::spawn(move || capture_stderr(stderr));

    let result = read_events(BufReader::new(stdout), on_delta);
    let status = child.wait().map_err(|error| {
        Error::new(
            format!("could not wait for Pi: {error}"),
            "restart wut and try again",
        )
    })?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| {
            Error::new(
                "could not read Pi error output",
                "restart wut and try again",
            )
        })?
        .into_detail();

    if !status.success() {
        return Err(command_error("Pi failed", &stderr));
    }
    result
}

/// Pi lists whichever providers have keys; `fast` means Cerebras, then Groq.
fn fastest_model(models: &[Model]) -> Option<&Model> {
    fast_model::cerebras_model(models)
        .or_else(|| models.iter().find(|model| model.id.starts_with("groq/")))
}

fn no_fast_provider() -> Error {
    Error::new(
        "Pi has no Cerebras or Groq models available",
        "set CEREBRAS_API_KEY (or GROQ_API_KEY) for Pi, or pick a model in 'wut --settings'",
    )
}

fn parse_models(output: &str) -> Result<Vec<Model>> {
    let mut models = output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let provider = columns.next()?;
            let id = columns.next()?;
            let context = columns.next()?;
            let _max_output = columns.next()?;
            let thinking = columns.next()?;
            let _images = columns.next()?;
            let reasoning = if thinking == "yes" {
                THINKING_LEVELS
                    .iter()
                    .map(|level| ReasoningLevel {
                        id: (*level).to_owned(),
                        description: String::new(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Some(Model {
                id: format!("{provider}/{id}"),
                name: id.to_owned(),
                description: format!("{provider} — {context} context"),
                is_default: false,
                reasoning,
                default_reasoning: None,
            })
        })
        .collect::<Vec<_>>();

    if let Some(first) = models.first_mut() {
        first.is_default = true;
    }
    if models.is_empty() {
        Err(Error::agent("pi", "Pi did not report any available models"))
    } else {
        Ok(models)
    }
}

fn read_events(
    reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut session_id = None;
    let mut answer = None;
    let mut reported_error = None;

    for line in reader.lines() {
        let line = line
            .map_err(|error| Error::agent("pi", format!("could not read Pi response: {error}")))?;
        let event: Value = serde_json::from_str(&line)
            .map_err(|error| Error::agent("pi", format!("could not parse Pi response: {error}")))?;
        match event.get("type").and_then(Value::as_str) {
            Some("session") => {
                session_id = event.get("id").and_then(Value::as_str).map(str::to_owned);
            }
            Some("message_update")
                if event["assistantMessageEvent"]["type"].as_str() == Some("text_delta") =>
            {
                if let Some(delta) = event["assistantMessageEvent"]["delta"].as_str() {
                    on_delta(delta)?;
                }
            }
            Some("message_end") if event["message"]["role"].as_str() == Some("assistant") => {
                if matches!(
                    event["message"]["stopReason"].as_str(),
                    Some("error" | "aborted")
                ) {
                    reported_error = event["message"]["errorMessage"].as_str().map(str::to_owned);
                }
                let text = event["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|content| content["type"].as_str() == Some("text"))
                    .filter_map(|content| content["text"].as_str())
                    .collect::<String>();
                if !text.is_empty() {
                    answer = Some(text);
                }
            }
            _ => {}
        }
    }

    if let Some(error) = reported_error {
        return Err(Error::agent("pi", format!("Pi reported an error: {error}")));
    }
    Ok(Response {
        answer: answer
            .ok_or_else(|| Error::agent("pi", "Pi completed without returning an answer"))?,
        session_id: session_id
            .ok_or_else(|| Error::agent("pi", "Pi completed without returning a session ID"))?,
    })
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "Pi is not installed or not on PATH",
            "install it, authenticate, then try again",
        )
    } else {
        Error::agent("pi", format!("could not start Pi: {error}"))
    }
}

fn command_error(message: &str, stderr: &str) -> Error {
    let detail = stderr.trim();
    let message = if detail.is_empty() {
        message.to_owned()
    } else {
        format!("{message}: {detail}")
    };
    Error::agent("pi", message)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io::Cursor;

    use super::{Pi, fastest_model, parse_models, read_events};
    use crate::harness::RunOptions;

    #[test]
    fn combines_read_only_tools_with_answer_instructions() {
        let command = Pi::command(
            OsStr::new("pi"),
            "hello",
            None,
            &RunOptions {
                model: None,
                reasoning: None,
                instructions: Some("Be concise."),
            },
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|args| args == ["--tools", "read,grep,find,ls"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--append-system-prompt", "Be concise."])
        );
    }

    #[test]
    fn dash_prefixed_question_follows_option_terminator() {
        let command = Pi::command(
            OsStr::new("pi"),
            "-why did this fail",
            None,
            &RunOptions {
                model: None,
                reasoning: None,
                instructions: None,
            },
        );
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
    fn parses_model_table() {
        let models = parse_models(
            "provider model context max-out thinking images\nopenai gpt-fast 128K 32K yes yes\nanthropic haiku 200K 8K no yes\n",
        )
        .unwrap();

        assert_eq!(models[0].id, "openai/gpt-fast");
        assert!(models[0].is_default);
        assert_eq!(models[0].reasoning.len(), 6);
        assert!(!models[1].is_default);
        assert!(models[1].reasoning.is_empty());
    }

    #[test]
    fn fast_means_cerebras_then_groq() {
        let models = parse_models(concat!(
            "provider model context max-out thinking images\n",
            "anthropic claude-sonnet 200K 8K no yes\n",
            "cerebras gemma-4-31b 131K 40K yes yes\n",
            "cerebras gpt-oss-120b 131K 40K yes no\n",
            "groq llama-3.3-70b-versatile 128K 32K no no\n",
        ))
        .unwrap();
        let fastest = fastest_model(&models).unwrap();
        assert_eq!(fastest.id, "cerebras/gpt-oss-120b");
        assert_eq!(fastest.reasoning.len(), 6);

        let groq_only = parse_models(concat!(
            "provider model context max-out thinking images\n",
            "groq llama-3.3-70b-versatile 128K 32K no no\n",
        ))
        .unwrap();
        assert_eq!(
            fastest_model(&groq_only).unwrap().id,
            "groq/llama-3.3-70b-versatile"
        );
        assert!(fastest_model(&models[..1]).is_none());
    }

    #[test]
    fn streams_text_and_extracts_the_final_response() {
        let input = concat!(
            "{\"type\":\"session\",\"id\":\"session-1\"}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"hel\"}}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"lo\"}}\n",
            "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"stop\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n"
        );
        let mut streamed = String::new();
        let response = read_events(Cursor::new(input), &mut |delta| {
            streamed.push_str(delta);
            Ok(())
        })
        .unwrap();

        assert_eq!(streamed, "hello");
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "session-1");
    }
}

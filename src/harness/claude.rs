use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::{Command, ExitStatus, Stdio};

use serde_json::Value;

use super::{Harness, Model, Response, RunOptions, bounded_output, capture_stderr};
use crate::error::{Error, Result};

pub(super) struct Claude {
    program: OsString,
}

struct StreamFailure {
    error: Error,
    /// A result event was parsed; the error is Claude's own report rather
    /// than a truncated or unreadable stream.
    completed: bool,
}

fn incomplete(error: Error) -> StreamFailure {
    StreamFailure {
        error,
        completed: false,
    }
}

impl Claude {
    pub(super) fn new(program: OsString) -> Self {
        Self { program }
    }

    fn command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Command {
        // --verbose is required by Claude Code for stream-json in print mode.
        self.command_with_format(
            question,
            session_id,
            options,
            &[
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
            ],
        )
    }

    fn legacy_command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Command {
        self.command_with_format(question, session_id, options, &["--output-format", "json"])
    }

    fn command_with_format(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
        format_args: &[&str],
    ) -> Command {
        let mut command = Command::new(&self.program);
        command.args(["--print", "--permission-mode", "plan"]);
        command.args(format_args);
        if let Some(session_id) = session_id {
            command.args(["--resume", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if let Some(instructions) = options.instructions {
            command.args(["--append-system-prompt", instructions]);
        }
        command.arg("--").arg(question);
        command
    }

    fn run_legacy(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Result<Response> {
        let mut command = self.legacy_command(question, session_id, options);
        let output = bounded_output(&mut command).map_err(start_error)?;

        let stdout = String::from_utf8(output.stdout).map_err(|_| {
            Error::agent(
                "claude",
                "Claude Code returned output that was not valid UTF-8",
            )
        })?;
        let stderr = output.stderr;

        let response =
            parse_response(&stdout).map_err(|error| with_stderr_detail(error, &stderr))?;

        if !output.status.success() {
            return Err(status_failure(output.status, &stderr));
        }

        Ok(response)
    }
}

impl Harness for Claude {
    fn models(&mut self) -> Result<Vec<Model>> {
        Ok([("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")]
            .into_iter()
            .map(|(id, name)| Model {
                id: id.into(),
                name: name.into(),
                description: format!("Claude Code's latest {name} model"),
                is_default: false,
                reasoning: Vec::new(),
                default_reasoning: None,
            })
            .collect())
    }

    fn run(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        if options.reasoning.is_some() {
            return Err(Error::new(
                "reasoning control is not supported for Claude Code",
                "choose Model default reasoning in 'wut --settings' and try again",
            ));
        }

        let mut child = self
            .command(question, session_id, &options)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(start_error)?;
        let stdout = child.stdout.take().expect("piped stdout is available");
        let stderr = child.stderr.take().expect("piped stderr is available");
        let stderr_reader = std::thread::spawn(move || capture_stderr(stderr));

        let mut streamed = false;
        let result = read_events(BufReader::new(stdout), &mut |delta| {
            streamed = true;
            on_delta(delta)
        });
        let status = child.wait().map_err(|error| {
            Error::agent("claude", format!("could not wait for Claude Code: {error}"))
        })?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| Error::agent("claude", "could not read Claude Code error output"))?
            .into_detail();

        match result {
            Ok(response) => {
                if !status.success() {
                    return Err(status_failure(status, &stderr));
                }
                Ok(response)
            }
            Err(failure) if failure.completed => Err(with_stderr_detail(failure.error, &stderr)),
            Err(failure) => {
                if !status.success() {
                    // Older Claude Code builds reject the streaming flags;
                    // fall back to the buffered JSON format once.
                    if !streamed && unsupported_option(&stderr) {
                        return self.run_legacy(question, session_id, &options);
                    }
                    return Err(status_failure(status, &stderr));
                }
                Err(with_stderr_detail(failure.error, &stderr))
            }
        }
    }
}

fn read_events(
    reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> std::result::Result<Response, StreamFailure> {
    for line in reader.lines() {
        let line = line.map_err(|error| {
            incomplete(Error::agent(
                "claude",
                format!("could not read Claude Code response: {error}"),
            ))
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(&line).map_err(|error| {
            incomplete(Error::agent(
                "claude",
                format!("could not parse Claude Code response: {error}"),
            ))
        })?;
        match event.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                let delta = &event["event"]["delta"];
                if event["event"]["type"].as_str() == Some("content_block_delta")
                    && delta["type"].as_str() == Some("text_delta")
                    && let Some(text) = delta["text"].as_str()
                {
                    on_delta(text).map_err(incomplete)?;
                }
            }
            Some("result") => {
                return parse_result(&event).map_err(|error| StreamFailure {
                    error,
                    completed: true,
                });
            }
            _ => {}
        }
    }

    Err(incomplete(Error::agent(
        "claude",
        "Claude Code completed without returning an answer",
    )))
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "Claude Code is not installed or not on PATH",
            "install it, authenticate, then try again",
        )
    } else {
        Error::agent("claude", format!("could not start Claude Code: {error}"))
    }
}

fn status_failure(status: ExitStatus, stderr: &str) -> Error {
    let detail = stderr.trim();
    let message = if detail.is_empty() {
        format!("Claude Code exited with {status}")
    } else {
        format!("Claude Code failed: {detail}")
    };
    Error::agent("claude", message)
}

fn with_stderr_detail(error: Error, stderr: &str) -> Error {
    let detail = stderr.trim();
    if detail.is_empty() {
        error
    } else {
        error.detail(detail)
    }
}

fn unsupported_option(stderr: &str) -> bool {
    let stderr = stderr.to_lowercase();
    stderr.contains("unknown option")
        || stderr.contains("unrecognized option")
        || stderr.contains("unexpected argument")
}

fn parse_response(output: &str) -> Result<Response> {
    let response: Value = serde_json::from_str(output).map_err(|error| {
        Error::agent(
            "claude",
            format!("could not parse Claude Code response: {error}"),
        )
    })?;
    parse_result(&response)
}

fn parse_result(response: &Value) -> Result<Response> {
    if response.get("is_error").and_then(Value::as_bool) == Some(true) {
        let detail = response
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(Error::agent(
            "claude",
            format!("Claude Code reported an error: {detail}"),
        ));
    }

    Ok(Response {
        answer: response
            .get("result")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::agent(
                    "claude",
                    "Claude Code completed without returning an answer",
                )
            })?
            .to_owned(),
        session_id: response
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::agent(
                    "claude",
                    "Claude Code completed without returning a session ID",
                )
            })?
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Claude, parse_response, read_events, unsupported_option};
    use crate::harness::{Harness, RunOptions};

    #[test]
    fn appends_answer_instructions() {
        let harness = Claude {
            program: "claude".into(),
        };
        let command = harness.command(
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
                .any(|args| args == ["--append-system-prompt", "Be concise."])
        );
    }

    #[test]
    fn command_requests_streaming_output() {
        let harness = Claude {
            program: "claude".into(),
        };
        let command = harness.command(
            "hello",
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
                .any(|args| args == ["--output-format", "stream-json"])
        );
        assert!(args.iter().any(|arg| arg == "--verbose"));
        assert!(args.iter().any(|arg| arg == "--include-partial-messages"));
        assert!(
            args.windows(2)
                .any(|args| args == ["--permission-mode", "plan"])
        );
    }

    #[test]
    fn dash_prefixed_question_follows_option_terminator() {
        let harness = Claude {
            program: "claude".into(),
        };
        let command = harness.command(
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
    fn extracts_answer_and_session() {
        let response = parse_response(
            r#"{"type":"result","is_error":false,"result":"hello","session_id":"abc"}"#,
        )
        .unwrap();

        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "abc");
    }

    #[test]
    fn surfaces_reported_error() {
        let error = parse_response(
            r#"{"type":"result","is_error":true,"result":"not authenticated","session_id":"abc"}"#,
        )
        .unwrap_err();

        assert_eq!(
            error.message(),
            "Claude Code reported an error: not authenticated"
        );
    }

    #[test]
    fn streams_text_deltas_and_extracts_the_result() {
        let input = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"abc\"}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
            "{\"type\":\"result\",\"is_error\":false,\"result\":\"hello\",\"session_id\":\"abc\"}\n"
        );
        let mut streamed = String::new();
        let response = read_events(Cursor::new(input), &mut |delta| {
            streamed.push_str(delta);
            Ok(())
        })
        .map_err(|failure| failure.error)
        .unwrap();

        assert_eq!(streamed, "hello");
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "abc");
    }

    #[test]
    fn non_text_deltas_are_not_streamed() {
        let input = concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}}\n",
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\"}}}\n",
            "{\"type\":\"result\",\"is_error\":false,\"result\":\"done\",\"session_id\":\"abc\"}\n"
        );
        let mut streamed = String::new();
        let response = read_events(Cursor::new(input), &mut |delta| {
            streamed.push_str(delta);
            Ok(())
        })
        .map_err(|failure| failure.error)
        .unwrap();

        assert!(streamed.is_empty());
        assert_eq!(response.answer, "done");
    }

    #[test]
    fn surfaces_reported_errors_from_the_stream() {
        let input = concat!(
            "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}}\n",
            "{\"type\":\"result\",\"is_error\":true,\"result\":\"rate limited\",\"session_id\":\"abc\"}\n"
        );
        let failure = read_events(Cursor::new(input), &mut |_| Ok(())).unwrap_err();

        assert!(failure.completed);
        assert_eq!(
            failure.error.message(),
            "Claude Code reported an error: rate limited"
        );
    }

    #[test]
    fn missing_result_event_is_an_incomplete_stream() {
        let input = "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}}\n";
        let failure = read_events(Cursor::new(input), &mut |_| Ok(())).unwrap_err();

        assert!(!failure.completed);
        assert_eq!(
            failure.error.message(),
            "Claude Code completed without returning an answer"
        );
    }

    #[test]
    fn detects_unsupported_streaming_flags() {
        assert!(unsupported_option(
            "error: unknown option '--include-partial-messages'"
        ));
        assert!(unsupported_option("claude: Unrecognized option: --verbose"));
        assert!(!unsupported_option("rate limited, try again later"));
    }

    fn fake_claude(label: &str, script: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let program = std::env::temp_dir().join(format!(
            "wut-claude-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    #[test]
    fn falls_back_to_buffered_json_for_older_claude_builds() {
        let program = fake_claude(
            "fallback",
            concat!(
                "#!/bin/sh\n",
                "for arg in \"$@\"; do\n",
                "  if [ \"$arg\" = \"--include-partial-messages\" ]; then\n",
                "    echo \"error: unknown option '--include-partial-messages'\" >&2\n",
                "    exit 1\n",
                "  fi\n",
                "done\n",
                "printf '%s' '{\"type\":\"result\",\"is_error\":false,\"result\":\"hello\",\"session_id\":\"abc\"}'\n",
            ),
        );
        let mut harness = Claude {
            program: program.clone().into_os_string(),
        };

        let response = harness
            .run(
                "hello",
                None,
                RunOptions {
                    model: None,
                    reasoning: None,
                    instructions: None,
                },
                &mut |_| Ok(()),
            )
            .unwrap();

        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "abc");
        fs::remove_file(program).unwrap();
    }

    #[test]
    fn nonzero_exit_rejects_a_parsed_success() {
        let program = fake_claude(
            "nonzero",
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' '{\"type\":\"result\",\"is_error\":false,\"result\":\"hello\",\"session_id\":\"abc\"}'\n",
                "printf '%s\\n' 'transport cleanup failed' >&2\n",
                "exit 7\n",
            ),
        );
        let mut harness = Claude {
            program: program.clone().into_os_string(),
        };

        let error = harness
            .run(
                "hello",
                None,
                RunOptions {
                    model: None,
                    reasoning: None,
                    instructions: None,
                },
                &mut |_| Ok(()),
            )
            .unwrap_err();

        assert!(error.message().contains("transport cleanup failed"));
        fs::remove_file(program).unwrap();
    }
}

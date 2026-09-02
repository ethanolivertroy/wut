use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::fast_model;
use super::{Harness, Model, ReasoningLevel, Response, RunOptions, bounded_output, capture_stderr};
use crate::error::{Error, Result};

const REASONING_LEVELS: &[(&str, &str)] = &[
    ("minimal", "Least reasoning"),
    ("low", "Low reasoning"),
    ("medium", "Balanced reasoning"),
    ("high", "Deep reasoning"),
];

// Ordered by expected latency: the small coding model serves at 100+ tok/s,
// the "-fast" flagship variants are the same weights on faster routing.
const FAST_MODELS: &[&str] = &["grok-code-fast-1", "grok-4.6-fast", "grok-4.1-fast"];
const FAST_MODEL_CACHE: fast_model::Cache = fast_model::Cache::new("grok");

pub(super) struct Grok {
    program: OsString,
    resolved_fast_model: Option<String>,
    fast_model_from_disk: bool,
}

impl Grok {
    pub(super) fn new(program: OsString) -> Self {
        Self {
            program,
            resolved_fast_model: None,
            fast_model_from_disk: false,
        }
    }

    fn catalog(&self) -> Result<Vec<Model>> {
        let mut command = Command::new(&self.program);
        command.arg("models");
        let output = bounded_output(&mut command).map_err(start_error)?;
        if !output.status.success() {
            return Err(Error::agent(
                "grok",
                format!("could not list Grok models: {}", output.stderr.trim()),
            ));
        }
        let output = String::from_utf8(output.stdout).map_err(|_| {
            Error::agent(
                "grok",
                "Grok returned a model list that was not valid UTF-8",
            )
        })?;
        parse_models(&output)
    }

    fn fast_model(&mut self) -> Result<String> {
        if let Some(model) = &self.resolved_fast_model {
            return Ok(model.clone());
        }
        if let Some(model) = FAST_MODEL_CACHE.read() {
            self.fast_model_from_disk = true;
            self.resolved_fast_model = Some(model.clone());
            return Ok(model);
        }
        self.resolve_fast_model()
    }

    fn resolve_fast_model(&mut self) -> Result<String> {
        let models = self.catalog()?;
        let model = fastest_model(&models)
            .map(|model| model.id.clone())
            .ok_or_else(|| Error::agent("grok", "Grok did not report an available model"))?;
        FAST_MODEL_CACHE.write(&model);
        self.fast_model_from_disk = false;
        self.resolved_fast_model = Some(model.clone());
        Ok(model)
    }

    fn run_once(
        &self,
        question: &str,
        session_id: Option<&str>,
        model: Option<&str>,
        options: &RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let options = RunOptions {
            model,
            reasoning: options.reasoning,
            instructions: options.instructions,
        };
        let mut child = self
            .command(question, session_id, &options)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(start_error)?;
        let stdout = child.stdout.take().expect("piped stdout is available");
        let stderr = child.stderr.take().expect("piped stderr is available");
        let stderr_reader = std::thread::spawn(move || capture_stderr(stderr));

        let result = read_events(BufReader::new(stdout), on_delta);
        let status = child
            .wait()
            .map_err(|error| Error::agent("grok", format!("could not wait for Grok: {error}")))?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| Error::agent("grok", "could not read Grok error output"))?
            .into_detail();

        if !status.success() {
            return Err(failure(status, &stderr));
        }
        result
    }

    fn command(
        &self,
        question: &str,
        session_id: Option<&str>,
        options: &RunOptions<'_>,
    ) -> Command {
        let mut command = Command::new(&self.program);
        command.args([
            "-p",
            question,
            "--output-format",
            "streaming-json",
            "--permission-mode",
            "plan",
            "--no-auto-update",
        ]);
        if let Some(session_id) = session_id {
            command.args(["--resume", session_id]);
        }
        if let Some(model) = options.model {
            command.args(["--model", model]);
        }
        if let Some(reasoning) = options.reasoning {
            command.args(["--reasoning-effort", reasoning]);
        }
        if let Some(instructions) = options.instructions {
            command.args(["--rules", instructions]);
        }
        command
    }
}

impl Harness for Grok {
    fn models(&mut self) -> Result<Vec<Model>> {
        let mut models = self.catalog()?;
        if !models.iter().any(|model| model.id == "fast")
            && let Some(model) = fastest_model(&models).cloned()
        {
            models.insert(
                0,
                Model {
                    id: "fast".into(),
                    name: "Fastest available".into(),
                    description: format!("Currently uses {}", model.name),
                    is_default: false,
                    reasoning: model.reasoning,
                    default_reasoning: model.default_reasoning,
                },
            );
        }
        Ok(models)
    }

    fn run(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response> {
        let fast = options.model == Some("fast");
        let model = if fast {
            Some(self.fast_model()?)
        } else {
            options.model.map(str::to_owned)
        };

        let mut streamed = false;
        let result = {
            let mut tracking = |delta: &str| {
                streamed = true;
                on_delta(delta)
            };
            self.run_once(
                question,
                session_id,
                model.as_deref(),
                &options,
                &mut tracking,
            )
        };
        let Err(error) = result else {
            return result;
        };
        // A cached fast model may have been retired since it was resolved.
        // Re-resolve once and retry, but only when nothing was shown yet and
        // the catalog actually names a different model; otherwise the failure
        // has some other cause and a second run would just repeat it.
        if !(fast && self.fast_model_from_disk && !streamed) {
            return Err(error);
        }
        FAST_MODEL_CACHE.invalidate();
        self.resolved_fast_model = None;
        self.fast_model_from_disk = false;
        let fresh = self.resolve_fast_model()?;
        if Some(fresh.as_str()) == model.as_deref() {
            return Err(error);
        }
        self.run_once(question, session_id, Some(&fresh), &options, on_delta)
    }
}

fn fastest_model(models: &[Model]) -> Option<&Model> {
    FAST_MODELS
        .iter()
        .find_map(|id| models.iter().find(|model| model.id == *id))
        .or_else(|| models.iter().find(|model| model.id.contains("fast")))
        .or_else(|| models.iter().find(|model| model.is_default))
        .or_else(|| models.first())
}

fn start_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        Error::new(
            "Grok is not installed or not on PATH",
            "install it with 'curl -fsSL https://x.ai/cli/install.sh | bash' and run 'grok login'",
        )
    } else {
        Error::agent("grok", format!("could not start Grok: {error}"))
    }
}

fn failure(status: std::process::ExitStatus, stderr: &str) -> Error {
    let detail = stderr.trim();
    if detail.is_empty() {
        Error::agent("grok", format!("Grok exited with {status}"))
    } else {
        Error::agent("grok", format!("Grok failed: {detail}"))
    }
}

fn parse_models(output: &str) -> Result<Vec<Model>> {
    let reasoning = REASONING_LEVELS
        .iter()
        .map(|(id, description)| ReasoningLevel {
            id: (*id).to_owned(),
            description: (*description).to_owned(),
        })
        .collect::<Vec<_>>();
    let mut models = Vec::new();
    let mut in_available = false;

    for line in output.lines() {
        let line = line.trim();
        if line.starts_with("Available models") {
            in_available = true;
            continue;
        }
        if !in_available {
            continue;
        }
        let Some(entry) = line.strip_prefix('*').or_else(|| line.strip_prefix('-')) else {
            continue;
        };
        let entry = entry.trim();
        let Some(id) = entry.split_whitespace().next() else {
            continue;
        };
        models.push(Model {
            id: id.to_owned(),
            name: id.to_owned(),
            description: "xAI Grok".to_owned(),
            is_default: entry.contains("(default)"),
            reasoning: reasoning.clone(),
            default_reasoning: None,
        });
    }

    if models.is_empty() {
        Err(Error::agent(
            "grok",
            "Grok did not report any available models",
        ))
    } else {
        Ok(models)
    }
}

fn read_events(
    reader: impl BufRead,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut answer = String::new();
    let mut session_id = None;
    let mut reported_error = None;

    for line in reader.lines() {
        let line = line.map_err(|error| {
            Error::agent("grok", format!("could not read Grok response: {error}"))
        })?;
        let event: Value = serde_json::from_str(&line).map_err(|error| {
            Error::agent("grok", format!("could not parse Grok response: {error}"))
        })?;
        match event.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(delta) = event.get("data").and_then(Value::as_str) {
                    answer.push_str(delta);
                    on_delta(delta)?;
                }
            }
            Some("error") => {
                reported_error = event
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("end") => {
                session_id = event
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                break;
            }
            _ => {}
        }
    }

    if let Some(error) = reported_error {
        return Err(Error::agent(
            "grok",
            format!("Grok reported an error: {error}"),
        ));
    }
    Ok(Response {
        answer: if answer.is_empty() {
            return Err(Error::agent(
                "grok",
                "Grok completed without returning an answer",
            ));
        } else {
            answer
        },
        session_id: session_id
            .ok_or_else(|| Error::agent("grok", "Grok completed without returning a session ID"))?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{Grok, fastest_model, parse_models, read_events};
    use crate::harness::{Harness, Model, RunOptions};

    fn fake_program(label: &str, script: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let program = std::env::temp_dir().join(format!(
            "wut-grok-{label}-test-{}-{unique}",
            std::process::id()
        ));
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    fn model(id: &str, is_default: bool) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            is_default,
            reasoning: Vec::new(),
            default_reasoning: None,
        }
    }

    #[test]
    fn nonzero_exit_rejects_a_parsed_success() {
        let program = fake_program(
            "nonzero",
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' '{\"type\":\"text\",\"data\":\"hello\"}'\n",
                "printf '%s\\n' '{\"type\":\"end\",\"stop_reason\":\"stop\",\"session_id\":\"session-1\"}'\n",
                "printf '%s\\n' 'transport cleanup failed' >&2\n",
                "exit 7\n",
            ),
        );
        let mut harness = Grok::new(program.clone().into_os_string());

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

    #[test]
    fn command_is_read_only_and_resumes_sessions() {
        let harness = Grok::new("grok".into());
        let command = harness.command(
            "hello",
            Some("session-1"),
            &RunOptions {
                model: Some("grok-4.6"),
                reasoning: Some("high"),
                instructions: Some("Be concise."),
            },
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        assert!(
            args.windows(2)
                .any(|args| args == ["--permission-mode", "plan"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--resume", "session-1"])
        );
        assert!(args.windows(2).any(|args| args == ["--model", "grok-4.6"]));
        assert!(
            args.windows(2)
                .any(|args| args == ["--reasoning-effort", "high"])
        );
        assert!(
            args.windows(2)
                .any(|args| args == ["--rules", "Be concise."])
        );
    }

    #[test]
    fn parses_model_table() {
        let models = parse_models(concat!(
            "You are not authenticated.\n",
            "\n",
            "Default model: grok-4.6\n",
            "\n",
            "Available models:\n",
            "  * grok-4.6 (default)\n",
            "  - grok-4.5\n",
        ))
        .unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "grok-4.6");
        assert!(models[0].is_default);
        assert_eq!(models[0].reasoning.len(), 4);
        assert_eq!(models[1].id, "grok-4.5");
        assert!(!models[1].is_default);
    }

    #[test]
    fn fastest_model_prefers_the_coding_model_then_fast_variants_then_default() {
        let full = [
            model("grok-4.6", true),
            model("grok-4.6-fast", false),
            model("grok-code-fast-1", false),
        ];
        assert_eq!(fastest_model(&full).unwrap().id, "grok-code-fast-1");

        let flagship_only = [model("grok-4.6", true), model("grok-4.6-fast", false)];
        assert_eq!(fastest_model(&flagship_only).unwrap().id, "grok-4.6-fast");

        let unknown_fast = [model("grok-4.6", true), model("grok-5-fast-preview", false)];
        assert_eq!(
            fastest_model(&unknown_fast).unwrap().id,
            "grok-5-fast-preview"
        );

        let no_fast = [model("grok-4.5", false), model("grok-4.6", true)];
        assert_eq!(fastest_model(&no_fast).unwrap().id, "grok-4.6");
        assert_eq!(
            fastest_model(&[model("custom", false)]).unwrap().id,
            "custom"
        );
        assert!(fastest_model(&[]).is_none());
    }

    #[test]
    fn catalog_gains_a_fast_alias_that_names_its_target() {
        let program = fake_program(
            "catalog",
            concat!(
                "#!/bin/sh\n",
                "[ \"$1\" = models ] || exit 9\n",
                "printf '%s\\n' 'Default model: grok-4.6' '' 'Available models:' '  * grok-4.6 (default)' '  - grok-code-fast-1'\n",
            ),
        );
        let mut harness = Grok::new(program.clone().into_os_string());

        let models = harness.models().unwrap();

        assert_eq!(models[0].id, "fast");
        assert_eq!(models[0].description, "Currently uses grok-code-fast-1");
        assert_eq!(models[0].reasoning.len(), 4);
        assert!(!models[0].is_default);
        assert_eq!(models[1].id, "grok-4.6");
        assert!(models[1].is_default);
        assert_eq!(models[2].id, "grok-code-fast-1");
        fs::remove_file(program).unwrap();
    }

    #[test]
    fn streams_text_and_extracts_the_session() {
        let input = concat!(
            "{\"type\":\"text\",\"data\":\"hel\"}\n",
            "{\"type\":\"text\",\"data\":\"lo\"}\n",
            "{\"type\":\"usage\",\"messageId\":\"m-1\"}\n",
            "{\"type\":\"end\",\"stop_reason\":\"stop\",\"session_id\":\"session-1\",\"request_id\":\"r-1\"}\n"
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

    #[test]
    fn surfaces_reported_errors() {
        let input = concat!(
            "{\"type\":\"text\",\"data\":\"partial\"}\n",
            "{\"type\":\"error\",\"message\":\"rate limited\"}\n",
            "{\"type\":\"end\",\"stop_reason\":\"error\",\"session_id\":\"session-1\"}\n"
        );
        let error = read_events(Cursor::new(input), &mut |_| Ok(())).unwrap_err();
        assert_eq!(error.message(), "Grok reported an error: rate limited");
    }
}

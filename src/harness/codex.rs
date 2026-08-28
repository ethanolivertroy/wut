use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::{Harness, Model, ReasoningLevel, Response, RunOptions};
use crate::error::{Error, Result};
use crate::storage;

const FAST_MODELS: &[&str] = &["gpt-5.3-codex-spark", "gpt-5.6-luna"];
const FAST_MODEL_CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;

pub(super) struct Codex {
    program: OsString,
    server: Option<AppServer>,
}

impl Codex {
    pub(super) fn new(program: OsString) -> Self {
        Self {
            program,
            server: None,
        }
    }

    fn server(&mut self) -> Result<&mut AppServer> {
        if self.server.is_none() {
            self.server = Some(AppServer::start(&self.program)?);
        }
        Ok(self.server.as_mut().expect("app server was initialized"))
    }
}

impl Harness for Codex {
    fn models(&mut self) -> Result<Vec<Model>> {
        let mut models = self.server()?.models()?;
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
        self.server()?.run(question, session_id, options, on_delta)
    }
}

struct AppServer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
    thread_id: Option<String>,
    instructions: Option<String>,
    resolved_fast_model: Option<String>,
    fast_model_from_disk: bool,
}

struct TurnFailure {
    error: Error,
    request_rejected: bool,
}

impl From<Error> for TurnFailure {
    fn from(error: Error) -> Self {
        Self {
            error,
            request_rejected: false,
        }
    }
}

impl AppServer {
    fn start(program: &OsStr) -> Result<Self> {
        let mut child = Command::new(program)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::new(
                        "Codex is not installed or not on PATH",
                        "install it, run 'codex login', then try again",
                    )
                } else {
                    Error::new(
                        format!("could not start Codex app server: {error}"),
                        "run 'codex' directly to check its setup, then try again",
                    )
                }
            })?;

        let input = child.stdin.take().expect("piped stdin is available");
        let output = BufReader::new(child.stdout.take().expect("piped stdout is available"));
        let mut server = Self {
            child,
            input,
            output,
            next_id: 1,
            thread_id: None,
            instructions: None,
            resolved_fast_model: None,
            fast_model_from_disk: false,
        };

        server.send(&json!({
            "method": "initialize",
            "id": 0,
            "params": {
                "clientInfo": {
                    "name": "wut",
                    "title": "wut",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }))?;
        server.wait_for_response(0)?;
        server.send(&json!({"method": "initialized", "params": {}}))?;
        Ok(server)
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
        let thread_id = self.prepare_thread(session_id, options.instructions)?;
        match self.start_turn(&thread_id, question, model.as_deref(), &options, on_delta) {
            Err(failure) if failure.request_rejected && fast && self.fast_model_from_disk => {
                // The cached fast model may have been retired since it was
                // resolved; re-resolve once and retry before surfacing errors.
                if let Some(path) = fast_model_cache_path() {
                    invalidate_cached_fast_model(&path);
                }
                self.resolved_fast_model = None;
                self.fast_model_from_disk = false;
                let model = self.resolve_fast_model()?;
                self.start_turn(&thread_id, question, Some(&model), &options, on_delta)
                    .map_err(|failure| failure.error)
            }
            result => result.map_err(|failure| failure.error),
        }
    }

    fn start_turn(
        &mut self,
        thread_id: &str,
        question: &str,
        model: Option<&str>,
        options: &RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> std::result::Result<Response, TurnFailure> {
        let request_id = self.request_id();
        self.send(&json!({
            "method": "turn/start",
            "id": request_id,
            "params": {
                "threadId": thread_id,
                "input": [{"type": "text", "text": question}],
                "model": model,
                "effort": options.reasoning
            }
        }))?;

        let mut streamed_answer = String::new();
        let mut final_answer = None;
        let mut reported_error = None;

        loop {
            let message = self.read()?;
            if message.get("id").and_then(Value::as_u64) == Some(request_id)
                && let Err(error) = response_error(&message)
            {
                return Err(TurnFailure {
                    request_rejected: streamed_answer.is_empty(),
                    error,
                });
            }

            match message.get("method").and_then(Value::as_str) {
                Some("item/agentMessage/delta") => {
                    if let Some(delta) = message["params"]["delta"].as_str() {
                        streamed_answer.push_str(delta);
                        on_delta(delta)?;
                    }
                }
                Some("item/completed") => {
                    let item = &message["params"]["item"];
                    if item.get("type").and_then(Value::as_str) == Some("agentMessage")
                        && let Some(text) = item.get("text").and_then(Value::as_str)
                    {
                        final_answer = Some(text.to_owned());
                    }
                }
                Some("error") => {
                    reported_error = message["params"]["error"]["message"]
                        .as_str()
                        .map(str::to_owned);
                }
                Some("turn/completed") => {
                    let status = message["params"]["turn"]["status"].as_str();
                    if status != Some("completed") {
                        let message = reported_error.unwrap_or_else(|| {
                            format!(
                                "Codex turn ended with status '{}'",
                                status.unwrap_or("unknown")
                            )
                        });
                        return Err(Error::agent("codex", message).into());
                    }
                    break;
                }
                _ => {}
            }
        }

        if let Some(error) = reported_error {
            return Err(Error::agent("codex", format!("Codex reported an error: {error}")).into());
        }

        let answer = final_answer.unwrap_or(streamed_answer);
        if answer.is_empty() {
            return Err(Error::new(
                "Codex completed without returning an answer",
                "update Codex and try again",
            )
            .into());
        }

        Ok(Response {
            answer,
            session_id: thread_id.to_owned(),
        })
    }

    fn fast_model(&mut self) -> Result<String> {
        if let Some(model) = &self.resolved_fast_model {
            return Ok(model.clone());
        }
        if let Some(path) = fast_model_cache_path()
            && let Some(model) = read_cached_fast_model(&path, now())
        {
            self.fast_model_from_disk = true;
            self.resolved_fast_model = Some(model.clone());
            return Ok(model);
        }
        self.resolve_fast_model()
    }

    fn resolve_fast_model(&mut self) -> Result<String> {
        let request_id = self.request_id();
        self.send(&json!({
            "method": "model/list",
            "id": request_id,
            "params": {"limit": 100, "includeHidden": false}
        }))?;
        let response = self.wait_for_response(request_id)?;
        let model = select_fast_model(&response["result"]["data"]).ok_or_else(|| {
            Error::new(
                "Codex did not report an available model",
                "run 'codex login', then try again",
            )
        })?;
        if let Some(path) = fast_model_cache_path() {
            write_cached_fast_model(&path, &model, now());
        }
        self.fast_model_from_disk = false;
        self.resolved_fast_model = Some(model.clone());
        Ok(model)
    }

    fn models(&mut self) -> Result<Vec<Model>> {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let id = self.request_id();
            self.send(&json!({
                "method": "model/list",
                "id": id,
                "params": {
                    "cursor": cursor,
                    "limit": 100,
                    "includeHidden": false
                }
            }))?;
            let response = self.wait_for_response(id)?;
            let result = &response["result"];
            let data = result["data"].as_array().ok_or_else(|| {
                Error::new(
                    "Codex returned an invalid model list",
                    "update Codex and try again",
                )
            })?;

            for value in data {
                let id = required_string(value, "model", "model")?;
                let name = required_string(value, "displayName", "model display name")?;
                let description = value["description"].as_str().unwrap_or_default().to_owned();
                let reasoning = value["supportedReasoningEfforts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|effort| {
                        Some(ReasoningLevel {
                            id: effort["reasoningEffort"].as_str()?.to_owned(),
                            description: effort["description"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect();
                models.push(Model {
                    id,
                    name,
                    description,
                    is_default: value["isDefault"].as_bool().unwrap_or(false),
                    reasoning,
                    default_reasoning: value["defaultReasoningEffort"].as_str().map(str::to_owned),
                });
            }

            cursor = result["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }

        if models.is_empty() {
            Err(Error::new(
                "Codex did not report any available models",
                "run 'codex login', then try again",
            ))
        } else {
            Ok(models)
        }
    }

    fn prepare_thread(
        &mut self,
        requested: Option<&str>,
        instructions: Option<&str>,
    ) -> Result<String> {
        if let Some(current) = &self.thread_id
            && requested == Some(current.as_str())
            && self.instructions.as_deref() == instructions
        {
            return Ok(current.clone());
        }

        let id = self.request_id();
        let request = if let Some(thread_id) = requested {
            json!({
                "method": "thread/resume",
                "id": id,
                "params": {
                    "threadId": thread_id,
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "developerInstructions": instructions
                }
            })
        } else {
            let cwd = std::env::current_dir().map_err(|error| {
                Error::new(
                    format!("could not determine current directory: {error}"),
                    "change to an existing directory and try again",
                )
            })?;
            json!({
                "method": "thread/start",
                "id": id,
                "params": {
                    "cwd": cwd,
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "developerInstructions": instructions
                }
            })
        };

        self.send(&request)?;
        let response = self.wait_for_response(id)?;
        let thread_id = response["result"]["thread"]["id"]
            .as_str()
            .ok_or_else(|| {
                Error::new(
                    "Codex did not return a thread ID",
                    "update Codex and try again",
                )
            })?
            .to_owned();
        self.thread_id = Some(thread_id.clone());
        self.instructions = instructions.map(str::to_owned);
        Ok(thread_id)
    }

    fn request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.input, message)
            .map_err(|error| Error::internal(format!("could not encode Codex request: {error}")))?;
        self.input
            .write_all(b"\n")
            .and_then(|()| self.input.flush())
            .map_err(|error| {
                Error::new(
                    format!("could not send request to Codex: {error}"),
                    "restart wut and try again",
                )
            })
    }

    fn read(&mut self) -> Result<Value> {
        let mut line = String::new();
        let bytes = self.output.read_line(&mut line).map_err(|error| {
            Error::new(
                format!("could not read Codex response: {error}"),
                "restart wut and try again",
            )
        })?;
        if bytes == 0 {
            return Err(Error::new(
                "Codex app server stopped unexpectedly",
                "restart wut and try again",
            ));
        }
        serde_json::from_str(&line).map_err(|error| {
            Error::new(
                format!("could not parse Codex response: {error}"),
                "update Codex and try again",
            )
        })
    }

    fn wait_for_response(&mut self, id: u64) -> Result<Value> {
        loop {
            let message = self.read()?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                response_error(&message)?;
                return Ok(message);
            }
        }
    }
}

fn select_fast_model(models: &Value) -> Option<String> {
    let models = models.as_array()?;
    for preferred in FAST_MODELS {
        if models.iter().any(|model| model["model"] == *preferred) {
            return Some((*preferred).to_owned());
        }
    }
    models
        .iter()
        .find(|model| model["isDefault"].as_bool() == Some(true))
        .or_else(|| models.first())?
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn fastest_model(models: &[Model]) -> Option<&Model> {
    FAST_MODELS
        .iter()
        .find_map(|id| models.iter().find(|model| model.id == *id))
        .or_else(|| models.iter().find(|model| model.is_default))
        .or_else(|| models.first())
}

fn required_string(value: &Value, key: &str, name: &str) -> Result<String> {
    value[key].as_str().map(str::to_owned).ok_or_else(|| {
        Error::new(
            format!("Codex returned a model without a {name}"),
            "update Codex and try again",
        )
    })
}

fn fast_model_cache_path() -> Option<PathBuf> {
    fast_model_cache_path_from(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

fn fast_model_cache_path_from(
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(path) = xdg_cache_home.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path).join("wut/codex.json"));
    }
    home.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".cache/wut/codex.json"))
}

fn read_cached_fast_model(path: &Path, now: u64) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let model = value.get("fast_model")?.as_str()?;
    if model.is_empty() {
        return None;
    }
    let resolved_at = value.get("resolved_at")?.as_u64()?;
    // A resolution timestamp in the future means the clock moved backwards;
    // treat the entry as stale rather than trusting it indefinitely.
    let age = now.checked_sub(resolved_at)?;
    (age < FAST_MODEL_CACHE_TTL_SECONDS).then(|| model.to_owned())
}

fn write_cached_fast_model(path: &Path, model: &str, resolved_at: u64) {
    let value = json!({
        "fast_model": model,
        "resolved_at": resolved_at,
    });
    // The cache only saves a round-trip; never fail a turn over it.
    if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
        let _ = storage::write_private(path, &bytes, "codex model cache");
    }
}

fn invalidate_cached_fast_model(path: &Path) {
    let _ = fs::remove_file(path);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn response_error(response: &Value) -> Result<()> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown protocol error");
        if message.starts_with("thread ") && message.ends_with(" already has an active writer") {
            Err(Error::new(
                "this session is open in another process",
                "close the other wut process and try again",
            ))
        } else {
            Err(Error::agent(
                "codex",
                format!("Codex app server error: {message}"),
            ))
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{
        FAST_MODEL_CACHE_TTL_SECONDS, fast_model_cache_path_from, fastest_model,
        invalidate_cached_fast_model, read_cached_fast_model, select_fast_model,
        write_cached_fast_model,
    };
    use crate::harness::Model;

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
    fn fast_model_prefers_spark_and_falls_back_to_luna() {
        let with_spark = json!([
            {"model": "gpt-5.6-luna"},
            {"model": "gpt-5.3-codex-spark"}
        ]);
        assert_eq!(
            select_fast_model(&with_spark).as_deref(),
            Some("gpt-5.3-codex-spark")
        );

        let without_spark = json!([{"model": "gpt-5.6-luna"}]);
        assert_eq!(
            select_fast_model(&without_spark).as_deref(),
            Some("gpt-5.6-luna")
        );
    }

    #[test]
    fn fast_catalog_model_uses_the_same_preference() {
        let models = [
            model("gpt-5.6-luna", true),
            model("gpt-5.3-codex-spark", false),
        ];

        assert_eq!(fastest_model(&models).unwrap().id, "gpt-5.3-codex-spark");
        assert_eq!(
            fastest_model(&[model("custom", true)]).unwrap().id,
            "custom"
        );
    }

    fn unique_cache_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("wut-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn fast_model_cache_round_trips_until_the_ttl_expires() {
        let directory = unique_cache_directory("codex-cache-ttl");
        let path = directory.join("codex.json");

        write_cached_fast_model(&path, "gpt-5.3-codex-spark", 1_000);

        assert_eq!(
            read_cached_fast_model(&path, 1_000).as_deref(),
            Some("gpt-5.3-codex-spark")
        );
        assert_eq!(
            read_cached_fast_model(&path, 1_000 + FAST_MODEL_CACHE_TTL_SECONDS - 1).as_deref(),
            Some("gpt-5.3-codex-spark")
        );
        assert_eq!(
            read_cached_fast_model(&path, 1_000 + FAST_MODEL_CACHE_TTL_SECONDS),
            None
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fast_model_cache_rejects_clock_rollback_and_malformed_entries() {
        let directory = unique_cache_directory("codex-cache-invalid");
        let path = directory.join("codex.json");

        assert_eq!(read_cached_fast_model(&path, 1_000), None);

        write_cached_fast_model(&path, "gpt-5.3-codex-spark", 2_000);
        assert_eq!(read_cached_fast_model(&path, 1_999), None);

        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, b"not-json").unwrap();
        assert_eq!(read_cached_fast_model(&path, 1_000), None);

        fs::write(&path, b"{\"fast_model\":\"\",\"resolved_at\":1000}").unwrap();
        assert_eq!(read_cached_fast_model(&path, 1_000), None);

        fs::write(&path, b"{\"fast_model\":\"gpt\"}").unwrap();
        assert_eq!(read_cached_fast_model(&path, 1_000), None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalidating_the_fast_model_cache_removes_the_entry() {
        let directory = unique_cache_directory("codex-cache-invalidate");
        let path = directory.join("codex.json");

        write_cached_fast_model(&path, "gpt-5.3-codex-spark", 1_000);
        assert!(read_cached_fast_model(&path, 1_000).is_some());

        invalidate_cached_fast_model(&path);
        assert_eq!(read_cached_fast_model(&path, 1_000), None);

        invalidate_cached_fast_model(&path);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn empty_home_does_not_create_a_relative_cache_path() {
        assert_eq!(
            fast_model_cache_path_from(None, Some(OsString::new())),
            None
        );
        assert_eq!(fast_model_cache_path_from(None, None), None);
        assert_eq!(
            fast_model_cache_path_from(Some(OsString::from("/cache")), None),
            Some(PathBuf::from("/cache/wut/codex.json"))
        );
        assert_eq!(
            fast_model_cache_path_from(None, Some(OsString::from("/home/user"))),
            Some(PathBuf::from("/home/user/.cache/wut/codex.json"))
        );
    }
}

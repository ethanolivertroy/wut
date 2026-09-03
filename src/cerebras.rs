use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://api.cerebras.ai/v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MODEL: &str = "gpt-oss-120b";
const ENV_KEY: &str = "CEREBRAS_API_KEY";
const CREDENTIALS_FILE: &str = "wut/credentials";

pub const MODELS: &[Model] = &[
    Model {
        id: "gpt-oss-120b",
        name: "GPT OSS 120B",
        description: "Fastest, lowest-cost Cerebras text model",
        default_reasoning: Some("low"),
        levels: &["low", "medium", "high"],
    },
    Model {
        id: "gemma-4-31b",
        name: "Gemma 4 31B",
        description: "Cerebras public endpoint · vision capable",
        default_reasoning: Some("none"),
        levels: &["none", "low", "medium", "high"],
    },
];

pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default_reasoning: Option<&'static str>,
    pub levels: &'static [&'static str],
}

pub fn default_model() -> &'static str {
    DEFAULT_MODEL
}

pub fn resolve_model(id: Option<&str>) -> &'static str {
    MODELS
        .iter()
        .find(|model| Some(model.id) == id)
        .map_or(DEFAULT_MODEL, |model| model.id)
}

pub fn find_model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.id == id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

impl Message {
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    fn to_json(&self) -> Value {
        let mut value = json!({
            "role": self.role.as_str(),
            "content": if self.content.is_empty() && !self.tool_calls.is_empty() {
                Value::Null
            } else {
                Value::String(self.content.clone())
            },
        });
        if !self.tool_calls.is_empty() {
            value["tool_calls"] = json!(
                self.tool_calls
                    .iter()
                    .map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments,
                        },
                    }))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(id) = &self.tool_call_id {
            value["tool_call_id"] = Value::String(id.clone());
        }
        value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    fn merge(&mut self, fragment: &Fragment) {
        if self.id.is_empty()
            && let Some(id) = &fragment.id
        {
            self.id.clone_from(id);
        }
        if self.name.is_empty()
            && let Some(name) = &fragment.name
        {
            self.name.clone_from(name);
        }
        if let Some(arguments) = &fragment.arguments {
            self.arguments.push_str(arguments);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

impl Tool {
    fn to_json(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        })
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Outcome {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

pub struct Client {
    api_key: String,
    base_url: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        Ok(Self {
            api_key: auth_key()?,
            base_url: DEFAULT_BASE_URL.to_owned(),
        })
    }

    pub fn stream(
        &self,
        messages: &[Message],
        tools: &[Tool],
        model: &str,
        effort: Option<&str>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Outcome> {
        let body = request_body(messages, tools, model, effort);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .build();
        let response = agent
            .post(&format!("{}/chat/completions", self.base_url))
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(api_error)?;

        let status = response.status();
        if status != 200 {
            let body = response.into_string().unwrap_or_default();
            return Err(api_status_error(status, &body));
        }

        let stream = EventStream::new(BufReader::new(response.into_reader()));
        collect_stream(stream, on_delta)
    }
}

pub fn request_body(
    messages: &[Message],
    tools: &[Tool],
    model: &str,
    effort: Option<&str>,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages.iter().map(Message::to_json).collect::<Vec<_>>(),
        "stream": true,
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools.iter().map(Tool::to_json).collect::<Vec<_>>());
    }
    match effort {
        Some(effort) if effort != "none" => {
            body["reasoning_effort"] = Value::String(effort.to_owned());
        }
        _ => {}
    }
    body
}

fn collect_stream(
    mut stream: EventStream<impl BufRead>,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Outcome> {
    let mut content = String::new();
    let mut calls: Vec<Option<ToolCall>> = Vec::new();
    while let Some(event) = stream.next()? {
        if let Some(message) = event.error {
            return Err(Error::new(
                format!("the model reported an error: {message}"),
                "check your API key and usage limits, then try again",
            ));
        }
        if let Some(text) = event.content {
            content.push_str(&text);
            on_delta(&text)?;
        }
        for fragment in event.tool_calls {
            let index = fragment.index;
            if index >= calls.len() {
                calls.resize(index + 1, None);
            }
            let slot = &mut calls[index];
            match slot {
                Some(call) => call.merge(&fragment),
                None => {
                    *slot = Some(ToolCall {
                        id: fragment.id.unwrap_or_default(),
                        name: fragment.name.unwrap_or_default(),
                        arguments: fragment.arguments.unwrap_or_default(),
                    });
                }
            }
        }
    }
    if !stream.completed {
        return Err(Error::new(
            "the streaming response ended before its completion marker",
            "check your connection and try again",
        ));
    }
    Ok(Outcome {
        content,
        tool_calls: calls.into_iter().flatten().collect(),
    })
}

#[derive(Debug, Default)]
struct Fragment {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Default)]
struct Event {
    content: Option<String>,
    error: Option<String>,
    tool_calls: Vec<Fragment>,
}

fn decode_event(data: &str) -> Result<Option<Event>> {
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(data).map_err(|error| {
        Error::new(
            format!("could not decode the streaming response: {error}"),
            "check your connection and try again",
        )
    })?;
    if let Some(message) = value["error"]["message"].as_str() {
        return Ok(Some(Event {
            error: Some(message.to_owned()),
            ..Event::default()
        }));
    }
    let choice = &value["choices"][0];
    let content = choice["delta"]["content"]
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let mut tool_calls = Vec::new();
    if let Some(values) = choice["delta"]["tool_calls"].as_array() {
        for value in values {
            tool_calls.push(Fragment {
                index: value["index"].as_u64().unwrap_or(0) as usize,
                id: value["id"].as_str().map(str::to_owned),
                name: value["function"]["name"].as_str().map(str::to_owned),
                arguments: value["function"]["arguments"].as_str().map(str::to_owned),
            });
        }
    }
    if content.is_none() && tool_calls.is_empty() {
        return Ok(None);
    }
    Ok(Some(Event {
        content,
        tool_calls,
        ..Event::default()
    }))
}

struct EventStream<R: BufRead> {
    reader: R,
    completed: bool,
}

impl<R: BufRead> EventStream<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            completed: false,
        }
    }

    fn next(&mut self) -> Result<Option<Event>> {
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).map_err(|error| {
                Error::new(
                    format!("could not read the streaming response: {error}"),
                    "check your connection and try again",
                )
            })?;
            if read == 0 {
                return Ok(None);
            }
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(data) = line.strip_prefix("data:") {
                if data.trim() == "[DONE]" {
                    self.completed = true;
                    return Ok(None);
                }
                if let Some(event) = decode_event(data)? {
                    return Ok(Some(event));
                }
            }
        }
    }
}

fn auth_key() -> Result<String> {
    if let Some(key) = std::env::var_os(ENV_KEY)
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(key);
    }
    if let Some(key) = key_from_credentials_file() {
        return Ok(key);
    }
    Err(Error::new(
        format!("no API key found (set ${ENV_KEY})"),
        "rerun install.sh from a terminal to save one, or set CEREBRAS_API_KEY",
    ))
}

fn key_from_credentials_file() -> Option<String> {
    let path = if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(config_home)
    } else {
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config")
    }
    .join(CREDENTIALS_FILE);
    std::fs::read_to_string(path)
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

fn api_error(error: ureq::Error) -> Error {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            api_status_error(code, &body)
        }
        ureq::Error::Transport(error) => {
            let message = match error.kind() {
                ureq::ErrorKind::Dns => "could not reach the inference endpoint (DNS failure)",
                ureq::ErrorKind::ConnectionFailed => "connection to the inference endpoint failed",
                ureq::ErrorKind::Io => "the request failed or timed out",
                _ => "the request could not be sent",
            };
            Error::new(message, "check your connection and try again")
        }
    }
}

fn api_status_error(status: u16, body: &str) -> Error {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value["error"]["message"]
                .as_str()
                .or_else(|| value["message"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.trim().to_owned());
    let help = match status {
        401 | 403 => "check your API key and try again",
        429 => "you hit a rate limit; wait a moment and try again",
        400..=499 => "check the request and try again",
        _ => "the service may be temporarily unavailable; try again shortly",
    };
    let message = if detail.is_empty() {
        format!("the inference endpoint returned HTTP {status}")
    } else {
        format!("the inference endpoint returned HTTP {status}: {detail}")
    };
    Error::new(message, help)
}

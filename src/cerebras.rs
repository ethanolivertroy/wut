use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{Value, json};

use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://api.cerebras.ai/v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest wait for the next bytes of a response. Each read restarts
/// it, so a long answer that keeps streaming is never cut off.
const READ_TIMEOUT: Duration = Duration::from_secs(90);
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
    Model {
        id: "qwen-3.8-27b",
        name: "Qwen 3.8 27B",
        description: "Cerebras public endpoint · configurable reasoning",
        default_reasoning: Some("high"),
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
    // Keep the agent's connection pool for tool rounds and chat follow-ups.
    http: ureq::Agent,
}

impl Client {
    #[cfg(test)]
    pub fn for_test(base_url: &str) -> Self {
        Self {
            api_key: "test".to_owned(),
            base_url: base_url.to_owned(),
            http: http_agent(),
        }
    }

    pub fn new() -> Result<Self> {
        Ok(Self {
            api_key: auth_key()?,
            base_url: DEFAULT_BASE_URL.to_owned(),
            http: http_agent(),
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
        let response = self
            .http
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

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build()
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
    if let Some(effort) = effort {
        body["reasoning_effort"] = Value::String(effort.to_owned());
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
            crate::cancel::check()?;
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
    if let Some(key) = key_from_credentials_file()? {
        return Ok(key);
    }
    Err(Error::new(
        format!("no API key found (set ${ENV_KEY})"),
        "rerun install.sh from a terminal to save one, or set CEREBRAS_API_KEY",
    ))
}

fn key_from_credentials_file() -> Result<Option<String>> {
    let path = if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(config_home)
    } else {
        let Some(home) = std::env::var_os("HOME") else {
            return Ok(None);
        };
        std::path::PathBuf::from(home).join(".config")
    }
    .join(CREDENTIALS_FILE);
    read_credentials(&path)
}

fn read_credentials(path: &std::path::Path) -> Result<Option<String>> {
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Read};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let invalid = || {
        Error::new(
            format!(
                "refusing unsafe or unreadable credentials '{}'",
                path.display()
            ),
            "use an owner-only directory (chmod 700) and a regular credentials file (chmod 600), owned by you with no symlinks or hard links; or set CEREBRAS_API_KEY",
        )
    };
    let Some(parent) = path.parent() else {
        return Err(invalid());
    };
    let directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)
    {
        Ok(directory) => directory,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid()),
    };
    // Pin the directory before opening its entry, so a renamed directory
    // cannot swap in a different credentials file after these checks.
    let metadata = directory.metadata().map_err(|_| invalid())?;
    // SAFETY: geteuid has no arguments or memory requirements.
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    // SAFETY: the directory descriptor and static, NUL-terminated filename
    // remain valid for the call. O_NONBLOCK prevents a FIFO from hanging us.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"credentials".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return if std::io::Error::last_os_error().kind() == ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(invalid())
        };
    }
    // SAFETY: openat returned a fresh descriptor that this File now owns.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > 4096
    {
        return Err(invalid());
    }
    let mut key = String::new();
    file.take(4097)
        .read_to_string(&mut key)
        .map_err(|_| invalid())?;
    if key.len() > 4096 {
        return Err(invalid());
    }
    let key = key.trim();
    Ok((!key.is_empty()).then(|| key.to_owned()))
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

#[cfg(test)]
mod tests {
    use super::{EventStream, api_status_error, collect_stream};
    use crate::error::Result;

    #[test]
    fn consecutive_completions_reuse_an_available_http_connection() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::time::{Duration, Instant};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut connection: Option<BufReader<TcpStream>> = None;
            let mut connections = 0;
            let deadline = Instant::now() + Duration::from_secs(5);
            for _ in 0..2 {
                let mut request_line = String::new();
                loop {
                    if let Some(reader) = &mut connection {
                        if reader.read_line(&mut request_line).unwrap() > 0 {
                            break;
                        }
                        connection = None;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for a completion"
                    );
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            connection = Some(BufReader::new(stream));
                            connections += 1;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("accept failed: {error}"),
                    }
                }
                assert!(request_line.starts_with("POST /chat/completions "));
                let reader = connection.as_mut().unwrap();
                let mut length = None;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut request = vec![0; length.unwrap()];
                reader.read_exact(&mut request).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
                assert_eq!(request["stream"], true);
                let body = concat!(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
                    "data: [DONE]\n\n"
                );
                write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
                reader.get_mut().flush().unwrap();
            }
            connections
        });
        let client = super::Client::for_test(&base_url);
        for _ in 0..2 {
            let outcome = client
                .stream(&[], &[], super::DEFAULT_MODEL, None, &mut |_| Ok(()))
                .unwrap();
            assert_eq!(outcome.content, "Hi");
        }
        assert_eq!(
            server.join().unwrap(),
            1,
            "each completion opened a new connection"
        );
    }

    #[test]
    fn chunked_responses_stream_immediately_and_finish_at_done() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (signal, observed) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(socket);
            let mut length = None;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
                if line == "\r\n" {
                    break;
                }
            }
            reader.read_exact(&mut vec![0; length.unwrap()]).unwrap();
            let socket = reader.get_mut();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
            let first = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";
            write!(socket, "{:x}\r\n{first}\r\n", first.len()).unwrap();
            socket.flush().unwrap();
            // The client must deliver the first delta before we finish SSE.
            assert_eq!(
                observed.recv_timeout(Duration::from_secs(2)).unwrap(),
                "delta"
            );
            let done = "data: [DONE]\n\n";
            write!(socket, "{:x}\r\n{done}\r\n", done.len()).unwrap();
            socket.flush().unwrap();
            // Waiting for HTTP EOF just to recycle a connection would hold
            // up an otherwise complete answer. Finish at the SSE marker.
            assert_eq!(
                observed.recv_timeout(Duration::from_secs(2)).unwrap(),
                "done"
            );
        });
        let client = super::Client::for_test(&base_url);
        let mut deltas = String::new();
        let outcome = client
            .stream(&[], &[], super::DEFAULT_MODEL, None, &mut |delta| {
                deltas.push_str(delta);
                signal.send("delta").unwrap();
                Ok(())
            })
            .unwrap();
        signal.send("done").unwrap();
        assert_eq!(deltas, "Hi");
        assert_eq!(outcome.content, "Hi");
        server.join().unwrap();
    }

    struct CredentialsFixture(std::path::PathBuf);

    impl CredentialsFixture {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "wut-credentials-test-{}-{nonce}",
                std::process::id()
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }

        fn write(&self, contents: &[u8], mode: u32) -> std::path::PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = self.0.join("credentials");
            std::fs::write(&path, contents).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }
    }

    impl Drop for CredentialsFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn private_credentials_load_and_missing_or_empty_credentials_do_not() {
        let fixture = CredentialsFixture::new();
        assert_eq!(
            super::read_credentials(&fixture.0.join("credentials")).unwrap(),
            None
        );
        let path = fixture.write(b"  test-secret\n", 0o600);
        assert_eq!(
            super::read_credentials(&path).unwrap().as_deref(),
            Some("test-secret")
        );
        fixture.write(b" \n", 0o600);
        assert_eq!(super::read_credentials(&path).unwrap(), None);
    }

    #[test]
    fn credentials_require_private_file_and_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = CredentialsFixture::new();
        for mode in [0o644, 0o640, 0o620, 0o604] {
            let path = fixture.write(b"test-secret", mode);
            let error = super::read_credentials(&path).unwrap_err();
            assert!(!error.to_string().contains("test-secret"));
        }
        let path = fixture.write(b"test-secret", 0o600);
        for mode in [0o755, 0o750, 0o720] {
            std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(super::read_credentials(&path).is_err());
        }
    }

    #[test]
    fn credentials_reject_symlinks_and_hard_links() {
        use std::os::unix::fs::symlink;
        let fixture = CredentialsFixture::new();
        let target = fixture.write(b"test-secret", 0o600);
        let alias = fixture.0.join("alias");
        symlink(&fixture.0, &alias).unwrap();
        assert!(super::read_credentials(&alias.join("credentials")).is_err());
        std::fs::rename(&target, fixture.0.join("original")).unwrap();
        symlink("original", &target).unwrap();
        assert!(super::read_credentials(&target).is_err());
        std::fs::remove_file(&target).unwrap();
        std::fs::hard_link(fixture.0.join("original"), &target).unwrap();
        assert!(super::read_credentials(&target).is_err());
    }

    #[test]
    fn credentials_reject_directories_and_fifos_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let fixture = CredentialsFixture::new();
        let path = fixture.0.join("credentials");
        std::fs::create_dir(&path).unwrap();
        assert!(super::read_credentials(&path).is_err());
        std::fs::remove_dir(&path).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid NUL-terminated path in our private test directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(super::read_credentials(&path).is_err());
    }

    #[test]
    fn credentials_reject_oversized_and_non_utf8_files() {
        let fixture = CredentialsFixture::new();
        for bytes in [vec![b'x'; 4097], vec![0xff]] {
            let path = fixture.write(&bytes, 0o600);
            assert!(super::read_credentials(&path).is_err());
        }
    }

    #[test]
    fn credentials_reject_foreign_ownership() {
        use std::os::unix::ffi::OsStrExt;
        let fixture = CredentialsFixture::new();
        let path = fixture.write(b"test-secret", 0o600);
        // Only root can construct a file owned by another uid for this test.
        // SAFETY: geteuid has no arguments or memory requirements.
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid NUL-terminated path to our test file.
        assert_eq!(unsafe { libc::chown(name.as_ptr(), 1, 1) }, 0);
        assert!(super::read_credentials(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(fixture.0.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a valid NUL-terminated path to our test directory.
        assert_eq!(unsafe { libc::chown(name.as_ptr(), 1, 1) }, 0);
        assert!(super::read_credentials(&path).is_err());
    }

    fn collect(body: &str) -> (Result<super::Outcome>, String) {
        let mut streamed = String::new();
        let outcome = collect_stream(EventStream::new(body.as_bytes()), &mut |delta| {
            streamed.push_str(delta);
            Ok(())
        });
        (outcome, streamed)
    }

    #[test]
    fn streams_content_until_the_done_marker() {
        let (outcome, streamed) = collect(concat!(
            ": keep-alive\n\n",
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\r\n\r\n",
            "data:{\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: [DONE]\n\n",
        ));
        let outcome = outcome.unwrap();
        assert_eq!(outcome.content, "Hello");
        assert_eq!(streamed, "Hello");
        assert!(outcome.tool_calls.is_empty());
    }

    #[test]
    fn merges_tool_call_fragments_by_index() {
        let (outcome, _) = collect(concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
            "{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"pa\"}},",
            "{\"index\":1,\"id\":\"b\",\"function\":{\"name\":\"ls\",\"arguments\":\"{}\"}}",
            "]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[",
            "{\"index\":0,\"function\":{\"arguments\":\"th\\\":\\\"x\\\"}\"}}",
            "]}}]}\n",
            "data: [DONE]\n",
        ));
        let calls = outcome.unwrap().tool_calls;
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (
                calls[0].id.as_str(),
                calls[0].name.as_str(),
                calls[0].arguments.as_str()
            ),
            ("a", "read", "{\"path\":\"x\"}")
        );
        assert_eq!(
            (calls[1].name.as_str(), calls[1].arguments.as_str()),
            ("ls", "{}")
        );
    }

    #[test]
    fn a_stream_cut_off_before_done_is_an_error() {
        let (outcome, streamed) =
            collect("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n");
        assert_eq!(streamed, "partial");
        let error = outcome.unwrap_err().to_string();
        assert!(
            error.contains("ended before its completion marker"),
            "{error}"
        );
    }

    #[test]
    fn stream_errors_and_bad_json_are_reported() {
        let (outcome, _) = collect("data: {\"error\":{\"message\":\"quota exceeded\"}}\n");
        let error = outcome.unwrap_err().to_string();
        assert!(error.contains("quota exceeded"), "{error}");

        let (outcome, _) = collect("data: {not json\n");
        let error = outcome.unwrap_err().to_string();
        assert!(error.contains("could not decode"), "{error}");
    }

    #[test]
    fn http_errors_keep_the_detail_and_suggest_a_fix() {
        let error = api_status_error(401, r#"{"error":{"message":"bad key"}}"#);
        assert!(error.to_string().contains("HTTP 401: bad key"), "{error}");
        assert!(error.help().contains("API key"));

        let error = api_status_error(429, r#"{"message":"slow down"}"#);
        assert!(error.to_string().contains("HTTP 429: slow down"));
        assert!(error.help().contains("rate limit"));

        let error = api_status_error(503, "  ");
        assert_eq!(
            error.to_string(),
            "the inference endpoint returned HTTP 503"
        );
        assert!(error.help().contains("temporarily unavailable"));
    }
}

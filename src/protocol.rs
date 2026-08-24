use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, ExitStatus, Stdio};

use serde_json::Value;

use crate::agent::Definition;
use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Claude,
    Codex,
    Cursor,
    Grok,
    OpenCode,
    Pi,
}

#[derive(Debug)]
pub struct Invocation {
    pub agent_id: &'static str,
    pub agent_name: &'static str,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub kind: Kind,
}

impl Invocation {
    pub fn new(definition: &Definition, kind: Kind) -> Self {
        Self {
            agent_id: definition.id,
            agent_name: definition.name,
            program: definition.program(),
            args: Vec::new(),
            env: Vec::new(),
            kind,
        }
    }

    pub fn arg(&mut self, value: impl Into<OsString>) {
        self.args.push(value.into());
    }

    pub fn args<I, S>(&mut self, values: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(values.into_iter().map(Into::into));
    }

    pub fn env(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        self.env.push((key.into(), value.into()));
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Response {
    pub answer: String,
    pub session_id: String,
    pub streamed: bool,
}

const STDERR_LIMIT: usize = 1_024;
const STDERR_HEAD_LIMIT: usize = STDERR_LIMIT / 2;
const STDERR_TAIL_LIMIT: usize = STDERR_LIMIT - STDERR_HEAD_LIMIT;
const STDERR_TRUNCATION_MARKER: &str = "\n[provider stderr truncated]\n";

struct StderrCapture {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl StderrCapture {
    fn into_detail(self) -> String {
        let mut bytes = self.head.clone();
        bytes.extend(self.tail.iter().copied());
        let detail = String::from_utf8_lossy(&bytes).trim().to_owned();
        if self.total <= STDERR_LIMIT && detail.len() <= STDERR_LIMIT {
            return detail;
        }

        let payload_limit = STDERR_LIMIT - STDERR_TRUNCATION_MARKER.len();
        let head_limit = payload_limit / 2;
        let tail_limit = payload_limit - head_limit;
        let head = String::from_utf8_lossy(&self.head);
        let tail_bytes = self.tail.iter().copied().collect::<Vec<_>>();
        let tail = String::from_utf8_lossy(&tail_bytes);
        format!(
            "{}{}{}",
            utf8_prefix(&head, head_limit),
            STDERR_TRUNCATION_MARKER,
            utf8_suffix(&tail, tail_limit)
        )
        .trim()
        .to_owned()
    }
}

fn utf8_prefix(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn utf8_suffix(value: &str, limit: usize) -> &str {
    let mut start = value.len().saturating_sub(limit);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
}

fn capture_stderr(mut stderr: impl Read) -> StderrCapture {
    let mut head = Vec::with_capacity(STDERR_HEAD_LIMIT);
    let mut tail = std::collections::VecDeque::with_capacity(STDERR_TAIL_LIMIT);
    let mut total = 0_usize;
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
            Ok(read) => read,
        };
        total = total.saturating_add(read);
        for byte in &buffer[..read] {
            if head.len() < STDERR_HEAD_LIMIT {
                head.push(*byte);
            } else {
                if tail.len() == STDERR_TAIL_LIMIT {
                    tail.pop_front();
                }
                tail.push_back(*byte);
            }
        }
    }
    StderrCapture { head, tail, total }
}

pub(crate) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub(crate) fn bounded_output(command: &mut Command) -> std::io::Result<BoundedOutput> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout is available");
    let stderr = child.stderr.take().expect("piped stderr is available");
    let stderr_reader = std::thread::spawn(move || capture_stderr(stderr));

    let mut stdout_bytes = Vec::new();
    let stdout_result = stdout.read_to_end(&mut stdout_bytes);
    if stdout_result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("provider stderr reader panicked"))?;
    stdout_result?;

    Ok(BoundedOutput {
        status,
        stdout: stdout_bytes,
        stderr: stderr.into_detail(),
    })
}

pub fn run(
    invocation: Invocation,
    on_delta: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Response> {
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .envs(invocation.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::new(format!(
                "{} is not installed or '{}' is not on PATH",
                invocation.agent_name,
                invocation.program.to_string_lossy()
            ))
            .hint(format!(
                "install and authenticate {}, then retry",
                invocation.agent_name
            ))
        } else {
            Error::new(format!(
                "could not start {}: {error}",
                invocation.agent_name
            ))
        }
    })?;

    let stdout = child.stdout.take().expect("piped stdout is available");
    let stderr = child.stderr.take().expect("piped stderr is available");
    let stderr_reader = std::thread::spawn(move || capture_stderr(stderr));

    let mut decoder = Decoder::new(invocation.kind, invocation.agent_name);
    let mut stream_error = None;
    for line in BufReader::new(stdout).lines() {
        let result = line
            .map_err(|error| {
                Error::new(format!(
                    "could not read {} output: {error}",
                    invocation.agent_name
                ))
            })
            .and_then(|line| decoder.consume_line(&line, on_delta));
        if let Err(error) = result {
            stream_error = Some(error);
            let _ = child.kill();
            break;
        }
    }

    let status = child.wait().map_err(|error| {
        Error::new(format!(
            "could not wait for {}: {error}",
            invocation.agent_name
        ))
    })?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| Error::new(format!("could not read {} errors", invocation.agent_name)))?;

    if let Some(error) = stream_error {
        return Err(error);
    }
    if !status.success() {
        let detail = stderr.into_detail();
        if detail.trim().is_empty() {
            return Err(Error::new(format!(
                "{} exited with {status}",
                invocation.agent_name
            )));
        }
        return Err(Error::new(format!(
            "{} failed: {}",
            invocation.agent_name, detail
        )));
    }
    decoder.finish()
}

struct Decoder {
    kind: Kind,
    agent_name: &'static str,
    streamed_answer: String,
    final_answer: Option<String>,
    session_id: Option<String>,
    reported_error: Option<String>,
}

impl Decoder {
    fn new(kind: Kind, agent_name: &'static str) -> Self {
        Self {
            kind,
            agent_name,
            streamed_answer: String::new(),
            final_answer: None,
            session_id: None,
            reported_error: None,
        }
    }

    fn consume_line(
        &mut self,
        line: &str,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        if line.trim().is_empty() {
            return Ok(());
        }
        let event: Value = serde_json::from_str(line).map_err(|error| {
            Error::new(format!(
                "could not parse {} JSON output: {error}",
                self.agent_name
            ))
        })?;
        let delta = match self.kind {
            Kind::Cursor => self.cursor(&event),
            Kind::Grok => self.grok(&event),
            Kind::Codex => self.codex(&event),
            Kind::Claude => self.claude(&event),
            Kind::Pi => self.pi(&event),
            Kind::OpenCode => self.opencode(&event),
        };
        if let Some(delta) = delta.filter(|delta| !delta.is_empty()) {
            self.streamed_answer.push_str(&delta);
            on_delta(&delta)?;
        }
        Ok(())
    }

    fn cursor(&mut self, event: &Value) -> Option<String> {
        match event["type"].as_str() {
            Some("assistant") => {
                let text = message_text(&event["message"]);
                if event.get("timestamp_ms").is_some() {
                    text
                } else {
                    self.final_answer = text;
                    None
                }
            }
            Some("result") => {
                self.capture_session(event, "session_id");
                if event["is_error"].as_bool() == Some(true) {
                    self.reported_error = event["result"].as_str().map(str::to_owned);
                } else if self.final_answer.is_none() {
                    self.final_answer = event["result"].as_str().map(str::to_owned);
                }
                None
            }
            _ => None,
        }
    }

    fn grok(&mut self, event: &Value) -> Option<String> {
        match event["type"].as_str() {
            Some("text") => event["data"].as_str().map(str::to_owned),
            Some("error") => {
                self.reported_error = event["message"].as_str().map(str::to_owned);
                None
            }
            Some("end") => {
                self.capture_session(event, "session_id");
                None
            }
            _ => None,
        }
    }

    fn codex(&mut self, event: &Value) -> Option<String> {
        match event["type"].as_str() {
            Some("thread.started") => {
                self.capture_session(event, "thread_id");
                None
            }
            Some("item.completed") if event["item"]["type"].as_str() == Some("agent_message") => {
                event["item"]["text"].as_str().map(str::to_owned)
            }
            Some("error") => {
                self.reported_error = event["message"]
                    .as_str()
                    .or_else(|| event["error"]["message"].as_str())
                    .map(str::to_owned);
                None
            }
            _ => None,
        }
    }

    fn claude(&mut self, event: &Value) -> Option<String> {
        match event["type"].as_str() {
            Some("system") if event["subtype"].as_str() == Some("init") => {
                self.capture_session(event, "session_id");
                None
            }
            Some("stream_event")
                if event["event"]["type"].as_str() == Some("content_block_delta")
                    && event["event"]["delta"]["type"].as_str() == Some("text_delta") =>
            {
                event["event"]["delta"]["text"].as_str().map(str::to_owned)
            }
            Some("assistant") => {
                self.final_answer = message_text(&event["message"]);
                None
            }
            Some("result") => {
                self.capture_session(event, "session_id");
                if event["is_error"].as_bool() == Some(true) {
                    self.reported_error = event["result"].as_str().map(str::to_owned);
                } else if self.final_answer.is_none() {
                    self.final_answer = event["result"].as_str().map(str::to_owned);
                }
                None
            }
            _ => None,
        }
    }

    fn pi(&mut self, event: &Value) -> Option<String> {
        match event["type"].as_str() {
            Some("session") => {
                self.capture_session(event, "id");
                None
            }
            Some("message_update")
                if event["assistantMessageEvent"]["type"].as_str() == Some("text_delta") =>
            {
                event["assistantMessageEvent"]["delta"]
                    .as_str()
                    .map(str::to_owned)
            }
            Some("message_end") if event["message"]["role"].as_str() == Some("assistant") => {
                if matches!(
                    event["message"]["stopReason"].as_str(),
                    Some("error" | "aborted")
                ) {
                    self.reported_error =
                        event["message"]["errorMessage"].as_str().map(str::to_owned);
                }
                self.final_answer = message_text(&event["message"]);
                None
            }
            _ => None,
        }
    }

    fn opencode(&mut self, event: &Value) -> Option<String> {
        if self.session_id.is_none() {
            self.capture_session(event, "sessionID");
        }
        match event["type"].as_str() {
            Some("text") => event["part"]["text"].as_str().map(str::to_owned),
            Some("error") => {
                self.reported_error = event["error"]["data"]["message"]
                    .as_str()
                    .or_else(|| event["error"]["message"].as_str())
                    .or_else(|| event["error"]["name"].as_str())
                    .map(str::to_owned);
                None
            }
            _ => None,
        }
    }

    fn capture_session(&mut self, event: &Value, field: &str) {
        if self.session_id.is_none() {
            self.session_id = event[field].as_str().map(str::to_owned);
        }
    }

    fn finish(self) -> Result<Response> {
        if let Some(error) = self.reported_error {
            return Err(Error::new(format!(
                "{} reported an error: {error}",
                self.agent_name
            )));
        }
        let streamed = !self.streamed_answer.is_empty();
        let answer = self.final_answer.unwrap_or(self.streamed_answer);
        if answer.is_empty() {
            return Err(Error::new(format!(
                "{} completed without returning an answer",
                self.agent_name
            )));
        }
        let session_id = self.session_id.ok_or_else(|| {
            Error::new(format!(
                "{} completed without returning a session ID",
                self.agent_name
            ))
        })?;
        Ok(Response {
            answer,
            session_id,
            streamed,
        })
    }
}

fn message_text(message: &Value) -> Option<String> {
    let text = message["content"]
        .as_array()?
        .iter()
        .filter(|block| block["type"].as_str() == Some("text"))
        .filter_map(|block| block["text"].as_str())
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use std::io::{self, BufRead, BufReader, Read};

    use super::{Decoder, Kind, Response, STDERR_LIMIT, capture_stderr};

    struct InterruptedOnce {
        interrupted: bool,
        remaining: &'static [u8],
    }

    impl Read for InterruptedOnce {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.remaining.read(buffer)
        }
    }

    fn decode(kind: Kind, input: &str) -> Response {
        let mut decoder = Decoder::new(kind, "test agent");
        let mut streamed = String::new();
        for line in BufReader::new(input.as_bytes()).lines() {
            decoder
                .consume_line(&line.unwrap(), &mut |delta| {
                    streamed.push_str(delta);
                    Ok(())
                })
                .unwrap();
        }
        let response = decoder.finish().unwrap();
        assert_eq!(streamed.is_empty(), !response.streamed);
        response
    }

    #[test]
    fn decodes_cursor() {
        let response = decode(
            Kind::Cursor,
            concat!(
                "{\"type\":\"assistant\",\"timestamp_ms\":1,\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hel\"}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
                "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"c-1\",\"result\":\"hello\"}\n"
            ),
        );
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "c-1");
    }

    #[test]
    fn decodes_grok() {
        let response = decode(
            Kind::Grok,
            "{\"type\":\"text\",\"data\":\"hello\"}\n{\"type\":\"end\",\"session_id\":\"g-1\"}\n",
        );
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "g-1");
    }

    #[test]
    fn decodes_codex() {
        let response = decode(
            Kind::Codex,
            "{\"type\":\"thread.started\",\"thread_id\":\"x-1\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"hello\"}}\n",
        );
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "x-1");
    }

    #[test]
    fn decodes_claude() {
        let response = decode(
            Kind::Claude,
            concat!(
                "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"a-1\"}\n",
                "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}}\n",
                "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"a-1\",\"result\":\"hello\"}\n"
            ),
        );
        assert_eq!(response.answer, "hello");
    }

    #[test]
    fn decodes_pi() {
        let response = decode(
            Kind::Pi,
            concat!(
                "{\"type\":\"session\",\"id\":\"p-1\"}\n",
                "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"hello\"}}\n",
                "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"stopReason\":\"stop\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n"
            ),
        );
        assert_eq!(response.answer, "hello");
    }

    #[test]
    fn decodes_opencode() {
        let response = decode(
            Kind::OpenCode,
            "{\"type\":\"text\",\"sessionID\":\"o-1\",\"part\":{\"text\":\"hello\"}}\n",
        );
        assert_eq!(response.answer, "hello");
        assert_eq!(response.session_id, "o-1");
    }

    #[test]
    fn surfaces_protocol_errors() {
        let mut decoder = Decoder::new(Kind::Grok, "Grok");
        decoder
            .consume_line(
                "{\"type\":\"error\",\"message\":\"rate limited\"}",
                &mut |_| Ok(()),
            )
            .unwrap();
        assert_eq!(
            decoder.finish().unwrap_err().message(),
            "Grok reported an error: rate limited"
        );
    }

    #[test]
    fn stderr_capture_retries_interrupted_reads() {
        let capture = capture_stderr(InterruptedOnce {
            interrupted: false,
            remaining: b"final diagnostic",
        });
        assert_eq!(capture.into_detail(), "final diagnostic");
    }

    #[test]
    fn stderr_detail_stays_bounded_after_lossy_utf8_conversion() {
        let capture = capture_stderr(&vec![0xff; STDERR_LIMIT][..]);
        let detail = capture.into_detail();
        assert!(
            detail.len() <= STDERR_LIMIT,
            "detail was {} bytes",
            detail.len()
        );
        assert!(detail.contains("provider stderr truncated"));
    }

    #[test]
    fn stderr_detail_preserves_final_diagnostic_after_noisy_prefix() {
        let mut input = vec![b'x'; STDERR_LIMIT * 4];
        input.extend_from_slice(b"\nFINAL_AUTH_DIAGNOSTIC");
        let detail = capture_stderr(&input[..]).into_detail();
        assert!(
            detail.len() <= STDERR_LIMIT,
            "detail was {} bytes",
            detail.len()
        );
        assert!(detail.contains("provider stderr truncated"));
        assert!(detail.contains("FINAL_AUTH_DIAGNOSTIC"));
    }
}

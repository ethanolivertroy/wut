mod claude;
mod codex;
mod cursor;
mod grok;
mod opencode;
mod pi;

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use crate::environment;
use crate::error::{Error, Result};

const STDERR_LIMIT: usize = 1_024;
const STDERR_HEAD_LIMIT: usize = STDERR_LIMIT / 2;
const STDERR_TAIL_LIMIT: usize = STDERR_LIMIT - STDERR_HEAD_LIMIT;
const STDERR_TRUNCATION_MARKER: &str = "\n[provider stderr truncated]\n";

pub(super) struct StderrCapture {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl StderrCapture {
    pub(super) fn into_detail(self) -> String {
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

pub(super) fn capture_stderr(mut stderr: impl Read) -> StderrCapture {
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

pub(super) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub(super) fn bounded_output(command: &mut Command) -> std::io::Result<BoundedOutput> {
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

#[derive(Debug)]
pub struct Response {
    pub answer: String,
    pub session_id: String,
}

pub struct RunOptions<'a> {
    pub model: Option<&'a str>,
    pub reasoning: Option<&'a str>,
    pub instructions: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_default: bool,
    pub reasoning: Vec<ReasoningLevel>,
    pub default_reasoning: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningLevel {
    pub id: String,
    pub description: String,
}

pub trait Harness {
    fn models(&mut self) -> Result<Vec<Model>>;

    fn run(
        &mut self,
        question: &str,
        session_id: Option<&str>,
        options: RunOptions<'_>,
        on_delta: &mut dyn FnMut(&str) -> Result<()>,
    ) -> Result<Response>;
}

#[derive(Clone, Copy)]
pub enum ReasoningControl {
    Selectable,
    Managed {
        label: &'static str,
        explanation: &'static str,
    },
}

pub struct Definition {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub name: &'static str,
    pub description: &'static str,
    pub default_model: Option<&'static str>,
    pub default_reasoning: Option<&'static str>,
    pub reasoning: ReasoningControl,
    program_env: &'static str,
    legacy_program_env: &'static str,
    default_program: &'static str,
    fallback_program: Option<&'static str>,
    create: fn(OsString) -> Box<dyn Harness>,
}

impl Definition {
    fn program(&self) -> OsString {
        environment::canonical_or_legacy(
            std::env::var_os(self.program_env),
            std::env::var_os(self.legacy_program_env),
        )
        .unwrap_or_else(|| {
            select_default_program(
                self.default_program,
                self.fallback_program,
                executable_available,
            )
        })
    }

    pub fn is_available(&self) -> bool {
        executable_available(&self.program())
    }

    fn create(&self) -> Box<dyn Harness> {
        (self.create)(self.program())
    }
}

pub static DEFINITIONS: &[Definition] = &[
    Definition {
        id: "codex",
        aliases: &[],
        name: "Codex",
        description: "OpenAI Codex",
        default_model: Some("fast"),
        default_reasoning: Some("low"),
        reasoning: ReasoningControl::Selectable,
        program_env: "WUT_CODEX_BIN",
        legacy_program_env: "ASK_CODEX_BIN",
        default_program: "codex",
        fallback_program: None,
        create: |program| Box::new(codex::Codex::new(program)),
    },
    Definition {
        id: "claude",
        aliases: &["claude-code"],
        name: "Claude Code",
        description: "Anthropic Claude Code",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Managed {
            label: "Managed by Claude",
            explanation: "Claude Code manages reasoning automatically for its selected model.",
        },
        program_env: "WUT_CLAUDE_BIN",
        legacy_program_env: "ASK_CLAUDE_BIN",
        default_program: "claude",
        fallback_program: None,
        create: |program| Box::new(claude::Claude::new(program)),
    },
    Definition {
        id: "cerebras",
        aliases: &[],
        name: "Cerebras",
        description: "Cerebras Inference through OpenCode",
        default_model: Some("cerebras/gpt-oss-120b"),
        default_reasoning: Some("medium"),
        reasoning: ReasoningControl::Selectable,
        program_env: "WUT_CEREBRAS_BIN",
        legacy_program_env: "ASK_CEREBRAS_BIN",
        default_program: "opencode",
        fallback_program: None,
        create: |program| Box::new(opencode::OpenCode::cerebras(program)),
    },
    Definition {
        id: "opencode",
        aliases: &["open-code"],
        name: "OpenCode",
        description: "OpenCode coding agent",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Managed {
            label: "Managed by OpenCode",
            explanation: "OpenCode manages reasoning through model-specific variants.",
        },
        program_env: "WUT_OPENCODE_BIN",
        legacy_program_env: "ASK_OPENCODE_BIN",
        default_program: "opencode",
        fallback_program: None,
        create: |program| Box::new(opencode::OpenCode::new(program)),
    },
    Definition {
        id: "pi",
        aliases: &[],
        name: "Pi",
        description: "Pi coding agent",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Selectable,
        program_env: "WUT_PI_BIN",
        legacy_program_env: "ASK_PI_BIN",
        default_program: "pi",
        fallback_program: None,
        create: |program| Box::new(pi::Pi::new(program)),
    },
    Definition {
        id: "cursor",
        aliases: &["cursor-agent"],
        name: "Cursor",
        description: "Cursor Agent CLI",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Managed {
            label: "Managed by Cursor",
            explanation: "Cursor manages model selection and reasoning for its supported models.",
        },
        program_env: "WUT_CURSOR_BIN",
        legacy_program_env: "ASK_CURSOR_BIN",
        default_program: "cursor-agent",
        fallback_program: Some("agent"),
        create: |program| Box::new(cursor::Cursor::new(program)),
    },
    Definition {
        id: "grok",
        aliases: &["grok-cli"],
        name: "Grok",
        description: "xAI Grok Build",
        default_model: None,
        default_reasoning: None,
        reasoning: ReasoningControl::Selectable,
        program_env: "WUT_GROK_BIN",
        legacy_program_env: "ASK_GROK_BIN",
        default_program: "grok",
        fallback_program: None,
        create: |program| Box::new(grok::Grok::new(program)),
    },
];

pub fn find(name: &str) -> Option<&'static Definition> {
    DEFINITIONS
        .iter()
        .find(|definition| definition.id == name || definition.aliases.contains(&name))
}

pub fn agent_name(agent: &str) -> &str {
    find(agent).map_or(agent, |definition| definition.name)
}

pub fn resolve(name: &str) -> Result<&'static Definition> {
    find(name).ok_or_else(|| {
        let available = DEFINITIONS
            .iter()
            .map(|definition| definition.id)
            .collect::<Vec<_>>()
            .join(", ");
        Error::new(
            format!("unknown agent '{name}' (available: {available})"),
            "run 'wut --settings' to choose an installed agent",
        )
    })
}

pub fn create(name: &str) -> Result<Box<dyn Harness>> {
    Ok(resolve(name)?.create())
}

fn select_default_program(
    primary: &str,
    fallback: Option<&str>,
    available: impl Fn(&OsStr) -> bool,
) -> OsString {
    if available(OsStr::new(primary)) {
        return primary.into();
    }
    fallback
        .filter(|program| available(OsStr::new(program)))
        .unwrap_or(primary)
        .into()
}

fn executable_available(program: &OsStr) -> bool {
    let program = Path::new(program);
    if program.components().count() > 1 {
        return is_executable(program);
    }

    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|directory| is_executable(&directory.join(program)))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io::{self, Cursor, Read};

    use super::{
        STDERR_LIMIT, capture_stderr, executable_available, resolve, select_default_program,
    };

    struct InterruptedThen {
        interrupted: bool,
        inner: Cursor<Vec<u8>>,
    }

    impl Read for InterruptedThen {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.inner.read(buffer)
        }
    }

    #[test]
    fn cursor_default_program_falls_back_to_legacy_agent() {
        let only_agent = |program: &OsStr| program == "agent";
        assert_eq!(
            select_default_program("cursor-agent", Some("agent"), only_agent),
            "agent"
        );
        let both = |program: &OsStr| program == "cursor-agent" || program == "agent";
        assert_eq!(
            select_default_program("cursor-agent", Some("agent"), both),
            "cursor-agent"
        );
    }

    #[test]
    fn detects_absolute_executables_and_missing_paths() {
        let current = std::env::current_exe().unwrap();

        assert!(executable_available(current.as_os_str()));
        assert!(!executable_available(OsStr::new(
            "/definitely/not/a/wut-agent"
        )));
    }

    #[test]
    fn stderr_capture_retries_interrupted_reads() {
        let reader = InterruptedThen {
            interrupted: false,
            inner: Cursor::new(b"final diagnostic".to_vec()),
        };
        assert_eq!(capture_stderr(reader).into_detail(), "final diagnostic");
    }

    #[test]
    fn stderr_capture_bounds_malformed_utf8_and_keeps_the_tail() {
        let mut bytes = vec![0xff; 8_192];
        bytes.extend_from_slice(b"FINAL_AUTH_DIAGNOSTIC");
        let detail = capture_stderr(Cursor::new(bytes)).into_detail();
        assert!(detail.len() <= STDERR_LIMIT);
        assert!(detail.contains("provider stderr truncated"));
        assert!(detail.ends_with("FINAL_AUTH_DIAGNOSTIC"));
    }

    #[test]
    fn registry_resolves_ids_and_aliases() {
        assert_eq!(resolve("codex").unwrap().id, "codex");
        assert_eq!(resolve("claude-code").unwrap().id, "claude");
        assert_eq!(resolve("cerebras").unwrap().id, "cerebras");
        assert_eq!(
            resolve("cerebras").unwrap().default_model,
            Some("cerebras/gpt-oss-120b")
        );
        assert_eq!(resolve("cerebras").unwrap().default_program, "opencode");
        assert_eq!(resolve("cerebras").unwrap().program_env, "WUT_CEREBRAS_BIN");
        assert_eq!(resolve("open-code").unwrap().id, "opencode");
        assert_eq!(resolve("cursor").unwrap().id, "cursor");
        assert_eq!(resolve("cursor").unwrap().default_program, "cursor-agent");
        assert_eq!(resolve("cursor").unwrap().program_env, "WUT_CURSOR_BIN");
        assert_eq!(
            resolve("cursor").unwrap().legacy_program_env,
            "ASK_CURSOR_BIN"
        );
        assert_eq!(resolve("cursor-agent").unwrap().id, "cursor");
        assert_eq!(resolve("grok").unwrap().id, "grok");
        assert_eq!(resolve("grok-cli").unwrap().id, "grok");
        assert!(resolve("missing").is_err());
    }
}

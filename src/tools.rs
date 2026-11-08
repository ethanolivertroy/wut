use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::cerebras::Tool;
use crate::error::{Error, Result};

/// Which tools a turn offers the model. Everything is on by default; triage
/// switches groups off when a question clearly does not need them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolSet {
    pub workspace: bool,
    pub web_search: bool,
}

impl Default for ToolSet {
    fn default() -> Self {
        Self {
            workspace: true,
            web_search: true,
        }
    }
}

impl ToolSet {
    /// The groups that offer any of the named tools.
    pub fn offering<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut set = Self {
            workspace: false,
            web_search: false,
        };
        for name in names {
            if name == "web_search" {
                set.web_search = true;
            } else if workspace_tools().iter().any(|tool| tool.name == name) {
                set.workspace = true;
            }
        }
        set
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            workspace: self.workspace || other.workspace,
            web_search: self.web_search || other.web_search,
        }
    }
}

pub fn catalog(set: ToolSet) -> Vec<Tool> {
    let mut tools = Vec::new();
    if set.workspace {
        tools.extend(workspace_tools());
    }
    if set.web_search && exa_api_key().is_some() {
        tools.push(Tool {
            name: "web_search",
            description: "Search the current web. Use for recent information or facts that are not available in the workspace. Results include source URLs that should be cited in the answer.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "A focused web search query"},
                },
                "required": ["query"],
            }),
        });
    }
    tools
}

fn workspace_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "read",
            description: "Read a text file. Paths are relative to the workspace and must stay inside it. Secret files like .env are never readable.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path, relative to the workspace"},
                },
                "required": ["path"],
            }),
        },
        Tool {
            name: "grep",
            description: "Search file contents with a regular expression. Returns matches as path:line:text.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Rust-style regular expression"},
                    "path": {"type": "string", "description": "File or directory inside the workspace to search (default: workspace root)"},
                    "glob": {"type": "string", "description": "Only search files whose name matches this glob, e.g. *.rs"},
                },
                "required": ["pattern"],
            }),
        },
        Tool {
            name: "find",
            description: "Find files whose path matches a glob pattern with * wildcards, e.g. src/*.rs.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern matched against workspace-relative paths"},
                },
                "required": ["pattern"],
            }),
        },
        Tool {
            name: "ls",
            description: "List the entries of a directory.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory inside the workspace (default: workspace root)"},
                },
            }),
        },
    ]
}

const MAX_OUTPUT_CHARS: usize = 32 * 1_024;
const MAX_MATCH_LINES: usize = 200;
const MAX_FIND_RESULTS: usize = 500;
const MAX_FILE_BYTES: usize = 512 * 1_024;
const MAX_WALK_ENTRIES: usize = 20_000;
const MAX_GLOB_BYTES: usize = 512;
const MAX_GLOB_REGEX_BYTES: usize = 256 * 1024;
/// Folders a search walk never enters: they are huge or generated, and
/// almost never what a question is about. Naming one as the path still works.
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules"];
/// Folders whose contents are credentials, matched case-insensitively.
const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".kube", ".docker"];
/// Files that hold credentials, matched case-insensitively.
const SECRET_FILES: &[&str] = &[
    "credentials",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".pgpass",
    ".git-credentials",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
];
/// Extensions of private keys and certificate bundles.
const SECRET_EXTENSIONS: &[&str] = &["pem", "key", "p12", "pfx"];
const ENV_EXAMPLES: &[&str] = &[".env.example", ".env.sample", ".env.template"];
const EXA_API_URL: &str = "https://api.exa.ai/search";
const EXA_API_KEY: &str = "EXA_API_KEY";
const WEB_SEARCH_RESULTS: u8 = 5;
/// Whether a tool returns file contents rather than only names.
pub fn reads_contents(name: &str) -> bool {
    matches!(name, "read" | "grep")
}

const WEB_SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn execute(name: &str, arguments: &Value, root: &Path) -> Result<String> {
    let empty = serde_json::Map::new();
    let arguments = match arguments {
        Value::Object(map) => map,
        Value::Null => &empty,
        _ => return Err(tool_error(name, "arguments must be a JSON object")),
    };
    match name {
        "read" => read(arguments, root),
        "grep" => grep(arguments, root),
        "find" => find(arguments, root),
        "ls" => list(arguments, root),
        "web_search" => web_search(arguments),
        other => Err(Error::new(
            format!("the model called unknown tool '{other}'"),
            "try again; if it keeps happening, report it at \
             https://github.com/ethanolivertroy/wut/issues",
        )),
    }
}

/// A one-line summary of a tool call for the terminal, such as
/// `read src/main.rs`. Control characters are replaced so a model-chosen
/// argument cannot move the cursor or change colors.
pub fn describe_call(name: &str, arguments: &Value) -> String {
    let argument = |key: &str| arguments[key].as_str().unwrap_or_default();
    let summary = match name {
        "read" => format!("read {}", argument("path")),
        "ls" => format!("ls {}", argument("path")),
        "find" => format!("find {}", argument("pattern")),
        "grep" => {
            let mut text = format!("grep {:?}", argument("pattern"));
            if !argument("path").is_empty() {
                text.push_str(&format!(" in {}", argument("path")));
            }
            if !argument("glob").is_empty() {
                text.push_str(&format!(" ({})", argument("glob")));
            }
            text
        }
        "web_search" => format!("web search {:?}", argument("query")),
        other => other.to_owned(),
    };
    let mut summary: String = summary
        .trim_end()
        .chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect();
    if summary.chars().count() > 120 {
        summary = summary.chars().take(119).collect();
        summary.push('…');
    }
    summary
}

fn exa_api_key() -> Option<String> {
    std::env::var(EXA_API_KEY)
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

fn web_search(arguments: &serde_json::Map<String, Value>) -> Result<String> {
    let query = string_argument(arguments, "query", "web_search", true)?;
    let key = exa_api_key().ok_or_else(|| {
        tool_error(
            "web_search",
            format!("{EXA_API_KEY} is not set; answer without web search"),
        )
    })?;
    let body = json!({
        "query": query,
        "type": "instant",
        "numResults": WEB_SEARCH_RESULTS,
        "contents": {"highlights": true},
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(WEB_SEARCH_TIMEOUT)
        .timeout_read(WEB_SEARCH_TIMEOUT)
        .timeout_write(WEB_SEARCH_TIMEOUT)
        .build();
    let response = agent
        .post(EXA_API_URL)
        .set("x-api-key", &key)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|error| tool_error("web_search", format!("Exa request failed: {error}")))?;
    let response = response.into_string().map_err(|error| {
        tool_error(
            "web_search",
            format!("could not read Exa response: {error}"),
        )
    })?;
    let value: Value = serde_json::from_str(&response)
        .map_err(|error| tool_error("web_search", format!("invalid Exa response: {error}")))?;
    format_search_results(&value)
}

fn format_search_results(value: &Value) -> Result<String> {
    let results = value["results"]
        .as_array()
        .ok_or_else(|| tool_error("web_search", "Exa returned no results array"))?;
    if results.is_empty() {
        return Ok("no web results".to_owned());
    }
    let mut output = String::new();
    for (index, result) in results
        .iter()
        .take(usize::from(WEB_SEARCH_RESULTS))
        .enumerate()
    {
        let title = result["title"].as_str().unwrap_or("Untitled");
        let url = result["url"].as_str().unwrap_or_default();
        output.push_str(&format!("[{}] {title}\nURL: {url}\n", index + 1));
        if let Some(date) = result["publishedDate"].as_str() {
            output.push_str(&format!("Published: {date}\n"));
        }
        if let Some(highlights) = result["highlights"].as_array() {
            let excerpt = highlights
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" … ");
            if !excerpt.is_empty() {
                output.push_str(&format!("Excerpt: {excerpt}\n"));
            }
        }
        output.push('\n');
    }
    Ok(truncate_output(output.trim_end().to_owned()))
}

fn tool_error(name: &str, message: impl std::fmt::Display) -> Error {
    Error::new(
        format!("{name}: {message}"),
        "the model received this error and will adapt",
    )
}

fn read(arguments: &serde_json::Map<String, Value>, root: &Path) -> Result<String> {
    let path = string_argument(arguments, "path", "read", true)?;
    let path = resolve(&path, root, "read")?;
    if is_denied(&path) {
        return Err(tool_error(
            "read",
            format!("'{}' is a secret file and cannot be read", path.display()),
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        tool_error(
            "read",
            format!("could not stat '{}': {error}", path.display()),
        )
    })?;
    if metadata.is_dir() {
        return Err(tool_error(
            "read",
            format!("'{}' is a directory; use ls", path.display()),
        ));
    }
    let bytes = read_text_file(&path).map_err(|error| {
        tool_error(
            "read",
            format!("could not read '{}': {error}", path.display()),
        )
    })?;
    let mut content = String::from_utf8_lossy(&bytes).into_owned();
    if content.len() > MAX_OUTPUT_CHARS {
        content.truncate(char_floor(&content, MAX_OUTPUT_CHARS));
        content.push_str("\n[truncated]");
    }
    Ok(content)
}

fn grep(arguments: &serde_json::Map<String, Value>, root: &Path) -> Result<String> {
    let pattern = string_argument(arguments, "pattern", "grep", true)?;
    let path = string_argument(arguments, "path", "grep", false)?;
    let glob = string_argument(arguments, "glob", "grep", false)?;
    let pattern = regex::Regex::new(&pattern)
        .map_err(|error| tool_error("grep", format!("invalid pattern: {error}")))?;
    let glob = if glob.is_empty() {
        None
    } else {
        Some(Glob::compile(&glob, "grep")?)
    };

    let mut lines: Vec<String> = Vec::new();
    let mut truncated = false;
    let target = if path.is_empty() {
        None
    } else {
        Some(path.as_str())
    };
    let (targets, stopped) = search_targets(target, root)?;
    for path in targets {
        if is_denied(&path) || !path.is_file() {
            continue;
        }
        if glob
            .as_ref()
            .is_some_and(|glob| !glob.matches(&file_name(&path)))
        {
            continue;
        }
        let Ok(bytes) = read_text_file(&path) else {
            continue;
        };
        let content = String::from_utf8_lossy(&bytes);
        for (index, line) in content.lines().enumerate() {
            if pattern.is_match(line) {
                if lines.len() >= MAX_MATCH_LINES {
                    truncated = true;
                    break;
                }
                let shown: String = line.chars().take(240).collect();
                lines.push(format!("{}:{}:{}", display(&path, root), index + 1, shown));
            }
        }
        if lines.len() >= MAX_MATCH_LINES {
            break;
        }
    }
    if lines.is_empty() {
        return Ok(if stopped {
            format!("no matches in the first {MAX_WALK_ENTRIES} entries; search a narrower path")
        } else {
            "no matches".to_owned()
        });
    }
    let mut output = truncate_output(lines.join("\n"));
    if truncated {
        output.push_str(&format!("\n[stopped at {MAX_MATCH_LINES} matching lines]"));
    } else if stopped {
        output.push_str(&format!(
            "\n[stopped after {MAX_WALK_ENTRIES} entries; search a narrower path]"
        ));
    }
    Ok(output)
}

/// Open only regular text files, and bound the actual read as well as the
/// initial size check: a file can grow after its metadata was inspected.
fn read_text_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};
    use std::os::unix::fs::OpenOptionsExt;

    let invalid = || Error::new(ErrorKind::InvalidData, "not a readable text file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        // Do not block opening a FIFO or follow a swapped-in final symlink.
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
        return Err(invalid());
    }
    Ok(bytes)
}

fn find(arguments: &serde_json::Map<String, Value>, root: &Path) -> Result<String> {
    let pattern = string_argument(arguments, "pattern", "find", true)?;
    let pattern = Glob::compile(&pattern, "find")?;
    let mut results: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut walk = walk(root);
    for entry in walk.by_ref() {
        let path = entry.path();
        let Some(relative) = path.strip_prefix(root).ok() else {
            continue;
        };
        let text = relative.to_string_lossy();
        if text.is_empty() {
            continue;
        }
        if is_denied(path) {
            continue;
        }
        if pattern.matches(&text) || pattern.matches(&file_name(path)) {
            if results.len() >= MAX_FIND_RESULTS {
                break;
            }
            results.push(text.into_owned());
            if results.len() >= MAX_FIND_RESULTS {
                break;
            }
        }
    }
    if results.len() >= MAX_FIND_RESULTS {
        truncated = true;
    }
    if results.is_empty() {
        return Ok(if walk.stopped {
            format!("no matches in the first {MAX_WALK_ENTRIES} entries; search a narrower path")
        } else {
            "no matches".to_owned()
        });
    }
    let mut output = results.join("\n");
    if output.len() > MAX_OUTPUT_CHARS {
        output.truncate(char_floor(&output, MAX_OUTPUT_CHARS));
        output.push_str("\n[truncated]");
    } else if truncated {
        output.push_str(&format!("\n[stopped at {MAX_FIND_RESULTS} results]"));
    } else if walk.stopped {
        output.push_str(&format!(
            "\n[stopped after {MAX_WALK_ENTRIES} entries; search a narrower path]"
        ));
    }
    Ok(output)
}

fn list(arguments: &serde_json::Map<String, Value>, root: &Path) -> Result<String> {
    let path = string_argument(arguments, "path", "ls", false)?;
    let path = if path.is_empty() {
        root.to_owned()
    } else {
        resolve(&path, root, "ls")?
    };
    if is_denied(&path) {
        return Err(tool_error(
            "ls",
            format!("'{}' holds secrets and cannot be listed", path.display()),
        ));
    }
    let entries = std::fs::read_dir(&path).map_err(|error| {
        tool_error(
            "ls",
            format!("could not read '{}': {error}", path.display()),
        )
    })?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return Ok("(empty)".to_owned());
    }
    Ok(names.join("\n"))
}

/// The files grep searches, and whether the walk hit its entry limit.
fn search_targets(path: Option<&str>, root: &Path) -> Result<(Vec<PathBuf>, bool)> {
    let path = match path {
        Some(path) => resolve(path, root, "grep")?,
        None => root.to_owned(),
    };
    if path.is_file() {
        return Ok((vec![path], false));
    }
    // Only regular files: a symlink found while walking could point anywhere.
    let mut walk = walk(&path);
    let files = walk
        .by_ref()
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect();
    Ok((files, walk.stopped))
}

/// Walks `start` without following symlinks, skipping secret and generated
/// folders below it, and stopping after `MAX_WALK_ENTRIES` entries.
fn walk(start: &Path) -> Walk {
    walk_limited(start, MAX_WALK_ENTRIES)
}

fn walk_limited(start: &Path, limit: usize) -> Walk {
    let entries = walkdir::WalkDir::new(start)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !(is_denied(entry.path())
                    || SKIPPED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()))
        });
    Walk {
        entries: Box::new(entries),
        seen: 0,
        limit,
        stopped: false,
    }
}

struct Walk {
    entries: Box<dyn Iterator<Item = walkdir::Result<walkdir::DirEntry>>>,
    seen: usize,
    limit: usize,
    stopped: bool,
}

impl Iterator for Walk {
    type Item = walkdir::DirEntry;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.seen >= self.limit || crate::cancel::requested() {
                self.stopped = true;
                return None;
            }
            self.seen += 1;
            match self.entries.next()? {
                Ok(entry) => return Some(entry),
                Err(_) => continue,
            }
        }
    }
}

fn string_argument(
    arguments: &serde_json::Map<String, Value>,
    key: &str,
    tool: &str,
    required: bool,
) -> Result<String> {
    match arguments.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        None | Some(Value::Null) | Some(Value::String(_)) => {
            if required {
                Err(tool_error(tool, format!("missing '{key}' argument")))
            } else {
                Ok(String::new())
            }
        }
        Some(_) => Err(tool_error(tool, format!("'{key}' must be a string"))),
    }
}

/// Resolves a path the model asked for and keeps it inside the workspace.
/// Symlinks and `..` are resolved before the check, so neither can point a
/// tool outside it. The result is rooted at `root` so it displays relative.
fn resolve(path: &str, root: &Path, tool: &str) -> Result<PathBuf> {
    let canonical_root = root.canonicalize().map_err(|error| {
        tool_error(
            tool,
            format!("could not open the workspace '{}': {error}", root.display()),
        )
    })?;
    let candidate = Path::new(path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        canonical_root.join(candidate)
    };
    // Check the requested name before canonicalization erases it. A secret
    // path such as .env may itself be an alias to an ordinary-looking file.
    if is_denied(&joined) {
        return Err(tool_error(
            tool,
            format!("'{path}' is a secret path and cannot be accessed"),
        ));
    }
    let target = joined
        .canonicalize()
        .map_err(|error| tool_error(tool, format!("could not open '{path}': {error}")))?;
    let relative = target
        .strip_prefix(&canonical_root)
        .map_err(|_| tool_error(tool, format!("'{path}' is outside the workspace")))?;
    Ok(if relative.as_os_str().is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    })
}

fn display(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Whether a path is, or sits inside, something that holds credentials.
/// Names are compared case-insensitively because macOS and Windows
/// filesystems usually are.
pub fn is_denied(path: &Path) -> bool {
    let in_secret_dir = path.parent().is_some_and(|parent| {
        parent.components().any(|component| {
            let name = component.as_os_str().to_string_lossy().to_lowercase();
            SECRET_DIRS.contains(&name.as_str())
        })
    });
    if in_secret_dir {
        return true;
    }
    let name = file_name(path).to_lowercase();
    if SECRET_DIRS.contains(&name.as_str()) || SECRET_FILES.contains(&name.as_str()) {
        return true;
    }
    if name == ".env" || name.starts_with(".env.") {
        return !ENV_EXAMPLES.contains(&name.as_str());
    }
    Path::new(&name)
        .extension()
        .is_some_and(|extension| SECRET_EXTENSIONS.contains(&extension.to_string_lossy().as_ref()))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn truncate_output(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_CHARS {
        text.truncate(char_floor(&text, MAX_OUTPUT_CHARS));
        text.push_str("\n[truncated]");
    }
    text
}

pub(crate) fn char_floor(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[derive(Debug)]
enum Glob {
    Literal(Vec<u8>),
    Suffix(Vec<u8>),
    Pattern(regex::bytes::Regex),
}

impl Glob {
    /// Compile once per search. Common literal/suffix patterns avoid a regex;
    /// other patterns use the existing non-backtracking regex engine.
    fn compile(pattern: &str, tool: &str) -> Result<Self> {
        if pattern.len() > MAX_GLOB_BYTES {
            return Err(tool_error(
                tool,
                format!("glob exceeds {MAX_GLOB_BYTES} bytes; use a shorter pattern"),
            ));
        }
        let has_wildcard = |byte: &u8| matches!(byte, b'*' | b'?');
        if !pattern.as_bytes().iter().any(has_wildcard) {
            return Ok(Self::Literal(pattern.as_bytes().to_vec()));
        }
        if let Some(suffix) = pattern.strip_prefix('*')
            && !suffix.as_bytes().iter().any(has_wildcard)
        {
            return Ok(Self::Suffix(suffix.as_bytes().to_vec()));
        }
        let mut expression = String::from(r"\A");
        let mut previous_star = false;
        for character in pattern.chars() {
            match character {
                '*' if previous_star => continue,
                '*' => expression.push_str("[^/]*"),
                '?' => expression.push('.'),
                other => expression.push_str(&regex::escape(&other.to_string())),
            }
            previous_star = character == '*';
        }
        expression.push_str(r"\z");
        regex::bytes::RegexBuilder::new(&expression)
            // Preserve byte-oriented '?' matching, including newline and '/'.
            .unicode(false)
            .dot_matches_new_line(true)
            .size_limit(MAX_GLOB_REGEX_BYTES)
            .dfa_size_limit(MAX_GLOB_REGEX_BYTES)
            .build()
            .map(Self::Pattern)
            .map_err(|error| tool_error(tool, format!("could not compile glob: {error}")))
    }

    fn matches(&self, text: &str) -> bool {
        let text = text.as_bytes();
        match self {
            Self::Literal(literal) => text == literal,
            Self::Suffix(suffix) => text
                .strip_suffix(suffix.as_slice())
                .is_some_and(|prefix| !prefix.contains(&b'/')),
            Self::Pattern(pattern) => pattern.is_match(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        ToolSet, catalog, describe_call, execute, format_search_results, is_denied, walk_limited,
    };

    /// A fresh `workspace/` with a `secret.txt` beside it, outside the workspace.
    fn sandbox() -> (PathBuf, PathBuf) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "wut-tools-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&base);
        let workspace = base.join("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/main.rs"), "fn main() {} // needle\n").unwrap();
        std::fs::write(base.join("secret.txt"), "needle outside\n").unwrap();
        (base, workspace)
    }

    fn run(
        name: &str,
        arguments: serde_json::Value,
        root: &std::path::Path,
    ) -> Result<String, String> {
        execute(name, &arguments, root).map_err(|error| error.to_string())
    }

    // Bounded fixtures only: retain the previous implementation as an oracle.
    fn reference_glob(pattern: &[u8], text: &[u8]) -> bool {
        match (pattern.first(), text.first()) {
            (None, None) => true,
            (Some(b'*'), _) => {
                let end = text
                    .iter()
                    .position(|byte| *byte == b'/')
                    .unwrap_or(text.len());
                (0..=end).any(|index| reference_glob(&pattern[1..], &text[index..]))
            }
            (Some(b'?'), Some(_)) => reference_glob(&pattern[1..], &text[1..]),
            (Some(a), Some(b)) if a == b => reference_glob(&pattern[1..], &text[1..]),
            _ => false,
        }
    }

    #[test]
    fn compiled_globs_preserve_byte_and_separator_semantics() {
        fn words(alphabet: &[char], length: usize) -> Vec<String> {
            let mut all = vec![String::new()];
            let mut level = vec![String::new()];
            for _ in 0..length {
                level = level
                    .iter()
                    .flat_map(|prefix| {
                        alphabet
                            .iter()
                            .map(move |character| format!("{prefix}{character}"))
                    })
                    .collect();
                all.extend(level.iter().cloned());
            }
            all
        }
        let texts = words(&['a', 'b', '/', '\n'], 3);
        for pattern in words(&['a', 'b', '*', '?', '/'], 4) {
            let compiled = super::Glob::compile(&pattern, "find").unwrap();
            for text in &texts {
                assert_eq!(
                    compiled.matches(text),
                    reference_glob(pattern.as_bytes(), text.as_bytes()),
                    "{pattern:?} against {text:?}"
                );
            }
        }
        for pattern in ["é", "??", "?", "*é", "*?*", "a[b].rs", r"a\*.rs"] {
            let compiled = super::Glob::compile(pattern, "find").unwrap();
            for text in ["é", "a/b", "a[b].rs", "a\\test.rs", "\n", "☃"] {
                assert_eq!(
                    compiled.matches(text),
                    reference_glob(pattern.as_bytes(), text.as_bytes()),
                    "{pattern:?} against {text:?}"
                );
            }
        }
    }

    #[test]
    fn adversarial_globs_are_bounded_and_oversized_patterns_are_rejected() {
        let compiled = super::Glob::compile(&format!("{}b", "*a".repeat(200)), "find").unwrap();
        assert!(!compiled.matches(&"a".repeat(4096)));
        let (base, workspace) = sandbox();
        let long = "*".repeat(super::MAX_GLOB_BYTES + 1);
        assert!(
            run("find", json!({"pattern":long}), &workspace)
                .unwrap_err()
                .contains("glob exceeds")
        );
        assert!(
            run("grep", json!({"pattern":"needle","glob":long}), &workspace)
                .unwrap_err()
                .contains("glob exceeds")
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[ignore = "local synthetic glob benchmark; run with --release --ignored --nocapture"]
    fn benchmark_glob_matching() {
        use std::hint::black_box;
        use std::time::Instant;
        for (case, pattern, paths) in [
            (
                "common_suffix",
                "*.rs".to_owned(),
                (0..20_000)
                    .map(|index| format!("module_{index:05}.rs"))
                    .collect::<Vec<_>>(),
            ),
            (
                "repeated_wildcards",
                format!("{}b", "*a".repeat(8)),
                vec!["a".repeat(18); 8],
            ),
        ] {
            let compiled = super::Glob::compile(&pattern, "find").unwrap();
            let mut before = Vec::new();
            let mut after = Vec::new();
            for _ in 0..5 {
                let start = Instant::now();
                for path in &paths {
                    black_box(reference_glob(
                        black_box(pattern.as_bytes()),
                        black_box(path.as_bytes()),
                    ));
                }
                before.push(start.elapsed().as_secs_f64() * 1_000_000.0);
                let start = Instant::now();
                for path in &paths {
                    black_box(compiled.matches(black_box(path)));
                }
                after.push(start.elapsed().as_secs_f64() * 1_000_000.0);
            }
            println!(
                "{}",
                json!({"case":case,"paths_per_sample":paths.len(),"before_us":before,"after_us":after,"optimized":!cfg!(debug_assertions),"scope":"Local matcher calls only, compiled once per search; excludes filesystem work and model latency."})
            );
        }
    }

    #[test]
    fn paths_outside_the_workspace_are_refused() {
        let (base, workspace) = sandbox();
        let secret = base.join("secret.txt");
        let secret = secret.to_str().unwrap();
        for path in ["../secret.txt", "src/../../secret.txt", secret] {
            let error = run("read", json!({"path": path}), &workspace).unwrap_err();
            assert!(error.contains("outside the workspace"), "{path}: {error}");
        }
        for path in ["..", "/"] {
            let error = run("ls", json!({"path": path}), &workspace).unwrap_err();
            assert!(error.contains("outside the workspace"), "{path}: {error}");
            let error = run(
                "grep",
                json!({"pattern": "needle", "path": path}),
                &workspace,
            )
            .unwrap_err();
            assert!(error.contains("outside the workspace"), "{path}: {error}");
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn paths_inside_the_workspace_still_work() {
        let (base, workspace) = sandbox();
        let absolute = workspace.join("src/main.rs");
        for path in [
            "src/main.rs",
            "src/../src/main.rs",
            absolute.to_str().unwrap(),
        ] {
            let content = run("read", json!({"path": path}), &workspace).unwrap();
            assert!(content.contains("needle"), "{path}");
        }
        assert_eq!(run("ls", json!({"path": "."}), &workspace).unwrap(), "src/");
        assert_eq!(
            run(
                "grep",
                json!({"pattern": "needle", "path": "src"}),
                &workspace
            )
            .unwrap(),
            "src/main.rs:1:fn main() {} // needle"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_reach_outside_the_workspace() {
        let (base, workspace) = sandbox();
        std::os::unix::fs::symlink(base.join("secret.txt"), workspace.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&base, workspace.join("up")).unwrap();
        std::fs::write(workspace.join(".env"), "TOKEN=needle\n").unwrap();
        std::os::unix::fs::symlink(workspace.join(".env"), workspace.join("notes.txt")).unwrap();

        for path in ["link.txt", "up/secret.txt"] {
            let error = run("read", json!({"path": path}), &workspace).unwrap_err();
            assert!(error.contains("outside the workspace"), "{path}: {error}");
        }
        let error = run("read", json!({"path": "notes.txt"}), &workspace).unwrap_err();
        assert!(error.contains("secret file"), "{error}");
        assert_eq!(
            run("grep", json!({"pattern": "needle"}), &workspace).unwrap(),
            "src/main.rs:1:fn main() {} // needle"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn secret_named_symlinks_cannot_expose_innocuously_named_targets() {
        use std::os::unix::fs::symlink;
        let (base, workspace) = sandbox();
        std::fs::write(workspace.join("config.txt"), "needle secret\n").unwrap();
        symlink("config.txt", workspace.join(".env")).unwrap();
        std::fs::create_dir(workspace.join("config")).unwrap();
        std::fs::write(workspace.join("config/token.txt"), "needle secret\n").unwrap();
        symlink("config", workspace.join(".ssh")).unwrap();

        for path in [".env", ".ssh/token.txt"] {
            for (tool, args) in [
                ("read", json!({"path": path})),
                ("grep", json!({"path": path, "pattern": "needle"})),
            ] {
                let error = run(tool, args, &workspace).unwrap_err();
                assert!(error.contains("secret"), "{tool} {path}: {error}");
            }
        }
        assert!(run("ls", json!({"path": ".ssh"}), &workspace).is_err());
        // Ordinary in-workspace aliases are still supported.
        symlink("src/main.rs", workspace.join("main.rs")).unwrap();
        assert!(
            run("read", json!({"path": "main.rs"}), &workspace)
                .unwrap()
                .contains("needle")
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn reading_a_fifo_fails_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        use std::time::Duration;
        let (base, workspace) = sandbox();
        let path = std::ffi::CString::new(workspace.join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: path is a NUL-terminated filename in our test workspace.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(run("read", json!({"path": "pipe"}), &workspace));
        });
        let result = receive.recv_timeout(Duration::from_secs(2));
        std::fs::remove_dir_all(base).unwrap();
        let error = result.expect("read blocked on a FIFO").unwrap_err();
        assert!(error.contains("not a readable text file"), "{error}");
    }

    #[test]
    fn file_reads_enforce_size_and_text_limits() {
        use std::io::{Seek, SeekFrom, Write};
        let (base, workspace) = sandbox();
        let file = workspace.join("large.txt");
        // A sparse, oversized file must be rejected without loading it.
        let mut sparse = std::fs::File::create(&file).unwrap();
        sparse
            .seek(SeekFrom::Start(super::MAX_FILE_BYTES as u64))
            .unwrap();
        sparse.write_all(b"x").unwrap();
        assert!(run("read", json!({"path": "large.txt"}), &workspace).is_err());
        assert_eq!(
            run(
                "grep",
                json!({"path": "large.txt", "pattern": "x"}),
                &workspace
            )
            .unwrap(),
            "no matches"
        );
        std::fs::write(&file, vec![b'x'; super::MAX_FILE_BYTES]).unwrap();
        assert!(
            run("read", json!({"path": "large.txt"}), &workspace)
                .unwrap()
                .ends_with("[truncated]")
        );
        std::fs::write(&file, b"needle\0binary").unwrap();
        assert!(run("read", json!({"path": "large.txt"}), &workspace).is_err());
        assert_eq!(
            run(
                "grep",
                json!({"path": "large.txt", "pattern": "needle"}),
                &workspace
            )
            .unwrap(),
            "no matches"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn secret_files_are_denied_in_any_case() {
        for path in [
            ".env",
            ".ENV",
            ".env.local",
            "app/.Env.Production",
            ".npmrc",
            ".netrc",
            "home/.config/wut/credentials",
            ".aws/config",
            "x/.SSH/known_hosts",
            ".ssh",
            "id_ed25519",
            "certs/server.PEM",
            "tls.key",
        ] {
            assert!(is_denied(std::path::Path::new(path)), "{path}");
        }
        for path in [
            ".env.example",
            ".ENV.SAMPLE",
            "src/env.rs",
            "id_ed25519.pub",
            "docs/credentials.md",
            "keys.rs",
            ".envrc.md",
        ] {
            assert!(!is_denied(std::path::Path::new(path)), "{path}");
        }
    }

    #[test]
    fn walks_skip_generated_and_secret_folders() {
        let (base, workspace) = sandbox();
        for dir in [".git", "target/debug", "node_modules/pkg", ".ssh"] {
            std::fs::create_dir_all(workspace.join(dir)).unwrap();
            std::fs::write(workspace.join(dir).join("hit.rs"), "needle\n").unwrap();
        }
        assert_eq!(
            run("grep", json!({"pattern": "needle"}), &workspace).unwrap(),
            "src/main.rs:1:fn main() {} // needle"
        );
        assert_eq!(
            run("find", json!({"pattern": "*.rs"}), &workspace).unwrap(),
            "src/main.rs"
        );
        // Naming a skipped folder still searches it, but never a secret one.
        assert_eq!(
            run(
                "grep",
                json!({"pattern": "needle", "path": "node_modules"}),
                &workspace
            )
            .unwrap(),
            "node_modules/pkg/hit.rs:1:needle"
        );
        assert!(
            run(
                "grep",
                json!({"pattern": "needle", "path": ".ssh"}),
                &workspace
            )
            .is_err()
        );
        assert!(run("ls", json!({"path": ".ssh"}), &workspace).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn walks_stop_at_their_entry_limit() {
        let (base, workspace) = sandbox();
        let mut walk = walk_limited(&workspace, 2);
        assert_eq!(walk.by_ref().count(), 2);
        assert!(walk.stopped);
        let mut walk = walk_limited(&workspace, 100);
        assert_eq!(walk.by_ref().count(), 3);
        assert!(!walk.stopped);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn describes_tool_calls_on_one_safe_line() {
        assert_eq!(
            describe_call("read", &json!({"path": "src/main.rs"})),
            "read src/main.rs"
        );
        assert_eq!(
            describe_call(
                "grep",
                &json!({"pattern": "fn main", "path": "src", "glob": "*.rs"})
            ),
            "grep \"fn main\" in src (*.rs)"
        );
        assert_eq!(
            describe_call("web_search", &json!({"query": "rust 1.90"})),
            "web search \"rust 1.90\""
        );
        assert_eq!(describe_call("ls", &json!({})), "ls");
        assert_eq!(
            describe_call("read", &json!({"path": "a\u{1b}[2Jb\nc"})),
            "read a?[2Jb?c"
        );
        let long = describe_call("read", &json!({"path": "x".repeat(300)}));
        assert_eq!(long.chars().count(), 120);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn catalog_follows_the_tool_set() {
        let names = |set: ToolSet| {
            catalog(set)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(ToolSet::default())
                .iter()
                .filter(|name| **name != "web_search")
                .copied()
                .collect::<Vec<_>>(),
            ["read", "grep", "find", "ls"]
        );
        assert!(
            names(ToolSet {
                workspace: false,
                web_search: false,
            })
            .is_empty()
        );
        assert!(
            !names(ToolSet {
                workspace: true,
                web_search: false,
            })
            .contains(&"web_search")
        );
    }

    #[test]
    fn tool_names_map_to_their_groups() {
        let none = ToolSet {
            workspace: false,
            web_search: false,
        };
        assert_eq!(ToolSet::offering([]), none);
        assert_eq!(ToolSet::offering(["made_up"]), none);
        for name in ["read", "grep", "find", "ls"] {
            assert_eq!(
                ToolSet::offering([name]),
                ToolSet {
                    workspace: true,
                    web_search: false,
                }
            );
        }
        let web = ToolSet::offering(["web_search"]);
        assert_eq!(
            web,
            ToolSet {
                workspace: false,
                web_search: true,
            }
        );
        assert_eq!(ToolSet::offering(["ls", "web_search"]), ToolSet::default());
        assert_eq!(none.union(web), web);
        assert_eq!(web.union(ToolSet::default()), ToolSet::default());
    }

    #[test]
    fn formats_compact_search_results_with_sources() {
        let output = format_search_results(&json!({
            "results": [{
                "title": "Kubectl reference",
                "url": "https://example.com/kubectl",
                "publishedDate": "2026-09-01",
                "highlights": ["Use kubectl get pods."]
            }]
        }))
        .unwrap();
        assert_eq!(
            output,
            "[1] Kubectl reference\nURL: https://example.com/kubectl\nPublished: 2026-09-01\nExcerpt: Use kubectl get pods."
        );
    }

    #[test]
    fn handles_empty_search_results() {
        assert_eq!(
            format_search_results(&json!({"results": []})).unwrap(),
            "no web results"
        );
    }
}

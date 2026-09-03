use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::cerebras::Tool;
use crate::error::{Error, Result};

pub fn catalog() -> Vec<Tool> {
    let mut tools = vec![
        Tool {
            name: "read",
            description: "Read a text file. Paths are relative to the workspace. Secret files like .env are never readable.",
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
                    "path": {"type": "string", "description": "File or directory to search (default: workspace root)"},
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
                    "path": {"type": "string", "description": "Directory path (default: workspace root)"},
                },
            }),
        },
    ];
    if exa_api_key().is_some() {
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

const MAX_OUTPUT_CHARS: usize = 32 * 1_024;
const MAX_MATCH_LINES: usize = 200;
const MAX_FIND_RESULTS: usize = 500;
const MAX_FILE_BYTES: usize = 512 * 1_024;
const EXA_API_URL: &str = "https://api.exa.ai/search";
const EXA_API_KEY: &str = "EXA_API_KEY";
const WEB_SEARCH_RESULTS: u8 = 5;
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
    let path = resolve(&path, root)?;
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
    let bytes = std::fs::read(&path).map_err(|error| {
        tool_error(
            "read",
            format!("could not read '{}': {error}", path.display()),
        )
    })?;
    if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
        return Err(tool_error(
            "read",
            format!("'{}' is not a readable text file", path.display()),
        ));
    }
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

    let mut lines: Vec<String> = Vec::new();
    let mut truncated = false;
    let target = if path.is_empty() {
        None
    } else {
        Some(path.as_str())
    };
    for path in search_targets(target, root)? {
        if is_denied(&path) || !path.is_file() {
            continue;
        }
        if !glob.is_empty() && !glob_match(&glob, &file_name(&path)) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
            continue;
        }
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
        return Ok("no matches".to_owned());
    }
    let mut output = truncate_output(lines.join("\n"));
    if truncated {
        output.push_str(&format!("\n[stopped at {MAX_MATCH_LINES} matching lines]"));
    }
    Ok(output)
}

fn find(arguments: &serde_json::Map<String, Value>, root: &Path) -> Result<String> {
    let pattern = string_argument(arguments, "pattern", "find", true)?;
    let mut results: Vec<String> = Vec::new();
    let mut truncated = false;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
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
        if glob_match(&pattern, &text) || glob_match(&pattern, &file_name(path)) {
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
        return Ok("no matches".to_owned());
    }
    let mut output = results.join("\n");
    if output.len() > MAX_OUTPUT_CHARS {
        output.truncate(char_floor(&output, MAX_OUTPUT_CHARS));
        output.push_str("\n[truncated]");
    } else if truncated {
        output.push_str(&format!("\n[stopped at {MAX_FIND_RESULTS} results]"));
    }
    Ok(output)
}

fn list(arguments: &serde_json::Map<String, Value>, root: &Path) -> Result<String> {
    let path = string_argument(arguments, "path", "ls", false)?;
    let path = if path.is_empty() {
        root.to_owned()
    } else {
        resolve(&path, root)?
    };
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

fn search_targets(path: Option<&str>, root: &Path) -> Result<Vec<PathBuf>> {
    match path {
        Some(path) => {
            let path = resolve(path, root)?;
            if path.is_file() {
                Ok(vec![path])
            } else {
                Ok(walkdir::WalkDir::new(&path)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.into_path())
                    .collect())
            }
        }
        None => Ok(walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.into_path())
            .collect()),
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

fn resolve(path: &str, root: &Path) -> Result<PathBuf> {
    let candidate = Path::new(path);
    Ok(if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    })
}

fn display(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn is_denied(path: &Path) -> bool {
    let name = file_name(path);
    if name == ".env" || name.starts_with(".env.") {
        return name != ".env.example";
    }
    false
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

fn char_floor(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn glob_match(pattern: &str, text: &str) -> bool {
    glob_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_inner(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            let segment_end = text
                .iter()
                .position(|byte| *byte == b'/')
                .unwrap_or(text.len());
            (0..=segment_end).any(|index| glob_inner(&pattern[1..], &text[index..]))
        }
        (Some(b'?'), Some(_)) => glob_inner(&pattern[1..], &text[1..]),
        (Some(a), Some(b)) if a == b => glob_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::format_search_results;

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

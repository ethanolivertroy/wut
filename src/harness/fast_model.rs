use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::storage;

pub(super) const CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;

/// Per-agent disk memo of the concrete model the `fast` alias resolved to.
///
/// Resolving the alias needs a catalog round-trip to the provider, which is
/// pure overhead on the hot path of a one-shot question. The memo is
/// best-effort: it saves a round-trip, never fails a turn, and callers
/// invalidate it when the provider rejects the remembered model.
pub(super) struct Cache {
    agent: &'static str,
}

impl Cache {
    pub(super) const fn new(agent: &'static str) -> Self {
        Self { agent }
    }

    pub(super) fn read(&self) -> Option<String> {
        read_cached_model(&self.path()?, now())
    }

    pub(super) fn write(&self, model: &str) {
        if let Some(path) = self.path() {
            write_cached_model(&path, model, now(), self.agent);
        }
    }

    pub(super) fn invalidate(&self) {
        if let Some(path) = self.path() {
            invalidate_cached_model(&path);
        }
    }

    fn path(&self) -> Option<PathBuf> {
        cache_path_from(
            self.agent,
            std::env::var_os("XDG_CACHE_HOME"),
            std::env::var_os("HOME"),
        )
    }
}

fn cache_path_from(
    agent: &str,
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    let file = format!("wut/{agent}.json");
    if let Some(path) = xdg_cache_home.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path).join(file));
    }
    home.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".cache").join(file))
}

fn read_cached_model(path: &Path, now: u64) -> Option<String> {
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
    (age < CACHE_TTL_SECONDS).then(|| model.to_owned())
}

fn write_cached_model(path: &Path, model: &str, resolved_at: u64, agent: &str) {
    let value = json!({
        "fast_model": model,
        "resolved_at": resolved_at,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
        let _ = storage::write_private(path, &bytes, &format!("{agent} model cache"));
    }
}

fn invalidate_cached_model(path: &Path) {
    let _ = fs::remove_file(path);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        CACHE_TTL_SECONDS, cache_path_from, invalidate_cached_model, read_cached_model,
        write_cached_model,
    };

    fn unique_cache_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("wut-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn cache_round_trips_until_the_ttl_expires() {
        let directory = unique_cache_directory("fast-cache-ttl");
        let path = directory.join("codex.json");

        write_cached_model(&path, "gpt-5.3-codex-spark", 1_000, "codex");

        assert_eq!(
            read_cached_model(&path, 1_000).as_deref(),
            Some("gpt-5.3-codex-spark")
        );
        assert_eq!(
            read_cached_model(&path, 1_000 + CACHE_TTL_SECONDS - 1).as_deref(),
            Some("gpt-5.3-codex-spark")
        );
        assert_eq!(read_cached_model(&path, 1_000 + CACHE_TTL_SECONDS), None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_rejects_clock_rollback_and_malformed_entries() {
        let directory = unique_cache_directory("fast-cache-invalid");
        let path = directory.join("grok.json");

        assert_eq!(read_cached_model(&path, 1_000), None);

        write_cached_model(&path, "grok-code-fast-1", 2_000, "grok");
        assert_eq!(read_cached_model(&path, 1_999), None);

        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, b"not-json").unwrap();
        assert_eq!(read_cached_model(&path, 1_000), None);

        fs::write(&path, b"{\"fast_model\":\"\",\"resolved_at\":1000}").unwrap();
        assert_eq!(read_cached_model(&path, 1_000), None);

        fs::write(&path, b"{\"fast_model\":\"grok\"}").unwrap();
        assert_eq!(read_cached_model(&path, 1_000), None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalidating_the_cache_removes_the_entry() {
        let directory = unique_cache_directory("fast-cache-invalidate");
        let path = directory.join("codex.json");

        write_cached_model(&path, "gpt-5.3-codex-spark", 1_000, "codex");
        assert!(read_cached_model(&path, 1_000).is_some());

        invalidate_cached_model(&path);
        assert_eq!(read_cached_model(&path, 1_000), None);

        invalidate_cached_model(&path);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_paths_are_per_agent_and_never_relative() {
        assert_eq!(cache_path_from("codex", None, Some(OsString::new())), None);
        assert_eq!(cache_path_from("codex", None, None), None);
        assert_eq!(
            cache_path_from("codex", Some(OsString::from("/cache")), None),
            Some(PathBuf::from("/cache/wut/codex.json"))
        );
        assert_eq!(
            cache_path_from("grok", None, Some(OsString::from("/home/user"))),
            Some(PathBuf::from("/home/user/.cache/wut/grok.json"))
        );
    }
}

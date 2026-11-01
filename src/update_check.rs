use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::{storage, upgrade};

const CHECK_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Cache {
    checked_at: u64,
    latest: Option<String>,
    last_notified: Option<String>,
}

pub struct Check {
    notice: Option<String>,
    notification: Option<(PathBuf, String)>,
}

impl Check {
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Self::disabled();
        }
        let Some(cache_path) = cache_path() else {
            return Self::disabled();
        };
        let now = now();
        let cache = read_cache(&cache_path).unwrap_or_default();
        if let Some(notice) = notice_for(&cache) {
            return Self {
                notice: Some(notice),
                notification: Some((cache_path, cache.latest.unwrap_or_default())),
            };
        }
        if begin_refresh(&cache_path, &cache, now)
            && let Ok(executable) = std::env::current_exe()
        {
            let _ = spawn_refresh_process(&executable);
        }
        Self {
            notice: None,
            notification: None,
        }
    }

    pub fn notice(self) -> Option<String> {
        if let Some((path, version)) = self.notification {
            mark_notified(&path, &version).ok()?;
        }
        self.notice
    }

    fn disabled() -> Self {
        Self {
            notice: None,
            notification: None,
        }
    }
}

pub fn refresh() {
    let Some(cache_path) = cache_path() else {
        return;
    };
    let now = now();
    if let Ok(latest) = upgrade::latest_release_version() {
        let _ = store_latest(&cache_path, now, latest);
    }
}

fn spawn_refresh_process(executable: &Path) -> io::Result<()> {
    let mut child = Command::new(executable)
        .arg("--internal-update-check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn begin_refresh(path: &Path, fallback: &Cache, checked_at: u64) -> bool {
    let mut cache = read_cache(path).unwrap_or_else(|| fallback.clone());
    if checked_at.saturating_sub(cache.checked_at) < CHECK_INTERVAL_SECONDS {
        return false;
    }
    cache.checked_at = checked_at;
    write_cache(path, &cache).is_ok()
}

fn store_latest(path: &Path, checked_at: u64, latest: String) -> Result<()> {
    let mut cache = read_cache(path).unwrap_or_default();
    cache.checked_at = cache.checked_at.max(checked_at);
    cache.latest = Some(latest);
    write_cache(path, &cache)
}

fn mark_notified(path: &Path, version: &str) -> Result<()> {
    let mut cache = read_cache(path).unwrap_or_default();
    cache.last_notified = Some(version.to_owned());
    write_cache(path, &cache)
}

fn notice_for(cache: &Cache) -> Option<String> {
    let latest = cache.latest.as_deref()?;
    if !upgrade::is_newer_release(latest) || cache.last_notified.as_deref() == Some(latest) {
        return None;
    }
    let current = env!("CARGO_PKG_VERSION");
    Some(format!(
        "wut {latest} is available (you have {current})\nrun `wut --upgrade` to install it"
    ))
}

fn cache_path() -> Option<PathBuf> {
    cache_path_from(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

fn cache_path_from(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = xdg_cache_home.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path).join("wut/update.json"));
    }
    home.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".cache/wut/update.json"))
}

fn read_cache(path: &Path) -> Option<Cache> {
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    Some(Cache {
        checked_at: value.get("checked_at")?.as_u64()?,
        latest: optional_string(&value, "latest")?,
        last_notified: optional_string(&value, "last_notified")?,
    })
}

fn optional_string(value: &Value, key: &str) -> Option<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => None,
    }
}

fn write_cache(path: &Path, cache: &Cache) -> Result<()> {
    let value = json!({
        "checked_at": cache.checked_at,
        "latest": cache.latest,
        "last_notified": cache.last_notified,
    });
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| Error::internal(format!("could not encode update cache: {error}")))?;
    storage::write_private(path, &bytes, "update cache")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

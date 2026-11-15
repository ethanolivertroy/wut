use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::{storage, upgrade};

const CHECK_INTERVAL_SECONDS: u64 = 24 * 60 * 60;
// A crashed or failed background check may be retried after two minutes.
const REFRESH_LEASE_SECONDS: u64 = 120;
const MAX_CACHE_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Cache {
    checked_at: u64,
    refresh_started_at: Option<u64>,
    latest: Option<String>,
    last_notified: Option<String>,
}

pub enum Check {
    Inactive,
    Pending {
        path: PathBuf,
        version: String,
        message: String,
    },
}

impl Check {
    pub fn start(enabled: bool) -> Self {
        if !enabled {
            return Self::Inactive;
        }
        let Some(cache_path) = cache_path() else {
            return Self::Inactive;
        };
        let now = now();
        let cache = read_cache(&cache_path).unwrap_or_default();
        if let Some(notice) = notice_for(&cache) {
            return Self::Pending {
                path: cache_path,
                version: cache.latest.unwrap_or_default(),
                message: notice,
            };
        }
        if let Ok(executable) = std::env::current_exe() {
            let _ = schedule_refresh(&cache_path, now, || spawn_refresh_process(&executable));
        }
        Self::Inactive
    }

    /// An advisory is delivered only after a successful command. Record it
    /// after writing, so a failed command or output keeps it pending.
    pub fn finish(self, succeeded: bool) {
        self.finish_to(succeeded, &mut io::stderr().lock());
    }

    fn finish_to(self, succeeded: bool, output: &mut impl io::Write) {
        if succeeded
            && let Self::Pending {
                path,
                version,
                message,
            } = self
            && writeln!(output, "{message}")
                .and_then(|()| output.flush())
                .is_ok()
        {
            // A cache failure must not hide a notice that can be displayed.
            let _ = mark_notified(&path, &version);
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

fn schedule_refresh(
    path: &Path,
    started_at: u64,
    spawn: impl FnOnce() -> io::Result<()>,
) -> Result<bool> {
    let _lock = CacheLock::acquire(path, false)?;
    let mut cache = read_cache(path).unwrap_or_default();
    if started_at.saturating_sub(cache.checked_at) < CHECK_INTERVAL_SECONDS
        || cache
            .refresh_started_at
            .is_some_and(|time| started_at.saturating_sub(time) < REFRESH_LEASE_SECONDS)
    {
        return Ok(false);
    }
    cache.refresh_started_at = Some(started_at);
    write_cache(path, &cache)?;
    if spawn().is_err() {
        cache.refresh_started_at = None;
        write_cache(path, &cache)?;
        return Ok(false);
    }
    Ok(true)
}

fn store_latest(path: &Path, checked_at: u64, latest: String) -> Result<()> {
    let _lock = CacheLock::acquire(path, true)?;
    let mut cache = read_cache(path).unwrap_or_default();
    cache.checked_at = cache.checked_at.max(checked_at);
    cache.refresh_started_at = None;
    cache.latest = Some(latest);
    write_cache(path, &cache)
}

fn mark_notified(path: &Path, version: &str) -> Result<()> {
    let _lock = CacheLock::acquire(path, true)?;
    let mut cache = read_cache(path).unwrap_or_default();
    cache.last_notified = Some(version.to_owned());
    write_cache(path, &cache)
}

// Serialize cache read/modify/write operations across foreground and refresh
// processes. Startup uses a nonblocking lock to avoid delaying the prompt.
struct CacheLock {
    _file: File,
}

impl CacheLock {
    fn acquire(path: &Path, wait: bool) -> Result<Self> {
        let acquire = || -> io::Result<File> {
            if let Some(parent) = path.parent() {
                let mut builder = fs::DirBuilder::new();
                builder.recursive(true).mode(0o700).create(parent)?;
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path.with_extension("lock"))?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::other("update lock must be a regular file"));
            }
            let operation = libc::LOCK_EX | if wait { 0 } else { libc::LOCK_NB };
            // SAFETY: the descriptor is a live regular file; flock retains
            // the lock until this owned file is closed.
            if unsafe { libc::flock(file.as_raw_fd(), operation) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(file)
        };
        acquire().map(|file| Self { _file: file }).map_err(|error| {
            Error::new(
                format!("could not lock update cache: {error}"),
                "try again later",
            )
        })
    }
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
    let bytes = storage::read_bounded(path, MAX_CACHE_BYTES).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    Some(Cache {
        checked_at: value.get("checked_at")?.as_u64()?,
        refresh_started_at: match value.get("refresh_started_at") {
            None | Some(Value::Null) => None,
            Some(time) => Some(time.as_u64()?),
        },
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
        "refresh_started_at": cache.refresh_started_at,
        "latest": cache.latest,
        "last_notified": cache.last_notified,
    });
    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| Error::internal(format!("could not encode update cache: {error}")))?;
    storage::write_private(path, &bytes, "update cache", MAX_CACHE_BYTES)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "wut-update-test-{}-{}-{}",
                std::process::id(),
                now(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let fixture = Self(path);
            write_cache(
                &fixture.path(),
                &Cache {
                    checked_at: 42,
                    refresh_started_at: None,
                    latest: Some("999.0.0".to_owned()),
                    last_notified: None,
                },
            )
            .unwrap();
            fixture
        }

        fn path(&self) -> PathBuf {
            self.0.join("update.json")
        }

        fn check(&self) -> Check {
            let cache = read_cache(&self.path()).unwrap();
            Check::Pending {
                path: self.path(),
                message: notice_for(&cache).unwrap(),
                version: cache.latest.unwrap(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn failed_commands_leave_the_update_notice_pending() {
        let fixture = Fixture::new();
        let before = fs::read(fixture.path()).unwrap();
        let mut output = Vec::new();
        fixture.check().finish_to(false, &mut output);
        assert!(output.is_empty());
        assert_eq!(fs::read(fixture.path()).unwrap(), before);
        assert!(notice_for(&read_cache(&fixture.path()).unwrap()).is_some());
    }

    #[test]
    fn failed_launches_do_not_mark_the_cache_fresh_or_hold_a_lease() {
        let fixture = Fixture::new();
        let time = CHECK_INTERVAL_SECONDS + 100;
        assert!(
            !schedule_refresh(&fixture.path(), time, || Err(io::Error::other(
                "spawn failed"
            )))
            .unwrap()
        );
        let cache = read_cache(&fixture.path()).unwrap();
        assert_eq!(cache.checked_at, 42);
        assert_eq!(cache.refresh_started_at, None);
        assert!(schedule_refresh(&fixture.path(), time, || Ok(())).unwrap());
    }

    #[test]
    fn refresh_leases_expire_and_success_alone_marks_the_cache_fresh() {
        let fixture = Fixture::new();
        let time = CHECK_INTERVAL_SECONDS + 100;
        assert!(schedule_refresh(&fixture.path(), time, || Ok(())).unwrap());
        assert!(!schedule_refresh(&fixture.path(), time + 1, || panic!("active lease")).unwrap());
        assert!(
            schedule_refresh(&fixture.path(), time + REFRESH_LEASE_SECONDS, || Ok(())).unwrap()
        );
        store_latest(
            &fixture.path(),
            time + REFRESH_LEASE_SECONDS,
            "1000.0.0".to_owned(),
        )
        .unwrap();
        let cache = read_cache(&fixture.path()).unwrap();
        assert_eq!(cache.checked_at, time + REFRESH_LEASE_SECONDS);
        assert_eq!(cache.refresh_started_at, None);
        assert!(
            !schedule_refresh(
                &fixture.path(),
                time + REFRESH_LEASE_SECONDS + 1,
                || panic!("fresh cache")
            )
            .unwrap()
        );
    }

    #[test]
    fn startup_does_not_wait_for_a_busy_cache() {
        let fixture = Fixture::new();
        let _lock = CacheLock::acquire(&fixture.path(), true).unwrap();
        assert!(
            schedule_refresh(&fixture.path(), CHECK_INTERVAL_SECONDS + 100, || panic!(
                "busy cache"
            ))
            .is_err()
        );
    }

    #[test]
    fn concurrent_refresh_and_notification_preserve_both_updates() {
        let fixture = Fixture::new();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let path = fixture.path();
        let ready = barrier.clone();
        let refresh = std::thread::spawn(move || {
            ready.wait();
            store_latest(&path, CHECK_INTERVAL_SECONDS + 100, "1000.0.0".to_owned()).unwrap();
        });
        barrier.wait();
        mark_notified(&fixture.path(), "999.0.0").unwrap();
        refresh.join().unwrap();
        let cache = read_cache(&fixture.path()).unwrap();
        assert_eq!(cache.latest.as_deref(), Some("1000.0.0"));
        assert_eq!(cache.last_notified.as_deref(), Some("999.0.0"));
        assert!(notice_for(&cache).unwrap().contains("1000.0.0"));
    }

    #[test]
    fn delivered_notices_are_not_repeated() {
        let fixture = Fixture::new();
        let mut output = Vec::new();
        fixture.check().finish_to(true, &mut output);
        assert!(String::from_utf8(output).unwrap().contains("wut 999.0.0"));
        let cache = read_cache(&fixture.path()).unwrap();
        assert_eq!(cache.last_notified.as_deref(), Some("999.0.0"));
        assert_eq!(cache.checked_at, 42);
        assert!(notice_for(&cache).is_none());
    }

    struct FailedOutput {
        on_flush: bool,
    }

    impl io::Write for FailedOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.on_flush {
                Ok(bytes.len())
            } else {
                Err(io::Error::other("output failed"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[test]
    fn failed_writes_and_flushes_do_not_consume_notices() {
        for on_flush in [false, true] {
            let fixture = Fixture::new();
            let before = fs::read(fixture.path()).unwrap();
            fixture
                .check()
                .finish_to(true, &mut FailedOutput { on_flush });
            assert_eq!(fs::read(fixture.path()).unwrap(), before);
        }
    }

    #[test]
    fn cache_errors_do_not_hide_a_displayable_notice() {
        let fixture = Fixture::new();
        let check = fixture.check();
        fs::remove_file(fixture.path()).unwrap();
        fs::remove_dir(&fixture.0).unwrap();
        fs::write(&fixture.0, b"not a directory").unwrap();
        let mut output = Vec::new();
        check.finish_to(true, &mut output);
        assert!(String::from_utf8(output).unwrap().contains("wut 999.0.0"));
        fs::remove_file(&fixture.0).unwrap();
    }
}

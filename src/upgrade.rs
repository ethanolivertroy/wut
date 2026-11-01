use std::cmp::Ordering;
use std::ffi::OsStr;
use std::fmt;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::error::{Error, Result};

const RELEASE_URL: &str = "https://api.github.com/repos/ethanolivertroy/wut/releases/latest";
const RELEASE_ENDPOINT: &str = "repos/ethanolivertroy/wut/releases/latest";
const REPOSITORY_URL: &str = "https://github.com/ethanolivertroy/wut";
const USER_AGENT: &str = concat!("wut/", env!("CARGO_PKG_VERSION"));
const INSTALLER: &[u8] = include_bytes!("../install.sh");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version([u64; 3]);

impl Version {
    fn parse(value: &str) -> Result<Self> {
        let mut parts = value.split('.');
        let version = Self([
            component(parts.next(), value)?,
            component(parts.next(), value)?,
            component(parts.next(), value)?,
        ]);
        if parts.next().is_some() {
            return Err(invalid_version(value));
        }
        Ok(version)
    }

    fn from_tag(tag: &str) -> Result<Self> {
        let version = tag
            .strip_prefix('v')
            .ok_or_else(|| invalid_release_tag(tag))?;
        Self::parse(version).map_err(|_| invalid_release_tag(tag))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

pub fn run() -> Result<Option<String>> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| Error::internal("wut was built with an unsupported version"))?;
    let latest = latest_release()?;
    apply_upgrade(current, latest, install)
}

fn apply_upgrade(
    current: Version,
    latest: Version,
    install: impl FnOnce(Version) -> Result<()>,
) -> Result<Option<String>> {
    match latest.cmp(&current) {
        Ordering::Equal => Ok(Some(format!("wut is already up to date (v{current})"))),
        Ordering::Less => Ok(Some(format!(
            "wut v{current} is newer than the latest release (v{latest})"
        ))),
        Ordering::Greater => {
            install(latest)?;
            Ok(None)
        }
    }
}

fn latest_release() -> Result<Version> {
    let output = fetch_release()?;
    parse_release(&output)
}

pub(crate) fn latest_release_version() -> Result<String> {
    latest_release().map(|version| version.to_string())
}

pub(crate) fn is_newer_release(latest: &str) -> bool {
    let Ok(current) = Version::parse(env!("CARGO_PKG_VERSION")) else {
        return false;
    };
    Version::parse(latest).is_ok_and(|latest| latest > current)
}

fn fetch_release() -> Result<Vec<u8>> {
    if let Ok(output) = Command::new("gh")
        .args([
            "api",
            "-H",
            "Accept: application/vnd.github+json",
            RELEASE_ENDPOINT,
        ])
        .output()
        && output.status.success()
    {
        return Ok(output.stdout);
    }

    match Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            USER_AGENT,
            RELEASE_URL,
        ])
        .output()
    {
        Ok(output) => checked_output("curl", output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let output = Command::new("wget")
                .args(["-T", "30", "-t", "1", "-qO-", RELEASE_URL])
                .output()
                .map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Error::new(
                            "curl or wget is required to check for upgrades",
                            "install either command, then run 'wut --upgrade' again",
                        )
                    } else {
                        release_check_error(format!("could not start wget: {error}"))
                    }
                })?;
            checked_output("wget", output)
        }
        Err(error) => Err(release_check_error(format!(
            "could not start curl: {error}"
        ))),
    }
}

fn checked_output(program: &str, output: Output) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    let message = if detail.is_empty() {
        format!(
            "could not check GitHub releases: {program} exited with {}",
            output.status
        )
    } else {
        format!("could not check GitHub releases: {detail}")
    };
    Err(release_check_error(message))
}

fn parse_release(output: &[u8]) -> Result<Version> {
    let release: Value = serde_json::from_slice(output)
        .map_err(|error| Error::internal(format!("could not parse GitHub release: {error}")))?;
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::internal("GitHub latest release is missing 'tag_name'"))?;
    Version::from_tag(tag)
}

fn install(version: Version) -> Result<()> {
    let executable = std::env::current_exe().map_err(|error| {
        Error::internal(format!(
            "could not locate the current wut executable: {error}"
        ))
    })?;
    let directory = install_directory(&executable)?;
    let version = version.to_string();
    let mut child = Command::new("/bin/sh")
        .args(["-s", "--", version.as_str()])
        .env("WUT_INSTALL_DIR", directory)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| install_error(format!("could not start the installer: {error}")))?;

    let write = child
        .stdin
        .take()
        .expect("piped installer input is available")
        .write_all(INSTALLER);
    if let Err(error) = write {
        let _ = child.kill();
        let _ = child.wait();
        return Err(install_error(format!(
            "could not run the installer: {error}"
        )));
    }

    let status = child
        .wait()
        .map_err(|error| install_error(format!("could not wait for the installer: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(install_error("upgrade failed"))
    }
}

fn install_directory(executable: &Path) -> Result<&Path> {
    if executable.file_name() != Some(OsStr::new("wut")) {
        return Err(install_error(format!(
            "cannot upgrade renamed executable '{}'",
            executable.display()
        )));
    }
    executable
        .parent()
        .ok_or_else(|| Error::internal("could not determine wut's install directory"))
}

fn component(part: Option<&str>, value: &str) -> Result<u64> {
    let part = part.ok_or_else(|| invalid_version(value))?;
    if !part.bytes().all(|byte| byte.is_ascii_digit()) || (part.len() > 1 && part.starts_with('0'))
    {
        return Err(invalid_version(value));
    }
    part.parse().map_err(|_| invalid_version(value))
}

fn invalid_version(value: &str) -> Error {
    Error::internal(format!("invalid release version '{value}'"))
}

fn invalid_release_tag(tag: &str) -> Error {
    Error::internal(format!("latest GitHub release has invalid tag '{tag}'"))
}

fn release_check_error(message: impl Into<String>) -> Error {
    Error::new(
        message,
        "check your connection, then run 'wut --upgrade' again",
    )
}

fn install_error(message: impl Into<String>) -> Error {
    Error::new(
        message,
        format!("reinstall wut using the instructions at {REPOSITORY_URL}"),
    )
}

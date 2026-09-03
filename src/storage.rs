use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

pub fn write_private(path: &Path, bytes: &[u8], subject: &str) -> Result<()> {
    write_private_inner(path, bytes, subject, true).map(drop)
}

pub fn write_private_if_absent(path: &Path, bytes: &[u8], subject: &str) -> Result<bool> {
    write_private_inner(path, bytes, subject, false)
}

fn write_private_inner(path: &Path, bytes: &[u8], subject: &str, replace: bool) -> Result<bool> {
    let directory = parent_directory(path);
    if directory != Path::new(".") && !directory.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(directory).map_err(|error| {
            Error::new(
                format!(
                    "could not create {subject} directory '{}': {error}",
                    directory.display()
                ),
                "check the parent directory permissions and try again",
            )
        })?;
    }

    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| Error::internal(format!("invalid {subject} path")))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = directory.join(format!(".{filename}.{}.{}.tmp", std::process::id(), nonce));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| {
                Error::new(
                    format!("could not create {subject}: {error}"),
                    format!("check '{}' permissions and try again", directory.display()),
                )
            })?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| {
                Error::new(
                    format!("could not write {subject}: {error}"),
                    "check available disk space and try again",
                )
            })?;

        if replace {
            fs::rename(&temporary, path).map_err(|error| {
                Error::new(
                    format!("could not save {subject}: {error}"),
                    format!("check '{}' permissions and try again", directory.display()),
                )
            })?;
            return Ok(true);
        }

        match fs::hard_link(&temporary, path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(Error::new(
                format!("could not save {subject}: {error}"),
                format!("check '{}' permissions and try again", directory.display()),
            )),
        }
    })();

    let _ = fs::remove_file(temporary);
    result
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

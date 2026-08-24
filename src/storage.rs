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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{parent_directory, write_private, write_private_if_absent};

    #[test]
    fn bare_relative_files_use_the_current_directory() {
        assert_eq!(parent_directory(Path::new("config.json")), Path::new("."));
    }

    #[test]
    fn writes_private_files_atomically() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("wut-storage-{}-{nonce}", std::process::id()));
        let path = root.join("wut/value.json");

        write_private(&path, b"first", "test data").unwrap();
        write_private(&path, b"second", "test data").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_if_absent_writes_never_replace_existing_data() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "wut-storage-no-clobber-{}-{nonce}",
            std::process::id()
        ));
        let path = root.join("value.json");

        assert!(write_private_if_absent(&path, b"canonical", "test data").unwrap());
        assert!(!write_private_if_absent(&path, b"legacy", "test data").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"canonical");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_parent_permissions_are_preserved() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "wut-existing-parent-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

        write_private(&root.join("value.json"), b"private", "test data").unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(root.join("value.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }
}

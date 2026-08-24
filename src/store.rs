use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::error::{Error, Result};

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = nonempty_env("WUT_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    Ok(config_home()?.join("wut/config.json"))
}

pub fn session_dir() -> Result<PathBuf> {
    if let Some(path) = nonempty_env("WUT_STATE_DIR") {
        return Ok(PathBuf::from(path).join("sessions"));
    }
    Ok(state_home()?.join("wut/sessions"))
}

fn config_home() -> Result<PathBuf> {
    if let Some(path) = nonempty_env("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home()?.join(".config"))
}

fn state_home() -> Result<PathBuf> {
    if let Some(path) = nonempty_env("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path));
    }
    Ok(home()?.join(".local/state"))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::new("HOME is not set").hint("set the relevant XDG variable"))
}

fn nonempty_env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T, subject: &str) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::new(format!("could not encode {subject}: {error}")))?;
    bytes.push(b'\n');
    write_private(path, &bytes, subject)
}

fn create_private_directories(directory: &Path, subject: &str) -> Result<()> {
    if directory.as_os_str().is_empty() {
        return Ok(());
    }
    let mut missing = Vec::new();
    let mut current = Some(directory);
    while let Some(path) = current {
        if path.as_os_str().is_empty() || path.exists() {
            break;
        }
        missing.push(path.to_path_buf());
        current = path.parent();
    }

    for path in missing.into_iter().rev() {
        let result = fs::DirBuilder::new().mode(0o700).create(&path);
        match result {
            Ok(()) => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|error| {
                    Error::new(format!(
                        "could not secure {subject} directory '{}': {error}",
                        path.display()
                    ))
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(Error::new(format!(
                    "could not create '{}': {error}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

pub fn write_private(path: &Path, bytes: &[u8], subject: &str) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| Error::new(format!("invalid {subject} path")))?;
    create_private_directories(directory, subject)?;

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::new(format!("invalid {subject} filename")))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = directory.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| Error::new(format!("could not create {subject}: {error}")))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| Error::new(format!("could not write {subject}: {error}")))?;
        fs::rename(&temporary, path)
            .map_err(|error| Error::new(format!("could not save {subject}: {error}")))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(directory) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::write_private;

    #[test]
    fn atomically_writes_private_data() {
        let root = std::env::temp_dir().join(format!("wut-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = root.join("nested/value");
        write_private(&path, b"first", "test").unwrap();
        write_private(&path, b"second", "test").unwrap();
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
    fn secures_every_directory_it_creates() {
        let root = std::env::temp_dir().join(format!("wut-private-tree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let path = root.join("first/second/value");

        write_private(&path, b"data", "test").unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        for directory in [root.join("first"), root.join("first/second")] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_permissions_on_existing_parent_directories() {
        let root = std::env::temp_dir().join(format!("wut-shared-parent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

        write_private(&root.join("value"), b"data", "test").unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(root).unwrap();
    }
}

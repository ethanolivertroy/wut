use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// Read a regular file without blocking on FIFOs, bounding the actual read
/// as well as its initial size. Symlinks to regular files remain supported.
pub fn read_bounded(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a regular file",
        ));
    }
    let too_large = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file exceeds {max_bytes} bytes"),
        )
    };
    if metadata.len() > max_bytes as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(too_large());
    }
    Ok(bytes)
}

/// Writes `bytes` to `path` atomically with mode 600, creating missing
/// directories with mode 700. Reject oversized data before changing any files.
pub fn write_private(path: &Path, bytes: &[u8], subject: &str, max_bytes: usize) -> Result<()> {
    if bytes.len() > max_bytes {
        return Err(Error::new(
            format!("{subject} exceeds {max_bytes} bytes"),
            "reduce the data size before saving again",
        ));
    }
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

        fs::rename(&temporary, path).map_err(|error| {
            Error::new(
                format!("could not save {subject}: {error}"),
                format!("check '{}' permissions and try again", directory.display()),
            )
        })
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
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "wut-storage-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_regular_files_at_the_limit_and_preserves_symlink_support() {
        let directory = Directory::new();
        let path = directory.0.join("data.json");
        fs::write(&path, b"1234").unwrap();
        assert_eq!(read_bounded(&path, 4).unwrap(), b"1234");
        let link = directory.0.join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(read_bounded(&link, 4).unwrap(), b"1234");
        fs::write(&path, b"").unwrap();
        assert!(read_bounded(&path, 0).unwrap().is_empty());
    }

    #[test]
    fn rejects_oversized_and_nonregular_files() {
        let directory = Directory::new();
        let path = directory.0.join("large.json");
        fs::File::create(&path)
            .unwrap()
            .set_len(1024 * 1024)
            .unwrap();
        assert_eq!(
            read_bounded(&path, 4).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(
            read_bounded(&directory.0, 4)
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
        assert_eq!(
            read_bounded(&directory.0.join("missing"), 4)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn oversized_writes_preserve_existing_files_and_create_nothing() {
        let directory = Directory::new();
        let path = directory.0.join("data.json");
        write_private(&path, b"1234", "test data", 4).unwrap();
        assert!(write_private(&path, b"12345", "test data", 4).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"1234");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
        let missing = directory.0.join("missing/data.json");
        assert!(write_private(&missing, b"12345", "test data", 4).is_err());
        assert!(!directory.0.join("missing").exists());
    }
}

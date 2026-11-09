use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};

use crate::cerebras::Provider;
use crate::error::{Error, Result};
use crate::select::{self, Choice, Item};

const MAX_KEY_BYTES: usize = 4096;

pub fn run(provider: Option<Provider>) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(Error::new(
            "API key entry requires an interactive terminal",
            "run 'wut --set-key' from a terminal; do not pass the key as an argument",
        ));
    }
    let provider = match provider {
        Some(provider) => provider,
        None => {
            let _screen = select::Screen::enter()?;
            let providers = [Provider::Cerebras, Provider::Groq, Provider::AiGateway];
            let items: Vec<_> = providers
                .iter()
                .map(|provider| Item::new(provider.name(), "Save or replace its API key"))
                .collect();
            let Choice::Selected(index) =
                select::choose("API key", "Choose a provider", &items, 0)?
            else {
                return Ok(());
            };
            providers[index]
        }
    };
    let Some(key) = prompt(provider)? else {
        return Ok(());
    };
    let root = config_home()?;
    save_at(&root, provider, &key)?;
    eprintln!(
        "Saved {} API key with private file permissions.",
        provider.name()
    );
    if std::env::var_os(provider.env_key())
        .is_some_and(|value| !value.to_string_lossy().trim().is_empty())
    {
        eprintln!(
            "{} is set and takes precedence over the saved key.",
            provider.env_key()
        );
    }
    Ok(())
}

pub fn load(provider: Provider) -> Result<Option<String>> {
    load_at(&config_home()?, provider)
}

/// Validate without ever including token contents in a diagnostic.
pub fn validate(key: &str) -> Result<&str> {
    let key = key.trim();
    if key.is_empty()
        || key.len() > MAX_KEY_BYTES
        || !key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(Error::new(
            "API key must be a nonempty token of at most 4096 bytes, without spaces or control characters",
            "copy only the API key and run 'wut --set-key' again",
        ));
    }
    Ok(key)
}

struct RawMode(bool);

impl RawMode {
    fn enter() -> Result<Self> {
        let restore = !is_raw_mode_enabled()
            .map_err(|error| Error::terminal("could not check terminal mode", error))?;
        if restore {
            enable_raw_mode()
                .map_err(|error| Error::terminal("could not hide API key input", error))?;
        }
        let guard = Self(restore);
        execute!(io::stderr(), EnableBracketedPaste)
            .map_err(|error| Error::terminal("could not protect pasted API key input", error))?;
        Ok(guard)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), DisableBracketedPaste);
        if self.0 {
            let _ = disable_raw_mode();
        }
    }
}

fn prompt(provider: Provider) -> Result<Option<String>> {
    // Raw input disables echo and turns Ctrl-C into an event, allowing the
    // guard to restore the terminal before cancellation returns.
    let _raw = RawMode::enter()?;
    let mut output = io::stderr();
    write!(
        output,
        "{} API key (input hidden; Esc cancels): ",
        provider.name()
    )
    .and_then(|()| output.flush())
    .map_err(|error| Error::terminal("could not prompt for API key", error))?;
    let result = (|| {
        let mut key = String::new();
        loop {
            match event::read().map_err(|error| Error::terminal("could not read API key", error))? {
                Event::Key(event)
                    if matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match event.code {
                        KeyCode::Esc => return Ok(None),
                        KeyCode::Char('c' | 'd')
                            if event.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            return Ok(None);
                        }
                        KeyCode::Enter => {
                            validate(&key)?;
                            return Ok(Some(key.trim().to_owned()));
                        }
                        KeyCode::Backspace => {
                            key.pop();
                        }
                        KeyCode::Char(character)
                            if !event
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                && !character.is_control() =>
                        {
                            key.push(character);
                        }
                        _ => {}
                    }
                }
                Event::Paste(paste) => key.push_str(&paste),
                _ => {}
            }
            if key.len() > MAX_KEY_BYTES {
                return Err(Error::new(
                    "API key is too long",
                    "copy only the API key and try again",
                ));
            }
        }
    })();
    let _ = write!(output, "\r\n");
    result
}

fn config_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config"))
        .ok_or_else(|| {
            Error::new(
                "HOME is not set",
                "set XDG_CONFIG_HOME to your private config directory",
            )
        })
}

fn key_error(provider: Provider, message: impl std::fmt::Display) -> Error {
    Error::new(
        format!("could not use saved {} API key: {message}", provider.name()),
        format!(
            "use owner-only credential directories and file permissions, or run 'wut --set-key {}' to save a new key",
            provider.id()
        ),
    )
}

fn open_at(directory: &File, name: &CStr, flags: i32, mode: libc::mode_t) -> io::Result<File> {
    // SAFETY: name is NUL-terminated; directory owns a live descriptor.
    // Darwin's mode_t is u16; variadic arguments require C integer promotion.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::c_uint,
        )
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new descriptor, transferred exactly once.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(target_os = "macos")]
fn check_acl(file: &File, provider: Provider) -> Result<()> {
    // Darwin extended ACL grants can override mode bits. Reject nonempty ACLs
    // rather than silently changing the user's ACL policy.
    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut libc::c_void;
        fn acl_get_entry(
            acl: *mut libc::c_void,
            entry_id: libc::c_int,
            entry: *mut *mut libc::c_void,
        ) -> libc::c_int;
        fn acl_free(object: *mut libc::c_void) -> libc::c_int;
    }
    const ACL_TYPE_EXTENDED: libc::c_int = 0x100;
    const ACL_FIRST_ENTRY: libc::c_int = 0;
    // SAFETY: the descriptor is live and the ACL type is Darwin's extended ACL.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        // Apple's filesec implementation reports ENOENT for an unset ACL property.
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(());
        }
        return Err(key_error(
            provider,
            format!("could not verify credential ACL ({error})"),
        ));
    }
    let mut entry = std::ptr::null_mut();
    // SAFETY: acl is owned; entry receives a borrowed entry, never dereferenced.
    let status = unsafe { acl_get_entry(acl, ACL_FIRST_ENTRY, &mut entry) };
    let error = io::Error::last_os_error();
    // SAFETY: acl is the owned allocation returned above, freed exactly once.
    unsafe {
        acl_free(acl);
    }
    // Darwin uses -1/EINVAL for exhaustion, unlike Linux's ACL iterator.
    if status == -1 && error.raw_os_error() == Some(libc::EINVAL) {
        return Ok(());
    }
    Err(key_error(
        provider,
        "credential files and directories must not have extended ACL entries; remove their ACL entries before trying again",
    ))
}

#[cfg(not(target_os = "macos"))]
fn check_acl(_file: &File, _provider: Provider) -> Result<()> {
    Ok(())
}

/// Hold the provider directory open so renaming a path cannot redirect reads
/// or writes. Only managed directories are tightened; shared config parents
/// retain their existing permissions.
fn directory(root: &Path, provider: Provider, create: bool) -> Result<Option<File>> {
    if create {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(root)
            .map_err(|error| key_error(provider, error))?;
    }
    let mut directory = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(root)
    {
        Ok(directory) => directory,
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(key_error(provider, error)),
    };
    let components: &[&CStr] = match provider {
        Provider::Cerebras => &[c"wut"],
        Provider::AiGateway => &[c"wut", c"ai-gateway"],
        Provider::Groq => &[c"wut", c"groq"],
    };
    for name in components {
        if create {
            // SAFETY: the parent descriptor and constant component are valid.
            let status = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if status < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(key_error(provider, error));
                }
            }
        }
        directory = match open_at(
            &directory,
            name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        ) {
            Ok(directory) => directory,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(key_error(
                    provider,
                    format!(
                        "credential directory must not be a symlink and must be accessible ({error})"
                    ),
                ));
            }
        };
        let metadata = directory
            .metadata()
            .map_err(|error| key_error(provider, error))?;
        // SAFETY: geteuid takes no pointers and has no failure case.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(key_error(
                provider,
                "credential directory belongs to another user",
            ));
        }
        if create {
            // SAFETY: this is the already-verified directory descriptor.
            if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } < 0 {
                return Err(key_error(provider, io::Error::last_os_error()));
            }
        } else if metadata.mode() & 0o077 != 0 {
            return Err(key_error(
                provider,
                "credential directories must have mode 0700",
            ));
        }
        check_acl(&directory, provider)?;
    }
    Ok(Some(directory))
}

fn load_at(root: &Path, provider: Provider) -> Result<Option<String>> {
    let Some(directory) = directory(root, provider, false)? else {
        return Ok(None);
    };
    let file = match open_at(
        &directory,
        c"credentials",
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(key_error(
                provider,
                format!("credential file must not be a symlink and must be readable ({error})"),
            ));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|error| key_error(provider, error))?;
    // SAFETY: geteuid takes no pointers and has no failure case.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err(key_error(
            provider,
            "credential file must be a regular file owned by you, without hard links",
        ));
    }
    if metadata.mode() & 0o7177 != 0 {
        return Err(key_error(
            provider,
            "credential file must have mode 0600 (0400 is also allowed)",
        ));
    }
    check_acl(&file, provider)?;
    let mut bytes = Vec::new();
    file.take((MAX_KEY_BYTES + 3) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| key_error(provider, error))?;
    if bytes.len() > MAX_KEY_BYTES + 2 {
        return Err(key_error(provider, "credential file is too large"));
    }
    let key = std::str::from_utf8(&bytes)
        .map_err(|_| key_error(provider, "credential file is not a valid token"))?;
    Ok(Some(validate(key)?.to_owned()))
}

fn save_at(root: &Path, provider: Provider, key: &str) -> Result<()> {
    let key = validate(key)?;
    let directory = directory(root, provider, true)?
        .ok_or_else(|| key_error(provider, "credential directory is missing"))?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat points to writable storage; the constant name and fd are valid.
    let status = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            c"credentials".as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        // SAFETY: successful fstatat initialized the structure.
        let stat = unsafe { stat.assume_init() };
        // SAFETY: geteuid has no failure case.
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_uid != unsafe { libc::geteuid() }
            || stat.st_nlink != 1
        {
            return Err(key_error(
                provider,
                "existing credential file must be owned by you and must not be a symlink, special file, or hard link",
            ));
        }
    } else if io::Error::last_os_error().kind() != io::ErrorKind::NotFound {
        return Err(key_error(provider, io::Error::last_os_error()));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = CString::new(format!(".credentials.{}.{}.tmp", std::process::id(), nonce))
        .map_err(|_| Error::internal("invalid generated credential filename"))?;
    write_key(&directory, provider, key, &name)
}

fn write_key(directory: &File, provider: Provider, key: &str, name: &CStr) -> Result<()> {
    // A failed exclusive creation returns before cleanup: an existing file is
    // never removed, followed, or overwritten by a temporary-file collision.
    let mut file = open_at(
        directory,
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        0o600,
    )
    .map_err(|error| key_error(provider, error))?;
    let mut renamed = false;
    let result = (|| {
        // Enforce the promised mode even when the caller has a restrictive umask.
        // SAFETY: file is the exclusively created temporary file.
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } < 0 {
            return Err(key_error(provider, io::Error::last_os_error()));
        }
        check_acl(&file, provider)?;
        file.write_all(key.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| key_error(provider, error))?;
        // SAFETY: both paths are constants/generated names in the held directory.
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                name.as_ptr(),
                directory.as_raw_fd(),
                c"credentials".as_ptr(),
            )
        } < 0
        {
            return Err(key_error(provider, io::Error::last_os_error()));
        }
        renamed = true;
        directory
            .sync_all()
            .map_err(|error| key_error(provider, error))
    })();
    if !renamed {
        // SAFETY: only the temporary name created above is removed.
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{MAX_KEY_BYTES, directory, load_at, save_at, validate, write_key};
    use crate::cerebras::Provider;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "wut-credentials-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn key(&self, provider: Provider) -> PathBuf {
            self.0.join(provider.credentials_file())
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o777
    }
    fn chmod(path: &std::path::Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn saves_and_replaces_all_providers_privately_without_crossing_keys() {
        let root = Root::new();
        chmod(&root.0, 0o755);
        for provider in [Provider::Cerebras, Provider::Groq, Provider::AiGateway] {
            save_at(&root.0, provider, provider.id()).unwrap();
            assert_eq!(
                load_at(&root.0, provider).unwrap().as_deref(),
                Some(provider.id())
            );
            assert_eq!(mode(&root.key(provider)), 0o600);
            assert_eq!(mode(root.key(provider).parent().unwrap()), 0o700);
        }
        save_at(&root.0, Provider::AiGateway, "replacement").unwrap();
        assert_eq!(
            load_at(&root.0, Provider::AiGateway).unwrap().as_deref(),
            Some("replacement")
        );
        assert_eq!(
            load_at(&root.0, Provider::Cerebras).unwrap().as_deref(),
            Some("cerebras")
        );
        assert_eq!(
            mode(&root.0),
            0o755,
            "the shared config root is not chmodded"
        );
        assert_eq!(
            fs::read_dir(root.key(Provider::AiGateway).parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn load_rejects_broad_permissions_and_save_repairs_owned_regular_files() {
        let root = Root::new();
        let provider = Provider::AiGateway;
        save_at(&root.0, provider, "old-key").unwrap();
        let path = root.key(provider);
        chmod(&path, 0o644);
        let error = load_at(&root.0, provider).unwrap_err();
        assert!(error.to_string().contains("0600"));
        assert!(!error.to_string().contains("old-key"));
        chmod(path.parent().unwrap(), 0o755);
        assert!(load_at(&root.0, provider).is_err());
        save_at(&root.0, provider, "new-key").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        chmod(&path, 0o400);
        assert_eq!(
            load_at(&root.0, provider).unwrap().as_deref(),
            Some("new-key")
        );
    }

    #[test]
    fn rejects_symlinked_files_without_reading_or_overwriting_targets() {
        let root = Root::new();
        let provider = Provider::Cerebras;
        save_at(&root.0, provider, "old-key").unwrap();
        let path = root.key(provider);
        let target = root.0.join("elsewhere");
        fs::write(&target, "keep-this").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(load_at(&root.0, provider).is_err());
        assert!(save_at(&root.0, provider, "replacement").is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "keep-this");
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }

    #[test]
    fn rejects_managed_directory_symlinks_for_load_and_save() {
        for provider in [Provider::Cerebras, Provider::Groq, Provider::AiGateway] {
            let root = Root::new();
            save_at(&root.0, provider, "old-key").unwrap();
            let path = root.key(provider).parent().unwrap().to_path_buf();
            let moved = root.0.join("moved");
            fs::rename(&path, &moved).unwrap();
            symlink(&moved, &path).unwrap();
            assert!(load_at(&root.0, provider).is_err());
            assert!(save_at(&root.0, provider, "replacement").is_err());
            assert_eq!(
                fs::read_to_string(moved.join("credentials")).unwrap(),
                "old-key"
            );
        }
    }

    #[test]
    fn rejects_hard_links_and_special_files_without_blocking() {
        let root = Root::new();
        let provider = Provider::Cerebras;
        save_at(&root.0, provider, "old-key").unwrap();
        let path = root.key(provider);
        let link = root.0.join("other-link");
        fs::hard_link(&path, &link).unwrap();
        assert!(load_at(&root.0, provider).is_err());
        assert!(save_at(&root.0, provider, "replacement").is_err());
        fs::remove_file(&path).unwrap();
        let directory = directory(&root.0, provider, false).unwrap().unwrap();
        // SAFETY: live directory fd and constant filename; creates a test FIFO.
        assert_eq!(
            unsafe {
                libc::mkfifoat(
                    std::os::fd::AsRawFd::as_raw_fd(&directory),
                    c"credentials".as_ptr(),
                    0o600,
                )
            },
            0
        );
        assert!(load_at(&root.0, provider).is_err());
        assert!(save_at(&root.0, provider, "replacement").is_err());
        assert_eq!(fs::read_to_string(link).unwrap(), "old-key");
    }

    #[test]
    fn invalid_or_large_keys_are_rejected_without_disclosing_or_replacing_them() {
        let root = Root::new();
        let provider = Provider::Cerebras;
        save_at(&root.0, provider, "old-key").unwrap();
        for key in ["", "one two", "private-key\nother-key", "private\0key"] {
            let error = save_at(&root.0, provider, key).unwrap_err();
            assert!(!error.to_string().contains("private-key"));
            assert_eq!(fs::read_to_string(root.key(provider)).unwrap(), "old-key");
        }
        let long = "x".repeat(MAX_KEY_BYTES + 1);
        assert!(validate(&long).is_err());
        fs::write(root.key(provider), long).unwrap();
        assert!(load_at(&root.0, provider).is_err());
        fs::write(root.key(provider), "valid-token\r\n").unwrap();
        assert_eq!(
            load_at(&root.0, provider).unwrap().as_deref(),
            Some("valid-token")
        );
        fs::write(root.key(provider), "valid-token\nsecond-token").unwrap();
        assert!(load_at(&root.0, provider).is_err());
    }

    #[test]
    fn missing_keys_are_optional_and_exclusive_temp_collisions_preserve_all_files() {
        let root = Root::new();
        let provider = Provider::Cerebras;
        assert_eq!(load_at(&root.0, provider).unwrap(), None);
        save_at(&root.0, provider, "old-key").unwrap();
        let held = directory(&root.0, provider, false).unwrap().unwrap();
        let temp = root
            .key(provider)
            .parent()
            .unwrap()
            .join(".credentials.collision.tmp");
        fs::write(&temp, "keep-temp").unwrap();
        assert!(
            write_key(
                &held,
                provider,
                "replacement",
                c".credentials.collision.tmp"
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&temp).unwrap(), "keep-temp");
        assert_eq!(fs::read_to_string(root.key(provider)).unwrap(), "old-key");
        fs::remove_file(&temp).unwrap();
        fs::remove_file(root.key(provider)).unwrap();
        fs::create_dir(root.key(provider)).unwrap();
        assert!(write_key(&held, provider, "replacement", c".credentials.cleanup.tmp").is_err());
        assert!(
            !root
                .key(provider)
                .parent()
                .unwrap()
                .join(".credentials.cleanup.tmp")
                .exists()
        );
    }

    #[test]
    fn rejects_non_utf8_keys_without_disclosing_file_contents() {
        let root = Root::new();
        let provider = Provider::Cerebras;
        save_at(&root.0, provider, "old-key").unwrap();
        fs::write(root.key(provider), [0xff]).unwrap();
        assert!(load_at(&root.0, provider).is_err());
    }

    #[test]
    fn rejects_foreign_file_and_directory_ownership_when_root_can_construct_it() {
        // SAFETY: geteuid has no arguments or failure case. Non-root test runs
        // cannot construct a file owned by a different uid.
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        use std::os::unix::ffi::OsStrExt;
        let root = Root::new();
        let provider = Provider::Cerebras;
        save_at(&root.0, provider, "old-key").unwrap();
        let path = root.key(provider);
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a disposable test file, not a real credential path.
        assert_eq!(unsafe { libc::chown(name.as_ptr(), 1, 1) }, 0);
        assert!(load_at(&root.0, provider).is_err());
        assert!(save_at(&root.0, provider, "replacement").is_err());
        fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.parent().unwrap().as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a disposable test directory.
        assert_eq!(unsafe { libc::chown(name.as_ptr(), 1, 1) }, 0);
        assert!(load_at(&root.0, provider).is_err());
        assert!(save_at(&root.0, provider, "replacement").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_extended_acl_grants_even_with_private_mode_bits() {
        let root = Root::new();
        let provider = Provider::Cerebras;
        save_at(&root.0, provider, "old-key").unwrap();
        let path = root.key(provider);
        assert!(
            std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(load_at(&root.0, provider).is_err());
        save_at(&root.0, provider, "new-key").unwrap();
        assert_eq!(
            load_at(&root.0, provider).unwrap().as_deref(),
            Some("new-key")
        );
        assert!(
            std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(path.parent().unwrap())
                .status()
                .unwrap()
                .success()
        );
        assert!(load_at(&root.0, provider).is_err());
        assert!(save_at(&root.0, provider, "replacement").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "new-key");
    }
}

use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::Shutdown;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::AsRawFd;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, PermissionsExt};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use tauri::{AppHandle, Manager};

use super::{AppResult, PRIMARY_WINDOW_LABEL};

const APPLICATION_ID: &str = "com.cornellmethod.notebook";
#[cfg(debug_assertions)]
const DEBUG_APPLICATION_ID: &str = "com.cornellmethod.notebook.debug";
const CORNELL_DESKTOP_HOME_ENV: &str = "CORNELL_DESKTOP_HOME";
const INSTANCE_LOCK_FILE: &str = ".instance.lock";
const INSTANCE_OWNER_FILE: &str = ".instance.owner";
const INSTANCE_SOCKET_FILE: &str = ".instance.sock";
const INSTANCE_SOCKET_DIRECTORY_PREFIX: &str = "cmn-";
const INSTANCE_SOCKET_HASH_HEX_LENGTH: usize = 24;
const MACOS_UNIX_SOCKET_PATH_LIMIT: usize = 104;
const INSTANCE_SCHEMA_VERSION: u32 = 1;
const FOCUS_RETRY_COUNT: usize = 20;
const FOCUS_RETRY_DELAY: Duration = Duration::from_millis(100);
const FOCUS_REQUEST_READ_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_FOCUS_REQUEST_BYTES: usize = 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstanceOwner {
    schema_version: u32,
    pid: u32,
    application_id: String,
}

#[derive(Clone, Debug)]
struct InstancePaths {
    settings_directory: PathBuf,
    lock_path: PathBuf,
    owner_path: PathBuf,
    socket_path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct InstanceGuard {
    owner_path: PathBuf,
    socket_path: PathBuf,
    owner: InstanceOwner,
    socket_owned: bool,
    _lock_file: File,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if self.socket_owned {
            let _ = fs::remove_file(&self.socket_path);
        }
        remove_instance_owner_if_matches(&self.owner_path, &self.owner);
    }
}

impl InstanceGuard {
    pub(crate) fn mark_socket_owned(&mut self) {
        self.socket_owned = true;
    }

    pub(crate) fn socket_path(&self) -> PathBuf {
        self.socket_path.clone()
    }
}

#[derive(Debug)]
pub(crate) enum InstanceAcquire {
    Primary(InstanceGuard),
    Focused,
    AlreadyRunningNotReady,
}

#[derive(Debug)]
enum AdvisoryLockError {
    WouldBlock,
    Io(std::io::Error),
}

#[derive(Debug)]
enum LockAttempt {
    Acquired(File),
    WouldBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusAttempt {
    Focused,
    NotReady,
    Unavailable,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FocusSocketCheckStage {
    SocketConnect,
    MetadataLookup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SanitizedIoError {
    stage: FocusSocketCheckStage,
    kind: std::io::ErrorKind,
    raw_os_error: Option<i32>,
}

impl SanitizedIoError {
    fn from_error(stage: FocusSocketCheckStage, error: &std::io::Error) -> Self {
        Self {
            stage,
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
        }
    }

    fn diagnostic(&self) -> String {
        let stage = match self.stage {
            FocusSocketCheckStage::SocketConnect => "socket-connect",
            FocusSocketCheckStage::MetadataLookup => "metadata-lookup",
        };
        let raw_os_error = self
            .raw_os_error
            .map_or_else(|| "unset".to_string(), |code| code.to_string());
        format!(
            "stage={stage} kind={:?} raw_os_error={raw_os_error}",
            self.kind
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FocusSocketStatus {
    Missing,
    Stale,
    Active,
    Unknown,
    PermissionDenied,
    Unavailable(SanitizedIoError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusRequestError {
    RequestTooLarge,
    InvalidUtf8,
    IncompleteRead,
}

fn configured_home_directory() -> AppResult<PathBuf> {
    let value = env::var_os(CORNELL_DESKTOP_HOME_ENV)
        .or_else(|| env::var_os("HOME"))
        .ok_or_else(|| "macOS home directory is unavailable".to_string())?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("home directory must be absolute".to_string());
    }
    Ok(path)
}

#[cfg(debug_assertions)]
fn application_id_for(explicit_home_override: bool) -> &'static str {
    if explicit_home_override {
        APPLICATION_ID
    } else {
        DEBUG_APPLICATION_ID
    }
}

#[cfg(not(debug_assertions))]
fn application_id_for(_explicit_home_override: bool) -> &'static str {
    APPLICATION_ID
}

pub(crate) fn desktop_application_id() -> &'static str {
    application_id_for(env::var_os(CORNELL_DESKTOP_HOME_ENV).is_some())
}

fn application_support_root(home: &Path, application_id: &str) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join(application_id)
}

fn focus_socket_identity_for(application_id: &str, settings_directory: &Path) -> String {
    let settings_bytes = settings_directory.as_os_str().as_bytes();
    let mut identity = Vec::with_capacity(application_id.len() + 1 + settings_bytes.len());
    identity.extend_from_slice(application_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(settings_bytes);

    let hash = digest(&SHA256, &identity);
    let hex = b"0123456789abcdef";
    let mut encoded = String::with_capacity(INSTANCE_SOCKET_HASH_HEX_LENGTH);
    for byte in hash
        .as_ref()
        .iter()
        .take(INSTANCE_SOCKET_HASH_HEX_LENGTH / 2)
    {
        encoded.push(char::from(hex[(byte >> 4) as usize]));
        encoded.push(char::from(hex[(byte & 0x0f) as usize]));
    }
    encoded
}

fn fallback_focus_socket_root() -> PathBuf {
    PathBuf::from("/tmp").join(format!("{INSTANCE_SOCKET_DIRECTORY_PREFIX}{}", unsafe {
        libc::geteuid()
    }))
}

fn focus_socket_path_is_bounded(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() < MACOS_UNIX_SOCKET_PATH_LIMIT
}

fn focus_socket_path_at_for(
    temp_directory: &Path,
    settings_directory: &Path,
    application_id: &str,
) -> PathBuf {
    let identity = focus_socket_identity_for(application_id, settings_directory);
    let socket_directory = format!("{INSTANCE_SOCKET_DIRECTORY_PREFIX}{identity}");
    let candidate = temp_directory
        .join(&socket_directory)
        .join(INSTANCE_SOCKET_FILE);
    if focus_socket_path_is_bounded(&candidate) {
        return candidate;
    }

    fallback_focus_socket_root()
        .join(socket_directory)
        .join(INSTANCE_SOCKET_FILE)
}

fn focus_socket_path_at(temp_directory: &Path, settings_directory: &Path) -> PathBuf {
    focus_socket_path_at_for(temp_directory, settings_directory, desktop_application_id())
}

fn focus_socket_path(settings_directory: &Path) -> PathBuf {
    focus_socket_path_at(&env::temp_dir(), settings_directory)
}

fn instance_paths_at(settings_directory: PathBuf) -> InstancePaths {
    InstancePaths {
        lock_path: settings_directory.join(INSTANCE_LOCK_FILE),
        owner_path: settings_directory.join(INSTANCE_OWNER_FILE),
        socket_path: focus_socket_path(&settings_directory),
        settings_directory,
    }
}

fn instance_paths() -> AppResult<InstancePaths> {
    let root = application_support_root(&configured_home_directory()?, desktop_application_id());
    let settings = root.join("settings");
    fs::create_dir_all(&settings)
        .map_err(|error| format!("cannot create desktop settings directory: {error}"))?;
    Ok(instance_paths_at(settings))
}

fn prepare_focus_socket_directory(socket_path: &Path) -> AppResult<()> {
    let directory = socket_path
        .parent()
        .ok_or_else(|| "single-instance focus socket has no parent directory".to_string())?;

    let fallback_root = fallback_focus_socket_root();
    if directory.starts_with(&fallback_root) {
        fs::create_dir_all(&fallback_root)
            .map_err(|error| format!("cannot create focus socket root: {error}"))?;
        fs::set_permissions(&fallback_root, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure focus socket root: {error}"))?;
    }
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create focus socket directory: {error}"))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot secure focus socket directory: {error}"))?;
    Ok(())
}

fn instance_owner() -> InstanceOwner {
    instance_owner_with_pid(std::process::id())
}

fn instance_owner_with_pid(pid: u32) -> InstanceOwner {
    InstanceOwner {
        schema_version: INSTANCE_SCHEMA_VERSION,
        pid,
        application_id: desktop_application_id().to_string(),
    }
}

static INSTANCE_OWNER_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_write_instance_owner(path: &Path, owner: &InstanceOwner) -> AppResult<()> {
    let content = serde_json::to_vec(owner).map_err(|error| error.to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "single-instance owner marker has no parent directory".to_string())?;

    for _ in 0..16 {
        let counter = INSTANCE_OWNER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary_path = parent.join(format!(
            ".instance.owner.tmp-{}-{counter}",
            std::process::id()
        ));
        let Ok(mut temporary_file) = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        else {
            continue;
        };

        let write_result = (|| {
            temporary_file
                .write_all(&content)
                .map_err(|error| format!("cannot write single-instance owner marker: {error}"))?;
            temporary_file
                .write_all(b"\n")
                .map_err(|error| format!("cannot finish single-instance owner marker: {error}"))?;
            temporary_file
                .sync_all()
                .map_err(|error| format!("cannot sync single-instance owner marker: {error}"))
        })();
        drop(temporary_file);

        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }

        if let Err(error) = fs::rename(&temporary_path, path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(format!(
                "cannot replace single-instance owner marker: {error}"
            ));
        }
        return Ok(());
    }

    Err("cannot create a temporary single-instance owner marker".to_string())
}

fn read_instance_owner(path: &Path) -> AppResult<InstanceOwner> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read single-instance marker: {error}"))?;
    serde_json::from_str(&content)
        .map_err(|error| format!("single-instance marker is invalid: {error}"))
}

fn remove_instance_owner_if_matches(path: &Path, expected: &InstanceOwner) {
    if read_instance_owner(path).is_ok_and(|owner| owner == *expected) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(unix)]
fn try_advisory_lock(file: &File) -> Result<(), AdvisoryLockError> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
    ) {
        Err(AdvisoryLockError::WouldBlock)
    } else {
        Err(AdvisoryLockError::Io(error))
    }
}

#[cfg(not(unix))]
fn try_advisory_lock(_file: &File) -> Result<(), AdvisoryLockError> {
    Ok(())
}

fn try_open_instance_lock(path: &Path) -> AppResult<LockAttempt> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| "single-instance lock could not be opened".to_string())?;
    match try_advisory_lock(&file) {
        Ok(()) => Ok(LockAttempt::Acquired(file)),
        Err(AdvisoryLockError::WouldBlock) => Ok(LockAttempt::WouldBlock),
        Err(AdvisoryLockError::Io(error)) => {
            Err(format!("single-instance advisory lock failed: {error}"))
        }
    }
}

fn stable_lock_file_must_be_empty(lock_file: &mut File) -> AppResult<()> {
    lock_file
        .seek(SeekFrom::Start(0))
        .map_err(|_| "single-instance lock could not be inspected".to_string())?;
    let mut content = Vec::new();
    lock_file
        .read_to_end(&mut content)
        .map_err(|_| "single-instance lock could not be inspected".to_string())?;
    if content.is_empty() {
        return Ok(());
    }

    Err("existing single-instance lock uses a legacy marker format; close the previous desktop instance before retrying".to_string())
}

fn read_focus_request(stream: &mut UnixStream) -> Result<String, FocusRequestError> {
    stream
        .set_read_timeout(Some(FOCUS_REQUEST_READ_TIMEOUT))
        .map_err(|_| FocusRequestError::IncompleteRead)?;

    let mut request = Vec::with_capacity(MAX_FOCUS_REQUEST_BYTES);
    let mut buffer = [0_u8; 256];
    loop {
        let remaining = MAX_FOCUS_REQUEST_BYTES.saturating_sub(request.len());
        let read_size = remaining.min(buffer.len()).max(1);
        match stream.read(&mut buffer[..read_size]) {
            Ok(0) => break,
            Ok(read) => {
                request.extend_from_slice(&buffer[..read]);
                if request.len() > MAX_FOCUS_REQUEST_BYTES {
                    return Err(FocusRequestError::RequestTooLarge);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(FocusRequestError::IncompleteRead),
        }
    }

    String::from_utf8(request).map_err(|_| FocusRequestError::InvalidUtf8)
}

fn focus_response(mut stream: UnixStream, timeout: Duration) -> FocusAttempt {
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if stream.write_all(b"focus\n").is_err() || stream.shutdown(Shutdown::Write).is_err() {
        return FocusAttempt::Unknown;
    }

    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return FocusAttempt::Unknown;
    }
    match response.trim() {
        "focused" => FocusAttempt::Focused,
        "not-ready" => FocusAttempt::NotReady,
        _ => FocusAttempt::Unknown,
    }
}

fn request_focus_once(socket_path: &Path, timeout: Duration) -> FocusAttempt {
    match UnixStream::connect(socket_path) {
        Ok(stream) => focus_response(stream, timeout),
        Err(_) => FocusAttempt::Unavailable,
    }
}

fn focus_socket_status(socket_path: &Path, timeout: Duration) -> FocusSocketStatus {
    match UnixStream::connect(socket_path) {
        Ok(stream) => match focus_response(stream, timeout) {
            FocusAttempt::Focused | FocusAttempt::NotReady => FocusSocketStatus::Active,
            FocusAttempt::Unavailable | FocusAttempt::Unknown => FocusSocketStatus::Unknown,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FocusSocketStatus::Missing,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            match fs::symlink_metadata(socket_path) {
                Ok(metadata) if metadata.file_type().is_socket() => FocusSocketStatus::Stale,
                Ok(_) => FocusSocketStatus::Unknown,
                Err(metadata_error) if metadata_error.kind() == std::io::ErrorKind::NotFound => {
                    FocusSocketStatus::Missing
                }
                Err(metadata_error)
                    if metadata_error.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    FocusSocketStatus::PermissionDenied
                }
                Err(metadata_error) => {
                    FocusSocketStatus::Unavailable(SanitizedIoError::from_error(
                        FocusSocketCheckStage::MetadataLookup,
                        &metadata_error,
                    ))
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            FocusSocketStatus::PermissionDenied
        }
        Err(error) => focus_socket_status_unavailable(FocusSocketCheckStage::SocketConnect, &error),
    }
}

fn focus_socket_status_unavailable(
    stage: FocusSocketCheckStage,
    error: &std::io::Error,
) -> FocusSocketStatus {
    FocusSocketStatus::Unavailable(SanitizedIoError::from_error(stage, error))
}

fn bind_focus_listener(socket_path: &Path) -> AppResult<UnixListener> {
    bind_focus_listener_with_status(
        socket_path,
        focus_socket_status(socket_path, FOCUS_RETRY_DELAY),
    )
}

fn bind_focus_listener_with_status(
    socket_path: &Path,
    status: FocusSocketStatus,
) -> AppResult<UnixListener> {
    match status {
        FocusSocketStatus::Missing => {}
        FocusSocketStatus::Stale => {
            fs::remove_file(socket_path).map_err(|_| {
                "stale single-instance focus socket could not be removed".to_string()
            })?;
        }
        FocusSocketStatus::Active => {
            return Err(
                "single-instance focus endpoint is active; refusing to replace it".to_string(),
            );
        }
        FocusSocketStatus::Unknown => {
            return Err(
                "single-instance focus endpoint uses an unknown protocol; refusing to replace it"
                    .to_string(),
            );
        }
        FocusSocketStatus::PermissionDenied => {
            return Err("single-instance focus endpoint permission was denied".to_string());
        }
        FocusSocketStatus::Unavailable(diagnostic) => {
            return Err(format!(
                "single-instance focus endpoint could not be checked ({})",
                diagnostic.diagnostic()
            ));
        }
    }

    UnixListener::bind(socket_path)
        .map_err(|_| "single-instance focus socket bind failed".to_string())
}

fn acquire_instance_at(
    paths: &InstancePaths,
    owner: InstanceOwner,
    retry_count: usize,
    retry_delay: Duration,
) -> AppResult<InstanceAcquire> {
    fs::create_dir_all(&paths.settings_directory)
        .map_err(|_| "desktop settings directory could not be prepared".to_string())?;
    prepare_focus_socket_directory(&paths.socket_path)?;

    for attempt in 0..=retry_count {
        match try_open_instance_lock(&paths.lock_path)? {
            LockAttempt::Acquired(mut lock_file) => {
                stable_lock_file_must_be_empty(&mut lock_file)?;
                atomic_write_instance_owner(&paths.owner_path, &owner)?;
                return Ok(InstanceAcquire::Primary(InstanceGuard {
                    owner_path: paths.owner_path.clone(),
                    socket_path: paths.socket_path.clone(),
                    owner,
                    socket_owned: false,
                    _lock_file: lock_file,
                }));
            }
            LockAttempt::WouldBlock => {
                if request_focus_once(&paths.socket_path, retry_delay) == FocusAttempt::Focused {
                    return Ok(InstanceAcquire::Focused);
                }
                if attempt == retry_count {
                    return Ok(InstanceAcquire::AlreadyRunningNotReady);
                }
                thread::sleep(retry_delay);
            }
        }
    }

    Err("single-application instance acquisition stopped unexpectedly".to_string())
}

pub(crate) fn acquire_instance() -> AppResult<InstanceAcquire> {
    let paths = instance_paths()?;
    acquire_instance_at(
        &paths,
        instance_owner(),
        FOCUS_RETRY_COUNT,
        FOCUS_RETRY_DELAY,
    )
}

fn handle_focus_connection<F>(mut stream: UnixStream, focus: F)
where
    F: FnOnce() -> bool,
{
    let focused = match read_focus_request(&mut stream) {
        Ok(request) if request.trim() == "focus" => focus(),
        Ok(_) | Err(_) => false,
    };
    let _ = stream.write_all(if focused {
        b"focused\n"
    } else {
        b"not-ready\n"
    });
}

pub(crate) fn start_focus_listener(socket_path: PathBuf, app: AppHandle) -> AppResult<()> {
    let listener = bind_focus_listener(&socket_path)?;
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(stream) = connection else {
                continue;
            };
            handle_focus_connection(stream, || {
                app.get_webview_window(PRIMARY_WINDOW_LABEL)
                    .is_some_and(|window| {
                        let _ = window.unminimize();
                        let _ = window.show();
                        window.set_focus().is_ok()
                    })
            });
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn request_focus(socket_path: &Path) -> bool {
        for attempt in 0..FOCUS_RETRY_COUNT {
            if request_focus_once(socket_path, FOCUS_RETRY_DELAY) == FocusAttempt::Focused {
                return true;
            }
            if attempt + 1 < FOCUS_RETRY_COUNT {
                thread::sleep(FOCUS_RETRY_DELAY);
            }
        }
        false
    }

    fn test_directory(_label: &str) -> PathBuf {
        let suffix = INSTANCE_OWNER_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = env::temp_dir().join(format!("cmn-d-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&directory).expect("test directory should be created");
        directory
    }

    fn test_instance_paths(label: &str) -> (PathBuf, InstancePaths) {
        let directory = test_directory(label);
        let mut paths = instance_paths_at(directory.clone());
        paths.socket_path = directory.join("focus").join(INSTANCE_SOCKET_FILE);
        prepare_focus_socket_directory(&paths.socket_path)
            .expect("test focus socket directory should be prepared");
        (directory, paths)
    }

    fn acquire_primary(paths: &InstancePaths, pid: u32) -> InstanceGuard {
        match acquire_instance_at(
            paths,
            instance_owner_with_pid(pid),
            0,
            Duration::from_millis(1),
        )
        .expect("test instance should be acquired")
        {
            InstanceAcquire::Primary(guard) => guard,
            InstanceAcquire::Focused => panic!("test instance unexpectedly focused another app"),
            InstanceAcquire::AlreadyRunningNotReady => {
                panic!("test instance was unexpectedly already running")
            }
        }
    }

    fn bind_test_socket(path: &Path) -> Option<UnixListener> {
        match UnixListener::bind(path) {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping Unix socket recovery test: {error}");
                None
            }
            Err(error) => panic!("Unix socket test setup failed: {error}"),
        }
    }

    #[test]
    fn debug_default_identity_is_separate_from_the_product_storage_namespace() {
        let home = PathBuf::from("/Users/cornell-test");
        let product_root = application_support_root(&home, APPLICATION_ID);
        let default_root = application_support_root(&home, application_id_for(false));

        assert_eq!(application_id_for(true), APPLICATION_ID);
        assert_eq!(
            product_root,
            home.join("Library/Application Support")
                .join(APPLICATION_ID)
        );

        #[cfg(debug_assertions)]
        {
            assert_eq!(application_id_for(false), DEBUG_APPLICATION_ID);
            assert_ne!(default_root, product_root);
            assert!(default_root.ends_with(DEBUG_APPLICATION_ID));
        }

        #[cfg(not(debug_assertions))]
        {
            assert_eq!(application_id_for(false), APPLICATION_ID);
            assert_eq!(default_root, product_root);
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_focus_socket_identity_is_separate_from_the_product_identity() {
        let settings = PathBuf::from("/Users/cornell-test/Library/Application Support/settings");
        let temp_directory = Path::new("/tmp");

        let debug_socket =
            focus_socket_path_at_for(temp_directory, &settings, DEBUG_APPLICATION_ID);
        let product_socket = focus_socket_path_at_for(temp_directory, &settings, APPLICATION_ID);

        assert_ne!(debug_socket, product_socket);
    }

    #[test]
    fn focus_socket_path_is_stable_identity_scoped_and_bounded() {
        let first_settings = PathBuf::from("/Users/first/Library/Application Support")
            .join(APPLICATION_ID)
            .join("settings");
        let second_settings = PathBuf::from("/Users/second/Library/Application Support")
            .join(APPLICATION_ID)
            .join("settings");
        let temp_directory = Path::new("/tmp");

        let first_path = focus_socket_path_at(temp_directory, &first_settings);
        let first_path_again = focus_socket_path_at(temp_directory, &first_settings);
        let second_path = focus_socket_path_at(temp_directory, &second_settings);
        let expected_first_path = temp_directory
            .join(format!(
                "{INSTANCE_SOCKET_DIRECTORY_PREFIX}{}",
                focus_socket_identity_for(desktop_application_id(), &first_settings)
            ))
            .join(INSTANCE_SOCKET_FILE);

        assert_eq!(first_path, first_path_again);
        assert_eq!(first_path, expected_first_path);
        assert_ne!(first_path, second_path);
        assert_eq!(
            first_path
                .parent()
                .and_then(Path::file_name)
                .map(|name| name.len()),
            Some(INSTANCE_SOCKET_DIRECTORY_PREFIX.len() + INSTANCE_SOCKET_HASH_HEX_LENGTH)
        );
        assert!(focus_socket_path_is_bounded(&first_path));
        assert!(!first_path
            .as_os_str()
            .as_bytes()
            .windows(first_settings.as_os_str().as_bytes().len())
            .any(|window| window == first_settings.as_os_str().as_bytes()));
    }

    #[test]
    fn long_temp_and_storage_paths_use_a_deterministic_bounded_fallback() {
        let directory = test_directory("bounded-socket");
        let long_component = "x".repeat(120);
        let long_temp_directory = PathBuf::from("/tmp").join(&long_component);
        let settings_directory = PathBuf::from("/")
            .join(&long_component)
            .join(&long_component)
            .join(directory.file_name().expect("test directory has a name"))
            .join("settings");
        let socket_path =
            focus_socket_path_at_for(&long_temp_directory, &settings_directory, APPLICATION_ID);
        let socket_path_again =
            focus_socket_path_at_for(&long_temp_directory, &settings_directory, APPLICATION_ID);
        let expected_socket_path = fallback_focus_socket_root()
            .join(format!(
                "{INSTANCE_SOCKET_DIRECTORY_PREFIX}{}",
                focus_socket_identity_for(APPLICATION_ID, &settings_directory)
            ))
            .join(INSTANCE_SOCKET_FILE);

        assert_eq!(socket_path, socket_path_again);
        assert_eq!(socket_path, expected_socket_path);
        assert!(focus_socket_path_is_bounded(&socket_path));
        assert!(!socket_path.starts_with(&long_temp_directory));
        assert!(!socket_path.starts_with(&settings_directory));
        prepare_focus_socket_directory(&socket_path).unwrap();
        assert_eq!(
            fs::metadata(fallback_focus_socket_root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(socket_path.parent().expect("socket has a parent"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let listener = bind_test_socket(&socket_path);
        if let Some(listener) = listener {
            drop(listener);
        }
        let _ = fs::remove_file(&socket_path);
        fs::remove_dir(socket_path.parent().expect("socket has a parent")).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn focus_request_uses_a_unix_socket_without_creating_another_window() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let socket_path =
            env::temp_dir().join(format!("cmn-f-{}-{suffix}.sock", std::process::id()));
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping Unix socket test: {error}");
                return;
            }
            Err(error) => panic!("Unix socket test setup failed: {error}"),
        };
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            assert_eq!(request.trim(), "focus");
            stream.write_all(b"focused\n").unwrap();
        });
        assert!(request_focus(&socket_path));
        thread.join().unwrap();
        fs::remove_file(socket_path).unwrap();
    }

    #[test]
    fn malformed_focus_requests_use_fixed_errors() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client
            .write_all(&vec![b'x'; MAX_FOCUS_REQUEST_BYTES + 1])
            .unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        assert_eq!(
            read_focus_request(&mut server),
            Err(FocusRequestError::RequestTooLarge)
        );

        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(&[0xff]).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        assert_eq!(
            read_focus_request(&mut server),
            Err(FocusRequestError::InvalidUtf8)
        );
    }

    #[test]
    fn focus_request_without_eof_is_rejected_after_the_read_timeout() {
        let (_client, mut server) = UnixStream::pair().unwrap();
        let started = Instant::now();
        assert_eq!(
            read_focus_request(&mut server),
            Err(FocusRequestError::IncompleteRead)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn hanging_focus_client_does_not_block_the_next_focus_request() {
        let (directory, paths) = test_instance_paths("bounded-focus-request");
        let Some(listener) = bind_test_socket(&paths.socket_path) else {
            fs::remove_dir_all(directory).unwrap();
            return;
        };
        let socket_path = paths.socket_path.clone();
        let listener_thread = thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                handle_focus_connection(stream, || true);
            }
        });

        let mut hanging_client = UnixStream::connect(&socket_path).unwrap();
        hanging_client
            .set_read_timeout(Some(FOCUS_REQUEST_READ_TIMEOUT + Duration::from_secs(1)))
            .unwrap();
        let started = Instant::now();
        let mut response = String::new();
        hanging_client.read_to_string(&mut response).unwrap();
        assert_eq!(response, "not-ready\n");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(hanging_client);

        let mut normal_client = UnixStream::connect(&socket_path).unwrap();
        normal_client.write_all(b"focus\n").unwrap();
        normal_client.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        normal_client.read_to_string(&mut response).unwrap();
        assert_eq!(response, "focused\n");

        listener_thread.join().unwrap();
        fs::remove_file(socket_path).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stable_lock_is_the_authority_and_guard_cleanup_keeps_the_lock_file() {
        let (directory, paths) = test_instance_paths("stable-lock");
        let mut guard = acquire_primary(&paths, 101);
        if let Some(stale_listener) = bind_test_socket(&paths.socket_path) {
            drop(stale_listener);
        } else {
            fs::write(&paths.socket_path, b"owned socket placeholder").unwrap();
        }
        assert!(paths.socket_path.exists());
        guard.mark_socket_owned();
        drop(guard);

        assert!(paths.lock_path.exists());
        assert!(!paths.owner_path.exists());
        assert!(!paths.socket_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_marker_in_stable_lock_fails_safe_without_unlinking_it() {
        let (directory, paths) = test_instance_paths("legacy-lock");
        let legacy = br#"{"schemaVersion":1,"pid":7,"applicationId":"com.cornellmethod.notebook"}"#;
        fs::write(&paths.lock_path, legacy).unwrap();

        let result = acquire_instance_at(
            &paths,
            instance_owner_with_pid(202),
            0,
            Duration::from_millis(1),
        );
        let error = result.expect_err("legacy stable marker must fail safe");
        assert!(error.contains("legacy marker format"));
        assert_eq!(fs::read(&paths.lock_path).unwrap(), legacy);
        assert!(!paths.owner_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_owner_marker_is_atomically_replaced_after_lock_acquisition() {
        let (directory, paths) = test_instance_paths("atomic-owner");
        fs::write(&paths.owner_path, b"{\"schemaVersion\":").unwrap();
        let guard = acquire_primary(&paths, 303);

        let owner = read_instance_owner(&paths.owner_path).unwrap();
        assert_eq!(owner, instance_owner_with_pid(303));
        let temporary_markers = fs::read_dir(&paths.settings_directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".instance.owner.tmp-")
            })
            .count();
        assert_eq!(temporary_markers, 0);
        drop(guard);
        assert!(paths.lock_path.exists());
        assert!(!paths.owner_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn empty_owner_marker_is_replaced_without_touching_the_stable_lock() {
        let (directory, paths) = test_instance_paths("empty-owner");
        fs::write(&paths.owner_path, b"").unwrap();

        let guard = acquire_primary(&paths, 350);
        assert_eq!(read_instance_owner(&paths.owner_path).unwrap().pid, 350);
        drop(guard);
        assert!(paths.lock_path.exists());
        assert!(!paths.owner_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn schema_application_and_pid_changes_are_metadata_only_when_lock_is_free() {
        let (directory, paths) = test_instance_paths("owner-mismatch");
        let old_owner = InstanceOwner {
            schema_version: INSTANCE_SCHEMA_VERSION + 1,
            pid: 404,
            application_id: "foreign.application".to_string(),
        };
        atomic_write_instance_owner(&paths.owner_path, &old_owner).unwrap();

        let guard = acquire_primary(&paths, 505);
        assert_eq!(read_instance_owner(&paths.owner_path).unwrap().pid, 505);
        drop(guard);
        assert!(paths.lock_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn lock_held_secondary_does_not_remove_owner_or_socket_metadata() {
        let (directory, paths) = test_instance_paths("held-metadata");
        let guard = acquire_primary(&paths, 606);
        let owner_before = fs::read(&paths.owner_path).unwrap();
        fs::write(&paths.socket_path, b"socket metadata").unwrap();
        let socket_before = fs::read(&paths.socket_path).unwrap();

        let secondary = acquire_instance_at(
            &paths,
            instance_owner_with_pid(707),
            0,
            Duration::from_millis(1),
        )
        .unwrap();
        assert!(matches!(secondary, InstanceAcquire::AlreadyRunningNotReady));
        assert_eq!(fs::read(&paths.owner_path).unwrap(), owner_before);
        assert_eq!(fs::read(&paths.socket_path).unwrap(), socket_before);

        drop(guard);
        fs::remove_file(&paths.socket_path).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn secondary_focus_success_does_not_create_a_primary_guard() {
        let (directory, paths) = test_instance_paths("focused-secondary");
        let guard = acquire_primary(&paths, 808);
        let Some(listener) = bind_test_socket(&paths.socket_path) else {
            drop(guard);
            fs::remove_dir_all(directory).unwrap();
            return;
        };
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            assert_eq!(request.trim(), "focus");
            stream.write_all(b"focused\n").unwrap();
        });

        let secondary = acquire_instance_at(
            &paths,
            instance_owner_with_pid(909),
            1,
            Duration::from_millis(5),
        )
        .unwrap();
        assert!(matches!(secondary, InstanceAcquire::Focused));
        thread.join().unwrap();
        drop(guard);
        fs::remove_file(&paths.socket_path).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_socket_is_reused_only_after_an_unavailable_endpoint_check() {
        let (directory, paths) = test_instance_paths("stale-socket");
        let guard = acquire_primary(&paths, 1001);
        let Some(stale_listener) = bind_test_socket(&paths.socket_path) else {
            drop(guard);
            fs::remove_dir_all(directory).unwrap();
            return;
        };
        drop(stale_listener);
        assert!(paths.socket_path.exists());

        let rebound = bind_focus_listener(&paths.socket_path).unwrap();
        assert!(paths.socket_path.exists());
        drop(rebound);
        drop(guard);
        let _ = fs::remove_file(&paths.socket_path);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn active_unknown_and_non_socket_endpoints_are_preserved() {
        let (directory, paths) = test_instance_paths("endpoint-safety");

        fs::write(&paths.socket_path, b"not a unix socket").unwrap();
        assert!(bind_focus_listener(&paths.socket_path).is_err());
        assert!(paths.socket_path.exists());
        fs::remove_file(&paths.socket_path).unwrap();

        let Some(listener) = bind_test_socket(&paths.socket_path) else {
            fs::remove_dir_all(directory).unwrap();
            return;
        };
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            stream.write_all(b"not-ready\n").unwrap();
        });
        assert!(bind_focus_listener(&paths.socket_path).is_err());
        assert!(paths.socket_path.exists());
        thread.join().unwrap();
        fs::remove_file(&paths.socket_path).unwrap();

        let Some(listener) = bind_test_socket(&paths.socket_path) else {
            let _ = fs::remove_file(&paths.socket_path);
            fs::remove_dir_all(directory).unwrap();
            return;
        };
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            stream.write_all(b"unknown-protocol\n").unwrap();
        });
        assert!(bind_focus_listener(&paths.socket_path).is_err());
        assert!(paths.socket_path.exists());
        thread.join().unwrap();
        fs::remove_file(&paths.socket_path).unwrap();

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn permission_denied_endpoint_is_preserved_without_reuse() {
        let (directory, paths) = test_instance_paths("permission-endpoint");
        fs::write(&paths.socket_path, b"permission-denied endpoint").unwrap();

        let error = bind_focus_listener_with_status(
            &paths.socket_path,
            FocusSocketStatus::PermissionDenied,
        )
        .expect_err("permission-denied endpoint must not be rebound");
        assert_eq!(
            error,
            "single-instance focus endpoint permission was denied"
        );
        assert!(paths.socket_path.exists());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unavailable_endpoint_diagnostic_is_structured_and_path_free() {
        let sensitive_path = "/Users/example/Library/Application Support/Cornell/secret.sock";
        let connect_error = std::io::Error::from_raw_os_error(libc::ENOTSOCK);
        let connect_status =
            focus_socket_status_unavailable(FocusSocketCheckStage::SocketConnect, &connect_error);
        let FocusSocketStatus::Unavailable(connect_diagnostic) = connect_status else {
            panic!("expected unavailable connect status");
        };
        let connect_message = bind_focus_listener_with_status(
            Path::new(sensitive_path),
            FocusSocketStatus::Unavailable(connect_diagnostic),
        )
        .expect_err("unavailable endpoint must fail closed");
        assert!(connect_message.contains("stage=socket-connect"));
        assert!(connect_message.contains(&format!("kind={:?}", connect_error.kind())));
        assert!(connect_message.contains(&format!("raw_os_error={}", libc::ENOTSOCK)));
        assert!(!connect_message.contains("/Users/"));
        assert!(!connect_message.contains("secret.sock"));

        let metadata_error = std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("metadata failed for {sensitive_path}"),
        );
        let FocusSocketStatus::Unavailable(metadata_diagnostic) =
            focus_socket_status_unavailable(FocusSocketCheckStage::MetadataLookup, &metadata_error)
        else {
            panic!("expected unavailable metadata status");
        };
        let metadata_message = bind_focus_listener_with_status(
            Path::new(sensitive_path),
            FocusSocketStatus::Unavailable(metadata_diagnostic),
        )
        .expect_err("unavailable metadata must fail closed");
        assert!(metadata_message.contains("stage=metadata-lookup"));
        assert!(metadata_message.contains("kind=Other"));
        assert!(metadata_message.contains("raw_os_error=unset"));
        assert!(!metadata_message.contains("/Users/"));
        assert!(!metadata_message.contains("secret.sock"));
    }

    #[test]
    fn concurrent_acquire_has_one_primary_and_one_existing_owner_result() {
        let (directory, paths) = test_instance_paths("concurrent-acquire");
        let paths = std::sync::Arc::new(paths);
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|index| {
                let paths = paths.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    acquire_instance_at(
                        &paths,
                        instance_owner_with_pid(1100 + index),
                        0,
                        Duration::from_millis(1),
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, InstanceAcquire::Primary(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, InstanceAcquire::AlreadyRunningNotReady))
                .count(),
            1
        );
        drop(results);
        assert!(paths.lock_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn secondary_retries_acquire_after_the_advisory_lock_is_released() {
        let (directory, paths) = test_instance_paths("retry-after-release");
        let guard = acquire_primary(&paths, 1201);
        let paths_for_thread = Arc::new(paths.clone());
        let thread_paths = paths_for_thread.clone();
        let handle = thread::spawn(move || {
            acquire_instance_at(
                &thread_paths,
                instance_owner_with_pid(1202),
                5,
                Duration::from_millis(10),
            )
            .unwrap()
        });
        thread::sleep(Duration::from_millis(20));
        drop(guard);

        let result = handle.join().unwrap();
        let secondary_guard = match result {
            InstanceAcquire::Primary(guard) => guard,
            InstanceAcquire::Focused => panic!("secondary unexpectedly focused a missing socket"),
            InstanceAcquire::AlreadyRunningNotReady => {
                panic!("secondary did not retry after the advisory lock was released")
            }
        };
        drop(secondary_guard);
        assert!(paths_for_thread.lock_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}

//! PNG-only, one-use destinations. SQLite/diagnostic selections are deliberately separate.
mod validation;

use self::validation::validate_png;
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) const FRAGMENT: &str = "cornell-desktop-png=";
pub(crate) const EVENT: &str = "cornell:desktop-png-result";
const MAX_PNG: usize = 64 * 1024 * 1024;
const MAX_CHUNK: usize = 32 * 1024;
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Request {
    Select {
        id: String,
        suggestion: String,
    },
    Append {
        id: String,
        token: String,
        offset: usize,
        data: String,
    },
    Publish {
        id: String,
        token: String,
    },
    Discard {
        id: String,
        token: String,
    },
}

impl Request {
    fn id(&self) -> &str {
        match self {
            Self::Select { id, .. }
            | Self::Append { id, .. }
            | Self::Publish { id, .. }
            | Self::Discard { id, .. } => id,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Response {
    id: String,
    status: &'static str,
    token: Option<String>,
    code: Option<&'static str>,
}

#[derive(Default)]
pub(crate) struct PngExportStore(Mutex<Option<Selection>>);

struct Selection {
    token: String,
    page: String,
    created: Instant,
    destination: Destination,
    bytes: Vec<u8>,
}

fn random_id() -> Result<String> {
    let mut bytes = [0; 24];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "unknown")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn io_code(error: io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(libc::EACCES | libc::EPERM | libc::EROFS) => "permission",
        Some(libc::ENOSPC | libc::EDQUOT) => "space",
        Some(libc::ENOENT | libc::ENODEV | libc::ENXIO | libc::ESTALE) => "unavailable",
        Some(libc::EEXIST | libc::ELOOP) => "changed",
        _ => "unknown",
    }
}

fn cstring(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| "invalid-name")
}

fn filename(value: &str) -> Result<String> {
    if value.trim().is_empty()
        || value.starts_with('.') && value.trim_matches('.').is_empty()
        || value.ends_with(['.', ' '])
        || value
            .chars()
            .any(|c| c.is_ascii_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err("invalid-name");
    }
    match value.rsplit_once('.') {
        Some((stem, extension))
            if !stem.trim().is_empty() && extension.eq_ignore_ascii_case("png") =>
        {
            Ok(value.to_string())
        }
        None => Ok(format!("{value}.png")),
        _ => Err("invalid-name"),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.size(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

struct Destination {
    original_parent: PathBuf,
    canonical_parent: PathBuf,
    directory: File,
    parent_identity: (u64, u64),
    name: CString,
    target: Option<Identity>,
    approved: bool,
}

impl Destination {
    fn capture(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err("invalid-name");
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid-name")?;
        if filename(name)? != name {
            return Err("invalid-name");
        }
        let original_parent = path.parent().ok_or("unavailable")?.to_path_buf();
        let canonical_parent = original_parent.canonicalize().map_err(io_code)?;
        let directory = File::open(&canonical_parent).map_err(io_code)?;
        let metadata = directory.metadata().map_err(io_code)?;
        if !metadata.is_dir() {
            return Err("unavailable");
        }
        let limit = unsafe { libc::fpathconf(directory.as_raw_fd(), libc::_PC_NAME_MAX) };
        if name.len() > if limit > 0 { limit as usize } else { 255 } {
            return Err("invalid-name");
        }
        let mut destination = Self {
            original_parent,
            canonical_parent,
            directory,
            parent_identity: (metadata.dev(), metadata.ino()),
            name: cstring(name)?,
            target: None,
            approved: false,
        };
        destination.target = destination.target_identity()?;
        Ok(destination)
    }

    fn target_identity(&self) -> Result<Option<Identity>> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(io_code(error))
            };
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err("changed");
        }
        // Open relative to the pinned directory without following a symlink.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(io_code(io::Error::last_os_error()));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(io_code)?;
        if !metadata.is_file() {
            return Err("changed");
        }
        Ok(Some(Identity::of(&metadata)))
    }

    fn validate(&self) -> Result<()> {
        let canonical = self.original_parent.canonicalize().map_err(io_code)?;
        let metadata = fs::metadata(&canonical).map_err(io_code)?;
        if canonical != self.canonical_parent
            || (metadata.dev(), metadata.ino()) != self.parent_identity
            || self.target_identity()? != self.target
            || self.target.is_some() && !self.approved
        {
            return Err("changed");
        }
        Ok(())
    }
}

struct Stage<'a> {
    directory: &'a File,
    name: CString,
    cleanup: bool,
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if self.cleanup
            && unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0) } != 0
        {
            eprintln!("PNG stage cleanup warning");
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Write,
    Sync,
    BeforePublish,
    Publish,
}

fn publish(destination: Destination, bytes: &[u8], fault: Fault) -> Result<()> {
    validate_png(bytes)?;
    let name = cstring(&format!(".cuetrio-png-{}.tmp", random_id()?))?;
    let fd = unsafe {
        libc::openat(
            destination.directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io_code(io::Error::last_os_error()));
    }
    // Only clean up a staging entry that this operation successfully created.
    let mut stage = Stage {
        directory: &destination.directory,
        name,
        cleanup: true,
    };
    let mut file = unsafe { File::from_raw_fd(fd) };
    if fault == Fault::Write {
        file.write_all(&bytes[..bytes.len() / 2]).map_err(io_code)?;
        return Err("space");
    }
    file.write_all(bytes).map_err(io_code)?;
    if fault == Fault::Sync {
        return Err("unknown");
    }
    file.sync_all().map_err(io_code)?;
    if fault == Fault::BeforePublish {
        return Err("unknown");
    }
    destination.validate()?;
    if fault == Fault::Publish {
        return Err("permission");
    }
    let dir = destination.directory.as_raw_fd();
    // Ordinary validated atomic replacement, NOT a compare-and-swap against
    // every noncooperating external writer. New files always use no-replace.
    let status = unsafe {
        if destination.target.is_some() {
            libc::renameat(dir, stage.name.as_ptr(), dir, destination.name.as_ptr())
        } else {
            libc::linkat(dir, stage.name.as_ptr(), dir, destination.name.as_ptr(), 0)
        }
    };
    if status != 0 {
        return Err(io_code(io::Error::last_os_error()));
    }
    if destination.target.is_some() {
        // rename consumed the staging name; never roll back after publication.
        stage.cleanup = false;
    }
    drop(stage);
    if destination.directory.sync_all().is_err() {
        eprintln!("PNG published; directory flush warning");
    }
    Ok(())
}

// Static scripts and argv keep titles/names out of executable AppleScript.
#[cfg(target_os = "macos")]
fn native_script(script: &str, argument: &str) -> Result<String> {
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", script, "--", argument])
        .output()
        .map_err(io_code)?;
    if !output.status.success() || output.stdout.len() > 16 * 1024 {
        return Err("unknown");
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim_end_matches(['\r', '\n']).to_string())
        .map_err(|_| "unknown")
}

#[cfg(target_os = "macos")]
fn choose(suggestion: &str) -> Result<Option<Destination>> {
    // Standard Additions owns the native dialog lifecycle in the osascript host.
    // A bare NSSavePanel can return cancellation before displaying a usable UI.
    const PANEL: &str = r#"function run(argv) {
  var app = Application.currentApplication();
  app.includeStandardAdditions = true;
  try {
    var path = app.chooseFileName({withPrompt: 'PNGを保存', defaultName: argv[0]}).toString();
    return JSON.stringify({path: path, name: path.split('/').pop()});
  } catch (error) {
    if (error.errorNumber === -128) return 'cancel';
    throw error;
  }
}"#;
    const INVALID: &str = r#"function run(argv) {
  var app = Application.currentApplication();
  app.includeStandardAdditions = true;
  app.displayDialog('有効な名前とPNG拡張子を指定してください。長い名前は短くしてください。',
    {withTitle: 'ファイル名を確認してください', buttons: ['戻る'], defaultButton: '戻る'});
  return 'ok';
}"#;
    const CONFIRM: &str = r#"function run(argv) {
  var app = Application.currentApplication();
  app.includeStandardAdditions = true;
  try {
    var response = app.displayDialog(argv[0], {withTitle: '既存のPNGを置き換えますか？',
      buttons: ['キャンセル', '置き換える'], defaultButton: 'キャンセル', cancelButton: 'キャンセル'});
    return response.buttonReturned === '置き換える' ? 'approved' : 'cancel';
  } catch (error) {
    if (error.errorNumber === -128) return 'cancel';
    throw error;
  }
}"#;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selected {
        path: String,
        name: String,
    }
    let mut suggestion = suggestion.to_string();
    loop {
        let output = native_script(PANEL, &suggestion)?;
        if output == "cancel" {
            return Ok(None);
        }
        let selected: Selected = serde_json::from_str(&output).map_err(|_| "unknown")?;
        let path = PathBuf::from(&selected.path);
        let validated = filename(&selected.name).and_then(|name| {
            if path.file_name().and_then(|value| value.to_str()) != Some(name.as_str()) {
                return Err("invalid-name");
            }
            Destination::capture(&path)
        });
        let mut destination = match validated {
            Ok(destination) => destination,
            Err("invalid-name") => {
                native_script(INVALID, "")?;
                suggestion = selected.name;
                continue;
            }
            Err(error) => return Err(error),
        };
        if destination.target.is_some() {
            // Capture identity BEFORE this explicit native approval. NSSavePanel
            // may also show its own system replacement prompt.
            if native_script(CONFIRM, &selected.name)? != "approved" {
                return Ok(None);
            }
            destination.approved = true;
        }
        destination.validate()?;
        return Ok(Some(destination));
    }
}

#[cfg(not(target_os = "macos"))]
fn choose(_suggestion: &str) -> Result<Option<Destination>> {
    Err("unknown")
}

pub(crate) fn parse(encoded: &str) -> Result<Request> {
    if encoded.len() > 64 * 1024 {
        return Err("unknown");
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| "unknown")?;
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| "unknown")?;
    if request.id().is_empty()
        || request.id().len() > 64
        || !request
            .id()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("unknown");
    }
    Ok(request)
}

impl PngExportStore {
    pub(crate) fn run(&self, request: Request, page: &str) -> Response {
        let id = request.id().to_string();
        let result = (|| {
            let mut state = self.0.try_lock().map_err(|_| "unknown")?;
            if state
                .as_ref()
                .is_some_and(|selection| selection.created.elapsed() > Duration::from_secs(600))
            {
                *state = None;
            }
            let is_publish = matches!(&request, Request::Publish { .. });
            match request {
                Request::Select { suggestion, .. } => {
                    if suggestion.len() > 240 || filename(&suggestion)? != suggestion {
                        return Err("unknown");
                    }
                    // A new dialog invalidates abandoned selections, never reuses approval.
                    *state = None;
                    let Some(destination) = choose(&suggestion)? else {
                        return Ok(("cancelled", None));
                    };
                    let token = random_id()?;
                    *state = Some(Selection {
                        token: token.clone(),
                        page: page.to_string(),
                        created: Instant::now(),
                        destination,
                        bytes: Vec::new(),
                    });
                    Ok(("selected", Some(token)))
                }
                Request::Append {
                    token,
                    offset,
                    data,
                    ..
                } => {
                    let selected = state
                        .as_mut()
                        .filter(|selected| selected.token == token && selected.page == page)
                        .ok_or("unknown")?;
                    let bytes = STANDARD.decode(data).map_err(|_| "unknown")?;
                    if bytes.is_empty()
                        || bytes.len() > MAX_CHUNK
                        || offset != selected.bytes.len()
                        || offset + bytes.len() > MAX_PNG
                    {
                        *state = None;
                        return Err("unknown");
                    }
                    selected.bytes.extend_from_slice(&bytes);
                    Ok(("appended", None))
                }
                Request::Publish { token, .. } | Request::Discard { token, .. } => {
                    if !state
                        .as_ref()
                        .is_some_and(|selected| selected.token == token && selected.page == page)
                    {
                        return Err("unknown");
                    }
                    let selected = state.take().ok_or("unknown")?;
                    if is_publish {
                        publish(selected.destination, &selected.bytes, Fault::None)?;
                        Ok(("published", None))
                    } else {
                        Ok(("discarded", None))
                    }
                }
            }
        })();
        match result {
            Ok((status, token)) => Response {
                id,
                status,
                token,
                code: None,
            },
            Err(code) => Response {
                id,
                status: "error",
                token: None,
                code: Some(code),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validation::crc32;
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("cuetrio-png-test-{}", random_id().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn png() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            bytes.extend_from_slice(&body);
            bytes.extend_from_slice(&crc32(&body).to_be_bytes());
        };
        chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        chunk(b"sRGB", &[0]);
        // Real zlib-compressed RGBA scanline (one opaque red pixel).
        use flate2::{write::ZlibEncoder, Compression};
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[0, 255, 0, 0, 255]).unwrap();
        chunk(b"IDAT", &encoder.finish().unwrap());
        chunk(b"IEND", &[]);
        bytes
    }

    #[test]
    fn png_export_preserves_old_bytes_on_generation_write_flush_and_publication_failure() {
        let temp = Temp::new();
        let path = temp.0.join("existing.png");
        for fault in [
            Fault::None,
            Fault::Write,
            Fault::Sync,
            Fault::BeforePublish,
            Fault::Publish,
        ] {
            fs::write(&path, b"OLD TARGET").unwrap();
            let mut destination = Destination::capture(&path).unwrap();
            destination.approved = true;
            let bytes = if fault == Fault::None {
                b"invalid PNG".to_vec()
            } else {
                png()
            };
            assert!(publish(destination, &bytes, fault).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"OLD TARGET");
            assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn png_export_new_publication_is_complete_and_failures_leave_no_final_file() {
        let temp = Temp::new();
        let path = temp.0.join("new.png");
        for fault in [
            Fault::Write,
            Fault::Sync,
            Fault::BeforePublish,
            Fault::Publish,
        ] {
            assert!(publish(Destination::capture(&path).unwrap(), &png(), fault).is_err());
            assert!(!path.exists());
            assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 0);
        }
        publish(Destination::capture(&path).unwrap(), &png(), Fault::None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), png());
    }

    #[test]
    fn png_export_rejects_unapproved_changed_targets_new_collisions_and_parent_changes() {
        let temp = Temp::new();
        let path = temp.0.join("target.png");
        fs::write(&path, b"OLD").unwrap();
        assert!(publish(Destination::capture(&path).unwrap(), &png(), Fault::None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"OLD");
        let mut selected = Destination::capture(&path).unwrap();
        selected.approved = true;
        fs::write(&path, b"CHANGED").unwrap();
        assert!(publish(selected, &png(), Fault::None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"CHANGED");
        fs::remove_file(&path).unwrap();
        let selected = Destination::capture(&path).unwrap();
        fs::write(&path, b"COLLISION").unwrap();
        assert!(publish(selected, &png(), Fault::None).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"COLLISION");
        let parent = temp.0.join("folder");
        fs::create_dir(&parent).unwrap();
        let selected = Destination::capture(&parent.join("new.png")).unwrap();
        fs::rename(&parent, temp.0.join("moved")).unwrap();
        fs::create_dir(&parent).unwrap();
        assert!(publish(selected, &png(), Fault::None).is_err());
        assert!(!parent.join("new.png").exists());
    }

    #[test]
    fn png_export_approved_replacement_and_cancellation_preserve_contract() {
        let temp = Temp::new();
        let path = temp.0.join("target.png");
        fs::write(&path, b"OLD").unwrap();
        drop(Destination::capture(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"OLD");
        let mut selected = Destination::capture(&path).unwrap();
        selected.approved = true;
        publish(selected, &png(), Fault::None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), png());
    }

    #[test]
    fn png_export_validates_names_requests_and_one_use_page_bound_tokens() {
        assert_eq!(filename("学習").unwrap(), "学習.png");
        assert_eq!(filename("学習.PNG").unwrap(), "学習.PNG");
        for name in [
            "", " ", ".", ".png", "x.jpg", "a/b.png", "a\\b.png", "a:b.png", "a\n.png", "a.png.",
        ] {
            assert!(filename(name).is_err(), "{name}");
        }
        for json in [
            r#"{"operation":"select","id":"a","suggestion":"x.png","path":"/tmp/a.png"}"#,
            r#"{"operation":"publish","id":"a","token":"x","confirmed":true}"#,
        ] {
            assert!(parse(&STANDARD.encode(json)).is_err());
        }
        let temp = Temp::new();
        let path = temp.0.join("target.png");
        let store = PngExportStore::default();
        *store.0.lock().unwrap() = Some(Selection {
            token: "opaque".into(),
            page: "page".into(),
            created: Instant::now(),
            destination: Destination::capture(&path).unwrap(),
            bytes: png(),
        });
        for (id, page, expected) in [
            ("a", "other", "error"),
            ("b", "page", "published"),
            ("c", "page", "error"),
        ] {
            let response = store.run(
                Request::Publish {
                    id: id.into(),
                    token: "opaque".into(),
                },
                page,
            );
            assert_eq!(response.status, expected);
        }
        assert_eq!(fs::read(&path).unwrap(), png());
    }
}

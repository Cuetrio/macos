use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use plist::Value;

use crate::update_archive::ExtractedArchive;
use crate::update_manifest::{MANIFEST_PRODUCT_ID, TARGET_ARCHITECTURE};

const APP_BUNDLE_NAME: &str = "Cuetrio.app";
const CONTENTS_DIRECTORY_NAME: &str = "Contents";
const INFO_PLIST_NAME: &str = "Info.plist";
const MACOS_DIRECTORY_NAME: &str = "MacOS";
const MAX_INFO_PLIST_BYTES: u64 = 16 * 1024 * 1024;

const MACH_HEADER_64_BYTES: u64 = 32;
const LOAD_COMMAND_HEADER_BYTES: u64 = 8;
const MAX_LOAD_COMMANDS: u32 = 1_000_000;

const MH_MAGIC_64: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
const MH_CIGAM_64: [u8; 4] = [0xfe, 0xed, 0xfa, 0xcf];
const MH_MAGIC_32: [u8; 4] = [0xce, 0xfa, 0xed, 0xfe];
const MH_CIGAM_32: [u8; 4] = [0xfe, 0xed, 0xfa, 0xce];
const FAT_MAGIC: [u8; 4] = [0xca, 0xfe, 0xba, 0xbe];
const FAT_CIGAM: [u8; 4] = [0xbe, 0xba, 0xfe, 0xca];
const FAT_MAGIC_64: [u8; 4] = [0xca, 0xfe, 0xba, 0xbf];
const FAT_CIGAM_64: [u8; 4] = [0xbf, 0xba, 0xfe, 0xca];

const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MH_EXECUTE: u32 = 0x0000_0002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BundleValidationError {
    BundleLayout,
    BundlePlist,
    BundleIdentity,
    BundleVersion,
    BundleExecutable,
    BundleArchitecture,
    StagingPath,
    StagingRead,
}

impl BundleValidationError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::BundleLayout => "bundle-layout",
            Self::BundlePlist => "bundle-plist",
            Self::BundleIdentity => "bundle-identity",
            Self::BundleVersion => "bundle-version",
            Self::BundleExecutable => "bundle-executable",
            Self::BundleArchitecture => "bundle-architecture",
            Self::StagingPath => "staging-path",
            Self::StagingRead => "staging-read",
        }
    }
}

impl std::fmt::Display for BundleValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for BundleValidationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedAppBundle {
    pub(crate) relative_app_path: PathBuf,
    pub(crate) bundle_identifier: String,
    pub(crate) version: String,
    pub(crate) executable_filename: String,
    pub(crate) architecture: &'static str,
}

struct BundleMetadata {
    bundle_identifier: String,
    executable_filename: String,
}

pub(crate) fn validate_extracted_app_bundle(
    extracted_archive: &ExtractedArchive,
    staging_root: &Path,
) -> Result<VerifiedAppBundle, BundleValidationError> {
    validate_staging_root(staging_root)?;
    let (relative_app_path, app_path) =
        resolve_app_path(staging_root, &extracted_archive.relative_app_path)?;

    let contents_path = app_path.join(CONTENTS_DIRECTORY_NAME);
    require_directory(&contents_path, BundleValidationError::BundleLayout)?;

    let info_plist_path = contents_path.join(INFO_PLIST_NAME);
    let macos_path = contents_path.join(MACOS_DIRECTORY_NAME);
    let bundle_metadata = read_bundle_metadata(&info_plist_path, &extracted_archive.version)?;
    require_directory(&macos_path, BundleValidationError::BundleLayout)?;

    let executable_path =
        validate_main_executable(&macos_path, &bundle_metadata.executable_filename)?;
    inspect_macho_file(&executable_path, true)?;
    scan_contents_for_macho(&contents_path)?;

    Ok(VerifiedAppBundle {
        relative_app_path,
        bundle_identifier: bundle_metadata.bundle_identifier,
        version: extracted_archive.version.clone(),
        executable_filename: bundle_metadata.executable_filename,
        architecture: TARGET_ARCHITECTURE,
    })
}

fn validate_staging_root(staging_root: &Path) -> Result<(), BundleValidationError> {
    if !staging_root.is_absolute()
        || staging_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BundleValidationError::StagingPath);
    }

    match fs::symlink_metadata(staging_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(BundleValidationError::StagingPath)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(BundleValidationError::StagingRead)
        }
        Err(_) => Err(BundleValidationError::StagingRead),
    }
}

fn resolve_app_path(
    staging_root: &Path,
    relative_path: &Path,
) -> Result<(PathBuf, PathBuf), BundleValidationError> {
    let components = safe_relative_components(relative_path)?;
    if components.is_empty() {
        return Err(BundleValidationError::BundleLayout);
    }
    if components.last().map(String::as_str) != Some(APP_BUNDLE_NAME) {
        return Err(BundleValidationError::BundleLayout);
    }

    let mut normalized_relative = PathBuf::new();
    let mut app_path = staging_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        normalized_relative.push(component);
        app_path.push(component);

        let metadata = fs::symlink_metadata(&app_path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                if index + 1 == components.len() {
                    BundleValidationError::BundleLayout
                } else {
                    BundleValidationError::StagingRead
                }
            } else {
                BundleValidationError::StagingRead
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(BundleValidationError::StagingPath);
        }
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(BundleValidationError::StagingPath);
        }
        if index + 1 == components.len() && !metadata.is_dir() {
            return Err(BundleValidationError::BundleLayout);
        }
    }

    Ok((normalized_relative, app_path))
}

fn safe_relative_components(path: &Path) -> Result<Vec<String>, BundleValidationError> {
    if path.is_absolute() {
        return Err(BundleValidationError::StagingPath);
    }

    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(BundleValidationError::StagingPath);
        };
        let value = value.to_str().ok_or(BundleValidationError::StagingPath)?;
        if value.is_empty() || value.contains('\\') || value.chars().any(char::is_control) {
            return Err(BundleValidationError::StagingPath);
        }
        components.push(value.to_owned());
    }
    Ok(components)
}

fn require_directory(
    path: &Path,
    invalid_error: BundleValidationError,
) -> Result<(), BundleValidationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            invalid_error
        } else {
            BundleValidationError::StagingRead
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_error);
    }
    Ok(())
}

fn require_regular_file(
    path: &Path,
    invalid_error: BundleValidationError,
) -> Result<fs::Metadata, BundleValidationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            invalid_error
        } else {
            BundleValidationError::StagingRead
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_error);
    }
    Ok(metadata)
}

fn open_read_no_follow(
    path: &Path,
    failure: BundleValidationError,
) -> Result<File, BundleValidationError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| failure)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| failure)
    }
}

fn read_bundle_metadata(
    info_plist_path: &Path,
    expected_version: &str,
) -> Result<BundleMetadata, BundleValidationError> {
    let metadata = require_regular_file(info_plist_path, BundleValidationError::BundlePlist)?;
    if metadata.len() > MAX_INFO_PLIST_BYTES {
        return Err(BundleValidationError::BundlePlist);
    }

    let file = open_read_no_follow(info_plist_path, BundleValidationError::BundlePlist)?;
    let value =
        Value::from_reader(BufReader::new(file)).map_err(|_| BundleValidationError::BundlePlist)?;
    let dictionary = value
        .as_dictionary()
        .ok_or(BundleValidationError::BundlePlist)?;

    let bundle_identifier = dictionary
        .get("CFBundleIdentifier")
        .and_then(Value::as_string)
        .ok_or(BundleValidationError::BundlePlist)?;
    if bundle_identifier != MANIFEST_PRODUCT_ID {
        return Err(BundleValidationError::BundleIdentity);
    }

    let version = dictionary
        .get("CFBundleShortVersionString")
        .and_then(Value::as_string)
        .ok_or(BundleValidationError::BundlePlist)?;
    // CFBundleShortVersionString does not carry SemVer build metadata.
    let expected_bundle_version = expected_version
        .split_once('+')
        .map_or(expected_version, |(version, _)| version);
    if version != expected_bundle_version {
        return Err(BundleValidationError::BundleVersion);
    }

    let executable_value = dictionary
        .get("CFBundleExecutable")
        .ok_or(BundleValidationError::BundleExecutable)?;
    let executable_filename = executable_value
        .as_string()
        .ok_or(BundleValidationError::BundlePlist)?;
    if !is_safe_single_filename(executable_filename) {
        return Err(BundleValidationError::BundleExecutable);
    }

    Ok(BundleMetadata {
        bundle_identifier: bundle_identifier.to_owned(),
        executable_filename: executable_filename.to_owned(),
    })
}

fn is_safe_single_filename(value: &str) -> bool {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return false;
    }

    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn validate_main_executable(
    macos_path: &Path,
    executable_filename: &str,
) -> Result<PathBuf, BundleValidationError> {
    let mut executable_candidate_count = 0_usize;
    let mut candidate_name: Option<OsString> = None;
    let entries = fs::read_dir(macos_path).map_err(|_| BundleValidationError::StagingRead)?;
    for entry in entries {
        let entry = entry.map_err(|_| BundleValidationError::StagingRead)?;
        let file_type = entry
            .file_type()
            .map_err(|_| BundleValidationError::StagingRead)?;
        let file_name = entry.file_name();
        if file_type.is_symlink() {
            if file_name == OsStr::new(executable_filename) {
                return Err(BundleValidationError::BundleExecutable);
            }
            continue;
        }
        if !file_type.is_file() {
            if file_name == OsStr::new(executable_filename) {
                return Err(BundleValidationError::BundleExecutable);
            }
            continue;
        }

        let path = entry.path();
        let metadata = require_regular_file(&path, BundleValidationError::StagingRead)?;
        if has_owner_execute(&metadata) {
            executable_candidate_count += 1;
            candidate_name = Some(file_name);
        }
    }

    if executable_candidate_count != 1
        || candidate_name.as_deref() != Some(OsStr::new(executable_filename))
    {
        return Err(BundleValidationError::BundleExecutable);
    }

    let executable_path = macos_path.join(executable_filename);
    let metadata = require_regular_file(&executable_path, BundleValidationError::BundleExecutable)?;
    if !has_owner_execute(&metadata) {
        return Err(BundleValidationError::BundleExecutable);
    }
    Ok(executable_path)
}

fn has_owner_execute(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o100 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn scan_contents_for_macho(contents_path: &Path) -> Result<(), BundleValidationError> {
    let mut directories = vec![contents_path.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let metadata =
            fs::symlink_metadata(&directory).map_err(|_| BundleValidationError::StagingRead)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if !metadata.is_dir() {
            return Err(BundleValidationError::BundleLayout);
        }

        let entries = fs::read_dir(&directory).map_err(|_| BundleValidationError::StagingRead)?;
        for entry in entries {
            let entry = entry.map_err(|_| BundleValidationError::StagingRead)?;
            let file_type = entry
                .file_type()
                .map_err(|_| BundleValidationError::StagingRead)?;
            if file_type.is_symlink() {
                continue;
            }

            let path = entry.path();
            if file_type.is_dir() {
                let metadata =
                    fs::symlink_metadata(&path).map_err(|_| BundleValidationError::StagingRead)?;
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    directories.push(path);
                }
                continue;
            }
            if file_type.is_file() {
                let metadata =
                    fs::symlink_metadata(&path).map_err(|_| BundleValidationError::StagingRead)?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    continue;
                }
                inspect_macho_file(&path, false)?;
            }
        }
    }
    Ok(())
}

fn inspect_macho_file(
    path: &Path,
    required_main_executable: bool,
) -> Result<(), BundleValidationError> {
    let metadata = require_regular_file(
        path,
        if required_main_executable {
            BundleValidationError::BundleExecutable
        } else {
            BundleValidationError::StagingRead
        },
    )?;
    let file = open_read_no_follow(
        path,
        if required_main_executable {
            BundleValidationError::BundleExecutable
        } else {
            BundleValidationError::StagingRead
        },
    )?;
    inspect_macho_reader(file, metadata.len(), required_main_executable)
}

fn inspect_macho_reader(
    mut file: File,
    file_size: u64,
    required_main_executable: bool,
) -> Result<(), BundleValidationError> {
    let mut magic = [0_u8; 4];
    let bytes_read = file
        .read(&mut magic)
        .map_err(|_| BundleValidationError::StagingRead)?;
    if bytes_read < magic.len() {
        return if required_main_executable {
            Err(BundleValidationError::BundleArchitecture)
        } else {
            Ok(())
        };
    }

    if magic == MH_MAGIC_64 {
        return validate_thin_macho_64(&mut file, file_size, required_main_executable);
    }
    if is_known_non_thin_macho_magic(magic) {
        return Err(BundleValidationError::BundleArchitecture);
    }
    if required_main_executable {
        Err(BundleValidationError::BundleArchitecture)
    } else {
        Ok(())
    }
}

fn is_known_non_thin_macho_magic(magic: [u8; 4]) -> bool {
    matches!(
        magic,
        MH_CIGAM_64
            | MH_MAGIC_32
            | MH_CIGAM_32
            | FAT_MAGIC
            | FAT_CIGAM
            | FAT_MAGIC_64
            | FAT_CIGAM_64
    )
}

fn validate_thin_macho_64(
    file: &mut File,
    file_size: u64,
    required_main_executable: bool,
) -> Result<(), BundleValidationError> {
    if file_size < MACH_HEADER_64_BYTES {
        return Err(BundleValidationError::BundleArchitecture);
    }

    file.seek(SeekFrom::Start(4))
        .map_err(|_| BundleValidationError::StagingRead)?;
    let mut header_tail = [0_u8; 28];
    file.read_exact(&mut header_tail).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            BundleValidationError::BundleArchitecture
        } else {
            BundleValidationError::StagingRead
        }
    })?;

    let cpu_type = u32::from_le_bytes(header_tail[0..4].try_into().expect("cpu type"));
    if cpu_type != CPU_TYPE_ARM64 {
        return Err(BundleValidationError::BundleArchitecture);
    }

    let file_type = u32::from_le_bytes(header_tail[8..12].try_into().expect("file type"));
    if required_main_executable && file_type != MH_EXECUTE {
        return Err(BundleValidationError::BundleArchitecture);
    }

    let command_count = u32::from_le_bytes(header_tail[12..16].try_into().expect("ncmds"));
    let command_bytes = u32::from_le_bytes(header_tail[16..20].try_into().expect("sizeofcmds"));
    if command_count > MAX_LOAD_COMMANDS {
        return Err(BundleValidationError::BundleArchitecture);
    }

    let command_end = MACH_HEADER_64_BYTES
        .checked_add(u64::from(command_bytes))
        .ok_or(BundleValidationError::BundleArchitecture)?;
    if command_end > file_size {
        return Err(BundleValidationError::BundleArchitecture);
    }

    let minimum_command_bytes = u64::from(command_count)
        .checked_mul(LOAD_COMMAND_HEADER_BYTES)
        .ok_or(BundleValidationError::BundleArchitecture)?;
    if minimum_command_bytes > u64::from(command_bytes)
        || (command_count == 0 && command_bytes != 0)
    {
        return Err(BundleValidationError::BundleArchitecture);
    }

    let mut command_offset = MACH_HEADER_64_BYTES;
    for _ in 0..command_count {
        let command_header_end = command_offset
            .checked_add(LOAD_COMMAND_HEADER_BYTES)
            .ok_or(BundleValidationError::BundleArchitecture)?;
        if command_header_end > command_end {
            return Err(BundleValidationError::BundleArchitecture);
        }

        file.seek(SeekFrom::Start(command_offset))
            .map_err(|_| BundleValidationError::StagingRead)?;
        let mut command_header = [0_u8; 8];
        file.read_exact(&mut command_header).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                BundleValidationError::BundleArchitecture
            } else {
                BundleValidationError::StagingRead
            }
        })?;
        let command_size =
            u32::from_le_bytes(command_header[4..8].try_into().expect("load command size"));
        if u64::from(command_size) < LOAD_COMMAND_HEADER_BYTES {
            return Err(BundleValidationError::BundleArchitecture);
        }
        command_offset = command_offset
            .checked_add(u64::from(command_size))
            .ok_or(BundleValidationError::BundleArchitecture)?;
        if command_offset > command_end {
            return Err(BundleValidationError::BundleArchitecture);
        }
    }

    if command_offset != command_end {
        return Err(BundleValidationError::BundleArchitecture);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);
    const TEST_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const TEST_VERSION: &str = "1.2.3";
    const TEST_EXECUTABLE: &str = "CornellMethodNotebook";

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let counter = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("cuetrio-update-bundle-{timestamp}-{counter}"));
            fs::create_dir(&path).expect("test root");
            Self { path }
        }

        fn app_path(&self) -> PathBuf {
            self.path
                .join("extract")
                .join(TEST_DIGEST)
                .join(APP_BUNDLE_NAME)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn extracted_archive_with_version(version: &str) -> ExtractedArchive {
        ExtractedArchive {
            relative_app_path: PathBuf::from("extract")
                .join(TEST_DIGEST)
                .join(APP_BUNDLE_NAME),
            artifact_id: "test-artifact".to_owned(),
            raw_sha256: TEST_DIGEST.to_owned(),
            version: version.to_owned(),
            architecture: "x86_64-apple-darwin".to_owned(),
        }
    }

    fn extracted_archive() -> ExtractedArchive {
        extracted_archive_with_version(TEST_VERSION)
    }

    fn create_layout(root: &TestRoot) -> PathBuf {
        let app = root.app_path();
        fs::create_dir_all(app.join(CONTENTS_DIRECTORY_NAME).join(MACOS_DIRECTORY_NAME))
            .expect("bundle layout");
        app
    }

    fn plist_xml(
        bundle_identifier: Option<&str>,
        version: Option<&str>,
        executable: Option<&str>,
    ) -> Vec<u8> {
        let mut entries = String::new();
        for (key, value) in [
            ("CFBundleIdentifier", bundle_identifier),
            ("CFBundleShortVersionString", version),
            ("CFBundleExecutable", executable),
        ] {
            if let Some(value) = value {
                entries.push_str(&format!("<key>{key}</key><string>{value}</string>"));
            }
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>{entries}</dict></plist>"
        )
        .into_bytes()
    }

    fn write_xml_plist(
        app: &Path,
        bundle_identifier: Option<&str>,
        version: Option<&str>,
        executable: Option<&str>,
    ) {
        fs::write(
            app.join(CONTENTS_DIRECTORY_NAME).join(INFO_PLIST_NAME),
            plist_xml(bundle_identifier, version, executable),
        )
        .expect("plist");
    }

    fn write_binary_plist(app: &Path) {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "CFBundleIdentifier".to_owned(),
            Value::String(MANIFEST_PRODUCT_ID.to_owned()),
        );
        dictionary.insert(
            "CFBundleShortVersionString".to_owned(),
            Value::String(TEST_VERSION.to_owned()),
        );
        dictionary.insert(
            "CFBundleExecutable".to_owned(),
            Value::String(TEST_EXECUTABLE.to_owned()),
        );
        Value::Dictionary(dictionary)
            .to_file_binary(app.join(CONTENTS_DIRECTORY_NAME).join(INFO_PLIST_NAME))
            .expect("binary plist");
    }

    fn write_non_string_executable_plist(app: &Path) {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "CFBundleIdentifier".to_owned(),
            Value::String(MANIFEST_PRODUCT_ID.to_owned()),
        );
        dictionary.insert(
            "CFBundleShortVersionString".to_owned(),
            Value::String(TEST_VERSION.to_owned()),
        );
        dictionary.insert("CFBundleExecutable".to_owned(), Value::Boolean(true));
        Value::Dictionary(dictionary)
            .to_file_xml(app.join(CONTENTS_DIRECTORY_NAME).join(INFO_PLIST_NAME))
            .expect("typed plist");
    }

    fn write_main_executable(app: &Path, bytes: &[u8], executable: bool) {
        let path = app
            .join(CONTENTS_DIRECTORY_NAME)
            .join(MACOS_DIRECTORY_NAME)
            .join(TEST_EXECUTABLE);
        fs::write(&path, bytes).expect("executable");
        set_mode(&path, executable);
    }

    fn write_extra_file(app: &Path, relative_path: &str, bytes: &[u8], executable: bool) {
        let path = app.join(CONTENTS_DIRECTORY_NAME).join(relative_path);
        fs::create_dir_all(path.parent().expect("extra parent")).expect("extra directory");
        fs::write(&path, bytes).expect("extra file");
        set_mode(&path, executable);
    }

    fn set_mode(path: &Path, executable: bool) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                path,
                fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
            )
            .expect("permissions");
        }
        #[cfg(not(unix))]
        {
            let _ = (path, executable);
        }
    }

    fn thin_macho_with_file_type(
        file_type: u32,
        cpu_type: u32,
        command_count: u32,
        command_bytes: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MH_MAGIC_64);
        bytes.extend_from_slice(&cpu_type.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&file_type.to_le_bytes());
        bytes.extend_from_slice(&command_count.to_le_bytes());
        bytes.extend_from_slice(&command_bytes.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes
    }

    fn thin_macho(cpu_type: u32, command_count: u32, command_bytes: u32) -> Vec<u8> {
        thin_macho_with_file_type(MH_EXECUTE, cpu_type, command_count, command_bytes)
    }

    fn valid_main_macho() -> Vec<u8> {
        thin_macho(CPU_TYPE_ARM64, 0, 0)
    }

    fn assert_code(result: Result<VerifiedAppBundle, BundleValidationError>, code: &str) {
        let error = result.expect_err("expected bundle validation error");
        assert_eq!(error.code(), code);
        assert_eq!(error.to_string(), code);
    }

    fn valid_bundle(root: &TestRoot) -> ExtractedArchive {
        let app = create_layout(root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some(TEST_VERSION),
            Some(TEST_EXECUTABLE),
        );
        write_main_executable(&app, &valid_main_macho(), true);
        extracted_archive()
    }

    #[test]
    fn verifies_xml_and_binary_plists_with_a_thin_arm64_executable() {
        let root = TestRoot::new();
        let archive = valid_bundle(&root);
        let verified = validate_extracted_app_bundle(&archive, &root.path).expect("verify xml");
        assert_eq!(verified.relative_app_path, archive.relative_app_path);
        assert_eq!(verified.bundle_identifier, MANIFEST_PRODUCT_ID);
        assert_eq!(verified.version, TEST_VERSION);
        assert_eq!(verified.executable_filename, TEST_EXECUTABLE);
        assert_eq!(verified.architecture, TARGET_ARCHITECTURE);

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_binary_plist(&app);
        write_main_executable(&app, &valid_main_macho(), true);
        validate_extracted_app_bundle(&extracted_archive(), &root.path).expect("verify binary");
    }

    #[test]
    fn verifies_build_metadata_release_against_bundle_release_version() {
        const MANIFEST_VERSION: &str = "1.2.3+build.7";

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some("1.2.3"),
            Some(TEST_EXECUTABLE),
        );
        write_main_executable(&app, &valid_main_macho(), true);

        let archive = extracted_archive_with_version(MANIFEST_VERSION);
        let verified =
            validate_extracted_app_bundle(&archive, &root.path).expect("verify build metadata");
        assert_eq!(verified.version, MANIFEST_VERSION);
    }

    #[test]
    fn rejects_different_bundle_core_version_for_build_metadata_release() {
        const MANIFEST_VERSION: &str = "1.2.3+build.7";

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some("1.2.4"),
            Some(TEST_EXECUTABLE),
        );
        write_main_executable(&app, &valid_main_macho(), true);

        assert_code(
            validate_extracted_app_bundle(
                &extracted_archive_with_version(MANIFEST_VERSION),
                &root.path,
            ),
            "bundle-version",
        );
    }

    #[test]
    fn rejects_missing_malformed_wrong_type_identity_and_version_plists() {
        let root = TestRoot::new();
        let app = create_layout(&root);
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-plist",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        fs::write(
            app.join(CONTENTS_DIRECTORY_NAME).join(INFO_PLIST_NAME),
            b"not a plist",
        )
        .expect("malformed plist");
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-plist",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some("com.example.other"),
            Some(TEST_VERSION),
            Some(TEST_EXECUTABLE),
        );
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-identity",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some("9.9.9"),
            Some(TEST_EXECUTABLE),
        );
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-version",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_non_string_executable_plist(&app);
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-plist",
        );
    }

    #[cfg(unix)]
    fn create_fifo(path: &Path) {
        let path =
            std::ffi::CString::new(path.to_str().expect("fifo path")).expect("fifo path bytes");
        let result = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(result, 0, "create fifo");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_special_files_at_required_bundle_entries() {
        let root = TestRoot::new();
        let app = create_layout(&root);
        create_fifo(&app.join(CONTENTS_DIRECTORY_NAME).join(INFO_PLIST_NAME));
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-plist",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some(TEST_VERSION),
            Some(TEST_EXECUTABLE),
        );
        let macos = app.join(CONTENTS_DIRECTORY_NAME).join(MACOS_DIRECTORY_NAME);
        fs::remove_dir_all(&macos).expect("remove macos");
        create_fifo(&macos);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-layout",
        );

        let root = TestRoot::new();
        let _archive = valid_bundle(&root);
        let app = root.app_path();
        let executable = app
            .join(CONTENTS_DIRECTORY_NAME)
            .join(MACOS_DIRECTORY_NAME)
            .join(TEST_EXECUTABLE);
        fs::remove_file(&executable).expect("remove executable");
        create_fifo(&executable);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-executable",
        );

        let root = TestRoot::new();
        let _archive = valid_bundle(&root);
        let app = root.app_path();
        fs::remove_dir_all(&app).expect("remove app");
        create_fifo(&app);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-layout",
        );
    }

    #[test]
    fn rejects_missing_unsafe_multiple_and_non_executable_main_files() {
        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some(TEST_VERSION),
            Some(TEST_EXECUTABLE),
        );
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-executable",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some(TEST_VERSION),
            Some("../outside"),
        );
        write_main_executable(&app, &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-executable",
        );

        let root = TestRoot::new();
        let _archive = valid_bundle(&root);
        let app = root.app_path();
        write_extra_file(&app, "MacOS/second", &valid_main_macho(), true);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-executable",
        );

        let root = TestRoot::new();
        let app = create_layout(&root);
        write_xml_plist(
            &app,
            Some(MANIFEST_PRODUCT_ID),
            Some(TEST_VERSION),
            Some(TEST_EXECUTABLE),
        );
        write_main_executable(&app, &valid_main_macho(), false);
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-executable",
        );
    }

    #[test]
    fn rejects_non_arm64_thin_fat_and_malformed_macho_headers() {
        let cases = [
            thin_macho(0x0100_0007, 0, 0),
            thin_macho(0x0000_0007, 0, 0),
            {
                let mut bytes = FAT_MAGIC.to_vec();
                bytes.extend_from_slice(&thin_macho(CPU_TYPE_ARM64, 0, 0));
                bytes
            },
            {
                let mut bytes = MH_MAGIC_32.to_vec();
                bytes.extend_from_slice(&[0_u8; 24]);
                bytes
            },
            {
                let mut bytes = MH_MAGIC_64.to_vec();
                bytes.extend_from_slice(&[0_u8; 8]);
                bytes
            },
            thin_macho(CPU_TYPE_ARM64, 1, 8),
            {
                let mut bytes = thin_macho(CPU_TYPE_ARM64, 1, 8);
                bytes.extend_from_slice(&[0_u8; 4]);
                bytes
            },
        ];

        for bytes in cases {
            let root = TestRoot::new();
            let app = create_layout(&root);
            write_xml_plist(
                &app,
                Some(MANIFEST_PRODUCT_ID),
                Some(TEST_VERSION),
                Some(TEST_EXECUTABLE),
            );
            write_main_executable(&app, &bytes, true);
            assert_code(
                validate_extracted_app_bundle(&extracted_archive(), &root.path),
                "bundle-architecture",
            );
        }
    }

    #[test]
    fn rejects_non_execute_main_macho_filetypes_with_fixed_architecture_error() {
        const MH_OBJECT: u32 = 0x0000_0001;
        const MH_DYLIB: u32 = 0x0000_0006;
        const UNKNOWN_FILE_TYPE: u32 = 0x7fff_ffff;

        for file_type in [MH_OBJECT, MH_DYLIB, UNKNOWN_FILE_TYPE] {
            let root = TestRoot::new();
            let app = create_layout(&root);
            write_xml_plist(
                &app,
                Some(MANIFEST_PRODUCT_ID),
                Some(TEST_VERSION),
                Some(TEST_EXECUTABLE),
            );
            write_main_executable(
                &app,
                &thin_macho_with_file_type(file_type, CPU_TYPE_ARM64, 0, 0),
                true,
            );
            assert_code(
                validate_extracted_app_bundle(&extracted_archive(), &root.path),
                "bundle-architecture",
            );
        }
    }

    #[test]
    fn validates_additional_macho_code_and_allows_non_macho_resources() {
        let root = TestRoot::new();
        let _archive = valid_bundle(&root);
        let app = root.app_path();
        write_extra_file(&app, "Helpers/arm64-helper", &valid_main_macho(), false);
        write_extra_file(&app, "Resources/readme.txt", b"resource", false);
        validate_extracted_app_bundle(&extracted_archive(), &root.path)
            .expect("additional arm64 code");

        let root = TestRoot::new();
        let _archive = valid_bundle(&root);
        let app = root.app_path();
        write_extra_file(
            &app,
            "Helpers/x86-helper",
            &thin_macho(0x0100_0007, 0, 0),
            false,
        );
        assert_code(
            validate_extracted_app_bundle(&extracted_archive(), &root.path),
            "bundle-architecture",
        );
    }

    #[test]
    fn rejects_unsafe_relative_paths_and_required_entry_symlinks() {
        let root = TestRoot::new();
        let mut archive = extracted_archive();
        archive.relative_app_path = PathBuf::from("../outside/Cuetrio.app");
        assert_code(
            validate_extracted_app_bundle(&archive, &root.path),
            "staging-path",
        );

        #[cfg(unix)]
        {
            let root = TestRoot::new();
            let _archive = valid_bundle(&root);
            let app = root.app_path();
            let outside = root.path.join("outside-app");
            fs::create_dir_all(
                outside
                    .join(CONTENTS_DIRECTORY_NAME)
                    .join(MACOS_DIRECTORY_NAME),
            )
            .expect("outside app");
            fs::remove_dir_all(&app).expect("remove app");
            std::os::unix::fs::symlink(&outside, &app).expect("root symlink");
            assert_code(
                validate_extracted_app_bundle(&extracted_archive(), &root.path),
                "staging-path",
            );
        }

        #[cfg(unix)]
        {
            let root = TestRoot::new();
            let _archive = valid_bundle(&root);
            let app = root.app_path();
            let contents = app.join(CONTENTS_DIRECTORY_NAME);
            let outside = root.path.join("outside-contents");
            fs::create_dir(&outside).expect("outside contents");
            fs::remove_dir_all(&contents).expect("remove contents");
            std::os::unix::fs::symlink(&outside, &contents).expect("contents symlink");
            assert_code(
                validate_extracted_app_bundle(&extracted_archive(), &root.path),
                "bundle-layout",
            );
        }

        #[cfg(unix)]
        {
            let root = TestRoot::new();
            let _archive = valid_bundle(&root);
            let app = root.app_path();
            let info = app.join(CONTENTS_DIRECTORY_NAME).join(INFO_PLIST_NAME);
            let outside = root.path.join("outside-info.plist");
            fs::write(
                &outside,
                plist_xml(
                    Some(MANIFEST_PRODUCT_ID),
                    Some(TEST_VERSION),
                    Some(TEST_EXECUTABLE),
                ),
            )
            .expect("outside plist");
            fs::remove_file(&info).expect("remove info");
            std::os::unix::fs::symlink(&outside, &info).expect("info symlink");
            assert_code(
                validate_extracted_app_bundle(&extracted_archive(), &root.path),
                "bundle-plist",
            );
        }

        #[cfg(unix)]
        {
            let root = TestRoot::new();
            let _archive = valid_bundle(&root);
            let app = root.app_path();
            let executable = app
                .join(CONTENTS_DIRECTORY_NAME)
                .join(MACOS_DIRECTORY_NAME)
                .join(TEST_EXECUTABLE);
            let outside = root.path.join("outside-executable");
            fs::write(&outside, valid_main_macho()).expect("outside executable");
            set_mode(&outside, true);
            fs::remove_file(&executable).expect("remove executable");
            std::os::unix::fs::symlink(&outside, &executable).expect("executable symlink");
            assert_code(
                validate_extracted_app_bundle(&extracted_archive(), &root.path),
                "bundle-executable",
            );
        }
    }
}

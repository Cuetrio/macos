use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, CONTENT_TYPE, LOCATION};
use reqwest::{Client, Response, Url};
use ring::digest::{Context, SHA256};

use crate::update_manifest::{
    is_safe_public_update_url, UpdateManifest, UpdateRelease, TARGET_ARTIFACT_FORMAT,
};
use crate::update_provider::{
    validate_public_address, validate_public_ip_literal, PinnedDnsResolver, PublicAddressResolver,
    SystemPublicAddressResolver,
};
use crate::update_signature::{EmbeddedTrustedKeyStore, SignatureVerificationError};

pub(crate) const PACKAGE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const PACKAGE_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_ARTIFACT_REDIRECT_HOPS: usize = 5;
pub(crate) const MAX_PACKAGE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub(crate) const ALLOWED_ARTIFACT_CONTENT_TYPES: &[&str] =
    &["application/gzip", "application/octet-stream"];

const STREAM_BUFFER_BYTES: usize = 64 * 1024;
const PACKAGE_BODY_MIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const PACKAGE_BODY_BYTES_PER_SECOND: u64 = 1024 * 1024;
const PACKAGE_BODY_MAX_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

fn package_body_timeout(size_bytes: u64) -> Duration {
    let transfer_seconds = size_bytes.saturating_add(PACKAGE_BODY_BYTES_PER_SECOND - 1)
        / PACKAGE_BODY_BYTES_PER_SECOND;
    let timeout_seconds = PACKAGE_BODY_MIN_TIMEOUT
        .as_secs()
        .saturating_add(transfer_seconds);
    Duration::from_secs(timeout_seconds).min(PACKAGE_BODY_MAX_TIMEOUT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackageDownloadError {
    Network,
    Timeout,
    HttpStatus,
    Redirect,
    ContentType,
    Size,
    Digest,
    Signature,
    SignatureKey,
    SignatureProof,
    StagingPath,
    StagingRead,
    StagingWrite,
    StagingRename,
}

impl PackageDownloadError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Network => "package-network",
            Self::Timeout => "package-timeout",
            Self::HttpStatus => "package-http-status",
            Self::Redirect => "package-redirect",
            Self::ContentType => "package-content-type",
            Self::Size => "package-size",
            Self::Digest => "package-digest",
            Self::Signature => "package-signature",
            Self::SignatureKey => "package-signature-key",
            Self::SignatureProof => "package-signature-proof",
            Self::StagingPath => "staging-path",
            Self::StagingRead => "staging-read",
            Self::StagingWrite => "staging-write",
            Self::StagingRename => "staging-rename",
        }
    }
}

impl fmt::Display for PackageDownloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for PackageDownloadError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactHttpError {
    Network,
    Timeout,
    Redirect,
    Size,
    Internal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactHttpRequest {
    pub(crate) url: String,
    pub(crate) body_timeout: Duration,
    pub(crate) connect_timeout: Duration,
    pub(crate) read_timeout: Duration,
    pub(crate) max_redirects: usize,
    pub(crate) max_body_bytes: u64,
    pub(crate) expected_size_bytes: u64,
    pub(crate) accepted_content_types: &'static [&'static str],
}

impl ArtifactHttpRequest {
    fn fixed(selected_release: &UpdateRelease) -> Self {
        Self {
            url: selected_release.artifact.url.as_str().to_string(),
            body_timeout: package_body_timeout(selected_release.artifact.size_bytes),
            connect_timeout: PACKAGE_CONNECTION_TIMEOUT,
            read_timeout: PACKAGE_READ_IDLE_TIMEOUT,
            max_redirects: MAX_ARTIFACT_REDIRECT_HOPS,
            max_body_bytes: MAX_PACKAGE_BYTES,
            expected_size_bytes: selected_release.artifact.size_bytes,
            accepted_content_types: ALLOWED_ARTIFACT_CONTENT_TYPES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactRedirect {
    pub(crate) status: u16,
    pub(crate) location: Option<String>,
}

pub(crate) struct ArtifactHttpResponse {
    pub(crate) status: u16,
    pub(crate) content_type: Option<String>,
    pub(crate) content_length: Option<u64>,
    pub(crate) body: Box<dyn Read>,
    pub(crate) redirects: Vec<ArtifactRedirect>,
    pub(crate) final_url: String,
}

impl ArtifactHttpResponse {
    #[cfg(test)]
    fn from_reader(
        status: u16,
        content_type: Option<&str>,
        content_length: Option<u64>,
        body: impl Read + 'static,
        redirects: Vec<ArtifactRedirect>,
        final_url: &str,
    ) -> Self {
        Self {
            status,
            content_type: content_type.map(str::to_owned),
            content_length,
            body: Box::new(body),
            redirects,
            final_url: final_url.to_string(),
        }
    }
}

pub(crate) trait ArtifactHttpTransport {
    fn get(&self, request: ArtifactHttpRequest) -> Result<ArtifactHttpResponse, ArtifactHttpError>;

    fn validate_url(&self, url: &Url) -> Result<(), ArtifactHttpError> {
        validate_public_ip_literal(url).map_err(|_| ArtifactHttpError::Network)
    }
}

pub(crate) struct ReqwestArtifactHttpTransport {
    client: Client,
    resolver: Arc<dyn PublicAddressResolver>,
    pinned_resolver: PinnedDnsResolver,
}

struct AsyncResponseBody {
    response: Response,
    pending: Vec<u8>,
    pending_offset: usize,
    finished: bool,
}

impl AsyncResponseBody {
    fn new(response: Response) -> Self {
        Self {
            response,
            pending: Vec::new(),
            pending_offset: 0,
            finished: false,
        }
    }
}

impl Read for AsyncResponseBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }

        loop {
            if self.pending_offset < self.pending.len() {
                let bytes_to_copy = (self.pending.len() - self.pending_offset).min(buffer.len());
                buffer[..bytes_to_copy].copy_from_slice(
                    &self.pending[self.pending_offset..self.pending_offset + bytes_to_copy],
                );
                self.pending_offset += bytes_to_copy;
                return Ok(bytes_to_copy);
            }

            if self.finished {
                return Ok(0);
            }

            let chunk = tauri::async_runtime::block_on(self.response.chunk())
                .map_err(map_reqwest_body_error)?;
            match chunk {
                Some(chunk) => {
                    self.pending = chunk.to_vec();
                    self.pending_offset = 0;
                }
                None => {
                    self.finished = true;
                    return Ok(0);
                }
            }
        }
    }
}

impl ReqwestArtifactHttpTransport {
    pub(crate) fn new() -> Result<Self, ArtifactHttpError> {
        Self::with_resolver(Arc::new(SystemPublicAddressResolver))
    }

    fn with_resolver(resolver: Arc<dyn PublicAddressResolver>) -> Result<Self, ArtifactHttpError> {
        let pinned_resolver = PinnedDnsResolver::new();
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // The blocking builder has no separate read-idle option. The async
            // client resets this timeout after every successful body chunk and
            // does not impose a total request deadline.
            .read_timeout(PACKAGE_READ_IDLE_TIMEOUT)
            .connect_timeout(PACKAGE_CONNECTION_TIMEOUT)
            .dns_resolver(Arc::new(pinned_resolver.clone()))
            .build()
            .map_err(|_| ArtifactHttpError::Internal)?;
        Ok(Self {
            client,
            resolver,
            pinned_resolver,
        })
    }
}

impl ArtifactHttpTransport for ReqwestArtifactHttpTransport {
    fn validate_url(&self, url: &Url) -> Result<(), ArtifactHttpError> {
        let addresses = validate_public_address(url, self.resolver.as_ref())
            .map_err(|_| ArtifactHttpError::Network)?;
        let host = crate::update_provider::canonical_host(url).ok_or(ArtifactHttpError::Network)?;
        self.pinned_resolver
            .pin(&host, &addresses)
            .map_err(|_| ArtifactHttpError::Network)
    }

    fn get(&self, request: ArtifactHttpRequest) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
        if request.max_body_bytes != MAX_PACKAGE_BYTES
            || request.max_redirects != MAX_ARTIFACT_REDIRECT_HOPS
            || request.accepted_content_types != ALLOWED_ARTIFACT_CONTENT_TYPES
        {
            return Err(ArtifactHttpError::Internal);
        }
        if request.expected_size_bytes == 0 || request.expected_size_bytes > MAX_PACKAGE_BYTES {
            return Err(ArtifactHttpError::Size);
        }
        if request.body_timeout != package_body_timeout(request.expected_size_bytes)
            || request.connect_timeout != PACKAGE_CONNECTION_TIMEOUT
            || request.read_timeout != PACKAGE_READ_IDLE_TIMEOUT
        {
            return Err(ArtifactHttpError::Internal);
        }

        let mut current_url = Url::parse(&request.url).map_err(|_| ArtifactHttpError::Internal)?;
        if !is_safe_artifact_url(&current_url) {
            return Err(ArtifactHttpError::Internal);
        }
        self.validate_url(&current_url)?;

        let mut visited = HashSet::new();
        visited.insert(current_url.to_string());
        let mut redirects = Vec::new();

        loop {
            let response =
                tauri::async_runtime::block_on(self.client.get(current_url.clone()).send())
                    .map_err(map_reqwest_error)?;
            let status = response.status().as_u16();

            if is_redirect_status(status) {
                if redirects.len() >= request.max_redirects {
                    return Err(ArtifactHttpError::Redirect);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(ArtifactHttpError::Redirect)?;
                let next_url = resolve_artifact_redirect(&current_url, location)
                    .map_err(|_| ArtifactHttpError::Redirect)?;
                self.validate_url(&next_url)?;
                if !visited.insert(next_url.to_string()) {
                    return Err(ArtifactHttpError::Redirect);
                }
                redirects.push(ArtifactRedirect {
                    status,
                    location: Some(location.to_string()),
                });
                current_url = next_url;
                continue;
            }

            let content_type = response_content_type(response.headers());
            let content_length = response.content_length();
            return Ok(ArtifactHttpResponse {
                status,
                content_type,
                content_length,
                body: Box::new(AsyncResponseBody::new(response)),
                redirects,
                final_url: current_url.to_string(),
            });
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VerifiedArchive {
    pub(crate) relative_package_path: PathBuf,
    pub(crate) artifact_id: String,
    pub(crate) raw_size_bytes: u64,
    pub(crate) raw_sha256: String,
    pub(crate) version: String,
    pub(crate) architecture: String,
}

#[derive(Debug)]
pub(crate) struct VerifiedArchiveHandle {
    pub(crate) archive: VerifiedArchive,
    pub(crate) package_file: File,
}

impl VerifiedArchiveHandle {
    pub(crate) fn new(archive: VerifiedArchive, package_file: File) -> Self {
        Self {
            archive,
            package_file,
        }
    }

    pub(crate) fn verify_contents(&self) -> Result<(), PackageDownloadError> {
        let mut package_file = self
            .package_file
            .try_clone()
            .map_err(|_| PackageDownloadError::StagingRead)?;
        let metadata = package_file
            .metadata()
            .map_err(|_| PackageDownloadError::StagingRead)?;
        if !metadata.is_file() {
            return Err(PackageDownloadError::StagingPath);
        }
        if metadata.len() != self.archive.raw_size_bytes {
            return Err(PackageDownloadError::Size);
        }
        package_file
            .seek(SeekFrom::Start(0))
            .map_err(|_| PackageDownloadError::StagingRead)?;
        let (actual_size_bytes, actual_sha256) = hash_cached_package(&mut package_file)?;
        if actual_size_bytes != self.archive.raw_size_bytes {
            return Err(PackageDownloadError::Size);
        }
        let expected_sha256 =
            decode_sha256(&self.archive.raw_sha256).ok_or(PackageDownloadError::Digest)?;
        if actual_sha256 != expected_sha256 {
            return Err(PackageDownloadError::Digest);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) enum CachedArtifact {
    Missing,
    Verified(VerifiedArchiveHandle),
    Invalid {
        error: PackageDownloadError,
        removable: bool,
    },
}

pub(crate) trait ArtifactSignatureVerifier {
    fn verify_selected_package(
        &self,
        selected_release: &UpdateRelease,
        actual_size_bytes: u64,
        actual_sha256: [u8; 32],
    ) -> Result<(), SignatureVerificationError>;
}

pub(crate) trait ArtifactSignatureVerifierFactory {
    fn create<'a>(
        &'a self,
        manifest_root: &UpdateManifest,
    ) -> Box<dyn ArtifactSignatureVerifier + 'a>;
}

pub(crate) struct UpdateSignatureVerifierAdapter<'a> {
    manifest_root: UpdateManifest,
    trust_store: &'a EmbeddedTrustedKeyStore,
}

impl<'a> UpdateSignatureVerifierAdapter<'a> {
    pub(crate) fn new(
        manifest_root: &UpdateManifest,
        trust_store: &'a EmbeddedTrustedKeyStore,
    ) -> Self {
        Self {
            manifest_root: manifest_root.clone(),
            trust_store,
        }
    }
}

impl ArtifactSignatureVerifier for UpdateSignatureVerifierAdapter<'_> {
    fn verify_selected_package(
        &self,
        selected_release: &UpdateRelease,
        actual_size_bytes: u64,
        actual_sha256: [u8; 32],
    ) -> Result<(), SignatureVerificationError> {
        crate::update_signature::verify_selected_package(
            &self.manifest_root,
            selected_release,
            actual_size_bytes,
            actual_sha256,
            self.trust_store,
        )
        .map(|_| ())
    }
}

impl ArtifactSignatureVerifierFactory for EmbeddedTrustedKeyStore {
    fn create<'a>(
        &'a self,
        manifest_root: &UpdateManifest,
    ) -> Box<dyn ArtifactSignatureVerifier + 'a> {
        Box::new(UpdateSignatureVerifierAdapter::new(manifest_root, self))
    }
}

pub(crate) fn download_and_verify_artifact(
    selected_release: &UpdateRelease,
    staging_root: &Path,
    artifact_transport: &dyn ArtifactHttpTransport,
    signature_verifier: &dyn ArtifactSignatureVerifier,
) -> Result<VerifiedArchiveHandle, PackageDownloadError> {
    let expected_digest = validate_selected_release(selected_release)?;
    let (incoming_directory, packages_directory) = ensure_staging_directories(staging_root)?;
    let digest_hex = selected_release.artifact.sha256.as_str();
    let part_path = incoming_directory.join(format!("{digest_hex}.part"));
    let final_path = packages_directory.join(format!("{digest_hex}.app.tar.gz"));
    ensure_final_absent(&final_path)?;

    let request = ArtifactHttpRequest::fixed(selected_release);
    let initial_url = Url::parse(&request.url).map_err(|_| PackageDownloadError::Network)?;
    artifact_transport
        .validate_url(&initial_url)
        .map_err(map_artifact_http_error)?;
    let mut response = artifact_transport
        .get(request.clone())
        .map_err(map_artifact_http_error)?;
    validate_artifact_response(artifact_transport, &request, &response)?;

    let mut package_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&part_path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                PackageDownloadError::StagingPath
            } else {
                PackageDownloadError::StagingWrite
            }
        })?;

    let result = stream_package_bytes(
        &mut response.body,
        &mut package_file,
        selected_release.artifact.size_bytes,
        request.body_timeout,
    );
    let (actual_size_bytes, digest_context) = match result {
        Ok(result) => result,
        Err(error) => {
            drop(package_file);
            return Err(cleanup_part(&part_path, error));
        }
    };

    if actual_size_bytes != selected_release.artifact.size_bytes {
        drop(package_file);
        return Err(cleanup_part(&part_path, PackageDownloadError::Size));
    }
    let actual_sha256 = finish_sha256(digest_context);
    if actual_sha256 != expected_digest {
        drop(package_file);
        return Err(cleanup_part(&part_path, PackageDownloadError::Digest));
    }

    if package_file.flush().is_err() {
        drop(package_file);
        return Err(cleanup_part(&part_path, PackageDownloadError::StagingWrite));
    }
    if package_file.sync_all().is_err() {
        drop(package_file);
        return Err(cleanup_part(&part_path, PackageDownloadError::StagingWrite));
    }
    if let Err(error) = signature_verifier.verify_selected_package(
        selected_release,
        actual_size_bytes,
        actual_sha256,
    ) {
        drop(package_file);
        return Err(cleanup_part(&part_path, map_signature_error(error)));
    }

    if let Err(error) = ensure_final_absent(&final_path) {
        drop(package_file);
        return Err(cleanup_part(&part_path, error));
    }
    if fs::rename(&part_path, &final_path).is_err() {
        drop(package_file);
        return Err(cleanup_part(
            &part_path,
            PackageDownloadError::StagingRename,
        ));
    }

    Ok(verified_archive_for_release(
        selected_release,
        actual_size_bytes,
        package_file,
    ))
}

pub(crate) fn revalidate_cached_artifact(
    selected_release: &UpdateRelease,
    staging_root: &Path,
    signature_verifier: &dyn ArtifactSignatureVerifier,
) -> Result<CachedArtifact, PackageDownloadError> {
    let expected_digest = validate_selected_release(selected_release)?;
    let (_, packages_directory) = ensure_staging_directories(staging_root)?;
    let digest_hex = selected_release.artifact.sha256.as_str();
    let final_path = packages_directory.join(format!("{digest_hex}.app.tar.gz"));

    let metadata = match fs::symlink_metadata(&final_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(CachedArtifact::Missing)
        }
        Err(_) => return Err(PackageDownloadError::StagingRead),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(CachedArtifact::Invalid {
            error: PackageDownloadError::StagingPath,
            removable: false,
        });
    }
    if metadata.len() != selected_release.artifact.size_bytes {
        return Ok(CachedArtifact::Invalid {
            error: PackageDownloadError::Size,
            removable: true,
        });
    }

    let mut package_file = open_cached_package(&final_path)?;
    let (actual_size_bytes, actual_sha256) = hash_cached_package(&mut package_file)?;
    if actual_size_bytes != selected_release.artifact.size_bytes {
        return Ok(CachedArtifact::Invalid {
            error: PackageDownloadError::Size,
            removable: true,
        });
    }
    if actual_sha256 != expected_digest {
        return Ok(CachedArtifact::Invalid {
            error: PackageDownloadError::Digest,
            removable: true,
        });
    }

    if let Err(error) = signature_verifier.verify_selected_package(
        selected_release,
        actual_size_bytes,
        actual_sha256,
    ) {
        return Ok(CachedArtifact::Invalid {
            error: map_signature_error(error),
            removable: true,
        });
    }

    Ok(CachedArtifact::Verified(verified_archive_for_release(
        selected_release,
        actual_size_bytes,
        package_file,
    )))
}

pub(crate) fn remove_invalid_cached_artifact(
    selected_release: &UpdateRelease,
    staging_root: &Path,
) -> Result<(), PackageDownloadError> {
    validate_selected_release(selected_release)?;
    let (_, packages_directory) = ensure_staging_directories(staging_root)?;
    let final_path =
        packages_directory.join(format!("{}.app.tar.gz", selected_release.artifact.sha256));
    let metadata = match fs::symlink_metadata(&final_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PackageDownloadError::StagingRead),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PackageDownloadError::StagingPath);
    }
    fs::remove_file(&final_path).map_err(|_| PackageDownloadError::StagingWrite)
}

fn verified_archive_for_release(
    selected_release: &UpdateRelease,
    raw_size_bytes: u64,
    package_file: File,
) -> VerifiedArchiveHandle {
    VerifiedArchiveHandle::new(
        VerifiedArchive {
            relative_package_path: PathBuf::from("packages")
                .join(format!("{}.app.tar.gz", selected_release.artifact.sha256)),
            artifact_id: selected_release.artifact.artifact_id.clone(),
            raw_size_bytes,
            raw_sha256: selected_release.artifact.sha256.clone(),
            version: selected_release.version.to_string(),
            architecture: selected_release.architecture.clone(),
        },
        package_file,
    )
}

fn open_cached_package(path: &Path) -> Result<File, PackageDownloadError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| PackageDownloadError::StagingRead)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| PackageDownloadError::StagingRead)
    }
}

fn hash_cached_package(package_file: &mut File) -> Result<(u64, [u8; 32]), PackageDownloadError> {
    let mut digest_context = Context::new(&SHA256);
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    let mut actual_size_bytes = 0_u64;
    loop {
        let bytes_read = package_file
            .read(&mut buffer)
            .map_err(|_| PackageDownloadError::StagingRead)?;
        if bytes_read == 0 {
            break;
        }
        actual_size_bytes = actual_size_bytes
            .checked_add(bytes_read as u64)
            .ok_or(PackageDownloadError::Size)?;
        if actual_size_bytes > MAX_PACKAGE_BYTES {
            return Err(PackageDownloadError::Size);
        }
        digest_context.update(&buffer[..bytes_read]);
    }
    Ok((actual_size_bytes, finish_sha256(digest_context)))
}

fn map_signature_error(error: SignatureVerificationError) -> PackageDownloadError {
    match error {
        SignatureVerificationError::KeyIdMalformed
        | SignatureVerificationError::KeyUnknown
        | SignatureVerificationError::KeyRetired
        | SignatureVerificationError::KeyRevoked => PackageDownloadError::SignatureKey,
        SignatureVerificationError::ProofEncoding
        | SignatureVerificationError::CanonicalPayload => PackageDownloadError::SignatureProof,
        SignatureVerificationError::ProofMismatch => PackageDownloadError::Signature,
        SignatureVerificationError::PackageDigestMismatch => PackageDownloadError::Digest,
        SignatureVerificationError::PackageSizeMismatch => PackageDownloadError::Size,
    }
}

fn validate_selected_release(
    selected_release: &UpdateRelease,
) -> Result<[u8; 32], PackageDownloadError> {
    if selected_release.artifact.format != TARGET_ARTIFACT_FORMAT {
        return Err(PackageDownloadError::Network);
    }
    let url = Url::parse(selected_release.artifact.url.as_str())
        .map_err(|_| PackageDownloadError::Network)?;
    if !is_safe_artifact_url(&url) {
        return Err(PackageDownloadError::Network);
    }
    if selected_release.artifact.size_bytes == 0
        || selected_release.artifact.size_bytes > MAX_PACKAGE_BYTES
    {
        return Err(PackageDownloadError::Size);
    }
    decode_sha256(&selected_release.artifact.sha256).ok_or(PackageDownloadError::Digest)
}

fn ensure_staging_directories(
    staging_root: &Path,
) -> Result<(PathBuf, PathBuf), PackageDownloadError> {
    validate_staging_root(staging_root)?;
    let incoming = staging_root.join("incoming");
    let packages = staging_root.join("packages");
    ensure_child_directory(&incoming)?;
    ensure_child_directory(&packages)?;
    Ok((incoming, packages))
}

fn validate_staging_root(staging_root: &Path) -> Result<(), PackageDownloadError> {
    if !staging_root.is_absolute()
        || staging_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PackageDownloadError::StagingPath);
    }

    match fs::symlink_metadata(staging_root) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(PackageDownloadError::StagingPath);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(staging_root).map_err(|_| PackageDownloadError::StagingWrite)?;
            let metadata = fs::symlink_metadata(staging_root)
                .map_err(|_| PackageDownloadError::StagingRead)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(PackageDownloadError::StagingPath);
            }
        }
        Err(_) => return Err(PackageDownloadError::StagingRead),
    }
    Ok(())
}

fn ensure_child_directory(path: &Path) -> Result<(), PackageDownloadError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(PackageDownloadError::StagingPath);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(PackageDownloadError::StagingWrite),
            }
            let metadata =
                fs::symlink_metadata(path).map_err(|_| PackageDownloadError::StagingRead)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(PackageDownloadError::StagingPath);
            }
        }
        Err(_) => return Err(PackageDownloadError::StagingRead),
    }
    Ok(())
}

fn ensure_final_absent(path: &Path) -> Result<(), PackageDownloadError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(PackageDownloadError::StagingPath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PackageDownloadError::StagingRead),
    }
}

fn stream_package_bytes(
    body: &mut dyn Read,
    package_file: &mut File,
    expected_size_bytes: u64,
    body_timeout: Duration,
) -> Result<(u64, Context), PackageDownloadError> {
    let deadline = Instant::now()
        .checked_add(body_timeout)
        .ok_or(PackageDownloadError::Timeout)?;
    let mut digest_context = Context::new(&SHA256);
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    let mut actual_size_bytes = 0_u64;

    loop {
        if Instant::now() >= deadline {
            return Err(PackageDownloadError::Timeout);
        }
        let bytes_read = match body.read(&mut buffer) {
            Ok(bytes_read) => bytes_read,
            Err(error) => {
                return Err(if Instant::now() >= deadline {
                    PackageDownloadError::Timeout
                } else {
                    map_body_read_error(error)
                })
            }
        };
        if Instant::now() >= deadline {
            return Err(PackageDownloadError::Timeout);
        }
        if bytes_read == 0 {
            break;
        }
        actual_size_bytes = actual_size_bytes
            .checked_add(bytes_read as u64)
            .ok_or(PackageDownloadError::Size)?;
        if actual_size_bytes > MAX_PACKAGE_BYTES || actual_size_bytes > expected_size_bytes {
            return Err(PackageDownloadError::Size);
        }
        package_file
            .write_all(&buffer[..bytes_read])
            .map_err(|_| PackageDownloadError::StagingWrite)?;
        digest_context.update(&buffer[..bytes_read]);
    }

    Ok((actual_size_bytes, digest_context))
}

fn finish_sha256(context: Context) -> [u8; 32] {
    let digest = context.finish();
    let mut result = [0_u8; 32];
    result.copy_from_slice(digest.as_ref());
    result
}

fn cleanup_part(path: &Path, error: PackageDownloadError) -> PackageDownloadError {
    match fs::remove_file(path) {
        Ok(()) => error,
        Err(_) => PackageDownloadError::StagingWrite,
    }
}

fn validate_artifact_response(
    transport: &dyn ArtifactHttpTransport,
    request: &ArtifactHttpRequest,
    response: &ArtifactHttpResponse,
) -> Result<(), PackageDownloadError> {
    validate_redirect_trace(transport, request, response)?;
    if response.status != 200 {
        return Err(PackageDownloadError::HttpStatus);
    }
    if !is_allowed_content_type(
        response.content_type.as_deref(),
        request.accepted_content_types,
    ) {
        return Err(PackageDownloadError::ContentType);
    }
    if response
        .content_length
        .is_some_and(|length| length > MAX_PACKAGE_BYTES || length != request.expected_size_bytes)
    {
        return Err(PackageDownloadError::Size);
    }
    Ok(())
}

fn validate_redirect_trace(
    transport: &dyn ArtifactHttpTransport,
    request: &ArtifactHttpRequest,
    response: &ArtifactHttpResponse,
) -> Result<(), PackageDownloadError> {
    let mut current_url = Url::parse(&request.url).map_err(|_| PackageDownloadError::Redirect)?;
    if !is_safe_artifact_url(&current_url) {
        return Err(PackageDownloadError::Redirect);
    }
    transport
        .validate_url(&current_url)
        .map_err(map_artifact_http_error)?;
    if response.redirects.len() > request.max_redirects {
        return Err(PackageDownloadError::Redirect);
    }

    let mut visited = HashSet::new();
    visited.insert(current_url.to_string());
    for redirect in &response.redirects {
        if !is_redirect_status(redirect.status) {
            return Err(PackageDownloadError::Redirect);
        }
        let location = redirect
            .location
            .as_deref()
            .ok_or(PackageDownloadError::Redirect)?;
        let next_url = resolve_artifact_redirect(&current_url, location)
            .map_err(|_| PackageDownloadError::Redirect)?;
        transport
            .validate_url(&next_url)
            .map_err(map_artifact_http_error)?;
        if !visited.insert(next_url.to_string()) {
            return Err(PackageDownloadError::Redirect);
        }
        current_url = next_url;
    }

    let final_url = Url::parse(&response.final_url).map_err(|_| PackageDownloadError::Redirect)?;
    if !is_safe_artifact_url(&final_url) || final_url != current_url {
        return Err(PackageDownloadError::Redirect);
    }
    transport
        .validate_url(&final_url)
        .map_err(map_artifact_http_error)?;
    Ok(())
}

fn response_content_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn is_allowed_content_type(content_type: Option<&str>, allowed: &[&str]) -> bool {
    let Some(media_type) = content_type.and_then(|value| value.split(';').next()) else {
        return false;
    };
    let media_type = media_type.trim();
    !media_type.is_empty()
        && allowed
            .iter()
            .any(|candidate| media_type.eq_ignore_ascii_case(candidate))
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn is_safe_artifact_url(url: &Url) -> bool {
    is_safe_public_update_url(url) && validate_public_ip_literal(url).is_ok()
}

fn resolve_artifact_redirect(current_url: &Url, location: &str) -> Result<Url, ()> {
    if location.is_empty() || location.trim() != location || location.chars().any(char::is_control)
    {
        return Err(());
    }
    let next_url = current_url.join(location).map_err(|_| ())?;
    if !is_safe_artifact_url(&next_url) {
        return Err(());
    }
    Ok(next_url)
}

fn decode_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }

    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_hex_nibble(pair[0])?;
        let low = decode_hex_nibble(pair[1])?;
        digest[index] = (high << 4) | low;
    }
    Some(digest)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn map_artifact_http_error(error: ArtifactHttpError) -> PackageDownloadError {
    match error {
        ArtifactHttpError::Network | ArtifactHttpError::Internal => PackageDownloadError::Network,
        ArtifactHttpError::Timeout => PackageDownloadError::Timeout,
        ArtifactHttpError::Redirect => PackageDownloadError::Redirect,
        ArtifactHttpError::Size => PackageDownloadError::Size,
    }
}

fn map_reqwest_error(error: reqwest::Error) -> ArtifactHttpError {
    if error.is_timeout() {
        ArtifactHttpError::Timeout
    } else {
        ArtifactHttpError::Network
    }
}

fn map_reqwest_body_error(error: reqwest::Error) -> io::Error {
    let kind = if error.is_timeout() {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, "package response body read failed")
}

fn map_body_read_error(error: io::Error) -> PackageDownloadError {
    if error.kind() == io::ErrorKind::TimedOut {
        PackageDownloadError::Timeout
    } else {
        PackageDownloadError::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_manifest::{parse_manifest, UpdateManifest};
    use crate::update_provider::PublicAddressResolver;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::net::SocketAddr;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_URL: &str = "https://updates.example.test/package";
    const INVALID_ARCHIVE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test/desktop/fixtures/update-download/invalid-archive.bin"
    ));

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let counter = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("cuetrio-update-download-{timestamp}-{counter}"));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct FakeTransport {
        result: RefCell<Option<Result<ArtifactHttpResponse, ArtifactHttpError>>>,
        requests: RefCell<Vec<ArtifactHttpRequest>>,
    }

    impl FakeTransport {
        fn response(response: ArtifactHttpResponse) -> Self {
            Self {
                result: RefCell::new(Some(Ok(response))),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn error(error: ArtifactHttpError) -> Self {
            Self {
                result: RefCell::new(Some(Err(error))),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl ArtifactHttpTransport for FakeTransport {
        fn get(
            &self,
            request: ArtifactHttpRequest,
        ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
            self.requests.borrow_mut().push(request);
            self.result
                .borrow_mut()
                .take()
                .expect("fake transport called once")
        }
    }

    #[derive(Default)]
    struct FakeAddressResolver {
        results: HashMap<String, Result<Vec<SocketAddr>, ()>>,
    }

    impl FakeAddressResolver {
        fn addresses(mut self, host: &str, addresses: &[&str]) -> Self {
            self.results.insert(
                host.to_ascii_lowercase(),
                Ok(addresses
                    .iter()
                    .map(|address| address.parse().unwrap())
                    .collect()),
            );
            self
        }

        fn failure(mut self, host: &str) -> Self {
            self.results.insert(host.to_ascii_lowercase(), Err(()));
            self
        }
    }

    impl PublicAddressResolver for FakeAddressResolver {
        fn resolve(&self, host: &str, _port: u16) -> Result<Vec<SocketAddr>, ()> {
            self.results
                .get(&host.to_ascii_lowercase())
                .cloned()
                .unwrap_or(Err(()))
        }
    }

    struct AddressCheckingTransport {
        result: RefCell<Option<Result<ArtifactHttpResponse, ArtifactHttpError>>>,
        resolver: FakeAddressResolver,
        calls: Cell<u32>,
    }

    impl AddressCheckingTransport {
        fn response(response: ArtifactHttpResponse, resolver: FakeAddressResolver) -> Self {
            Self {
                result: RefCell::new(Some(Ok(response))),
                resolver,
                calls: Cell::new(0),
            }
        }

        fn error(error: ArtifactHttpError, resolver: FakeAddressResolver) -> Self {
            Self {
                result: RefCell::new(Some(Err(error))),
                resolver,
                calls: Cell::new(0),
            }
        }
    }

    impl ArtifactHttpTransport for AddressCheckingTransport {
        fn get(
            &self,
            _request: ArtifactHttpRequest,
        ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
            self.calls.set(self.calls.get() + 1);
            self.result
                .borrow_mut()
                .take()
                .expect("address checking transport called once")
        }

        fn validate_url(&self, url: &Url) -> Result<(), ArtifactHttpError> {
            validate_public_address(url, &self.resolver)
                .map(|_| ())
                .map_err(|_| ArtifactHttpError::Network)
        }
    }

    struct VirtualSlowReader {
        bytes: Vec<u8>,
        offset: usize,
        elapsed: Rc<Cell<Duration>>,
    }

    impl Read for VirtualSlowReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.elapsed
                .set(self.elapsed.get().saturating_add(Duration::from_secs(1)));
            if self.offset == self.bytes.len() {
                return Ok(0);
            }
            buffer[0] = self.bytes[self.offset];
            self.offset += 1;
            Ok(1)
        }
    }

    struct FailingReader {
        kind: io::ErrorKind,
    }

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(self.kind, "synthetic package body failure"))
        }
    }

    struct FakeVerifier {
        result: Result<(), SignatureVerificationError>,
        calls: RefCell<Vec<(String, u64, [u8; 32])>>,
    }

    impl FakeVerifier {
        fn success() -> Self {
            Self {
                result: Ok(()),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn failure() -> Self {
            Self {
                result: Err(SignatureVerificationError::ProofMismatch),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl ArtifactSignatureVerifier for FakeVerifier {
        fn verify_selected_package(
            &self,
            selected_release: &UpdateRelease,
            actual_size_bytes: u64,
            actual_sha256: [u8; 32],
        ) -> Result<(), SignatureVerificationError> {
            self.calls.borrow_mut().push((
                selected_release.artifact.artifact_id.clone(),
                actual_size_bytes,
                actual_sha256,
            ));
            self.result
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = finish_sha256({
            let mut context = Context::new(&SHA256);
            context.update(bytes);
            context
        });
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn manifest_and_release(bytes: &[u8]) -> (UpdateManifest, UpdateRelease) {
        manifest_and_release_at(bytes, TEST_URL)
    }

    fn manifest_and_release_at(bytes: &[u8], url: &str) -> (UpdateManifest, UpdateRelease) {
        let digest = sha256_hex(bytes);
        let manifest_json = serde_json::json!({
            "productId": "com.cornellmethod.notebook",
            "schemaVersion": 1,
            "releases": [{
                "channel": "stable",
                "version": "1.2.3",
                "architecture": "aarch64-apple-darwin",
                "minVersion": "14",
                "artifact": {
                    "artifactId": "opaque-artifact-id-without-path-use",
                    "format": "app-archive",
                    "url": url,
                    "sizeBytes": bytes.len(),
                    "sha256": digest,
                },
                "signature": {
                    "keyId": "opaque-key-id",
                    "proof": "opaque-proof",
                },
            }]
        });
        let manifest = parse_manifest(&manifest_json.to_string()).unwrap();
        let release = manifest.releases[0].clone();
        (manifest, release)
    }

    fn response(
        body: impl Into<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> ArtifactHttpResponse {
        ArtifactHttpResponse::from_reader(
            200,
            content_type,
            content_length,
            Cursor::new(body.into()),
            Vec::new(),
            TEST_URL,
        )
    }

    fn assert_error<T>(result: Result<T, PackageDownloadError>, code: &str) {
        let error = match result {
            Ok(_) => panic!("expected package download failure"),
            Err(error) => error,
        };
        assert_eq!(error.code(), code);
        assert_eq!(error.to_string(), code);
    }

    fn assert_no_part_or_final(root: &Path, digest: &str) {
        assert!(!root
            .join("incoming")
            .join(format!("{digest}.part"))
            .exists());
        assert!(!root
            .join("packages")
            .join(format!("{digest}.app.tar.gz"))
            .exists());
    }

    #[test]
    fn rejects_private_resolving_artifacts_before_transport_get() {
        for (host, addresses) in [
            ("private.example.test", vec!["10.0.0.1:443"]),
            (
                "mixed.example.test",
                vec!["93.184.216.34:443", "192.168.1.1:443"],
            ),
        ] {
            let bytes = b"raw bytes".to_vec();
            let artifact_url = format!("https://{host}/package");
            let (_manifest, release) = manifest_and_release_at(&bytes, &artifact_url);
            let resolver = FakeAddressResolver::default()
                .addresses("updates.example.test", &["93.184.216.34:443"])
                .addresses(host, &addresses);
            let transport = AddressCheckingTransport::error(ArtifactHttpError::Network, resolver);
            let verifier = FakeVerifier::success();
            let root = TestRoot::new();

            assert_error(
                download_and_verify_artifact(&release, &root.path, &transport, &verifier),
                "package-network",
            );
            assert_eq!(transport.calls.get(), 0);
            assert_no_part_or_final(&root.path, &release.artifact.sha256);
        }
    }

    #[test]
    fn classifies_resolve_failure_and_rejects_private_redirect_hops_without_details() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let request = ArtifactHttpRequest::fixed(&release);

        for (host, addresses) in [
            ("private.example.test", vec!["10.0.0.1:443"]),
            (
                "mixed.example.test",
                vec!["93.184.216.34:443", "192.168.1.1:443"],
            ),
        ] {
            let final_url = format!("https://{host}/package");
            let response = ArtifactHttpResponse::from_reader(
                200,
                Some("application/gzip"),
                Some(bytes.len() as u64),
                Cursor::new(bytes.clone()),
                vec![ArtifactRedirect {
                    status: 302,
                    location: Some(final_url.clone()),
                }],
                &final_url,
            );
            let resolver = FakeAddressResolver::default()
                .addresses("updates.example.test", &["93.184.216.34:443"])
                .addresses(host, &addresses);
            let transport = AddressCheckingTransport::error(ArtifactHttpError::Network, resolver);
            assert_error(
                validate_redirect_trace(&transport, &request, &response),
                "package-network",
            );
        }

        let resolver = FakeAddressResolver::default().failure("updates.example.test");
        let transport = AddressCheckingTransport::error(ArtifactHttpError::Network, resolver);
        let url = Url::parse(TEST_URL).unwrap();
        assert_eq!(
            transport.validate_url(&url),
            Err(ArtifactHttpError::Network)
        );
    }

    #[test]
    fn accepts_public_https_artifact_with_ipv4_and_ipv6_resolution() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let resolver = FakeAddressResolver::default().addresses(
            "updates.example.test",
            &["93.184.216.34:443", "[2001:4860:4860::8888]:443"],
        );
        let transport = AddressCheckingTransport::response(
            response(
                bytes.clone(),
                Some("application/gzip"),
                Some(bytes.len() as u64),
            ),
            resolver,
        );
        let verifier = FakeVerifier::success();
        let root = TestRoot::new();
        let archive = download_and_verify_artifact(&release, &root.path, &transport, &verifier)
            .expect("public resolved addresses should be accepted");
        assert_eq!(archive.archive.raw_size_bytes, bytes.len() as u64);
        assert_eq!(transport.calls.get(), 1);
    }

    #[test]
    fn reqwest_artifact_rejects_dns_failure_before_request() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let request = ArtifactHttpRequest::fixed(&release);
        let resolver = FakeAddressResolver::default().failure("updates.example.test");
        let transport = ReqwestArtifactHttpTransport::with_resolver(Arc::new(resolver)).unwrap();
        assert!(matches!(
            transport.get(request),
            Err(ArtifactHttpError::Network)
        ));
    }

    #[test]
    fn valid_raw_bytes_are_streamed_verified_and_published_as_relative_package() {
        let bytes = b"small raw package bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let digest = release.artifact.sha256.clone();
        let root = TestRoot::new();
        let transport = FakeTransport::response(response(
            bytes.clone(),
            Some("Application/GZIP; charset=binary"),
            Some(bytes.len() as u64),
        ));
        let verifier = FakeVerifier::success();

        let archive = download_and_verify_artifact(&release, &root.path, &transport, &verifier)
            .expect("valid package should be published");

        assert_eq!(
            archive.archive.relative_package_path,
            PathBuf::from("packages").join(format!("{digest}.app.tar.gz"))
        );
        assert!(!archive.archive.relative_package_path.is_absolute());
        assert_eq!(
            archive.archive.artifact_id,
            "opaque-artifact-id-without-path-use"
        );
        assert_eq!(archive.archive.raw_size_bytes, bytes.len() as u64);
        assert_eq!(archive.archive.raw_sha256, digest);
        assert_eq!(archive.archive.version, "1.2.3");
        assert_eq!(archive.archive.architecture, "aarch64-apple-darwin");
        assert_eq!(
            fs::read(root.path.join(&archive.archive.relative_package_path)).unwrap(),
            bytes
        );
        assert!(root.path.join("extract").exists() == false);
        let calls = verifier.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "opaque-artifact-id-without-path-use");
        assert_eq!(calls[0].1, bytes.len() as u64);
        assert_eq!(calls[0].2, decode_sha256(&digest).unwrap());
        assert_eq!(transport.requests.borrow().len(), 1);
        assert_eq!(transport.requests.borrow()[0].url, TEST_URL);
    }

    #[test]
    fn valid_package_can_progress_beyond_read_idle_timeout_without_body_timeout() {
        let bytes = b"0123456789abcdefghijklmnopqrstuvwxyz".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let elapsed = Rc::new(Cell::new(Duration::ZERO));
        let response = ArtifactHttpResponse::from_reader(
            200,
            Some("application/gzip"),
            None,
            VirtualSlowReader {
                bytes: bytes.clone(),
                offset: 0,
                elapsed: Rc::clone(&elapsed),
            },
            Vec::new(),
            TEST_URL,
        );
        let root = TestRoot::new();
        let transport = FakeTransport::response(response);
        let verifier = FakeVerifier::success();

        let archive = download_and_verify_artifact(&release, &root.path, &transport, &verifier)
            .expect("a progressing package body should not hit the read idle timeout");

        assert!(elapsed.get() > PACKAGE_READ_IDLE_TIMEOUT);
        assert!(transport.requests.borrow()[0].body_timeout > elapsed.get());
        assert_eq!(
            fs::read(root.path.join(archive.archive.relative_package_path)).unwrap(),
            bytes
        );
    }

    #[test]
    fn valid_cached_package_is_revalidated_without_a_second_transport_request() {
        let bytes = b"cached raw package bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let root = TestRoot::new();
        let packages = root.path.join("packages");
        fs::create_dir(&packages).unwrap();
        let final_path = packages.join(format!("{}.app.tar.gz", release.artifact.sha256));
        fs::write(&final_path, &bytes).unwrap();
        let verifier = FakeVerifier::success();

        let cached = revalidate_cached_artifact(&release, &root.path, &verifier).unwrap();
        let CachedArtifact::Verified(archive) = cached else {
            panic!("expected a verified cache hit");
        };
        assert_eq!(
            archive.archive.relative_package_path,
            PathBuf::from("packages").join(format!("{}.app.tar.gz", release.artifact.sha256))
        );
        assert_eq!(verifier.calls.borrow().len(), 1);
    }

    #[test]
    fn invalid_regular_cached_package_can_be_removed_but_is_not_trusted() {
        let bytes = b"cached raw package bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let root = TestRoot::new();
        let packages = root.path.join("packages");
        fs::create_dir(&packages).unwrap();
        let final_path = packages.join(format!("{}.app.tar.gz", release.artifact.sha256));
        fs::write(&final_path, b"cached raw package bytez").unwrap();
        let verifier = FakeVerifier::success();

        let cached = revalidate_cached_artifact(&release, &root.path, &verifier).unwrap();
        assert!(matches!(
            cached,
            CachedArtifact::Invalid {
                error: PackageDownloadError::Digest,
                removable: true
            }
        ));
        remove_invalid_cached_artifact(&release, &root.path).unwrap();
        assert!(!final_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cached_package_is_not_removable() {
        use std::os::unix::fs::symlink;

        let bytes = b"cached raw package bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let root = TestRoot::new();
        let packages = root.path.join("packages");
        fs::create_dir(&packages).unwrap();
        let target = root.path.join("outside-package");
        fs::write(&target, &bytes).unwrap();
        let final_path = packages.join(format!("{}.app.tar.gz", release.artifact.sha256));
        symlink(&target, &final_path).unwrap();
        let verifier = FakeVerifier::success();

        let cached = revalidate_cached_artifact(&release, &root.path, &verifier).unwrap();
        assert!(matches!(
            cached,
            CachedArtifact::Invalid {
                error: PackageDownloadError::StagingPath,
                removable: false
            }
        ));
        assert!(final_path.is_symlink());
        assert_eq!(
            remove_invalid_cached_artifact(&release, &root.path),
            Err(PackageDownloadError::StagingPath)
        );
    }

    #[test]
    fn invalid_gzip_or_tar_bytes_are_not_parsed_by_download_boundary() {
        assert_ne!(INVALID_ARCHIVE.get(0..2), Some([0x1f, 0x8b].as_slice()));
        let bytes = INVALID_ARCHIVE.to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let root = TestRoot::new();
        let transport = FakeTransport::response(response(
            bytes.clone(),
            Some("application/octet-stream"),
            Some(bytes.len() as u64),
        ));
        let verifier = FakeVerifier::success();

        let result = download_and_verify_artifact(&release, &root.path, &transport, &verifier);

        assert!(result.is_ok());
        assert_eq!(verifier.calls.borrow().len(), 1);
    }

    #[test]
    fn status_content_type_and_declared_size_fail_before_part_creation() {
        let bytes = b"raw bytes".to_vec();
        for (status, content_type, content_length, expected_code) in [
            (
                404,
                Some("application/gzip"),
                Some(bytes.len() as u64),
                "package-http-status",
            ),
            (200, None, Some(bytes.len() as u64), "package-content-type"),
            (
                200,
                Some("text/plain"),
                Some(bytes.len() as u64),
                "package-content-type",
            ),
            (
                200,
                Some("application/gzip"),
                Some((bytes.len() + 1) as u64),
                "package-size",
            ),
        ] {
            let (_manifest, release) = manifest_and_release(&bytes);
            let root = TestRoot::new();
            let mut response = response(bytes.clone(), content_type, content_length);
            response.status = status;
            let transport = FakeTransport::response(response);
            let verifier = FakeVerifier::success();

            assert_error(
                download_and_verify_artifact(&release, &root.path, &transport, &verifier),
                expected_code,
            );
            assert_no_part_or_final(&root.path, &release.artifact.sha256);
            assert!(verifier.calls.borrow().is_empty());
        }
    }

    #[test]
    fn streamed_size_digest_and_signature_failures_clean_up_part() {
        let expected_bytes = b"expected raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&expected_bytes);

        let cases = [
            (b"short".to_vec(), "package-size", false),
            (
                b"expected raw bytes plus extra".to_vec(),
                "package-size",
                false,
            ),
            (b"expected raw bytez".to_vec(), "package-digest", false),
        ];
        for (bytes, expected_code, verifier_should_be_called) in cases {
            let root = TestRoot::new();
            let transport =
                FakeTransport::response(response(bytes, Some("application/gzip"), None));
            let verifier = FakeVerifier::success();
            assert_error(
                download_and_verify_artifact(&release, &root.path, &transport, &verifier),
                expected_code,
            );
            assert_no_part_or_final(&root.path, &release.artifact.sha256);
            assert_eq!(
                !verifier.calls.borrow().is_empty(),
                verifier_should_be_called,
                "verifier call expectation for {expected_code}"
            );
        }

        let root = TestRoot::new();
        let transport = FakeTransport::response(response(
            expected_bytes.clone(),
            Some("application/gzip"),
            None,
        ));
        let verifier = FakeVerifier::failure();
        assert_error(
            download_and_verify_artifact(&release, &root.path, &transport, &verifier),
            "package-signature",
        );
        assert_no_part_or_final(&root.path, &release.artifact.sha256);
        assert_eq!(verifier.calls.borrow().len(), 1);
    }

    #[test]
    fn streamed_timeout_and_read_errors_clean_up_part() {
        let expected_bytes = b"expected raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&expected_bytes);

        for (kind, expected_code) in [
            (io::ErrorKind::TimedOut, "package-timeout"),
            (io::ErrorKind::ConnectionReset, "package-network"),
        ] {
            let root = TestRoot::new();
            let response = ArtifactHttpResponse::from_reader(
                200,
                Some("application/gzip"),
                None,
                FailingReader { kind },
                Vec::new(),
                TEST_URL,
            );
            let transport = FakeTransport::response(response);
            let verifier = FakeVerifier::success();

            assert_error(
                download_and_verify_artifact(&release, &root.path, &transport, &verifier),
                expected_code,
            );
            assert_no_part_or_final(&root.path, &release.artifact.sha256);
        }
    }

    #[test]
    fn body_deadline_is_enforced_before_reading_more_bytes() {
        let root = TestRoot::new();
        let part_path = root.path.join("body.part");
        let mut package_file = File::create(&part_path).unwrap();
        let mut body = Cursor::new(b"body bytes");
        let expected_size_bytes = body.get_ref().len() as u64;

        let result = stream_package_bytes(
            &mut body,
            &mut package_file,
            expected_size_bytes,
            Duration::ZERO,
        );

        assert!(matches!(result, Err(PackageDownloadError::Timeout)));
        assert_eq!(package_file.metadata().unwrap().len(), 0);
    }

    #[test]
    fn manifest_cap_is_checked_before_transport_request() {
        let bytes = b"raw bytes".to_vec();
        let (manifest, mut release) = manifest_and_release(&bytes);
        release.artifact.size_bytes = MAX_PACKAGE_BYTES + 1;
        let root = TestRoot::new();
        let transport = FakeTransport::error(ArtifactHttpError::Network);
        let verifier = FakeVerifier::success();

        assert_error(
            download_and_verify_artifact(&release, &root.path, &transport, &verifier),
            "package-size",
        );
        assert!(transport.requests.borrow().is_empty());
        assert!(manifest.releases[0].artifact.size_bytes < MAX_PACKAGE_BYTES);
    }

    #[test]
    fn redirects_are_https_only_bounded_and_loop_free() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        for (redirects, final_url) in [
            (
                vec![ArtifactRedirect {
                    status: 302,
                    location: Some("http://updates.example.test/package".to_string()),
                }],
                "http://updates.example.test/package",
            ),
            (
                vec![ArtifactRedirect {
                    status: 302,
                    location: Some(TEST_URL.to_string()),
                }],
                TEST_URL,
            ),
            (
                vec![ArtifactRedirect {
                    status: 302,
                    location: Some("https://updates.example.test/next\n".to_string()),
                }],
                "https://updates.example.test/next",
            ),
        ] {
            let root = TestRoot::new();
            let response = ArtifactHttpResponse::from_reader(
                200,
                Some("application/gzip"),
                Some(bytes.len() as u64),
                Cursor::new(bytes.clone()),
                redirects,
                final_url,
            );
            let transport = FakeTransport::response(response);
            let verifier = FakeVerifier::success();
            assert_error(
                download_and_verify_artifact(&release, &root.path, &transport, &verifier),
                "package-redirect",
            );
            assert_no_part_or_final(&root.path, &release.artifact.sha256);
        }

        let root = TestRoot::new();
        let response = ArtifactHttpResponse::from_reader(
            200,
            Some("application/gzip"),
            Some(bytes.len() as u64),
            Cursor::new(bytes),
            (0..=MAX_ARTIFACT_REDIRECT_HOPS)
                .map(|_| ArtifactRedirect {
                    status: 302,
                    location: Some("https://updates.example.test/next".to_string()),
                })
                .collect(),
            "https://updates.example.test/next",
        );
        let transport = FakeTransport::response(response);
        let verifier = FakeVerifier::success();
        assert_error(
            download_and_verify_artifact(&release, &root.path, &transport, &verifier),
            "package-redirect",
        );
    }

    #[test]
    fn accepts_query_free_https_redirect() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let next_url = "https://updates.example.test/next";
        let response = ArtifactHttpResponse::from_reader(
            200,
            Some("application/gzip"),
            Some(bytes.len() as u64),
            Cursor::new(bytes.clone()),
            vec![ArtifactRedirect {
                status: 302,
                location: Some(next_url.to_string()),
            }],
            next_url,
        );
        let transport = FakeTransport::response(response);
        let verifier = FakeVerifier::success();
        let root = TestRoot::new();

        assert!(download_and_verify_artifact(&release, &root.path, &transport, &verifier).is_ok());
    }

    #[test]
    fn rejects_signed_token_opaque_and_encoded_queries_in_redirects() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let request = ArtifactHttpRequest::fixed(&release);

        for query in [
            "?sig=opaque-signature",
            "?signature=opaque-signature",
            "?X-Amz-Signature=opaque-signature",
            "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=release%2F20260824%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260824T000000Z&X-Amz-Expires=300&X-Amz-SignedHeaders=host&X-Amz-Signature=opaque-signature",
            "?access_token=opaque-token",
            "?opaque=provider-value",
            "?opaque",
            "?%73ig=opaque-signature",
            "?",
        ] {
            let redirect_url = format!("{TEST_URL}{query}");
            let response = ArtifactHttpResponse::from_reader(
                200,
                Some("application/gzip"),
                Some(bytes.len() as u64),
                Cursor::new(bytes.clone()),
                vec![ArtifactRedirect {
                    status: 302,
                    location: Some(redirect_url.clone()),
                }],
                &redirect_url,
            );
            let transport = FakeTransport::error(ArtifactHttpError::Network);
            assert_error(
                validate_redirect_trace(&transport, &request, &response),
                "package-redirect",
            );
        }

        let redirect_url = format!("{TEST_URL}?opaque=provider-value");
        let response = ArtifactHttpResponse::from_reader(
            200,
            Some("application/gzip"),
            Some(bytes.len() as u64),
            Cursor::new(bytes),
            Vec::new(),
            &redirect_url,
        );
        let transport = FakeTransport::error(ArtifactHttpError::Network);
        assert_error(
            validate_redirect_trace(&transport, &request, &response),
            "package-redirect",
        );
    }

    #[test]
    fn transport_failures_are_sanitized() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        for (transport_error, code) in [
            (ArtifactHttpError::Network, "package-network"),
            (ArtifactHttpError::Timeout, "package-timeout"),
            (ArtifactHttpError::Redirect, "package-redirect"),
            (ArtifactHttpError::Size, "package-size"),
            (ArtifactHttpError::Internal, "package-network"),
        ] {
            let root = TestRoot::new();
            let transport = FakeTransport::error(transport_error);
            let verifier = FakeVerifier::success();
            assert_error(
                download_and_verify_artifact(&release, &root.path, &transport, &verifier),
                code,
            );
            assert_no_part_or_final(&root.path, &release.artifact.sha256);
        }
    }

    #[test]
    fn existing_final_package_is_a_collision_and_is_not_overwritten() {
        let bytes = b"new raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let root = TestRoot::new();
        let packages = root.path.join("packages");
        fs::create_dir(&packages).unwrap();
        let final_path = packages.join(format!("{}.app.tar.gz", release.artifact.sha256));
        fs::write(&final_path, b"existing package sentinel").unwrap();
        let transport =
            FakeTransport::response(response(bytes, Some("application/gzip"), Some(13)));
        let verifier = FakeVerifier::success();

        assert_error(
            download_and_verify_artifact(&release, &root.path, &transport, &verifier),
            "staging-path",
        );
        assert_eq!(fs::read(final_path).unwrap(), b"existing package sentinel");
        assert!(transport.requests.borrow().is_empty());
    }

    #[test]
    fn staging_root_rejects_traversal_and_symlink_roots() {
        let bytes = b"raw bytes".to_vec();
        let (_manifest, release) = manifest_and_release(&bytes);
        let transport = FakeTransport::response(response(
            bytes.clone(),
            Some("application/gzip"),
            Some(bytes.len() as u64),
        ));
        let verifier = FakeVerifier::success();
        let root = TestRoot::new();
        let traversal = root.path.join("..").join("outside");
        assert_error(
            download_and_verify_artifact(&release, &traversal, &transport, &verifier),
            "staging-path",
        );

        #[cfg(unix)]
        {
            let link = root.path.join("staging-link");
            std::os::unix::fs::symlink(&root.path, &link).unwrap();
            let transport =
                FakeTransport::response(response(bytes, Some("application/gzip"), Some(9)));
            assert_error(
                download_and_verify_artifact(&release, &link, &transport, &verifier),
                "staging-path",
            );
        }
    }
}

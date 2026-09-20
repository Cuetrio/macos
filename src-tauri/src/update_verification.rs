use std::fs;
use std::io;
use std::path::{Component, Path};

use serde::Serialize;

use crate::update_archive::{
    extract_verified_archive, revalidate_verified_archive, ArchiveExtractionError, ExtractedArchive,
};
use crate::update_bundle::{
    validate_extracted_app_bundle, BundleValidationError, VerifiedAppBundle,
};
use crate::update_download::{
    download_and_verify_artifact, remove_invalid_cached_artifact, revalidate_cached_artifact,
    ArtifactHttpTransport, ArtifactSignatureVerifier, ArtifactSignatureVerifierFactory,
    CachedArtifact, PackageDownloadError, VerifiedArchiveHandle,
};
use crate::update_manifest::{UpdateManifest, UpdateRelease, MANIFEST_PRODUCT_ID};
use crate::update_provider::{fetch_manifest, ManifestHttpTransport};
use crate::update_selection::{select_update, UpdateSelection};
use crate::update_signature::signed_release_identity_sha256;
use crate::update_signature::EmbeddedTrustedKeyStore;
use crate::update_state::{
    PendingUpdate, UpdateState, UpdateStateError, UpdateStateSnapshot, UpdateStateStore,
    UpdateStatus,
};
use crate::update_target::{UpdateTargetContext, UpdateTargetError};

const UPDATE_REVALIDATION: &str = "update-revalidation";
const UPDATE_DOWNLOAD: &str = "update-download";
const UPDATE_INTEGRITY: &str = "update-integrity";
const UPDATE_SIGNATURE_KEY: &str = "update-signature-key";
const UPDATE_SIGNATURE_PROOF: &str = "update-signature-proof";
const UPDATE_ARCHIVE: &str = "update-archive";
const UPDATE_BUNDLE: &str = "update-bundle";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VerifyPendingUpdateOutcome {
    Verified,
    NoPendingUpdate,
    NoUpdate,
    #[serde(rename = "update-candidate-changed")]
    CandidateChanged,
    Failed,
    Busy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct VerifyPendingUpdateResponse {
    pub(crate) outcome: VerifyPendingUpdateOutcome,
    pub(crate) state: UpdateStateSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VerifyPendingUpdateCommandCode {
    UpdateRevalidation,
    UpdateDownload,
    UpdateSignatureKey,
    UpdateState,
    StagingPath,
    StagingRead,
    StagingWrite,
    StagingRename,
    UpdateTargetAppVersionInvalid,
    UpdateTargetMacosCommandFailed,
    UpdateTargetMacosOutputInvalid,
    UpdateCommandWorkerFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum VerifyPendingUpdateCommandError {
    CommandError {
        code: VerifyPendingUpdateCommandCode,
    },
    StateError {
        code: VerifyPendingUpdateCommandCode,
    },
}

impl VerifyPendingUpdateCommandError {
    pub(crate) const fn from_target(error: UpdateTargetError) -> Self {
        let code = match error {
            UpdateTargetError::InvalidAppVersion => {
                VerifyPendingUpdateCommandCode::UpdateTargetAppVersionInvalid
            }
            UpdateTargetError::MacOsCommandFailed => {
                VerifyPendingUpdateCommandCode::UpdateTargetMacosCommandFailed
            }
            UpdateTargetError::MacOsOutputInvalid => {
                VerifyPendingUpdateCommandCode::UpdateTargetMacosOutputInvalid
            }
        };
        Self::CommandError { code }
    }

    pub(crate) const fn revalidation() -> Self {
        Self::CommandError {
            code: VerifyPendingUpdateCommandCode::UpdateRevalidation,
        }
    }

    pub(crate) const fn download() -> Self {
        Self::CommandError {
            code: VerifyPendingUpdateCommandCode::UpdateDownload,
        }
    }

    pub(crate) const fn signature_key() -> Self {
        Self::CommandError {
            code: VerifyPendingUpdateCommandCode::UpdateSignatureKey,
        }
    }

    pub(crate) const fn worker_failed() -> Self {
        Self::CommandError {
            code: VerifyPendingUpdateCommandCode::UpdateCommandWorkerFailed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateVerificationError {
    StateStorage,
    Staging(&'static str),
}

impl From<UpdateStateError> for UpdateVerificationError {
    fn from(_: UpdateStateError) -> Self {
        Self::StateStorage
    }
}

impl From<UpdateVerificationError> for VerifyPendingUpdateCommandError {
    fn from(error: UpdateVerificationError) -> Self {
        match error {
            UpdateVerificationError::StateStorage => Self::StateError {
                code: VerifyPendingUpdateCommandCode::UpdateState,
            },
            UpdateVerificationError::Staging(code) => Self::CommandError {
                code: match code {
                    "staging-path" => VerifyPendingUpdateCommandCode::StagingPath,
                    "staging-read" => VerifyPendingUpdateCommandCode::StagingRead,
                    "staging-write" => VerifyPendingUpdateCommandCode::StagingWrite,
                    "staging-rename" => VerifyPendingUpdateCommandCode::StagingRename,
                    _ => VerifyPendingUpdateCommandCode::UpdateState,
                },
            },
        }
    }
}

#[derive(Debug)]
pub(crate) enum VerifiedCandidateRevalidationError {
    Revalidation,
    CandidateChanged,
    Package(PackageDownloadError),
}

pub(crate) struct RevalidatedVerifiedCandidate {
    pub(crate) release: UpdateRelease,
    pub(crate) archive: VerifiedArchiveHandle,
}

pub(crate) fn revalidate_verified_candidate(
    candidate: &PendingUpdate,
    target_context: &UpdateTargetContext,
    staging_root: &Path,
    manifest_transport: &dyn ManifestHttpTransport,
    signature_verifier_factory: &dyn ArtifactSignatureVerifierFactory,
) -> Result<RevalidatedVerifiedCandidate, VerifiedCandidateRevalidationError> {
    let manifest = fetch_manifest(manifest_transport)
        .map_err(|_| VerifiedCandidateRevalidationError::Revalidation)?;
    let selection = select_update(
        &manifest,
        &target_context.current_app_version.to_string(),
        target_context.target_channel,
        target_context.target_architecture,
        &target_context.current_macos_version.to_string(),
    )
    .map_err(|_| VerifiedCandidateRevalidationError::Revalidation)?;
    let selected_release = match selection {
        UpdateSelection::NoUpdate => {
            return Err(VerifiedCandidateRevalidationError::CandidateChanged)
        }
        UpdateSelection::Selected(release) => release,
    };

    if !candidate_identity_matches_release(candidate, &manifest, selected_release) {
        return Err(VerifiedCandidateRevalidationError::CandidateChanged);
    }

    let signature_verifier = signature_verifier_factory.create(&manifest);
    let cached_artifact =
        revalidate_cached_artifact(selected_release, staging_root, signature_verifier.as_ref())
            .map_err(VerifiedCandidateRevalidationError::Package)?;
    let archive = match cached_artifact {
        CachedArtifact::Verified(archive) => archive,
        CachedArtifact::Missing => {
            return Err(VerifiedCandidateRevalidationError::Package(
                PackageDownloadError::StagingRead,
            ));
        }
        CachedArtifact::Invalid { error, .. } => {
            return Err(VerifiedCandidateRevalidationError::Package(error));
        }
    };

    Ok(RevalidatedVerifiedCandidate {
        release: selected_release.clone(),
        archive,
    })
}

pub(crate) struct UpdateVerificationCoordinator<'a> {
    state_store: &'a UpdateStateStore,
    target_context: &'a UpdateTargetContext,
    staging_root: &'a Path,
    manifest_transport: &'a dyn ManifestHttpTransport,
    artifact_transport: &'a dyn ArtifactHttpTransport,
    signature_verifier_factory: &'a dyn ArtifactSignatureVerifierFactory,
}

impl<'a> UpdateVerificationCoordinator<'a> {
    pub(crate) fn new(
        state_store: &'a UpdateStateStore,
        target_context: &'a UpdateTargetContext,
        staging_root: &'a Path,
        manifest_transport: &'a dyn ManifestHttpTransport,
        artifact_transport: &'a dyn ArtifactHttpTransport,
        signature_verifier_factory: &'a dyn ArtifactSignatureVerifierFactory,
    ) -> Self {
        Self {
            state_store,
            target_context,
            staging_root,
            manifest_transport,
            artifact_transport,
            signature_verifier_factory,
        }
    }

    pub(crate) fn run(
        &self,
        now: u64,
    ) -> Result<VerifyPendingUpdateOutcome, UpdateVerificationError> {
        let Some(_operation) = self.state_store.try_acquire_operation()? else {
            return Ok(VerifyPendingUpdateOutcome::Busy);
        };
        let initial_state = self.state_store.snapshot();
        if initial_state.status == UpdateStatus::Checking {
            return Ok(VerifyPendingUpdateOutcome::Busy);
        }
        let Some(initial_candidate) = initial_state.pending_update.clone() else {
            recover_staging_artifacts(self.staging_root)
                .map_err(UpdateVerificationError::Staging)?;
            return Ok(VerifyPendingUpdateOutcome::NoPendingUpdate);
        };
        if initial_state.status != UpdateStatus::Available {
            recover_staging_artifacts(self.staging_root)
                .map_err(UpdateVerificationError::Staging)?;
            return Ok(VerifyPendingUpdateOutcome::NoPendingUpdate);
        }

        let manifest = match fetch_manifest(self.manifest_transport) {
            Ok(manifest) => manifest,
            Err(_) => return self.fail(UPDATE_REVALIDATION, now),
        };
        let selection = match select_update(
            &manifest,
            &self.target_context.current_app_version.to_string(),
            self.target_context.target_channel,
            self.target_context.target_architecture,
            &self.target_context.current_macos_version.to_string(),
        ) {
            Ok(selection) => selection,
            Err(_) => return self.fail(UPDATE_REVALIDATION, now),
        };

        let selected_release = match selection {
            UpdateSelection::NoUpdate => {
                self.state_store.record_revalidated_no_update()?;
                return Ok(VerifyPendingUpdateOutcome::NoUpdate);
            }
            UpdateSelection::Selected(release) => release,
        };
        let fresh_candidate = match pending_from_release(&manifest, selected_release, now) {
            Ok(candidate) => candidate,
            Err(_) => return self.fail(UPDATE_REVALIDATION, now),
        };

        let active_candidate = self.state_store.snapshot().pending_update;
        let Some(active_candidate) = active_candidate else {
            return self.fail(UPDATE_REVALIDATION, now);
        };
        let candidate_matches_selected_release =
            candidate_identity_matches_release(&initial_candidate, &manifest, selected_release);
        if !candidate_matches_selected_release
            || !active_candidate.candidate_identity_matches(&initial_candidate)
        {
            self.state_store
                .replace_available_candidate(fresh_candidate)?;
            return Ok(VerifyPendingUpdateOutcome::CandidateChanged);
        }

        if self
            .state_store
            .begin_package_verification(&initial_candidate, now)
            .is_err()
        {
            if self.state_store.snapshot().status == UpdateStatus::Checking {
                return Ok(VerifyPendingUpdateOutcome::Busy);
            }
            return Err(UpdateVerificationError::StateStorage);
        }

        if let Err(code) = recover_staging_artifacts(self.staging_root) {
            return self.fail(code, now);
        }

        let signature_verifier = self.signature_verifier_factory.create(&manifest);
        let cached_artifact = match revalidate_cached_artifact(
            selected_release,
            self.staging_root,
            signature_verifier.as_ref(),
        ) {
            Ok(cached_artifact) => cached_artifact,
            Err(error) => return self.fail(map_package_error(error), now),
        };
        let verified_archive_result: Result<VerifiedArchiveHandle, &'static str> =
            match cached_artifact {
                CachedArtifact::Missing => download_and_verify_artifact(
                    selected_release,
                    self.staging_root,
                    self.artifact_transport,
                    signature_verifier.as_ref(),
                )
                .map_err(map_package_error),
                CachedArtifact::Verified(verified_archive) => Ok(verified_archive),
                CachedArtifact::Invalid { error, removable } => {
                    if !removable {
                        Err(map_package_error(error))
                    } else {
                        match remove_invalid_cached_artifact(selected_release, self.staging_root) {
                            Ok(()) => download_and_verify_artifact(
                                selected_release,
                                self.staging_root,
                                self.artifact_transport,
                                signature_verifier.as_ref(),
                            )
                            .map_err(map_package_error),
                            Err(error) => Err(map_package_error(error)),
                        }
                    }
                }
            };
        let verified_archive = match verified_archive_result {
            Ok(verified_archive) => verified_archive,
            Err(code) => return self.fail(code, now),
        };
        self.state_store
            .record_package_checkpoint(&verified_archive.archive)?;

        let extracted_archive = match extract_or_revalidate(&verified_archive, self.staging_root) {
            Ok(extracted_archive) => extracted_archive,
            Err(code) => return self.fail(code, now),
        };
        self.state_store
            .record_extraction_checkpoint(&extracted_archive)?;

        let verified_bundle =
            match validate_extracted_app_bundle(&extracted_archive, self.staging_root) {
                Ok(verified_bundle) => verified_bundle,
                Err(error) => {
                    let code = match cleanup_ready_directory(
                        self.staging_root,
                        &verified_archive.archive.raw_sha256,
                    ) {
                        Ok(()) => map_bundle_error(error),
                        Err(code) => code,
                    };
                    return self.fail(code, now);
                }
            };
        if !bundle_matches_candidate(&verified_bundle, &extracted_archive, selected_release) {
            let code =
                cleanup_ready_directory(self.staging_root, &verified_archive.archive.raw_sha256)
                    .err()
                    .unwrap_or(UPDATE_BUNDLE);
            return self.fail(code, now);
        }

        if let Err(error) = revalidate_verified_archive(&verified_archive, self.staging_root) {
            let code = match cleanup_ready_directory(
                self.staging_root,
                &verified_archive.archive.raw_sha256,
            ) {
                Ok(()) => map_archive_error(error),
                Err(code) => code,
            };
            return self.fail(code, now);
        }

        self.state_store.record_verified(
            &verified_archive.archive,
            &extracted_archive,
            &verified_bundle,
            now,
        )?;
        Ok(VerifyPendingUpdateOutcome::Verified)
    }

    fn fail(
        &self,
        code: &'static str,
        now: u64,
    ) -> Result<VerifyPendingUpdateOutcome, UpdateVerificationError> {
        self.state_store.record_verification_failure(code, now)?;
        Ok(VerifyPendingUpdateOutcome::Failed)
    }
}

pub(crate) fn verify_pending_update_response(
    outcome: VerifyPendingUpdateOutcome,
    state: &UpdateState,
) -> VerifyPendingUpdateResponse {
    VerifyPendingUpdateResponse {
        outcome,
        state: UpdateStateSnapshot::from(state),
    }
}

pub(crate) fn verify_pending_update_worker(
    state_store: &UpdateStateStore,
    target_context: &UpdateTargetContext,
    staging_root: &Path,
    manifest_transport: &dyn ManifestHttpTransport,
    artifact_transport: &dyn ArtifactHttpTransport,
    signature_verifier_factory: &dyn ArtifactSignatureVerifierFactory,
    now: u64,
) -> Result<VerifyPendingUpdateResponse, UpdateVerificationError> {
    let coordinator = UpdateVerificationCoordinator::new(
        state_store,
        target_context,
        staging_root,
        manifest_transport,
        artifact_transport,
        signature_verifier_factory,
    );
    let outcome = coordinator.run(now)?;
    Ok(verify_pending_update_response(
        outcome,
        &state_store.snapshot(),
    ))
}

fn pending_from_release(
    manifest: &UpdateManifest,
    release: &UpdateRelease,
    discovered_at: u64,
) -> Result<PendingUpdate, UpdateStateError> {
    let signed_identity_sha256 = signed_release_identity_sha256(manifest, release)
        .map_err(|error| UpdateStateError::Invalid(error.to_string()))?;
    PendingUpdate::new_with_signed_identity(
        release.version.to_string(),
        release.channel.clone(),
        release.architecture.clone(),
        release.artifact.artifact_id.clone(),
        release.artifact.size_bytes,
        release.artifact.sha256.clone(),
        release.signature.key_id.clone(),
        signed_identity_sha256,
        discovered_at,
    )
}

fn candidate_identity_matches_release(
    candidate: &PendingUpdate,
    manifest: &UpdateManifest,
    release: &UpdateRelease,
) -> bool {
    pending_from_release(manifest, release, 0)
        .ok()
        .is_some_and(|fresh_candidate| candidate.candidate_identity_matches(&fresh_candidate))
}

fn bundle_matches_candidate(
    verified_bundle: &VerifiedAppBundle,
    extracted_archive: &ExtractedArchive,
    selected_release: &UpdateRelease,
) -> bool {
    verified_bundle.relative_app_path == extracted_archive.relative_app_path
        && verified_bundle.bundle_identifier == MANIFEST_PRODUCT_ID
        && verified_bundle.version == selected_release.version.to_string()
        && verified_bundle.architecture == selected_release.architecture
        && extracted_archive.artifact_id == selected_release.artifact.artifact_id
        && extracted_archive.raw_sha256 == selected_release.artifact.sha256
}

fn extract_or_revalidate(
    verified_archive: &VerifiedArchiveHandle,
    staging_root: &Path,
) -> Result<ExtractedArchive, &'static str> {
    let digest = verified_archive.archive.raw_sha256.as_str();
    let ready_directory = staging_root.join("extract").join(digest);
    match fs::symlink_metadata(&ready_directory) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("staging-path");
            }
            cleanup_ready_directory(staging_root, digest)?;
            extract_verified_archive(verified_archive, staging_root).map_err(map_archive_error)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            extract_verified_archive(verified_archive, staging_root).map_err(map_archive_error)
        }
        Err(_) => Err("staging-read"),
    }
}

fn cleanup_ready_directory(staging_root: &Path, digest: &str) -> Result<(), &'static str> {
    validate_staging_root(staging_root)?;
    if !is_lower_hex_digest(digest) {
        return Err("staging-path");
    }
    let extract_directory = staging_root.join("extract");
    let extract_metadata = match fs::symlink_metadata(&extract_directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("staging-read"),
    };
    if extract_metadata.file_type().is_symlink() || !extract_metadata.is_dir() {
        return Err("staging-path");
    }
    let ready_directory = extract_directory.join(digest);
    let ready_metadata = match fs::symlink_metadata(&ready_directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("staging-read"),
    };
    if ready_metadata.file_type().is_symlink() || !ready_metadata.is_dir() {
        return Err("staging-path");
    }
    fs::remove_dir_all(ready_directory).map_err(|_| "staging-write")
}

fn recover_staging_artifacts(staging_root: &Path) -> Result<(), &'static str> {
    validate_staging_root(staging_root)?;
    recover_incoming_parts(staging_root)?;
    recover_extract_temps(staging_root)
}

fn recover_incoming_parts(staging_root: &Path) -> Result<(), &'static str> {
    let incoming = staging_root.join("incoming");
    let metadata = match fs::symlink_metadata(&incoming) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("staging-read"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("staging-path");
    }
    for entry in fs::read_dir(&incoming).map_err(|_| "staging-read")? {
        let entry = entry.map_err(|_| "staging-read")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err("staging-path");
        };
        if !name.ends_with(".part") {
            return Err("staging-path");
        }
        let digest = name.strip_suffix(".part").unwrap_or_default();
        if !is_lower_hex_digest(digest) {
            return Err("staging-path");
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "staging-read")?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("staging-path");
        }
        fs::remove_file(entry.path()).map_err(|_| "staging-write")?;
    }
    Ok(())
}

fn recover_extract_temps(staging_root: &Path) -> Result<(), &'static str> {
    let extract = staging_root.join("extract");
    let metadata = match fs::symlink_metadata(&extract) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("staging-read"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("staging-path");
    }
    for entry in fs::read_dir(&extract).map_err(|_| "staging-read")? {
        let entry = entry.map_err(|_| "staging-read")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err("staging-path");
        };
        if name.ends_with(".tmp") {
            let digest = name.strip_suffix(".tmp").unwrap_or_default();
            if !is_lower_hex_digest(digest) {
                return Err("staging-path");
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "staging-read")?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("staging-path");
            }
            fs::remove_dir_all(entry.path()).map_err(|_| "staging-write")?;
        } else if !is_lower_hex_digest(name) {
            return Err("staging-path");
        } else {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "staging-read")?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("staging-path");
            }
        }
    }
    Ok(())
}

fn validate_staging_root(staging_root: &Path) -> Result<(), &'static str> {
    let Some(parent) = staging_root.parent() else {
        return Err("staging-path");
    };
    if !staging_root.is_absolute()
        || staging_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("staging-path");
    }
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("staging-path");
        }
        Ok(_) => {}
        Err(_) => return Err("staging-read"),
    }
    match fs::symlink_metadata(staging_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err("staging-path")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(staging_root).map_err(|_| "staging-write")?;
            let metadata = fs::symlink_metadata(staging_root).map_err(|_| "staging-read")?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                Err("staging-path")
            } else {
                Ok(())
            }
        }
        Err(_) => Err("staging-read"),
    }
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn map_package_error(error: PackageDownloadError) -> &'static str {
    match error {
        PackageDownloadError::Network
        | PackageDownloadError::Timeout
        | PackageDownloadError::HttpStatus
        | PackageDownloadError::Redirect
        | PackageDownloadError::ContentType => UPDATE_DOWNLOAD,
        PackageDownloadError::Size | PackageDownloadError::Digest => UPDATE_INTEGRITY,
        PackageDownloadError::SignatureKey => UPDATE_SIGNATURE_KEY,
        PackageDownloadError::Signature | PackageDownloadError::SignatureProof => {
            UPDATE_SIGNATURE_PROOF
        }
        PackageDownloadError::StagingPath => "staging-path",
        PackageDownloadError::StagingRead => "staging-read",
        PackageDownloadError::StagingWrite => "staging-write",
        PackageDownloadError::StagingRename => "staging-rename",
    }
}

fn map_archive_error(error: ArchiveExtractionError) -> &'static str {
    match error {
        ArchiveExtractionError::StagingPath => "staging-path",
        ArchiveExtractionError::StagingRead => "staging-read",
        ArchiveExtractionError::StagingWrite => "staging-write",
        ArchiveExtractionError::StagingRename => "staging-rename",
        ArchiveExtractionError::ArchiveGzip
        | ArchiveExtractionError::ArchiveFormatUnsupported
        | ArchiveExtractionError::ArchiveTar
        | ArchiveExtractionError::ArchiveTrailingData
        | ArchiveExtractionError::ArchivePath
        | ArchiveExtractionError::ArchiveRoot
        | ArchiveExtractionError::ArchiveLimit
        | ArchiveExtractionError::ArchiveSymlink
        | ArchiveExtractionError::ArchiveSpecialFile
        | ArchiveExtractionError::ArchivePermission
        | ArchiveExtractionError::ArchiveTree => UPDATE_ARCHIVE,
    }
}

fn map_bundle_error(error: BundleValidationError) -> &'static str {
    match error {
        BundleValidationError::StagingPath => "staging-path",
        BundleValidationError::StagingRead => "staging-read",
        BundleValidationError::BundleLayout
        | BundleValidationError::BundlePlist
        | BundleValidationError::BundleIdentity
        | BundleValidationError::BundleVersion
        | BundleValidationError::BundleExecutable
        | BundleValidationError::BundleArchitecture => UPDATE_BUNDLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_download::{
        ArtifactHttpError, ArtifactHttpRequest, ArtifactHttpResponse, VerifiedArchive,
    };
    use crate::update_manifest::{MacOsVersion, SemVer, TARGET_ARCHITECTURE, TARGET_CHANNEL};
    use crate::update_provider::{
        ManifestHttpError, ManifestHttpRequest, ManifestHttpResponse, GITHUB_RELEASES_MANIFEST_URL,
    };
    use crate::update_signature::SignatureVerificationError;
    use crate::update_state::VerificationState;
    use std::cell::Cell;
    use std::fs;
    use std::io::{self, Cursor, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use flate2::{write::GzEncoder, Compression};
    use ring::digest::{digest, SHA256};
    use tar::{Builder, EntryType, Header};

    static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_directory(label: &str) -> PathBuf {
        let counter = TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "cuetrio-update-verification-{label}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    struct FakeManifestTransport {
        response: ManifestHttpResponse,
        calls: Cell<u32>,
    }

    impl ManifestHttpTransport for FakeManifestTransport {
        fn get(
            &self,
            request: ManifestHttpRequest,
        ) -> Result<ManifestHttpResponse, ManifestHttpError> {
            assert_eq!(request.url, GITHUB_RELEASES_MANIFEST_URL);
            self.calls.set(self.calls.get() + 1);
            Ok(self.response.clone())
        }
    }

    struct CountingArtifactTransport {
        calls: Cell<u32>,
    }

    impl ArtifactHttpTransport for CountingArtifactTransport {
        fn get(
            &self,
            _request: ArtifactHttpRequest,
        ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
            self.calls.set(self.calls.get() + 1);
            Err(ArtifactHttpError::Network)
        }
    }

    struct ByteArtifactTransport {
        bytes: Vec<u8>,
        calls: Cell<u32>,
    }

    impl ArtifactHttpTransport for ByteArtifactTransport {
        fn get(
            &self,
            request: ArtifactHttpRequest,
        ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
            self.calls.set(self.calls.get() + 1);
            Ok(ArtifactHttpResponse {
                status: 200,
                content_type: Some("application/gzip".to_string()),
                content_length: Some(self.bytes.len() as u64),
                body: Box::new(Cursor::new(self.bytes.clone())),
                redirects: Vec::new(),
                final_url: request.url,
            })
        }
    }

    struct AcceptingSignatureVerifier;

    impl ArtifactSignatureVerifier for AcceptingSignatureVerifier {
        fn verify_selected_package(
            &self,
            _selected_release: &UpdateRelease,
            _actual_size_bytes: u64,
            _actual_sha256: [u8; 32],
        ) -> Result<(), SignatureVerificationError> {
            Ok(())
        }
    }

    struct AcceptingSignatureVerifierFactory {
        calls: Cell<u32>,
    }

    impl ArtifactSignatureVerifierFactory for AcceptingSignatureVerifierFactory {
        fn create<'a>(
            &'a self,
            _manifest_root: &crate::update_manifest::UpdateManifest,
        ) -> Box<dyn ArtifactSignatureVerifier + 'a> {
            self.calls.set(self.calls.get() + 1);
            Box::new(AcceptingSignatureVerifier)
        }
    }

    struct ReplacingSignatureVerifierFactory {
        package_path: PathBuf,
        replacement: Vec<u8>,
    }

    struct ReplacingSignatureVerifier {
        package_path: PathBuf,
        replacement: Vec<u8>,
    }

    impl ArtifactSignatureVerifier for ReplacingSignatureVerifier {
        fn verify_selected_package(
            &self,
            _selected_release: &UpdateRelease,
            _actual_size_bytes: u64,
            _actual_sha256: [u8; 32],
        ) -> Result<(), SignatureVerificationError> {
            let moved_path = self.package_path.with_extension("verified");
            fs::rename(&self.package_path, &moved_path).unwrap();
            fs::write(&self.package_path, &self.replacement).unwrap();
            Ok(())
        }
    }

    impl ArtifactSignatureVerifierFactory for ReplacingSignatureVerifierFactory {
        fn create<'a>(
            &'a self,
            _manifest_root: &crate::update_manifest::UpdateManifest,
        ) -> Box<dyn ArtifactSignatureVerifier + 'a> {
            Box::new(ReplacingSignatureVerifier {
                package_path: self.package_path.clone(),
                replacement: self.replacement.clone(),
            })
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        digest(&SHA256, bytes)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn append_archive_directory<W: Write>(builder: &mut Builder<W>, path: &str) {
        let mut header = Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_entry_type(EntryType::dir());
        header.set_mode(0o755);
        header.set_size(0);
        builder.append_data(&mut header, path, io::empty()).unwrap();
    }

    fn append_archive_file<W: Write>(
        builder: &mut Builder<W>,
        path: &str,
        mode: u32,
        contents: &[u8],
    ) {
        let mut header = Header::new_gnu();
        header.set_path(path).unwrap();
        header.set_entry_type(EntryType::Regular);
        header.set_mode(mode);
        header.set_size(contents.len() as u64);
        builder.append_data(&mut header, path, contents).unwrap();
    }

    fn valid_update_archive(version: &str) -> Vec<u8> {
        const ROOT: &str = "Cuetrio.app";
        const EXECUTABLE: &str = "CornellMethodNotebook";
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{MANIFEST_PRODUCT_ID}</string><key>CFBundleShortVersionString</key><string>{version}</string><key>CFBundleExecutable</key><string>{EXECUTABLE}</string></dict></plist>"
        );
        let mut macho = Vec::new();
        macho.extend_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        macho.extend_from_slice(&0x0100_000c_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&2_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = Builder::new(&mut encoder);
            append_archive_directory(&mut builder, &format!("{ROOT}/"));
            append_archive_directory(&mut builder, &format!("{ROOT}/Contents/"));
            append_archive_directory(&mut builder, &format!("{ROOT}/Contents/MacOS/"));
            append_archive_directory(&mut builder, &format!("{ROOT}/Contents/Resources/"));
            append_archive_file(
                &mut builder,
                &format!("{ROOT}/Contents/Info.plist"),
                0o644,
                plist.as_bytes(),
            );
            append_archive_file(
                &mut builder,
                &format!("{ROOT}/Contents/MacOS/{EXECUTABLE}"),
                0o755,
                &macho,
            );
            append_archive_file(
                &mut builder,
                &format!("{ROOT}/Contents/Resources/config.js"),
                0o644,
                b"archive resource",
            );
            builder.finish().unwrap();
        }
        encoder.finish().unwrap()
    }

    fn manifest_response(version: &str, artifact_id: &str) -> ManifestHttpResponse {
        manifest_response_for(
            version,
            artifact_id,
            4,
            "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
        )
    }

    fn manifest_response_for(
        version: &str,
        artifact_id: &str,
        size_bytes: u64,
        sha256: &str,
    ) -> ManifestHttpResponse {
        manifest_response_with_signed_fields(
            version,
            artifact_id,
            size_bytes,
            sha256,
            "14",
            None,
            "opaque-proof",
        )
    }

    fn manifest_response_with_signed_fields(
        version: &str,
        artifact_id: &str,
        size_bytes: u64,
        sha256: &str,
        min_version: &str,
        max_version_exclusive: Option<&str>,
        proof: &str,
    ) -> ManifestHttpResponse {
        let max_version_field = max_version_exclusive
            .map(|value| format!(r#""maxVersionExclusive":"{value}","#))
            .unwrap_or_default();
        let body = format!(
            r#"{{
              "productId":"com.cornellmethod.notebook",
              "schemaVersion":1,
              "releases":[{{
                "channel":"stable",
                "version":"{version}",
                "architecture":"aarch64-apple-darwin",
                "minVersion":"{min_version}",
                {max_version_field}
                "artifact":{{
                  "artifactId":"{artifact_id}",
                  "format":"app-archive",
                  "url":"https://updates.example.test/{artifact_id}",
                  "sizeBytes":{size_bytes},
                  "sha256":"{sha256}"
                }},
                "signature":{{"keyId":"test-key","proof":"{proof}"}}
              }}]
            }}"#
        );
        ManifestHttpResponse {
            status: 200,
            content_type: Some("application/json".to_string()),
            content_length: Some(body.len() as u64),
            body: body.into_bytes(),
            redirects: Vec::new(),
            final_url: GITHUB_RELEASES_MANIFEST_URL.to_string(),
        }
    }

    fn pending_for_manifest_response(
        response: &ManifestHttpResponse,
        discovered_at: u64,
    ) -> PendingUpdate {
        let manifest = crate::update_manifest::parse_manifest(
            std::str::from_utf8(&response.body).expect("manifest fixture must be UTF-8"),
        )
        .unwrap();
        pending_from_release(&manifest, &manifest.releases[0], discovered_at).unwrap()
    }

    fn target_context() -> UpdateTargetContext {
        UpdateTargetContext {
            current_app_version: SemVer::parse("1.0.0").unwrap(),
            target_channel: TARGET_CHANNEL,
            target_architecture: TARGET_ARCHITECTURE,
            current_macos_version: MacOsVersion::parse("14", "test macOS version").unwrap(),
        }
    }

    #[test]
    fn candidate_identity_compares_every_persisted_identity_field() {
        let manifest = crate::update_manifest::parse_manifest(
            r#"{
              "productId":"com.cornellmethod.notebook",
              "schemaVersion":1,
              "releases":[{
                "channel":"stable",
                "version":"1.2.3",
                "architecture":"aarch64-apple-darwin",
                "minVersion":"14",
                "artifact":{
                  "artifactId":"artifact",
                  "format":"app-archive",
                  "url":"https://updates.example.test/artifact",
                  "sizeBytes":4,
                  "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                },
                "signature":{"keyId":"test-key","proof":"opaque-proof"}
              }]
            }"#,
        )
        .unwrap();
        let release = &manifest.releases[0];
        let candidate = pending_from_release(&manifest, release, 1).unwrap();
        assert!(candidate_identity_matches_release(
            &candidate, &manifest, release
        ));

        for mutation in [
            |candidate: &mut PendingUpdate| candidate.version = "1.2.4".to_string(),
            |candidate: &mut PendingUpdate| candidate.channel = "beta".to_string(),
            |candidate: &mut PendingUpdate| {
                candidate.architecture = "x86_64-apple-darwin".to_string()
            },
            |candidate: &mut PendingUpdate| candidate.artifact = "other".to_string(),
            |candidate: &mut PendingUpdate| candidate.size_bytes = Some(5),
            |candidate: &mut PendingUpdate| {
                candidate.sha256 = Some(
                    "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd".to_string(),
                )
            },
            |candidate: &mut PendingUpdate| candidate.key_id = Some("other-key".to_string()),
        ] {
            let mut changed = candidate.clone();
            mutation(&mut changed);
            assert!(!candidate_identity_matches_release(
                &changed, &manifest, release
            ));
        }

        let mut range_changed = release.clone();
        range_changed.min_version = MacOsVersion::parse("15", "test min version").unwrap();
        assert!(!candidate_identity_matches_release(
            &candidate,
            &manifest,
            &range_changed
        ));

        let mut max_changed = release.clone();
        max_changed.max_version_exclusive =
            Some(MacOsVersion::parse("16", "test max version").unwrap());
        assert!(!candidate_identity_matches_release(
            &candidate,
            &manifest,
            &max_changed
        ));

        let mut proof_changed = release.clone();
        proof_changed.signature.proof = "different-proof".to_string();
        assert!(!candidate_identity_matches_release(
            &candidate,
            &manifest,
            &proof_changed
        ));
    }

    #[test]
    fn response_is_limited_to_outcome_and_ui_snapshot() {
        let response = verify_pending_update_response(
            VerifyPendingUpdateOutcome::Failed,
            &UpdateState::initial(),
        );
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 2);
        assert!(value["state"]["pendingUpdate"].is_null());
        assert!(value.get("url").is_none());
        assert!(value.get("proof").is_none());
    }

    #[test]
    fn full_pipeline_commits_verified_and_revalidates_same_artifact_cache() {
        let directory = test_directory("full-pipeline");
        let staging = directory.join("staging");
        let version = "1.3.0";
        let artifact_id = "full-pipeline-artifact";
        let package_bytes = valid_update_archive(version);
        let digest = sha256_hex(&package_bytes);
        let size_bytes = package_bytes.len() as u64;
        let manifest_response = manifest_response_for(version, artifact_id, size_bytes, &digest);
        let candidate = pending_for_manifest_response(&manifest_response, 10);
        let state = UpdateStateStore::load_or_default(&directory, &staging);
        state
            .begin_check(crate::update_state::CheckTrigger::Manual, 10)
            .unwrap();
        state.record_available(candidate).unwrap();

        let manifest = FakeManifestTransport {
            response: manifest_response,
            calls: Cell::new(0),
        };
        let artifact = ByteArtifactTransport {
            bytes: package_bytes,
            calls: Cell::new(0),
        };
        let signature_factory = AcceptingSignatureVerifierFactory {
            calls: Cell::new(0),
        };
        let target = target_context();
        let coordinator = UpdateVerificationCoordinator::new(
            &state,
            &target,
            &staging,
            &manifest,
            &artifact,
            &signature_factory,
        );

        assert_eq!(
            coordinator.run(20).unwrap(),
            VerifyPendingUpdateOutcome::Verified
        );
        let first_snapshot = state.snapshot();
        let first_pending = first_snapshot.pending_update.as_ref().unwrap();
        assert_eq!(first_snapshot.status, UpdateStatus::Available);
        assert_eq!(
            first_pending.verification_state,
            crate::update_state::VerificationState::Verified
        );
        assert_eq!(
            first_pending.package_path.as_deref(),
            Some(
                PathBuf::from("packages")
                    .join(format!("{digest}.app.tar.gz"))
                    .as_path()
            )
        );
        assert_eq!(
            first_pending.extracted_app_path.as_deref(),
            Some(
                PathBuf::from("extract")
                    .join(&digest)
                    .join("Cuetrio.app")
                    .as_path()
            )
        );
        assert_eq!(manifest.calls.get(), 1);
        assert_eq!(artifact.calls.get(), 1);
        assert_eq!(signature_factory.calls.get(), 1);
        assert!(staging
            .join("packages")
            .join(format!("{digest}.app.tar.gz"))
            .is_file());
        assert!(staging
            .join("extract")
            .join(&digest)
            .join("Cuetrio.app")
            .is_dir());
        let resource_path = staging
            .join("extract")
            .join(&digest)
            .join("Cuetrio.app/Contents/Resources/config.js");
        assert_eq!(fs::read(&resource_path).unwrap(), b"archive resource");
        assert!(!staging
            .join("incoming")
            .join(format!("{digest}.part"))
            .exists());
        assert!(!staging
            .join("extract")
            .join(format!("{digest}.tmp"))
            .exists());

        fs::write(&resource_path, b"tampered resource").unwrap();
        assert_eq!(
            coordinator.run(25).unwrap(),
            VerifyPendingUpdateOutcome::Verified
        );
        assert_eq!(artifact.calls.get(), 1);
        assert_eq!(fs::read(&resource_path).unwrap(), b"archive resource");
        assert!(!fs::symlink_metadata(&resource_path)
            .unwrap()
            .file_type()
            .is_symlink());

        #[cfg(unix)]
        {
            let external_resource = directory.join("external-resource.js");
            fs::write(&external_resource, b"external resource").unwrap();
            fs::remove_file(&resource_path).unwrap();
            std::os::unix::fs::symlink(&external_resource, &resource_path).unwrap();

            assert_eq!(
                coordinator.run(27).unwrap(),
                VerifyPendingUpdateOutcome::Verified
            );
            assert_eq!(artifact.calls.get(), 1);
            assert_eq!(fs::read(&resource_path).unwrap(), b"archive resource");
            assert!(!fs::symlink_metadata(&resource_path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read(&external_resource).unwrap(), b"external resource");
        }

        let verification_runs_before_corrupted_cache = if cfg!(unix) { 3 } else { 2 };

        let package_path = staging
            .join("packages")
            .join(format!("{digest}.app.tar.gz"));
        fs::write(&package_path, b"corrupted cache").unwrap();
        fs::remove_file(
            staging
                .join("extract")
                .join(&digest)
                .join("Cuetrio.app")
                .join("Contents/Info.plist"),
        )
        .unwrap();
        assert_eq!(
            coordinator.run(30).unwrap(),
            VerifyPendingUpdateOutcome::Verified
        );
        assert_eq!(
            manifest.calls.get(),
            verification_runs_before_corrupted_cache + 1
        );
        assert_eq!(artifact.calls.get(), 2);
        assert_eq!(
            signature_factory.calls.get(),
            verification_runs_before_corrupted_cache + 1
        );
        assert!(package_path.is_file());
        assert!(staging
            .join("extract")
            .join(&digest)
            .join("Cuetrio.app/Contents/Info.plist")
            .is_file());

        assert_eq!(
            coordinator.run(40).unwrap(),
            VerifyPendingUpdateOutcome::Verified
        );
        assert_eq!(
            manifest.calls.get(),
            verification_runs_before_corrupted_cache + 2
        );
        assert_eq!(artifact.calls.get(), 2);
        assert_eq!(
            signature_factory.calls.get(),
            verification_runs_before_corrupted_cache + 2
        );
        assert_eq!(
            state.snapshot().pending_update.unwrap().verification_state,
            crate::update_state::VerificationState::Verified
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn changed_signed_identity_does_not_reuse_verified_artifact_cache() {
        let directory = test_directory("signed-identity-cache");
        let staging = directory.join("staging");
        let version = "1.3.2";
        let artifact_id = "signed-identity-cache-artifact";
        let package_bytes = valid_update_archive(version);
        let digest = sha256_hex(&package_bytes);
        let size_bytes = package_bytes.len() as u64;
        let original_response = manifest_response_for(version, artifact_id, size_bytes, &digest);
        let candidate = pending_for_manifest_response(&original_response, 10);
        let state = UpdateStateStore::load_or_default(&directory, &staging);
        state
            .begin_check(crate::update_state::CheckTrigger::Manual, 10)
            .unwrap();
        state.record_available(candidate).unwrap();

        let original_manifest = FakeManifestTransport {
            response: original_response,
            calls: Cell::new(0),
        };
        let artifact = ByteArtifactTransport {
            bytes: package_bytes,
            calls: Cell::new(0),
        };
        let signature_factory = AcceptingSignatureVerifierFactory {
            calls: Cell::new(0),
        };
        let target = target_context();
        let coordinator = UpdateVerificationCoordinator::new(
            &state,
            &target,
            &staging,
            &original_manifest,
            &artifact,
            &signature_factory,
        );
        assert_eq!(
            coordinator.run(20).unwrap(),
            VerifyPendingUpdateOutcome::Verified
        );
        assert_eq!(artifact.calls.get(), 1);

        let changed_manifest = FakeManifestTransport {
            response: manifest_response_with_signed_fields(
                version,
                artifact_id,
                size_bytes,
                &digest,
                "14",
                Some("16"),
                "opaque-proof",
            ),
            calls: Cell::new(0),
        };
        let changed_coordinator = UpdateVerificationCoordinator::new(
            &state,
            &target,
            &staging,
            &changed_manifest,
            &artifact,
            &signature_factory,
        );
        assert_eq!(
            changed_coordinator.run(30).unwrap(),
            VerifyPendingUpdateOutcome::CandidateChanged
        );
        assert_eq!(artifact.calls.get(), 1);
        let pending = state.snapshot().pending_update.unwrap();
        assert_eq!(pending.verification_state, VerificationState::NotVerified);
        assert_eq!(pending.package_path, None);
        assert_eq!(pending.extracted_app_path, None);
        assert_eq!(pending.verified_at, None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn package_path_replacement_after_package_verification_fails_closed() {
        let directory = test_directory("package-path-replacement");
        let staging = directory.join("staging");
        let version = "1.3.1";
        let artifact_id = "package-path-replacement-artifact";
        let package_bytes = valid_update_archive(version);
        let digest = sha256_hex(&package_bytes);
        let size_bytes = package_bytes.len() as u64;
        let manifest_response = manifest_response_for(version, artifact_id, size_bytes, &digest);
        let candidate = pending_for_manifest_response(&manifest_response, 10);
        let state = UpdateStateStore::load_or_default(&directory, &staging);
        state
            .begin_check(crate::update_state::CheckTrigger::Manual, 10)
            .unwrap();
        state.record_available(candidate).unwrap();

        let package_path = staging
            .join("packages")
            .join(format!("{digest}.app.tar.gz"));
        fs::create_dir_all(package_path.parent().unwrap()).unwrap();
        fs::write(&package_path, &package_bytes).unwrap();
        let moved_path = package_path.with_extension("verified");

        let manifest = FakeManifestTransport {
            response: manifest_response,
            calls: Cell::new(0),
        };
        let artifact = CountingArtifactTransport {
            calls: Cell::new(0),
        };
        let signature_factory = ReplacingSignatureVerifierFactory {
            package_path: package_path.clone(),
            replacement: b"replacement package bytes".to_vec(),
        };
        let target = target_context();
        let coordinator = UpdateVerificationCoordinator::new(
            &state,
            &target,
            &staging,
            &manifest,
            &artifact,
            &signature_factory,
        );

        assert_eq!(
            coordinator.run(20).unwrap(),
            VerifyPendingUpdateOutcome::Failed
        );
        assert_eq!(artifact.calls.get(), 0);
        assert_eq!(state.snapshot().status, UpdateStatus::Failed);
        assert!(state.snapshot().pending_update.is_none());
        assert_eq!(fs::read(&moved_path).unwrap(), package_bytes);
        assert_eq!(
            fs::read(&package_path).unwrap(),
            b"replacement package bytes"
        );
        assert!(!staging.join("extract").join(&digest).exists());

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_ready_path_anomalies_fail_closed() {
        let directory = test_directory("ready-anomaly");
        let staging = directory.join("staging");
        let extract = staging.join("extract");
        fs::create_dir_all(&extract).unwrap();
        let digest = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let package_path = staging.join("packages").join("unused.app.tar.gz");
        fs::create_dir_all(package_path.parent().unwrap()).unwrap();
        fs::write(&package_path, b"x").unwrap();
        let verified_archive = VerifiedArchiveHandle::new(
            VerifiedArchive {
                relative_package_path: PathBuf::from("packages").join("unused.app.tar.gz"),
                artifact_id: "test-artifact".to_string(),
                raw_size_bytes: 1,
                raw_sha256: digest.to_string(),
                version: "1.2.3".to_string(),
                architecture: TARGET_ARCHITECTURE.to_string(),
            },
            fs::File::open(package_path).unwrap(),
        );
        let ready = extract.join(digest);

        fs::write(&ready, b"not a directory").unwrap();
        assert_eq!(
            extract_or_revalidate(&verified_archive, &staging),
            Err("staging-path")
        );
        fs::remove_file(&ready).unwrap();

        #[cfg(unix)]
        {
            let external = directory.join("external-ready");
            fs::create_dir_all(&external).unwrap();
            let marker = external.join("marker");
            fs::write(&marker, b"external marker").unwrap();
            std::os::unix::fs::symlink(&external, &ready).unwrap();
            assert_eq!(
                extract_or_revalidate(&verified_archive, &staging),
                Err("staging-path")
            );
            assert_eq!(fs::read(&marker).unwrap(), b"external marker");
            fs::remove_file(&ready).unwrap();

            fs::remove_dir(&extract).unwrap();
            let external_extract = directory.join("external-extract");
            fs::create_dir_all(external_extract.join(digest)).unwrap();
            let external_marker = external_extract.join("marker");
            fs::write(&external_marker, b"external extract marker").unwrap();
            std::os::unix::fs::symlink(&external_extract, &extract).unwrap();
            assert_eq!(
                extract_or_revalidate(&verified_archive, &staging),
                Err("staging-path")
            );
            assert_eq!(
                fs::read(&external_marker).unwrap(),
                b"external extract marker"
            );
            fs::remove_file(&extract).unwrap();
        }

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn changed_fresh_candidate_is_saved_without_artifact_work() {
        let directory = test_directory("candidate-changed");
        let staging = directory.join("staging");
        let state = UpdateStateStore::load_or_default(&directory, &staging);
        state
            .begin_check(crate::update_state::CheckTrigger::Manual, 10)
            .unwrap();
        state
            .record_available(
                PendingUpdate::new_with_signed_identity(
                    "1.2.3",
                    "stable",
                    "aarch64-apple-darwin",
                    "old-artifact",
                    4,
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "old-key",
                    "4444444444444444444444444444444444444444444444444444444444444444",
                    10,
                )
                .unwrap(),
            )
            .unwrap();

        let manifest = FakeManifestTransport {
            response: manifest_response("1.3.0", "new-artifact"),
            calls: Cell::new(0),
        };
        let artifact = CountingArtifactTransport {
            calls: Cell::new(0),
        };
        let trust_store = EmbeddedTrustedKeyStore::embedded().unwrap();
        let target = target_context();
        let outcome = UpdateVerificationCoordinator::new(
            &state,
            &target,
            &staging,
            &manifest,
            &artifact,
            &trust_store,
        )
        .run(20)
        .unwrap();

        assert_eq!(outcome, VerifyPendingUpdateOutcome::CandidateChanged);
        assert_eq!(manifest.calls.get(), 1);
        assert_eq!(artifact.calls.get(), 0);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert_eq!(snapshot.phase, None);
        let pending = snapshot.pending_update.unwrap();
        assert_eq!(pending.version, "1.3.0");
        assert_eq!(pending.artifact, "new-artifact");
        assert_eq!(
            pending.verification_state,
            crate::update_state::VerificationState::NotVerified
        );
        assert_eq!(pending.package_path, None);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recovery_removes_only_exact_digest_part_and_tmp_targets() {
        let directory = test_directory("recovery");
        let incoming = directory.join("incoming");
        let extract = directory.join("extract");
        fs::create_dir_all(&incoming).unwrap();
        fs::create_dir_all(&extract).unwrap();
        let digest = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let part = incoming.join(format!("{digest}.part"));
        let temporary = extract.join(format!("{digest}.tmp"));
        fs::write(&part, b"partial").unwrap();
        fs::create_dir(&temporary).unwrap();

        recover_staging_artifacts(&directory).unwrap();
        assert!(!part.exists());
        assert!(!temporary.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn operation_lock_returns_busy_without_fetching_manifest() {
        let directory = test_directory("busy");
        let staging = directory.join("staging");
        let state = UpdateStateStore::load_or_default(&directory, &staging);
        let operation = state.try_acquire_operation().unwrap().unwrap();
        let manifest = FakeManifestTransport {
            response: manifest_response("1.3.0", "new-artifact"),
            calls: Cell::new(0),
        };
        let artifact = CountingArtifactTransport {
            calls: Cell::new(0),
        };
        let trust_store = EmbeddedTrustedKeyStore::embedded().unwrap();
        let target = target_context();
        let outcome = UpdateVerificationCoordinator::new(
            &state,
            &target,
            &directory,
            &manifest,
            &artifact,
            &trust_store,
        )
        .run(20)
        .unwrap();

        assert_eq!(outcome, VerifyPendingUpdateOutcome::Busy);
        assert_eq!(manifest.calls.get(), 0);
        assert_eq!(artifact.calls.get(), 0);
        drop(operation);
        fs::remove_dir_all(directory).unwrap();
    }
}

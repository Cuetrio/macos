use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::update_archive::ExtractedArchive;
use crate::update_bundle::VerifiedAppBundle;
use crate::update_download::VerifiedArchive;
use crate::update_manifest::MANIFEST_PRODUCT_ID;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub(crate) const UPDATE_STATE_FILE_NAME: &str = "update-state.json";
pub(crate) const UPDATE_STATE_SCHEMA_VERSION: u32 = 2;
pub(crate) const UPDATE_STATE_SNAPSHOT_VERSION: u8 = 1;
pub(crate) const AUTO_CHECK_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

const MAX_IDENTIFIER_LENGTH: usize = 256;
const MAX_ERROR_CODE_LENGTH: usize = 64;
const INTERRUPTED_CHECK_ERROR_CODE: &str = "check-interrupted";
const INTERRUPTED_VERIFICATION_ERROR_CODE: &str = "verification-interrupted";
const INTERRUPTED_UPDATE_ERROR_CODE: &str = "update-interrupted";
const INTERRUPTED_ROLLBACK_ERROR_CODE: &str = "update-rollback";
const TEMP_FILE_ATTEMPTS: usize = 16;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UpdateStatus {
    NotChecked,
    Checking,
    NoUpdate,
    Available,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VerificationState {
    NotVerified,
    Verified,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UpdatePhase {
    ManifestCheck,
    PackageVerification,
    ApplyPreparation,
    RestartHealthCheck,
    Rollback,
    Cleanup,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UpdateRestartHandoff {
    Requested,
    MigrationStarted,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum UpdateRecoveryStage {
    HealthPending,
    BundleSwitching,
    BundleSwitched,
    CleanupPending,
    RollbackPending,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UpdateRecoveryCheckpoint {
    pub(crate) stage: UpdateRecoveryStage,
    // Only Some(true) proves that the live database was switched.  None is
    // retained for old or interrupted checkpoints and is fail-closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) database_switched: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckTrigger {
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckStart {
    Started,
    Suppressed,
    AlreadyChecking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateStateLoadIssue {
    Io,
    Symlink,
    Empty,
    Invalid,
    UnsupportedSchema,
    MigrationWrite,
}

impl UpdateStateLoadIssue {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Io => "state-read-failed",
            Self::Symlink => "state-symlink",
            Self::Empty => "state-empty",
            Self::Invalid => "state-invalid",
            Self::UnsupportedSchema => "state-schema-unsupported",
            Self::MigrationWrite => "state-migration-write-failed",
        }
    }
}

#[derive(Debug)]
pub(crate) enum UpdateStateError {
    Invalid(String),
    Storage(String),
    LockPoisoned,
}

impl std::fmt::Display for UpdateStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "{message}"),
            Self::Storage(message) => write!(formatter, "{message}"),
            Self::LockPoisoned => formatter.write_str("update state lock is poisoned"),
        }
    }
}

impl std::error::Error for UpdateStateError {}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PendingUpdate {
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) architecture: String,
    pub(crate) artifact: String,
    pub(crate) verification_state: VerificationState,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) sha256: Option<String>,
    pub(crate) key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) signed_identity_sha256: Option<String>,
    pub(crate) package_path: Option<PathBuf>,
    pub(crate) extracted_app_path: Option<PathBuf>,
    pub(crate) discovered_at: u64,
    pub(crate) verified_at: Option<u64>,
}

impl PendingUpdate {
    pub(crate) fn new_with_signed_identity(
        version: impl Into<String>,
        channel: impl Into<String>,
        architecture: impl Into<String>,
        artifact: impl Into<String>,
        size_bytes: u64,
        sha256: impl Into<String>,
        key_id: impl Into<String>,
        signed_identity_sha256: impl Into<String>,
        discovered_at: u64,
    ) -> Result<Self, UpdateStateError> {
        let pending = Self {
            version: version.into(),
            channel: channel.into(),
            architecture: architecture.into(),
            artifact: artifact.into(),
            verification_state: VerificationState::NotVerified,
            size_bytes: Some(size_bytes),
            sha256: Some(sha256.into()),
            key_id: Some(key_id.into()),
            signed_identity_sha256: Some(signed_identity_sha256.into()),
            package_path: None,
            extracted_app_path: None,
            discovered_at,
            verified_at: None,
        };
        pending.validate()?;
        Ok(pending)
    }

    fn validate(&self) -> Result<(), UpdateStateError> {
        validate_identifier(&self.version, "update version")?;
        validate_identifier(&self.channel, "update channel")?;
        validate_identifier(&self.architecture, "update architecture")?;
        validate_identifier(&self.artifact, "update artifact")?;

        let evidence = match (&self.size_bytes, &self.sha256, &self.key_id) {
            (None, None, None) => false,
            (Some(size_bytes), Some(sha256), Some(key_id)) => {
                if *size_bytes == 0 {
                    return Err(UpdateStateError::Invalid(
                        "update package size must be positive".to_string(),
                    ));
                }
                validate_sha256(sha256)?;
                validate_identifier(key_id, "update key id")?;
                true
            }
            _ => {
                return Err(UpdateStateError::Invalid(
                    "update package evidence is incomplete".to_string(),
                ));
            }
        };

        if let Some(signed_identity_sha256) = &self.signed_identity_sha256 {
            validate_sha256(signed_identity_sha256)?;
        }

        match self.verification_state {
            VerificationState::NotVerified => {
                if self.verified_at.is_some() {
                    return Err(UpdateStateError::Invalid(
                        "unverified update has a verification timestamp".to_string(),
                    ));
                }
            }
            VerificationState::Verified => {
                if !evidence
                    || self.signed_identity_sha256.is_none()
                    || self.package_path.is_none()
                    || self.extracted_app_path.is_none()
                    || self.verified_at.is_none()
                {
                    return Err(UpdateStateError::Invalid(
                        "verified update is missing verification evidence".to_string(),
                    ));
                }
            }
            VerificationState::Failed => {
                return Err(UpdateStateError::Invalid(
                    "failed update verification state is not supported".to_string(),
                ));
            }
        }

        if !evidence && (self.package_path.is_some() || self.extracted_app_path.is_some()) {
            return Err(UpdateStateError::Invalid(
                "update paths require package evidence".to_string(),
            ));
        }

        if let Some(package_path) = &self.package_path {
            let sha256 = self.sha256.as_deref().ok_or_else(|| {
                UpdateStateError::Invalid("package path has no digest".to_string())
            })?;
            validate_canonical_package_path(package_path, sha256)?;
        }
        if let Some(extracted_app_path) = &self.extracted_app_path {
            let sha256 = self.sha256.as_deref().ok_or_else(|| {
                UpdateStateError::Invalid("extracted app path has no digest".to_string())
            })?;
            validate_canonical_extracted_app_path(extracted_app_path, sha256)?;
        }
        Ok(())
    }

    pub(crate) fn validate_paths_at(
        &self,
        staging_directory: &Path,
    ) -> Result<(), UpdateStateError> {
        self.validate()?;
        if let Some(package_path) = &self.package_path {
            reject_symlink_components(staging_directory, package_path, "package path")?;
        }
        if let Some(extracted_app_path) = &self.extracted_app_path {
            reject_symlink_components(staging_directory, extracted_app_path, "extracted app path")?;
        }
        Ok(())
    }

    fn legacy(
        version: String,
        channel: String,
        architecture: String,
        artifact: String,
        discovered_at: u64,
    ) -> Self {
        Self {
            version,
            channel,
            architecture,
            artifact,
            verification_state: VerificationState::NotVerified,
            size_bytes: None,
            sha256: None,
            key_id: None,
            signed_identity_sha256: None,
            package_path: None,
            extracted_app_path: None,
            discovered_at,
            verified_at: None,
        }
    }

    pub(crate) fn candidate_identity_matches(&self, other: &Self) -> bool {
        let signed_identity_matches = match (
            self.signed_identity_sha256.as_deref(),
            other.signed_identity_sha256.as_deref(),
        ) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        };

        self.version == other.version
            && self.channel == other.channel
            && self.architecture == other.architecture
            && self.artifact == other.artifact
            && self.size_bytes == other.size_bytes
            && self.sha256 == other.sha256
            && self.key_id == other.key_id
            && signed_identity_matches
    }

    fn notification_key_matches(&self, notification: &UpdateNotification) -> bool {
        self.version == notification.version && self.artifact == notification.artifact
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UpdateFailure {
    pub(crate) code: String,
    pub(crate) retry_at: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UpdateNotification {
    pub(crate) version: String,
    pub(crate) artifact: String,
    pub(crate) notified_at: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UpdateState {
    pub(crate) schema_version: u32,
    pub(crate) status: UpdateStatus,
    pub(crate) phase: Option<UpdatePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) restart_handoff: Option<UpdateRestartHandoff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_check_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) check_started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_update: Option<PendingUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure: Option<UpdateFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recovery: Option<UpdateRecoveryCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) notification: Option<UpdateNotification>,
}

impl UpdateState {
    pub(crate) fn initial() -> Self {
        Self {
            schema_version: UPDATE_STATE_SCHEMA_VERSION,
            status: UpdateStatus::NotChecked,
            phase: None,
            restart_handoff: None,
            last_check_at: None,
            check_started_at: None,
            pending_update: None,
            failure: None,
            recovery: None,
            notification: None,
        }
    }

    fn validate(&self) -> Result<(), UpdateStateError> {
        if self.schema_version != UPDATE_STATE_SCHEMA_VERSION {
            return Err(UpdateStateError::Invalid(
                "update state schema version is unsupported".to_string(),
            ));
        }

        if let Some(pending_update) = &self.pending_update {
            pending_update.validate()?;
        }

        if self.status == UpdateStatus::Checking {
            if self.phase.is_none() || self.check_started_at.is_none() {
                return Err(UpdateStateError::Invalid(
                    "checking state is missing its phase or start timestamp".to_string(),
                ));
            }
            if matches!(
                self.phase,
                Some(
                    UpdatePhase::PackageVerification
                        | UpdatePhase::ApplyPreparation
                        | UpdatePhase::RestartHealthCheck
                        | UpdatePhase::Rollback
                        | UpdatePhase::Cleanup
                )
            ) && self.pending_update.is_none()
            {
                return Err(UpdateStateError::Invalid(
                    "active update phase has no pending update".to_string(),
                ));
            }
            if matches!(
                self.phase,
                Some(
                    UpdatePhase::ApplyPreparation
                        | UpdatePhase::RestartHealthCheck
                        | UpdatePhase::Rollback
                        | UpdatePhase::Cleanup
                )
            ) && self.pending_update.as_ref().is_some_and(|pending_update| {
                pending_update.verification_state != VerificationState::Verified
            }) {
                return Err(UpdateStateError::Invalid(
                    "active apply phase requires a verified update".to_string(),
                ));
            }
        } else if self.phase.is_some() {
            return Err(UpdateStateError::Invalid(
                "non-checking state contains a phase".to_string(),
            ));
        }

        if self.restart_handoff.is_some()
            && (self.status != UpdateStatus::Checking
                || self.phase != Some(UpdatePhase::ApplyPreparation)
                || self.failure.is_some()
                || self.pending_update.as_ref().is_none_or(|candidate| {
                    candidate.verification_state != VerificationState::Verified
                }))
        {
            return Err(UpdateStateError::Invalid(
                "restart handoff has no pending verified apply preparation".to_string(),
            ));
        }

        if let Some(failure) = &self.failure {
            validate_error_code(&failure.code)?;
        }

        if let Some(recovery) = &self.recovery {
            let expected_phase = match recovery.stage {
                UpdateRecoveryStage::HealthPending
                | UpdateRecoveryStage::BundleSwitching
                | UpdateRecoveryStage::BundleSwitched => UpdatePhase::RestartHealthCheck,
                UpdateRecoveryStage::CleanupPending => UpdatePhase::Cleanup,
                UpdateRecoveryStage::RollbackPending => UpdatePhase::Rollback,
            };
            if self.status != UpdateStatus::Checking
                || self.pending_update.is_none()
                || self.phase != Some(expected_phase)
            {
                return Err(UpdateStateError::Invalid(
                    "update recovery checkpoint has an invalid phase".to_string(),
                ));
            }
        }

        if let Some(notification) = &self.notification {
            validate_identifier(&notification.version, "notified update version")?;
            validate_identifier(&notification.artifact, "notified update artifact")?;
            let Some(pending_update) = &self.pending_update else {
                return Err(UpdateStateError::Invalid(
                    "update notification has no pending update".to_string(),
                ));
            };
            if !pending_update.notification_key_matches(notification) {
                return Err(UpdateStateError::Invalid(
                    "update notification does not match pending update".to_string(),
                ));
            }
        }

        match self.status {
            UpdateStatus::NotChecked => {
                if self.check_started_at.is_some()
                    || self.pending_update.is_some()
                    || self.failure.is_some()
                    || self.notification.is_some()
                {
                    return Err(UpdateStateError::Invalid(
                        "not-checked state contains a completed check result".to_string(),
                    ));
                }
            }
            UpdateStatus::Checking => {
                if self.last_check_at.is_none() || self.check_started_at.is_none() {
                    return Err(UpdateStateError::Invalid(
                        "checking state is missing its timestamps".to_string(),
                    ));
                }
                if self.failure.is_some()
                    && !matches!(
                        self.phase,
                        Some(UpdatePhase::Rollback) | Some(UpdatePhase::Cleanup)
                    )
                {
                    return Err(UpdateStateError::Invalid(
                        "checking state contains a failure".to_string(),
                    ));
                }
            }
            UpdateStatus::NoUpdate => {
                if self.check_started_at.is_some()
                    || self.pending_update.is_some()
                    || self.failure.is_some()
                    || self.notification.is_some()
                {
                    return Err(UpdateStateError::Invalid(
                        "no-update state contains another result".to_string(),
                    ));
                }
            }
            UpdateStatus::Available => {
                if self.check_started_at.is_some() || self.pending_update.is_none() {
                    return Err(UpdateStateError::Invalid(
                        "available state is incomplete".to_string(),
                    ));
                }
            }
            UpdateStatus::Failed => {
                if self.check_started_at.is_some()
                    || self.failure.is_none()
                    || self.pending_update.is_some()
                    || self.notification.is_some()
                {
                    return Err(UpdateStateError::Invalid(
                        "failed state is incomplete".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_paths_at(&self, staging_directory: &Path) -> Result<(), UpdateStateError> {
        if let Some(pending_update) = &self.pending_update {
            pending_update.validate_paths_at(staging_directory)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PendingUpdateSnapshot {
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) architecture: String,
    pub(crate) artifact: String,
    pub(crate) verification_state: VerificationState,
    pub(crate) discovered_at: u64,
}

impl From<&PendingUpdate> for PendingUpdateSnapshot {
    fn from(pending_update: &PendingUpdate) -> Self {
        Self {
            version: pending_update.version.clone(),
            channel: pending_update.channel.clone(),
            architecture: pending_update.architecture.clone(),
            artifact: pending_update.artifact.clone(),
            verification_state: pending_update.verification_state,
            discovered_at: pending_update.discovered_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UpdateFailureSnapshot {
    pub(crate) code: String,
    pub(crate) retry_at: u64,
}

impl From<&UpdateFailure> for UpdateFailureSnapshot {
    fn from(failure: &UpdateFailure) -> Self {
        Self {
            code: failure.code.clone(),
            retry_at: failure.retry_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UpdateStateSnapshot {
    pub(crate) snapshot_version: u8,
    pub(crate) status: UpdateStatus,
    pub(crate) last_check_at: Option<u64>,
    pub(crate) check_started_at: Option<u64>,
    pub(crate) pending_update: Option<PendingUpdateSnapshot>,
    pub(crate) failure: Option<UpdateFailureSnapshot>,
}

impl From<&UpdateState> for UpdateStateSnapshot {
    fn from(state: &UpdateState) -> Self {
        Self {
            snapshot_version: UPDATE_STATE_SNAPSHOT_VERSION,
            status: state.status,
            last_check_at: state.last_check_at,
            check_started_at: state.check_started_at,
            pending_update: state
                .pending_update
                .as_ref()
                .map(PendingUpdateSnapshot::from),
            failure: state.failure.as_ref().map(UpdateFailureSnapshot::from),
        }
    }
}

pub(crate) struct UpdateStateStore {
    state_path: PathBuf,
    staging_directory: PathBuf,
    state: Mutex<UpdateState>,
    transition: Mutex<()>,
    operation: Mutex<()>,
    load_issue: Option<UpdateStateLoadIssue>,
    writes_blocked: bool,
}

pub(crate) struct UpdateOperationGuard<'a> {
    _guard: MutexGuard<'a, ()>,
}

impl UpdateStateStore {
    pub(crate) fn load_or_default(settings_directory: &Path, staging_directory: &Path) -> Self {
        Self::load_or_default_at(settings_directory, staging_directory, current_timestamp())
    }

    fn load_or_default_at(settings_directory: &Path, staging_directory: &Path, now: u64) -> Self {
        let state_path = settings_directory.join(UPDATE_STATE_FILE_NAME);
        let (mut state, mut load_issue, mut writes_blocked, needs_migration) =
            match fs::symlink_metadata(&state_path) {
                Ok(metadata) if metadata.file_type().is_symlink() => (
                    UpdateState::initial(),
                    Some(UpdateStateLoadIssue::Symlink),
                    true,
                    false,
                ),
                Ok(_) => match fs::read(&state_path) {
                    Ok(contents) => match parse_state(&contents, now) {
                        Ok(parsed) => (parsed.state, None, false, parsed.needs_migration),
                        Err(issue) => (
                            UpdateState::initial(),
                            Some(issue),
                            issue == UpdateStateLoadIssue::UnsupportedSchema,
                            false,
                        ),
                    },
                    Err(_) => (
                        UpdateState::initial(),
                        Some(UpdateStateLoadIssue::Io),
                        false,
                        false,
                    ),
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    (UpdateState::initial(), None, false, false)
                }
                Err(_) => (
                    UpdateState::initial(),
                    Some(UpdateStateLoadIssue::Io),
                    false,
                    false,
                ),
            };

        let mut needs_persist = needs_migration;
        if load_issue.is_none() && recover_persisted_staged_migration_failure(&mut state, now) {
            needs_persist = true;
        }

        let defer_apply_path_validation = state.status == UpdateStatus::Checking
            && matches!(
                state.phase,
                Some(
                    UpdatePhase::ApplyPreparation
                        | UpdatePhase::RestartHealthCheck
                        | UpdatePhase::Rollback
                        | UpdatePhase::Cleanup
                )
            );
        if load_issue.is_none() && !defer_apply_path_validation {
            if state.validate_paths_at(staging_directory).is_err() {
                state = UpdateState::initial();
                load_issue = Some(UpdateStateLoadIssue::Invalid);
                writes_blocked = false;
            }
        };

        let recover_interrupted = state.status == UpdateStatus::Checking
            && matches!(
                state.phase,
                Some(
                    UpdatePhase::ManifestCheck
                        | UpdatePhase::PackageVerification
                        | UpdatePhase::ApplyPreparation
                )
            )
            && !(state.phase == Some(UpdatePhase::ApplyPreparation)
                && state.restart_handoff == Some(UpdateRestartHandoff::Requested));
        if recover_interrupted {
            needs_persist |= recover_interrupted_check(&mut state, now);
        }

        if load_issue.is_none() && ensure_recovery_checkpoint(&mut state) {
            needs_persist = true;
        }

        if needs_persist && load_issue.is_none() {
            if write_state_atomically(&state_path, &state).is_err() {
                load_issue = Some(UpdateStateLoadIssue::MigrationWrite);
                writes_blocked = true;
            }
        }

        Self {
            state_path,
            staging_directory: staging_directory.to_path_buf(),
            state: Mutex::new(state),
            transition: Mutex::new(()),
            operation: Mutex::new(()),
            load_issue,
            writes_blocked,
        }
    }

    pub(crate) fn load_issue(&self) -> Option<UpdateStateLoadIssue> {
        self.load_issue
    }

    pub(crate) fn snapshot(&self) -> UpdateState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| UpdateState::initial())
    }

    pub(crate) fn has_pending_apply_preparation(&self) -> bool {
        self.state.lock().map_or(false, |state| {
            state.status == UpdateStatus::Checking
                && state.phase == Some(UpdatePhase::ApplyPreparation)
                && state.failure.is_none()
                && state.restart_handoff.is_some()
                && state.pending_update.as_ref().is_some_and(|candidate| {
                    candidate.verification_state == VerificationState::Verified
                })
        })
    }

    pub(crate) fn record_explicit_restart_handoff(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::ApplyPreparation)
            || state.failure.is_some()
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
        {
            return Err(UpdateStateError::Invalid(
                "explicit restart handoff has no pending verified apply preparation".to_string(),
            ));
        }
        if state.restart_handoff == Some(UpdateRestartHandoff::Requested) {
            return Ok(());
        }
        if state.restart_handoff == Some(UpdateRestartHandoff::MigrationStarted) {
            return Err(UpdateStateError::Invalid(
                "explicit restart handoff has already started migration".to_string(),
            ));
        }

        let previous = state.clone();
        state.restart_handoff = Some(UpdateRestartHandoff::Requested);
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn claim_staged_migration(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::ApplyPreparation)
            || state.failure.is_some()
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
        {
            return Err(UpdateStateError::Invalid(
                "staged migration claim has no pending verified apply preparation".to_string(),
            ));
        }
        if state.restart_handoff == Some(UpdateRestartHandoff::MigrationStarted) {
            return Err(UpdateStateError::Invalid(
                "staged migration claim has already been consumed".to_string(),
            ));
        }
        if state.restart_handoff != Some(UpdateRestartHandoff::Requested) {
            return Err(UpdateStateError::Invalid(
                "staged migration claim has no explicit restart handoff".to_string(),
            ));
        }

        let previous = state.clone();
        state.restart_handoff = Some(UpdateRestartHandoff::MigrationStarted);
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn read_only_snapshot(&self) -> Result<UpdateStateSnapshot, UpdateStateError> {
        if self
            .load_issue
            .is_some_and(|issue| issue != UpdateStateLoadIssue::MigrationWrite)
        {
            return Err(UpdateStateError::Storage(
                "update state is unavailable".to_string(),
            ));
        }

        self.state
            .lock()
            .map(|state| UpdateStateSnapshot::from(&*state))
            .map_err(|_| UpdateStateError::LockPoisoned)
    }

    pub(crate) fn begin_check(
        &self,
        trigger: CheckTrigger,
        now: u64,
    ) -> Result<CheckStart, UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status == UpdateStatus::Checking {
            return Ok(CheckStart::AlreadyChecking);
        }
        if trigger == CheckTrigger::Automatic && !automatic_check_is_due(&state, now) {
            return Ok(CheckStart::Suppressed);
        }

        let previous = state.clone();
        state.status = UpdateStatus::Checking;
        state.phase = Some(UpdatePhase::ManifestCheck);
        state.last_check_at = Some(now);
        state.check_started_at = Some(now);
        state.failure = None;
        state.restart_handoff = None;
        state.recovery = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(CheckStart::Started)
    }

    pub(crate) fn try_acquire_operation(
        &self,
    ) -> Result<Option<UpdateOperationGuard<'_>>, UpdateStateError> {
        match self.operation.try_lock() {
            Ok(guard) => Ok(Some(UpdateOperationGuard { _guard: guard })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Poisoned(_)) => Err(UpdateStateError::LockPoisoned),
        }
    }

    pub(crate) fn record_no_update(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        self.mutate_check_result(|state| {
            cleanup_discarded_verified_artifacts(
                &self.staging_directory,
                state.pending_update.as_ref(),
                None,
            )?;
            state.status = UpdateStatus::NoUpdate;
            state.phase = None;
            state.check_started_at = None;
            state.pending_update = None;
            state.failure = None;
            state.restart_handoff = None;
            state.recovery = None;
            state.notification = None;
            Ok(())
        })
    }

    pub(crate) fn record_available(
        &self,
        pending_update: PendingUpdate,
    ) -> Result<(), UpdateStateError> {
        pending_update.validate()?;
        self.ensure_writable()?;
        self.mutate_check_result(|state| {
            let same_pending = state
                .pending_update
                .as_ref()
                .is_some_and(|existing| existing.candidate_identity_matches(&pending_update));
            if !same_pending {
                cleanup_discarded_verified_artifacts(
                    &self.staging_directory,
                    state.pending_update.as_ref(),
                    Some(&pending_update),
                )?;
            }
            let pending_update = if same_pending {
                let existing = state
                    .pending_update
                    .as_ref()
                    .expect("same pending update must exist");
                let mut merged = pending_update;
                merged.verification_state = existing.verification_state;
                merged.package_path = existing.package_path.clone();
                merged.extracted_app_path = existing.extracted_app_path.clone();
                merged.verified_at = existing.verified_at;
                merged
            } else {
                pending_update
            };
            state.status = UpdateStatus::Available;
            state.phase = None;
            state.check_started_at = None;
            state.pending_update = Some(pending_update);
            state.failure = None;
            state.restart_handoff = None;
            state.recovery = None;
            if !same_pending {
                state.notification = None;
            }
            Ok(())
        })
    }

    pub(crate) fn replace_available_candidate(
        &self,
        pending_update: PendingUpdate,
    ) -> Result<(), UpdateStateError> {
        pending_update.validate()?;
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Available {
            return Err(UpdateStateError::Invalid(
                "revalidated candidate has no available update".to_string(),
            ));
        }
        let pending_update = if state
            .pending_update
            .as_ref()
            .is_some_and(|existing| existing.candidate_identity_matches(&pending_update))
        {
            let existing = state
                .pending_update
                .as_ref()
                .expect("same pending update must exist");
            let mut merged = pending_update;
            merged.verification_state = existing.verification_state;
            merged.package_path = existing.package_path.clone();
            merged.extracted_app_path = existing.extracted_app_path.clone();
            merged.verified_at = existing.verified_at;
            merged
        } else {
            pending_update
        };
        let previous = state.clone();
        cleanup_discarded_verified_artifacts(
            &self.staging_directory,
            state.pending_update.as_ref(),
            Some(&pending_update),
        )?;
        state.phase = None;
        state.check_started_at = None;
        state.pending_update = Some(pending_update);
        state.failure = None;
        state.restart_handoff = None;
        state.recovery = None;
        state.notification = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_revalidated_no_update(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Available {
            return Err(UpdateStateError::Invalid(
                "revalidated no-update result has no available update".to_string(),
            ));
        }
        let previous = state.clone();
        cleanup_discarded_verified_artifacts(
            &self.staging_directory,
            state.pending_update.as_ref(),
            None,
        )?;
        state.status = UpdateStatus::NoUpdate;
        state.phase = None;
        state.check_started_at = None;
        state.pending_update = None;
        state.failure = None;
        state.restart_handoff = None;
        state.recovery = None;
        state.notification = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_failure(
        &self,
        error_code: &str,
        retry_at: u64,
    ) -> Result<(), UpdateStateError> {
        let code = sanitize_error_code(error_code);
        self.ensure_writable()?;
        self.mutate_check_result(|state| {
            let has_pending_update = state.pending_update.is_some();
            state.status = if has_pending_update {
                UpdateStatus::Available
            } else {
                UpdateStatus::Failed
            };
            state.phase = None;
            state.check_started_at = None;
            if !has_pending_update {
                state.pending_update = None;
                state.notification = None;
            }
            state.recovery = None;
            state.restart_handoff = None;
            state.failure = Some(UpdateFailure { code, retry_at });
            Ok(())
        })
    }

    pub(crate) fn record_verification_failure(
        &self,
        error_code: &str,
        retry_at: u64,
    ) -> Result<(), UpdateStateError> {
        let code = sanitize_error_code(error_code);
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if !matches!(
            state.status,
            UpdateStatus::Available | UpdateStatus::Checking
        ) {
            return Err(UpdateStateError::Invalid(
                "verification failure has no active update candidate".to_string(),
            ));
        }
        let previous = state.clone();
        state.status = UpdateStatus::Failed;
        state.phase = None;
        state.check_started_at = None;
        state.pending_update = None;
        state.notification = None;
        state.restart_handoff = None;
        state.recovery = None;
        state.failure = Some(UpdateFailure { code, retry_at });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    fn mutate_check_result<F>(&self, mutation: F) -> Result<(), UpdateStateError>
    where
        F: FnOnce(&mut UpdateState) -> Result<(), UpdateStateError>,
    {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking {
            return Err(UpdateStateError::Invalid(
                "update check result has no active check".to_string(),
            ));
        }

        let previous = state.clone();
        mutation(&mut state)?;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn begin_package_verification(
        &self,
        expected_candidate: &PendingUpdate,
        now: u64,
    ) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Available {
            return Err(UpdateStateError::Invalid(
                "package verification has no available update".to_string(),
            ));
        }
        let current_candidate = state.pending_update.as_ref().ok_or_else(|| {
            UpdateStateError::Invalid("package verification has no candidate".to_string())
        })?;
        if !current_candidate.candidate_identity_matches(expected_candidate) {
            return Err(UpdateStateError::Invalid(
                "package verification candidate identity changed".to_string(),
            ));
        }

        let previous = state.clone();
        let pending_update = state.pending_update.as_mut().ok_or_else(|| {
            UpdateStateError::Invalid("verification has no candidate".to_string())
        })?;
        pending_update.verification_state = VerificationState::NotVerified;
        pending_update.package_path = None;
        pending_update.extracted_app_path = None;
        pending_update.verified_at = None;
        state.status = UpdateStatus::Checking;
        state.phase = Some(UpdatePhase::PackageVerification);
        state.check_started_at = Some(now);
        state.failure = None;
        state.restart_handoff = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_package_checkpoint(
        &self,
        verified_archive: &VerifiedArchive,
    ) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::PackageVerification)
        {
            return Err(UpdateStateError::Invalid(
                "package checkpoint has no active verification".to_string(),
            ));
        }
        let pending_update = state.pending_update.as_ref().ok_or_else(|| {
            UpdateStateError::Invalid("package checkpoint has no pending candidate".to_string())
        })?;
        let expected_path = validate_verified_archive(pending_update, verified_archive)?;
        let mut next = state.clone();
        let pending_update = next.pending_update.as_mut().ok_or_else(|| {
            UpdateStateError::Invalid("package checkpoint has no pending candidate".to_string())
        })?;
        pending_update.package_path = Some(expected_path);
        pending_update.extracted_app_path = None;
        pending_update.verification_state = VerificationState::NotVerified;
        pending_update.verified_at = None;
        next.validate()?;
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            return Err(error);
        }
        let mut state = self.lock_state()?;
        *state = next;
        Ok(())
    }

    pub(crate) fn record_extraction_checkpoint(
        &self,
        extracted_archive: &ExtractedArchive,
    ) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::PackageVerification)
        {
            return Err(UpdateStateError::Invalid(
                "extraction checkpoint has no active verification".to_string(),
            ));
        }
        let pending_update = state.pending_update.as_ref().ok_or_else(|| {
            UpdateStateError::Invalid("extraction checkpoint has no pending candidate".to_string())
        })?;
        if pending_update.package_path.is_none() {
            return Err(UpdateStateError::Invalid(
                "extraction checkpoint has no package checkpoint".to_string(),
            ));
        }
        let expected_path = validate_extracted_archive(pending_update, extracted_archive)?;
        let mut next = state.clone();
        let pending_update = next.pending_update.as_mut().ok_or_else(|| {
            UpdateStateError::Invalid("extraction checkpoint has no pending candidate".to_string())
        })?;
        pending_update.extracted_app_path = Some(expected_path);
        pending_update.verification_state = VerificationState::NotVerified;
        pending_update.verified_at = None;
        next.validate()?;
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            return Err(error);
        }
        let mut state = self.lock_state()?;
        *state = next;
        Ok(())
    }

    pub(crate) fn record_verified(
        &self,
        verified_archive: &VerifiedArchive,
        extracted_archive: &ExtractedArchive,
        verified_bundle: &VerifiedAppBundle,
        verified_at: u64,
    ) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::PackageVerification)
        {
            return Err(UpdateStateError::Invalid(
                "verified update has no active package verification".to_string(),
            ));
        }

        let mut pending_update = state.pending_update.clone().ok_or_else(|| {
            UpdateStateError::Invalid("verified update has no pending candidate".to_string())
        })?;
        let expected_package_path = validate_verified_archive(&pending_update, verified_archive)?;
        let expected_extracted_path =
            validate_extracted_archive(&pending_update, extracted_archive)?;
        if pending_update.package_path.as_ref() != Some(&expected_package_path)
            || pending_update.extracted_app_path.as_ref() != Some(&expected_extracted_path)
        {
            return Err(UpdateStateError::Invalid(
                "verified update checkpoints do not match pipeline results".to_string(),
            ));
        }
        if verified_bundle.relative_app_path != expected_extracted_path
            || verified_bundle.bundle_identifier != MANIFEST_PRODUCT_ID
            || verified_bundle.version != pending_update.version
            || verified_bundle.architecture != pending_update.architecture
        {
            return Err(UpdateStateError::Invalid(
                "verified app bundle does not match pipeline results".to_string(),
            ));
        }
        pending_update.verification_state = VerificationState::Verified;
        pending_update.verified_at = Some(verified_at);
        drop(state);
        pending_update.validate_paths_at(&self.staging_directory)?;

        let mut state = self.lock_state()?;

        let previous = state.clone();
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::PackageVerification)
        {
            return Err(UpdateStateError::Invalid(
                "verified update operation changed before commit".to_string(),
            ));
        }
        state.status = UpdateStatus::Available;
        state.phase = None;
        state.check_started_at = None;
        state.pending_update = Some(pending_update);
        state.failure = None;
        state.restart_handoff = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn begin_apply_preparation(
        &self,
        expected_candidate: &PendingUpdate,
        now: u64,
    ) -> Result<(), UpdateStateError> {
        expected_candidate.validate_paths_at(&self.staging_directory)?;
        if expected_candidate.verification_state != VerificationState::Verified {
            return Err(UpdateStateError::Invalid(
                "apply preparation requires a verified update".to_string(),
            ));
        }
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Available {
            return Err(UpdateStateError::Invalid(
                "apply preparation has no available update".to_string(),
            ));
        }
        let current_candidate = state.pending_update.as_ref().ok_or_else(|| {
            UpdateStateError::Invalid("apply preparation has no candidate".to_string())
        })?;
        if !current_candidate.candidate_identity_matches(expected_candidate) {
            return Err(UpdateStateError::Invalid(
                "apply preparation candidate identity changed".to_string(),
            ));
        }
        if current_candidate.verification_state != VerificationState::Verified {
            return Err(UpdateStateError::Invalid(
                "apply preparation candidate is not verified".to_string(),
            ));
        }
        current_candidate.validate_paths_at(&self.staging_directory)?;

        let previous = state.clone();
        state.status = UpdateStatus::Checking;
        state.phase = Some(UpdatePhase::ApplyPreparation);
        state.check_started_at = Some(now);
        state.failure = None;
        state.restart_handoff = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_staged_migration_no_pending(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::ApplyPreparation)
            || state.restart_handoff != Some(UpdateRestartHandoff::MigrationStarted)
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
        {
            return Err(UpdateStateError::Invalid(
                "staged migration result has no pending apply preparation".to_string(),
            ));
        }
        let previous = state.clone();
        state.phase = Some(UpdatePhase::RestartHealthCheck);
        state.failure = None;
        state.restart_handoff = None;
        state.recovery = Some(UpdateRecoveryCheckpoint {
            stage: UpdateRecoveryStage::HealthPending,
            database_switched: Some(false),
        });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_staged_migration_switched(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::ApplyPreparation)
            || state.restart_handoff != Some(UpdateRestartHandoff::MigrationStarted)
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
        {
            return Err(UpdateStateError::Invalid(
                "staged migration success has no pending apply preparation".to_string(),
            ));
        }
        let previous = state.clone();
        state.phase = Some(UpdatePhase::RestartHealthCheck);
        state.failure = None;
        state.restart_handoff = None;
        state.recovery = Some(UpdateRecoveryCheckpoint {
            stage: UpdateRecoveryStage::HealthPending,
            database_switched: Some(true),
        });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_staged_migration_failure(
        &self,
        error_code: &str,
        retry_at: u64,
    ) -> Result<(), UpdateStateError> {
        let code = sanitize_error_code(error_code);
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::ApplyPreparation)
            || state.restart_handoff != Some(UpdateRestartHandoff::MigrationStarted)
        {
            return Err(UpdateStateError::Invalid(
                "staged migration failure has no pending apply preparation".to_string(),
            ));
        }
        let previous = state.clone();
        state.phase = Some(UpdatePhase::Rollback);
        state.failure = Some(UpdateFailure { code, retry_at });
        state.restart_handoff = None;
        state.recovery = Some(UpdateRecoveryCheckpoint {
            stage: UpdateRecoveryStage::RollbackPending,
            database_switched: Some(false),
        });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_bundle_switching(&self) -> Result<(), UpdateStateError> {
        self.record_recovery_checkpoint(
            UpdatePhase::RestartHealthCheck,
            UpdateRecoveryStage::BundleSwitching,
            None,
        )
    }

    fn record_recovery_checkpoint(
        &self,
        phase: UpdatePhase,
        stage: UpdateRecoveryStage,
        database_switched: Option<bool>,
    ) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(phase)
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
        {
            return Err(UpdateStateError::Invalid(
                "update recovery checkpoint has no verified candidate".to_string(),
            ));
        }
        if database_switched.is_none() {
            if let Some(existing) = state.recovery.as_ref() {
                if existing.stage == stage {
                    return Ok(());
                }
            }
        }
        let previous = state.clone();
        let existing_database_switched = state
            .recovery
            .as_ref()
            .and_then(|checkpoint| checkpoint.database_switched);
        state.recovery = Some(UpdateRecoveryCheckpoint {
            stage,
            database_switched: merge_database_switch_evidence(
                existing_database_switched,
                database_switched,
            ),
        });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_bundle_switched(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::RestartHealthCheck)
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
        {
            return Err(UpdateStateError::Invalid(
                "bundle switch success has no restart health checkpoint".to_string(),
            ));
        }
        if state
            .recovery
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.stage == UpdateRecoveryStage::BundleSwitched)
        {
            return Ok(());
        }
        if !state.recovery.as_ref().is_some_and(|checkpoint| {
            matches!(
                checkpoint.stage,
                UpdateRecoveryStage::BundleSwitching | UpdateRecoveryStage::BundleSwitched
            )
        }) {
            return Err(UpdateStateError::Invalid(
                "bundle switch success has no switching checkpoint".to_string(),
            ));
        }
        let previous = state.clone();
        if let Some(recovery) = state.recovery.as_mut() {
            recovery.stage = UpdateRecoveryStage::BundleSwitched;
        }
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_cleanup_pending(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::RestartHealthCheck)
            || !state
                .recovery
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.stage == UpdateRecoveryStage::BundleSwitched)
        {
            return Err(UpdateStateError::Invalid(
                "cleanup has no successful candidate health checkpoint".to_string(),
            ));
        }
        let previous = state.clone();
        state.phase = Some(UpdatePhase::Cleanup);
        if let Some(recovery) = state.recovery.as_mut() {
            recovery.stage = UpdateRecoveryStage::CleanupPending;
        }
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_recovery_failure(
        &self,
        phase: UpdatePhase,
        stage: UpdateRecoveryStage,
        database_switched: Option<bool>,
        error_code: &str,
        retry_at: u64,
    ) -> Result<(), UpdateStateError> {
        if !matches!(
            phase,
            UpdatePhase::RestartHealthCheck | UpdatePhase::Rollback | UpdatePhase::Cleanup
        ) {
            return Err(UpdateStateError::Invalid(
                "recovery failure has an invalid phase".to_string(),
            ));
        }
        let expected_phase = match stage {
            UpdateRecoveryStage::HealthPending
            | UpdateRecoveryStage::BundleSwitching
            | UpdateRecoveryStage::BundleSwitched => UpdatePhase::RestartHealthCheck,
            UpdateRecoveryStage::CleanupPending => UpdatePhase::Cleanup,
            UpdateRecoveryStage::RollbackPending => UpdatePhase::Rollback,
        };
        if phase != expected_phase {
            return Err(UpdateStateError::Invalid(
                "recovery failure has an invalid phase".to_string(),
            ));
        }
        let code = sanitize_error_code(error_code);
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking || state.pending_update.is_none() {
            return Err(UpdateStateError::Invalid(
                "recovery failure has no pending update".to_string(),
            ));
        }
        let previous = state.clone();
        state.phase = Some(phase);
        state.failure = Some(UpdateFailure { code, retry_at });
        state.restart_handoff = None;
        let existing_database_switched = state
            .recovery
            .as_ref()
            .and_then(|checkpoint| checkpoint.database_switched);
        state.recovery = Some(UpdateRecoveryCheckpoint {
            stage,
            database_switched: merge_database_switch_evidence(
                existing_database_switched,
                database_switched,
            ),
        });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn record_rollback_completed(
        &self,
        error_code: &str,
        retry_at: u64,
    ) -> Result<(), UpdateStateError> {
        let code = sanitize_error_code(error_code);
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status != UpdateStatus::Checking
            || !matches!(
                state.phase,
                Some(UpdatePhase::RestartHealthCheck)
                    | Some(UpdatePhase::Rollback)
                    | Some(UpdatePhase::Cleanup)
            )
            || state
                .pending_update
                .as_ref()
                .is_none_or(|candidate| candidate.verification_state != VerificationState::Verified)
            || state.recovery.is_none()
        {
            return Err(UpdateStateError::Invalid(
                "rollback completion has no active verified recovery".to_string(),
            ));
        }

        let previous = state.clone();
        state.status = UpdateStatus::Available;
        state.phase = None;
        state.check_started_at = None;
        state.failure = Some(UpdateFailure { code, retry_at });
        state.restart_handoff = None;
        state.recovery = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn complete_update(&self) -> Result<(), UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        if state.status == UpdateStatus::NoUpdate && state.pending_update.is_none() {
            return Ok(());
        }
        if state.status != UpdateStatus::Checking
            || state.phase != Some(UpdatePhase::Cleanup)
            || !state
                .recovery
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.stage == UpdateRecoveryStage::CleanupPending)
        {
            return Err(UpdateStateError::Invalid(
                "update completion has no cleanup checkpoint".to_string(),
            ));
        }
        let previous = state.clone();
        state.status = UpdateStatus::NoUpdate;
        state.phase = None;
        state.check_started_at = None;
        state.pending_update = None;
        state.failure = None;
        state.restart_handoff = None;
        state.recovery = None;
        state.notification = None;
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(())
    }

    fn ensure_writable(&self) -> Result<(), UpdateStateError> {
        if self.writes_blocked {
            return Err(UpdateStateError::Storage(
                "update state writes are unavailable".to_string(),
            ));
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, UpdateState>, UpdateStateError> {
        self.state
            .lock()
            .map_err(|_| UpdateStateError::LockPoisoned)
    }

    fn lock_transition(&self) -> Result<MutexGuard<'_, ()>, UpdateStateError> {
        self.transition
            .lock()
            .map_err(|_| UpdateStateError::LockPoisoned)
    }
}

fn automatic_check_is_due(state: &UpdateState, now: u64) -> bool {
    let Some(last_check_at) = state.last_check_at else {
        return true;
    };
    if now < last_check_at.saturating_add(AUTO_CHECK_INTERVAL_SECONDS) {
        return false;
    }
    state
        .failure
        .as_ref()
        .is_none_or(|failure| now >= failure.retry_at)
}

fn merge_database_switch_evidence(existing: Option<bool>, reported: Option<bool>) -> Option<bool> {
    if existing == Some(true) || reported == Some(true) {
        Some(true)
    } else if existing == Some(false) || reported == Some(false) {
        Some(false)
    } else {
        None
    }
}

fn validate_verified_archive(
    pending_update: &PendingUpdate,
    verified_archive: &VerifiedArchive,
) -> Result<PathBuf, UpdateStateError> {
    if verified_archive.artifact_id != pending_update.artifact
        || verified_archive.raw_size_bytes != pending_update.size_bytes.unwrap_or_default()
        || verified_archive.raw_sha256 != pending_update.sha256.as_deref().unwrap_or_default()
        || verified_archive.version != pending_update.version
        || verified_archive.architecture != pending_update.architecture
    {
        return Err(UpdateStateError::Invalid(
            "verified package does not match pending candidate".to_string(),
        ));
    }
    let digest = pending_update
        .sha256
        .as_deref()
        .ok_or_else(|| UpdateStateError::Invalid("pending candidate has no digest".to_string()))?;
    let expected_path = canonical_package_path(digest)?;
    if verified_archive.relative_package_path != expected_path {
        return Err(UpdateStateError::Invalid(
            "verified package path is not canonical".to_string(),
        ));
    }
    Ok(expected_path)
}

fn validate_extracted_archive(
    pending_update: &PendingUpdate,
    extracted_archive: &ExtractedArchive,
) -> Result<PathBuf, UpdateStateError> {
    if extracted_archive.artifact_id != pending_update.artifact
        || extracted_archive.raw_sha256 != pending_update.sha256.as_deref().unwrap_or_default()
        || extracted_archive.version != pending_update.version
        || extracted_archive.architecture != pending_update.architecture
    {
        return Err(UpdateStateError::Invalid(
            "extracted archive does not match pending candidate".to_string(),
        ));
    }
    let digest = pending_update
        .sha256
        .as_deref()
        .ok_or_else(|| UpdateStateError::Invalid("pending candidate has no digest".to_string()))?;
    let expected_path = canonical_extracted_app_path(digest)?;
    if extracted_archive.relative_app_path != expected_path {
        return Err(UpdateStateError::Invalid(
            "extracted app path is not canonical".to_string(),
        ));
    }
    Ok(expected_path)
}

fn interrupted_failure_code(phase: Option<UpdatePhase>) -> &'static str {
    match phase {
        Some(UpdatePhase::ManifestCheck) | None => INTERRUPTED_CHECK_ERROR_CODE,
        Some(UpdatePhase::PackageVerification) => INTERRUPTED_VERIFICATION_ERROR_CODE,
        Some(UpdatePhase::ApplyPreparation) | Some(UpdatePhase::RestartHealthCheck) => {
            INTERRUPTED_UPDATE_ERROR_CODE
        }
        Some(UpdatePhase::Rollback) | Some(UpdatePhase::Cleanup) => INTERRUPTED_ROLLBACK_ERROR_CODE,
    }
}

fn is_staged_migration_failure_code(code: &str) -> bool {
    code == "staged-migration-failed"
        || code.starts_with("staged-migration-")
        || code == "staged_migration_failed"
        || code.starts_with("staged_migration_")
}

fn recover_persisted_staged_migration_failure(state: &mut UpdateState, now: u64) -> bool {
    let is_staged_failure = state
        .failure
        .as_ref()
        .is_some_and(|failure| is_staged_migration_failure_code(&failure.code));
    let has_verified_candidate = state
        .pending_update
        .as_ref()
        .is_some_and(|candidate| candidate.verification_state == VerificationState::Verified);
    if state.status != UpdateStatus::Available
        || state.phase.is_some()
        || state.check_started_at.is_some()
        || state.restart_handoff.is_some()
        || !is_staged_failure
        || !has_verified_candidate
        || state.recovery.as_ref().is_some_and(|checkpoint| {
            checkpoint.stage != UpdateRecoveryStage::RollbackPending
                || checkpoint.database_switched != Some(false)
        })
    {
        return false;
    }

    state.status = UpdateStatus::Checking;
    state.phase = Some(UpdatePhase::Rollback);
    state.check_started_at = Some(now);
    state.recovery = Some(UpdateRecoveryCheckpoint {
        stage: UpdateRecoveryStage::RollbackPending,
        database_switched: Some(false),
    });
    true
}

fn recover_interrupted_check(state: &mut UpdateState, now: u64) -> bool {
    if state.phase == Some(UpdatePhase::ApplyPreparation)
        && state.restart_handoff == Some(UpdateRestartHandoff::Requested)
    {
        return false;
    }

    if state.phase == Some(UpdatePhase::ApplyPreparation) {
        state.phase = Some(UpdatePhase::Rollback);
        state.failure = Some(UpdateFailure {
            code: INTERRUPTED_UPDATE_ERROR_CODE.to_string(),
            retry_at: now,
        });
        state.restart_handoff = None;
        state.recovery = Some(UpdateRecoveryCheckpoint {
            stage: UpdateRecoveryStage::RollbackPending,
            database_switched: None,
        });
        return true;
    }

    if matches!(
        state.phase,
        Some(UpdatePhase::RestartHealthCheck | UpdatePhase::Rollback | UpdatePhase::Cleanup)
    ) {
        return false;
    }

    let preserve_manifest_candidate =
        state.phase == Some(UpdatePhase::ManifestCheck) && state.pending_update.is_some();
    let interrupted_code = interrupted_failure_code(state.phase);

    state.status = if preserve_manifest_candidate {
        UpdateStatus::Available
    } else {
        UpdateStatus::Failed
    };
    state.phase = None;
    state.check_started_at = None;
    if !preserve_manifest_candidate {
        state.pending_update = None;
        state.notification = None;
    }
    state.failure = Some(UpdateFailure {
        code: interrupted_code.to_string(),
        retry_at: now,
    });
    true
}

fn ensure_recovery_checkpoint(state: &mut UpdateState) -> bool {
    if state.status != UpdateStatus::Checking || state.pending_update.is_none() {
        return false;
    }
    let (stage, database_switched) = match state.phase {
        Some(UpdatePhase::RestartHealthCheck) => (UpdateRecoveryStage::HealthPending, None),
        Some(UpdatePhase::Rollback) => (UpdateRecoveryStage::RollbackPending, None),
        Some(UpdatePhase::Cleanup) => (UpdateRecoveryStage::CleanupPending, None),
        _ => return false,
    };
    if state.recovery.is_some() {
        return false;
    }
    state.recovery = Some(UpdateRecoveryCheckpoint {
        stage,
        database_switched,
    });
    true
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyPendingUpdateV1 {
    version: String,
    channel: String,
    architecture: String,
    artifact: String,
    verification_state: VerificationState,
    discovered_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyUpdateStateV1 {
    schema_version: u32,
    status: UpdateStatus,
    #[serde(default)]
    last_check_at: Option<u64>,
    #[serde(default)]
    check_started_at: Option<u64>,
    #[serde(default)]
    pending_update: Option<LegacyPendingUpdateV1>,
    #[serde(default)]
    failure: Option<UpdateFailure>,
    #[serde(default)]
    notification: Option<UpdateNotification>,
}

struct ParsedState {
    state: UpdateState,
    needs_migration: bool,
}

fn parse_state(contents: &[u8], now: u64) -> Result<ParsedState, UpdateStateLoadIssue> {
    if contents.iter().all(u8::is_ascii_whitespace) {
        return Err(UpdateStateLoadIssue::Empty);
    }

    let value: Value =
        serde_json::from_slice(contents).map_err(|_| UpdateStateLoadIssue::Invalid)?;
    let schema_version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or(UpdateStateLoadIssue::Invalid)?;

    match schema_version {
        1 => {
            let legacy = serde_json::from_value::<LegacyUpdateStateV1>(value)
                .map_err(|_| UpdateStateLoadIssue::Invalid)?;
            migrate_legacy_state(legacy, now)
                .map(|state| ParsedState {
                    state,
                    needs_migration: true,
                })
                .map_err(|_| UpdateStateLoadIssue::Invalid)
        }
        version if version == u64::from(UPDATE_STATE_SCHEMA_VERSION) => {
            let mut state = serde_json::from_value::<UpdateState>(value)
                .map_err(|_| UpdateStateLoadIssue::Invalid)?;
            let needs_migration = downgrade_unbound_verified_candidate(&mut state)
                || recover_persisted_staged_migration_failure(&mut state, now);
            state
                .validate()
                .map_err(|_| UpdateStateLoadIssue::Invalid)?;
            Ok(ParsedState {
                state,
                needs_migration,
            })
        }
        _ => Err(UpdateStateLoadIssue::UnsupportedSchema),
    }
}

fn downgrade_unbound_verified_candidate(state: &mut UpdateState) -> bool {
    let Some(pending_update) = state.pending_update.as_mut() else {
        return false;
    };
    if pending_update.verification_state != VerificationState::Verified
        || pending_update.signed_identity_sha256.is_some()
    {
        return false;
    }

    pending_update.verification_state = VerificationState::NotVerified;
    pending_update.package_path = None;
    pending_update.extracted_app_path = None;
    pending_update.verified_at = None;
    true
}

fn migrate_legacy_state(
    legacy: LegacyUpdateStateV1,
    now: u64,
) -> Result<UpdateState, UpdateStateError> {
    validate_legacy_state(&legacy)?;

    let was_checking = legacy.status == UpdateStatus::Checking;
    let pending_update = if was_checking {
        None
    } else {
        legacy.pending_update.map(|pending| {
            let _ = pending.verification_state;
            PendingUpdate::legacy(
                pending.version,
                pending.channel,
                pending.architecture,
                pending.artifact,
                pending.discovered_at,
            )
        })
    };

    let state = UpdateState {
        schema_version: UPDATE_STATE_SCHEMA_VERSION,
        status: if was_checking {
            UpdateStatus::Failed
        } else {
            legacy.status
        },
        phase: None,
        restart_handoff: None,
        last_check_at: legacy.last_check_at,
        check_started_at: None,
        pending_update,
        failure: if was_checking {
            Some(UpdateFailure {
                code: INTERRUPTED_CHECK_ERROR_CODE.to_string(),
                retry_at: now,
            })
        } else {
            legacy.failure
        },
        recovery: None,
        notification: if was_checking {
            None
        } else {
            legacy.notification
        },
    };
    state.validate()?;
    Ok(state)
}

fn validate_legacy_state(state: &LegacyUpdateStateV1) -> Result<(), UpdateStateError> {
    if state.schema_version != 1 {
        return Err(UpdateStateError::Invalid(
            "legacy update state schema version is unsupported".to_string(),
        ));
    }

    if let Some(pending_update) = &state.pending_update {
        validate_identifier(&pending_update.version, "update version")?;
        validate_identifier(&pending_update.channel, "update channel")?;
        validate_identifier(&pending_update.architecture, "update architecture")?;
        validate_identifier(&pending_update.artifact, "update artifact")?;
    }
    if let Some(failure) = &state.failure {
        validate_error_code(&failure.code)?;
    }
    if let Some(notification) = &state.notification {
        validate_identifier(&notification.version, "notified update version")?;
        validate_identifier(&notification.artifact, "notified update artifact")?;
        let Some(pending_update) = &state.pending_update else {
            return Err(UpdateStateError::Invalid(
                "update notification has no pending update".to_string(),
            ));
        };
        if pending_update.version != notification.version
            || pending_update.artifact != notification.artifact
        {
            return Err(UpdateStateError::Invalid(
                "update notification does not match pending update".to_string(),
            ));
        }
    }

    match state.status {
        UpdateStatus::NotChecked => {
            if state.check_started_at.is_some()
                || state.pending_update.is_some()
                || state.failure.is_some()
                || state.notification.is_some()
            {
                return Err(UpdateStateError::Invalid(
                    "not-checked legacy state contains a completed check result".to_string(),
                ));
            }
        }
        UpdateStatus::Checking => {
            if state.last_check_at.is_none() || state.check_started_at.is_none() {
                return Err(UpdateStateError::Invalid(
                    "checking legacy state is missing its timestamps".to_string(),
                ));
            }
            if state.failure.is_some() {
                return Err(UpdateStateError::Invalid(
                    "checking legacy state contains a failure".to_string(),
                ));
            }
        }
        UpdateStatus::NoUpdate => {
            if state.check_started_at.is_some()
                || state.pending_update.is_some()
                || state.failure.is_some()
                || state.notification.is_some()
            {
                return Err(UpdateStateError::Invalid(
                    "no-update legacy state contains another result".to_string(),
                ));
            }
        }
        UpdateStatus::Available => {
            if state.check_started_at.is_some()
                || state.pending_update.is_none()
                || state.failure.is_some()
            {
                return Err(UpdateStateError::Invalid(
                    "available legacy state is incomplete".to_string(),
                ));
            }
        }
        UpdateStatus::Failed => {
            if state.check_started_at.is_some()
                || state.failure.is_none()
                || state.pending_update.is_some()
                || state.notification.is_some()
            {
                return Err(UpdateStateError::Invalid(
                    "failed legacy state is incomplete".to_string(),
                ));
            }
        }
    }

    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), UpdateStateError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(UpdateStateError::Invalid(
            "update digest must be lowercase hexadecimal".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn canonical_package_path(sha256: &str) -> Result<PathBuf, UpdateStateError> {
    validate_sha256(sha256)?;
    Ok(PathBuf::from("packages").join(format!("{sha256}.app.tar.gz")))
}

pub(crate) fn canonical_extracted_app_path(sha256: &str) -> Result<PathBuf, UpdateStateError> {
    validate_sha256(sha256)?;
    Ok(PathBuf::from("extract").join(sha256).join("Cuetrio.app"))
}

#[derive(Clone, Copy)]
enum CleanupTargetKind {
    RegularFile,
    Directory,
}

struct CleanupTarget {
    path: PathBuf,
    kind: CleanupTargetKind,
}

fn cleanup_discarded_verified_artifacts(
    staging_directory: &Path,
    discarded_candidate: Option<&PendingUpdate>,
    replacement_candidate: Option<&PendingUpdate>,
) -> Result<(), UpdateStateError> {
    let Some(discarded_candidate) = discarded_candidate else {
        return Ok(());
    };
    if discarded_candidate.verification_state != VerificationState::Verified {
        return Ok(());
    }

    discarded_candidate.validate_paths_at(staging_directory)?;
    let replacement_paths = replacement_candidate
        .and_then(|candidate| candidate.sha256.as_deref())
        .map(|digest| {
            Ok::<_, UpdateStateError>((
                canonical_package_path(digest)?,
                canonical_extracted_app_path(digest)?,
            ))
        })
        .transpose()?;

    if !validate_cleanup_staging_root(staging_directory)? {
        return Ok(());
    }

    let package_path = discarded_candidate.package_path.as_ref().ok_or_else(|| {
        UpdateStateError::Invalid("verified update has no package path to clean".to_string())
    })?;
    let extracted_app_path = discarded_candidate
        .extracted_app_path
        .as_ref()
        .ok_or_else(|| {
            UpdateStateError::Invalid(
                "verified update has no extracted app path to clean".to_string(),
            )
        })?;

    let mut targets = Vec::with_capacity(2);
    if replacement_paths
        .as_ref()
        .is_none_or(|paths| paths.0 != *package_path)
    {
        if let Some(target) = resolve_cleanup_target(
            staging_directory,
            package_path,
            CleanupTargetKind::RegularFile,
            "package path",
        )? {
            targets.push(target);
        }
    }
    if replacement_paths
        .as_ref()
        .is_none_or(|paths| paths.1 != *extracted_app_path)
    {
        if let Some(target) = resolve_cleanup_target(
            staging_directory,
            extracted_app_path,
            CleanupTargetKind::Directory,
            "extracted app path",
        )? {
            targets.push(target);
        }
    }

    for target in targets {
        remove_cleanup_target(&target)?;
    }
    Ok(())
}

fn validate_cleanup_staging_root(staging_directory: &Path) -> Result<bool, UpdateStateError> {
    if !staging_directory.is_absolute()
        || staging_directory
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(UpdateStateError::Invalid(
            "staging directory is not a safe absolute path".to_string(),
        ));
    }

    match fs::symlink_metadata(staging_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(UpdateStateError::Invalid(
            "staging directory is a symlink".to_string(),
        )),
        Ok(metadata) if !metadata.is_dir() => Err(UpdateStateError::Invalid(
            "staging directory is not a directory".to_string(),
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(UpdateStateError::Storage(
            "staging directory could not be inspected".to_string(),
        )),
    }
}

fn resolve_cleanup_target(
    staging_directory: &Path,
    relative_path: &Path,
    kind: CleanupTargetKind,
    label: &str,
) -> Result<Option<CleanupTarget>, UpdateStateError> {
    let mut current = staging_directory.to_path_buf();
    let components: Vec<_> = relative_path.components().collect();
    if components.is_empty() {
        return Err(UpdateStateError::Invalid(format!("{label} is empty")));
    }

    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(UpdateStateError::Invalid(format!(
                "{label} contains an unsafe path component"
            )));
        };
        current.push(component);
        let is_target = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(UpdateStateError::Invalid(format!(
                    "{label} contains a symlink"
                )));
            }
            Ok(metadata) if is_target => {
                let expected = match kind {
                    CleanupTargetKind::RegularFile => metadata.is_file(),
                    CleanupTargetKind::Directory => metadata.is_dir(),
                };
                if !expected {
                    return Err(UpdateStateError::Invalid(format!(
                        "{label} has an unexpected file type"
                    )));
                }
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(UpdateStateError::Invalid(format!(
                    "{label} contains a non-directory component"
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(UpdateStateError::Storage(format!(
                    "{label} could not be inspected"
                )));
            }
        }
    }

    Ok(Some(CleanupTarget {
        path: current,
        kind,
    }))
}

fn remove_cleanup_target(target: &CleanupTarget) -> Result<(), UpdateStateError> {
    let metadata = match fs::symlink_metadata(&target.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(UpdateStateError::Storage(
                "verified staging artifact could not be inspected".to_string(),
            ));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(UpdateStateError::Invalid(
            "verified staging artifact is a symlink".to_string(),
        ));
    }

    let expected = match target.kind {
        CleanupTargetKind::RegularFile => metadata.is_file(),
        CleanupTargetKind::Directory => metadata.is_dir(),
    };
    if !expected {
        return Err(UpdateStateError::Invalid(
            "verified staging artifact has an unexpected file type".to_string(),
        ));
    }

    let result = match target.kind {
        CleanupTargetKind::RegularFile => fs::remove_file(&target.path),
        CleanupTargetKind::Directory => fs::remove_dir_all(&target.path),
    };
    result.map_err(|_| {
        UpdateStateError::Storage("verified staging artifact cleanup failed".to_string())
    })
}

fn validate_canonical_package_path(path: &Path, sha256: &str) -> Result<(), UpdateStateError> {
    validate_canonical_relative_path(path, &canonical_package_path(sha256)?, "package path")
}

fn validate_canonical_extracted_app_path(
    path: &Path,
    sha256: &str,
) -> Result<(), UpdateStateError> {
    validate_canonical_relative_path(
        path,
        &canonical_extracted_app_path(sha256)?,
        "extracted app path",
    )
}

fn validate_canonical_relative_path(
    path: &Path,
    expected: &Path,
    label: &str,
) -> Result<(), UpdateStateError> {
    let text = path
        .to_str()
        .ok_or_else(|| UpdateStateError::Invalid(format!("{label} is not valid UTF-8")))?;
    if path.is_absolute()
        || text.contains('\\')
        || text.contains("://")
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        || path != expected
    {
        return Err(UpdateStateError::Invalid(format!(
            "{label} is not the canonical relative path"
        )));
    }
    Ok(())
}

fn reject_symlink_components(
    settings_directory: &Path,
    relative_path: &Path,
    label: &str,
) -> Result<(), UpdateStateError> {
    let mut current = settings_directory.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(component) = component else {
            return Err(UpdateStateError::Invalid(format!(
                "{label} contains an unsafe path component"
            )));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(UpdateStateError::Invalid(format!(
                    "{label} contains a symlink"
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(_) => {
                return Err(UpdateStateError::Invalid(format!(
                    "{label} could not be inspected"
                )));
            }
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), UpdateStateError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_LENGTH
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
        || value.contains("://")
    {
        return Err(UpdateStateError::Invalid(format!(
            "{label} is not a safe opaque identifier"
        )));
    }
    Ok(())
}

fn validate_error_code(code: &str) -> Result<(), UpdateStateError> {
    if code.is_empty()
        || code.len() > MAX_ERROR_CODE_LENGTH
        || !code.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(UpdateStateError::Invalid(
            "update failure code is not sanitized".to_string(),
        ));
    }
    Ok(())
}

fn sanitize_error_code(error_code: &str) -> String {
    let candidate = error_code.trim().to_ascii_lowercase();
    if validate_error_code(&candidate).is_ok() {
        candidate
    } else {
        "unknown".to_string()
    }
}

pub(crate) fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn temporary_state_path(path: &Path) -> PathBuf {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = format!(
        ".{}.{}.{}.tmp",
        UPDATE_STATE_FILE_NAME,
        std::process::id(),
        counter
    );
    path.with_file_name(file_name)
}

fn open_temporary_state_file(path: &Path) -> io::Result<(File, PathBuf)> {
    for _ in 0..TEMP_FILE_ATTEMPTS {
        let temporary_path = temporary_state_path(path);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&temporary_path) {
            Ok(file) => return Ok((file, temporary_path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate an update state temporary file",
    ))
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

fn write_state_atomically(path: &Path, state: &UpdateState) -> Result<(), UpdateStateError> {
    state.validate()?;
    let parent = path.parent().ok_or_else(|| {
        UpdateStateError::Storage("update state has no parent directory".to_string())
    })?;
    if !parent.is_dir() {
        return Err(UpdateStateError::Storage(
            "update state settings directory is unavailable".to_string(),
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(UpdateStateError::Storage(
                "update state path is a symlink".to_string(),
            ));
        }
    }

    let contents = serde_json::to_vec_pretty(state)
        .map_err(|_| UpdateStateError::Storage("update state serialization failed".to_string()))?;
    let (mut temporary_file, temporary_path) =
        open_temporary_state_file(path).map_err(|error| {
            UpdateStateError::Storage(format!(
                "update state temporary file could not be opened: {error}"
            ))
        })?;

    let result = (|| -> io::Result<()> {
        temporary_file.write_all(&contents)?;
        temporary_file.write_all(b"\n")?;
        temporary_file.sync_all()?;
        fs::rename(&temporary_path, path)?;
        sync_directory(parent)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }

    result.map_err(|error| {
        UpdateStateError::Storage(format!("update state atomic write failed: {error}"))
    })
}

#[cfg(test)]
impl UpdateStateStore {
    pub(crate) fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub(crate) fn can_start_check(
        &self,
        trigger: CheckTrigger,
        now: u64,
    ) -> Result<bool, UpdateStateError> {
        self.ensure_writable()?;
        let state = self.lock_state()?;
        if state.status == UpdateStatus::Checking {
            return Ok(false);
        }
        if trigger == CheckTrigger::Manual {
            return Ok(true);
        }
        Ok(automatic_check_is_due(&state, now))
    }

    pub(crate) fn claim_pending_notification(&self, now: u64) -> Result<bool, UpdateStateError> {
        self.ensure_writable()?;
        let _transition = self.lock_transition()?;
        let mut state = self.lock_state()?;
        let Some(pending_update) = state.pending_update.clone() else {
            return Ok(false);
        };
        if state.status != UpdateStatus::Available {
            return Ok(false);
        }
        if state
            .notification
            .as_ref()
            .is_some_and(|notification| pending_update.notification_key_matches(notification))
        {
            return Ok(false);
        }

        let previous = state.clone();
        state.notification = Some(UpdateNotification {
            version: pending_update.version,
            artifact: pending_update.artifact,
            notified_at: now,
        });
        let next = state.clone();
        drop(state);
        if let Err(error) = write_state_atomically(&self.state_path, &next) {
            let mut state = self.lock_state()?;
            *state = previous;
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_TEST_DIGEST: &str =
        "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
    const TEST_SIGNED_IDENTITY: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const OTHER_TEST_SIGNED_IDENTITY: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";

    fn test_directory(label: &str) -> PathBuf {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "cuetrio-update-state-{label}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn store(label: &str) -> (PathBuf, UpdateStateStore) {
        let directory = test_directory(label);
        let staging_directory = directory.join("staging");
        fs::create_dir_all(&staging_directory).unwrap();
        let store = UpdateStateStore::load_or_default_at(&directory, &staging_directory, 100);
        (directory, store)
    }

    fn load_at(settings_directory: &Path, now: u64) -> UpdateStateStore {
        let staging_directory = settings_directory.join("staging");
        fs::create_dir_all(&staging_directory).unwrap();
        UpdateStateStore::load_or_default_at(settings_directory, &staging_directory, now)
    }

    fn cleanup(directory: &Path) {
        fs::remove_dir_all(directory).unwrap();
    }

    fn pending(version: &str, artifact: &str, discovered_at: u64) -> PendingUpdate {
        PendingUpdate::new_with_signed_identity(
            version,
            "stable",
            "aarch64-apple-darwin",
            artifact,
            1,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "test-key",
            TEST_SIGNED_IDENTITY,
            discovered_at,
        )
        .unwrap()
    }

    fn record_test_verified(store: &UpdateStateStore) {
        let verified_archive = VerifiedArchive {
            relative_package_path: canonical_package_path(TEST_DIGEST).unwrap(),
            artifact_id: "artifact".to_string(),
            raw_size_bytes: 1,
            raw_sha256: TEST_DIGEST.to_string(),
            version: "1.2.3".to_string(),
            architecture: "aarch64-apple-darwin".to_string(),
        };
        let extracted_archive = ExtractedArchive {
            relative_app_path: canonical_extracted_app_path(TEST_DIGEST).unwrap(),
            artifact_id: "artifact".to_string(),
            raw_sha256: TEST_DIGEST.to_string(),
            version: "1.2.3".to_string(),
            architecture: "aarch64-apple-darwin".to_string(),
        };
        let verified_bundle = VerifiedAppBundle {
            relative_app_path: extracted_archive.relative_app_path.clone(),
            bundle_identifier: MANIFEST_PRODUCT_ID.to_string(),
            version: "1.2.3".to_string(),
            executable_filename: "Cuetrio".to_string(),
            architecture: "aarch64-apple-darwin",
        };
        store.record_package_checkpoint(&verified_archive).unwrap();
        store
            .record_extraction_checkpoint(&extracted_archive)
            .unwrap();
        store
            .record_verified(&verified_archive, &extracted_archive, &verified_bundle, 111)
            .unwrap();
    }

    fn claim_test_staged_migration(store: &UpdateStateStore) {
        store.record_explicit_restart_handoff().unwrap();
        store.claim_staged_migration().unwrap();
    }

    fn create_test_artifacts(directory: &Path, digest: &str) -> (PathBuf, PathBuf) {
        let staging_directory = directory.join("staging");
        let package_path = staging_directory.join(canonical_package_path(digest).unwrap());
        fs::create_dir_all(package_path.parent().unwrap()).unwrap();
        fs::write(&package_path, b"verified package").unwrap();

        let extracted_app_path =
            staging_directory.join(canonical_extracted_app_path(digest).unwrap());
        fs::create_dir_all(&extracted_app_path).unwrap();
        (package_path, extracted_app_path)
    }

    #[test]
    fn ui_snapshot_has_a_fixed_allowlist_and_does_not_expose_persistence_fields() {
        let initial =
            serde_json::to_value(UpdateStateSnapshot::from(&UpdateState::initial())).unwrap();
        let initial_object = initial.as_object().unwrap();
        assert_eq!(initial_object.len(), 6);
        assert_eq!(initial["snapshotVersion"], UPDATE_STATE_SNAPSHOT_VERSION);
        assert_eq!(initial["status"], "not-checked");
        assert!(initial["lastCheckAt"].is_null());
        assert!(initial["checkStartedAt"].is_null());
        assert!(initial["pendingUpdate"].is_null());
        assert!(initial["failure"].is_null());
        for forbidden in [
            "schemaVersion",
            "notification",
            "url",
            "sizeBytes",
            "sha256",
            "signature",
            "keyId",
            "proof",
            "response",
            "headers",
            "token",
            "body",
        ] {
            assert!(
                initial.get(forbidden).is_none(),
                "forbidden field: {forbidden}"
            );
        }

        let (directory, store) = store("ui-snapshot");
        store.begin_check(CheckTrigger::Manual, 123).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 123))
            .unwrap();
        let available = serde_json::to_value(UpdateStateSnapshot::from(&store.snapshot())).unwrap();
        let pending = available["pendingUpdate"].as_object().unwrap();
        assert_eq!(pending.len(), 6);
        assert_eq!(pending["version"], "1.2.3");
        assert_eq!(pending["verificationState"], "not-verified");
        assert!(pending.get("url").is_none());
        assert!(pending.get("sha256").is_none());
        assert!(pending.get("signature").is_none());
        assert!(available.get("notification").is_none());

        cleanup(&directory);
    }

    #[test]
    fn read_only_snapshot_rejects_a_persisted_state_load_issue() {
        let directory = test_directory("read-only-snapshot-error");
        let staging_directory = directory.join("staging");
        fs::create_dir_all(&staging_directory).unwrap();
        fs::write(directory.join(UPDATE_STATE_FILE_NAME), b"{").unwrap();

        let store = UpdateStateStore::load_or_default_at(&directory, &staging_directory, 100);

        assert!(matches!(
            store.read_only_snapshot(),
            Err(UpdateStateError::Storage(_))
        ));
        cleanup(&directory);
    }

    #[test]
    fn missing_state_starts_unchecked_without_creating_a_file() {
        let (directory, store) = store("missing");

        assert_eq!(store.snapshot().status, UpdateStatus::NotChecked);
        assert_eq!(store.load_issue(), None);
        assert!(!store.state_path().exists());

        cleanup(&directory);
    }

    #[test]
    fn v1_available_state_migrates_to_v2_without_treating_old_verification_as_verified() {
        let directory = test_directory("migrate-available");
        let path = directory.join(UPDATE_STATE_FILE_NAME);
        fs::write(
            &path,
            r#"{
              "schemaVersion":1,
              "status":"available",
              "lastCheckAt":100,
              "pendingUpdate":{
                "version":"1.2.3",
                "channel":"stable",
                "architecture":"aarch64-apple-darwin",
                "artifact":"artifact",
                "verificationState":"verified",
                "discoveredAt":101
              },
              "notification":{
                "version":"1.2.3",
                "artifact":"artifact",
                "notifiedAt":102
              }
            }"#,
        )
        .unwrap();

        let store = load_at(&directory, 200);
        assert_eq!(store.load_issue(), None);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.schema_version, 2);
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert_eq!(snapshot.phase, None);
        let pending = snapshot.pending_update.unwrap();
        assert_eq!(pending.verification_state, VerificationState::NotVerified);
        assert_eq!(pending.size_bytes, None);
        assert_eq!(pending.sha256, None);
        assert_eq!(pending.key_id, None);
        assert_eq!(pending.package_path, None);
        assert_eq!(pending.extracted_app_path, None);
        assert_eq!(pending.verified_at, None);
        assert!(snapshot.notification.is_some());

        let migrated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(migrated["schemaVersion"], 2);
        assert_eq!(migrated["phase"], Value::Null);
        assert_eq!(
            migrated["pendingUpdate"]["verificationState"],
            "not-verified"
        );
        assert!(migrated["pendingUpdate"]["sizeBytes"].is_null());
        assert!(migrated["pendingUpdate"]["sha256"].is_null());
        assert!(migrated["pendingUpdate"]["keyId"].is_null());

        cleanup(&directory);
    }

    #[test]
    fn v1_checking_state_migrates_to_a_retryable_interrupted_failure() {
        let directory = test_directory("migrate-checking");
        let path = directory.join(UPDATE_STATE_FILE_NAME);
        fs::write(
            &path,
            r#"{
              "schemaVersion":1,
              "status":"checking",
              "lastCheckAt":100,
              "checkStartedAt":150
            }"#,
        )
        .unwrap();

        let store = load_at(&directory, 200);
        let snapshot = store.snapshot();
        assert_eq!(snapshot.schema_version, 2);
        assert_eq!(snapshot.status, UpdateStatus::Failed);
        assert_eq!(snapshot.phase, None);
        assert_eq!(snapshot.check_started_at, None);
        assert_eq!(snapshot.failure.as_ref().unwrap().code, "check-interrupted");
        assert_eq!(snapshot.failure.as_ref().unwrap().retry_at, 200);

        let migrated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(migrated["schemaVersion"], 2);
        assert_eq!(migrated["status"], "failed");
        assert_eq!(migrated["failure"]["code"], "check-interrupted");

        cleanup(&directory);
    }

    #[test]
    fn v2_verified_state_without_signed_identity_is_downgraded_before_restore() {
        let (directory, store) = store("missing-signed-identity");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (package_path, extracted_app_path) = create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);

        let path = store.state_path().to_path_buf();
        let mut state_json: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        state_json["pendingUpdate"]
            .as_object_mut()
            .unwrap()
            .remove("signedIdentitySha256");
        fs::write(&path, serde_json::to_vec(&state_json).unwrap()).unwrap();
        drop(store);

        let migrated = load_at(&directory, 200);
        assert_eq!(migrated.load_issue(), None);
        let pending = migrated.snapshot().pending_update.unwrap();
        assert_eq!(pending.verification_state, VerificationState::NotVerified);
        assert_eq!(pending.package_path, None);
        assert_eq!(pending.extracted_app_path, None);
        assert_eq!(pending.verified_at, None);
        assert!(package_path.exists());
        assert!(extracted_app_path.exists());

        let persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["pendingUpdate"]["verificationState"],
            "not-verified"
        );
        assert!(persisted["pendingUpdate"]["packagePath"].is_null());
        assert!(persisted["pendingUpdate"]["extractedAppPath"].is_null());
        assert!(persisted["pendingUpdate"]
            .get("signedIdentitySha256")
            .is_none());

        cleanup(&directory);
    }

    #[test]
    fn migration_write_failure_keeps_the_original_v1_file() {
        let directory = test_directory("migration-write-failure");
        let path = directory.join(UPDATE_STATE_FILE_NAME);
        let original = br#"{
          "schemaVersion":1,
          "status":"not-checked"
        }"#;
        fs::write(&path, original).unwrap();

        let next_counter = TEMP_FILE_COUNTER.load(Ordering::Relaxed);
        for counter in next_counter..next_counter + 4096 {
            let temporary_path = directory.join(format!(
                ".{}.{}.{}.tmp",
                UPDATE_STATE_FILE_NAME,
                std::process::id(),
                counter
            ));
            fs::write(temporary_path, b"collision").unwrap();
        }

        let store = load_at(&directory, 200);
        assert_eq!(
            store.load_issue(),
            Some(UpdateStateLoadIssue::MigrationWrite)
        );
        assert_eq!(
            store.read_only_snapshot().unwrap().status,
            UpdateStatus::NotChecked
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(store.begin_check(CheckTrigger::Manual, 201).is_err());

        cleanup(&directory);
    }

    #[test]
    fn v2_rejects_unknown_fields_phase_mismatch_digest_mismatch_and_unsafe_paths() {
        let cases = [
            ("unknown-v2-field", "unexpected", serde_json::json!(true)),
            ("phase-on-available", "phase", serde_json::json!("manifest-check")),
            (
                "digest-mismatch",
                "pendingUpdate.packagePath",
                serde_json::json!("packages/ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff.app.tar.gz"),
            ),
            (
                "absolute-path",
                "pendingUpdate.packagePath",
                serde_json::json!("/tmp/package.app.tar.gz"),
            ),
        ];

        for (label, field, value) in cases {
            let (directory, source_store) = store(label);
            source_store.begin_check(CheckTrigger::Manual, 100).unwrap();
            source_store
                .record_available(pending("1.2.3", "artifact", 101))
                .unwrap();
            let mut state: Value =
                serde_json::from_slice(&fs::read(source_store.state_path()).unwrap()).unwrap();
            if let Some((parent, child)) = field.split_once('.') {
                state[parent][child] = value;
            } else {
                state[field] = value;
            }
            fs::write(
                source_store.state_path(),
                serde_json::to_vec_pretty(&state).unwrap(),
            )
            .unwrap();

            let loaded = load_at(&directory, 200);
            assert_eq!(
                loaded.load_issue(),
                Some(UpdateStateLoadIssue::Invalid),
                "{label}"
            );
            assert_eq!(loaded.snapshot().status, UpdateStatus::NotChecked);
            cleanup(&directory);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_state_file_fails_closed_without_following_or_replacing_the_target() {
        use std::os::unix::fs::symlink;

        let directory = test_directory("state-symlink");
        let target = directory.join("target.json");
        let path = directory.join(UPDATE_STATE_FILE_NAME);
        fs::write(
            &target,
            b"{\"schemaVersion\":2,\"status\":\"not-checked\",\"phase\":null}",
        )
        .unwrap();
        symlink(&target, &path).unwrap();

        let store = load_at(&directory, 100);
        assert_eq!(store.load_issue(), Some(UpdateStateLoadIssue::Symlink));
        assert_eq!(store.snapshot().status, UpdateStatus::NotChecked);
        assert!(store.begin_check(CheckTrigger::Manual, 101).is_err());
        assert!(path.is_symlink());
        assert_eq!(
            fs::read(&target).unwrap(),
            b"{\"schemaVersion\":2,\"status\":\"not-checked\",\"phase\":null}"
        );

        cleanup(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn verified_checkpoint_is_not_restored_when_staging_component_becomes_symlink() {
        use std::os::unix::fs::symlink;

        for component in ["packages", "extract"] {
            let directory = test_directory(&format!("staging-{component}-symlink"));
            let staging_directory = directory.join("staging");
            fs::create_dir_all(&staging_directory).unwrap();
            let store = UpdateStateStore::load_or_default_at(&directory, &staging_directory, 100);
            store.begin_check(CheckTrigger::Manual, 100).unwrap();
            store
                .record_available(pending("1.2.3", "artifact", 100))
                .unwrap();
            store
                .begin_package_verification(&pending("1.2.3", "artifact", 100), 101)
                .unwrap();
            record_test_verified(&store);

            let state_path = directory.join(UPDATE_STATE_FILE_NAME);
            assert_eq!(store.state_path(), state_path.as_path());
            assert_eq!(store.snapshot().status, UpdateStatus::Available);
            assert_eq!(
                store.snapshot().pending_update.unwrap().verification_state,
                VerificationState::Verified
            );
            drop(store);

            let target = directory.join(format!("{component}-target"));
            fs::create_dir_all(&target).unwrap();
            symlink(&target, staging_directory.join(component)).unwrap();

            let loaded = UpdateStateStore::load_or_default_at(&directory, &staging_directory, 200);
            assert_eq!(loaded.load_issue(), Some(UpdateStateLoadIssue::Invalid));
            assert_eq!(loaded.snapshot().status, UpdateStatus::NotChecked);
            assert_eq!(loaded.snapshot().pending_update, None);

            cleanup(&directory);
        }
    }

    #[cfg(unix)]
    #[test]
    fn interrupted_apply_preparation_with_staging_symlink_becomes_recovery_state() {
        use std::os::unix::fs::symlink;

        let directory = test_directory("apply-staging-symlink");
        let staging_directory = directory.join("staging");
        fs::create_dir_all(&staging_directory).unwrap();
        let store = UpdateStateStore::load_or_default_at(&directory, &staging_directory, 100);
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 101).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 102).unwrap();
        drop(store);

        fs::remove_dir_all(staging_directory.join("packages")).unwrap();
        let target = directory.join("packages-target");
        fs::create_dir_all(&target).unwrap();
        symlink(&target, staging_directory.join("packages")).unwrap();

        let loaded = UpdateStateStore::load_or_default_at(&directory, &staging_directory, 200);
        assert_eq!(loaded.load_issue(), None);
        assert_eq!(loaded.snapshot().status, UpdateStatus::Checking);
        assert_eq!(loaded.snapshot().phase, Some(UpdatePhase::Rollback));
        assert_eq!(
            loaded.snapshot().failure.as_ref().unwrap().code,
            INTERRUPTED_UPDATE_ERROR_CODE
        );
        assert!(!loaded.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[test]
    fn empty_corrupt_and_unknown_schema_are_fail_safe() {
        for (label, contents, expected_issue) in [
            ("empty", "  \n", UpdateStateLoadIssue::Empty),
            ("corrupt", "{not-json", UpdateStateLoadIssue::Invalid),
            (
                "unknown-schema",
                r#"{"schemaVersion":99,"status":"not-checked"}"#,
                UpdateStateLoadIssue::UnsupportedSchema,
            ),
            (
                "unknown-field",
                r#"{"schemaVersion":1,"status":"not-checked","unexpected":true}"#,
                UpdateStateLoadIssue::Invalid,
            ),
        ] {
            let directory = test_directory(label);
            let path = directory.join(UPDATE_STATE_FILE_NAME);
            fs::write(&path, contents).unwrap();

            let store = load_at(&directory, 100);
            assert_eq!(store.snapshot().status, UpdateStatus::NotChecked);
            assert_eq!(store.load_issue(), Some(expected_issue));

            cleanup(&directory);
        }
    }

    #[test]
    fn unknown_schema_preserves_the_original_file_and_blocks_state_writes() {
        let directory = test_directory("unknown-schema-write-block");
        let path = directory.join(UPDATE_STATE_FILE_NAME);
        let original = br#"{
          "schemaVersion":99,
          "status":"not-checked",
          "futureState":{"verifiedPackage":"preserved"}
        }
"#;
        fs::write(&path, original).unwrap();

        let store = load_at(&directory, 100);
        assert_eq!(
            store.load_issue(),
            Some(UpdateStateLoadIssue::UnsupportedSchema)
        );
        assert_eq!(fs::read(&path).unwrap(), original);

        for trigger in [CheckTrigger::Automatic, CheckTrigger::Manual] {
            assert!(matches!(
                store.begin_check(trigger, 101),
                Err(UpdateStateError::Storage(message))
                    if message == "update state writes are unavailable"
            ));
        }
        assert!(matches!(
            store.record_no_update(),
            Err(UpdateStateError::Storage(message))
                if message == "update state writes are unavailable"
        ));
        assert_eq!(fs::read(&path).unwrap(), original);

        cleanup(&directory);
    }

    #[test]
    fn non_file_state_path_is_a_read_failure_and_falls_back_without_panic() {
        let directory = test_directory("read-failure");
        let path = directory.join(UPDATE_STATE_FILE_NAME);
        fs::create_dir(&path).unwrap();

        let store = load_at(&directory, 100);
        assert_eq!(store.snapshot().status, UpdateStatus::NotChecked);
        assert_eq!(store.load_issue(), Some(UpdateStateLoadIssue::Io));

        cleanup(&directory);
    }

    #[test]
    fn automatic_check_is_limited_to_once_per_day_and_manual_check_overrides_it() {
        let (directory, store) = store("daily");

        assert!(store.can_start_check(CheckTrigger::Automatic, 0).unwrap());
        assert_eq!(
            store.begin_check(CheckTrigger::Automatic, 0).unwrap(),
            CheckStart::Started
        );
        store.record_no_update().unwrap();

        assert!(!store
            .can_start_check(CheckTrigger::Automatic, AUTO_CHECK_INTERVAL_SECONDS - 1)
            .unwrap());
        assert_eq!(
            store
                .begin_check(CheckTrigger::Automatic, AUTO_CHECK_INTERVAL_SECONDS - 1)
                .unwrap(),
            CheckStart::Suppressed
        );
        assert!(store.can_start_check(CheckTrigger::Manual, 1).unwrap());
        assert_eq!(
            store.begin_check(CheckTrigger::Manual, 1).unwrap(),
            CheckStart::Started
        );
        store.record_no_update().unwrap();

        cleanup(&directory);
    }

    #[test]
    fn failed_check_waits_for_both_daily_limit_and_retry_time_but_manual_can_retry() {
        let (directory, store) = store("retry");

        assert_eq!(
            store.begin_check(CheckTrigger::Manual, 0).unwrap(),
            CheckStart::Started
        );
        store
            .record_failure("Network timeout: secret", AUTO_CHECK_INTERVAL_SECONDS)
            .unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Failed);
        assert_eq!(snapshot.failure.as_ref().unwrap().code, "unknown");
        assert_eq!(
            snapshot.failure.as_ref().unwrap().retry_at,
            AUTO_CHECK_INTERVAL_SECONDS
        );

        assert!(!store
            .can_start_check(CheckTrigger::Automatic, AUTO_CHECK_INTERVAL_SECONDS - 1)
            .unwrap());
        assert!(store
            .can_start_check(CheckTrigger::Automatic, AUTO_CHECK_INTERVAL_SECONDS)
            .unwrap());

        assert_eq!(
            store.begin_check(CheckTrigger::Manual, 10).unwrap(),
            CheckStart::Started
        );
        assert_eq!(
            store.begin_check(CheckTrigger::Manual, 11).unwrap(),
            CheckStart::AlreadyChecking
        );
        store.record_no_update().unwrap();

        cleanup(&directory);
    }

    #[test]
    fn same_pending_update_is_not_claimed_for_notification_twice() {
        let (directory, store) = store("notification");
        let update = pending("1.2.3", "arm64-package", 10);

        store.begin_check(CheckTrigger::Manual, 10).unwrap();
        store.record_available(update.clone()).unwrap();
        assert!(store.claim_pending_notification(11).unwrap());
        assert!(!store.claim_pending_notification(12).unwrap());

        store.begin_check(CheckTrigger::Manual, 13).unwrap();
        store.record_available(update).unwrap();
        assert!(!store.claim_pending_notification(14).unwrap());

        store.begin_check(CheckTrigger::Manual, 15).unwrap();
        store
            .record_available(pending("1.2.3", "arm64-package-new", 15))
            .unwrap();
        assert!(store.claim_pending_notification(16).unwrap());

        cleanup(&directory);
    }

    #[test]
    fn same_candidate_identity_preserves_verified_evidence_but_refreshes_discovery_time() {
        let (directory, store) = store("candidate-identity");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (package_path, extracted_app_path) = create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);

        let verified_before = store.snapshot().pending_update.unwrap();
        assert_eq!(
            verified_before.package_path,
            Some(PathBuf::from(
                "packages/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.app.tar.gz"
            ))
        );
        assert_eq!(
            verified_before.extracted_app_path,
            Some(PathBuf::from(
                "extract/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/Cuetrio.app"
            ))
        );

        store.begin_check(CheckTrigger::Manual, 200).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 201))
            .unwrap();
        let verified_after = store.snapshot().pending_update.unwrap();
        assert_eq!(
            verified_after.verification_state,
            VerificationState::Verified
        );
        assert_eq!(verified_after.verified_at, Some(111));
        assert_eq!(verified_after.discovered_at, 201);
        assert_eq!(verified_after.package_path, verified_before.package_path);
        assert_eq!(
            verified_after.extracted_app_path,
            verified_before.extracted_app_path
        );
        assert!(package_path.exists());
        assert!(extracted_app_path.exists());

        cleanup(&directory);
    }

    #[test]
    fn provider_recheck_failure_preserves_verified_candidate_and_clears_on_success() {
        let (directory, store) = store("provider-recheck-failure");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        record_test_verified(&store);
        let verified_before = store.snapshot().pending_update.unwrap();

        store.begin_check(CheckTrigger::Manual, 200).unwrap();
        store
            .record_failure(
                "Provider timeout: sensitive detail",
                200 + AUTO_CHECK_INTERVAL_SECONDS,
            )
            .unwrap();

        let failed_recheck = store.snapshot();
        assert_eq!(failed_recheck.status, UpdateStatus::Available);
        assert_eq!(failed_recheck.phase, None);
        assert_eq!(failed_recheck.check_started_at, None);
        assert_eq!(failed_recheck.pending_update, Some(verified_before.clone()));
        let failure = failed_recheck.failure.as_ref().expect("provider failure");
        assert_eq!(failure.code, "unknown");
        assert_eq!(failure.retry_at, 200 + AUTO_CHECK_INTERVAL_SECONDS);
        assert!(!store
            .can_start_check(
                CheckTrigger::Automatic,
                200 + AUTO_CHECK_INTERVAL_SECONDS - 1,
            )
            .unwrap());
        assert!(store
            .can_start_check(CheckTrigger::Automatic, 200 + AUTO_CHECK_INTERVAL_SECONDS)
            .unwrap());

        let snapshot = serde_json::to_value(UpdateStateSnapshot::from(&failed_recheck)).unwrap();
        assert_eq!(snapshot["status"], "available");
        assert_eq!(snapshot["pendingUpdate"]["verificationState"], "verified");
        assert_eq!(snapshot["failure"]["code"], "unknown");

        store.begin_check(CheckTrigger::Manual, 300).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 301))
            .unwrap();
        let recovered = store.snapshot();
        assert_eq!(recovered.status, UpdateStatus::Available);
        assert_eq!(recovered.failure, None);
        let pending = recovered.pending_update.unwrap();
        assert_eq!(pending.verification_state, VerificationState::Verified);
        assert_eq!(pending.package_path, verified_before.package_path);
        assert_eq!(
            pending.extracted_app_path,
            verified_before.extracted_app_path
        );
        assert_eq!(pending.verified_at, verified_before.verified_at);

        cleanup(&directory);
    }

    #[test]
    fn changed_candidate_identity_returns_to_not_verified_without_old_paths() {
        let (directory, store) = store("candidate-changed");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (old_package_path, old_extracted_app_path) =
            create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);

        store.begin_check(CheckTrigger::Manual, 200).unwrap();
        let changed = PendingUpdate::new_with_signed_identity(
            "1.2.3",
            "stable",
            "aarch64-apple-darwin",
            "artifact",
            2,
            "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            "new-key",
            OTHER_TEST_SIGNED_IDENTITY,
            201,
        )
        .unwrap();
        let (new_package_path, new_extracted_app_path) =
            create_test_artifacts(&directory, OTHER_TEST_DIGEST);
        let outside_sentinel = directory.join("outside-sentinel");
        fs::write(&outside_sentinel, b"keep").unwrap();
        store.record_available(changed).unwrap();

        let pending = store.snapshot().pending_update.unwrap();
        assert_eq!(pending.verification_state, VerificationState::NotVerified);
        assert_eq!(pending.package_path, None);
        assert_eq!(pending.extracted_app_path, None);
        assert_eq!(pending.verified_at, None);
        assert!(!old_package_path.exists());
        assert!(!old_extracted_app_path.exists());
        assert!(new_package_path.exists());
        assert!(new_extracted_app_path.exists());
        assert!(outside_sentinel.exists());

        cleanup(&directory);
    }

    #[test]
    fn changed_signed_identity_drops_verified_evidence_but_keeps_safe_cache() {
        let (directory, store) = store("signed-identity-changed");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (old_package_path, old_extracted_app_path) =
            create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);

        store.begin_check(CheckTrigger::Manual, 200).unwrap();
        let mut changed = pending("1.2.3", "artifact", 201);
        changed.signed_identity_sha256 = Some(OTHER_TEST_SIGNED_IDENTITY.to_string());
        store.record_available(changed).unwrap();

        let pending = store.snapshot().pending_update.unwrap();
        assert_eq!(pending.verification_state, VerificationState::NotVerified);
        assert_eq!(pending.package_path, None);
        assert_eq!(pending.extracted_app_path, None);
        assert_eq!(pending.verified_at, None);
        assert!(old_package_path.exists());
        assert!(old_extracted_app_path.exists());

        cleanup(&directory);
    }

    #[test]
    fn replace_available_candidate_cleans_only_the_discarded_verified_artifact() {
        let (directory, store) = store("replace-candidate-cleanup");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (old_package_path, old_extracted_app_path) =
            create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);

        let replacement = PendingUpdate::new_with_signed_identity(
            "1.2.4",
            "stable",
            "aarch64-apple-darwin",
            "artifact-new",
            2,
            OTHER_TEST_DIGEST,
            "new-key",
            OTHER_TEST_SIGNED_IDENTITY,
            200,
        )
        .unwrap();
        let (new_package_path, new_extracted_app_path) =
            create_test_artifacts(&directory, OTHER_TEST_DIGEST);
        let outside_sentinel = directory.join("outside-sentinel");
        fs::write(&outside_sentinel, b"keep").unwrap();

        store.replace_available_candidate(replacement).unwrap();

        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert_eq!(snapshot.pending_update.unwrap().version, "1.2.4");
        assert!(!old_package_path.exists());
        assert!(!old_extracted_app_path.exists());
        assert!(new_package_path.exists());
        assert!(new_extracted_app_path.exists());
        assert!(outside_sentinel.exists());

        cleanup(&directory);
    }

    #[test]
    fn record_no_update_cleans_the_discarded_verified_artifact() {
        let (directory, store) = store("no-update-cleanup");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (old_package_path, old_extracted_app_path) =
            create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        store.begin_check(CheckTrigger::Manual, 200).unwrap();
        let outside_sentinel = directory.join("outside-sentinel");
        fs::write(&outside_sentinel, b"keep").unwrap();

        store.record_no_update().unwrap();

        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::NoUpdate);
        assert_eq!(snapshot.pending_update, None);
        assert!(!old_package_path.exists());
        assert!(!old_extracted_app_path.exists());
        assert!(outside_sentinel.exists());

        cleanup(&directory);
    }

    #[test]
    fn record_revalidated_no_update_cleans_the_discarded_verified_artifact() {
        let (directory, store) = store("revalidated-no-update-cleanup");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (old_package_path, old_extracted_app_path) =
            create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let outside_sentinel = directory.join("outside-sentinel");
        fs::write(&outside_sentinel, b"keep").unwrap();

        store.record_revalidated_no_update().unwrap();

        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::NoUpdate);
        assert_eq!(snapshot.pending_update, None);
        assert!(!old_package_path.exists());
        assert!(!old_extracted_app_path.exists());
        assert!(outside_sentinel.exists());

        cleanup(&directory);
    }

    #[test]
    fn unsafe_discarded_verified_path_fails_closed_without_replacing_state() {
        let (directory, store) = store("unsafe-cleanup-path");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (old_package_path, old_extracted_app_path) =
            create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let outside_sentinel = directory.join("outside-sentinel");
        fs::write(&outside_sentinel, b"keep").unwrap();
        {
            let mut state = store.state.lock().unwrap();
            state.pending_update.as_mut().unwrap().package_path =
                Some(PathBuf::from("../outside-sentinel"));
        }

        let result = store.record_revalidated_no_update();

        assert!(matches!(result, Err(UpdateStateError::Invalid(_))));
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert!(snapshot.pending_update.is_some());
        assert!(old_package_path.exists());
        assert!(old_extracted_app_path.exists());
        assert!(outside_sentinel.exists());

        cleanup(&directory);
    }

    #[test]
    fn unexpected_discarded_verified_file_type_fails_closed_before_any_delete() {
        let (directory, store) = store("unexpected-cleanup-type");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (package_path, extracted_app_path) = create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        fs::remove_file(&package_path).unwrap();
        fs::create_dir(&package_path).unwrap();

        let result = store.record_revalidated_no_update();

        assert!(matches!(result, Err(UpdateStateError::Invalid(_))));
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert!(snapshot.pending_update.is_some());
        assert!(package_path.is_dir());
        assert!(extracted_app_path.exists());

        cleanup(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_discarded_verified_artifact_fails_closed_without_following_target() {
        use std::os::unix::fs::symlink;

        let (directory, store) = store("symlink-cleanup-target");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        let (package_path, extracted_app_path) = create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);

        let outside_directory = directory.join("outside");
        fs::create_dir_all(&outside_directory).unwrap();
        let outside_package_path = outside_directory.join("package");
        fs::write(&outside_package_path, b"keep").unwrap();
        fs::remove_file(&package_path).unwrap();
        symlink(&outside_package_path, &package_path).unwrap();

        let result = store.record_revalidated_no_update();

        assert!(matches!(result, Err(UpdateStateError::Invalid(_))));
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert!(snapshot.pending_update.is_some());
        assert!(package_path.is_symlink());
        assert!(outside_package_path.exists());
        assert!(extracted_app_path.exists());

        cleanup(&directory);
    }

    #[test]
    fn atomic_write_preserves_a_valid_state_boundary_and_no_forbidden_payload_fields() {
        let (directory, store) = store("atomic");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.0.1", "artifact", 101))
            .unwrap();

        let contents = fs::read_to_string(store.state_path()).unwrap();
        let value: Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(value["schemaVersion"], UPDATE_STATE_SCHEMA_VERSION);
        assert_eq!(value["status"], "available");
        assert!(value.get("url").is_none());
        assert!(value.get("token").is_none());
        assert!(value.get("database").is_none());
        assert!(value.get("notes").is_none());
        assert!(value.get("backup").is_none());

        cleanup(&directory);
    }

    #[test]
    fn write_failure_does_not_stop_state_store_or_mutate_in_memory_state() {
        let (directory, store) = store("write-failure");
        fs::create_dir(store.state_path()).unwrap();

        let result = store.begin_check(CheckTrigger::Manual, 100);
        assert!(matches!(result, Err(UpdateStateError::Storage(_))));
        assert_eq!(store.snapshot().status, UpdateStatus::NotChecked);
        assert!(store.state_path().is_dir());

        cleanup(&directory);
    }

    #[test]
    fn checking_state_recovers_to_retryable_failure_after_restart() {
        let (directory, first_store) = store("recovery");
        first_store.begin_check(CheckTrigger::Manual, 100).unwrap();

        let second_store = load_at(&directory, 200);
        let snapshot = second_store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Failed);
        assert_eq!(
            snapshot.failure.as_ref().unwrap().code,
            INTERRUPTED_CHECK_ERROR_CODE
        );
        assert_eq!(snapshot.failure.as_ref().unwrap().retry_at, 200);
        assert!(second_store
            .can_start_check(CheckTrigger::Manual, 201)
            .unwrap());

        cleanup(&directory);
    }

    #[test]
    fn manifest_check_recovery_preserves_verified_candidate_and_notification() {
        let (directory, first_store) = store("manifest-check-recovery");
        first_store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        first_store.record_available(candidate.clone()).unwrap();
        first_store
            .begin_package_verification(&candidate, 110)
            .unwrap();
        record_test_verified(&first_store);
        assert!(first_store.claim_pending_notification(120).unwrap());

        let verified_before = first_store.snapshot().pending_update.clone().unwrap();
        let notification_before = first_store.snapshot().notification.clone();
        first_store.begin_check(CheckTrigger::Manual, 200).unwrap();
        assert_eq!(
            first_store.snapshot().phase,
            Some(UpdatePhase::ManifestCheck)
        );
        drop(first_store);

        let second_store = load_at(&directory, 300);
        let snapshot = second_store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert_eq!(snapshot.phase, None);
        assert_eq!(snapshot.check_started_at, None);
        assert_eq!(snapshot.pending_update, Some(verified_before));
        assert_eq!(snapshot.notification, notification_before);
        assert_eq!(
            snapshot.failure.as_ref().unwrap().code,
            INTERRUPTED_CHECK_ERROR_CODE
        );
        assert_eq!(snapshot.failure.as_ref().unwrap().retry_at, 300);

        let ui_snapshot = serde_json::to_value(UpdateStateSnapshot::from(&snapshot)).unwrap();
        assert_eq!(ui_snapshot["status"], "available");
        assert_eq!(
            ui_snapshot["pendingUpdate"]["verificationState"],
            "verified"
        );
        assert!(!second_store
            .can_start_check(
                CheckTrigger::Automatic,
                200 + AUTO_CHECK_INTERVAL_SECONDS - 1,
            )
            .unwrap());
        assert!(second_store
            .can_start_check(CheckTrigger::Manual, 301)
            .unwrap());

        cleanup(&directory);
    }

    #[test]
    fn package_verification_phase_recovers_with_a_distinct_sanitized_failure_code() {
        let (directory, first_store) = store("verification-recovery");
        first_store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        first_store.record_available(candidate.clone()).unwrap();
        first_store
            .begin_package_verification(&candidate, 110)
            .unwrap();
        drop(first_store);

        let second_store = load_at(&directory, 200);
        let snapshot = second_store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Failed);
        assert_eq!(snapshot.phase, None);
        assert_eq!(snapshot.pending_update, None);
        assert_eq!(
            snapshot.failure.as_ref().unwrap().code,
            "verification-interrupted"
        );
        assert_eq!(snapshot.failure.as_ref().unwrap().retry_at, 200);

        cleanup(&directory);
    }

    #[test]
    fn apply_preparation_requires_a_verified_candidate() {
        let (directory, store) = store("apply-requires-verified");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate.clone()).unwrap();

        assert!(matches!(
            store.begin_apply_preparation(&candidate, 110),
            Err(UpdateStateError::Invalid(_))
        ));
        assert_eq!(store.snapshot().status, UpdateStatus::Available);
        assert_eq!(
            store.snapshot().pending_update.unwrap().verification_state,
            VerificationState::NotVerified
        );

        cleanup(&directory);
    }

    #[test]
    fn apply_preparation_write_failure_preserves_the_available_verified_state() {
        let (directory, store) = store("apply-write-failure");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate).unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let before = store.snapshot();
        fs::remove_file(store.state_path()).unwrap();
        fs::create_dir(store.state_path()).unwrap();

        assert!(matches!(
            store.begin_apply_preparation(before.pending_update.as_ref().unwrap(), 120),
            Err(UpdateStateError::Storage(_))
        ));
        assert_eq!(store.snapshot(), before);

        cleanup(&directory);
    }

    #[test]
    fn apply_preparation_recovery_keeps_verified_candidate_and_current_app() {
        let (directory, first_store) = store("apply-recovery");
        first_store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        first_store.record_available(candidate).unwrap();
        let candidate = first_store.snapshot().pending_update.unwrap();
        first_store
            .begin_package_verification(&candidate, 110)
            .unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&first_store);
        let verified = first_store.snapshot().pending_update.clone().unwrap();
        first_store.begin_apply_preparation(&verified, 120).unwrap();
        drop(first_store);

        let second_store = load_at(&directory, 200);
        let snapshot = second_store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Checking);
        assert_eq!(snapshot.phase, Some(UpdatePhase::Rollback));
        assert_eq!(snapshot.pending_update, Some(verified));
        assert_eq!(
            snapshot.failure.as_ref().unwrap().code,
            "update-interrupted"
        );
        assert!(!second_store.has_pending_apply_preparation());
        let persisted: Value =
            serde_json::from_slice(&fs::read(second_store.state_path()).unwrap()).unwrap();
        assert_eq!(persisted["phase"], "rollback");
        assert_eq!(persisted["failure"]["code"], "update-interrupted");

        cleanup(&directory);
    }

    #[test]
    fn explicit_restart_handoff_preserves_apply_and_claims_staged_migration_once() {
        let (directory, first_store) = store("explicit-restart-handoff");
        first_store.begin_check(CheckTrigger::Manual, 100).unwrap();
        first_store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = first_store.snapshot().pending_update.unwrap();
        first_store
            .begin_package_verification(&candidate, 110)
            .unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&first_store);
        let verified = first_store.snapshot().pending_update.unwrap();
        first_store.begin_apply_preparation(&verified, 120).unwrap();
        first_store.record_explicit_restart_handoff().unwrap();
        assert!(first_store.has_pending_apply_preparation());
        drop(first_store);

        let second_store = load_at(&directory, 200);
        assert_eq!(
            second_store.snapshot().restart_handoff,
            Some(UpdateRestartHandoff::Requested)
        );
        assert_eq!(
            second_store.snapshot().phase,
            Some(UpdatePhase::ApplyPreparation)
        );
        assert!(second_store.has_pending_apply_preparation());
        second_store.claim_staged_migration().unwrap();
        assert_eq!(
            second_store.snapshot().restart_handoff,
            Some(UpdateRestartHandoff::MigrationStarted)
        );
        assert!(second_store.has_pending_apply_preparation());
        assert!(second_store.claim_staged_migration().is_err());
        drop(second_store);

        let interrupted = load_at(&directory, 201);
        assert_eq!(interrupted.snapshot().phase, Some(UpdatePhase::Rollback));
        assert_eq!(
            interrupted.snapshot().failure.as_ref().unwrap().code,
            INTERRUPTED_UPDATE_ERROR_CODE
        );
        assert_eq!(
            interrupted.snapshot().recovery.as_ref().unwrap().stage,
            UpdateRecoveryStage::RollbackPending
        );
        assert!(!interrupted.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[test]
    fn staged_migration_failure_is_persisted_as_a_retryable_typed_state() {
        let (directory, store) = store("staged-migration-failure");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate).unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.clone().unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);

        store
            .record_staged_migration_failure("staged-migration-failed", 200)
            .unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Checking);
        assert_eq!(snapshot.phase, Some(UpdatePhase::Rollback));
        assert_eq!(snapshot.check_started_at, Some(120));
        assert_eq!(snapshot.pending_update, Some(verified.clone()));
        assert_eq!(
            snapshot.failure.as_ref().unwrap().code,
            "staged-migration-failed"
        );
        assert_eq!(snapshot.failure.as_ref().unwrap().retry_at, 200);
        assert_eq!(
            snapshot.recovery.as_ref().unwrap().database_switched,
            Some(false)
        );

        drop(store);
        let reloaded = load_at(&directory, 201);
        assert_eq!(reloaded.snapshot().status, UpdateStatus::Checking);
        assert_eq!(reloaded.snapshot().phase, Some(UpdatePhase::Rollback));
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));
        assert_eq!(
            reloaded.snapshot().failure.as_ref().unwrap().code,
            "staged-migration-failed"
        );
        assert_eq!(
            reloaded
                .snapshot()
                .recovery
                .as_ref()
                .unwrap()
                .database_switched,
            Some(false)
        );
        assert!(!reloaded.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[test]
    fn rollback_completion_reloads_as_failure_retained_terminal_state() {
        let (directory, store) = store("rollback-completion-terminal-state");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);
        store
            .record_staged_migration_failure("candidate-health-failed", 200)
            .unwrap();
        store
            .record_recovery_failure(
                UpdatePhase::Rollback,
                UpdateRecoveryStage::RollbackPending,
                Some(true),
                "update-rollback-failed",
                201,
            )
            .unwrap();
        let rollback_failed = store.snapshot();
        assert_eq!(rollback_failed.status, UpdateStatus::Checking);
        assert_eq!(rollback_failed.phase, Some(UpdatePhase::Rollback));
        assert_eq!(
            rollback_failed.recovery.as_ref().unwrap().stage,
            UpdateRecoveryStage::RollbackPending
        );
        assert_eq!(
            rollback_failed.failure.as_ref().unwrap().code,
            "update-rollback-failed"
        );
        assert_eq!(
            rollback_failed.recovery.as_ref().unwrap().database_switched,
            Some(true)
        );

        store
            .record_rollback_completed("update-rollback-failed", 201)
            .unwrap();
        let completed = store.snapshot();
        assert_eq!(completed.status, UpdateStatus::Available);
        assert_eq!(completed.phase, None);
        assert_eq!(completed.check_started_at, None);
        assert_eq!(completed.pending_update, Some(verified.clone()));
        assert_eq!(completed.recovery, None);
        assert_eq!(completed.restart_handoff, None);
        assert_eq!(
            completed.failure.as_ref().unwrap().code,
            "update-rollback-failed"
        );
        assert_eq!(completed.failure.as_ref().unwrap().retry_at, 201);
        drop(store);

        let reloaded = load_at(&directory, 202);
        assert_eq!(reloaded.snapshot().status, UpdateStatus::Available);
        assert_eq!(reloaded.snapshot().phase, None);
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));
        assert_eq!(reloaded.snapshot().recovery, None);
        assert_eq!(
            reloaded.snapshot().failure.as_ref().unwrap().code,
            "update-rollback-failed"
        );
        assert!(reloaded
            .can_start_check(CheckTrigger::Automatic, 100 + AUTO_CHECK_INTERVAL_SECONDS)
            .unwrap());
        assert_eq!(
            reloaded.begin_check(CheckTrigger::Manual, 300).unwrap(),
            CheckStart::Started
        );

        cleanup(&directory);
    }

    #[test]
    fn staged_migration_failure_survives_missing_staging_artifact() {
        let (directory, store) = store("staged-migration-failure-missing-artifact");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);
        store
            .record_staged_migration_failure("staged-migration-runner-failed", 200)
            .unwrap();
        drop(store);

        fs::remove_dir_all(directory.join("staging").join("packages")).unwrap();
        let reloaded = load_at(&directory, 201);
        assert_eq!(reloaded.load_issue(), None);
        assert_eq!(reloaded.snapshot().phase, Some(UpdatePhase::Rollback));
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));
        assert_eq!(
            reloaded.snapshot().failure.as_ref().unwrap().code,
            "staged-migration-runner-failed"
        );
        assert!(!reloaded.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn staged_migration_failure_survives_symlinked_staging_artifact() {
        use std::os::unix::fs::symlink;

        let (directory, store) = store("staged-migration-failure-symlink");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);
        store
            .record_staged_migration_failure("staged-migration-source-invalid", 200)
            .unwrap();
        drop(store);

        let staging_directory = directory.join("staging");
        fs::remove_dir_all(staging_directory.join("packages")).unwrap();
        let target = directory.join("staged-package-target");
        fs::create_dir_all(&target).unwrap();
        symlink(&target, staging_directory.join("packages")).unwrap();

        let reloaded = load_at(&directory, 201);
        assert_eq!(reloaded.load_issue(), None);
        assert_eq!(reloaded.snapshot().phase, Some(UpdatePhase::Rollback));
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));
        assert_eq!(
            reloaded.snapshot().failure.as_ref().unwrap().code,
            "staged-migration-source-invalid"
        );
        assert!(!reloaded.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[test]
    fn legacy_available_staged_failure_is_upgraded_before_path_validation() {
        let (directory, store) = store("legacy-staged-migration-failure");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);
        store
            .record_staged_migration_failure("staged-migration-failed", 200)
            .unwrap();

        let mut legacy: Value =
            serde_json::from_slice(&fs::read(store.state_path()).unwrap()).unwrap();
        legacy["status"] = Value::String("available".to_string());
        legacy["phase"] = Value::Null;
        legacy["checkStartedAt"] = Value::Null;
        fs::write(
            store.state_path(),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        drop(store);

        fs::remove_dir_all(directory.join("staging").join("packages")).unwrap();
        let reloaded = load_at(&directory, 201);
        assert_eq!(reloaded.load_issue(), None);
        assert_eq!(reloaded.snapshot().status, UpdateStatus::Checking);
        assert_eq!(reloaded.snapshot().phase, Some(UpdatePhase::Rollback));
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));
        assert!(!reloaded.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[test]
    fn staged_migration_no_pending_keeps_restart_health_checkpoint() {
        let (directory, store) = store("staged-migration-no-pending");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);

        store.record_staged_migration_no_pending().unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Checking);
        assert_eq!(snapshot.phase, Some(UpdatePhase::RestartHealthCheck));
        assert_eq!(snapshot.pending_update, Some(verified.clone()));
        assert_eq!(snapshot.failure, None);
        assert_eq!(
            snapshot.recovery.as_ref().unwrap().stage,
            UpdateRecoveryStage::HealthPending
        );
        assert_eq!(
            snapshot.recovery.as_ref().unwrap().database_switched,
            Some(false)
        );
        assert!(!store.has_pending_apply_preparation());

        drop(store);
        let reloaded = load_at(&directory, 201);
        assert_eq!(reloaded.snapshot().status, UpdateStatus::Checking);
        assert_eq!(
            reloaded.snapshot().phase,
            Some(UpdatePhase::RestartHealthCheck)
        );
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));
        assert!(!reloaded.has_pending_apply_preparation());

        cleanup(&directory);
    }

    #[test]
    fn staged_migration_success_keeps_restart_health_checkpoint_after_switch() {
        let (directory, store) = store("staged-migration-success");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        let candidate = pending("1.2.3", "artifact", 100);
        store.record_available(candidate).unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.clone().unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);

        store.record_staged_migration_switched().unwrap();
        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Checking);
        assert_eq!(snapshot.phase, Some(UpdatePhase::RestartHealthCheck));
        assert_eq!(snapshot.pending_update, Some(verified.clone()));
        assert_eq!(snapshot.failure, None);
        assert_eq!(
            snapshot.recovery.as_ref().unwrap().stage,
            UpdateRecoveryStage::HealthPending
        );
        assert_eq!(
            snapshot.recovery.as_ref().unwrap().database_switched,
            Some(true)
        );
        assert!(!store.has_pending_apply_preparation());

        drop(store);
        let reloaded = load_at(&directory, 201);
        assert_eq!(reloaded.snapshot().status, UpdateStatus::Checking);
        assert_eq!(
            reloaded.snapshot().phase,
            Some(UpdatePhase::RestartHealthCheck)
        );
        assert_eq!(reloaded.snapshot().pending_update, Some(verified));

        cleanup(&directory);
    }

    #[test]
    fn recovery_checkpoint_transitions_are_idempotent_and_complete_atomically() {
        let (directory, store) = store("recovery-checkpoint-transitions");
        store.begin_check(CheckTrigger::Manual, 100).unwrap();
        store
            .record_available(pending("1.2.3", "artifact", 100))
            .unwrap();
        let candidate = store.snapshot().pending_update.unwrap();
        store.begin_package_verification(&candidate, 110).unwrap();
        create_test_artifacts(&directory, TEST_DIGEST);
        record_test_verified(&store);
        let verified = store.snapshot().pending_update.unwrap();
        store.begin_apply_preparation(&verified, 120).unwrap();
        claim_test_staged_migration(&store);
        store.record_staged_migration_no_pending().unwrap();

        store.record_bundle_switching().unwrap();
        store.record_bundle_switching().unwrap();
        assert_eq!(
            store.snapshot().recovery.as_ref().unwrap().stage,
            UpdateRecoveryStage::BundleSwitching
        );
        store.record_bundle_switched().unwrap();
        store.record_bundle_switched().unwrap();
        store.record_cleanup_pending().unwrap();
        store
            .record_recovery_failure(
                UpdatePhase::Cleanup,
                UpdateRecoveryStage::CleanupPending,
                Some(false),
                "update-cleanup-failed",
                200,
            )
            .unwrap();
        assert_eq!(
            store.snapshot().failure.as_ref().unwrap().code,
            "update-cleanup-failed"
        );
        store.complete_update().unwrap();
        assert_eq!(store.snapshot().status, UpdateStatus::NoUpdate);
        assert_eq!(store.snapshot().recovery, None);

        cleanup(&directory);
    }
}

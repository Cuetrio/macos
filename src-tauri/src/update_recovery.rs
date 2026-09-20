use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::runtime::{
    packaged_runtime_root, start_sidecar, validate_database_command, StorageLayout,
};
use crate::update_apply::revalidate_staged_candidate;
use crate::update_archive::{
    normalize_relative_symlink_target, resolve_relative_symlink_path, ExtractedArchive,
    SafeSymlinkPathError, MAX_SYMLINK_HOPS,
};
use crate::update_bundle::{validate_extracted_app_bundle, VerifiedAppBundle};
use crate::update_state::{
    PendingUpdate, UpdatePhase, UpdateRecoveryStage, UpdateState, UpdateStateError,
    UpdateStateStore, VerificationState,
};

const APP_BUNDLE_NAME: &str = "Cuetrio.app";
const CONTENTS_DIRECTORY_NAME: &str = "Contents";
const MACOS_EXECUTABLE_DIRECTORY_NAME: &str = "MacOS";
const BUNDLE_ROLLBACK_PREFIX: &str = ".Cuetrio.rollback-";
const BUNDLE_SWITCH_PREFIX: &str = ".Cuetrio.switch-";
const BUNDLE_FAILED_PREFIX: &str = ".Cuetrio.failed-";
const DATABASE_RESTORE_PREFIX: &str = ".notebook.sqlite.restore-";
const DATABASE_ORIGINAL_PREFIX: &str = ".notebook.sqlite.original-";
const DATABASE_MIGRATION_DIRECTORY_NAME: &str = "database-migrations";
const DATABASE_BACKUP_PREFIX: &str = "notebook-";
const DATABASE_BACKUP_SUFFIX: &str = ".sqlite.bak";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryOutcome {
    Continue,
    RestartRequired,
}

#[derive(Clone, Debug)]
struct CandidateBundle {
    pending: PendingUpdate,
    source_path: PathBuf,
    runtime_path: Option<PathBuf>,
}

impl CandidateBundle {
    fn runtime_root(&self) -> Result<&Path, String> {
        self.runtime_path.as_deref().ok_or_else(|| {
            recovery_error(
                "update-candidate-invalid",
                "candidate runtime root is not available",
            )
        })
    }
}

#[derive(Clone, Debug)]
struct BundlePaths {
    current: PathBuf,
    rollback: PathBuf,
    switch_temp: PathBuf,
    failed: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RestoreTemporaryPaths {
    restore_temp: PathBuf,
    original_temp: PathBuf,
}

pub(crate) fn run_startup_update_recovery(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
) -> Result<RecoveryOutcome, String> {
    let state = state_store.snapshot();
    if state.status != crate::update_state::UpdateStatus::Checking {
        return Ok(RecoveryOutcome::Continue);
    }

    let result = match state.phase {
        Some(UpdatePhase::RestartHealthCheck) => {
            recover_restart_health(root, storage, state_store, &state)
        }
        Some(UpdatePhase::Rollback) => recover_rollback(root, storage, state_store, &state),
        Some(UpdatePhase::Cleanup) => recover_cleanup(root, storage, state_store, &state),
        _ => Ok(RecoveryOutcome::Continue),
    };
    if let Err(error) = &result {
        record_unhandled_recovery_failure(state_store, &state, error)?;
    }
    result
}

fn record_unhandled_recovery_failure(
    state_store: &UpdateStateStore,
    _state: &UpdateState,
    error: &str,
) -> Result<(), String> {
    let current = state_store.snapshot();
    if current.failure.is_some() {
        return Ok(());
    }
    let (phase, stage) = match current.phase {
        Some(UpdatePhase::Cleanup) => (UpdatePhase::Cleanup, UpdateRecoveryStage::CleanupPending),
        Some(UpdatePhase::Rollback) => {
            (UpdatePhase::Rollback, UpdateRecoveryStage::RollbackPending)
        }
        Some(UpdatePhase::RestartHealthCheck) => {
            (UpdatePhase::Rollback, UpdateRecoveryStage::RollbackPending)
        }
        _ => return Ok(()),
    };
    let code = if error.contains("candidate") {
        "update-candidate-invalid"
    } else if error.contains("path") || error.contains("bundle") {
        "update-path-unresolved"
    } else {
        "update-recovery-failed"
    };
    state_store
        .record_recovery_failure(
            phase,
            stage,
            current
                .recovery
                .as_ref()
                .and_then(|checkpoint| checkpoint.database_switched),
            code,
            current_timestamp(),
        )
        .map_err(|state_error| state_transition_error("unhandled recovery failure", state_error))
}

fn recover_restart_health(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
) -> Result<RecoveryOutcome, String> {
    let candidate = candidate_from_state(state, storage)?;
    let paths = resolve_bundle_paths(root, storage, &candidate.pending)?;
    let stage = state
        .recovery
        .as_ref()
        .map(|checkpoint| checkpoint.stage)
        .unwrap_or(UpdateRecoveryStage::HealthPending);

    match stage {
        UpdateRecoveryStage::HealthPending => {
            revalidate_staged_candidate(&candidate.pending, &storage.staging_directory())?;
            require_safe_bundle_tree(&paths.current, "current app bundle")?;
            if let Err(error) = run_candidate_health(candidate.runtime_root()?, storage) {
                return rollback_after_health_failure(
                    root,
                    storage,
                    state_store,
                    state,
                    &paths,
                    error,
                );
            }
            state_store
                .record_bundle_switching()
                .map_err(|error| state_transition_error("bundle switching", error))?;
            if let Err(error) = switch_bundle(&paths, &candidate) {
                return handle_switch_failure(
                    state_store,
                    error,
                    state
                        .recovery
                        .as_ref()
                        .and_then(|checkpoint| checkpoint.database_switched),
                );
            }
            state_store
                .record_bundle_switched()
                .map_err(|error| state_transition_error("bundle switched", error))?;
            Ok(RecoveryOutcome::RestartRequired)
        }
        UpdateRecoveryStage::BundleSwitching => {
            reconcile_bundle_switch(root, storage, state_store, state, &candidate, &paths)
        }
        UpdateRecoveryStage::BundleSwitched => {
            finish_switched_candidate(root, storage, state_store, state, &candidate, &paths)
        }
        UpdateRecoveryStage::CleanupPending => {
            finish_cleanup(storage, state_store, state, &candidate, &paths)
        }
        UpdateRecoveryStage::RollbackPending => recover_rollback(root, storage, state_store, state),
    }
}

fn reconcile_bundle_switch(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
    candidate: &CandidateBundle,
    paths: &BundlePaths,
) -> Result<RecoveryOutcome, String> {
    let current_is_candidate = is_candidate_bundle_at(&paths.current, &candidate.pending)?;
    let rollback_exists = path_exists(&paths.rollback)?;
    let temp_exists = path_exists(&paths.switch_temp)?;

    if current_is_candidate && rollback_exists {
        if temp_exists {
            return Err(recovery_error(
                "update-switch-failed",
                "bundle switch left both current and temporary candidates",
            ));
        }
        require_safe_bundle_tree(&paths.rollback, "rollback app bundle")?;
        let runtime_root = validated_candidate_runtime_root(&paths.current, &candidate.pending)?;
        if let Err(error) = run_candidate_health(&runtime_root, storage) {
            return rollback_after_health_failure(root, storage, state_store, state, paths, error);
        }
        state_store
            .record_bundle_switched()
            .map_err(|error| state_transition_error("bundle switch recovery", error))?;
        return Ok(RecoveryOutcome::RestartRequired);
    }

    if current_is_candidate && !rollback_exists {
        return Err(recovery_error(
            "update-switch-failed",
            "bundle switch checkpoint is ambiguous",
        ));
    }
    if !path_exists(&paths.current)? {
        if temp_exists && rollback_exists {
            validate_complete_switch_temp(paths, candidate)?;
            let runtime_root =
                validated_candidate_runtime_root(&paths.switch_temp, &candidate.pending)?;
            require_safe_bundle_tree(&paths.rollback, "rollback app bundle")?;
            if let Err(error) = run_candidate_health(&runtime_root, storage) {
                return rollback_after_health_failure(
                    root,
                    storage,
                    state_store,
                    state,
                    paths,
                    error,
                );
            }
            fs::rename(&paths.switch_temp, &paths.current)
                .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
            sync_directory(paths.current.parent().ok_or_else(|| {
                recovery_error("update-switch-failed", "current app bundle has no parent")
            })?)?;
            require_candidate_bundle_at(&paths.current, &candidate.pending)?;
            state_store
                .record_bundle_switched()
                .map_err(|error| state_transition_error("bundle switch recovery", error))?;
            return Ok(RecoveryOutcome::RestartRequired);
        }
        return Err(recovery_error(
            "update-switch-failed",
            "current app bundle disappeared during switch",
        ));
    }

    if rollback_exists || temp_exists {
        return Err(recovery_error(
            "update-switch-failed",
            "bundle switch left an unclassified artifact",
        ));
    }
    require_safe_bundle_tree(&paths.current, "current app bundle")?;
    if let Err(error) = run_candidate_health(candidate.runtime_root()?, storage) {
        return handle_health_failure(
            root,
            storage,
            state_store,
            state,
            paths,
            error,
            state
                .recovery
                .as_ref()
                .and_then(|checkpoint| checkpoint.database_switched),
        );
    }
    if let Err(error) = switch_bundle(paths, candidate) {
        return handle_switch_failure(
            state_store,
            error,
            state
                .recovery
                .as_ref()
                .and_then(|checkpoint| checkpoint.database_switched),
        );
    }
    state_store
        .record_bundle_switched()
        .map_err(|error| state_transition_error("bundle switch recovery", error))?;
    Ok(RecoveryOutcome::RestartRequired)
}

fn finish_switched_candidate(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
    candidate: &CandidateBundle,
    paths: &BundlePaths,
) -> Result<RecoveryOutcome, String> {
    let runtime_root = validated_candidate_runtime_root(&paths.current, &candidate.pending)?;
    if let Err(error) = run_candidate_health(&runtime_root, storage) {
        return rollback_after_health_failure(root, storage, state_store, state, paths, error);
    }
    state_store
        .record_cleanup_pending()
        .map_err(|error| state_transition_error("cleanup checkpoint", error))?;
    finish_cleanup(storage, state_store, state, candidate, paths)
}

fn recover_cleanup(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
) -> Result<RecoveryOutcome, String> {
    let mut candidate = candidate_metadata_from_state(state)?;
    candidate.source_path = candidate_source_path(&candidate.pending, storage)?;
    let paths = resolve_bundle_paths_from_current(storage, &candidate.pending)?;
    let runtime_root = validated_candidate_runtime_root(&paths.current, &candidate.pending)?;
    if let Err(error) = run_candidate_health(&runtime_root, storage) {
        return rollback_after_health_failure(root, storage, state_store, state, &paths, error);
    }
    finish_cleanup(storage, state_store, state, &candidate, &paths)
}

fn finish_cleanup(
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    _state: &UpdateState,
    candidate: &CandidateBundle,
    paths: &BundlePaths,
) -> Result<RecoveryOutcome, String> {
    match cleanup_after_success(storage, candidate, paths) {
        Ok(()) => {
            state_store
                .complete_update()
                .map_err(|error| state_transition_error("update completion", error))?;
            Ok(RecoveryOutcome::Continue)
        }
        Err(error) => {
            state_store
                .record_recovery_failure(
                    UpdatePhase::Cleanup,
                    UpdateRecoveryStage::CleanupPending,
                    candidate_database_switched(state_store),
                    "update-cleanup-failed",
                    current_timestamp(),
                )
                .map_err(|state_error| state_transition_error("cleanup failure", state_error))?;
            eprintln!("desktop update cleanup deferred: {error}");
            Ok(RecoveryOutcome::Continue)
        }
    }
}

fn recover_rollback(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
) -> Result<RecoveryOutcome, String> {
    let candidate = candidate_metadata_from_state(state)?;
    let paths = resolve_bundle_paths(root, storage, &candidate.pending)?;
    let current_was_candidate = is_candidate_bundle_at(&paths.current, &candidate.pending)?;
    let bundle_changed = rollback_bundle_if_needed(&paths, &candidate.pending)?;
    let database_hint = state
        .recovery
        .as_ref()
        .and_then(|checkpoint| checkpoint.database_switched);
    if let Err(error) = restore_database_if_needed(root, storage, &candidate.pending, database_hint)
    {
        state_store
            .record_recovery_failure(
                UpdatePhase::Rollback,
                UpdateRecoveryStage::RollbackPending,
                database_hint,
                "update-restore-failed",
                current_timestamp(),
            )
            .map_err(|state_error| state_transition_error("restore failure", state_error))?;
        return Err(error);
    }
    cleanup_failed_marker_before_rollback_completion(storage, state_store, &paths, database_hint)?;

    let (failure_code, retry_at) = state
        .failure
        .as_ref()
        .map_or(("update-rollback", current_timestamp()), |failure| {
            (failure.code.as_str(), failure.retry_at)
        });
    state_store
        .record_rollback_completed(failure_code, retry_at)
        .map_err(|error| state_transition_error("rollback completion", error))?;

    if bundle_changed && current_was_candidate {
        Ok(RecoveryOutcome::RestartRequired)
    } else {
        Ok(RecoveryOutcome::Continue)
    }
}

fn rollback_after_health_failure(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
    paths: &BundlePaths,
    health_error: String,
) -> Result<RecoveryOutcome, String> {
    let candidate = candidate_metadata_from_state(state)?;
    let database_hint = state
        .recovery
        .as_ref()
        .and_then(|checkpoint| checkpoint.database_switched);
    let current_was_candidate = is_candidate_bundle_at(&paths.current, &candidate.pending)?;
    let bundle_changed = match rollback_bundle_if_needed(paths, &candidate.pending) {
        Ok(changed) => changed,
        Err(error) => {
            state_store
                .record_recovery_failure(
                    UpdatePhase::Rollback,
                    UpdateRecoveryStage::RollbackPending,
                    database_hint,
                    "update-rollback-failed",
                    current_timestamp(),
                )
                .map_err(|state_error| state_transition_error("rollback failure", state_error))?;
            return Err(error);
        }
    };
    if let Err(error) = restore_database_if_needed(root, storage, &candidate.pending, database_hint)
    {
        state_store
            .record_recovery_failure(
                UpdatePhase::Rollback,
                UpdateRecoveryStage::RollbackPending,
                database_hint,
                "update-restore-failed",
                current_timestamp(),
            )
            .map_err(|state_error| state_transition_error("restore failure", state_error))?;
        return Err(error);
    }

    cleanup_failed_marker_before_rollback_completion(storage, state_store, paths, database_hint)?;
    state_store
        .record_rollback_completed("candidate-health-failed", current_timestamp())
        .map_err(|error| state_transition_error("health failure", error))?;
    eprintln!("desktop update candidate health failed: {health_error}");
    if bundle_changed && current_was_candidate {
        Ok(RecoveryOutcome::RestartRequired)
    } else {
        Ok(RecoveryOutcome::Continue)
    }
}

fn handle_health_failure(
    root: &Path,
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    state: &UpdateState,
    paths: &BundlePaths,
    health_error: String,
    database_hint: Option<bool>,
) -> Result<RecoveryOutcome, String> {
    let candidate = candidate_metadata_from_state(state)?;
    if let Err(error) = restore_database_if_needed(root, storage, &candidate.pending, database_hint)
    {
        state_store
            .record_recovery_failure(
                UpdatePhase::Rollback,
                UpdateRecoveryStage::RollbackPending,
                database_hint,
                "update-restore-failed",
                current_timestamp(),
            )
            .map_err(|state_error| state_transition_error("restore failure", state_error))?;
        return Err(error);
    }
    cleanup_failed_marker_before_rollback_completion(storage, state_store, paths, database_hint)?;
    state_store
        .record_rollback_completed("candidate-health-failed", current_timestamp())
        .map_err(|error| state_transition_error("health failure", error))?;
    eprintln!("desktop update candidate health failed: {health_error}");
    Ok(RecoveryOutcome::Continue)
}

fn cleanup_failed_marker_before_rollback_completion(
    storage: &StorageLayout,
    state_store: &UpdateStateStore,
    paths: &BundlePaths,
    database_hint: Option<bool>,
) -> Result<(), String> {
    if let Err(error) = remove_stale_restore_temporary_files(storage.live_directory()) {
        state_store
            .record_recovery_failure(
                UpdatePhase::Rollback,
                UpdateRecoveryStage::RollbackPending,
                database_hint,
                "update-restore-failed",
                current_timestamp(),
            )
            .map_err(|state_error| {
                state_transition_error("restore temporary cleanup", state_error)
            })?;
        return Err(error);
    }
    if let Err(error) = remove_failed_bundle_marker(paths) {
        state_store
            .record_recovery_failure(
                UpdatePhase::Rollback,
                UpdateRecoveryStage::RollbackPending,
                database_hint,
                "update-rollback-failed",
                current_timestamp(),
            )
            .map_err(|state_error| {
                state_transition_error("rollback marker cleanup", state_error)
            })?;
        return Err(error);
    }
    Ok(())
}

fn handle_switch_failure(
    state_store: &UpdateStateStore,
    error: String,
    database_hint: Option<bool>,
) -> Result<RecoveryOutcome, String> {
    state_store
        .record_recovery_failure(
            UpdatePhase::Rollback,
            UpdateRecoveryStage::RollbackPending,
            database_hint,
            "update-switch-failed",
            current_timestamp(),
        )
        .map_err(|state_error| state_transition_error("switch failure", state_error))?;
    Err(error)
}

fn candidate_from_state(
    state: &UpdateState,
    storage: &StorageLayout,
) -> Result<CandidateBundle, String> {
    let candidate = candidate_metadata_from_state(state)?;
    let source_path = candidate_source_path(&candidate.pending, storage)?;
    let runtime_path = validated_candidate_runtime_root(&source_path, &candidate.pending)?;
    Ok(CandidateBundle {
        pending: candidate.pending,
        source_path,
        runtime_path: Some(runtime_path),
    })
}

fn candidate_metadata_from_state(state: &UpdateState) -> Result<CandidateBundle, String> {
    let pending = state
        .pending_update
        .clone()
        .ok_or_else(|| recovery_error("update-candidate-invalid", "recovery has no candidate"))?;
    if pending.verification_state != VerificationState::Verified
        || pending.extracted_app_path.is_none()
        || pending.package_path.is_none()
        || pending.sha256.is_none()
        || pending.signed_identity_sha256.is_none()
    {
        return Err(recovery_error(
            "update-candidate-invalid",
            "recovery candidate is not fully verified",
        ));
    }
    Ok(CandidateBundle {
        pending,
        source_path: PathBuf::new(),
        runtime_path: None,
    })
}

fn candidate_source_path(
    candidate: &PendingUpdate,
    storage: &StorageLayout,
) -> Result<PathBuf, String> {
    candidate
        .validate_paths_at(&storage.staging_directory())
        .map_err(|error| recovery_error("update-candidate-path-invalid", error.to_string()))?;
    let relative = candidate.extracted_app_path.as_ref().ok_or_else(|| {
        recovery_error("update-candidate-invalid", "candidate app path is missing")
    })?;
    let path = storage.staging_directory().join(relative);
    require_absolute_without_parent(&storage.staging_directory(), "update staging directory")?;
    if path.file_name() != Some(OsStr::new(APP_BUNDLE_NAME)) {
        return Err(recovery_error(
            "update-candidate-path-invalid",
            "candidate app path has an unexpected bundle name",
        ));
    }
    Ok(path)
}

fn resolve_bundle_paths(
    root: &Path,
    storage: &StorageLayout,
    candidate: &PendingUpdate,
) -> Result<BundlePaths, String> {
    let current = resolve_current_app_bundle(root)?;
    let _candidate_source = candidate_source_path(candidate, storage)?;
    let digest = candidate
        .sha256
        .as_deref()
        .ok_or_else(|| recovery_error("update-candidate-invalid", "candidate digest is missing"))?;
    let parent = current.parent().ok_or_else(|| {
        recovery_error(
            "update-current-bundle-unresolved",
            "current app has no parent",
        )
    })?;
    require_safe_directory(parent, "current app bundle parent")?;
    Ok(BundlePaths {
        current: current.clone(),
        rollback: parent.join(format!("{BUNDLE_ROLLBACK_PREFIX}{digest}.app")),
        switch_temp: parent.join(format!("{BUNDLE_SWITCH_PREFIX}{digest}.app")),
        failed: parent.join(format!("{BUNDLE_FAILED_PREFIX}{digest}.app")),
    })
}

fn resolve_bundle_paths_from_current(
    storage: &StorageLayout,
    candidate: &PendingUpdate,
) -> Result<BundlePaths, String> {
    let current = resolve_current_app_bundle_from_executable()?;
    let digest = candidate
        .sha256
        .as_deref()
        .ok_or_else(|| recovery_error("update-candidate-invalid", "candidate digest is missing"))?;
    let parent = current.parent().ok_or_else(|| {
        recovery_error(
            "update-current-bundle-unresolved",
            "current app has no parent",
        )
    })?;
    let _candidate_source = candidate_source_path(candidate, storage)?;
    require_safe_directory(parent, "current app bundle parent")?;
    Ok(BundlePaths {
        current: current.clone(),
        rollback: parent.join(format!("{BUNDLE_ROLLBACK_PREFIX}{digest}.app")),
        switch_temp: parent.join(format!("{BUNDLE_SWITCH_PREFIX}{digest}.app")),
        failed: parent.join(format!("{BUNDLE_FAILED_PREFIX}{digest}.app")),
    })
}

fn resolve_current_app_bundle(root: &Path) -> Result<PathBuf, String> {
    let current = resolve_current_app_bundle_from_executable()?;
    let expected_runtime = packaged_runtime_root(&current)
        .map_err(|error| recovery_error("update-current-bundle-unresolved", error))?;
    if root != expected_runtime {
        return Err(recovery_error(
            "update-current-bundle-unresolved",
            "runtime root is not inside the current packaged app bundle",
        ));
    }
    require_safe_bundle_tree(&current, "current app bundle")?;
    Ok(current)
}

fn resolve_current_app_bundle_from_executable() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| recovery_error("update-current-bundle-unresolved", error.to_string()))?;
    let macos = executable.parent().ok_or_else(|| {
        recovery_error(
            "update-current-bundle-unresolved",
            "current executable has no parent",
        )
    })?;
    if macos.file_name() != Some(OsStr::new(MACOS_EXECUTABLE_DIRECTORY_NAME)) {
        return Err(recovery_error(
            "update-current-bundle-unresolved",
            "current executable is not inside Contents/MacOS",
        ));
    }
    let contents = macos.parent().ok_or_else(|| {
        recovery_error(
            "update-current-bundle-unresolved",
            "current Contents path is missing",
        )
    })?;
    if contents.file_name() != Some(OsStr::new(CONTENTS_DIRECTORY_NAME)) {
        return Err(recovery_error(
            "update-current-bundle-unresolved",
            "current executable is not inside Contents",
        ));
    }
    let bundle = contents.parent().ok_or_else(|| {
        recovery_error(
            "update-current-bundle-unresolved",
            "current bundle path is missing",
        )
    })?;
    if bundle.file_name() != Some(OsStr::new(APP_BUNDLE_NAME)) {
        return Err(recovery_error(
            "update-current-bundle-unresolved",
            "current bundle identity is not the packaged product",
        ));
    }
    validate_no_symlink_components(bundle, "current app bundle")?;
    require_safe_directory(bundle, "current app bundle")?;
    Ok(bundle.to_path_buf())
}

fn require_candidate_bundle_at(
    app_path: &Path,
    candidate: &PendingUpdate,
) -> Result<VerifiedAppBundle, String> {
    if app_path.file_name() != Some(OsStr::new(APP_BUNDLE_NAME)) {
        return Err(recovery_error(
            "update-candidate-path-invalid",
            "candidate bundle has an unexpected name",
        ));
    }
    let parent = app_path.parent().ok_or_else(|| {
        recovery_error(
            "update-candidate-path-invalid",
            "candidate bundle has no parent",
        )
    })?;
    let relative_app_path = PathBuf::from(APP_BUNDLE_NAME);
    let extracted = ExtractedArchive {
        relative_app_path,
        artifact_id: candidate.artifact.clone(),
        raw_sha256: candidate.sha256.clone().ok_or_else(|| {
            recovery_error("update-candidate-invalid", "candidate digest is missing")
        })?,
        version: candidate.version.clone(),
        architecture: candidate.architecture.clone(),
    };
    let verified = validate_extracted_app_bundle(&extracted, parent)
        .map_err(|error| recovery_error(error.code(), error.to_string()))?;
    if verified.bundle_identifier != crate::update_manifest::MANIFEST_PRODUCT_ID
        || verified.version != candidate.version
        || verified.architecture != candidate.architecture
    {
        return Err(recovery_error(
            "update-candidate-identity-invalid",
            "candidate bundle identity, version, or architecture changed",
        ));
    }
    require_safe_bundle_tree(app_path, "candidate app bundle")?;
    Ok(verified)
}

fn validated_candidate_runtime_root(
    app_path: &Path,
    candidate: &PendingUpdate,
) -> Result<PathBuf, String> {
    require_candidate_bundle_at(app_path, candidate)?;
    require_safe_bundle_tree(app_path, "candidate app bundle")?;
    packaged_runtime_root(app_path)
        .map_err(|error| recovery_error("update-candidate-path-invalid", error))
}

fn is_candidate_bundle_at(app_path: &Path, candidate: &PendingUpdate) -> Result<bool, String> {
    if !path_exists(app_path)? {
        return Ok(false);
    }
    match require_candidate_bundle_at(app_path, candidate) {
        Ok(_) => Ok(true),
        Err(error) if error.starts_with("bundle-") || error.starts_with("update-candidate-") => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn switch_bundle(paths: &BundlePaths, candidate: &CandidateBundle) -> Result<(), String> {
    require_safe_bundle_tree(&paths.current, "current app bundle")?;
    if path_exists(&paths.rollback)? {
        return Err(recovery_error(
            "update-switch-failed",
            "rollback bundle already exists",
        ));
    }
    prepare_switch_temp(paths, candidate)?;

    fs::rename(&paths.current, &paths.rollback)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    if let Err(error) = fs::rename(&paths.switch_temp, &paths.current) {
        let restore_result = if !path_exists(&paths.current)? && path_exists(&paths.rollback)? {
            fs::rename(&paths.rollback, &paths.current)
        } else {
            Err(io::Error::other(
                "current bundle is not in a restorable state",
            ))
        };
        if restore_result.is_err() {
            return Err(recovery_error(
                "update-rollback-failed",
                format!("bundle switch failed and current bundle restore failed: {error}"),
            ));
        }
        return Err(recovery_error("update-switch-failed", error.to_string()));
    }
    sync_directory(
        paths
            .current
            .parent()
            .ok_or_else(|| recovery_error("update-switch-failed", "current app has no parent"))?,
    )?;
    require_candidate_bundle_at(&paths.current, &candidate.pending)?;
    require_safe_bundle_tree(&paths.current, "current app bundle")?;
    require_safe_bundle_tree(&paths.rollback, "rollback app bundle")?;
    Ok(())
}

fn prepare_switch_temp(paths: &BundlePaths, candidate: &CandidateBundle) -> Result<(), String> {
    let should_copy = if path_exists(&paths.switch_temp)? {
        match validate_complete_switch_temp(paths, candidate) {
            Ok(()) => false,
            Err(validation_error) => {
                discard_switch_temp(paths).map_err(|cleanup_error| {
                    recovery_error(
                        "update-switch-failed",
                        format!(
                            "unverified switch temporary could not be discarded: {validation_error}; {cleanup_error}"
                        ),
                    )
                })?;
                true
            }
        }
    } else {
        true
    };

    if should_copy {
        copy_bundle_tree(&candidate.source_path, &paths.switch_temp)?;
        if let Err(validation_error) = validate_complete_switch_temp(paths, candidate) {
            let cleanup_result = discard_switch_temp(paths);
            return match cleanup_result {
                Ok(()) => Err(validation_error),
                Err(cleanup_error) => Err(recovery_error(
                    "update-switch-failed",
                    format!(
                        "copied switch temporary failed validation: {validation_error}; cleanup failed: {cleanup_error}"
                    ),
                )),
            };
        }
    }
    Ok(())
}

fn validate_complete_switch_temp(
    paths: &BundlePaths,
    candidate: &CandidateBundle,
) -> Result<(), String> {
    require_candidate_bundle_at(&candidate.source_path, &candidate.pending)?;
    require_safe_bundle_tree(&paths.switch_temp, "bundle switch temporary")?;
    require_safe_bundle_tree(&candidate.source_path, "candidate bundle source")?;
    compare_bundle_trees(
        &candidate.source_path,
        &paths.switch_temp,
        "bundle switch temporary",
    )
}

fn discard_switch_temp(paths: &BundlePaths) -> Result<(), String> {
    let parent = paths.current.parent().ok_or_else(|| {
        recovery_error(
            "update-switch-failed",
            "current app bundle has no parent for switch temporary cleanup",
        )
    })?;
    if paths.switch_temp.parent() != Some(parent) {
        return Err(recovery_error(
            "update-switch-failed",
            "switch temporary escaped the current bundle parent",
        ));
    }
    let name = paths
        .switch_temp
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            recovery_error(
                "update-switch-failed",
                "switch temporary name is not valid UTF-8",
            )
        })?;
    if !name.starts_with(BUNDLE_SWITCH_PREFIX) || !name.ends_with(".app") {
        return Err(recovery_error(
            "update-switch-failed",
            "switch temporary has an unexpected name",
        ));
    }
    remove_partial_bundle_tree(&paths.switch_temp, parent, "bundle switch temporary")
}

fn rollback_bundle_if_needed(
    paths: &BundlePaths,
    candidate: &PendingUpdate,
) -> Result<bool, String> {
    let current_exists = path_exists(&paths.current)?;
    let rollback_exists = path_exists(&paths.rollback)?;
    if rollback_exists {
        require_safe_bundle_tree(&paths.rollback, "rollback app bundle")?;
    }
    if !current_exists {
        if !rollback_exists {
            return Err(recovery_error(
                "update-rollback-failed",
                "neither current nor rollback bundle exists",
            ));
        }
        fs::rename(&paths.rollback, &paths.current)
            .map_err(|error| recovery_error("update-rollback-failed", error.to_string()))?;
        require_safe_bundle_tree(&paths.current, "restored current app bundle")?;
        return Ok(true);
    }

    let current_is_candidate = is_candidate_bundle_at(&paths.current, candidate)?;
    if !current_is_candidate {
        if rollback_exists {
            return Err(recovery_error(
                "update-rollback-failed",
                "old current bundle and rollback bundle both exist",
            ));
        }
        require_safe_bundle_tree(&paths.current, "current app bundle")?;
        return Ok(false);
    }
    if !rollback_exists {
        return Err(recovery_error(
            "update-rollback-failed",
            "candidate current bundle has no verified rollback source",
        ));
    }
    if path_exists(&paths.failed)? {
        require_safe_bundle_tree(&paths.failed, "failed candidate bundle")?;
        return Err(recovery_error(
            "update-rollback-failed",
            "failed candidate bundle marker is already occupied",
        ));
    }
    fs::rename(&paths.current, &paths.failed)
        .map_err(|error| recovery_error("update-rollback-failed", error.to_string()))?;
    if let Err(error) = fs::rename(&paths.rollback, &paths.current) {
        let restore_result = if !path_exists(&paths.current)? && path_exists(&paths.failed)? {
            fs::rename(&paths.failed, &paths.current)
        } else {
            Err(io::Error::other(
                "candidate bundle is not in a restorable state",
            ))
        };
        if restore_result.is_err() {
            return Err(recovery_error(
                "update-rollback-failed",
                format!("bundle rollback failed and candidate restore failed: {error}"),
            ));
        }
        return Err(recovery_error("update-rollback-failed", error.to_string()));
    }
    require_safe_bundle_tree(&paths.current, "restored current app bundle")?;
    sync_directory(
        paths
            .current
            .parent()
            .ok_or_else(|| recovery_error("update-rollback-failed", "current app has no parent"))?,
    )?;
    Ok(true)
}

fn run_candidate_health(runtime_root: &Path, storage: &StorageLayout) -> Result<(), String> {
    require_safe_directory(runtime_root, "candidate runtime root")?;
    let mut sidecar = start_sidecar(runtime_root, storage)
        .map_err(|error| recovery_error("candidate-health-failed", error))?;
    sidecar
        .stop()
        .map_err(|error| recovery_error("candidate-health-failed", error))?;
    validate_database_command(runtime_root, storage)
        .map_err(|error| recovery_error("candidate-health-failed", error))
}

fn database_switch_was_proven(database_hint: Option<bool>) -> bool {
    database_hint == Some(true)
}

fn restore_database_if_needed(
    root: &Path,
    storage: &StorageLayout,
    candidate: &PendingUpdate,
    database_hint: Option<bool>,
) -> Result<bool, String> {
    // A safety backup proves only that a pre-migration copy exists.  Restore is
    // authorized only when the checkpoint explicitly records the live DB
    // switch; false and legacy/unknown checkpoints are fail-closed.
    if !database_switch_was_proven(database_hint) {
        return Ok(false);
    }
    let backup = find_safety_backup(storage, candidate)?;
    let Some(backup) = backup else {
        if database_hint == Some(true) {
            return Err(recovery_error(
                "update-restore-failed",
                "database switch was recorded but no safety backup is available",
            ));
        }
        return Ok(false);
    };
    validate_database_paths(storage)?;
    let live_bytes = fs::read(storage.database_path())
        .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
    let backup_bytes = fs::read(&backup)
        .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
    if live_bytes == backup_bytes {
        let validation_result = validate_database_command(root, storage)
            .map_err(|error| recovery_error("update-restore-failed", error));
        let cleanup_result = remove_stale_restore_temporary_files(storage.live_directory());
        match (validation_result, cleanup_result) {
            (Ok(()), Ok(())) => {}
            (Err(validation_error), Ok(())) => return Err(validation_error),
            (Ok(()), Err(cleanup_error)) => return Err(cleanup_error),
            (Err(validation_error), Err(cleanup_error)) => {
                return Err(format!(
                    "{validation_error}; restore temporary cleanup failed: {cleanup_error}"
                ));
            }
        }
        return Ok(false);
    }
    let temporary_paths = atomic_restore_database(storage, &backup, &backup_bytes)?;
    let validation_result = validate_database_command(root, storage)
        .map_err(|error| recovery_error("update-restore-failed", error));
    let cleanup_result = remove_restore_temporary_files(storage.live_directory(), &temporary_paths);
    match (validation_result, cleanup_result) {
        (Ok(()), Ok(())) => {}
        (Err(validation_error), Ok(())) => return Err(validation_error),
        (Ok(()), Err(cleanup_error)) => return Err(cleanup_error),
        (Err(validation_error), Err(cleanup_error)) => {
            return Err(format!(
                "{validation_error}; restore temporary cleanup failed: {cleanup_error}"
            ));
        }
    }
    Ok(true)
}

fn atomic_restore_database(
    storage: &StorageLayout,
    backup: &Path,
    backup_bytes: &[u8],
) -> Result<RestoreTemporaryPaths, String> {
    let temporary_paths = restore_temporary_paths(storage.live_directory(), backup)?;
    let restore_result = (|| {
        let live_bytes = fs::read(storage.database_path())
            .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
        ensure_file_copy(
            &temporary_paths.original_temp,
            &live_bytes,
            "update-restore-failed",
        )?;
        ensure_file_copy(
            &temporary_paths.restore_temp,
            backup_bytes,
            "update-restore-failed",
        )?;
        fs::rename(&temporary_paths.restore_temp, storage.database_path())
            .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
        let read_back = fs::read(storage.database_path())
            .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
        if read_back != backup_bytes {
            return Err(recovery_error(
                "update-restore-failed",
                "restored SQLite read-back differs from the safety backup",
            ));
        }
        Ok(())
    })();

    match restore_result {
        Ok(()) => Ok(temporary_paths),
        Err(restore_error) => {
            let cleanup_result =
                remove_restore_temporary_files(storage.live_directory(), &temporary_paths);
            match cleanup_result {
                Ok(()) => Err(restore_error),
                Err(cleanup_error) => Err(format!(
                    "{restore_error}; restore temporary cleanup failed: {cleanup_error}"
                )),
            }
        }
    }
}

fn find_safety_backup(
    storage: &StorageLayout,
    candidate: &PendingUpdate,
) -> Result<Option<PathBuf>, String> {
    let digest = candidate
        .sha256
        .as_deref()
        .ok_or_else(|| recovery_error("update-candidate-invalid", "candidate digest is missing"))?;
    require_safe_directory(storage.backups_directory(), "managed backup directory")?;
    let prefix = format!("{DATABASE_BACKUP_PREFIX}{digest}-");
    let mut matches = Vec::new();
    for entry in fs::read_dir(storage.backups_directory())
        .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?
    {
        let entry =
            entry.map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(&prefix) || !name.ends_with(DATABASE_BACKUP_SUFFIX) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(recovery_error(
                "update-restore-failed",
                "candidate safety backup is not a regular file",
            ));
        }
        matches.push(path);
    }
    if matches.len() > 1 {
        return Err(recovery_error(
            "update-restore-failed",
            "multiple candidate safety backups make restore ambiguous",
        ));
    }
    Ok(matches.into_iter().next())
}

fn cleanup_after_success(
    storage: &StorageLayout,
    candidate: &CandidateBundle,
    paths: &BundlePaths,
) -> Result<(), String> {
    require_candidate_bundle_at(&paths.current, &candidate.pending)?;
    validate_database_paths(storage)?;
    remove_stale_restore_temporary_files(storage.live_directory())?;
    remove_if_safe_tree(&paths.rollback, "old app bundle cleanup")?;
    remove_if_safe_tree(&paths.switch_temp, "bundle temporary cleanup")?;
    remove_if_safe_tree(&paths.failed, "failed bundle cleanup")?;

    candidate
        .pending
        .validate_paths_at(&storage.staging_directory())
        .map_err(|error| recovery_error("update-cleanup-failed", error.to_string()))?;
    remove_candidate_artifact(
        &candidate.source_path,
        &storage.staging_directory(),
        "candidate bundle cleanup",
    )?;
    if let Some(package_path) = candidate.pending.package_path.as_ref() {
        let package = storage.staging_directory().join(package_path);
        remove_candidate_artifact(
            &package,
            &storage.staging_directory(),
            "candidate package cleanup",
        )?;
    }
    let migration_directory = storage
        .staging_directory()
        .join(DATABASE_MIGRATION_DIRECTORY_NAME);
    remove_if_safe_tree(&migration_directory, "database migration cleanup")?;
    if let Some(backup) = find_safety_backup(storage, &candidate.pending)? {
        remove_candidate_artifact(
            &backup,
            storage.backups_directory(),
            "migration safety backup cleanup",
        )?;
    }
    Ok(())
}

fn remove_restore_temporary_files(
    live_directory: &Path,
    temporary_paths: &RestoreTemporaryPaths,
) -> Result<(), String> {
    require_safe_directory(live_directory, "live database directory")?;
    for (path, prefix) in [
        (&temporary_paths.restore_temp, DATABASE_RESTORE_PREFIX),
        (&temporary_paths.original_temp, DATABASE_ORIGINAL_PREFIX),
    ] {
        validate_restore_temporary_path(live_directory, path, prefix)?;
        remove_if_safe_file(path, "database restore temporary cleanup")?;
    }
    sync_directory(live_directory)
        .map_err(|error| recovery_error("update-cleanup-failed", error))?;
    Ok(())
}

fn remove_stale_restore_temporary_files(live_directory: &Path) -> Result<(), String> {
    require_safe_directory(live_directory, "live database directory")?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(live_directory)
        .map_err(|error| recovery_error("update-cleanup-failed", error.to_string()))?
    {
        let entry =
            entry.map_err(|error| recovery_error("update-cleanup-failed", error.to_string()))?;
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(prefix) = restore_temporary_prefix(&name) else {
            continue;
        };
        validate_restore_temporary_path(live_directory, &path, prefix)?;
        paths.push(path);
    }

    for path in &paths {
        remove_if_safe_file(path, "database restore temporary cleanup")?;
    }
    if !paths.is_empty() {
        sync_directory(live_directory)
            .map_err(|error| recovery_error("update-cleanup-failed", error))?;
    }
    Ok(())
}

fn restore_temporary_paths(
    live_directory: &Path,
    backup: &Path,
) -> Result<RestoreTemporaryPaths, String> {
    require_safe_directory(live_directory, "live database directory")?;
    let token = safe_path_token(
        backup
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("backup"),
    );
    let token = if token.is_empty() {
        "backup".to_owned()
    } else {
        token
    };
    Ok(RestoreTemporaryPaths {
        restore_temp: live_directory.join(format!("{DATABASE_RESTORE_PREFIX}{token}.tmp")),
        original_temp: live_directory.join(format!("{DATABASE_ORIGINAL_PREFIX}{token}.tmp")),
    })
}

fn restore_temporary_prefix(name: &str) -> Option<&'static str> {
    [DATABASE_RESTORE_PREFIX, DATABASE_ORIGINAL_PREFIX]
        .into_iter()
        .find(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(".tmp"))
                .is_some_and(|token| {
                    !token.is_empty()
                        && token
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric() || character == '-')
                })
        })
}

fn validate_restore_temporary_path(
    live_directory: &Path,
    path: &Path,
    expected_prefix: &str,
) -> Result<(), String> {
    if path.parent() != Some(live_directory) {
        return Err(recovery_error(
            "update-cleanup-failed",
            "database restore temporary escaped the live database directory",
        ));
    }
    let name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        recovery_error(
            "update-cleanup-failed",
            "database restore temporary name is not valid UTF-8",
        )
    })?;
    if !name.starts_with(expected_prefix) || restore_temporary_prefix(name) != Some(expected_prefix)
    {
        return Err(recovery_error(
            "update-cleanup-failed",
            "database restore temporary has an unexpected name",
        ));
    }
    Ok(())
}

fn remove_candidate_artifact(path: &Path, root: &Path, label: &str) -> Result<(), String> {
    let Some(metadata) = validate_candidate_artifact_path(path, root, label)? else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err(recovery_error(
            "update-cleanup-failed",
            format!("{label} is not removable"),
        ));
    }
    if metadata.is_dir() {
        require_safe_bundle_tree(path, label)?;
    }
    remove_validated_tree(path, label)
}

fn validate_candidate_artifact_path(
    path: &Path,
    root: &Path,
    label: &str,
) -> Result<Option<fs::Metadata>, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(recovery_error(
            "update-cleanup-failed",
            format!("{label} is not an absolute safe path"),
        ));
    }

    // Validate the managed root before canonicalizing it.  canonicalize is
    // used only to make the root boundary explicit; all existing components
    // are still inspected with symlink_metadata so a symlink cannot be
    // accepted merely because it resolves inside the canonical root.
    let canonical_root = canonical_managed_root(root, label)?;
    if !path.starts_with(root) || !path.starts_with(&canonical_root) || path == root {
        return Err(recovery_error(
            "update-cleanup-failed",
            format!("{label} escaped its managed root"),
        ));
    }

    let relative = path.strip_prefix(&canonical_root).map_err(|_| {
        recovery_error(
            "update-cleanup-failed",
            format!("{label} escaped its canonical managed root"),
        )
    })?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return Err(recovery_error(
            "update-cleanup-failed",
            format!("{label} cannot be the managed root"),
        ));
    }

    let mut current = canonical_root;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(recovery_error(
                "update-cleanup-failed",
                format!("{label} contains an unsafe path component"),
            ));
        };
        current.push(component);
        let is_leaf = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(recovery_error(
                        "update-path-unresolved",
                        format!("{label} contains a symlink"),
                    ));
                }
                if !is_leaf && !metadata.is_dir() {
                    return Err(recovery_error(
                        "update-cleanup-failed",
                        format!("{label} contains a non-directory parent"),
                    ));
                }
                if is_leaf {
                    return Ok(Some(metadata));
                }
            }
            // A missing leaf is an idempotent cleanup success.  If an
            // intermediate component is already gone, no filesystem object
            // remains to follow either, so the same result is safe after all
            // existing parent components have been checked.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(recovery_error(
                    "update-path-unresolved",
                    format!("{label}: {error}"),
                ));
            }
        }
    }

    Ok(None)
}

fn canonical_managed_root(root: &Path, label: &str) -> Result<PathBuf, String> {
    require_absolute_without_parent(root, &format!("{label} managed root"))?;
    validate_no_symlink_components(root, &format!("{label} managed root"))?;
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        recovery_error(
            "update-cleanup-failed",
            format!("{label} managed root: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(recovery_error(
            "update-cleanup-failed",
            format!("{label} managed root is not a directory"),
        ));
    }
    fs::canonicalize(root).map_err(|error| {
        recovery_error(
            "update-cleanup-failed",
            format!("{label} managed root could not be canonicalized: {error}"),
        )
    })
}

fn remove_failed_bundle_marker(paths: &BundlePaths) -> Result<(), String> {
    let parent = paths.current.parent().ok_or_else(|| {
        recovery_error(
            "update-rollback-failed",
            "current app bundle has no parent for failed marker cleanup",
        )
    })?;
    if paths.failed.parent() != Some(parent) {
        return Err(recovery_error(
            "update-rollback-failed",
            "failed bundle marker escaped the current bundle parent",
        ));
    }
    let marker_name = paths
        .failed
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            recovery_error(
                "update-rollback-failed",
                "failed bundle marker name is not valid UTF-8",
            )
        })?;
    if !marker_name.starts_with(BUNDLE_FAILED_PREFIX) || !marker_name.ends_with(".app") {
        return Err(recovery_error(
            "update-rollback-failed",
            "failed bundle marker has an unexpected name",
        ));
    }
    require_safe_directory(parent, "failed bundle marker parent")
        .map_err(|error| recovery_error("update-rollback-failed", error))?;
    if !path_exists(&paths.failed)
        .map_err(|error| recovery_error("update-rollback-failed", error))?
    {
        return Ok(());
    }
    require_safe_bundle_tree(&paths.failed, "failed candidate bundle marker")
        .map_err(|error| recovery_error("update-rollback-failed", error))?;
    remove_validated_tree(&paths.failed, "failed candidate bundle marker")
        .map_err(|error| recovery_error("update-rollback-failed", error))?;
    sync_directory(parent).map_err(|error| recovery_error("update-rollback-failed", error))
}

fn remove_if_safe_tree(path: &Path, label: &str) -> Result<(), String> {
    if !path_exists(path)? {
        return Ok(());
    }
    require_safe_bundle_tree(path, label)?;
    remove_validated_tree(path, label)
}

fn remove_if_safe_file(path: &Path, label: &str) -> Result<(), String> {
    if !path_exists(path)? {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error("update-cleanup-failed", error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(recovery_error(
            "update-cleanup-failed",
            format!("{label} is not a regular file"),
        ));
    }
    fs::remove_file(path)
        .map_err(|error| recovery_error("update-cleanup-failed", error.to_string()))
}

fn remove_partial_bundle_tree(path: &Path, parent: &Path, label: &str) -> Result<(), String> {
    if path.parent() != Some(parent) {
        return Err(recovery_error(
            "update-switch-failed",
            format!("{label} escaped its managed parent"),
        ));
    }
    require_safe_directory(parent, &format!("{label} parent"))?;
    if !path_exists(path)? {
        return Ok(());
    }
    // The root is required to be a real directory.  Descendant symlinks are
    // removed as links only; cleanup never follows them.
    validate_no_symlink_components(path, label)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(recovery_error(
            "update-switch-failed",
            format!("{label} is not a removable directory"),
        ));
    }
    validate_tree_for_no_follow_removal(path, label)?;
    remove_tree_no_follow(path, label, "update-switch-failed")?;
    sync_directory(parent)
}

fn validate_tree_for_no_follow_removal(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?
        {
            let child = entry
                .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?
                .path();
            validate_tree_for_no_follow_removal(&child, label)?;
        }
        return Ok(());
    }
    Err(recovery_error(
        "update-switch-failed",
        format!("{label} contains a special file"),
    ))
}

fn remove_validated_tree(path: &Path, label: &str) -> Result<(), String> {
    remove_tree_no_follow(path, label, "update-cleanup-failed")
}

fn remove_tree_no_follow(path: &Path, label: &str, failure_code: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error(failure_code, error.to_string()))?;
    if metadata.file_type().is_symlink() {
        fs::remove_file(path)
            .map_err(|error| recovery_error(failure_code, format!("{label}: {error}")))?;
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in
            fs::read_dir(path).map_err(|error| recovery_error(failure_code, error.to_string()))?
        {
            let child = entry
                .map_err(|error| recovery_error(failure_code, error.to_string()))?
                .path();
            remove_tree_no_follow(&child, label, failure_code)?;
        }
        fs::remove_dir(path).map_err(|error| recovery_error(failure_code, error.to_string()))?;
    } else if metadata.is_file() {
        fs::remove_file(path).map_err(|error| recovery_error(failure_code, error.to_string()))?;
    } else {
        return Err(recovery_error(
            failure_code,
            format!("{label} contains a special file"),
        ));
    }
    Ok(())
}

fn copy_bundle_tree(source: &Path, destination: &Path) -> Result<(), String> {
    require_safe_bundle_tree(source, "candidate bundle source")?;
    if path_exists(destination)? {
        return Err(recovery_error(
            "update-switch-failed",
            "bundle switch temporary path exists",
        ));
    }
    let parent = destination.parent().ok_or_else(|| {
        recovery_error(
            "update-switch-failed",
            "bundle temporary path has no parent",
        )
    })?;
    require_safe_directory(parent, "bundle switch temporary parent")?;
    match copy_tree_preserving_safe_symlinks(source, destination) {
        Ok(()) => Ok(()),
        Err(copy_error) => {
            let cleanup_result =
                remove_partial_bundle_tree(destination, parent, "bundle switch temporary");
            match cleanup_result {
                Ok(()) => Err(copy_error),
                Err(cleanup_error) => Err(format!(
                    "{copy_error}; partial bundle cleanup failed: {cleanup_error}"
                )),
            }
        }
    }
}

fn copy_tree_preserving_safe_symlinks(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        create_recovery_symlink(&target, destination)?;
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir(destination)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        for entry in fs::read_dir(source)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?
        {
            let entry =
                entry.map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
            copy_tree_preserving_safe_symlinks(
                &entry.path(),
                &destination.join(entry.file_name()),
            )?;
        }
    } else if metadata.is_file() {
        copy_regular_file_no_follow(source, destination)?;
    } else {
        return Err(recovery_error(
            "update-switch-failed",
            "candidate bundle contains a special file",
        ));
    }
    Ok(())
}

fn compare_bundle_trees(source: &Path, destination: &Path, label: &str) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    let destination_metadata = fs::symlink_metadata(destination)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    let source_type = source_metadata.file_type();
    let destination_type = destination_metadata.file_type();

    if source_type.is_symlink() || destination_type.is_symlink() {
        if !source_type.is_symlink() || !destination_type.is_symlink() {
            return Err(recovery_error(
                "update-switch-failed",
                format!("{label} entry type differs from candidate source"),
            ));
        }
        let source_target = fs::read_link(source)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        let destination_target = fs::read_link(destination)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        if source_target != destination_target {
            return Err(recovery_error(
                "update-switch-failed",
                format!("{label} symlink target differs from candidate source"),
            ));
        }
        return Ok(());
    }

    if source_type.is_dir() || destination_type.is_dir() {
        if !source_type.is_dir() || !destination_type.is_dir() {
            return Err(recovery_error(
                "update-switch-failed",
                format!("{label} entry type differs from candidate source"),
            ));
        }
        for entry in fs::read_dir(source)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?
        {
            let entry =
                entry.map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
            let source_child = entry.path();
            let destination_child = destination.join(entry.file_name());
            if !path_exists(&destination_child)? {
                return Err(recovery_error(
                    "update-switch-failed",
                    format!("{label} is missing a candidate source entry"),
                ));
            }
            compare_bundle_trees(&source_child, &destination_child, label)?;
        }
        for entry in fs::read_dir(destination)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?
        {
            let entry =
                entry.map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
            let source_child = source.join(entry.file_name());
            if !path_exists(&source_child)? {
                return Err(recovery_error(
                    "update-switch-failed",
                    format!("{label} contains an extra entry"),
                ));
            }
        }
        return Ok(());
    }

    if source_type.is_file() || destination_type.is_file() {
        if !source_type.is_file() || !destination_type.is_file() {
            return Err(recovery_error(
                "update-switch-failed",
                format!("{label} entry type differs from candidate source"),
            ));
        }
        return compare_regular_file_bytes(source, destination, label);
    }

    Err(recovery_error(
        "update-switch-failed",
        format!("{label} contains a special file"),
    ))
}

fn compare_regular_file_bytes(
    source: &Path,
    destination: &Path,
    label: &str,
) -> Result<(), String> {
    let source_metadata = fs::symlink_metadata(source)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    let destination_metadata = fs::symlink_metadata(destination)
        .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    if source_metadata.len() != destination_metadata.len() {
        return Err(recovery_error(
            "update-switch-failed",
            format!("{label} file size differs from candidate source"),
        ));
    }

    let mut source_file = open_regular_file_no_follow(source)?;
    let mut destination_file = open_regular_file_no_follow(destination)?;
    let mut source_buffer = [0_u8; 16 * 1024];
    let mut destination_buffer = [0_u8; 16 * 1024];
    loop {
        let source_read = source_file
            .read(&mut source_buffer)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        let destination_read = destination_file
            .read(&mut destination_buffer)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        if source_read != destination_read
            || source_buffer[..source_read] != destination_buffer[..destination_read]
        {
            return Err(recovery_error(
                "update-switch-failed",
                format!("{label} file contents differ from candidate source"),
            ));
        }
        if source_read == 0 {
            return Ok(());
        }
    }
}

fn open_regular_file_no_follow(path: &Path) -> Result<File, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))
    }
}

fn create_recovery_symlink(target: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, destination)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, destination)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, destination);
        Err(recovery_error(
            "update-switch-failed",
            "safe internal symlinks are unsupported on this platform",
        ))
    }
}

fn copy_regular_file_no_follow(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        let source_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        let source_metadata = source_file
            .metadata()
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        if !source_metadata.is_file() {
            return Err(recovery_error(
                "update-switch-failed",
                "candidate bundle contains a special file",
            ));
        }
        let mut destination_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(destination)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        let mut source_file = source_file;
        io::copy(&mut source_file, &mut destination_file)
            .and_then(|_| destination_file.sync_all())
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        fs::set_permissions(destination, source_metadata.permissions())
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))
    }
    #[cfg(not(unix))]
    {
        fs::copy(source, destination)
            .map(|_| ())
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))
    }
}

fn require_safe_bundle_tree(path: &Path, label: &str) -> Result<(), String> {
    validate_no_symlink_components(path, label)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error("update-path-unresolved", format!("{label}: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(recovery_error(
            "update-path-unresolved",
            format!("{label} is not a directory"),
        ));
    }
    validate_tree_with_safe_symlinks(path, path, label)
}

fn validate_tree_with_safe_symlinks(
    path: &Path,
    bundle_root: &Path,
    label: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error("update-path-unresolved", format!("{label}: {error}")))?;
    if metadata.file_type().is_symlink() {
        return validate_internal_symlink(path, bundle_root, label);
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|error| recovery_error("update-path-unresolved", error.to_string()))?
        {
            validate_tree_with_safe_symlinks(
                &entry
                    .map_err(|error| recovery_error("update-path-unresolved", error.to_string()))?
                    .path(),
                bundle_root,
                label,
            )?;
        }
    } else if !metadata.is_file() {
        return Err(recovery_error(
            "update-path-unresolved",
            format!("{label} contains a special file"),
        ));
    }
    Ok(())
}

fn validate_internal_symlink(
    link_path: &Path,
    bundle_root: &Path,
    label: &str,
) -> Result<(), String> {
    let root_name = bundle_root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            unsafe_internal_symlink_error(label, "bundle root name is not valid UTF-8")
        })?;
    let mut current = link_path.to_path_buf();
    let mut visited = HashSet::new();
    let mut hops = 0_usize;

    loop {
        let current_archive_path = bundle_archive_path(bundle_root, &current, label)?;
        if !visited.insert(current_archive_path.clone()) {
            return Err(unsafe_internal_symlink_error(
                label,
                "symlink chain contains a cycle",
            ));
        }
        if hops >= MAX_SYMLINK_HOPS {
            return Err(unsafe_internal_symlink_error(
                label,
                "symlink chain exceeds the maximum hop count",
            ));
        }

        let raw_target = fs::read_link(&current).map_err(|error| {
            recovery_error(
                "update-path-unresolved",
                format!("{label} symlink target cannot be read: {error}"),
            )
        })?;
        let target = raw_target.to_str().ok_or_else(|| {
            unsafe_internal_symlink_error(label, "symlink target is not valid UTF-8")
        })?;
        let normalized_target = normalize_relative_symlink_target(target).map_err(|error| {
            unsafe_internal_symlink_error(label, safe_symlink_path_error_message(error))
        })?;
        let resolved_archive_path =
            resolve_relative_symlink_path(&current_archive_path, &normalized_target, root_name)
                .map_err(|error| {
                    unsafe_internal_symlink_error(label, safe_symlink_path_error_message(error))
                })?;
        let target_path =
            bundle_path_from_archive_path(bundle_root, &resolved_archive_path, label)?;
        let metadata = metadata_no_follow_inside_bundle(bundle_root, &target_path, label)?;
        if metadata.file_type().is_symlink() {
            current = target_path;
            hops += 1;
            continue;
        }
        if metadata.is_file() || metadata.is_dir() {
            return Ok(());
        }
        return Err(unsafe_internal_symlink_error(
            label,
            "symlink target is a special file",
        ));
    }
}

fn bundle_archive_path(bundle_root: &Path, path: &Path, label: &str) -> Result<String, String> {
    let root_name = bundle_root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            unsafe_internal_symlink_error(label, "bundle root name is not valid UTF-8")
        })?;
    let relative = path.strip_prefix(bundle_root).map_err(|_| {
        unsafe_internal_symlink_error(label, "symlink path escaped the bundle root")
    })?;
    let mut components = vec![root_name.to_owned()];
    for component in relative.components() {
        let Component::Normal(value) = component else {
            return Err(unsafe_internal_symlink_error(
                label,
                "symlink path contains an unsafe component",
            ));
        };
        let value = value.to_str().ok_or_else(|| {
            unsafe_internal_symlink_error(label, "symlink path is not valid UTF-8")
        })?;
        if !is_safe_archive_component(value) {
            return Err(unsafe_internal_symlink_error(
                label,
                "symlink path contains an unsafe component",
            ));
        }
        components.push(value.to_owned());
    }
    Ok(components.join("/"))
}

fn bundle_path_from_archive_path(
    bundle_root: &Path,
    archive_path: &str,
    label: &str,
) -> Result<PathBuf, String> {
    let root_name = bundle_root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            unsafe_internal_symlink_error(label, "bundle root name is not valid UTF-8")
        })?;
    let components = archive_path.split('/').collect::<Vec<_>>();
    if components.first().copied() != Some(root_name)
        || components
            .iter()
            .any(|component| !is_safe_archive_component(component))
    {
        return Err(unsafe_internal_symlink_error(
            label,
            "resolved symlink path is outside the bundle root",
        ));
    }
    let mut path = bundle_root.to_path_buf();
    for component in components.iter().skip(1) {
        path.push(component);
    }
    Ok(path)
}

fn metadata_no_follow_inside_bundle(
    bundle_root: &Path,
    path: &Path,
    label: &str,
) -> Result<fs::Metadata, String> {
    let relative = path.strip_prefix(bundle_root).map_err(|_| {
        unsafe_internal_symlink_error(label, "symlink target escaped the bundle root")
    })?;
    let mut current = bundle_root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return fs::symlink_metadata(bundle_root).map_err(|error| {
            recovery_error("update-path-unresolved", format!("{label}: {error}"))
        });
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(value) = component else {
            return Err(unsafe_internal_symlink_error(
                label,
                "symlink target contains an unsafe component",
            ));
        };
        current.push(value);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            recovery_error(
                "update-path-unresolved",
                format!("{label} symlink target is unresolved: {error}"),
            )
        })?;
        if index + 1 < components.len() && metadata.file_type().is_symlink() {
            return Err(unsafe_internal_symlink_error(
                label,
                "symlink target traverses another symlink component",
            ));
        }
        if index + 1 == components.len() {
            return Ok(metadata);
        }
    }
    Err(unsafe_internal_symlink_error(
        label,
        "symlink target is unresolved",
    ))
}

fn is_safe_archive_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .as_bytes()
            .iter()
            .any(|byte| *byte == b'\\' || *byte == 0 || *byte < 0x20 || *byte == 0x7f)
        && !value.chars().any(char::is_control)
}

fn safe_symlink_path_error_message(error: SafeSymlinkPathError) -> &'static str {
    match error {
        SafeSymlinkPathError::InvalidTarget => "symlink target is not a safe relative path",
        SafeSymlinkPathError::EscapesRoot => "symlink target escapes the bundle root",
    }
}

fn unsafe_internal_symlink_error(label: &str, message: impl std::fmt::Display) -> String {
    recovery_error(
        "update-path-unresolved",
        format!("{label} contains an unsafe internal symlink: {message}"),
    )
}

fn require_safe_directory(path: &Path, label: &str) -> Result<(), String> {
    validate_no_symlink_components(path, label)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| recovery_error("update-path-unresolved", format!("{label}: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(recovery_error(
            "update-path-unresolved",
            format!("{label} is not a directory"),
        ));
    }
    Ok(())
}

fn validate_no_symlink_components(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(recovery_error(
            "update-path-unresolved",
            format!("{label} is not an absolute safe path"),
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => current.push(component.as_os_str()),
            Component::Normal(value) => {
                current.push(value);
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    recovery_error("update-path-unresolved", format!("{label}: {error}"))
                })?;
                if metadata.file_type().is_symlink() {
                    return Err(recovery_error(
                        "update-path-unresolved",
                        format!("{label} contains a symlink"),
                    ));
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(recovery_error(
                    "update-path-unresolved",
                    format!("{label} contains parent traversal"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_database_paths(storage: &StorageLayout) -> Result<(), String> {
    require_safe_directory(storage.live_directory(), "live database directory")?;
    require_safe_directory(storage.backups_directory(), "managed backup directory")?;
    let metadata = fs::symlink_metadata(storage.database_path())
        .map_err(|error| recovery_error("update-restore-failed", error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(recovery_error(
            "update-restore-failed",
            "live SQLite database is not a regular file",
        ));
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!("{}{}", storage.database_path().display(), suffix));
        if path_exists(&sidecar)? {
            return Err(recovery_error(
                "update-restore-failed",
                "live SQLite sidecar is present",
            ));
        }
    }
    Ok(())
}

fn ensure_file_copy(path: &Path, bytes: &[u8], failure_code: &str) -> Result<(), String> {
    if path_exists(path)? {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| recovery_error(failure_code, error.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(recovery_error(
                failure_code,
                "restore temporary path is not a regular file",
            ));
        }
        let existing =
            fs::read(path).map_err(|error| recovery_error(failure_code, error.to_string()))?;
        if existing != bytes {
            return Err(recovery_error(
                failure_code,
                "restore temporary path contains different data",
            ));
        }
        return Ok(());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = OpenOptionsMode::mode(options, 0o600)
        .open(path)
        .map_err(|error| recovery_error(failure_code, error.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| recovery_error(failure_code, error.to_string()))
}

fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(recovery_error("update-path-unresolved", error.to_string())),
    }
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let file = File::open(path)
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
        file.sync_all()
            .map_err(|error| recovery_error("update-switch-failed", error.to_string()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn candidate_database_switched(state_store: &UpdateStateStore) -> Option<bool> {
    state_store
        .snapshot()
        .recovery
        .and_then(|checkpoint| checkpoint.database_switched)
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn safe_path_token(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .take(96)
        .collect()
}

fn require_absolute_without_parent(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(recovery_error(
            "update-path-unresolved",
            format!("{label} is not safe"),
        ));
    }
    Ok(())
}

fn state_transition_error(operation: &str, error: UpdateStateError) -> String {
    format!("update-state-write-failed: {operation}: {error}")
}

fn recovery_error(code: &str, message: impl std::fmt::Display) -> String {
    format!("{code}: {message}")
}

trait OpenOptionsMode {
    fn mode(self, mode: u32) -> Self;
}

impl OpenOptionsMode for OpenOptions {
    fn mode(mut self, mode: u32) -> Self {
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut self, mode);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn database_restore_requires_explicit_switch_evidence() {
        assert!(!database_switch_was_proven(None));
        assert!(!database_switch_was_proven(Some(false)));
        assert!(database_switch_was_proven(Some(true)));
    }

    #[test]
    fn restore_temporary_cleanup_uses_exact_backup_paths_and_preserves_database_files() {
        let root = TestRoot::new();
        let live = root.path.join("live");
        let backups = root.path.join("backups");
        fs::create_dir_all(&live).expect("live directory");
        fs::create_dir(&backups).expect("backup directory");
        let database = live.join("notebook.sqlite");
        let backup = backups.join(format!("notebook-{}-100-random.sqlite.bak", "a".repeat(64)));
        fs::write(&database, b"live").expect("live database");
        fs::write(&backup, b"backup").expect("safety backup");

        let temporary_paths = restore_temporary_paths(&live, &backup).expect("temp paths");
        let candidate_restore =
            live.join(format!("{DATABASE_RESTORE_PREFIX}{}.tmp", "b".repeat(64)));
        assert_ne!(temporary_paths.restore_temp, candidate_restore);
        fs::write(&temporary_paths.restore_temp, b"backup").expect("restore temp");
        fs::write(&temporary_paths.original_temp, b"live").expect("original temp");

        remove_restore_temporary_files(&live, &temporary_paths).expect("exact cleanup");
        remove_restore_temporary_files(&live, &temporary_paths).expect("retry cleanup");
        let candidate_original =
            live.join(format!("{DATABASE_ORIGINAL_PREFIX}{}.tmp", "b".repeat(64)));
        fs::write(&candidate_restore, b"stale restore").expect("retry restore temp");
        fs::write(&candidate_original, b"stale original").expect("retry original temp");
        remove_stale_restore_temporary_files(&live).expect("stale retry cleanup");

        assert!(!temporary_paths.restore_temp.exists());
        assert!(!temporary_paths.original_temp.exists());
        assert!(!candidate_restore.exists());
        assert!(!candidate_original.exists());
        assert_eq!(fs::read(&database).expect("database read"), b"live");
        assert_eq!(fs::read(&backup).expect("backup read"), b"backup");
    }

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
                std::env::temp_dir().join(format!("cuetrio-update-recovery-{timestamp}-{counter}"));
            fs::create_dir(&path).expect("test root");
            Self { path }
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = remove_validated_tree(&self.path, "test cleanup");
        }
    }

    #[cfg(unix)]
    fn bundle(root: &Path) -> PathBuf {
        let app = root.join("Candidate.app");
        fs::create_dir_all(app.join("Contents/MacOS")).expect("bundle layout");
        fs::write(app.join("Contents/MacOS/notebook"), b"runtime").expect("runtime");
        app
    }

    fn verified_pending(digest: &str) -> PendingUpdate {
        PendingUpdate {
            version: "1.2.3".to_owned(),
            channel: "stable".to_owned(),
            architecture: crate::update_manifest::TARGET_ARCHITECTURE.to_owned(),
            artifact: "test-artifact".to_owned(),
            verification_state: VerificationState::Verified,
            size_bytes: Some(1),
            sha256: Some(digest.to_owned()),
            key_id: Some("test-key".to_owned()),
            signed_identity_sha256: Some(digest.to_owned()),
            package_path: Some(PathBuf::from("package.tar.gz")),
            extracted_app_path: Some(PathBuf::from("extract").join(digest).join(APP_BUNDLE_NAME)),
            discovered_at: 1,
            verified_at: Some(2),
        }
    }

    fn valid_candidate_source(root: &Path) -> PathBuf {
        let app = root.join(APP_BUNDLE_NAME);
        let contents = app.join("Contents");
        fs::create_dir_all(contents.join("MacOS")).expect("candidate layout");
        fs::write(
            contents.join("Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{}</string><key>CFBundleShortVersionString</key><string>1.2.3</string><key>CFBundleExecutable</key><string>notebook</string></dict></plist>",
                crate::update_manifest::MANIFEST_PRODUCT_ID
            ),
        )
        .expect("candidate plist");
        let mut macho = Vec::new();
        macho.extend_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        macho.extend_from_slice(&0x0100_000c_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&2_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        macho.extend_from_slice(&0_u32.to_le_bytes());
        let executable = contents.join("MacOS/notebook");
        fs::write(&executable, macho).expect("candidate executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(&executable)
                .expect("candidate executable metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).expect("candidate executable mode");
        }
        fs::write(contents.join("Resources.marker"), b"source-candidate").expect("marker");
        app
    }

    #[cfg(unix)]
    #[test]
    fn safe_internal_symlink_survives_validation_copy_and_cleanup() {
        use std::os::unix::fs::symlink;

        let root = TestRoot::new();
        let source = bundle(&root.path);
        symlink("MacOS/notebook", source.join("Contents/current")).expect("internal symlink");
        require_safe_bundle_tree(&source, "candidate bundle").expect("safe source");

        let switch_parent = root.path.join("switch-parent");
        fs::create_dir(&switch_parent).expect("switch parent");
        let destination = switch_parent.join("Copied.app");
        copy_bundle_tree(&source, &destination).expect("copy bundle");
        require_safe_bundle_tree(&destination, "copied bundle").expect("safe copy");
        assert_eq!(
            fs::read_link(destination.join("Contents/current")).expect("copied symlink"),
            PathBuf::from("MacOS/notebook")
        );
        assert_eq!(
            fs::read(destination.join("Contents/MacOS/notebook")).expect("copied target"),
            b"runtime"
        );

        remove_if_safe_tree(&destination, "copied bundle cleanup").expect("cleanup");
        assert!(!destination.exists());
        assert!(source.join("Contents/current").is_symlink());
        assert_eq!(
            fs::read(source.join("Contents/MacOS/notebook")).expect("source target"),
            b"runtime"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retry_discards_partial_switch_temp_and_rebuilds_from_candidate_source() {
        use std::os::unix::fs::symlink;

        let root = TestRoot::new();
        let source = valid_candidate_source(&root.path.join("staging"));
        let parent = root.path.join("Application");
        fs::create_dir(&parent).expect("application parent");
        let digest = "a".repeat(64);
        let switch_temp = parent.join(format!("{BUNDLE_SWITCH_PREFIX}{digest}.app"));
        copy_bundle_tree(&source, &switch_temp).expect("initial temporary copy");
        fs::remove_file(switch_temp.join("Contents/Resources.marker")).expect("partial marker");

        let outside = root.path.join("outside");
        fs::write(&outside, b"must survive").expect("outside marker");
        symlink(&outside, switch_temp.join("Contents/external")).expect("external link");

        let candidate = CandidateBundle {
            pending: verified_pending(&digest),
            source_path: source.clone(),
            runtime_path: None,
        };
        let paths = BundlePaths {
            current: parent.join(APP_BUNDLE_NAME),
            rollback: parent.join("rollback.app"),
            switch_temp: switch_temp.clone(),
            failed: parent.join("failed.app"),
        };

        prepare_switch_temp(&paths, &candidate).expect("rebuild temporary from source");
        assert_eq!(
            fs::read(switch_temp.join("Contents/Resources.marker")).expect("rebuilt marker"),
            b"source-candidate"
        );
        assert!(!switch_temp.join("Contents/external").exists());
        assert_eq!(
            fs::read(&outside).expect("outside marker read"),
            b"must survive"
        );
        compare_bundle_trees(&source, &switch_temp, "rebuilt switch temporary")
            .expect("rebuilt tree matches source");
    }

    #[test]
    fn failed_bundle_marker_cleanup_is_scoped_to_the_candidate_marker() {
        let root = TestRoot::new();
        let current = root.path.join(APP_BUNDLE_NAME);
        let rollback = root.path.join(format!("{BUNDLE_ROLLBACK_PREFIX}old.app"));
        let switch_temp = root
            .path
            .join(format!("{BUNDLE_SWITCH_PREFIX}candidate.app"));
        let failed = root
            .path
            .join(format!("{BUNDLE_FAILED_PREFIX}candidate.app"));
        let other_failed = root
            .path
            .join(format!("{BUNDLE_FAILED_PREFIX}other-candidate.app"));

        for directory in [&current, &rollback, &switch_temp, &failed, &other_failed] {
            fs::create_dir_all(directory.join("Contents/MacOS")).expect("bundle directory");
            fs::write(directory.join("Contents/MacOS/notebook"), b"bundle").expect("bundle file");
        }

        let paths = BundlePaths {
            current: current.clone(),
            rollback: rollback.clone(),
            switch_temp: switch_temp.clone(),
            failed: failed.clone(),
        };
        remove_failed_bundle_marker(&paths).expect("failed marker cleanup");

        assert!(!failed.exists());
        assert!(current.exists());
        assert!(rollback.exists());
        assert!(switch_temp.exists());
        assert!(other_failed.exists());
    }

    #[test]
    fn missing_candidate_artifacts_are_idempotent_after_parent_validation() {
        let root = TestRoot::new();
        let staging = root.path.join("staging");
        let backups = root.path.join("backups");
        let digest = "a".repeat(64);
        let candidate = staging.join("extract").join(&digest).join(APP_BUNDLE_NAME);
        let package = staging
            .join("packages")
            .join(format!("{digest}.app.tar.gz"));
        let backup = backups.join(format!("notebook-{digest}-100.sqlite.bak"));

        fs::create_dir_all(candidate.parent().expect("candidate parent"))
            .expect("candidate parent");
        fs::create_dir(package.parent().expect("package parent")).expect("package parent");
        fs::create_dir(&backups).expect("backup root");

        for (path, label) in [
            (&candidate, "candidate bundle cleanup"),
            (&package, "candidate package cleanup"),
            (&backup, "migration safety backup cleanup"),
        ] {
            remove_candidate_artifact(
                path,
                if path == &backup { &backups } else { &staging },
                label,
            )
            .expect("missing leaf is an idempotent cleanup success");
        }

        fs::write(&package, b"package").expect("package artifact");
        remove_candidate_artifact(&package, &staging, "candidate package cleanup")
            .expect("existing regular artifact cleanup");
        assert!(!package.exists());
        assert!(candidate.parent().expect("candidate parent").is_dir());
        assert!(backups.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn missing_candidate_artifact_does_not_bypass_root_or_symlink_safety() {
        use std::os::unix::fs::symlink;

        let root = TestRoot::new();
        let managed = root.path.join("managed");
        let outside = root.path.join("outside");
        fs::create_dir(&managed).expect("managed root");
        fs::create_dir(&outside).expect("outside directory");

        let escaped = managed.join("..").join("outside").join("missing");
        assert!(remove_candidate_artifact(&escaped, &managed, "escaped artifact").is_err());

        let parent_link = managed.join("parent-link");
        symlink(&outside, &parent_link).expect("parent symlink");
        let missing_through_link = parent_link.join("missing");
        assert!(remove_candidate_artifact(
            &missing_through_link,
            &managed,
            "linked parent artifact"
        )
        .is_err());

        let outside_file = outside.join("outside.txt");
        fs::write(&outside_file, b"must survive").expect("outside file");
        let linked_leaf = managed.join("linked-leaf");
        symlink(&outside_file, &linked_leaf).expect("leaf symlink");
        assert!(remove_candidate_artifact(&linked_leaf, &managed, "linked leaf artifact").is_err());
        assert_eq!(
            fs::read(&outside_file).expect("outside file read"),
            b"must survive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_internal_symlinks_fail_closed_without_touching_external_files() {
        use std::os::unix::fs::symlink;

        assert!(normalize_relative_symlink_target("").is_err());
        for target in [
            "/outside",
            "../../outside",
            "missing",
            "./notebook",
            "MacOS//notebook",
            "MacOS/notebook/",
            "MacOS\\notebook",
        ] {
            let root = TestRoot::new();
            let source = bundle(&root.path);
            let outside = root.path.join("outside");
            fs::write(&outside, b"must survive").expect("outside marker");
            symlink(target, source.join("Contents/current")).expect("unsafe symlink");

            let error = require_safe_bundle_tree(&source, "candidate bundle")
                .expect_err("unsafe symlink must fail");
            assert!(error.starts_with("update-path-unresolved:"));
            assert_eq!(
                fs::read(&outside).expect("outside marker read"),
                b"must survive"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycles_and_hop_overflow_fail_closed() {
        use std::os::unix::fs::symlink;

        for count in [MAX_SYMLINK_HOPS, MAX_SYMLINK_HOPS + 1] {
            let root = TestRoot::new();
            let source = bundle(&root.path);
            for index in 0..count {
                let target = format!("link{}", index + 1);
                symlink(target, source.join("Contents").join(format!("link{index}")))
                    .expect("chain link");
            }
            fs::write(
                source.join("Contents").join(format!("link{count}")),
                b"terminal",
            )
            .expect("terminal");

            let result = require_safe_bundle_tree(&source, "candidate bundle");
            if count == MAX_SYMLINK_HOPS {
                result.expect("maximum safe hop count");
            } else {
                assert!(result.is_err(), "hop overflow must fail");
            }
        }

        let root = TestRoot::new();
        let source = bundle(&root.path);
        symlink("cycle-b", source.join("Contents/cycle-a")).expect("cycle a");
        symlink("cycle-a", source.join("Contents/cycle-b")).expect("cycle b");
        assert!(require_safe_bundle_tree(&source, "candidate bundle").is_err());
    }
}

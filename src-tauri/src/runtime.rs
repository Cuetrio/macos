use super::{instance, AppResult};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(all(unix, any(test, not(debug_assertions))))]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};

use tauri::{AppHandle, Manager};

const READY_TIMEOUT: Duration = Duration::from_secs(35);
const SIDECAR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const PACKAGED_NODE_BINARY_NAME: &str = "node";
const SIDECAR_HEALTH_PATH: &str = "/api/desktop/health";
const SIDECAR_HEALTH_KIND: &str = "cornell-desktop-health";
const MAX_HEALTH_RESPONSE_BYTES: usize = 8 * 1024;
const STAGED_MIGRATION_FAILURE_CODE: &str = "staged-migration-failed";
const PACKAGED_APP_BUNDLE_NAME: &str = "Cuetrio.app";
const PACKAGED_CONTENTS_DIRECTORY_NAME: &str = "Contents";
const PACKAGED_RESOURCES_DIRECTORY_NAME: &str = "Resources";
const PACKAGED_RUNTIME_DIRECTORY_NAME: &str = "runtime";
const DESKTOP_DATA_BACKUP_PROTOCOL_VERSION: u8 = 1;
const DESKTOP_DATA_BACKUP_COMMAND: &str = "data-backup-operation";
const DESKTOP_MANAGED_BACKUP_CATALOG_PROTOCOL_VERSION: u8 = 1;
const DESKTOP_MANAGED_BACKUP_CATALOG_COMMAND: &str = "managed-backup-catalog";
const DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION: u8 = 1;
const DESKTOP_PENDING_RESTORE_STATUS_COMMAND: &str = "pending-restore-status";
const DESKTOP_PENDING_RESTORE_RESUME_COMMAND: &str = "pending-restore-resume";
const DESKTOP_BACKUP_RECOVERY_PROTOCOL_VERSION: u8 = 1;
const DESKTOP_BACKUP_RECOVERY_COMMAND: &str = "attempt-backup-recovery";
const DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION: u8 = 1;
const DESKTOP_DATABASE_RECOVERY_STATE_FIRST_RUN: &str = "first-run";
const DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_AVAILABLE: &str = "restore-available";
const DESKTOP_DATABASE_RECOVERY_STATE_DIAGNOSTIC_REQUIRED: &str = "diagnostic-required";
const DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_UNAVAILABLE: &str = "restore-unavailable";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_MISSING: &str = "database-missing";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_MISSING_AFTER_INITIALIZATION: &str =
    "database-missing-after-initialization";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_NOT_A_FILE: &str = "database-not-a-file";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_READ_FAILED: &str = "database-read-failed";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INTEGRITY_FAILED: &str =
    "database-integrity-failed";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_FOREIGN_KEY_FAILED: &str =
    "database-foreign-key-failed";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_SCHEMA_INVALID: &str = "database-schema-invalid";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_MIGRATION_REQUIRED: &str =
    "database-migration-required";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INITIALIZATION_FAILED: &str =
    "database-initialization-failed";
const DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INITIALIZATION_MARKER_INVALID: &str =
    "database-initialization-marker-invalid";
const DESKTOP_DATABASE_RECOVERY_REASON_STORAGE_UNAVAILABLE: &str = "storage-unavailable";
const DESKTOP_STORAGE_BOOTSTRAP_ERROR_CODE: &str = "bootstrap-failed";
const DESKTOP_DIALOG_BINARY: &str = "/usr/bin/osascript";
const MAX_DESKTOP_DIALOG_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_SAVE_DESTINATION_NAME_ATTEMPTS: usize = 8;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopApiRequest {
    pub(crate) path: String,
    pub(crate) method: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopApiResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopDatabaseRecoverySnapshot {
    pub(crate) schema_version: u8,
    pub(crate) state: String,
    pub(crate) reason_code: String,
    pub(crate) managed_backup_available: bool,
    pub(crate) pending_restore_available: bool,
    pub(crate) can_start_empty: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BootstrapMessage {
    kind: String,
    status: String,
    application_support_root: Option<String>,
    live_directory: Option<String>,
    database_path: Option<String>,
    database_url: Option<String>,
    backups_directory: Option<String>,
    settings_directory: Option<String>,
    logs_directory: Option<String>,
    pending_restore_directory: Option<String>,
    reason: Option<String>,
    created: Option<bool>,
    recovery_snapshot: Option<DesktopDatabaseRecoverySnapshot>,
    code: Option<String>,
    context: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DesktopStorageBootstrapError {
    operation: &'static str,
    status: String,
    result_context: &'static str,
    stderr_context: &'static str,
}

impl DesktopStorageBootstrapError {
    fn from_output(operation: &'static str, result_context: &'static str, output: &Output) -> Self {
        Self {
            operation,
            status: process_status(&output.status),
            result_context,
            stderr_context: sanitize_bootstrap_stderr(&output.stderr),
        }
    }

    fn spawn_failed(operation: &'static str) -> Self {
        Self {
            operation,
            status: "spawn-failed".to_string(),
            result_context: "launcher-unavailable",
            stderr_context: "not-available",
        }
    }
}

impl std::fmt::Display for DesktopStorageBootstrapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "desktop storage {} failed (code={}; status={}; result={}; stderr={})",
            self.operation,
            DESKTOP_STORAGE_BOOTSTRAP_ERROR_CODE,
            self.status,
            self.result_context,
            self.stderr_context,
        )
    }
}

impl std::error::Error for DesktopStorageBootstrapError {}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagedMigrationMessage {
    kind: String,
    status: String,
    code: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseValidationMessage {
    kind: String,
    status: String,
    reason: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct StorageLayout {
    application_support_root: PathBuf,
    live_directory: PathBuf,
    database_path: PathBuf,
    database_url: String,
    backups_directory: PathBuf,
    settings_directory: PathBuf,
    logs_directory: PathBuf,
    pending_restore_directory: PathBuf,
}

impl StorageLayout {
    pub(crate) fn application_support_root(&self) -> &Path {
        &self.application_support_root
    }

    pub(crate) fn settings_directory(&self) -> &Path {
        &self.settings_directory
    }

    pub(crate) fn staging_directory(&self) -> PathBuf {
        self.application_support_root.join("staging")
    }

    pub(crate) fn live_directory(&self) -> &Path {
        &self.live_directory
    }

    pub(crate) fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub(crate) fn backups_directory(&self) -> &Path {
        &self.backups_directory
    }

    pub(crate) fn logs_directory(&self) -> &Path {
        &self.logs_directory
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopDatabaseRecoverySnapshotResponse {
    kind: &'static str,
    schema_version: u8,
    status: &'static str,
    snapshot: Option<DesktopDatabaseRecoverySnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) struct DesktopDatabaseRecoveryState {
    snapshot: Arc<Mutex<Option<DesktopDatabaseRecoverySnapshot>>>,
    recovery_only: Arc<AtomicBool>,
}

impl Default for DesktopDatabaseRecoveryState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl DesktopDatabaseRecoveryState {
    pub(crate) fn new(snapshot: Option<DesktopDatabaseRecoverySnapshot>) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(snapshot)),
            recovery_only: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn recovery_only(snapshot: DesktopDatabaseRecoverySnapshot) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(Some(snapshot))),
            recovery_only: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(crate) fn is_recovery_only(&self) -> bool {
        self.recovery_only.load(Ordering::Acquire)
    }

    pub(crate) fn mark_ready(&self) {
        self.recovery_only.store(false, Ordering::Release);
    }

    pub(crate) fn mark_recovery_only(&self, snapshot: DesktopDatabaseRecoverySnapshot) -> bool {
        if let Ok(mut current) = self.snapshot.lock() {
            *current = Some(snapshot);
            self.recovery_only.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    pub(crate) fn response(&self) -> DesktopDatabaseRecoverySnapshotResponse {
        let status = if self.is_recovery_only() {
            "recovery"
        } else {
            "ready"
        };
        let snapshot = self
            .snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.clone());
        DesktopDatabaseRecoverySnapshotResponse {
            kind: "desktop-database-recovery-snapshot",
            schema_version: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
            status,
            snapshot,
        }
    }

    pub(crate) fn unavailable_response() -> DesktopDatabaseRecoverySnapshotResponse {
        DesktopDatabaseRecoverySnapshotResponse {
            kind: "desktop-database-recovery-snapshot",
            schema_version: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
            status: "recovery",
            snapshot: Some(DesktopDatabaseRecoverySnapshot {
                schema_version: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
                state: DESKTOP_DATABASE_RECOVERY_STATE_DIAGNOSTIC_REQUIRED.to_string(),
                reason_code: DESKTOP_DATABASE_RECOVERY_REASON_STORAGE_UNAVAILABLE.to_string(),
                managed_backup_available: false,
                pending_restore_available: false,
                can_start_empty: false,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopRestoreMode {
    Normal,
    RecoveryOnly,
}

impl DesktopRestoreMode {
    pub(crate) fn is_recovery_only(self) -> bool {
        matches!(self, Self::RecoveryOnly)
    }
}

fn validate_database_recovery_snapshot(
    snapshot: &DesktopDatabaseRecoverySnapshot,
) -> AppResult<()> {
    if snapshot.schema_version != DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION {
        return Err(
            "desktop database recovery snapshot has an unsupported schema version".to_string(),
        );
    }
    if !matches!(
        snapshot.state.as_str(),
        DESKTOP_DATABASE_RECOVERY_STATE_FIRST_RUN
            | DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_AVAILABLE
            | DESKTOP_DATABASE_RECOVERY_STATE_DIAGNOSTIC_REQUIRED
            | DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_UNAVAILABLE
    ) {
        return Err("desktop database recovery snapshot has an invalid state".to_string());
    }
    if !matches!(
        snapshot.reason_code.as_str(),
        DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_MISSING
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_MISSING_AFTER_INITIALIZATION
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_NOT_A_FILE
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_READ_FAILED
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INTEGRITY_FAILED
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_FOREIGN_KEY_FAILED
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_SCHEMA_INVALID
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_MIGRATION_REQUIRED
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INITIALIZATION_FAILED
            | DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INITIALIZATION_MARKER_INVALID
            | DESKTOP_DATABASE_RECOVERY_REASON_STORAGE_UNAVAILABLE
    ) {
        return Err("desktop database recovery snapshot has an invalid reason code".to_string());
    }
    if snapshot.state == DESKTOP_DATABASE_RECOVERY_STATE_FIRST_RUN && !snapshot.can_start_empty {
        return Err("first-run recovery snapshot must allow an empty start".to_string());
    }
    if snapshot.state != DESKTOP_DATABASE_RECOVERY_STATE_FIRST_RUN && snapshot.can_start_empty {
        return Err("non-first-run recovery snapshot must not allow an empty start".to_string());
    }
    if snapshot.state == DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_AVAILABLE
        && !snapshot.managed_backup_available
        && !snapshot.pending_restore_available
    {
        return Err("restore-available snapshot has no recovery source".to_string());
    }
    if snapshot.state == DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_UNAVAILABLE
        && (snapshot.managed_backup_available || snapshot.pending_restore_available)
    {
        return Err("restore-unavailable snapshot has a recovery source".to_string());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopFileDialogKind {
    SaveDestination,
    OpenExternalSource,
    DiagnosticExport,
}

impl DesktopFileDialogKind {
    fn wire_name(self) -> &'static str {
        match self {
            Self::SaveDestination => "save-destination",
            Self::OpenExternalSource => "open-external-source",
            Self::DiagnosticExport => "diagnostic-export",
        }
    }

    fn requires_existing_file(self) -> bool {
        matches!(self, Self::OpenExternalSource)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopFileDialogFailurePhase {
    Command,
    DialogProcess,
    ResponseParse,
    PathValidation,
    SelectionStore,
}

impl DesktopFileDialogFailurePhase {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::DialogProcess => "dialog-process",
            Self::ResponseParse => "response-parse",
            Self::PathValidation => "path-validation",
            Self::SelectionStore => "selection-store",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopFileDialogExitStatus {
    Success,
    NonZero,
    Unavailable,
}

impl DesktopFileDialogExitStatus {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NonZero => "non-zero",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DesktopFileDialogFailure {
    pub(crate) phase: DesktopFileDialogFailurePhase,
    pub(crate) error_code: &'static str,
    pub(crate) exit_status: DesktopFileDialogExitStatus,
}

impl DesktopFileDialogFailure {
    pub(crate) fn new(
        phase: DesktopFileDialogFailurePhase,
        error_code: &'static str,
        exit_status: DesktopFileDialogExitStatus,
    ) -> Self {
        Self {
            phase,
            error_code,
            exit_status,
        }
    }

    pub(crate) fn record(self, app: &AppHandle, dialog: DesktopFileDialogKind) {
        crate::diagnostics::record_file_dialog_failure_for_app(
            app,
            dialog.wire_name(),
            self.phase.wire_name(),
            self.error_code,
            self.exit_status.wire_name(),
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DesktopNativeFileDialogSelection {
    pub(crate) path: PathBuf,
    pub(crate) exit_status: DesktopFileDialogExitStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopFileSelection {
    pub(crate) kind: &'static str,
    pub(crate) selection_id: String,
    pub(crate) file_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopFileDialogResult {
    kind: &'static str,
    schema_version: u8,
    dialog: &'static str,
    ok: bool,
    status: &'static str,
    phase: &'static str,
    selection: Option<DesktopFileSelection>,
    error_code: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopDataBackupExportResult {
    file_name: String,
    size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopDataBackupOperationResponse {
    kind: String,
    schema_version: u8,
    ok: bool,
    status: String,
    operation: Option<String>,
    phase: String,
    error_code: Option<String>,
    result: Option<DesktopDataBackupExportResult>,
}

impl DesktopDataBackupOperationResponse {
    pub(crate) fn is_success(&self) -> bool {
        self.ok && self.status == "success"
    }

    pub(crate) fn is_validation_phase(&self) -> bool {
        self.phase == "validation"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopManagedBackupCatalogEntry {
    backup_id: String,
    file_name: String,
    size: u64,
    created_at: String,
    recovery_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopManagedBackupCatalogResponse {
    kind: String,
    schema_version: u8,
    pub(crate) status: String,
    phase: String,
    pub(crate) error_code: Option<String>,
    pub(crate) backups: Vec<DesktopManagedBackupCatalogEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopPendingRestoreSummary {
    pending_id: String,
    manifest_token: String,
    source_kind: String,
    created_at: String,
    candidate_digest: String,
    candidate_size: u64,
    candidate_schema_identity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopPendingRestoreStatusResponse {
    kind: String,
    schema_version: u8,
    pub(crate) status: String,
    phase: String,
    operation_id: Option<String>,
    pub(crate) error_code: Option<String>,
    pending: Option<DesktopPendingRestoreSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopPendingRestoreResumeResult {
    pub(crate) safety_backup_id: Option<String>,
    size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopPendingRestoreResumeResponse {
    kind: String,
    schema_version: u8,
    pub(crate) ok: bool,
    pub(crate) status: String,
    phase: String,
    operation_id: Option<String>,
    pending_id: Option<String>,
    error_code: Option<String>,
    pub(crate) result: Option<DesktopPendingRestoreResumeResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopBackupRecoveryRequest {
    pub(crate) kind: String,
    pub(crate) schema_version: u8,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DesktopBackupRecoveryResponse {
    pub(crate) kind: String,
    pub(crate) schema_version: u8,
    pub(crate) status: String,
    pub(crate) phase: String,
    pub(crate) error_code: Option<String>,
    pub(crate) recovery_snapshot: Option<DesktopDatabaseRecoverySnapshot>,
}

impl DesktopBackupRecoveryResponse {
    pub(crate) fn not_recovered(error_code: &str) -> Self {
        Self {
            kind: "desktop-backup-recovery".to_string(),
            schema_version: DESKTOP_BACKUP_RECOVERY_PROTOCOL_VERSION,
            status: "not-recovered".to_string(),
            phase: "preflight".to_string(),
            error_code: Some(error_code.to_string()),
            recovery_snapshot: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DesktopPendingRestoreResumeRequest {
    schema_version: u8,
    pending_id: String,
    manifest_token: String,
    confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopPendingRestoreSidecarRequest {
    kind: &'static str,
    schema_version: u8,
    pending_id: String,
    manifest_token: String,
    confirmed: bool,
    operation_id: String,
    recovery_only: bool,
}

#[derive(Clone, Debug)]
struct SelectedExternalFile {
    dialog_kind: DesktopFileDialogKind,
    path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DesktopFileSelectionStore {
    selections: Arc<Mutex<HashMap<String, SelectedExternalFile>>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DesktopDataBackupOperationRequest {
    schema_version: u8,
    operation: DesktopDataBackupOperation,
    source: serde_json::Value,
    destination: serde_json::Value,
    #[serde(default)]
    confirmed: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
enum DesktopDataBackupOperation {
    #[serde(rename = "export")]
    Export,
    #[serde(rename = "restore")]
    Restore,
    #[serde(rename = "delete")]
    Delete,
}

impl DesktopDataBackupOperation {
    fn wire_name(&self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Restore => "restore",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum DesktopDataBackupLocation {
    #[serde(rename = "managed-backup")]
    ManagedBackup {
        #[serde(rename = "backupId")]
        backup_id: String,
    },
    #[serde(rename = "external-selection")]
    ExternalSelection {
        #[serde(rename = "selectionId")]
        selection_id: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopDataBackupSidecarRequest {
    kind: &'static str,
    schema_version: u8,
    operation: &'static str,
    source: Option<DesktopDataBackupSidecarLocation>,
    destination: Option<DesktopDataBackupSidecarLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed: Option<bool>,
    #[serde(rename = "operationId", skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_only: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum DesktopDataBackupSidecarLocation {
    #[serde(rename = "managed-backup")]
    ManagedBackup {
        #[serde(rename = "backupId")]
        backup_id: String,
    },
    #[serde(rename = "external-file")]
    ExternalFile { origin: &'static str, path: String },
}

fn desktop_data_backup_response(
    operation: Option<&str>,
    ok: bool,
    status: &str,
    phase: &str,
    error_code: Option<&str>,
) -> DesktopDataBackupOperationResponse {
    DesktopDataBackupOperationResponse {
        kind: "desktop-data-backup-operation".to_string(),
        schema_version: DESKTOP_DATA_BACKUP_PROTOCOL_VERSION,
        ok,
        status: status.to_string(),
        operation: operation.map(str::to_string),
        phase: phase.to_string(),
        error_code: error_code.map(str::to_string),
        result: None,
    }
}

pub(crate) fn desktop_data_backup_command_error(
    operation: Option<&str>,
    phase: &str,
    error_code: &str,
) -> DesktopDataBackupOperationResponse {
    desktop_data_backup_response(operation, false, "error", phase, Some(error_code))
}

fn managed_backup_catalog_response(
    status: &str,
    error_code: Option<&str>,
    backups: Vec<DesktopManagedBackupCatalogEntry>,
) -> DesktopManagedBackupCatalogResponse {
    DesktopManagedBackupCatalogResponse {
        kind: "desktop-managed-backup-catalog".to_string(),
        schema_version: DESKTOP_MANAGED_BACKUP_CATALOG_PROTOCOL_VERSION,
        status: status.to_string(),
        phase: "catalog".to_string(),
        error_code: error_code.map(str::to_string),
        backups,
    }
}

pub(crate) fn managed_backup_catalog_command_error(
    error_code: &str,
) -> DesktopManagedBackupCatalogResponse {
    managed_backup_catalog_response("error", Some(error_code), Vec::new())
}

fn pending_restore_status_response(
    status: &str,
    error_code: Option<&str>,
    pending: Option<DesktopPendingRestoreSummary>,
) -> DesktopPendingRestoreStatusResponse {
    DesktopPendingRestoreStatusResponse {
        kind: "desktop-pending-restore-status".to_string(),
        schema_version: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
        status: status.to_string(),
        phase: "status".to_string(),
        operation_id: None,
        error_code: error_code.map(str::to_string),
        pending,
    }
}

pub(crate) fn pending_restore_status_command_error(
    error_code: &str,
) -> DesktopPendingRestoreStatusResponse {
    pending_restore_status_response("invalid", Some(error_code), None)
}

fn pending_restore_resume_response(
    ok: bool,
    status: &str,
    phase: &str,
    operation_id: Option<&str>,
    pending_id: Option<&str>,
    error_code: Option<&str>,
    result: Option<DesktopPendingRestoreResumeResult>,
) -> DesktopPendingRestoreResumeResponse {
    DesktopPendingRestoreResumeResponse {
        kind: "desktop-pending-restore-resume".to_string(),
        schema_version: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
        ok,
        status: status.to_string(),
        phase: phase.to_string(),
        operation_id: operation_id.map(str::to_string),
        pending_id: pending_id.map(str::to_string),
        error_code: error_code.map(str::to_string),
        result,
    }
}

pub(crate) fn pending_restore_resume_command_error(
    operation_id: Option<&str>,
    pending_id: Option<&str>,
    phase: &str,
    error_code: &str,
) -> DesktopPendingRestoreResumeResponse {
    pending_restore_resume_response(
        false,
        "error",
        phase,
        operation_id,
        pending_id,
        Some(error_code),
        None,
    )
}

fn pending_restore_token_is_safe(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn validate_pending_restore_resume_request(
    value: &serde_json::Value,
) -> Result<(String, String), &'static str> {
    let request = serde_json::from_value::<DesktopPendingRestoreResumeRequest>(value.clone())
        .map_err(|_| "invalid-request")?;
    if request.schema_version != DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION {
        return Err("unsupported-protocol-version");
    }
    if !pending_restore_token_is_safe(&request.pending_id)
        || !pending_restore_token_is_safe(&request.manifest_token)
    {
        return Err("invalid-request");
    }
    if !request.confirmed {
        return Err("confirmation-required");
    }
    Ok((request.pending_id, request.manifest_token))
}

fn desktop_file_dialog_result(
    dialog: DesktopFileDialogKind,
    ok: bool,
    status: &'static str,
    selection: Option<DesktopFileSelection>,
    error_code: Option<&'static str>,
) -> DesktopFileDialogResult {
    DesktopFileDialogResult {
        kind: "desktop-file-dialog",
        schema_version: DESKTOP_DATA_BACKUP_PROTOCOL_VERSION,
        dialog: dialog.wire_name(),
        ok,
        status,
        phase: "dialog",
        selection,
        error_code,
    }
}

pub(crate) fn desktop_file_dialog_command_error(
    dialog: &'static str,
    error_code: &'static str,
) -> DesktopFileDialogResult {
    let dialog = match dialog {
        "save-destination" => DesktopFileDialogKind::SaveDestination,
        _ => DesktopFileDialogKind::OpenExternalSource,
    };
    desktop_file_dialog_result(dialog, false, "error", None, Some(error_code))
}

fn safe_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn validate_external_file_path(
    path: &Path,
    application_support_root: &Path,
    requires_existing_file: bool,
) -> Result<(), &'static str> {
    validate_external_path(
        path,
        application_support_root,
        ExternalPathKind::File,
        !requires_existing_file,
    )
}

fn validate_external_directory_path(
    path: &Path,
    application_support_root: &Path,
) -> Result<(), &'static str> {
    validate_external_path(
        path,
        application_support_root,
        ExternalPathKind::Directory,
        false,
    )
}

#[derive(Clone, Copy)]
enum ExternalPathKind {
    File,
    Directory,
}

fn validate_external_path(
    path: &Path,
    application_support_root: &Path,
    leaf_kind: ExternalPathKind,
    allow_missing_leaf: bool,
) -> Result<(), &'static str> {
    if !path.is_absolute() {
        return Err("relative-path");
    }
    let path_text = path.to_str().ok_or("invalid-path")?;
    if path_text.is_empty()
        || path_text.contains('\\')
        || path_text
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err("invalid-path");
    }
    if path.starts_with(application_support_root) {
        return Err("managed-path");
    }

    let mut normalized = PathBuf::new();
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir | Component::ParentDir => return Err("unsafe-path"),
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
                components.push(component);
            }
        }
    }
    if normalized != path || components.is_empty() {
        return Err("unsafe-path");
    }

    let mut current = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let is_leaf = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err("symlink-path");
                }
                if !is_leaf && !metadata.is_dir() {
                    return Err("path-unavailable");
                }
                if is_leaf {
                    match leaf_kind {
                        ExternalPathKind::File if !metadata.is_file() => {
                            return Err("path-not-file")
                        }
                        ExternalPathKind::Directory if !metadata.is_dir() => {
                            return Err("path-not-directory")
                        }
                        _ => {}
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && is_leaf => {
                if allow_missing_leaf {
                    return Ok(());
                }
                return Err("path-not-found");
            }
            Err(_) => return Err("path-unavailable"),
        }
    }
    Ok(())
}

fn create_selection_id() -> Result<String, &'static str> {
    create_random_hex(32)
}

fn create_random_hex(byte_len: usize) -> Result<String, &'static str> {
    let mut bytes = vec![0_u8; byte_len];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "selection-store-failed")?;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut value, "{byte:02x}").map_err(|_| "selection-store-failed")?;
    }
    Ok(value)
}

pub(crate) fn create_data_backup_operation_id() -> Result<String, &'static str> {
    create_random_hex(32)
}

fn format_external_backup_file_name(timestamp_seconds: u64, random: &str) -> String {
    let seconds = i64::try_from(timestamp_seconds).unwrap_or(i64::MAX);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);

    // Gregorian civil date conversion from days since 1970-01-01 (UTC).
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }).div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_part = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * month_part + 2).div_euclid(5) + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;

    format!(
        "cornell-method-backup-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}-{random}.sqlite"
    )
}

fn create_external_backup_destination(directory: &Path) -> Result<PathBuf, &'static str> {
    let timestamp_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for _ in 0..MAX_SAVE_DESTINATION_NAME_ATTEMPTS {
        let random = create_random_hex(8)?;
        let destination =
            directory.join(format_external_backup_file_name(timestamp_seconds, &random));
        match fs::symlink_metadata(&destination) {
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(destination),
            Err(_) => return Err("path-unavailable"),
        }
    }
    Err("destination-name-unavailable")
}

impl DesktopFileSelectionStore {
    pub(crate) fn insert(
        &self,
        dialog_kind: DesktopFileDialogKind,
        path: PathBuf,
    ) -> Result<DesktopFileSelection, &'static str> {
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| {
                !value.is_empty()
                    && !value
                        .bytes()
                        .any(|byte| byte == 0 || byte.is_ascii_control())
            })
            .ok_or("invalid-path")?
            .to_string();
        let selection_id = create_selection_id()?;
        let mut selections = self
            .selections
            .lock()
            .map_err(|_| "selection-store-failed")?;
        selections.insert(
            selection_id.clone(),
            SelectedExternalFile { dialog_kind, path },
        );
        Ok(DesktopFileSelection {
            kind: if dialog_kind == DesktopFileDialogKind::DiagnosticExport {
                "diagnostic-export"
            } else {
                "external-file"
            },
            selection_id,
            file_name,
        })
    }

    pub(crate) fn resolve(
        &self,
        selection_id: &str,
        expected_dialog_kind: DesktopFileDialogKind,
        application_support_root: &Path,
    ) -> Result<PathBuf, &'static str> {
        if !safe_identifier(selection_id, 128) {
            return Err("invalid-selection");
        }
        let selected = self
            .selections
            .lock()
            .map_err(|_| "selection-store-failed")?
            .get(selection_id)
            .cloned()
            .ok_or("selection-not-found")?;
        if selected.dialog_kind != expected_dialog_kind {
            return Err("selection-kind-mismatch");
        }
        validate_external_file_path(
            &selected.path,
            application_support_root,
            expected_dialog_kind.requires_existing_file(),
        )?;
        Ok(selected.path)
    }
}

#[cfg(target_os = "macos")]
fn desktop_file_dialog_script(dialog: DesktopFileDialogKind) -> &'static str {
    match dialog {
        DesktopFileDialogKind::SaveDestination => {
            r#"try
  set chosenItem to choose folder with prompt "Choose a folder for the export"
  return "selected" & linefeed & POSIX path of chosenItem
on error errorMessage number errorNumber
  if errorNumber is -128 then
    return "cancel"
  end if
  return "error"
end try"#
        }
        DesktopFileDialogKind::OpenExternalSource => {
            r#"try
  set chosenItem to choose file with prompt "Choose a Cornell Method SQLite file"
  return "selected" & linefeed & POSIX path of chosenItem
on error errorMessage number errorNumber
  if errorNumber is -128 then
    return "cancel"
  end if
  return "error"
end try"#
        }
        DesktopFileDialogKind::DiagnosticExport => {
            r#"try
  set chosenItem to choose file name with prompt "Choose a diagnostic export destination"
  return "selected" & linefeed & POSIX path of chosenItem
on error errorMessage number errorNumber
  if errorNumber is -128 then
    return "cancel"
  end if
  return "error"
end try"#
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn run_native_file_dialog(
    dialog: DesktopFileDialogKind,
) -> Result<Option<DesktopNativeFileDialogSelection>, DesktopFileDialogFailure> {
    let output = Command::new(DESKTOP_DIALOG_BINARY)
        .args(["-e", desktop_file_dialog_script(dialog)])
        .output()
        .map_err(|_| {
            DesktopFileDialogFailure::new(
                DesktopFileDialogFailurePhase::DialogProcess,
                "dialog-unavailable",
                DesktopFileDialogExitStatus::Unavailable,
            )
        })?;
    parse_native_file_dialog_output(&output.status, &output.stdout)
}

fn parse_native_file_dialog_output(
    status: &ExitStatus,
    stdout: &[u8],
) -> Result<Option<DesktopNativeFileDialogSelection>, DesktopFileDialogFailure> {
    let exit_status = if status.success() {
        DesktopFileDialogExitStatus::Success
    } else {
        DesktopFileDialogExitStatus::NonZero
    };
    parse_native_file_dialog_output_with_status(exit_status, stdout)
}

fn parse_native_file_dialog_output_with_status(
    exit_status: DesktopFileDialogExitStatus,
    stdout: &[u8],
) -> Result<Option<DesktopNativeFileDialogSelection>, DesktopFileDialogFailure> {
    if stdout.len() > MAX_DESKTOP_DIALOG_OUTPUT_BYTES {
        return Err(DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::ResponseParse,
            "dialog-response-too-large",
            exit_status,
        ));
    }
    let output = String::from_utf8(stdout.to_vec()).map_err(|_| {
        DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::ResponseParse,
            "dialog-invalid-response",
            exit_status,
        )
    })?;
    let output = output
        .trim_end_matches(|character| character == '\r' || character == '\n')
        .replace("\r\n", "\n");
    if output == "cancel" {
        return Ok(None);
    }
    let path = output
        .strip_prefix("selected\n")
        .filter(|path| !path.is_empty())
        .ok_or(if output == "error" {
            DesktopFileDialogFailure::new(
                DesktopFileDialogFailurePhase::ResponseParse,
                "dialog-error",
                exit_status,
            )
        } else {
            DesktopFileDialogFailure::new(
                DesktopFileDialogFailurePhase::ResponseParse,
                "dialog-invalid-response",
                exit_status,
            )
        })?;
    Ok(Some(DesktopNativeFileDialogSelection {
        path: PathBuf::from(path),
        exit_status,
    }))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn run_native_file_dialog(
    _dialog: DesktopFileDialogKind,
) -> Result<Option<DesktopNativeFileDialogSelection>, DesktopFileDialogFailure> {
    Err(DesktopFileDialogFailure::new(
        DesktopFileDialogFailurePhase::DialogProcess,
        "unsupported-platform",
        DesktopFileDialogExitStatus::Unavailable,
    ))
}

fn choose_data_backup_file(
    app: &AppHandle,
    dialog: DesktopFileDialogKind,
) -> DesktopFileDialogResult {
    let Some(storage) = app.try_state::<StorageLayout>() else {
        let failure = DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::Command,
            "storage-unavailable",
            DesktopFileDialogExitStatus::Unavailable,
        );
        failure.record(app, dialog);
        return desktop_file_dialog_result(dialog, false, "error", None, Some(failure.error_code));
    };
    let Some(selection_store) = app.try_state::<DesktopFileSelectionStore>() else {
        let failure = DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::Command,
            "selection-store-failed",
            DesktopFileDialogExitStatus::Unavailable,
        );
        failure.record(app, dialog);
        return desktop_file_dialog_result(dialog, false, "error", None, Some(failure.error_code));
    };
    let selection = match run_native_file_dialog(dialog) {
        Ok(Some(selection)) => selection,
        Ok(None) => return desktop_file_dialog_result(dialog, false, "cancelled", None, None),
        Err(failure) => {
            failure.record(app, dialog);
            return desktop_file_dialog_result(
                dialog,
                false,
                "error",
                None,
                Some(failure.error_code),
            );
        }
    };
    let selected_path = if dialog == DesktopFileDialogKind::SaveDestination {
        if let Err(error_code) =
            validate_external_directory_path(&selection.path, storage.application_support_root())
        {
            DesktopFileDialogFailure::new(
                DesktopFileDialogFailurePhase::PathValidation,
                error_code,
                selection.exit_status,
            )
            .record(app, dialog);
            return desktop_file_dialog_result(dialog, false, "error", None, Some(error_code));
        }
        match create_external_backup_destination(&selection.path) {
            Ok(path) => path,
            Err(error_code) => {
                DesktopFileDialogFailure::new(
                    DesktopFileDialogFailurePhase::PathValidation,
                    error_code,
                    selection.exit_status,
                )
                .record(app, dialog);
                return desktop_file_dialog_result(dialog, false, "error", None, Some(error_code));
            }
        }
    } else {
        if let Err(error_code) = validate_external_file_path(
            &selection.path,
            storage.application_support_root(),
            dialog.requires_existing_file(),
        ) {
            DesktopFileDialogFailure::new(
                DesktopFileDialogFailurePhase::PathValidation,
                error_code,
                selection.exit_status,
            )
            .record(app, dialog);
            return desktop_file_dialog_result(dialog, false, "error", None, Some(error_code));
        }
        selection.path
    };
    let exit_status = selection.exit_status;
    match selection_store.inner().insert(dialog, selected_path) {
        Ok(selection) => {
            desktop_file_dialog_result(dialog, true, "selected", Some(selection), None)
        }
        Err(error_code) => {
            DesktopFileDialogFailure::new(
                DesktopFileDialogFailurePhase::SelectionStore,
                error_code,
                exit_status,
            )
            .record(app, dialog);
            desktop_file_dialog_result(dialog, false, "error", None, Some(error_code))
        }
    }
}

pub(crate) fn choose_data_backup_save_destination(app: &AppHandle) -> DesktopFileDialogResult {
    choose_data_backup_file(app, DesktopFileDialogKind::SaveDestination)
}

pub(crate) fn choose_data_backup_external_source(app: &AppHandle) -> DesktopFileDialogResult {
    choose_data_backup_file(app, DesktopFileDialogKind::OpenExternalSource)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyMessage {
    kind: String,
    status: String,
    url: String,
    host: String,
    port: u16,
    ready_nonce: String,
    runtime_pid: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeHealthMessage {
    kind: String,
    status: String,
    nonce: String,
}

#[derive(Debug)]
pub(crate) struct SidecarHandle {
    child: Child,
    root_pid: u32,
    process_group_id: Option<u32>,
    runtime_url: tauri::Url,
    stopped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidecarStartupError {
    code: &'static str,
}

impl SidecarStartupError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub(crate) fn code(self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for SidecarStartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("The desktop runtime could not start.")
    }
}

pub(crate) fn runtime_project_root(app: &AppHandle) -> AppResult<PathBuf> {
    if let Some(configured) = env::var_os("CORNELL_DESKTOP_PROJECT_ROOT") {
        let path = PathBuf::from(configured);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err("CORNELL_DESKTOP_PROJECT_ROOT must be absolute".to_string());
    }

    if cfg!(debug_assertions) {
        return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    }

    app.path()
        .resource_dir()
        .map(|path| path.join("runtime"))
        .map_err(|error| format!("cannot resolve packaged runtime resources: {error}"))
}

pub(crate) fn packaged_runtime_root(bundle_root: &Path) -> AppResult<PathBuf> {
    if !bundle_root.is_absolute()
        || bundle_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(
            "packaged app bundle root must be an absolute path without parent traversal"
                .to_string(),
        );
    }
    if bundle_root.file_name() != Some(std::ffi::OsStr::new(PACKAGED_APP_BUNDLE_NAME)) {
        return Err("packaged app bundle root has an unexpected name".to_string());
    }
    Ok(bundle_root
        .join(PACKAGED_CONTENTS_DIRECTORY_NAME)
        .join(PACKAGED_RESOURCES_DIRECTORY_NAME)
        .join(PACKAGED_RUNTIME_DIRECTORY_NAME))
}

#[cfg(any(test, not(debug_assertions)))]
fn packaged_node_binary(root: &Path) -> AppResult<PathBuf> {
    let path = root.join(PACKAGED_NODE_BINARY_NAME);
    let metadata = fs::metadata(&path).map_err(|error| {
        format!(
            "packaged Node executable is missing: {} ({error})",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "packaged Node executable is not a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!(
            "packaged Node executable is not executable: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn node_binary(_root: &Path) -> AppResult<PathBuf> {
    #[cfg(debug_assertions)]
    {
        if let Some(configured) = env::var_os("CORNELL_DESKTOP_NODE_BINARY") {
            return Ok(PathBuf::from(configured));
        }

        Ok(PathBuf::from(PACKAGED_NODE_BINARY_NAME))
    }

    #[cfg(not(debug_assertions))]
    {
        packaged_node_binary(_root)
    }
}

fn launcher_path(root: &Path) -> AppResult<PathBuf> {
    #[cfg(debug_assertions)]
    let path = if let Some(configured) = env::var_os("CORNELL_DESKTOP_LAUNCHER") {
        PathBuf::from(configured)
    } else {
        let packaged_path = root.join("sidecar").join("launcher.cjs");
        if packaged_path.is_file() {
            packaged_path
        } else {
            root.join("src-tauri").join("sidecar").join("launcher.cjs")
        }
    };

    #[cfg(not(debug_assertions))]
    let path = root.join("sidecar").join("launcher.cjs");

    if !path.is_absolute() {
        return Err("sidecar launcher path must be absolute".to_string());
    }
    if !path.is_file() {
        return Err(format!("sidecar launcher is missing: {}", path.display()));
    }
    Ok(path)
}

fn process_status(status: &ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit-code-{code}");
    }

    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return format!("signal-{signal}");
    }

    "exit-status-unknown".to_string()
}

fn sanitize_bootstrap_stderr(stderr: &[u8]) -> &'static str {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    let message = message.trim();
    if message.is_empty() {
        return "empty";
    }
    if message.contains("permission denied") || message.contains("eperm") {
        return "permission-denied";
    }
    if message.contains("cannot find module") || message.contains("module not found") {
        return "dependency-unavailable";
    }
    if message.contains("no such file") || message.contains("enoent") {
        return "required-file-unavailable";
    }
    if message.contains("database") || message.contains("sqlite") {
        return "storage-operation-failed";
    }
    "reported-error"
}

fn parse_bootstrap_message(
    output: &Output,
    operation: &'static str,
) -> Result<BootstrapMessage, DesktopStorageBootstrapError> {
    let mut has_nonempty_output = false;
    for line in String::from_utf8_lossy(&output.stdout).lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        has_nonempty_output = true;
        if let Ok(message) = serde_json::from_str::<BootstrapMessage>(line) {
            return Ok(message);
        }
    }

    Err(DesktopStorageBootstrapError::from_output(
        operation,
        if has_nonempty_output {
            "malformed-result"
        } else {
            "no-result"
        },
        output,
    ))
}

fn required_absolute_path(value: Option<String>, label: &str) -> AppResult<PathBuf> {
    let value = value.ok_or_else(|| format!("desktop storage bootstrap omitted {label}"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("desktop storage {label} is not absolute"));
    }
    Ok(path)
}

fn database_url_path(database_url: &str) -> AppResult<PathBuf> {
    let path = database_url
        .strip_prefix("file:")
        .ok_or_else(|| "desktop sidecar DATABASE_URL must use file: scheme".to_string())?;
    if path.contains('?') || path.contains('#') {
        return Err(
            "desktop sidecar DATABASE_URL must not contain a query or fragment".to_string(),
        );
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("desktop sidecar DATABASE_URL must contain an absolute path".to_string());
    }
    Ok(path)
}

#[derive(Clone, Debug)]
pub(crate) enum BootstrapOutcome {
    Ready {
        storage: StorageLayout,
        recovery_snapshot: Option<DesktopDatabaseRecoverySnapshot>,
    },
    Recovery(DesktopDatabaseRecoverySnapshot),
}

fn launcher_command(
    root: &Path,
    command_name: &str,
    storage: Option<&StorageLayout>,
) -> AppResult<Command> {
    let launcher = launcher_path(root)?;
    let node = node_binary(root)?;
    let mut command = Command::new(&node);
    command
        .arg(&launcher)
        .arg(command_name)
        .current_dir(root)
        .env("CORNELL_DESKTOP_PROJECT_ROOT", root)
        .env(
            "CORNELL_DESKTOP_APPLICATION_ID",
            instance::desktop_application_id(),
        )
        .env("PRISMA_PROVIDER", "sqlite");
    if let Some(storage) = storage {
        command.env(
            "CORNELL_DESKTOP_APPLICATION_SUPPORT_ROOT",
            storage.application_support_root(),
        );
    }
    Ok(command)
}

fn launch_command(
    root: &Path,
    command_name: &str,
    storage: Option<&StorageLayout>,
) -> AppResult<std::process::Output> {
    launcher_command(root, command_name, storage)?
        .output()
        .map_err(|error| format!("desktop launcher process could not start: {error}"))
}

fn launch_storage_command(
    root: &Path,
    command_name: &'static str,
    storage: Option<&StorageLayout>,
) -> Result<Output, DesktopStorageBootstrapError> {
    let mut command = launcher_command(root, command_name, storage)
        .map_err(|_| DesktopStorageBootstrapError::spawn_failed(command_name))?;
    command
        .output()
        .map_err(|_| DesktopStorageBootstrapError::spawn_failed(command_name))
}

fn launch_backup_recovery(
    root: &Path,
    storage: &StorageLayout,
    request: &DesktopBackupRecoveryRequest,
) -> AppResult<std::process::Output> {
    let payload = serde_json::to_string(request)
        .map_err(|_| "desktop backup recovery request could not be encoded".to_string())?;
    launcher_command(root, DESKTOP_BACKUP_RECOVERY_COMMAND, Some(storage))?
        .arg(payload)
        .output()
        .map_err(|error| format!("desktop backup recovery sidecar could not start: {error}"))
}

fn parse_backup_recovery_message(output: &[u8]) -> AppResult<DesktopBackupRecoveryResponse> {
    let message = String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<DesktopBackupRecoveryResponse>(line).ok())
        .ok_or_else(|| "desktop backup recovery sidecar did not return a result".to_string())?;
    if message.kind != "desktop-backup-recovery"
        || message.schema_version != DESKTOP_BACKUP_RECOVERY_PROTOCOL_VERSION
        || message.phase != "preflight"
        || !matches!(
            message.status.as_str(),
            "ready" | "recovery-required" | "not-recovered"
        )
        || !message.error_code.as_deref().map_or(true, |code| {
            matches!(
                code,
                "invalid-request"
                    | "unsupported-protocol-version"
                    | "storage-unavailable"
                    | "database-unavailable"
                    | "runtime-unavailable"
                    | "sidecar-unavailable"
                    | "protocol-error"
                    | "recovery-transition-failed"
                    | "invalid-response"
            )
        })
        || (message.status == "ready"
            && (message.error_code.is_some() || message.recovery_snapshot.is_some()))
        || (message.status == "recovery-required"
            && (message.error_code.is_some() || message.recovery_snapshot.is_none()))
        || (message.status == "not-recovered"
            && (message.error_code.is_none() || message.recovery_snapshot.is_some()))
    {
        return Err("desktop backup recovery sidecar returned an invalid result".to_string());
    }
    if let Some(snapshot) = message.recovery_snapshot.as_ref() {
        validate_database_recovery_snapshot(snapshot)?;
    }
    Ok(message)
}

pub(crate) fn backup_recovery_command_error(error_code: &str) -> DesktopBackupRecoveryResponse {
    DesktopBackupRecoveryResponse::not_recovered(error_code)
}

pub(crate) fn run_desktop_backup_recovery_probe(
    app: &AppHandle,
    request_value: serde_json::Value,
) -> DesktopBackupRecoveryResponse {
    let request = match serde_json::from_value::<DesktopBackupRecoveryRequest>(request_value) {
        Ok(request)
            if request.kind == "desktop-backup-recovery"
                && request.schema_version == DESKTOP_BACKUP_RECOVERY_PROTOCOL_VERSION
                && matches!(
                    request.reason.as_str(),
                    "backup_configuration_invalid"
                        | "backup_database_unavailable"
                        | "backup_storage_failure"
                ) =>
        {
            request
        }
        Ok(request) if request.schema_version != DESKTOP_BACKUP_RECOVERY_PROTOCOL_VERSION => {
            return backup_recovery_command_error("unsupported-protocol-version");
        }
        _ => return backup_recovery_command_error("invalid-request"),
    };
    let Some(storage) = app.try_state::<StorageLayout>() else {
        return backup_recovery_command_error("storage-unavailable");
    };
    let root = match runtime_project_root(app) {
        Ok(root) => root,
        Err(_) => return backup_recovery_command_error("runtime-unavailable"),
    };
    let output = match launch_backup_recovery(&root, storage.inner(), &request) {
        Ok(output) if output.status.success() => output,
        Ok(_) | Err(_) => return backup_recovery_command_error("sidecar-unavailable"),
    };
    match parse_backup_recovery_message(&output.stdout) {
        Ok(response) => response,
        Err(_) => backup_recovery_command_error("protocol-error"),
    }
}

fn resolve_data_backup_location(
    location: &DesktopDataBackupLocation,
    expected_dialog_kind: Option<DesktopFileDialogKind>,
    storage: &StorageLayout,
    selection_store: &DesktopFileSelectionStore,
) -> Result<DesktopDataBackupSidecarLocation, &'static str> {
    match location {
        DesktopDataBackupLocation::ManagedBackup { backup_id } => {
            if !safe_identifier(backup_id, 128) {
                return Err("managed-source-invalid");
            }
            Ok(DesktopDataBackupSidecarLocation::ManagedBackup {
                backup_id: backup_id.clone(),
            })
        }
        DesktopDataBackupLocation::ExternalSelection { selection_id } => {
            let expected_dialog_kind = expected_dialog_kind.ok_or("invalid-request")?;
            let path = selection_store.resolve(
                selection_id,
                expected_dialog_kind,
                storage.application_support_root(),
            )?;
            let path = path.to_str().ok_or("invalid-path")?.to_string();
            Ok(DesktopDataBackupSidecarLocation::ExternalFile {
                origin: "native-dialog",
                path,
            })
        }
    }
}

fn parse_data_backup_location(
    value: serde_json::Value,
) -> Result<Option<DesktopDataBackupLocation>, &'static str> {
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|_| "invalid-request")
}

fn build_data_backup_sidecar_request(
    request: DesktopDataBackupOperationRequest,
    storage: &StorageLayout,
    selection_store: &DesktopFileSelectionStore,
    operation_id: Option<String>,
    restore_mode: DesktopRestoreMode,
) -> Result<DesktopDataBackupSidecarRequest, &'static str> {
    let DesktopDataBackupOperationRequest {
        schema_version,
        operation,
        source: raw_source,
        destination: raw_destination,
        confirmed,
    } = request;
    if schema_version != DESKTOP_DATA_BACKUP_PROTOCOL_VERSION {
        return Err("unsupported-protocol-version");
    }
    let source = parse_data_backup_location(raw_source)?;
    let destination = parse_data_backup_location(raw_destination)?;
    let operation_name = operation.wire_name();
    let recovery_only = match &operation {
        DesktopDataBackupOperation::Restore => Some(restore_mode.is_recovery_only()),
        DesktopDataBackupOperation::Export | DesktopDataBackupOperation::Delete => None,
    };
    let (source, destination) = match operation {
        DesktopDataBackupOperation::Export => {
            if source.is_some() {
                return Err("invalid-request");
            }
            let destination = destination.as_ref().ok_or("invalid-request")?;
            if !matches!(
                destination,
                DesktopDataBackupLocation::ExternalSelection { .. }
            ) {
                return Err("invalid-request");
            }
            let destination = resolve_data_backup_location(
                destination,
                Some(DesktopFileDialogKind::SaveDestination),
                storage,
                selection_store,
            )?;
            (None, Some(destination))
        }
        DesktopDataBackupOperation::Restore => {
            if destination.is_some() {
                return Err("invalid-request");
            }
            if confirmed != Some(true) {
                return Err("confirmation-required");
            }
            let source = source.as_ref().ok_or("invalid-request")?;
            let expected_dialog_kind = match source {
                DesktopDataBackupLocation::ManagedBackup { .. } => None,
                DesktopDataBackupLocation::ExternalSelection { .. } => {
                    Some(DesktopFileDialogKind::OpenExternalSource)
                }
            };
            let source = resolve_data_backup_location(
                source,
                expected_dialog_kind,
                storage,
                selection_store,
            )?;
            (Some(source), None)
        }
        DesktopDataBackupOperation::Delete => {
            if source.is_some() || destination.is_some() {
                return Err("invalid-request");
            }
            if confirmed != Some(true) {
                return Err("confirmation-required");
            }
            (None, None)
        }
    };
    Ok(DesktopDataBackupSidecarRequest {
        kind: "desktop-data-backup-operation",
        schema_version: DESKTOP_DATA_BACKUP_PROTOCOL_VERSION,
        operation: operation_name,
        source,
        destination,
        confirmed,
        operation_id,
        recovery_only,
    })
}

fn launch_data_backup_operation(
    root: &Path,
    storage: &StorageLayout,
    request: &DesktopDataBackupSidecarRequest,
) -> AppResult<std::process::Output> {
    let payload = serde_json::to_string(request)
        .map_err(|_| "desktop data and backup request could not be encoded".to_string())?;
    launcher_command(root, DESKTOP_DATA_BACKUP_COMMAND, Some(storage))?
        .arg(payload)
        .output()
        .map_err(|error| format!("desktop data and backup sidecar could not start: {error}"))
}

fn parse_data_backup_operation_message(
    output: &[u8],
    expected_operation: &str,
) -> AppResult<DesktopDataBackupOperationResponse> {
    let message = String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<DesktopDataBackupOperationResponse>(line).ok())
        .ok_or_else(|| "desktop data and backup sidecar did not return a result".to_string())?;
    if message.kind != "desktop-data-backup-operation"
        || message.schema_version != DESKTOP_DATA_BACKUP_PROTOCOL_VERSION
        || !matches!(message.status.as_str(), "success" | "cancelled" | "error")
        || !matches!(
            message.phase.as_str(),
            "request" | "validation" | "operation" | "complete"
        )
    {
        return Err("desktop data and backup sidecar returned an invalid result".to_string());
    }
    if message.operation.as_deref() != Some(expected_operation) {
        return Err("desktop data and backup sidecar returned an unexpected operation".to_string());
    }
    if let Some(result) = message.result.as_ref() {
        if message.operation.as_deref() != Some("export")
            || result.file_name.is_empty()
            || result.file_name.len() > 255
            || result.file_name.contains('/')
            || result.file_name.contains('\\')
            || result.file_name.contains('\0')
            || result.size == 0
        {
            return Err(
                "desktop data and backup sidecar returned an invalid export result".to_string(),
            );
        }
    }
    if message.status == "success" && (!message.ok || message.error_code.is_some()) {
        return Err(
            "desktop data and backup sidecar returned an inconsistent success result".to_string(),
        );
    }
    if message.status == "success"
        && message.operation.as_deref() == Some("export")
        && message.result.is_none()
    {
        return Err("desktop data and backup sidecar export result is missing".to_string());
    }
    if message.status == "cancelled"
        && (message.ok || message.error_code.is_some() || message.result.is_some())
    {
        return Err(
            "desktop data and backup sidecar returned an inconsistent cancel result".to_string(),
        );
    }
    if message.status == "error"
        && (message.ok || message.error_code.is_none() || message.result.is_some())
    {
        return Err(
            "desktop data and backup sidecar returned an inconsistent error result".to_string(),
        );
    }
    Ok(message)
}

pub(crate) fn run_data_backup_operation(
    app: &AppHandle,
    request_value: serde_json::Value,
) -> DesktopDataBackupOperationResponse {
    run_data_backup_operation_with_operation_id(app, request_value, None)
}

pub(crate) fn run_data_backup_operation_with_operation_id(
    app: &AppHandle,
    request_value: serde_json::Value,
    operation_id: Option<String>,
) -> DesktopDataBackupOperationResponse {
    run_data_backup_operation_with_restore_mode(
        app,
        request_value,
        operation_id,
        DesktopRestoreMode::Normal,
    )
}

pub(crate) fn run_data_backup_operation_with_restore_mode(
    app: &AppHandle,
    request_value: serde_json::Value,
    operation_id: Option<String>,
    restore_mode: DesktopRestoreMode,
) -> DesktopDataBackupOperationResponse {
    let Some(storage) = app.try_state::<StorageLayout>() else {
        return desktop_data_backup_command_error(None, "request", "storage-unavailable");
    };
    let Some(selection_store) = app.try_state::<DesktopFileSelectionStore>() else {
        return desktop_data_backup_command_error(None, "request", "selection-store-failed");
    };
    let request = match serde_json::from_value::<DesktopDataBackupOperationRequest>(request_value) {
        Ok(request) => request,
        Err(_) => return desktop_data_backup_command_error(None, "request", "invalid-request"),
    };
    let operation = request.operation.wire_name();
    let sidecar_request = match build_data_backup_sidecar_request(
        request,
        storage.inner(),
        selection_store.inner(),
        operation_id,
        restore_mode,
    ) {
        Ok(request) => request,
        Err(error_code) => {
            return desktop_data_backup_command_error(Some(operation), "validation", error_code);
        }
    };
    let root = match runtime_project_root(app) {
        Ok(root) => root,
        Err(_) => {
            return desktop_data_backup_command_error(
                Some(operation),
                "request",
                "runtime-unavailable",
            );
        }
    };
    let output = match launch_data_backup_operation(&root, storage.inner(), &sidecar_request) {
        Ok(output) => output,
        Err(_) => {
            return desktop_data_backup_command_error(
                Some(operation),
                "operation",
                "sidecar-unavailable",
            );
        }
    };
    match parse_data_backup_operation_message(&output.stdout, operation) {
        Ok(response) => response,
        Err(_) => desktop_data_backup_command_error(Some(operation), "operation", "protocol-error"),
    }
}

fn managed_backup_catalog_error_code_is_safe(value: &str) -> bool {
    matches!(value, "storage-unavailable" | "invalid-catalog")
}

fn managed_backup_catalog_timestamp_is_safe(value: &str) -> bool {
    if value.len() != 24 || !value.ends_with('Z') {
        return false;
    }
    let bytes = value.as_bytes();
    for (index, expected) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ] {
        if bytes[index] != expected {
            return false;
        }
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        ![4, 7, 10, 13, 16, 19, 23].contains(&index) && byte.is_ascii_digit()
            || [4, 7, 10, 13, 16, 19, 23].contains(&index)
    })
}

fn parse_managed_backup_catalog_message(
    output: &[u8],
) -> AppResult<DesktopManagedBackupCatalogResponse> {
    let message = String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<DesktopManagedBackupCatalogResponse>(line).ok())
        .ok_or_else(|| "managed backup catalog sidecar did not return a result".to_string())?;
    if message.kind != "desktop-managed-backup-catalog"
        || message.schema_version != DESKTOP_MANAGED_BACKUP_CATALOG_PROTOCOL_VERSION
        || message.phase != "catalog"
        || !matches!(message.status.as_str(), "ready" | "empty" | "error")
        || message
            .error_code
            .as_deref()
            .is_some_and(|code| !managed_backup_catalog_error_code_is_safe(code))
    {
        return Err("managed backup catalog returned an invalid result".to_string());
    }

    let mut identifiers = HashSet::new();
    for entry in &message.backups {
        if !safe_identifier(&entry.backup_id, 128)
            || entry.file_name != entry.backup_id
            || !safe_identifier(&entry.file_name, 128)
            || !managed_backup_catalog_timestamp_is_safe(&entry.created_at)
            || !identifiers.insert(entry.backup_id.as_str())
        {
            return Err("managed backup catalog returned invalid metadata".to_string());
        }
    }
    if message.backups.windows(2).any(|entries| {
        entries[0].created_at < entries[1].created_at
            || (entries[0].created_at == entries[1].created_at
                && entries[0].backup_id > entries[1].backup_id)
    }) {
        return Err("managed backup catalog returned a non-deterministic order".to_string());
    }

    match message.status.as_str() {
        "ready" if message.error_code.is_none() && !message.backups.is_empty() => {}
        "empty" if message.error_code.is_none() && message.backups.is_empty() => {}
        "error" if message.error_code.is_some() && message.backups.is_empty() => {}
        _ => return Err("managed backup catalog returned an inconsistent result".to_string()),
    }
    Ok(message)
}

fn launch_managed_backup_catalog(
    root: &Path,
    storage: &StorageLayout,
) -> AppResult<std::process::Output> {
    launcher_command(root, DESKTOP_MANAGED_BACKUP_CATALOG_COMMAND, Some(storage))?
        .output()
        .map_err(|error| format!("managed backup catalog sidecar could not start: {error}"))
}

pub(crate) fn read_managed_backup_catalog(app: &AppHandle) -> DesktopManagedBackupCatalogResponse {
    let Some(storage) = app.try_state::<StorageLayout>() else {
        return managed_backup_catalog_command_error("storage-unavailable");
    };
    let root = match runtime_project_root(app) {
        Ok(root) => root,
        Err(_) => return managed_backup_catalog_command_error("runtime-unavailable"),
    };
    let output = match launch_managed_backup_catalog(&root, storage.inner()) {
        Ok(output) => output,
        Err(_) => return managed_backup_catalog_command_error("sidecar-unavailable"),
    };
    if !output.status.success() {
        return managed_backup_catalog_command_error("sidecar-unavailable");
    }
    match parse_managed_backup_catalog_message(&output.stdout) {
        Ok(response) => response,
        Err(_) => managed_backup_catalog_command_error("invalid-catalog"),
    }
}

fn launch_pending_restore_status(
    root: &Path,
    storage: &StorageLayout,
) -> AppResult<std::process::Output> {
    launcher_command(root, DESKTOP_PENDING_RESTORE_STATUS_COMMAND, Some(storage))?
        .output()
        .map_err(|error| format!("pending restore status sidecar could not start: {error}"))
}

fn parse_pending_restore_status_message(
    output: &[u8],
) -> AppResult<DesktopPendingRestoreStatusResponse> {
    let message = String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<DesktopPendingRestoreStatusResponse>(line).ok())
        .ok_or_else(|| "pending restore status sidecar did not return a result".to_string())?;
    if message.kind != "desktop-pending-restore-status"
        || message.schema_version != DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION
        || message.phase != "status"
        || !matches!(message.status.as_str(), "none" | "available" | "invalid")
        || message.operation_id.is_some()
    {
        return Err("pending restore status returned an invalid result".to_string());
    }
    match message.status.as_str() {
        "none" if message.error_code.is_none() && message.pending.is_none() => {}
        "available" if message.error_code.is_none() && message.pending.is_some() => {}
        "invalid" if message.error_code.is_some() && message.pending.is_none() => {}
        _ => return Err("pending restore status returned an inconsistent result".to_string()),
    }
    if let Some(pending) = message.pending.as_ref() {
        if !pending_restore_token_is_safe(&pending.pending_id)
            || !pending_restore_token_is_safe(&pending.manifest_token)
            || !pending_restore_token_is_safe(&pending.candidate_digest)
            || !pending_restore_token_is_safe(&pending.candidate_schema_identity)
            || pending.candidate_size == 0
            || pending.source_kind != "managed-backup" && pending.source_kind != "external-file"
            || pending.created_at.is_empty()
        {
            return Err("pending restore status returned invalid metadata".to_string());
        }
    }
    Ok(message)
}

pub(crate) fn read_pending_restore_status(app: &AppHandle) -> DesktopPendingRestoreStatusResponse {
    let Some(storage) = app.try_state::<StorageLayout>() else {
        return pending_restore_status_command_error("storage-unavailable");
    };
    let root = match runtime_project_root(app) {
        Ok(root) => root,
        Err(_) => return pending_restore_status_command_error("runtime-unavailable"),
    };
    let output = match launch_pending_restore_status(&root, storage.inner()) {
        Ok(output) => output,
        Err(_) => return pending_restore_status_command_error("sidecar-unavailable"),
    };
    match parse_pending_restore_status_message(&output.stdout) {
        Ok(response) => response,
        Err(_) => pending_restore_status_command_error("protocol-error"),
    }
}

fn build_pending_restore_sidecar_request(
    request_value: &serde_json::Value,
    operation_id: String,
    restore_mode: DesktopRestoreMode,
) -> Result<DesktopPendingRestoreSidecarRequest, &'static str> {
    let request =
        serde_json::from_value::<DesktopPendingRestoreResumeRequest>(request_value.clone())
            .map_err(|_| "invalid-request")?;
    if request.schema_version != DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION {
        return Err("unsupported-protocol-version");
    }
    if !pending_restore_token_is_safe(&request.pending_id)
        || !pending_restore_token_is_safe(&request.manifest_token)
    {
        return Err("invalid-request");
    }
    if !request.confirmed {
        return Err("confirmation-required");
    }
    Ok(DesktopPendingRestoreSidecarRequest {
        kind: "desktop-pending-restore-resume",
        schema_version: DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION,
        pending_id: request.pending_id,
        manifest_token: request.manifest_token,
        confirmed: true,
        operation_id,
        recovery_only: restore_mode.is_recovery_only(),
    })
}

fn launch_pending_restore_operation(
    root: &Path,
    storage: &StorageLayout,
    request: &DesktopPendingRestoreSidecarRequest,
) -> AppResult<std::process::Output> {
    let payload = serde_json::to_string(request)
        .map_err(|_| "pending restore request could not be encoded".to_string())?;
    launcher_command(root, DESKTOP_PENDING_RESTORE_RESUME_COMMAND, Some(storage))?
        .arg(payload)
        .output()
        .map_err(|error| format!("pending restore sidecar could not start: {error}"))
}

fn parse_pending_restore_resume_message(
    output: &[u8],
    expected_operation_id: &str,
    expected_pending_id: &str,
) -> AppResult<DesktopPendingRestoreResumeResponse> {
    let message = String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<DesktopPendingRestoreResumeResponse>(line).ok())
        .ok_or_else(|| "pending restore sidecar did not return a result".to_string())?;
    if message.kind != "desktop-pending-restore-resume"
        || message.schema_version != DESKTOP_PENDING_RESTORE_PROTOCOL_VERSION
        || message.operation_id.as_deref() != Some(expected_operation_id)
        || message.pending_id.as_deref() != Some(expected_pending_id)
        || !matches!(message.status.as_str(), "success" | "error")
        || !matches!(
            message.phase.as_str(),
            "request" | "validation" | "operation" | "complete"
        )
    {
        return Err("pending restore sidecar returned an invalid result".to_string());
    }
    if message.status == "success"
        && (!message.ok
            || message.error_code.is_some()
            || message.result.is_none()
            || message.phase != "complete")
    {
        return Err("pending restore sidecar returned an inconsistent success result".to_string());
    }
    if message.status == "error"
        && (message.ok || message.error_code.is_none() || message.result.is_some())
    {
        return Err("pending restore sidecar returned an inconsistent error result".to_string());
    }
    if let Some(result) = message.result.as_ref() {
        if result
            .safety_backup_id
            .as_ref()
            .is_some_and(|backup_id| !safe_identifier(backup_id, 128))
            || result.size == 0
        {
            return Err("pending restore sidecar returned invalid result metadata".to_string());
        }
    }
    Ok(message)
}

pub(crate) fn run_pending_restore_operation_with_operation_id(
    app: &AppHandle,
    request_value: serde_json::Value,
    operation_id: String,
) -> DesktopPendingRestoreResumeResponse {
    run_pending_restore_operation_with_restore_mode(
        app,
        request_value,
        operation_id,
        DesktopRestoreMode::Normal,
    )
}

pub(crate) fn run_pending_restore_operation_with_restore_mode(
    app: &AppHandle,
    request_value: serde_json::Value,
    operation_id: String,
    restore_mode: DesktopRestoreMode,
) -> DesktopPendingRestoreResumeResponse {
    let pending_id = request_value
        .get("pendingId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let operation_id_for_error = Some(operation_id.as_str());
    let pending_id_for_error = pending_id.as_deref();
    let sidecar_request = match build_pending_restore_sidecar_request(
        &request_value,
        operation_id.clone(),
        restore_mode,
    ) {
        Ok(request) => request,
        Err(error_code) => {
            return pending_restore_resume_command_error(
                operation_id_for_error,
                pending_id_for_error,
                "validation",
                error_code,
            );
        }
    };
    let Some(storage) = app.try_state::<StorageLayout>() else {
        return pending_restore_resume_command_error(
            operation_id_for_error,
            pending_id_for_error,
            "request",
            "storage-unavailable",
        );
    };
    let root = match runtime_project_root(app) {
        Ok(root) => root,
        Err(_) => {
            return pending_restore_resume_command_error(
                operation_id_for_error,
                pending_id_for_error,
                "request",
                "runtime-unavailable",
            );
        }
    };
    let output = match launch_pending_restore_operation(&root, storage.inner(), &sidecar_request) {
        Ok(output) => output,
        Err(_) => {
            return pending_restore_resume_command_error(
                operation_id_for_error,
                pending_id_for_error,
                "operation",
                "sidecar-unavailable",
            );
        }
    };
    match parse_pending_restore_resume_message(
        &output.stdout,
        &operation_id,
        &sidecar_request.pending_id,
    ) {
        Ok(response) => response,
        Err(_) => pending_restore_resume_command_error(
            operation_id_for_error,
            pending_id_for_error,
            "operation",
            "protocol-error",
        ),
    }
}

fn storage_layout_from_message(message: BootstrapMessage) -> AppResult<StorageLayout> {
    let database_url = message
        .database_url
        .ok_or_else(|| "desktop storage bootstrap omitted databaseUrl".to_string())?;
    let database_path = database_url_path(&database_url)?;
    let layout = StorageLayout {
        application_support_root: required_absolute_path(
            message.application_support_root,
            "applicationSupportRoot",
        )?,
        live_directory: required_absolute_path(message.live_directory, "liveDirectory")?,
        database_path: required_absolute_path(message.database_path, "databasePath")?,
        database_url,
        backups_directory: required_absolute_path(message.backups_directory, "backupsDirectory")?,
        settings_directory: required_absolute_path(
            message.settings_directory,
            "settingsDirectory",
        )?,
        logs_directory: required_absolute_path(message.logs_directory, "logsDirectory")?,
        pending_restore_directory: required_absolute_path(
            message.pending_restore_directory,
            "pendingRestoreDirectory",
        )?,
    };
    if layout.database_path != database_path {
        return Err("desktop storage databaseUrl does not match databasePath".to_string());
    }
    let expected_live = layout.application_support_root.join("live");
    let expected_database = expected_live.join("notebook.sqlite");
    let expected_backups = layout.application_support_root.join("backups");
    let expected_settings = layout.application_support_root.join("settings");
    let expected_logs = layout.application_support_root.join("logs");
    let expected_pending_restore = layout.application_support_root.join("pending-restore");
    if layout.live_directory != expected_live
        || layout.database_path != expected_database
        || layout.backups_directory != expected_backups
        || layout.settings_directory != expected_settings
        || layout.logs_directory != expected_logs
        || layout.pending_restore_directory != expected_pending_restore
    {
        return Err(
            "desktop storage paths are outside the approved Application Support layout".to_string(),
        );
    }
    Ok(layout)
}

pub(crate) fn resolve_storage_layout(
    root: &Path,
) -> Result<StorageLayout, DesktopStorageBootstrapError> {
    let output = launch_storage_command(root, "paths", None)?;
    let message = parse_bootstrap_message(&output, "paths")?;
    if message.kind != "storage-paths" || message.status != "paths" {
        return Err(DesktopStorageBootstrapError::from_output(
            "paths",
            "invalid-result",
            &output,
        ));
    }
    if !output.status.success() {
        return Err(DesktopStorageBootstrapError::from_output(
            "paths",
            "non-zero-exit",
            &output,
        ));
    }
    storage_layout_from_message(message).map_err(|_| {
        DesktopStorageBootstrapError::from_output("paths", "storage-layout-invalid", &output)
    })
}

pub(crate) fn run_bootstrap_with_storage(
    root: &Path,
    storage: &StorageLayout,
) -> Result<BootstrapOutcome, DesktopStorageBootstrapError> {
    let output = launch_storage_command(root, "bootstrap", Some(storage))?;
    let message = parse_bootstrap_message(&output, "bootstrap")?;
    if message.kind != "bootstrap" {
        return Err(DesktopStorageBootstrapError::from_output(
            "bootstrap",
            "invalid-message-kind",
            &output,
        ));
    }
    if message.status == "failed" {
        let result_context = match (message.code.as_deref(), message.context.as_deref()) {
            (Some("bootstrap-failed"), Some("storage-options")) => "storage-options",
            (Some("bootstrap-failed"), Some("storage-bootstrap")) => "storage-bootstrap",
            (Some("bootstrap-failed"), Some("storage-bootstrap-result")) => {
                "storage-bootstrap-result"
            }
            _ => "invalid-failure-message",
        };
        let result_context = if output.status.success() {
            "failure-message-success-exit"
        } else {
            result_context
        };
        return Err(DesktopStorageBootstrapError::from_output(
            "bootstrap",
            result_context,
            &output,
        ));
    }
    if !output.status.success() {
        return Err(DesktopStorageBootstrapError::from_output(
            "bootstrap",
            "non-zero-exit",
            &output,
        ));
    }

    if message.status == "recovery" {
        if message.application_support_root.is_some()
            || message.live_directory.is_some()
            || message.database_path.is_some()
            || message.database_url.is_some()
            || message.backups_directory.is_some()
            || message.settings_directory.is_some()
            || message.logs_directory.is_some()
            || message.pending_restore_directory.is_some()
            || message.reason.is_some()
            || message.created.is_some()
        {
            return Err(DesktopStorageBootstrapError::from_output(
                "bootstrap",
                "recovery-result-invalid",
                &output,
            ));
        }
        let snapshot = message.recovery_snapshot.ok_or_else(|| {
            DesktopStorageBootstrapError::from_output(
                "bootstrap",
                "recovery-snapshot-missing",
                &output,
            )
        })?;
        if validate_database_recovery_snapshot(&snapshot).is_err() {
            return Err(DesktopStorageBootstrapError::from_output(
                "bootstrap",
                "recovery-snapshot-invalid",
                &output,
            ));
        }
        return Ok(BootstrapOutcome::Recovery(snapshot));
    }
    if message.status != "ready" {
        return Err(DesktopStorageBootstrapError::from_output(
            "bootstrap",
            "invalid-status",
            &output,
        ));
    }

    if let Some(snapshot) = message.recovery_snapshot.as_ref() {
        if validate_database_recovery_snapshot(snapshot).is_err() {
            return Err(DesktopStorageBootstrapError::from_output(
                "bootstrap",
                "ready-snapshot-invalid",
                &output,
            ));
        }
    }
    let recovery_snapshot = message.recovery_snapshot.clone();
    let returned = storage_layout_from_message(message).map_err(|_| {
        DesktopStorageBootstrapError::from_output("bootstrap", "storage-layout-invalid", &output)
    })?;
    if returned.application_support_root != storage.application_support_root
        || returned.database_path != storage.database_path
        || returned.settings_directory != storage.settings_directory
    {
        return Err(DesktopStorageBootstrapError::from_output(
            "bootstrap",
            "storage-layout-changed",
            &output,
        ));
    }
    Ok(BootstrapOutcome::Ready {
        storage: storage.clone(),
        recovery_snapshot,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StagedMigrationOutcome {
    NoPending,
    Switched,
    Failed { code: String },
}

pub(crate) fn run_staged_migration_command(
    root: &Path,
    storage: &StorageLayout,
) -> AppResult<StagedMigrationOutcome> {
    let output = launch_command(root, "staged-migrate", Some(storage))?;
    let message = parse_staged_migration_message(&output.stdout)?;
    if message.kind != "staged-migration" {
        return Err("staged migration returned an unknown message".to_string());
    }
    if message.status == "failed" || !output.status.success() {
        return Ok(StagedMigrationOutcome::Failed {
            code: message
                .code
                .unwrap_or_else(|| STAGED_MIGRATION_FAILURE_CODE.to_string()),
        });
    }
    match message.status.as_str() {
        "no-pending" => Ok(StagedMigrationOutcome::NoPending),
        "switched" => Ok(StagedMigrationOutcome::Switched),
        _ => Err("staged migration returned an invalid status".to_string()),
    }
}

pub(crate) fn validate_database_command(root: &Path, storage: &StorageLayout) -> AppResult<()> {
    let output = launch_command(root, "validate-database", Some(storage))?;
    let message = parse_database_validation_message(&output.stdout)?;
    if message.kind != "database-validation" {
        return Err("database validation returned an unknown message".to_string());
    }
    if message.status != "ready" || !output.status.success() {
        return Err(format!(
            "database validation failed: {}",
            message.reason.as_deref().unwrap_or("unknown reason")
        ));
    }
    Ok(())
}

fn parse_staged_migration_message(output: &[u8]) -> AppResult<StagedMigrationMessage> {
    String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<StagedMigrationMessage>(line).ok())
        .ok_or_else(|| "staged migration did not return a result".to_string())
}

fn parse_database_validation_message(output: &[u8]) -> AppResult<DatabaseValidationMessage> {
    String::from_utf8_lossy(output)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<DatabaseValidationMessage>(line).ok())
        .ok_or_else(|| "database validation did not return a result".to_string())
}

fn validate_ready_message(message: &ReadyMessage) -> Result<tauri::Url, SidecarStartupError> {
    if message.kind != "ready" || message.status != "ready" {
        return Err(SidecarStartupError::new("sidecar-ready-handshake-failed"));
    }
    if message.host != "127.0.0.1" || message.port == 0 || message.runtime_pid == 0 {
        return Err(SidecarStartupError::new("sidecar-ready-handshake-failed"));
    }
    if message.ready_nonce.len() != 64
        || !message
            .ready_nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SidecarStartupError::new("sidecar-ready-handshake-failed"));
    }
    let url = tauri::Url::parse(&message.url)
        .map_err(|_| SidecarStartupError::new("sidecar-ready-url-invalid"))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port() != Some(message.port)
        || url.path() != "/notes"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(SidecarStartupError::new("sidecar-ready-url-invalid"));
    }
    Ok(url)
}

fn read_ready_line(child: &mut Child) -> Result<ReadyMessage, SidecarStartupError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SidecarStartupError::new("sidecar-ready-handshake-failed"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader
            .read_line(&mut line)
            .map_err(|_| SidecarStartupError::new("sidecar-ready-handshake-failed"))
            .and_then(|_| {
                serde_json::from_str::<ReadyMessage>(line.trim())
                    .map_err(|_| SidecarStartupError::new("sidecar-ready-handshake-failed"))
            });
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(READY_TIMEOUT) {
        Ok(result) => result,
        Err(_) => Err(SidecarStartupError::new("sidecar-ready-handshake-failed")),
    }
}

fn decode_chunked_body(mut input: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let line_end = input.windows(2).position(|window| window == b"\r\n")?;
        let size_line = std::str::from_utf8(&input[..line_end]).ok()?;
        let size = usize::from_str_radix(size_line.split(';').next()?.trim(), 16).ok()?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Some(decoded);
        }
        let chunk_end = size.checked_add(2)?;
        if input.len() < chunk_end || &input[size..chunk_end] != b"\r\n" {
            return None;
        }
        decoded.extend_from_slice(&input[..size]);
        input = &input[chunk_end..];
    }
}

fn health_response_matches(response: &[u8], expected_nonce: &str) -> bool {
    let response = match std::str::from_utf8(response) {
        Ok(response) => response,
        Err(_) => return false,
    };
    let header_end = match response.find("\r\n\r\n") {
        Some(index) => index,
        None => return false,
    };
    let headers = &response[..header_end];
    let raw_body = &response.as_bytes()[header_end + 4..];
    let status_code = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok());
    if status_code != Some(200) {
        return false;
    }
    let is_chunked = headers.lines().skip(1).any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding")
                && value
                    .split(',')
                    .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"))
        })
    });
    let decoded_body = if is_chunked {
        match decode_chunked_body(raw_body) {
            Some(body) => Some(body),
            None => return false,
        }
    } else {
        None
    };
    let body = decoded_body.as_deref().unwrap_or(raw_body);
    let message = match serde_json::from_slice::<RuntimeHealthMessage>(body) {
        Ok(message) => message,
        Err(_) => return false,
    };
    message.kind == SIDECAR_HEALTH_KIND
        && message.status == "ready"
        && message.nonce == expected_nonce
}

fn wait_for_runtime(url: &tauri::Url, expected_nonce: &str) -> Result<(), SidecarStartupError> {
    let host = url
        .host_str()
        .ok_or_else(|| SidecarStartupError::new("sidecar-health-check-failed"))?;
    let port = url
        .port()
        .ok_or_else(|| SidecarStartupError::new("sidecar-health-check-failed"))?;
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(mut stream) = TcpStream::connect((host, port)) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
                SIDECAR_HEALTH_PATH,
                host
            );
            if stream.write_all(request.as_bytes()).is_ok() {
                let mut response = Vec::with_capacity(512);
                let mut read_ok = true;
                loop {
                    let mut buffer = [0u8; 1024];
                    match stream.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            if response.len() + read > MAX_HEALTH_RESPONSE_BYTES {
                                read_ok = false;
                                break;
                            }
                            response.extend_from_slice(&buffer[..read]);
                        }
                        Err(_) => {
                            read_ok = false;
                            break;
                        }
                    }
                }
                if read_ok && health_response_matches(&response, expected_nonce) {
                    return Ok(());
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(SidecarStartupError::new("sidecar-health-check-failed"))
}

pub(crate) fn start_sidecar(
    root: &Path,
    storage: &StorageLayout,
) -> Result<SidecarHandle, SidecarStartupError> {
    let launcher =
        launcher_path(root).map_err(|_| SidecarStartupError::new("sidecar-spawn-failed"))?;
    let node = node_binary(root).map_err(|_| SidecarStartupError::new("sidecar-spawn-failed"))?;
    let mut command = Command::new(&node);
    command
        .arg(&launcher)
        .arg("serve")
        .current_dir(root)
        .env("CORNELL_DESKTOP_PROJECT_ROOT", root)
        .env(
            "CORNELL_DESKTOP_APPLICATION_ID",
            instance::desktop_application_id(),
        )
        .env(
            "CORNELL_DESKTOP_APPLICATION_SUPPORT_ROOT",
            &storage.application_support_root,
        )
        .env("CORNELL_DESKTOP_LIVE_DIRECTORY", &storage.live_directory)
        .env("CORNELL_DESKTOP_DATABASE_PATH", &storage.database_path)
        .env("CORNELL_DESKTOP_DATABASE_URL", &storage.database_url)
        .env(
            "CORNELL_DESKTOP_BACKUPS_DIRECTORY",
            &storage.backups_directory,
        )
        .env(
            "CORNELL_DESKTOP_SETTINGS_DIRECTORY",
            &storage.settings_directory,
        )
        .env("CORNELL_DESKTOP_LOGS_DIRECTORY", &storage.logs_directory)
        .env(
            "CORNELL_DESKTOP_PENDING_RESTORE_DIRECTORY",
            &storage.pending_restore_directory,
        )
        .env(
            "CORNELL_DESKTOP_APPLICATION_SUPPORT_ROOT",
            storage.application_support_root(),
        )
        .env("DATABASE_URL", &storage.database_url)
        .env("PRISMA_PROVIDER", "sqlite")
        .env("NODE_ENV", "production")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(debug_assertions)]
    command.env("CORNELL_DESKTOP_ALLOW_RUNTIME_OVERRIDE", "1");
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command
        .spawn()
        .map_err(|_| SidecarStartupError::new("sidecar-spawn-failed"))?;
    let root_pid = child.id();
    let process_group_id = if cfg!(unix) { Some(root_pid) } else { None };

    let ready = match read_ready_line(&mut child) {
        Ok(message) => message,
        Err(error) => {
            let mut handle = SidecarHandle {
                child,
                root_pid,
                process_group_id,
                runtime_url: tauri::Url::parse("http://127.0.0.1/").expect("static URL is valid"),
                stopped: false,
            };
            return Err(cleanup_startup_failure(&mut handle, error));
        }
    };
    let runtime_url = match validate_ready_message(&ready) {
        Ok(url) => url,
        Err(error) => {
            let mut handle = SidecarHandle {
                child,
                root_pid,
                process_group_id,
                runtime_url: tauri::Url::parse("http://127.0.0.1/").expect("static URL is valid"),
                stopped: false,
            };
            return Err(cleanup_startup_failure(&mut handle, error));
        }
    };
    let mut handle = SidecarHandle {
        child,
        root_pid,
        process_group_id,
        runtime_url,
        stopped: false,
    };
    if let Err(error) = wait_for_runtime(&handle.runtime_url, &ready.ready_nonce) {
        return Err(cleanup_startup_failure(&mut handle, error));
    }
    Ok(handle)
}

fn cleanup_startup_failure(
    handle: &mut SidecarHandle,
    failure: SidecarStartupError,
) -> SidecarStartupError {
    if handle.stop().is_err() {
        SidecarStartupError::new("sidecar-startup-cleanup-failed")
    } else {
        failure
    }
}

fn desktop_api_url(runtime_url: &tauri::Url, path: &str) -> AppResult<tauri::Url> {
    if runtime_url.scheme() != "http"
        || runtime_url.host_str() != Some("127.0.0.1")
        || runtime_url.port().is_none()
        || runtime_url.path() != "/notes"
        || !runtime_url.username().is_empty()
        || runtime_url.password().is_some()
        || runtime_url.query().is_some()
        || runtime_url.fragment().is_some()
    {
        return Err("desktop API request runtime URL is not validated".to_string());
    }
    if path != "/api" && !path.starts_with("/api/") {
        return Err("desktop API request path is not an API path".to_string());
    }

    let url = runtime_url
        .join(path)
        .map_err(|_| "desktop API request path is invalid".to_string())?;
    if url.scheme() != runtime_url.scheme()
        || url.host_str() != runtime_url.host_str()
        || url.port() != runtime_url.port()
        || (url.path() != "/api" && !url.path().starts_with("/api/"))
    {
        return Err("desktop API request escaped the validated runtime origin".to_string());
    }
    Ok(url)
}

fn desktop_api_method(method: &str) -> AppResult<reqwest::Method> {
    match method.to_ascii_uppercase().as_str() {
        "POST" => Ok(reqwest::Method::POST),
        "PATCH" => Ok(reqwest::Method::PATCH),
        "DELETE" => Ok(reqwest::Method::DELETE),
        _ => Err("desktop API request method is not state-changing".to_string()),
    }
}

pub(crate) fn request_desktop_state_changing_api(
    runtime_url: &tauri::Url,
    request: DesktopApiRequest,
) -> AppResult<DesktopApiResponse> {
    let url = desktop_api_url(runtime_url, &request.path)?;
    let method = desktop_api_method(&request.method)?;
    let origin = runtime_url.origin().ascii_serialization();
    if origin == "null" {
        return Err("desktop API request runtime origin is not HTTP".to_string());
    }

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("desktop API client could not be created: {error}"))?;
    let mut builder = client
        .request(method, url)
        // These values are derived only from the validated sidecar URL. They
        // intentionally replace renderer-provided values at the native edge.
        .header("Origin", &origin)
        .header("Referer", runtime_url.as_str());
    for (name, value) in request.headers {
        if name.eq_ignore_ascii_case("accept") || name.eq_ignore_ascii_case("content-type") {
            builder = builder.header(name, value);
        }
    }
    if let Some(body) = request.body {
        builder = builder.body(body);
    }

    let response = builder
        .send()
        .map_err(|error| format!("desktop API request failed: {error}"))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .map_err(|error| format!("desktop API response could not be read: {error}"))?;
    Ok(DesktopApiResponse { status, body })
}

#[cfg(unix)]
fn signal_process_group(group_id: u32, signal: &str) {
    let _ = Command::new("/bin/kill")
        .args([signal, &format!("-{group_id}")])
        .status();
}

#[cfg(unix)]
fn process_group_exists(group_id: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &format!("-{group_id}")])
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => return false,
            Err(_) => return false,
        }
    }
}

impl SidecarHandle {
    pub(crate) fn runtime_url(&self) -> tauri::Url {
        self.runtime_url.clone()
    }

    pub(crate) fn stop(&mut self) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }

        #[cfg(unix)]
        if let Some(group_id) = self.process_group_id {
            signal_process_group(group_id, "-TERM");
        }

        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

        let root_exited = wait_for_child(&mut self.child, SIDECAR_SHUTDOWN_TIMEOUT);

        #[cfg(unix)]
        let mut group_exited = self
            .process_group_id
            .is_none_or(|group_id| !process_group_exists(group_id));
        #[cfg(not(unix))]
        let mut group_exited = true;

        if !root_exited || !group_exited {
            #[cfg(unix)]
            if let Some(group_id) = self.process_group_id {
                signal_process_group(group_id, "-KILL");
            }
            #[cfg(not(unix))]
            {
                let _ = self.child.kill();
            }
            let reaped = wait_for_child(&mut self.child, SIDECAR_SHUTDOWN_TIMEOUT);
            #[cfg(unix)]
            {
                group_exited = self
                    .process_group_id
                    .is_none_or(|group_id| !process_group_exists(group_id));
            }
            if !reaped || !group_exited {
                return Err(format!(
                    "sidecar cleanup did not finish for process {}",
                    self.root_pid
                ));
            }
        }
        self.stopped = true;
        Ok(())
    }
}

impl Drop for SidecarHandle {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is before the Unix epoch")
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "cornell-runtime-node-{}-{}",
                std::process::id(),
                nonce
            ));
            fs::create_dir_all(&path).expect("create runtime test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct EnvironmentVariableGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvironmentVariableGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = env::var_os(key);
            env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvironmentVariableGuard {
        fn drop(&mut self) {
            match self.previous.as_ref() {
                Some(value) => env::set_var(self.key, value),
                None => env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn sidecar_database_url_must_be_absolute() {
        assert!(database_url_path("file:/tmp/notebook.sqlite").is_ok());
        assert!(database_url_path("file:./notebook.sqlite").is_err());
        assert!(database_url_path("file:/tmp/notebook.sqlite?mode=ro").is_err());
    }

    fn synthetic_output(exit_command: &str, stdout: &[u8], stderr: &[u8]) -> Output {
        let mut output = Command::new("/bin/sh")
            .args(["-c", exit_command])
            .output()
            .expect("run disposable bootstrap output fixture");
        output.stdout = stdout.to_vec();
        output.stderr = stderr.to_vec();
        output
    }

    #[test]
    fn bootstrap_error_keeps_exit_status_and_redacts_stderr_context() {
        let output = synthetic_output(
            "exit 17",
            &[],
            b"permission denied: DATABASE_URL=file:/Users/private/notebook.sqlite note body",
        );
        let error = parse_bootstrap_message(&output, "bootstrap")
            .expect_err("empty bootstrap stdout must fail closed");
        let rendered = error.to_string();
        assert!(rendered.contains("code=bootstrap-failed"));
        assert!(rendered.contains("status=exit-code-17"));
        assert!(rendered.contains("result=no-result"));
        assert!(rendered.contains("stderr=permission-denied"));
        assert!(!rendered.contains("/Users/private"));
        assert!(!rendered.contains("DATABASE_URL"));
        assert!(!rendered.contains("note body"));
    }

    #[test]
    fn bootstrap_error_classifies_malformed_output_without_exposing_it() {
        let output = synthetic_output(
            "exit 1",
            b"not json /private/user-data/notebook.sqlite\n",
            b"Cannot find module '/private/user-data/node_modules/prisma'",
        );
        let error = parse_bootstrap_message(&output, "bootstrap")
            .expect_err("malformed bootstrap stdout must fail closed");
        let rendered = error.to_string();
        assert!(rendered.contains("status=exit-code-1"));
        assert!(rendered.contains("result=malformed-result"));
        assert!(rendered.contains("stderr=dependency-unavailable"));
        assert!(!rendered.contains("/private/user-data"));
    }

    #[test]
    fn bootstrap_failure_message_is_not_treated_as_success() {
        let output = synthetic_output(
            "exit 1",
            br#"{"kind":"bootstrap","status":"failed","code":"bootstrap-failed","context":"storage-bootstrap"}
"#,
            b"storage initialization failed",
        );
        let message = parse_bootstrap_message(&output, "bootstrap").expect("failure JSON parses");
        assert_eq!(message.status, "failed");
        assert!(!output.status.success());
    }

    #[test]
    fn bootstrap_ready_message_is_parseable_success_contract() {
        let output = synthetic_output(
            "exit 0",
            br#"{"kind":"bootstrap","status":"ready"}
"#,
            &[],
        );
        let message = parse_bootstrap_message(&output, "bootstrap").expect("ready JSON parses");
        assert_eq!(message.kind, "bootstrap");
        assert_eq!(message.status, "ready");
        assert!(output.status.success());
    }

    #[test]
    fn bootstrap_message_rejects_non_ready_database_status() {
        let message = BootstrapMessage {
            kind: "bootstrap".to_string(),
            status: "migration-required".to_string(),
            application_support_root: None,
            live_directory: None,
            database_path: None,
            database_url: None,
            backups_directory: None,
            settings_directory: None,
            logs_directory: None,
            pending_restore_directory: None,
            reason: Some("migration-missing".to_string()),
            created: None,
            recovery_snapshot: None,
            code: None,
            context: None,
        };
        assert_ne!(message.status, "ready");
        assert_eq!(message.reason.as_deref(), Some("migration-missing"));
    }

    #[test]
    fn database_recovery_snapshot_is_allowlisted_and_minimal() {
        let snapshot = DesktopDatabaseRecoverySnapshot {
            schema_version: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
            state: DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_AVAILABLE.to_string(),
            reason_code: DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_INTEGRITY_FAILED.to_string(),
            managed_backup_available: true,
            pending_restore_available: false,
            can_start_empty: false,
        };
        validate_database_recovery_snapshot(&snapshot).expect("valid recovery snapshot");
        let encoded = serde_json::to_value(&snapshot).expect("serialize recovery snapshot");
        assert_eq!(
            encoded,
            serde_json::json!({
                "schemaVersion": 1,
                "state": "restore-available",
                "reasonCode": "database-integrity-failed",
                "managedBackupAvailable": true,
                "pendingRestoreAvailable": false,
                "canStartEmpty": false,
            })
        );
        assert!(!encoded.to_string().contains("DATABASE_URL"));
        assert!(!encoded.to_string().contains("/Users/"));
    }

    #[test]
    fn database_recovery_snapshot_rejects_unknown_state_and_inconsistent_flags() {
        let unknown_state = DesktopDatabaseRecoverySnapshot {
            schema_version: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
            state: "unexpected".to_string(),
            reason_code: DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_READ_FAILED.to_string(),
            managed_backup_available: false,
            pending_restore_available: false,
            can_start_empty: false,
        };
        assert!(validate_database_recovery_snapshot(&unknown_state).is_err());

        let no_source = DesktopDatabaseRecoverySnapshot {
            schema_version: DESKTOP_DATABASE_RECOVERY_SCHEMA_VERSION,
            state: DESKTOP_DATABASE_RECOVERY_STATE_RESTORE_AVAILABLE.to_string(),
            reason_code: DESKTOP_DATABASE_RECOVERY_REASON_DATABASE_READ_FAILED.to_string(),
            managed_backup_available: false,
            pending_restore_available: false,
            can_start_empty: false,
        };
        assert!(validate_database_recovery_snapshot(&no_source).is_err());
    }

    #[test]
    fn health_response_requires_the_expected_nonce_and_ready_contract() {
        let nonce = "a".repeat(64);
        let body = format!(
            r#"{{"kind":"cornell-desktop-health","status":"ready","nonce":"{}"}}"#,
            nonce
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        assert!(health_response_matches(response.as_bytes(), &nonce));
        assert!(!health_response_matches(
            response.as_bytes(),
            &"b".repeat(64)
        ));

        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        );
        assert!(health_response_matches(chunked.as_bytes(), &nonce));
        assert!(!health_response_matches(
            response.replacen("200 OK", "302 Found", 1).as_bytes(),
            &nonce
        ));
    }

    #[test]
    fn ready_failure_mapping_is_stage_specific_and_does_not_expose_input() {
        let invalid_status = ReadyMessage {
            kind: "not-ready".to_string(),
            status: "failed".to_string(),
            url: "http://127.0.0.1:43127/notes?nonce=secret".to_string(),
            host: "127.0.0.1".to_string(),
            port: 43127,
            ready_nonce: "a".repeat(64),
            runtime_pid: 1,
        };
        let error = validate_ready_message(&invalid_status).expect_err("invalid handshake");
        assert_eq!(error.code(), "sidecar-ready-handshake-failed");
        assert!(!error.to_string().contains("secret"));

        let invalid_url = ReadyMessage {
            kind: "ready".to_string(),
            status: "ready".to_string(),
            url: "http://127.0.0.1:43127/notes?nonce=secret".to_string(),
            ..invalid_status
        };
        let error = validate_ready_message(&invalid_url).expect_err("invalid ready URL");
        assert_eq!(error.code(), "sidecar-ready-url-invalid");
        assert!(!error.to_string().contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn startup_failure_cleanup_stops_the_child_process_group() {
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("spawn disposable child");
        let root_pid = child.id();
        let mut handle = SidecarHandle {
            child,
            root_pid,
            process_group_id: Some(root_pid),
            runtime_url: tauri::Url::parse("http://127.0.0.1/").unwrap(),
            stopped: false,
        };

        let error = cleanup_startup_failure(
            &mut handle,
            SidecarStartupError::new("sidecar-health-check-failed"),
        );
        assert_eq!(error.code(), "sidecar-health-check-failed");
        assert!(handle.stopped);
        assert!(!process_group_exists(root_pid));
    }

    #[test]
    fn desktop_api_transport_uses_the_exact_validated_runtime_origin() {
        let runtime_url = tauri::Url::parse("http://127.0.0.1:43127/notes").unwrap();
        let url = desktop_api_url(&runtime_url, "/api/backups").unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:43127/api/backups");
        assert_eq!(
            runtime_url.origin().ascii_serialization(),
            "http://127.0.0.1:43127"
        );
        assert_eq!(runtime_url.as_str(), "http://127.0.0.1:43127/notes");
    }

    #[test]
    fn desktop_api_transport_rejects_non_loopback_or_escaped_targets() {
        let valid_runtime = tauri::Url::parse("http://127.0.0.1:43127/notes").unwrap();
        for path in [
            "/notes",
            "/api/../notes",
            "https://127.0.0.1:43127/api/backups",
        ] {
            assert!(desktop_api_url(&valid_runtime, path).is_err(), "{path}");
        }

        for runtime in [
            "http://localhost:43127/notes",
            "https://127.0.0.1:43127/notes",
            "http://127.0.0.1:43127/notes?origin=wrong",
        ] {
            let runtime_url = tauri::Url::parse(runtime).unwrap();
            assert!(
                desktop_api_url(&runtime_url, "/api/backups").is_err(),
                "{runtime}"
            );
        }

        let different_port = tauri::Url::parse("http://127.0.0.1:43128/notes").unwrap();
        assert_eq!(
            desktop_api_url(&different_port, "/api/backups")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:43128/api/backups"
        );
    }

    #[test]
    fn desktop_api_transport_only_allows_proxy_state_changing_methods() {
        for method in ["POST", "PATCH", "DELETE"] {
            assert!(desktop_api_method(method).is_ok(), "{method}");
        }
        for method in ["GET", "PUT", "OPTIONS", ""] {
            assert!(desktop_api_method(method).is_err(), "{method}");
        }
    }

    #[test]
    fn packaged_node_binary_uses_root_node_and_validates_the_resource() {
        let directory = TestDirectory::new();
        let node_path = directory.path().join(PACKAGED_NODE_BINARY_NAME);

        assert!(packaged_node_binary(directory.path()).is_err());

        fs::create_dir(&node_path).expect("create invalid node directory");
        assert!(packaged_node_binary(directory.path()).is_err());
        fs::remove_dir(&node_path).expect("remove invalid node directory");

        fs::write(&node_path, b"node").expect("create invalid node file");
        #[cfg(unix)]
        {
            fs::set_permissions(&node_path, fs::Permissions::from_mode(0o644))
                .expect("remove executable permission");
            assert!(packaged_node_binary(directory.path()).is_err());
            fs::set_permissions(&node_path, fs::Permissions::from_mode(0o755))
                .expect("add executable permission");
        }

        assert_eq!(packaged_node_binary(directory.path()).unwrap(), node_path);
    }

    fn fixture_exit_status(code: i32) -> ExitStatus {
        Command::new("/bin/sh")
            .args(["-c", &format!("exit {code}")])
            .status()
            .expect("run exit-status fixture")
    }

    #[test]
    fn native_dialog_spawn_failure_is_process_phase_and_unavailable() {
        let failure = DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::DialogProcess,
            "dialog-unavailable",
            DesktopFileDialogExitStatus::Unavailable,
        );
        assert_eq!(failure.phase.wire_name(), "dialog-process");
        assert_eq!(failure.error_code, "dialog-unavailable");
        assert_eq!(failure.exit_status.wire_name(), "unavailable");
    }

    #[test]
    fn native_dialog_non_zero_process_is_bounded_without_changing_typed_error() {
        let status = fixture_exit_status(17);
        let failure = parse_native_file_dialog_output(&status, b"error\n")
            .expect_err("AppleScript error marker must fail");
        assert_eq!(failure.phase, DesktopFileDialogFailurePhase::ResponseParse);
        assert_eq!(failure.error_code, "dialog-error");
        assert_eq!(failure.exit_status, DesktopFileDialogExitStatus::NonZero);
    }

    #[test]
    fn native_dialog_response_failures_are_phase_and_status_typed() {
        let cases = [
            (
                vec![b'x'; MAX_DESKTOP_DIALOG_OUTPUT_BYTES + 1],
                "dialog-response-too-large",
            ),
            (vec![0xff, 0xfe], "dialog-invalid-response"),
            (b"unexpected".to_vec(), "dialog-invalid-response"),
            (b"error\n".to_vec(), "dialog-error"),
        ];
        for (stdout, error_code) in cases {
            let failure = parse_native_file_dialog_output_with_status(
                DesktopFileDialogExitStatus::Success,
                &stdout,
            )
            .expect_err("invalid native response must fail");
            assert_eq!(failure.phase, DesktopFileDialogFailurePhase::ResponseParse);
            assert_eq!(failure.error_code, error_code);
            assert_eq!(failure.exit_status, DesktopFileDialogExitStatus::Success);
        }
    }

    #[test]
    fn native_dialog_cancel_and_selection_preserve_existing_semantics() {
        assert_eq!(
            parse_native_file_dialog_output_with_status(
                DesktopFileDialogExitStatus::Success,
                b"cancel\n",
            )
            .expect("cancel parses"),
            None
        );
        let selection = parse_native_file_dialog_output_with_status(
            DesktopFileDialogExitStatus::Success,
            b"selected\n/temporary/external.sqlite\n",
        )
        .expect("selection parses")
        .expect("selection exists");
        assert_eq!(selection.path, PathBuf::from("/temporary/external.sqlite"));
        assert_eq!(selection.exit_status, DesktopFileDialogExitStatus::Success);
    }

    #[test]
    fn save_destination_uses_a_validated_folder_and_generated_file_name() {
        let directory = TestDirectory::new();
        let managed_root = directory.path().join("managed");
        let external_root = directory.path().join("external");
        fs::create_dir_all(&managed_root).expect("create managed root");
        fs::create_dir_all(&external_root).expect("create external root");

        assert!(validate_external_directory_path(&external_root, &managed_root).is_ok());
        assert_eq!(
            validate_external_directory_path(&external_root.join("missing"), &managed_root),
            Err("path-not-found")
        );
        let file_path = external_root.join("not-a-folder");
        fs::write(&file_path, b"not a folder").expect("write non-directory fixture");
        assert_eq!(
            validate_external_directory_path(&file_path, &managed_root),
            Err("path-not-directory")
        );
        assert_eq!(
            format_external_backup_file_name(0, "0123456789abcdef"),
            "cornell-method-backup-19700101-000000-0123456789abcdef.sqlite"
        );
        let new_path = create_external_backup_destination(&external_root)
            .expect("generate external backup destination");
        let file_name = new_path
            .file_name()
            .and_then(|value| value.to_str())
            .expect("generated file name");
        assert!(file_name.starts_with("cornell-method-backup-"));
        assert!(file_name.ends_with(".sqlite"));
        assert!(file_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')));
        let store = DesktopFileSelectionStore::default();
        let live_directory = managed_root.join("live");
        let database_path = live_directory.join("notebook.sqlite");
        let storage = StorageLayout {
            application_support_root: managed_root.clone(),
            live_directory,
            database_path: database_path.clone(),
            database_url: format!("file:{}", database_path.display()),
            backups_directory: managed_root.join("backups"),
            settings_directory: managed_root.join("settings"),
            logs_directory: managed_root.join("logs"),
            pending_restore_directory: managed_root.join("pending-restore"),
        };

        let new_selection = store
            .insert(DesktopFileDialogKind::SaveDestination, new_path.clone())
            .expect("store new save destination");
        let new_resolution = store
            .resolve(
                &new_selection.selection_id,
                DesktopFileDialogKind::SaveDestination,
                &managed_root,
            )
            .expect("resolve new save destination");
        assert_eq!(new_resolution, new_path);
        assert_eq!(new_selection.file_name, file_name);
        let public_selection = serde_json::to_value(&new_selection).expect("encode selection");
        assert!(!public_selection
            .as_object()
            .expect("selection object")
            .contains_key("replaceExisting"));
        let new_sidecar_request = build_data_backup_sidecar_request(
            DesktopDataBackupOperationRequest {
                schema_version: DESKTOP_DATA_BACKUP_PROTOCOL_VERSION,
                operation: DesktopDataBackupOperation::Export,
                source: serde_json::Value::Null,
                destination: serde_json::json!({
                    "kind": "external-selection",
                    "selectionId": new_selection.selection_id,
                }),
                confirmed: None,
            },
            &storage,
            &store,
            None,
            DesktopRestoreMode::Normal,
        )
        .expect("build new destination sidecar request");
        let new_sidecar_value =
            serde_json::to_value(new_sidecar_request).expect("encode new sidecar request");
        assert!(!new_sidecar_value["destination"]
            .as_object()
            .expect("new destination object")
            .contains_key("replaceExisting"));
    }

    #[test]
    fn native_dialog_later_failures_keep_the_required_phase_categories() {
        let path_failure = DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::PathValidation,
            "path-not-found",
            DesktopFileDialogExitStatus::Success,
        );
        let store_failure = DesktopFileDialogFailure::new(
            DesktopFileDialogFailurePhase::SelectionStore,
            "selection-store-failed",
            DesktopFileDialogExitStatus::Success,
        );
        assert_eq!(path_failure.phase.wire_name(), "path-validation");
        assert_eq!(store_failure.phase.wire_name(), "selection-store");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_launcher_honors_the_environment_override() {
        let directory = TestDirectory::new();
        let packaged = directory.path().join("sidecar").join("launcher.cjs");
        let configured = directory.path().join("external-launcher.cjs");
        fs::create_dir_all(packaged.parent().expect("packaged launcher parent"))
            .expect("create packaged launcher directory");
        fs::write(&packaged, b"packaged launcher").expect("create packaged launcher");
        fs::write(&configured, b"external launcher").expect("create external launcher");
        let _environment = EnvironmentVariableGuard::set("CORNELL_DESKTOP_LAUNCHER", &configured);

        assert_eq!(launcher_path(directory.path()).unwrap(), configured);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_launcher_uses_only_the_packaged_resource() {
        let directory = TestDirectory::new();
        let packaged = directory.path().join("sidecar").join("launcher.cjs");
        let source_tree = directory
            .path()
            .join("src-tauri")
            .join("sidecar")
            .join("launcher.cjs");
        let configured = directory.path().join("external-launcher.cjs");
        fs::create_dir_all(source_tree.parent().expect("source launcher parent"))
            .expect("create source launcher directory");
        fs::write(&source_tree, b"source launcher").expect("create source launcher");
        fs::write(&configured, b"external launcher").expect("create external launcher");
        let _environment = EnvironmentVariableGuard::set("CORNELL_DESKTOP_LAUNCHER", &configured);

        let error = launcher_path(directory.path()).expect_err(
            "release launcher selection must not fall back to source-tree or external paths",
        );
        assert_eq!(
            error,
            format!("sidecar launcher is missing: {}", packaged.display())
        );

        fs::create_dir_all(packaged.parent().expect("packaged launcher parent"))
            .expect("create packaged launcher directory");
        fs::write(&packaged, b"packaged launcher").expect("create packaged launcher");
        assert_eq!(launcher_path(directory.path()).unwrap(), packaged);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_node_binary_honors_the_environment_override() {
        let directory = TestDirectory::new();
        let configured = directory.path().join("external-node");
        let _environment =
            EnvironmentVariableGuard::set("CORNELL_DESKTOP_NODE_BINARY", &configured);

        assert_eq!(node_binary(directory.path()).unwrap(), configured);
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_node_binary_ignores_the_environment_override() {
        let directory = TestDirectory::new();
        let node_path = directory.path().join(PACKAGED_NODE_BINARY_NAME);
        let configured = directory.path().join("external-node");
        fs::write(&node_path, b"node").expect("create packaged node file");
        #[cfg(unix)]
        fs::set_permissions(&node_path, fs::Permissions::from_mode(0o755))
            .expect("make packaged node executable");

        let _environment =
            EnvironmentVariableGuard::set("CORNELL_DESKTOP_NODE_BINARY", &configured);

        assert_eq!(node_binary(directory.path()).unwrap(), node_path);
    }
}

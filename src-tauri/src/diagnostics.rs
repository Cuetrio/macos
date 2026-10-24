use crate::runtime::{
    run_native_file_dialog, validate_external_file_path, DesktopFileDialogKind,
    DesktopFileSelectionStore, StorageLayout,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

pub(crate) const DIAGNOSTIC_PROTOCOL_VERSION: u8 = 1;
const DIAGNOSTIC_DIALOG_KIND: &str = "desktop-diagnostic-dialog";
const DIAGNOSTIC_EXPORT_KIND: &str = "desktop-diagnostic-export";
const DIAGNOSTIC_DIALOG_NAME: &str = "diagnostic-export";
const DIAGNOSTIC_OPERATION: &str = "export";
const DIAGNOSTIC_FILE_NAME: &str = "diagnostic.json";
const LOG_FILE_PREFIX: &str = "event-";
const LOG_FILE_SUFFIX: &str = ".jsonl";
const LOG_FILE_MAX_BYTES: usize = 16 * 1024;
const LOG_TOTAL_MAX_BYTES: u64 = 20 * 1024 * 1024;
const LOG_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const MAX_DIAGNOSTIC_JSON_BYTES: usize = 20 * 1024 * 1024;
const MAX_SELECTION_ID_LENGTH: usize = 128;

static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalLogRecord {
    timestamp: String,
    component: String,
    error_code: String,
    message: String,
    stack: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dialog_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_status_category: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticSelectionMetadata {
    pub(crate) kind: &'static str,
    #[serde(rename = "selectionId")]
    pub(crate) selection_id: String,
    #[serde(rename = "fileName")]
    pub(crate) file_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticDialogResponse {
    pub(crate) kind: &'static str,
    pub(crate) schema_version: u8,
    pub(crate) dialog: &'static str,
    pub(crate) operation: &'static str,
    pub(crate) status: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) ok: bool,
    pub(crate) selection: Option<DiagnosticSelectionMetadata>,
    pub(crate) error_code: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportResult {
    file_name: String,
    size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticExportResponse {
    pub(crate) kind: &'static str,
    pub(crate) schema_version: u8,
    pub(crate) dialog: &'static str,
    pub(crate) operation: &'static str,
    pub(crate) status: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) ok: bool,
    pub(crate) selection: Option<DiagnosticSelectionMetadata>,
    pub(crate) error_code: Option<&'static str>,
    result: Option<DiagnosticExportResult>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiagnosticExportRequest {
    schema_version: u8,
    operation: String,
    selection_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiagnosticDocument {
    schema_version: u8,
    app_version: String,
    macos_version: String,
    cpu_architecture: String,
    db_schema_version: u8,
    error_log: Vec<LocalLogRecord>,
}

#[derive(Clone, Debug)]
pub(crate) struct DiagnosticsState {
    writer: Arc<LocalLogWriter>,
    status: Arc<Mutex<DiagnosticStatus>>,
}

#[derive(Clone, Debug, Default)]
struct DiagnosticStatus {
    last_error_code: Option<&'static str>,
}

impl DiagnosticsState {
    pub(crate) fn new(logs_directory: PathBuf) -> Self {
        Self {
            writer: Arc::new(LocalLogWriter::new(logs_directory)),
            status: Arc::new(Mutex::new(DiagnosticStatus::default())),
        }
    }

    pub(crate) fn record_failure(&self, component: &str, error_code: &str) {
        if let Err(code) = self.writer.append_failure_with_metadata(
            component,
            error_code,
            SystemTime::now(),
            None,
            None,
            None,
        ) {
            if let Ok(mut status) = self.status.lock() {
                status.last_error_code = Some(code);
            }
        }
    }

    pub(crate) fn record_file_dialog_failure(
        &self,
        dialog_kind: &str,
        failure_phase: &str,
        error_code: &str,
        exit_status: &str,
    ) {
        if let Err(code) = self.writer.append_failure_with_metadata(
            "file-dialog",
            error_code,
            SystemTime::now(),
            Some(dialog_kind),
            Some(failure_phase),
            Some(exit_status),
        ) {
            if let Ok(mut status) = self.status.lock() {
                status.last_error_code = Some(code);
            }
        }
    }

    fn build_document(&self, now: SystemTime) -> Result<DiagnosticDocument, &'static str> {
        self.writer.build_document(now)
    }
}

pub(crate) fn record_failure_for_app(app: &AppHandle, component: &str, error_code: &str) {
    if let Some(state) = app.try_state::<DiagnosticsState>() {
        state.inner().record_failure(component, error_code);
    }
}

pub(crate) fn record_file_dialog_failure_for_app(
    app: &AppHandle,
    dialog_kind: &str,
    failure_phase: &str,
    error_code: &str,
    exit_status: &str,
) {
    if let Some(state) = app.try_state::<DiagnosticsState>() {
        state.inner().record_file_dialog_failure(
            dialog_kind,
            failure_phase,
            error_code,
            exit_status,
        );
    }
}

fn dialog_response(
    ok: bool,
    status: &'static str,
    selection: Option<DiagnosticSelectionMetadata>,
    error_code: Option<&'static str>,
) -> DiagnosticDialogResponse {
    DiagnosticDialogResponse {
        kind: DIAGNOSTIC_DIALOG_KIND,
        schema_version: DIAGNOSTIC_PROTOCOL_VERSION,
        dialog: DIAGNOSTIC_DIALOG_NAME,
        operation: "select-destination",
        status,
        phase: "dialog",
        ok,
        selection,
        error_code,
    }
}

pub(crate) fn diagnostic_dialog_command_error(
    error_code: &'static str,
) -> DiagnosticDialogResponse {
    dialog_response(false, "error", None, Some(error_code))
}

fn export_response(
    ok: bool,
    status: &'static str,
    phase: &'static str,
    selection: Option<DiagnosticSelectionMetadata>,
    error_code: Option<&'static str>,
    result: Option<DiagnosticExportResult>,
) -> DiagnosticExportResponse {
    DiagnosticExportResponse {
        kind: DIAGNOSTIC_EXPORT_KIND,
        schema_version: DIAGNOSTIC_PROTOCOL_VERSION,
        dialog: DIAGNOSTIC_DIALOG_NAME,
        operation: DIAGNOSTIC_OPERATION,
        status,
        phase,
        ok,
        selection,
        error_code,
        result,
    }
}

pub(crate) fn diagnostic_export_command_error(
    phase: &'static str,
    error_code: &'static str,
) -> DiagnosticExportResponse {
    export_response(false, "error", phase, None, Some(error_code), None)
}

pub(crate) fn choose_diagnostic_export_destination(app: &AppHandle) -> DiagnosticDialogResponse {
    let Some(storage) = app.try_state::<StorageLayout>() else {
        crate::runtime::DesktopFileDialogFailure::new(
            crate::runtime::DesktopFileDialogFailurePhase::Command,
            "storage-unavailable",
            crate::runtime::DesktopFileDialogExitStatus::Unavailable,
        )
        .record(app, DesktopFileDialogKind::DiagnosticExport);
        return diagnostic_dialog_command_error("storage-unavailable");
    };
    let Some(selection_store) = app.try_state::<DesktopFileSelectionStore>() else {
        crate::runtime::DesktopFileDialogFailure::new(
            crate::runtime::DesktopFileDialogFailurePhase::Command,
            "selection-store-failed",
            crate::runtime::DesktopFileDialogExitStatus::Unavailable,
        )
        .record(app, DesktopFileDialogKind::DiagnosticExport);
        return diagnostic_dialog_command_error("selection-store-failed");
    };
    let selection = match run_native_file_dialog(DesktopFileDialogKind::DiagnosticExport) {
        Ok(Some(selection)) => selection,
        Ok(None) => return dialog_response(false, "cancelled", None, None),
        Err(failure) => {
            failure.record(app, DesktopFileDialogKind::DiagnosticExport);
            return dialog_response(
                false,
                "error",
                None,
                Some(diagnostic_wire_error(failure.error_code)),
            );
        }
    };
    if let Err(error_code) =
        validate_external_file_path(&selection.path, storage.application_support_root(), false)
    {
        crate::runtime::DesktopFileDialogFailure::new(
            crate::runtime::DesktopFileDialogFailurePhase::PathValidation,
            error_code,
            selection.exit_status,
        )
        .record(app, DesktopFileDialogKind::DiagnosticExport);
        return dialog_response(
            false,
            "error",
            None,
            Some(diagnostic_wire_error(error_code)),
        );
    }
    let exit_status = selection.exit_status;
    match selection_store
        .inner()
        .insert(DesktopFileDialogKind::DiagnosticExport, selection.path)
    {
        Ok(selection) => dialog_response(
            true,
            "selected",
            Some(DiagnosticSelectionMetadata {
                kind: selection.kind,
                selection_id: selection.selection_id,
                file_name: selection.file_name,
            }),
            None,
        ),
        Err(error_code) => {
            crate::runtime::DesktopFileDialogFailure::new(
                crate::runtime::DesktopFileDialogFailurePhase::SelectionStore,
                error_code,
                exit_status,
            )
            .record(app, DesktopFileDialogKind::DiagnosticExport);
            dialog_response(
                false,
                "error",
                None,
                Some(diagnostic_wire_error(error_code)),
            )
        }
    }
}

pub(crate) fn export_diagnostics_command(
    app: &AppHandle,
    request: serde_json::Value,
) -> DiagnosticExportResponse {
    let parsed = match serde_json::from_value::<DiagnosticExportRequest>(request) {
        Ok(request) => request,
        Err(_) => return diagnostic_export_command_error("validation", "invalid-request"),
    };
    if parsed.schema_version != DIAGNOSTIC_PROTOCOL_VERSION {
        return diagnostic_export_command_error("validation", "unsupported-protocol-version");
    }
    if parsed.operation != DIAGNOSTIC_OPERATION || !safe_selection_id(&parsed.selection_id) {
        return diagnostic_export_command_error("validation", "invalid-request");
    }
    let Some(storage) = app.try_state::<StorageLayout>() else {
        return diagnostic_export_command_error("request", "storage-unavailable");
    };
    let Some(diagnostics) = app.try_state::<DiagnosticsState>() else {
        return diagnostic_export_command_error("request", "diagnostics-unavailable");
    };
    let Some(selection_store) = app.try_state::<DesktopFileSelectionStore>() else {
        return diagnostic_export_command_error("request", "selection-store-failed");
    };
    let destination = match selection_store.inner().resolve(
        &parsed.selection_id,
        DesktopFileDialogKind::DiagnosticExport,
        storage.application_support_root(),
    ) {
        Ok(path) => path,
        Err(error_code) => {
            record_failure_for_app(app, "diagnostic-export", error_code);
            return diagnostic_export_command_error(
                "validation",
                diagnostic_wire_error(error_code),
            );
        }
    };
    let file_name = match safe_file_name(&destination) {
        Some(file_name) => file_name,
        None => return diagnostic_export_command_error("validation", "invalid-path"),
    };
    let selection = || DiagnosticSelectionMetadata {
        kind: "diagnostic-export",
        selection_id: parsed.selection_id.clone(),
        file_name: file_name.clone(),
    };
    if destination_metadata_is_present(&destination) {
        return export_response(
            false,
            "error",
            "validation",
            Some(selection()),
            Some("destination-exists"),
            None,
        );
    }
    let document = match diagnostics.inner().build_document(SystemTime::now()) {
        Ok(document) => document,
        Err(error_code) => {
            record_failure_for_app(app, "diagnostic-export", error_code);
            return export_response(
                false,
                "error",
                "archive",
                Some(selection()),
                Some(diagnostic_wire_error(error_code)),
                None,
            );
        }
    };
    let temp_path = temporary_archive_path(&destination);
    if destination_metadata_is_present(&temp_path) {
        return export_response(
            false,
            "error",
            "archive",
            Some(selection()),
            Some("temporary-artifact-exists"),
            None,
        );
    }
    let size = match write_zip_archive(&temp_path, &document) {
        Ok(size) => size,
        Err(error_code) => {
            let cleanup_code = cleanup_temporary_archive(&temp_path).err();
            let error_code = cleanup_code.unwrap_or(error_code);
            record_failure_for_app(app, "diagnostic-export", error_code);
            return export_response(
                false,
                "error",
                "archive",
                Some(selection()),
                Some(diagnostic_wire_error(error_code)),
                None,
            );
        }
    };
    if let Err(error_code) = publish_archive(&temp_path, &destination) {
        let cleanup_code = cleanup_temporary_archive(&temp_path).err();
        let error_code = cleanup_code.unwrap_or(error_code);
        record_failure_for_app(app, "diagnostic-export", error_code);
        return export_response(
            false,
            "error",
            "publish",
            Some(selection()),
            Some(diagnostic_wire_error(error_code)),
            None,
        );
    }
    export_response(
        true,
        "success",
        "publish",
        Some(selection()),
        None,
        Some(DiagnosticExportResult { file_name, size }),
    )
}

fn safe_selection_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SELECTION_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn diagnostic_wire_error(value: &str) -> &'static str {
    match value {
        "storage-unavailable" => "storage-unavailable",
        "selection-store-failed" => "selection-store-failed",
        "dialog-unavailable" => "dialog-unavailable",
        "dialog-response-too-large" => "dialog-response-too-large",
        "dialog-invalid-response" => "dialog-invalid-response",
        "dialog-error" => "dialog-error",
        "unsupported-platform" => "unsupported-platform",
        "relative-path" => "relative-path",
        "invalid-path" => "invalid-path",
        "managed-path" => "managed-path",
        "unsafe-path" => "unsafe-path",
        "symlink-path" => "symlink-path",
        "path-unavailable" => "path-unavailable",
        "path-not-file" => "path-not-file",
        "path-not-found" => "path-not-found",
        "invalid-selection" => "invalid-selection",
        "selection-not-found" => "selection-not-found",
        "selection-kind-mismatch" => "selection-kind-mismatch",
        "invalid-request" => "invalid-request",
        "unsupported-protocol-version" => "unsupported-protocol-version",
        "destination-exists" => "destination-exists",
        "diagnostics-unavailable" => "diagnostics-unavailable",
        "log-lock-failed" => "log-lock-failed",
        "temporary-artifact-exists" => "temporary-artifact-exists",
        "serialization-failed" => "serialization-failed",
        "archive-too-large" => "archive-too-large",
        "archive-write-failed" => "archive-write-failed",
        "publish-failed" => "publish-failed",
        "cleanup-failed" => "cleanup-failed",
        "unsafe-log-entry" => "unsafe-log-entry",
        "unsafe-log-directory" => "unsafe-log-directory",
        "log-directory-unavailable" => "log-directory-unavailable",
        "log-read-failed" => "log-read-failed",
        "log-invalid" => "log-invalid",
        "log-file-too-large" => "log-file-too-large",
        "log-prune-failed" => "log-prune-failed",
        _ => "internal-error",
    }
}

fn safe_file_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return None;
    }
    Some(name.to_string())
}

#[derive(Debug)]
struct LocalLogWriter {
    logs_directory: PathBuf,
    operation_lock: Mutex<()>,
}

impl LocalLogWriter {
    fn new(logs_directory: PathBuf) -> Self {
        Self {
            logs_directory,
            operation_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    fn append_failure(
        &self,
        component: &str,
        error_code: &str,
        now: SystemTime,
    ) -> Result<(), &'static str> {
        self.append_failure_with_metadata(component, error_code, now, None, None, None)
    }

    fn append_failure_with_metadata(
        &self,
        component: &str,
        error_code: &str,
        now: SystemTime,
        dialog_kind: Option<&str>,
        failure_phase: Option<&str>,
        exit_status_category: Option<&str>,
    ) -> Result<(), &'static str> {
        let component = sanitize_component(component);
        let error_code = sanitize_error_code(error_code);
        let dialog_kind = dialog_kind.map(sanitize_dialog_kind);
        let failure_phase = failure_phase.map(sanitize_failure_phase);
        let exit_status_category = exit_status_category.map(sanitize_exit_status);
        if (dialog_kind.is_some() || failure_phase.is_some() || exit_status_category.is_some())
            && !(dialog_kind.is_some() && failure_phase.is_some() && exit_status_category.is_some())
        {
            return Err("invalid-file-dialog-metadata");
        }
        if dialog_kind == Some("unknown")
            || failure_phase == Some("unknown")
            || exit_status_category == Some("unknown")
        {
            return Err("invalid-file-dialog-metadata");
        }
        let record = LocalLogRecord {
            timestamp: format_timestamp(now),
            component: component.to_string(),
            error_code: error_code.to_string(),
            message: message_for_error(error_code).to_string(),
            stack: "redacted".to_string(),
            dialog_kind: dialog_kind.map(str::to_string),
            failure_phase: failure_phase.map(str::to_string),
            exit_status_category: exit_status_category.map(str::to_string),
        };
        let mut bytes = serde_json::to_vec(&record).map_err(|_| "serialization-failed")?;
        bytes.push(b'\n');
        if bytes.len() > LOG_FILE_MAX_BYTES {
            return Err("record-too-large");
        }
        let _operation_guard = self.operation_lock.lock().map_err(|_| "log-lock-failed")?;
        prepare_log_directory(&self.logs_directory)?;
        prune_logs(&self.logs_directory, now)?;
        let id = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
        let millis = now
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock-invalid")?
            .as_millis();
        let file_name = format!(
            "{LOG_FILE_PREFIX}{millis}-{}-{id}{LOG_FILE_SUFFIX}",
            std::process::id()
        );
        let destination = self.logs_directory.join(file_name);
        let temporary = self
            .logs_directory
            .join(format!(".event-{millis}-{}-{id}.tmp", std::process::id()));
        let result = write_atomic_log(&temporary, &destination, &bytes);
        if result.is_err() {
            cleanup_log_temp(&temporary);
        }
        result?;
        prune_logs(&self.logs_directory, now)
    }

    fn build_document(&self, now: SystemTime) -> Result<DiagnosticDocument, &'static str> {
        let _operation_guard = self.operation_lock.lock().map_err(|_| "log-lock-failed")?;
        build_diagnostic_document(&self.logs_directory, now)
    }
}

fn prepare_log_directory(directory: &Path) -> Result<(), &'static str> {
    if !directory.exists() {
        fs::create_dir_all(directory).map_err(|_| "log-directory-unavailable")?;
    }
    let metadata = fs::symlink_metadata(directory).map_err(|_| "log-directory-unavailable")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("unsafe-log-directory");
    }
    let entries = fs::read_dir(directory).map_err(|_| "log-directory-unavailable")?;
    for entry in entries {
        let entry = entry.map_err(|_| "log-directory-unavailable")?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| "unsafe-log-entry")?;
        let name = entry.file_name().to_string_lossy().to_string();
        if is_temp_log_name(&name) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("unsafe-log-entry");
            }
            fs::remove_file(path).map_err(|_| "log-cleanup-failed")?;
        } else if !is_log_name(&name) || metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("unsafe-log-entry");
        }
    }
    Ok(())
}

fn write_atomic_log(
    temporary: &Path,
    destination: &Path,
    bytes: &[u8],
) -> Result<(), &'static str> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)
        .map_err(|_| "log-write-failed")?;
    if file.write_all(bytes).is_err() || file.sync_all().is_err() {
        return Err("log-write-failed");
    }
    fs::rename(temporary, destination).map_err(|_| "log-publish-failed")?;
    Ok(())
}

fn cleanup_log_temp(path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.is_file() && !metadata.file_type().is_symlink() {
            let _ = fs::remove_file(path);
        }
    }
}

fn is_temp_log_name(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(".event-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    rest.split('-').count() == 3
        && rest
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_log_name(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(LOG_FILE_PREFIX)
        .and_then(|value| value.strip_suffix(LOG_FILE_SUFFIX))
    else {
        return false;
    };
    let parts: Vec<&str> = rest.split('-').collect();
    parts.len() == 3
        && parts[0].len() >= 1
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn log_file_millis(name: &str) -> Option<u128> {
    name.strip_prefix(LOG_FILE_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

fn prune_logs(directory: &Path, now: SystemTime) -> Result<(), &'static str> {
    let now_millis = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock-invalid")?
        .as_millis();
    let cutoff = now_millis.saturating_sub(LOG_MAX_AGE.as_millis());
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|_| "log-directory-unavailable")? {
        let entry = entry.map_err(|_| "log-directory-unavailable")?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_log_name(&name) {
            return Err("unsafe-log-entry");
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "unsafe-log-entry")?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("unsafe-log-entry");
        }
        if metadata.len() > LOG_FILE_MAX_BYTES as u64 {
            return Err("log-file-too-large");
        }
        let records = read_log_file(&entry.path())?;
        let millis = log_file_millis(&name).ok_or("unsafe-log-entry")?;
        files.push((millis, entry.path(), metadata.len(), records));
    }
    for (millis, path, _, _) in &files {
        if *millis < cutoff {
            fs::remove_file(path).map_err(|_| "log-prune-failed")?;
        }
    }
    files.retain(|(millis, _, _, _)| *millis >= cutoff);
    let mut total: u64 = files.iter().map(|(_, _, size, _)| *size).sum();
    files.sort_by_key(|(millis, _, _, _)| *millis);
    for (_, path, size, _) in files {
        if total <= LOG_TOTAL_MAX_BYTES {
            break;
        }
        fs::remove_file(path).map_err(|_| "log-prune-failed")?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

fn read_log_file(path: &Path) -> Result<Vec<LocalLogRecord>, &'static str> {
    let bytes = fs::read(path).map_err(|_| "log-read-failed")?;
    if bytes.is_empty() || bytes.len() > LOG_FILE_MAX_BYTES || !bytes.ends_with(b"\n") {
        return Err("log-invalid");
    }
    let mut records = Vec::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let record = serde_json::from_slice::<LocalLogRecord>(line).map_err(|_| "log-invalid")?;
        if !valid_record(&record) {
            return Err("log-invalid");
        }
        records.push(record);
    }
    if records.is_empty() {
        return Err("log-invalid");
    }
    Ok(records)
}

fn valid_record(record: &LocalLogRecord) -> bool {
    record.timestamp.len() <= 40
        && record.timestamp.ends_with('Z')
        && sanitize_component(&record.component) == record.component
        && sanitize_error_code(&record.error_code) == record.error_code
        && record.message == message_for_error(&record.error_code)
        && record.stack == "redacted"
        && valid_file_dialog_metadata(record)
}

fn valid_file_dialog_metadata(record: &LocalLogRecord) -> bool {
    match (
        record.dialog_kind.as_deref(),
        record.failure_phase.as_deref(),
        record.exit_status_category.as_deref(),
    ) {
        (None, None, None) => true,
        (Some(dialog), Some(phase), Some(exit_status)) => {
            matches!(
                dialog,
                "save-destination" | "open-external-source" | "diagnostic-export"
            ) && matches!(
                phase,
                "command"
                    | "dialog-process"
                    | "response-parse"
                    | "path-validation"
                    | "selection-store"
            ) && matches!(exit_status, "success" | "non-zero" | "unavailable")
                && record.component == "file-dialog"
        }
        _ => false,
    }
}

fn build_diagnostic_document(
    logs_directory: &Path,
    now: SystemTime,
) -> Result<DiagnosticDocument, &'static str> {
    prepare_log_directory(logs_directory)?;
    prune_logs(logs_directory, now)?;
    let mut records = Vec::new();
    for entry in fs::read_dir(logs_directory).map_err(|_| "log-directory-unavailable")? {
        let entry = entry.map_err(|_| "log-read-failed")?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_log_name(&name) {
            return Err("unsafe-log-entry");
        }
        records.extend(read_log_file(&entry.path())?);
    }
    let document = DiagnosticDocument {
        schema_version: DIAGNOSTIC_PROTOCOL_VERSION,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        macos_version: macos_version(),
        cpu_architecture: env::consts::ARCH.to_string(),
        db_schema_version: 1,
        error_log: records,
    };
    let bytes = serde_json::to_vec(&document).map_err(|_| "serialization-failed")?;
    if bytes.len() > MAX_DIAGNOSTIC_JSON_BYTES {
        return Err("archive-too-large");
    }
    Ok(document)
}

fn macos_version() -> String {
    if cfg!(target_os = "macos") {
        if let Ok(output) = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
        {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value.len() <= 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
            {
                return value;
            }
        }
    }
    "unknown".to_string()
}

fn temporary_archive_path(destination: &Path) -> PathBuf {
    let id = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("diagnostic.zip");
    destination.with_file_name(format!(".{name}.tmp-{}-{id}", std::process::id()))
}

fn cleanup_temporary_archive(path: &Path) -> Result<(), &'static str> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("cleanup-failed"),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| "cleanup-failed")
        }
        Ok(_) => Err("cleanup-failed"),
    }
}

fn publish_archive(temporary: &Path, destination: &Path) -> Result<(), &'static str> {
    if destination_metadata_is_present(destination) {
        return Err("destination-exists");
    }
    match fs::hard_link(temporary, destination) {
        Ok(()) => fs::remove_file(temporary).map_err(|_| "publish-failed"),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err("destination-exists"),
        Err(_) => Err("publish-failed"),
    }
}

fn destination_metadata_is_present(destination: &Path) -> bool {
    match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

fn write_zip_archive(path: &Path, document: &DiagnosticDocument) -> Result<u64, &'static str> {
    let content = serde_json::to_vec(document).map_err(|_| "serialization-failed")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "archive-write-failed")?;
    let crc = crc32(&content);
    let name = DIAGNOSTIC_FILE_NAME.as_bytes();
    write_u32(&mut file, 0x04034b50)?;
    write_u16(&mut file, 20)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u32(&mut file, crc)?;
    write_u32(&mut file, content.len() as u32)?;
    write_u32(&mut file, content.len() as u32)?;
    write_u16(&mut file, name.len() as u16)?;
    write_u16(&mut file, 0)?;
    file.write_all(name).map_err(|_| "archive-write-failed")?;
    file.write_all(&content)
        .map_err(|_| "archive-write-failed")?;
    let central_offset = file.stream_position().map_err(|_| "archive-write-failed")?;
    write_u32(&mut file, 0x02014b50)?;
    write_u16(&mut file, 20)?;
    write_u16(&mut file, 20)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u32(&mut file, crc)?;
    write_u32(&mut file, content.len() as u32)?;
    write_u32(&mut file, content.len() as u32)?;
    write_u16(&mut file, name.len() as u16)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u32(&mut file, 0)?;
    write_u32(&mut file, 0)?;
    file.write_all(name).map_err(|_| "archive-write-failed")?;
    let central_size = file
        .stream_position()
        .map_err(|_| "archive-write-failed")?
        .saturating_sub(central_offset);
    write_u32(&mut file, 0x06054b50)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 0)?;
    write_u16(&mut file, 1)?;
    write_u16(&mut file, 1)?;
    write_u32(&mut file, central_size as u32)?;
    write_u32(&mut file, central_offset as u32)?;
    write_u16(&mut file, 0)?;
    file.sync_all().map_err(|_| "archive-write-failed")?;
    Ok(file.metadata().map_err(|_| "archive-write-failed")?.len())
}

fn write_u16(file: &mut File, value: u16) -> Result<(), &'static str> {
    file.write_all(&value.to_le_bytes())
        .map_err(|_| "archive-write-failed")
}

fn write_u32(file: &mut File, value: u32) -> Result<(), &'static str> {
    file.write_all(&value.to_le_bytes())
        .map_err(|_| "archive-write-failed")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn sanitize_component(value: &str) -> &'static str {
    match value {
        "startup" => "startup",
        "sidecar" => "sidecar",
        "recovery" => "recovery",
        "restore" => "restore",
        "pending-restore" => "pending-restore",
        "diagnostic-export" => "diagnostic-export",
        "lifecycle" => "lifecycle",
        "storage" => "storage",
        "backup" => "backup",
        "file-dialog" => "file-dialog",
        _ => "internal",
    }
}

fn sanitize_dialog_kind(value: &str) -> &'static str {
    match value {
        "save-destination" => "save-destination",
        "open-external-source" => "open-external-source",
        "diagnostic-export" => "diagnostic-export",
        _ => "unknown",
    }
}

fn sanitize_failure_phase(value: &str) -> &'static str {
    match value {
        "command" => "command",
        "dialog-process" => "dialog-process",
        "response-parse" => "response-parse",
        "path-validation" => "path-validation",
        "selection-store" => "selection-store",
        _ => "unknown",
    }
}

fn sanitize_exit_status(value: &str) -> &'static str {
    match value {
        "success" => "success",
        "non-zero" => "non-zero",
        "unavailable" => "unavailable",
        _ => "unknown",
    }
}

fn sanitize_error_code(value: &str) -> &'static str {
    match value {
        "database-missing" => "database-missing",
        "database-missing-after-initialization" => "database-missing-after-initialization",
        "database-not-a-file" => "database-not-a-file",
        "database-read-failed" => "database-read-failed",
        "database-integrity-failed" => "database-integrity-failed",
        "database-foreign-key-failed" => "database-foreign-key-failed",
        "database-schema-invalid" => "database-schema-invalid",
        "database-migration-required" => "database-migration-required",
        "database-initialization-failed" => "database-initialization-failed",
        "database-initialization-marker-invalid" => "database-initialization-marker-invalid",
        "storage-unavailable" => "storage-unavailable",
        "command-worker-failed" => "command-worker-failed",
        "sidecar-start-failed" => "sidecar-start-failed",
        "sidecar-spawn-failed" => "sidecar-spawn-failed",
        "sidecar-ready-handshake-failed" => "sidecar-ready-handshake-failed",
        "sidecar-ready-url-invalid" => "sidecar-ready-url-invalid",
        "sidecar-health-check-failed" => "sidecar-health-check-failed",
        "sidecar-startup-cleanup-failed" => "sidecar-startup-cleanup-failed",
        "sidecar-restart-failed" => "sidecar-restart-failed",
        "sidecar-exit-cleanup-failed" => "sidecar-exit-cleanup-failed",
        "bootstrap-failed" => "bootstrap-failed",
        "startup-recovery-failed" => "startup-recovery-failed",
        "restore-operation-failed" => "restore-operation-failed",
        "pending-restore-failed" => "pending-restore-failed",
        "command-unavailable" => "command-unavailable",
        "dialog-unavailable" => "dialog-unavailable",
        "dialog-error" => "dialog-error",
        "dialog-invalid-response" => "dialog-invalid-response",
        "dialog-response-too-large" => "dialog-response-too-large",
        "unsupported-platform" => "unsupported-platform",
        "selection-store-failed" => "selection-store-failed",
        "selection-not-found" => "selection-not-found",
        "selection-kind-mismatch" => "selection-kind-mismatch",
        "invalid-selection" => "invalid-selection",
        "invalid-path" => "invalid-path",
        "relative-path" => "relative-path",
        "unsafe-path" => "unsafe-path",
        "managed-path" => "managed-path",
        "symlink-path" => "symlink-path",
        "path-unavailable" => "path-unavailable",
        "path-not-file" => "path-not-file",
        "path-not-found" => "path-not-found",
        "invalid-request" => "invalid-request",
        "unsupported-protocol-version" => "unsupported-protocol-version",
        "destination-exists" => "destination-exists",
        "publish-failed" => "publish-failed",
        "archive-write-failed" => "archive-write-failed",
        "archive-too-large" => "archive-too-large",
        "unsafe-log-entry" => "unsafe-log-entry",
        "unsafe-log-directory" => "unsafe-log-directory",
        "log-directory-unavailable" => "log-directory-unavailable",
        "log-lock-failed" => "log-lock-failed",
        "log-prune-failed" => "log-prune-failed",
        "log-write-failed" => "log-write-failed",
        "log-publish-failed" => "log-publish-failed",
        "log-invalid" => "log-invalid",
        "log-file-too-large" => "log-file-too-large",
        "log-read-failed" => "log-read-failed",
        "temporary-artifact-exists" => "temporary-artifact-exists",
        "cleanup-failed" => "cleanup-failed",
        _ => "internal-error",
    }
}

fn message_for_error(code: &str) -> &'static str {
    match code {
        "destination-exists" => "The selected destination already exists.",
        "unsupported-platform" => "This operation is unavailable on this platform.",
        "storage-unavailable" => "Application storage is unavailable.",
        "unsafe-log-entry" | "unsafe-log-directory" => {
            "Local diagnostic storage failed validation."
        }
        "internal-error" => "An internal desktop error occurred.",
        _ => "A desktop operation failed.",
    }
}

fn format_timestamp(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_seconds / 3_600,
        (day_seconds % 3_600) / 60,
        day_seconds % 60,
        duration.subsec_millis()
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096).div_euclid(365);
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2).div_euclid(153);
    let day = doy - (153 * mp + 2).div_euclid(5) + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn temp_directory(name: &str) -> PathBuf {
        let suffix = NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "cornell-diagnostics-{name}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    #[test]
    fn local_log_is_bounded_and_redacted() {
        let directory = temp_directory("redaction");
        let writer = LocalLogWriter::new(directory.clone());
        writer
            .append_failure(
                "restore",
                "/Users/private/note.sqlite",
                UNIX_EPOCH + Duration::from_secs(1_000_000),
            )
            .expect("write record");
        let entries: Vec<_> = fs::read_dir(&directory).expect("read logs").collect();
        assert_eq!(entries.len(), 1);
        let contents =
            fs::read_to_string(entries[0].as_ref().expect("entry").path()).expect("read record");
        assert!(!contents.contains("/Users/private"));
        assert!(!contents.contains("note.sqlite"));
        assert!(contents.len() <= LOG_FILE_MAX_BYTES);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn retention_removes_old_and_over_capacity_files() {
        let directory = temp_directory("retention");
        let writer = LocalLogWriter::new(directory.clone());
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        writer
            .append_failure(
                "startup",
                "internal-error",
                now - LOG_MAX_AGE - Duration::from_secs(1),
            )
            .expect("write old record");
        writer
            .append_failure("startup", "internal-error", now)
            .expect("write current record");
        assert_eq!(fs::read_dir(&directory).expect("read logs").count(), 1);
        for index in 0..3_000_u64 {
            writer
                .append_failure(
                    "startup",
                    "internal-error",
                    now - Duration::from_secs(index * 60),
                )
                .expect("write record");
        }
        prune_logs(&directory, now).expect("prune logs");
        let total: u64 = fs::read_dir(&directory)
            .expect("read logs")
            .map(|entry| entry.expect("entry").metadata().expect("metadata").len())
            .sum();
        assert!(total <= LOG_TOTAL_MAX_BYTES);
        let old = temp_directory("old");
        assert!(old.exists());
        let _ = fs::remove_dir_all(directory);
        let _ = fs::remove_dir_all(old);
    }

    #[test]
    fn unsafe_log_entry_fails_closed() {
        let directory = temp_directory("unsafe");
        fs::write(directory.join("unexpected.txt"), "private").expect("write unsafe entry");
        let writer = LocalLogWriter::new(directory.clone());
        assert_eq!(
            writer.append_failure("startup", "internal-error", SystemTime::now()),
            Err("unsafe-log-entry")
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn concurrent_appends_do_not_delete_active_temporary_logs() {
        let directory = temp_directory("concurrent");
        let writer = Arc::new(LocalLogWriter::new(directory.clone()));
        let start = Arc::new(Barrier::new(9));
        let mut handles = Vec::new();
        for index in 0..8 {
            let writer = Arc::clone(&writer);
            let start = Arc::clone(&start);
            handles.push(thread::spawn(move || {
                start.wait();
                for offset in 0..32 {
                    writer
                        .append_failure(
                            "startup",
                            "internal-error",
                            UNIX_EPOCH + Duration::from_secs(2_500_000_000 + index * 100 + offset),
                        )
                        .expect("write concurrent record");
                }
            }));
        }
        start.wait();
        for handle in handles {
            handle.join().expect("join concurrent writer");
        }

        let mut record_count = 0;
        for entry in fs::read_dir(&directory).expect("read concurrent logs") {
            let entry = entry.expect("read concurrent entry");
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(is_log_name(&name));
            assert!(!name.starts_with(".event-"));
            record_count += read_log_file(&entry.path())
                .expect("read complete record")
                .len();
        }
        assert_eq!(record_count, 8 * 32);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn diagnostic_document_prunes_expired_logs_before_export_snapshot() {
        let directory = temp_directory("export-retention");
        let writer = LocalLogWriter::new(directory.clone());
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        writer
            .append_failure(
                "startup",
                "internal-error",
                now - LOG_MAX_AGE - Duration::from_secs(1),
            )
            .expect("write expired record");
        writer
            .append_failure("restore", "internal-error", now)
            .expect("write retained record");

        let document = writer.build_document(now).expect("build export document");
        assert_eq!(document.error_log.len(), 1);
        assert_eq!(document.error_log[0].component, "restore");
        assert_eq!(
            fs::read_dir(&directory)
                .expect("read retained logs")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn diagnostic_document_prunes_over_capacity_logs_before_export_snapshot() {
        let directory = temp_directory("export-capacity");
        let writer = LocalLogWriter::new(directory.clone());
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let record = LocalLogRecord {
            timestamp: format_timestamp(now),
            component: "startup".to_string(),
            error_code: "internal-error".to_string(),
            message: message_for_error("internal-error").to_string(),
            stack: "redacted".to_string(),
            dialog_kind: None,
            failure_phase: None,
            exit_status_category: None,
        };
        let mut line = serde_json::to_vec(&record).expect("encode record");
        line.resize(LOG_FILE_MAX_BYTES - 1, b' ');
        line.push(b'\n');

        for index in 0..1_400_u64 {
            let millis = now
                .duration_since(UNIX_EPOCH)
                .expect("valid clock")
                .as_millis()
                .saturating_sub(u128::from(index) * 1_000);
            let path = directory.join(format!(
                "{LOG_FILE_PREFIX}{millis}-{}-{}.jsonl",
                std::process::id(),
                index
            ));
            fs::write(path, &line).expect("write oversized retention fixture");
        }
        let before: u64 = fs::read_dir(&directory)
            .expect("read capacity fixture")
            .map(|entry| entry.expect("entry").metadata().expect("metadata").len())
            .sum();
        assert!(before > LOG_TOTAL_MAX_BYTES);

        let document = writer
            .build_document(now)
            .expect("build capacity-pruned document");
        let after: u64 = fs::read_dir(&directory)
            .expect("read pruned logs")
            .map(|entry| entry.expect("entry").metadata().expect("metadata").len())
            .sum();
        assert!(after <= LOG_TOTAL_MAX_BYTES);
        assert!(document.error_log.len() < 1_400);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn archive_publish_does_not_replace_a_concurrent_destination() {
        let directory = temp_directory("publish-race");
        let temporary = directory.join(".diagnostic.zip.tmp");
        let destination = directory.join("diagnostic.zip");
        fs::write(&temporary, b"new archive").expect("write archive temporary");

        let barrier = Arc::new(Barrier::new(2));
        let creator_barrier = Arc::clone(&barrier);
        let creator_destination = destination.clone();
        let creator = thread::spawn(move || {
            creator_barrier.wait();
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(creator_destination)
            {
                Ok(mut file) => file
                    .write_all(b"created after validation")
                    .expect("write destination"),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create destination: {error}"),
            }
        });
        barrier.wait();
        let result = publish_archive(&temporary, &destination);
        creator.join().expect("join destination creator");

        let contents = fs::read(&destination).expect("read destination");
        assert!(contents == b"new archive" || contents == b"created after validation");
        if contents == b"created after validation" {
            assert_eq!(result, Err("destination-exists"));
        }
        cleanup_temporary_archive(&temporary).expect("clean up archive temporary");
        assert!(!temporary.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn diagnostic_document_contains_only_allowlisted_fields() {
        let directory = temp_directory("document");
        let writer = LocalLogWriter::new(directory.clone());
        writer
            .append_failure("sidecar", "command-worker-failed", SystemTime::now())
            .expect("write record");
        let document = writer
            .build_document(SystemTime::now())
            .expect("build document");
        let encoded = serde_json::to_value(document).expect("encode document");
        let mut keys = encoded
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "appVersion",
                "cpuArchitecture",
                "dbSchemaVersion",
                "errorLog",
                "macosVersion",
                "schemaVersion"
            ]
        );
        let text = encoded.to_string();
        assert!(text.contains("command-worker-failed"));
        assert!(!text.contains("path"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn file_dialog_metadata_is_allowlisted_and_contains_no_user_data() {
        let directory = temp_directory("file-dialog-metadata");
        let writer = LocalLogWriter::new(directory.clone());
        writer
            .append_failure_with_metadata(
                "file-dialog",
                "dialog-invalid-response",
                SystemTime::now(),
                Some("open-external-source"),
                Some("response-parse"),
                Some("non-zero"),
            )
            .expect("write file-dialog metadata");
        let entries: Vec<_> = fs::read_dir(&directory).expect("read logs").collect();
        assert_eq!(entries.len(), 1);
        let contents =
            fs::read_to_string(entries[0].as_ref().expect("entry").path()).expect("read record");
        assert!(contents.contains("\"dialogKind\":\"open-external-source\""));
        assert!(contents.contains("\"failurePhase\":\"response-parse\""));
        assert!(contents.contains("\"exitStatusCategory\":\"non-zero\""));
        assert!(!contents.contains("/Users/"));
        assert!(!contents.contains(".sqlite"));
        assert!(!contents.contains("selection-id"));

        let document = writer
            .build_document(SystemTime::now())
            .expect("build document");
        let record = &document.error_log[0];
        assert_eq!(record.component, "file-dialog");
        assert_eq!(record.error_code, "dialog-invalid-response");
        assert_eq!(record.dialog_kind.as_deref(), Some("open-external-source"));
        assert_eq!(record.failure_phase.as_deref(), Some("response-parse"));
        assert_eq!(record.exit_status_category.as_deref(), Some("non-zero"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn file_dialog_metadata_rejects_partial_metadata() {
        let directory = temp_directory("file-dialog-partial");
        let writer = LocalLogWriter::new(directory.clone());
        assert_eq!(
            writer.append_failure_with_metadata(
                "file-dialog",
                "dialog-error",
                SystemTime::now(),
                Some("save-destination"),
                None,
                Some("success"),
            ),
            Err("invalid-file-dialog-metadata")
        );
        let _ = fs::remove_dir_all(directory);
    }
}

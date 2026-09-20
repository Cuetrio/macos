use crate::update_provider::{fetch_manifest, ManifestHttpTransport};
use crate::update_selection::{select_update, UpdateSelection};
use crate::update_signature::signed_release_identity_sha256;
use crate::update_state::{
    CheckStart, CheckTrigger, PendingUpdate, UpdateState, UpdateStateError, UpdateStateSnapshot,
    UpdateStateStore, AUTO_CHECK_INTERVAL_SECONDS,
};
use crate::update_target::{UpdateTargetContext, UpdateTargetError};
use serde::Serialize;

const SELECTION_ERROR_CODE: &str = "update-selection";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckOutcome {
    NoUpdate,
    Available,
    Failed { code: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckResult {
    Started(UpdateCheckOutcome),
    Suppressed,
    AlreadyChecking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckError {
    StateStorage,
}

impl UpdateCheckError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::StateStorage => "update-state",
        }
    }
}

impl From<UpdateStateError> for UpdateCheckError {
    fn from(_: UpdateStateError) -> Self {
        Self::StateStorage
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManualUpdateCheckOutcome {
    NoUpdate,
    Available,
    Failed,
    Suppressed,
    AlreadyChecking,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ManualUpdateCheckResponse {
    pub(crate) outcome: ManualUpdateCheckOutcome,
    pub(crate) state: UpdateStateSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManualUpdateCheckCommandCode {
    UpdateTargetAppVersionInvalid,
    UpdateTargetMacosCommandFailed,
    UpdateTargetMacosOutputInvalid,
    ProviderInternal,
    UpdateCommandWorkerFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManualUpdateCheckStateCode {
    UpdateState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ManualUpdateCheckCommandError {
    CommandError { code: ManualUpdateCheckCommandCode },
    StateError { code: ManualUpdateCheckStateCode },
}

impl ManualUpdateCheckCommandError {
    pub(crate) const fn from_target(error: UpdateTargetError) -> Self {
        let code = match error {
            UpdateTargetError::InvalidAppVersion => {
                ManualUpdateCheckCommandCode::UpdateTargetAppVersionInvalid
            }
            UpdateTargetError::MacOsCommandFailed => {
                ManualUpdateCheckCommandCode::UpdateTargetMacosCommandFailed
            }
            UpdateTargetError::MacOsOutputInvalid => {
                ManualUpdateCheckCommandCode::UpdateTargetMacosOutputInvalid
            }
        };
        Self::CommandError { code }
    }

    pub(crate) const fn provider_internal() -> Self {
        Self::CommandError {
            code: ManualUpdateCheckCommandCode::ProviderInternal,
        }
    }

    pub(crate) const fn worker_failed() -> Self {
        Self::CommandError {
            code: ManualUpdateCheckCommandCode::UpdateCommandWorkerFailed,
        }
    }

    pub(crate) const fn state_error() -> Self {
        Self::StateError {
            code: ManualUpdateCheckStateCode::UpdateState,
        }
    }
}

impl From<UpdateCheckError> for ManualUpdateCheckCommandError {
    fn from(_: UpdateCheckError) -> Self {
        Self::StateError {
            code: ManualUpdateCheckStateCode::UpdateState,
        }
    }
}

pub(crate) fn manual_update_check_response(
    result: UpdateCheckResult,
    state: &UpdateState,
) -> ManualUpdateCheckResponse {
    let outcome = match result {
        UpdateCheckResult::Started(UpdateCheckOutcome::NoUpdate) => {
            ManualUpdateCheckOutcome::NoUpdate
        }
        UpdateCheckResult::Started(UpdateCheckOutcome::Available) => {
            ManualUpdateCheckOutcome::Available
        }
        UpdateCheckResult::Started(UpdateCheckOutcome::Failed { .. }) => {
            ManualUpdateCheckOutcome::Failed
        }
        UpdateCheckResult::Suppressed => ManualUpdateCheckOutcome::Suppressed,
        UpdateCheckResult::AlreadyChecking => ManualUpdateCheckOutcome::AlreadyChecking,
    };

    ManualUpdateCheckResponse {
        outcome,
        state: UpdateStateSnapshot::from(state),
    }
}

pub(crate) fn run_update_check<T: ManifestHttpTransport>(
    trigger: CheckTrigger,
    now: u64,
    target_context: &UpdateTargetContext,
    state_store: &UpdateStateStore,
    transport: &T,
) -> Result<UpdateCheckResult, UpdateCheckError> {
    let Some(_operation) = state_store.try_acquire_operation()? else {
        return Ok(UpdateCheckResult::AlreadyChecking);
    };
    match state_store.begin_check(trigger, now)? {
        CheckStart::Suppressed => return Ok(UpdateCheckResult::Suppressed),
        CheckStart::AlreadyChecking => return Ok(UpdateCheckResult::AlreadyChecking),
        CheckStart::Started => {}
    }

    let retry_at = now.saturating_add(AUTO_CHECK_INTERVAL_SECONDS);
    let manifest = match fetch_manifest(transport) {
        Ok(manifest) => manifest,
        Err(error) => {
            let code = error.code();
            state_store.record_failure(code, retry_at)?;
            return Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code,
            }));
        }
    };

    let current_version = target_context.current_app_version.to_string();
    let current_macos_version = target_context.current_macos_version.to_string();
    let selection = match select_update(
        &manifest,
        &current_version,
        target_context.target_channel,
        target_context.target_architecture,
        &current_macos_version,
    ) {
        Ok(selection) => selection,
        Err(_) => {
            state_store.record_failure(SELECTION_ERROR_CODE, retry_at)?;
            return Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code: SELECTION_ERROR_CODE,
            }));
        }
    };

    match selection {
        UpdateSelection::NoUpdate => {
            state_store.record_no_update()?;
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::NoUpdate))
        }
        UpdateSelection::Selected(release) => {
            let signed_identity_sha256 = match signed_release_identity_sha256(&manifest, release) {
                Ok(identity) => identity,
                Err(_) => {
                    state_store.record_failure(SELECTION_ERROR_CODE, retry_at)?;
                    return Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                        code: SELECTION_ERROR_CODE,
                    }));
                }
            };
            let pending_update = match PendingUpdate::new_with_signed_identity(
                release.version.to_string(),
                release.channel.clone(),
                release.architecture.clone(),
                release.artifact.artifact_id.clone(),
                release.artifact.size_bytes,
                release.artifact.sha256.clone(),
                release.signature.key_id.clone(),
                signed_identity_sha256,
                now,
            ) {
                Ok(pending_update) => pending_update,
                Err(_) => {
                    state_store.record_failure(SELECTION_ERROR_CODE, retry_at)?;
                    return Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                        code: SELECTION_ERROR_CODE,
                    }));
                }
            };
            state_store.record_available(pending_update)?;
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Available))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_manifest::{MacOsVersion, SemVer, TARGET_ARCHITECTURE, TARGET_CHANNEL};
    use crate::update_provider::{
        ManifestHttpError, ManifestHttpRequest, ManifestHttpResponse, GITHUB_RELEASES_MANIFEST_URL,
    };
    use crate::update_state::{UpdateStatus, VerificationState, UPDATE_STATE_FILE_NAME};
    use serde_json::Value;
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const EMPTY_MANIFEST: &str = r#"{
      "productId":"com.cornellmethod.notebook",
      "schemaVersion":1,
      "releases":[]
    }"#;

    const VALID_MANIFEST: &str = r#"{
      "productId":"com.cornellmethod.notebook",
      "schemaVersion":1,
      "releases":[{
        "channel":"stable",
        "version":"1.2.3",
        "architecture":"aarch64-apple-darwin",
        "minVersion":"14",
        "artifact":{
          "artifactId":"cornell-method-notebook-1.2.3",
          "format":"app-archive",
          "url":"https://updates.example.test/cornell-method-notebook-1.2.3",
          "sizeBytes":123,
          "sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        },
        "signature":{"keyId":"test-key","proof":"opaque-proof"}
      }]
    }"#;

    static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct FakeTransport {
        calls: Cell<u32>,
        result: Result<ManifestHttpResponse, ManifestHttpError>,
    }

    impl FakeTransport {
        fn from_body(body: &str) -> Self {
            let body = body.as_bytes().to_vec();
            Self {
                calls: Cell::new(0),
                result: Ok(ManifestHttpResponse {
                    status: 200,
                    content_type: Some("application/json".to_string()),
                    content_length: Some(body.len() as u64),
                    body,
                    redirects: Vec::new(),
                    final_url: GITHUB_RELEASES_MANIFEST_URL.to_string(),
                }),
            }
        }

        fn failure(error: ManifestHttpError) -> Self {
            Self {
                calls: Cell::new(0),
                result: Err(error),
            }
        }
    }

    impl ManifestHttpTransport for FakeTransport {
        fn get(
            &self,
            request: ManifestHttpRequest,
        ) -> Result<ManifestHttpResponse, ManifestHttpError> {
            assert_eq!(request.url, GITHUB_RELEASES_MANIFEST_URL);
            self.calls.set(self.calls.get() + 1);
            self.result.clone()
        }
    }

    fn test_directory(label: &str) -> PathBuf {
        let counter = TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "cuetrio-update-check-{label}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn store(label: &str) -> (PathBuf, UpdateStateStore) {
        let directory = test_directory(label);
        let staging_directory = directory.join("staging");
        fs::create_dir_all(&staging_directory).unwrap();
        let store = UpdateStateStore::load_or_default(&directory, &staging_directory);
        (directory, store)
    }

    fn cleanup(directory: &Path) {
        fs::remove_dir_all(directory).unwrap();
    }

    fn run<T: ManifestHttpTransport>(
        store: &UpdateStateStore,
        transport: &T,
        trigger: CheckTrigger,
        now: u64,
        current_version: &str,
    ) -> Result<UpdateCheckResult, UpdateCheckError> {
        let target_context = UpdateTargetContext {
            current_app_version: SemVer::parse(current_version).unwrap(),
            target_channel: TARGET_CHANNEL,
            target_architecture: TARGET_ARCHITECTURE,
            current_macos_version: MacOsVersion::parse("14", "test macOS version").unwrap(),
        };
        run_update_check(trigger, now, &target_context, store, transport)
    }

    #[test]
    fn automatic_checks_are_suppressed_for_a_day_but_manual_checks_bypass_the_limit() {
        let (directory, store) = store("daily");
        let transport = FakeTransport::from_body(EMPTY_MANIFEST);

        assert_eq!(
            run(&store, &transport, CheckTrigger::Automatic, 100, "1.0.0"),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::NoUpdate))
        );
        assert_eq!(transport.calls.get(), 1);

        assert_eq!(
            run(
                &store,
                &transport,
                CheckTrigger::Automatic,
                100 + AUTO_CHECK_INTERVAL_SECONDS - 1,
                "1.0.0",
            ),
            Ok(UpdateCheckResult::Suppressed)
        );
        assert_eq!(transport.calls.get(), 1);

        assert_eq!(
            run(
                &store,
                &transport,
                CheckTrigger::Manual,
                100 + AUTO_CHECK_INTERVAL_SECONDS - 1,
                "1.0.0",
            ),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::NoUpdate))
        );
        assert_eq!(transport.calls.get(), 2);

        cleanup(&directory);
    }

    #[test]
    fn already_checking_returns_without_calling_the_provider() {
        let (directory, store) = store("already-checking");
        let transport = FakeTransport::from_body(EMPTY_MANIFEST);
        assert_eq!(
            store.begin_check(CheckTrigger::Manual, 100).unwrap(),
            CheckStart::Started
        );

        assert_eq!(
            run(&store, &transport, CheckTrigger::Manual, 101, "1.0.0"),
            Ok(UpdateCheckResult::AlreadyChecking)
        );
        assert_eq!(transport.calls.get(), 0);
        assert_eq!(store.snapshot().status, UpdateStatus::Checking);

        cleanup(&directory);
    }

    #[test]
    fn empty_and_non_target_manifests_are_saved_as_no_update() {
        for (label, manifest) in [
            ("empty", EMPTY_MANIFEST.to_string()),
            (
                "non-target",
                VALID_MANIFEST.replace("\"stable\"", "\"beta\""),
            ),
        ] {
            let (directory, store) = store(label);
            let transport = FakeTransport::from_body(&manifest);

            assert_eq!(
                run(&store, &transport, CheckTrigger::Manual, 100, "1.0.0"),
                Ok(UpdateCheckResult::Started(UpdateCheckOutcome::NoUpdate))
            );
            assert_eq!(transport.calls.get(), 1);
            assert_eq!(store.snapshot().status, UpdateStatus::NoUpdate);

            cleanup(&directory);
        }
    }

    #[test]
    fn selected_release_is_saved_as_not_verified_pending_state_without_package_data() {
        let (directory, store) = store("available");
        let transport = FakeTransport::from_body(VALID_MANIFEST);

        assert_eq!(
            run(&store, &transport, CheckTrigger::Manual, 123, "1.0.0"),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Available))
        );
        assert_eq!(transport.calls.get(), 1);

        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        let pending = snapshot.pending_update.expect("available update");
        assert_eq!(pending.version, "1.2.3");
        assert_eq!(pending.channel, TARGET_CHANNEL);
        assert_eq!(pending.architecture, TARGET_ARCHITECTURE);
        assert_eq!(pending.artifact, "cornell-method-notebook-1.2.3");
        assert_eq!(pending.verification_state, VerificationState::NotVerified);
        assert_eq!(pending.size_bytes, Some(123));
        assert_eq!(
            pending.sha256.as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(pending.key_id.as_deref(), Some("test-key"));
        assert!(pending
            .signed_identity_sha256
            .as_deref()
            .is_some_and(|identity| identity.len() == 64));
        assert_eq!(pending.package_path, None);
        assert_eq!(pending.extracted_app_path, None);
        assert_eq!(pending.discovered_at, 123);
        assert_eq!(pending.verified_at, None);

        let state_contents = fs::read_to_string(store.state_path()).unwrap();
        let state_json: Value = serde_json::from_str(&state_contents).unwrap();
        assert_eq!(state_json["schemaVersion"], 2);
        assert_eq!(state_json["phase"], Value::Null);
        assert_eq!(state_json["pendingUpdate"]["sizeBytes"], 123);
        assert_eq!(
            state_json["pendingUpdate"]["sha256"],
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(state_json["pendingUpdate"]["keyId"], "test-key");
        assert_eq!(
            state_json["pendingUpdate"]["signedIdentitySha256"]
                .as_str()
                .map(str::len),
            Some(64)
        );
        assert!(state_json["pendingUpdate"]["packagePath"].is_null());
        assert!(state_json["pendingUpdate"]["extractedAppPath"].is_null());
        assert!(state_json.get("url").is_none());
        assert!(state_json.get("signature").is_none());
        assert!(!state_contents.contains("updates.example.test"));
        assert!(!state_contents.contains("opaque-proof"));

        cleanup(&directory);
    }

    #[test]
    fn provider_failure_persists_only_its_fixed_code_and_daily_retry_time() {
        let (directory, store) = store("provider-failure");
        let transport = FakeTransport::failure(ManifestHttpError::Timeout);

        assert_eq!(
            run(&store, &transport, CheckTrigger::Manual, 123, "1.0.0"),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code: "provider-timeout",
            }))
        );
        assert_eq!(transport.calls.get(), 1);

        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Failed);
        let failure = snapshot.failure.expect("provider failure");
        assert_eq!(failure.code, "provider-timeout");
        assert_eq!(failure.retry_at, 123 + AUTO_CHECK_INTERVAL_SECONDS);

        cleanup(&directory);
    }

    #[test]
    fn provider_timeout_releases_operation_guard_for_a_following_check() {
        let (directory, store) = store("provider-timeout-guard");
        let transport = FakeTransport::failure(ManifestHttpError::Timeout);

        assert_eq!(
            run(&store, &transport, CheckTrigger::Manual, 123, "1.0.0"),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code: "provider-timeout",
            }))
        );
        assert_eq!(store.snapshot().status, UpdateStatus::Failed);

        assert_eq!(
            run(&store, &transport, CheckTrigger::Manual, 124, "1.0.0"),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code: "provider-timeout",
            }))
        );
        assert_eq!(transport.calls.get(), 2);

        cleanup(&directory);
    }

    #[test]
    fn provider_failure_during_recheck_keeps_the_existing_candidate_available() {
        let (directory, store) = store("provider-recheck-failure");
        let available_transport = FakeTransport::from_body(VALID_MANIFEST);

        assert_eq!(
            run(
                &store,
                &available_transport,
                CheckTrigger::Manual,
                100,
                "1.0.0"
            ),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Available))
        );
        let candidate_before = store.snapshot().pending_update.unwrap();

        let failure_transport = FakeTransport::failure(ManifestHttpError::Timeout);
        assert_eq!(
            run(
                &store,
                &failure_transport,
                CheckTrigger::Manual,
                200,
                "1.0.0"
            ),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code: "provider-timeout",
            }))
        );

        let snapshot = store.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert_eq!(snapshot.pending_update, Some(candidate_before));
        assert_eq!(snapshot.failure.as_ref().unwrap().code, "provider-timeout");
        assert_eq!(
            snapshot.failure.as_ref().unwrap().retry_at,
            200 + AUTO_CHECK_INTERVAL_SECONDS
        );

        let response = manual_update_check_response(
            UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                code: "provider-timeout",
            }),
            &snapshot,
        );
        assert_eq!(response.state.status, UpdateStatus::Available);
        assert!(response.state.pending_update.is_some());
        assert!(response.state.failure.is_some());

        cleanup(&directory);
    }

    #[test]
    fn manifest_check_recovery_preserves_existing_candidate_and_notification() {
        let (directory, store) = store("manifest-check-recovery");
        let transport = FakeTransport::from_body(VALID_MANIFEST);

        assert_eq!(
            run(&store, &transport, CheckTrigger::Manual, 100, "1.0.0"),
            Ok(UpdateCheckResult::Started(UpdateCheckOutcome::Available))
        );
        let candidate_before = store.snapshot().pending_update.clone();
        assert!(store.claim_pending_notification(110).unwrap());
        let notification_before = store.snapshot().notification.clone();

        store.begin_check(CheckTrigger::Manual, 200).unwrap();
        drop(store);

        let staging_directory = directory.join("staging");
        let recovered = UpdateStateStore::load_or_default(&directory, &staging_directory);
        let snapshot = recovered.snapshot();
        assert_eq!(snapshot.status, UpdateStatus::Available);
        assert_eq!(snapshot.pending_update, candidate_before);
        assert_eq!(snapshot.notification, notification_before);
        assert_eq!(snapshot.failure.as_ref().unwrap().code, "check-interrupted");

        cleanup(&directory);
    }

    #[test]
    fn manual_response_mapping_keeps_terminal_outcomes_and_sanitized_errors() {
        let state = UpdateState::initial();
        for (result, expected) in [
            (
                UpdateCheckResult::Started(UpdateCheckOutcome::NoUpdate),
                ManualUpdateCheckOutcome::NoUpdate,
            ),
            (
                UpdateCheckResult::Started(UpdateCheckOutcome::Available),
                ManualUpdateCheckOutcome::Available,
            ),
            (
                UpdateCheckResult::Started(UpdateCheckOutcome::Failed {
                    code: "provider-timeout",
                }),
                ManualUpdateCheckOutcome::Failed,
            ),
            (
                UpdateCheckResult::Suppressed,
                ManualUpdateCheckOutcome::Suppressed,
            ),
            (
                UpdateCheckResult::AlreadyChecking,
                ManualUpdateCheckOutcome::AlreadyChecking,
            ),
        ] {
            let response = manual_update_check_response(result, &state);
            assert_eq!(response.outcome, expected);
            let value = serde_json::to_value(response).unwrap();
            assert_eq!(value["state"]["snapshotVersion"], 1);
            assert!(value.get("schemaVersion").is_none());
            assert!(value.get("notification").is_none());
            assert!(value.get("url").is_none());
        }

        let command_error = serde_json::to_value(ManualUpdateCheckCommandError::CommandError {
            code: ManualUpdateCheckCommandCode::ProviderInternal,
        })
        .unwrap();
        assert_eq!(command_error["kind"], "command-error");
        assert_eq!(command_error["code"], "provider-internal");
        assert!(command_error.get("message").is_none());
        assert!(command_error.get("source").is_none());
        assert!(command_error.get("url").is_none());

        let state_error = serde_json::to_value(ManualUpdateCheckCommandError::from(
            UpdateCheckError::StateStorage,
        ))
        .unwrap();
        assert_eq!(state_error["kind"], "state-error");
        assert_eq!(state_error["code"], "update-state");

        for (target_error, expected_code) in [
            (
                UpdateTargetError::InvalidAppVersion,
                "update-target-app-version-invalid",
            ),
            (
                UpdateTargetError::MacOsCommandFailed,
                "update-target-macos-command-failed",
            ),
            (
                UpdateTargetError::MacOsOutputInvalid,
                "update-target-macos-output-invalid",
            ),
        ] {
            let target_error =
                serde_json::to_value(ManualUpdateCheckCommandError::from_target(target_error))
                    .unwrap();
            assert_eq!(target_error["kind"], "command-error");
            assert_eq!(target_error["code"], expected_code);
        }

        let worker_error =
            serde_json::to_value(ManualUpdateCheckCommandError::worker_failed()).unwrap();
        assert_eq!(worker_error["kind"], "command-error");
        assert_eq!(worker_error["code"], "update-command-worker-failed");
    }

    #[test]
    fn state_write_failure_is_distinct_and_prevents_provider_work() {
        let (directory, store_path) = {
            let directory = test_directory("state-write-failure");
            let state_path = directory.join(UPDATE_STATE_FILE_NAME);
            fs::create_dir(&state_path).unwrap();
            let staging_directory = directory.join("staging");
            fs::create_dir_all(&staging_directory).unwrap();
            let store = UpdateStateStore::load_or_default(&directory, &staging_directory);
            (directory, store)
        };
        let transport = FakeTransport::from_body(EMPTY_MANIFEST);

        assert_eq!(
            run(&store_path, &transport, CheckTrigger::Manual, 123, "1.0.0"),
            Err(UpdateCheckError::StateStorage)
        );
        assert_eq!(transport.calls.get(), 0);
        assert_eq!(store_path.snapshot().status, UpdateStatus::NotChecked);

        cleanup(&directory);
    }
}

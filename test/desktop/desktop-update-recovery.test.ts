import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
export {};

const projectRoot = path.resolve(__dirname, "../..");

function read(relativePath: string) {
  return fs.readFileSync(path.join(projectRoot, relativePath), "utf8");
}

function disposableFixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cornell-update-recovery-"));
  const support = path.join(root, "Application Support", "com.cornellmethod.notebook");
  const staging = path.join(support, "staging", "extract", "a".repeat(64));
  const candidate = path.join(staging, "Cuetrio.app");
  const current = path.join(root, "Cuetrio.app");
  const live = path.join(support, "live", "notebook.sqlite");
  const backup = path.join(
    support,
    "backups",
    `notebook-${"a".repeat(64)}-100-random.sqlite.bak`,
  );
  for (const directory of [candidate, current, path.dirname(live), path.dirname(backup)]) {
    fs.mkdirSync(directory, { recursive: true });
  }
  fs.writeFileSync(path.join(candidate, "candidate.marker"), "candidate");
  fs.writeFileSync(path.join(current, "current.marker"), "current");
  fs.writeFileSync(live, "live-before-health");
  fs.writeFileSync(backup, "live-before-health");
  return {
    root,
    candidate,
    current,
    live,
    backup,
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

function partialCleanupFixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cornell-update-recovery-partial-"));
  const support = path.join(root, "Application Support", "com.cornellmethod.notebook");
  const digest = "a".repeat(64);
  const staging = path.join(support, "staging");
  const candidate = path.join(
    staging,
    "extract",
    digest,
    "Cuetrio.app",
  );
  const packagePath = path.join(staging, "packages", `${digest}.app.tar.gz`);
  const migration = path.join(staging, "database-migrations", "migration.sql");
  const backup = path.join(
    support,
    "backups",
    `notebook-${digest}-100-random.sqlite.bak`,
  );
  const current = path.join(root, "Cuetrio.app");
  const live = path.join(support, "live", "notebook.sqlite");

  for (const directory of [
    candidate,
    current,
    path.dirname(packagePath),
    path.dirname(migration),
    path.dirname(backup),
    path.dirname(live),
  ]) {
    fs.mkdirSync(directory, { recursive: true });
  }
  fs.writeFileSync(path.join(candidate, "candidate.marker"), "candidate");
  fs.writeFileSync(path.join(current, "current.marker"), "current");
  fs.writeFileSync(packagePath, "package remains after partial cleanup");
  fs.writeFileSync(migration, "migration remains after partial cleanup");
  fs.writeFileSync(backup, "backup remains after partial cleanup");
  fs.writeFileSync(live, "live");

  return {
    root,
    candidate,
    packagePath,
    migration,
    backup,
    current,
    live,
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

test("recovery persists typed checkpoints and separates health, switch, rollback, and cleanup", () => {
  const state = read("src-tauri/src/update_state.rs");
  const recovery = read("src-tauri/src/update_recovery.rs");

  for (const marker of [
    "HealthPending",
    "BundleSwitching",
    "BundleSwitched",
    "CleanupPending",
    "RollbackPending",
    "record_bundle_switching",
    "record_bundle_switched",
    "record_cleanup_pending",
    "record_recovery_failure",
    "record_rollback_completed",
    "complete_update",
  ]) {
    assert.match(state, new RegExp(marker));
  }
  assert.match(recovery, /run_candidate_health/);
  assert.match(recovery, /switch_bundle/);
  assert.match(recovery, /rollback_bundle_if_needed/);
  assert.match(recovery, /restore_database_if_needed/);
  assert.match(recovery, /validate_database_command/);
  assert.match(recovery, /cleanup_after_success/);
  assert.match(recovery, /state_store\s*\.record_bundle_switching\(\)/);
  assert.match(recovery, /state_store\s*\.record_bundle_switched\(\)/);
  assert.match(recovery, /state_store\s*\.record_cleanup_pending\(\)/);
  assert.match(recovery, /state_store\s*\.complete_update\(\)/);
});

test("candidate health precedes bundle switch and cleanup, while failures stay fail-closed", () => {
  const recovery = read("src-tauri/src/update_recovery.rs");
  const health = recovery.indexOf("run_candidate_health(candidate.runtime_root()?");
  const switchCheckpoint = recovery.indexOf("record_bundle_switching", health);
  const switchIndex = recovery.indexOf("switch_bundle(&paths", switchCheckpoint);
  const switchedCheckpoint = recovery.indexOf("record_bundle_switched", switchIndex);
  const cleanupCheckpoint = recovery.indexOf("record_cleanup_pending", switchedCheckpoint);
  const cleanup = recovery.indexOf("cleanup_after_success", cleanupCheckpoint);

  assert.ok(health >= 0);
  assert.ok(switchCheckpoint > health);
  assert.ok(switchIndex > switchCheckpoint);
  assert.ok(switchedCheckpoint > switchIndex);
  assert.ok(cleanupCheckpoint > switchedCheckpoint);
  assert.ok(cleanup > cleanupCheckpoint);
  assert.doesNotMatch(recovery, /run_candidate_health\(\s*&?candidate\.source_path/);
  assert.match(recovery, /candidate-health-failed/);
  assert.match(recovery, /update-switch-failed/);
  assert.match(recovery, /update-restore-failed/);
  assert.match(recovery, /update-cleanup-failed/);
  assert.doesNotMatch(recovery, /remove_dir_all/);
});

test("recovery shares archive safe-symlink policy and never follows links during copy or cleanup", () => {
  const recovery = read("src-tauri/src/update_recovery.rs");
  const archive = read("src-tauri/src/update_archive.rs");

  for (const marker of [
    "normalize_relative_symlink_target",
    "resolve_relative_symlink_path",
    "MAX_SYMLINK_HOPS",
    "metadata_no_follow_inside_bundle",
    "fs::symlink_metadata",
    "fs::read_link",
    "create_recovery_symlink",
    "copy_tree_preserving_safe_symlinks",
    "copy_regular_file_no_follow",
    "fs::remove_file(path)",
  ]) {
    assert.match(recovery, new RegExp(marker.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.match(archive, /pub\(crate\) fn normalize_relative_symlink_target/);
  assert.match(archive, /pub\(crate\) fn resolve_relative_symlink_path/);
  assert.doesNotMatch(recovery, /validate_tree_no_symlinks/);
  assert.match(recovery, /SafeSymlinkPathError::InvalidTarget/);
  assert.match(recovery, /SafeSymlinkPathError::EscapesRoot/);
  assert.match(recovery, /symlink chain contains a cycle/);
  assert.match(recovery, /maximum hop count/);
  assert.match(recovery, /candidate app bundle/);
  assert.match(recovery, /bundle switch temporary parent/);
  assert.match(recovery, /live database directory/);
});

test("successful rollback leaves a reloadable failure terminal state while rollback errors stay pending", () => {
  const state = read("src-tauri/src/update_state.rs");
  const recovery = read("src-tauri/src/update_recovery.rs");
  const completionStart = state.indexOf("pub(crate) fn record_rollback_completed");
  const completionEnd = state.indexOf("pub(crate) fn complete_update", completionStart);
  const completion = state.slice(completionStart, completionEnd);
  const rollbackStart = recovery.indexOf("fn recover_rollback");
  const rollbackEnd = recovery.indexOf("fn rollback_after_health_failure", rollbackStart);
  const rollback = recovery.slice(rollbackStart, rollbackEnd);

  assert.ok(completionStart >= 0);
  assert.ok(completionEnd > completionStart);
  assert.match(
    completion,
    /state\.status = UpdateStatus::Available[\s\S]*state\.phase = None[\s\S]*state\.failure = Some[\s\S]*state\.restart_handoff = None[\s\S]*state\.recovery = None/,
  );
  assert.match(completion, /pending_update[\s\S]*is_none_or/);
  assert.match(recovery, /record_recovery_failure[\s\S]*update-rollback-failed/);
  assert.ok(rollback.indexOf("restore_database_if_needed") < rollback.indexOf("record_rollback_completed"));
  assert.match(rollback, /record_rollback_completed\(failure_code, retry_at\)/);
  assert.match(recovery, /rollback_after_health_failure[\s\S]*record_rollback_completed\("candidate-health-failed"/);
  assert.match(recovery, /handle_health_failure[\s\S]*record_rollback_completed\("candidate-health-failed"/);
});

test("Issue #165: successful rollback removes only its failed marker before terminal completion", () => {
  const recovery = read("src-tauri/src/update_recovery.rs");
  const helperStart = recovery.indexOf("fn cleanup_failed_marker_before_rollback_completion");
  const helperEnd = recovery.indexOf("\nfn handle_switch_failure", helperStart);
  const helper = recovery.slice(helperStart, helperEnd);
  const markerStart = recovery.indexOf("fn remove_failed_bundle_marker");
  const markerEnd = recovery.indexOf("\nfn remove_if_safe_tree", markerStart);
  const marker = recovery.slice(markerStart, markerEnd);

  assert.ok(helperStart >= 0);
  assert.ok(helperEnd > helperStart);
  assert.ok(markerStart >= 0);
  assert.ok(markerEnd > markerStart);
  assert.match(marker, /paths\.failed\.parent\(\)[\s\S]*require_safe_directory\(parent/);
  assert.match(marker, /require_safe_bundle_tree[\s\S]*remove_validated_tree/);
  assert.doesNotMatch(marker, /remove_dir_all/);
  assert.match(
    helper,
    /record_recovery_failure\([\s\S]*UpdateRecoveryStage::RollbackPending[\s\S]*"update-rollback-failed"/,
  );

  function body(name: string) {
    const start = recovery.indexOf(`fn ${name}`);
    const end = recovery.indexOf("\nfn ", start + 1);
    return recovery.slice(start, end < 0 ? recovery.length : end);
  }

  for (const name of [
    "recover_rollback",
    "rollback_after_health_failure",
    "handle_health_failure",
  ]) {
    const rollback = body(name);
    const bundleRollback = rollback.indexOf("rollback_bundle_if_needed");
    const restore = rollback.indexOf("restore_database_if_needed");
    const cleanup = rollback.indexOf("cleanup_failed_marker_before_rollback_completion");
    const completion = rollback.indexOf("record_rollback_completed");
    if (name !== "handle_health_failure") {
      assert.ok(bundleRollback >= 0, `${name} rolls back the bundle`);
      assert.ok(cleanup > bundleRollback, `${name} cleans the marker after bundle rollback`);
    }
    assert.ok(restore >= 0, `${name} restores the database`);
    assert.ok(cleanup > restore, `${name} cleans the marker after restore`);
    assert.ok(completion > cleanup, `${name} records completion after marker cleanup`);
  }

  assert.match(
    recovery,
    /if path_exists\(&paths\.failed\)\?[\s\S]*failed candidate bundle marker is already occupied/,
  );
  assert.match(recovery, /BUNDLE_FAILED_PREFIX/);
  assert.match(recovery, /BUNDLE_ROLLBACK_PREFIX/);
});

test("candidate health derives the fixed packaged runtime root after bundle validation", () => {
  const recovery = read("src-tauri/src/update_recovery.rs");
  const runtime = read("src-tauri/src/runtime.rs");
  const candidateFromStateStart = recovery.indexOf("fn candidate_from_state");
  const candidateMetadataStart = recovery.indexOf("fn candidate_metadata_from_state", candidateFromStateStart);
  const runtimeValidationStart = recovery.indexOf("fn validated_candidate_runtime_root");
  const runtimeValidation = recovery.slice(runtimeValidationStart);

  assert.ok(candidateFromStateStart >= 0);
  assert.ok(candidateMetadataStart > candidateFromStateStart);
  const candidateFromState = recovery.slice(candidateFromStateStart, candidateMetadataStart);
  const sourcePath = candidateFromState.indexOf("candidate_source_path");
  const runtimeDerivation = candidateFromState.indexOf("validated_candidate_runtime_root", sourcePath);
  assert.ok(sourcePath >= 0);
  assert.ok(runtimeDerivation > sourcePath);

  assert.match(
    runtimeValidation,
    /require_candidate_bundle_at\([\s\S]*?require_safe_bundle_tree\([\s\S]*?packaged_runtime_root\(/,
  );
  assert.match(runtime, /pub\(crate\) fn packaged_runtime_root\(bundle_root: &Path\)/);
  assert.match(
    runtime,
    /bundle_root\s*\.join\(PACKAGED_CONTENTS_DIRECTORY_NAME\)\s*\.join\(PACKAGED_RESOURCES_DIRECTORY_NAME\)\s*\.join\(PACKAGED_RUNTIME_DIRECTORY_NAME\)/,
  );
  assert.doesNotMatch(recovery, /run_candidate_health\(\s*&?candidate\.source_path/);
  assert.doesNotMatch(recovery, /start_sidecar\(\s*&?candidate\.source_path/);
});

test("disposable current/live/backup fixture has the pre-health preservation boundary", () => {
  const fixture = disposableFixture();
  try {
    const currentBefore = fs.readFileSync(path.join(fixture.current, "current.marker"));
    const liveBefore = fs.readFileSync(fixture.live);
    const backupBefore = fs.readFileSync(fixture.backup);
    const recovery = read("src-tauri/src/update_recovery.rs");

    assert.equal(fs.existsSync(fixture.candidate), true);
    assert.deepEqual(fs.readFileSync(path.join(fixture.current, "current.marker")), currentBefore);
    assert.deepEqual(fs.readFileSync(fixture.live), liveBefore);
    assert.deepEqual(fs.readFileSync(fixture.backup), backupBefore);
    assert.match(recovery, /candidate health failed/);
    assert.match(recovery, /rollback_after_health_failure/);
  } finally {
    fixture.cleanup();
  }
});

test("startup invokes recovery before bootstrap and does not let normal update checks apply", () => {
  const main = read("src-tauri/src/main.rs");
  const setup = main.slice(main.indexOf(".setup(move |app|"), main.indexOf("\n        .run("));
  const migration = setup.indexOf("run_startup_staged_migration");
  const recovery = setup.indexOf("run_startup_update_recovery");
  const bootstrap = setup.indexOf("run_bootstrap_with_storage");

  assert.ok(migration >= 0);
  assert.ok(recovery > migration);
  assert.ok(bootstrap > recovery);
  assert.doesNotMatch(
    setup.slice(bootstrap),
    /apply_verified_update|verify_pending_update|run_update_check\(\s*CheckTrigger::Automatic/,
  );
});

test("Issue #167: staged migration failure is recovered in the same startup and recovery errors remain fatal", () => {
  const main = read("src-tauri/src/main.rs");
  const migration = read("src-tauri/src/update_migration.rs");
  const setup = main.slice(main.indexOf(".setup(move |app|"), main.indexOf("\n        .run("));
  const migrationCall = setup.indexOf("run_startup_staged_migration");
  const recoveryCall = setup.indexOf("run_startup_update_recovery");
  const bootstrapCall = setup.indexOf("run_bootstrap_with_storage");

  assert.match(migration, /record_staged_migration_failure/);
  assert.match(migration, /StartupStagedMigrationOutcome::Failed/);
  assert.match(setup, /StartupStagedMigrationOutcome::Failed \{ code, reason \}/);
  assert.match(setup, /startup update recovery failed/);
  assert.match(setup, /return Err\(boxed_error\(error\)\)/);
  assert.match(setup, /desktop staged migration failed \(\{code\}\); startup recovery completed/);
  assert.ok(recoveryCall > migrationCall);
  assert.ok(bootstrapCall > recoveryCall);
});

test("Issue #172: only an explicitly proven database switch authorizes restore", () => {
  const migration = read("src-tauri/src/update_migration.rs");
  const state = read("src-tauri/src/update_state.rs");
  const recovery = read("src-tauri/src/update_recovery.rs");
  const restoreStart = recovery.indexOf("fn restore_database_if_needed");
  const restoreEnd = recovery.indexOf("\nfn atomic_restore_database", restoreStart);
  const restore = recovery.slice(restoreStart, restoreEnd);
  const guard = restore.indexOf("if !database_switch_was_proven(database_hint)");
  const backupLookup = restore.indexOf("find_safety_backup");

  assert.match(
    migration,
    /StagedMigrationOutcome::Switched[\s\S]*?record_staged_migration_switched\(\)/,
  );
  assert.match(
    state,
    /record_staged_migration_failure[\s\S]*?database_switched: Some\(false\)/,
  );
  assert.match(
    state,
    /record_staged_migration_switched[\s\S]*?database_switched: Some\(true\)/,
  );
  assert.match(
    recovery,
    /fn database_switch_was_proven\(database_hint: Option<bool>\) -> bool[\s\S]*?database_hint == Some\(true\)/,
  );
  assert.ok(guard >= 0, "restore must have a fail-closed switch guard");
  assert.ok(backupLookup > guard, "backup presence must not authorize restore");
  assert.match(recovery, /database_restore_requires_explicit_switch_evidence/);
});

test("Issue #171: restore cleanup tracks created paths across token mismatches and retries", () => {
  const recovery = read("src-tauri/src/update_recovery.rs");
  const restoreStart = recovery.indexOf("fn restore_database_if_needed");
  const restoreEnd = recovery.indexOf("\nfn find_safety_backup", restoreStart);
  const restore = recovery.slice(restoreStart, restoreEnd);
  const atomicStart = recovery.indexOf("fn atomic_restore_database");
  const atomicEnd = recovery.indexOf("\nfn find_safety_backup", atomicStart);
  const atomic = recovery.slice(atomicStart, atomicEnd);
  const cleanupStart = recovery.indexOf("fn remove_restore_temporary_files");
  const cleanupEnd = recovery.indexOf("\nfn remove_stale_restore_temporary_files", cleanupStart);
  const cleanup = recovery.slice(cleanupStart, cleanupEnd);

  assert.match(atomic, /Result<RestoreTemporaryPaths, String>/);
  assert.match(
    atomic,
    /restore_temporary_paths\(storage\.live_directory\(\), backup\)/,
  );
  assert.match(
    restore,
    /atomic_restore_database\(storage, &backup, &backup_bytes\)[\s\S]*?remove_restore_temporary_files\(storage\.live_directory\(\), &temporary_paths\)/,
  );
  assert.doesNotMatch(cleanup, /candidate\.sha256/);
  assert.match(recovery, /path\.parent\(\) != Some\(live_directory\)/);
  assert.match(cleanup, /validate_restore_temporary_path/);
  assert.match(
    recovery,
    /fn remove_stale_restore_temporary_files[\s\S]*?fs::read_dir\(live_directory\)[\s\S]*?restore_temporary_prefix/,
  );
  assert.match(recovery, /restore temporary cleanup failed/);

  for (const name of [
    "recover_rollback",
    "rollback_after_health_failure",
    "handle_health_failure",
  ]) {
    const start = recovery.indexOf(`fn ${name}`);
    const end = recovery.indexOf("\nfn ", start + 1);
    const body = recovery.slice(start, end < 0 ? recovery.length : end);
    const restoreIndex = body.indexOf("restore_database_if_needed");
    const cleanupIndex = body.indexOf("cleanup_failed_marker_before_rollback_completion");
    const completionIndex = body.indexOf("record_rollback_completed");
    assert.ok(restoreIndex >= 0, `${name} restores the database`);
    assert.ok(cleanupIndex > restoreIndex, `${name} cleans restore temps before completion`);
    assert.ok(completionIndex > cleanupIndex, `${name} completes after cleanup`);
  }
});

test("Issue #173: partial switch temps are discarded and rebuilt from the candidate source", () => {
  const recovery = read("src-tauri/src/update_recovery.rs");
  const switchStart = recovery.indexOf("fn switch_bundle");
  const switchEnd = recovery.indexOf("\nfn rollback_bundle_if_needed", switchStart);
  const switchBundle = recovery.slice(switchStart, switchEnd);
  const reconcileStart = recovery.indexOf("fn reconcile_bundle_switch");
  const reconcileEnd = recovery.indexOf("\nfn finish_switched_candidate", reconcileStart);
  const reconcile = recovery.slice(reconcileStart, reconcileEnd);
  const prepareStart = recovery.indexOf("fn prepare_switch_temp");
  const prepareEnd = recovery.indexOf("\nfn validate_complete_switch_temp", prepareStart);
  const prepare = recovery.slice(prepareStart, prepareEnd);
  const copyStart = recovery.indexOf("fn copy_bundle_tree");
  const copyEnd = recovery.indexOf("\nfn copy_tree_preserving_safe_symlinks", copyStart);
  const copy = recovery.slice(copyStart, copyEnd);
  const cleanupStart = recovery.indexOf("fn remove_partial_bundle_tree");
  const cleanupEnd = recovery.indexOf("\nfn validate_tree_for_no_follow_removal", cleanupStart);
  const cleanup = recovery.slice(cleanupStart, cleanupEnd);

  assert.match(switchBundle, /prepare_switch_temp\(paths, candidate\)/);
  const recoveryTempValidation = reconcile.indexOf("validate_complete_switch_temp(paths, candidate)");
  const recoveryHealth = reconcile.indexOf("run_candidate_health", recoveryTempValidation);
  assert.ok(recoveryTempValidation >= 0);
  assert.ok(recoveryHealth > recoveryTempValidation);
  const existingTempValidation = prepare.indexOf("match validate_complete_switch_temp(paths, candidate)");
  const discard = prepare.indexOf("discard_switch_temp(paths)", existingTempValidation);
  const rebuild = prepare.indexOf("copy_bundle_tree(&candidate.source_path", discard);
  assert.ok(existingTempValidation >= 0);
  assert.ok(discard > existingTempValidation);
  assert.ok(rebuild > discard);
  assert.match(prepare, /validate_complete_switch_temp/);
  assert.match(recovery, /compare_bundle_trees\(/);
  assert.match(copy, /copy_tree_preserving_safe_symlinks[\s\S]*remove_partial_bundle_tree/);
  assert.match(cleanup, /path\.parent\(\) != Some\(parent\)/);
  assert.match(cleanup, /validate_no_symlink_components/);
  assert.match(recovery, /validate_tree_for_no_follow_removal[\s\S]*remove_tree_no_follow/);
  assert.match(recovery, /custom_flags\(libc::O_NOFOLLOW\)/);
});

test("Issue #170: partial cleanup retry accepts a deleted candidate leaf and reaches completion", () => {
  const fixture = partialCleanupFixture();
  try {
    // The first cleanup attempt already removed the candidate bundle before a
    // later artifact cleanup failed.  The remaining artifacts are the retry
    // fixture, not evidence that the candidate should be recreated.
    fs.rmSync(fixture.candidate, { recursive: true, force: true });
    assert.equal(fs.existsSync(fixture.candidate), false);
    assert.equal(fs.existsSync(fixture.packagePath), true);
    assert.equal(fs.existsSync(fixture.migration), true);
    assert.equal(fs.existsSync(fixture.backup), true);

    const recovery = read("src-tauri/src/update_recovery.rs");
    const cleanupStart = recovery.indexOf("fn cleanup_after_success");
    const cleanupEnd = recovery.indexOf("\nfn remove_restore_temporary_files", cleanupStart);
    const cleanup = recovery.slice(cleanupStart, cleanupEnd);
    const candidateCleanup = cleanup.indexOf('"candidate bundle cleanup"');
    const packageCleanup = cleanup.indexOf('"candidate package cleanup"');
    const migrationCleanup = cleanup.indexOf('"database migration cleanup"');
    const backupCleanup = cleanup.indexOf('"migration safety backup cleanup"');
    assert.ok(candidateCleanup >= 0);
    assert.ok(packageCleanup > candidateCleanup);
    assert.ok(migrationCleanup > packageCleanup);
    assert.ok(backupCleanup > migrationCleanup);

    const removeStart = recovery.indexOf("fn remove_candidate_artifact");
    const removeEnd = recovery.indexOf("\nfn remove_failed_bundle_marker", removeStart);
    const remove = recovery.slice(removeStart, removeEnd);
    assert.match(remove, /validate_candidate_artifact_path\(path, root, label\)/);
    assert.doesNotMatch(remove, /validate_no_symlink_components\(path, label\)/);

    const pathValidationStart = recovery.indexOf("fn validate_candidate_artifact_path");
    const pathValidationEnd = recovery.indexOf("\nfn canonical_managed_root", pathValidationStart);
    const pathValidation = recovery.slice(pathValidationStart, pathValidationEnd);
    assert.match(pathValidation, /canonical_managed_root\(root, label\)/);
    assert.match(pathValidation, /fs::symlink_metadata\(&current\)/);
    assert.match(pathValidation, /NotFound[^\n]*=> return Ok\(None\)/);

    const finishStart = recovery.indexOf("fn finish_cleanup");
    const finishEnd = recovery.indexOf("\nfn recover_rollback", finishStart);
    const finish = recovery.slice(finishStart, finishEnd);
    assert.match(finish, /cleanup_after_success[\s\S]*Ok\(\(\)\)[\s\S]*complete_update/);

    const state = read("src-tauri/src/update_state.rs");
    assert.match(state, /pub\(crate\) fn complete_update[\s\S]*UpdateStatus::Available/);
  } finally {
    fixture.cleanup();
  }
});

test("production candidate health cannot select a renderer or external runtime path", () => {
  const launcher = read("src-tauri/sidecar/launcher.cjs");
  const runtimeEntry = launcher.slice(
    launcher.indexOf("function runtimeEntry"),
    launcher.indexOf("function spawnRuntime"),
  );

  assert.match(runtimeEntry, /NODE_ENV !== "production"/);
  assert.match(runtimeEntry, /CORNELL_DESKTOP_ALLOW_RUNTIME_OVERRIDE/);
  assert.match(
    runtimeEntry,
    /path\.join\(root, "node_modules", "next", "dist", "bin", "next"\)/,
  );
  assert.doesNotMatch(runtimeEntry, /path\.join\(root, "node_modules", "\.bin"/);
  assert.match(launcher, /randomBytes\(READY_NONCE_BYTES\)/);
  assert.match(launcher, /parsed\.nonce === expectedNonce/);
});

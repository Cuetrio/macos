// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-nocheck -- browser DOM fakes intentionally model only the tested surface.
import assert from "node:assert/strict";
import vm from "node:vm";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

const projectRoot = path.resolve(__dirname, "../..");
const indexPath = path.join(projectRoot, "src-tauri", "ui", "index.html");
const scriptPath = path.join(projectRoot, "src-tauri", "ui", "recovery.js");

function read(filePath) {
  return fs.readFileSync(filePath, "utf8");
}

class FakeElement {
  constructor() {
    this.disabled = false;
    this.hidden = false;
    this.textContent = "";
    this.children = [];
    this.listeners = new Map();
    this.attributes = new Map();
  }

  addEventListener(type, listener) {
    this.listeners.set(type, listener);
  }

  append(...children) {
    this.children.push(...children);
  }

  appendChild(child) {
    this.children.push(child);
  }

  replaceChildren(...children) {
    this.children = children;
  }

  setAttribute(name, value) {
    this.attributes.set(name, value);
  }

  focus() {}

  click() {
    if (!this.disabled) this.listeners.get("click")?.();
  }
}

function recoverySnapshot({ managedBackupAvailable, pendingRestoreAvailable }) {
  return {
    kind: "desktop-database-recovery-snapshot",
    schemaVersion: 1,
    status: "recovery",
    snapshot: {
      schemaVersion: 1,
      state: "restore-available",
      reasonCode: "database-missing",
      managedBackupAvailable,
      pendingRestoreAvailable,
      canStartEmpty: false,
    },
  };
}

function makeRecoveryHarness({
  managedBackupAvailable = false,
  pendingRestoreAvailable = false,
  pendingResumeResponse,
  backupOperationResponse,
  externalSourceResponse,
  managedBackups,
}) {
  const index = read(indexPath);
  const ids = [...index.matchAll(/id="([^"]+)"/g)].map((match) => match[1]);
  const elements = new Map(ids.map((id) => [id, new FakeElement()]));
  const createdElements = [];
  const calls = [];
  const pendingId = "a".repeat(64);
  const manifestToken = "b".repeat(64);
  const document = {
    activeElement: elements.get("confirm-restore"),
    getElementById(id) {
      return elements.get(id);
    },
    createElement() {
      const element = new FakeElement();
      createdElements.push(element);
      return element;
    },
  };
  const window = {
    __TAURI_INTERNALS__: {
      invoke(command, args) {
        calls.push([command, args]);
        if (command === "read_desktop_database_recovery_snapshot") {
          return recoverySnapshot({ managedBackupAvailable, pendingRestoreAvailable });
        }
        if (command === "read_desktop_managed_backup_catalog") {
          return {
            kind: "desktop-managed-backup-catalog",
            schemaVersion: 1,
            status: "ready",
            phase: "catalog",
            errorCode: null,
            backups: managedBackups ?? [
              {
                backupId: "managed-backup",
                fileName: "managed-backup",
                size: 123,
                createdAt: "2026-08-27T00:00:00.000Z",
                recoveryOnly: false,
              },
              {
                backupId: "managed-backup-two",
                fileName: "managed-backup-two",
                size: 456,
                createdAt: "2026-08-27T00:00:01.000Z",
                recoveryOnly: false,
              },
            ],
          };
        }
        if (command === "read_desktop_pending_restore_status") {
          return {
            kind: "desktop-pending-restore-status",
            schemaVersion: 1,
            status: "available",
            phase: "status",
            operationId: null,
            errorCode: null,
            pending: {
              pendingId,
              manifestToken,
              sourceKind: "managed-backup",
              createdAt: "2026-08-27T00:00:00.000Z",
              candidateDigest: "c".repeat(64),
              candidateSize: 123,
              candidateSchemaIdentity: "d".repeat(64),
            },
          };
        }
        if (command === "resume_desktop_pending_restore") return pendingResumeResponse;
        if (command === "run_desktop_data_backup_operation") return backupOperationResponse;
        if (command === "choose_data_backup_external_source_command") return externalSourceResponse;
        throw new Error(`unexpected command: ${command}`);
      },
    },
  };
  vm.runInNewContext(read(scriptPath), { document, window, Promise, Number, Date, Object, String, RegExp });
  return { calls, elements, createdElements, pendingId, manifestToken };
}

async function flushRecovery() {
  for (let index = 0; index < 8; index += 1) await Promise.resolve();
}

test("recovery asset exposes a single-purpose accessible recovery UI", () => {
  const index = read(indexPath);

  assert.match(index, /<html lang="ja">/);
  assert.match(index, /id="page-title"[^>]*>ノートを開けませんでした/);
  assert.match(index, /id="diagnostic-button"[^>]*>診断情報を書き出して終了/);
  assert.match(index, /id="recovery-panel"/);
  assert.match(index, /id="external-button"[^>]*>外部のバックアップを選ぶ/);
  assert.match(index, /id="confirm-restore"[^>]*>復元する/);
  assert.match(index, /id="cancel-restore"[^>]*>キャンセル/);
  assert.match(index, /id="exit-button"[^>]*>そのまま終了/);
  assert.match(index, /role="alert"/);
  assert.match(index, /role="status"/);
  assert.match(index, /aria-live="(assertive|polite)"/);
  assert.match(index, /aria-busy="true"/);
  assert.match(index, /<script src="\.\/recovery\.js" defer><\/script>/);
  assert.match(index, /prefers-reduced-motion/);
  assert.match(index, /min-width:320px/);
});

test("recovery script uses the existing native commands and fixed request envelopes", () => {
  const script = read(scriptPath);

  for (const command of [
    "read_desktop_database_recovery_snapshot",
    "choose_diagnostic_export_destination_command",
    "export_desktop_diagnostics",
    "read_desktop_managed_backup_catalog",
    "choose_data_backup_external_source_command",
    "run_desktop_data_backup_operation",
    "read_desktop_pending_restore_status",
    "resume_desktop_pending_restore",
    "plugin:window|close",
  ]) {
    assert.match(script, new RegExp(command.replace(/[|]/g, "\\$&")));
  }

  assert.match(script, /window\.__TAURI_INTERNALS__/);
  assert.match(script, /schemaVersion: 1, operation: "export", selectionId:/);
  assert.match(script, /schemaVersion: 1, operation: "restore", source: restoreSelection, destination: null, confirmed: true/);
  assert.match(script, /schemaVersion: 1, pendingId: restoreSelection\.pendingId, manifestToken: restoreSelection\.manifestToken, confirmed: true/);
  assert.match(script, /label: "primary"/);
  assert.match(script, /textContent = backup\.fileName/);
  assert.match(script, /textContent = .*backup\.createdAt\.slice/);
});

test("diagnostic and restore safety boundaries are represented in the implementation", () => {
  const script = read(scriptPath);
  const diagnosticStart = script.indexOf("function exportDiagnostics");
  const diagnosticEnd = script.indexOf("function chooseExternalSource", diagnosticStart);
  const restoreStart = script.indexOf("function restoreSelected");
  const restoreEnd = script.indexOf("diagnosticButton.addEventListener", restoreStart);
  assert.ok(diagnosticStart >= 0 && diagnosticEnd > diagnosticStart);
  assert.ok(restoreStart >= 0 && restoreEnd > restoreStart);

  const diagnostic = script.slice(diagnosticStart, diagnosticEnd);
  const restore = script.slice(restoreStart, restoreEnd);
  assert.match(diagnostic, /destination\.selection\.selectionId/);
  assert.match(diagnostic, /destination\.status !== "selected"/);
  assert.match(script, /source\.status !== "selected"/);
  assert.doesNotMatch(diagnostic, /destination\.selection\.path|filePath|database/);
  assert.match(diagnostic, /result\.result\.fileName/);
  assert.match(diagnostic, /return closeWindow\(\)/);
  assert.match(diagnostic, /showMessage\(diagnosticErrorCopy/);
  assert.match(restore, /confirmed: true/);
  assert.match(restore, /if \(restoreSelection\.kind === "pending-restore"\)/);
  assert.doesNotMatch(restore, /closeWindow\(/);
  assert.match(script, /snapshot\.state === "first-run"/);
  assert.match(script, /snapshot\.canStartEmpty !== false/);
  assert.doesNotMatch(script, /create(?:Empty|.*Database)|startEmpty|空のデータベース/);
});

test("recovery UI does not add a remote dependency or expose private data", () => {
  const source = read(indexPath) + "\n" + read(scriptPath);

  assert.doesNotMatch(source, /<script[^>]+(?:https?:|\/\/)/i);
  assert.doesNotMatch(source, /https?:\/\//i);
  assert.doesNotMatch(source, /(?:^|\s)(?:import|export)\s/);
  assert.doesNotMatch(source, /\.\.\/|\.\/\.\//);
  assert.doesNotMatch(source, /fetch\(|XMLHttpRequest|WebSocket|localStorage|sessionStorage/);
  assert.doesNotMatch(source, /(?:innerHTML|outerHTML|insertAdjacentHTML)/);
  assert.doesNotMatch(source, /console\.(?:log|error|warn)|JSON\.stringify/);
  assert.doesNotMatch(source, /(?:error|exception)\.(?:message|stack)/i);
  assert.doesNotMatch(source, /note(?:Title|Body|Content)|\bcue\b|summary|canvasJson/i);
  assert.match(source, /pendingRestore = \{ pendingId: status\.pending\.pendingId, manifestToken: status\.pending\.manifestToken \}/);
  assert.doesNotMatch(source, /textContent\s*=\s*[^;]*(?:pendingId|manifestToken)/);
});

test("pending restore confirmation resumes once and accepts the native success result", async () => {
  const pendingId = "a".repeat(64);
  const harness = makeRecoveryHarness({
    pendingRestoreAvailable: true,
    pendingResumeResponse: {
      kind: "desktop-pending-restore-resume",
      schemaVersion: 1,
      ok: true,
      status: "success",
      phase: "complete",
      operationId: "pending-operation",
      pendingId,
      errorCode: null,
      result: { safetyBackupId: null, size: 123 },
    },
  });

  await flushRecovery();
  harness.elements.get("pending-button").click();
  harness.elements.get("confirm-restore").click();
  await flushRecovery();

  const restoreCalls = harness.calls.filter(([command]) =>
    ["run_desktop_data_backup_operation", "resume_desktop_pending_restore"].includes(command),
  );
  assert.deepEqual(JSON.parse(JSON.stringify(restoreCalls)), [[
    "resume_desktop_pending_restore",
    {
      request: {
        schemaVersion: 1,
        pendingId,
        manifestToken: "b".repeat(64),
        confirmed: true,
      },
    },
  ]]);
  assert.equal(harness.elements.get("restore-status").textContent, "復元が完了しました。ノートを開いています…");
  assert.equal(harness.elements.get("confirm-restore").disabled, true);
});

test("invalid pending restore success responses fail closed and remain retryable", async () => {
  const harness = makeRecoveryHarness({
    pendingRestoreAvailable: true,
    pendingResumeResponse: {
      kind: "desktop-pending-restore-resume",
      schemaVersion: 1,
      ok: true,
      status: "success",
      phase: "complete",
      operationId: "pending-operation",
      pendingId: "a".repeat(64),
      errorCode: null,
      result: null,
    },
  });

  await flushRecovery();
  harness.elements.get("pending-button").click();
  harness.elements.get("confirm-restore").click();
  await flushRecovery();

  assert.equal(harness.elements.get("restore-status").textContent, "バックアップを復元できませんでした。別のバックアップを選ぶか、少し待ってから再試行してください。");
  assert.equal(harness.elements.get("confirm-restore").disabled, false);
});

test("managed and external restore confirmations keep their existing request envelope", async () => {
  const operationResponse = {
    kind: "desktop-data-backup-operation",
    schemaVersion: 1,
    ok: true,
    status: "success",
    operation: "restore",
    phase: "complete",
    errorCode: null,
    result: null,
  };
  const managed = makeRecoveryHarness({
    managedBackupAvailable: true,
    backupOperationResponse: operationResponse,
  });
  await flushRecovery();
  managed.createdElements.find((element) => element.textContent === "このバックアップを選ぶ").click();
  managed.elements.get("confirm-restore").click();
  await flushRecovery();

  const externalSelection = "e".repeat(64);
  const external = makeRecoveryHarness({
    managedBackupAvailable: true,
    externalSourceResponse: {
      kind: "desktop-file-dialog",
      schemaVersion: 1,
      dialog: "open-external-source",
      ok: true,
      status: "selected",
      phase: "dialog",
      selection: { kind: "external-file", selectionId: externalSelection, fileName: "external.sqlite" },
      errorCode: null,
    },
    backupOperationResponse: operationResponse,
  });
  await flushRecovery();
  external.elements.get("external-button").click();
  await flushRecovery();
  external.elements.get("confirm-restore").click();
  await flushRecovery();

  const managedRestore = managed.calls.find(([command]) => command === "run_desktop_data_backup_operation");
  assert.deepEqual(JSON.parse(JSON.stringify(managedRestore)), ["run_desktop_data_backup_operation", {
    request: {
      schemaVersion: 1,
      operation: "restore",
      source: { kind: "managed-backup", backupId: "managed-backup" },
      destination: null,
      confirmed: true,
    },
  }]);
  const externalRestore = external.calls.find(([command]) => command === "run_desktop_data_backup_operation");
  assert.deepEqual(JSON.parse(JSON.stringify(externalRestore)), ["run_desktop_data_backup_operation", {
    request: {
      schemaVersion: 1,
      operation: "restore",
      source: { kind: "external-selection", selectionId: externalSelection },
      destination: null,
      confirmed: true,
    },
  }]);
});

test("recovery UI keeps recovery-only catalog entries available for internal restore", async () => {
  const operationResponse = {
    kind: "desktop-data-backup-operation",
    schemaVersion: 1,
    ok: true,
    status: "success",
    operation: "restore",
    phase: "complete",
    errorCode: null,
    result: null,
  };
  const harness = makeRecoveryHarness({
    managedBackupAvailable: true,
    managedBackups: [{
      backupId: "restore-internal.sqlite.bak",
      fileName: "restore-internal.sqlite.bak",
      size: 123,
      createdAt: "2026-08-27T00:00:00.000Z",
      recoveryOnly: true,
    }],
    backupOperationResponse: operationResponse,
  });

  await flushRecovery();
  harness.createdElements.find((element) => element.textContent === "このバックアップを選ぶ").click();
  harness.elements.get("confirm-restore").click();
  await flushRecovery();

  const restoreCall = harness.calls.find(([command]) => command === "run_desktop_data_backup_operation");
  assert.equal(restoreCall[1].request.source.backupId, "restore-internal.sqlite.bak");
});

test("managed backup selection stays unchanged while restore is in flight", async () => {
  let resolveRestore;
  const restoreResponse = new Promise((resolve) => {
    resolveRestore = resolve;
  });
  const harness = makeRecoveryHarness({
    managedBackupAvailable: true,
    backupOperationResponse: restoreResponse,
  });

  await flushRecovery();
  const selections = harness.createdElements.filter((element) => element.textContent === "このバックアップを選ぶ");
  selections[0].click();
  const firstConfirmation = harness.elements.get("confirmation-copy").textContent;
  harness.elements.get("confirm-restore").click();
  await flushRecovery();

  assert.equal(selections[1].disabled, true);
  selections[1].click();
  assert.equal(harness.elements.get("confirmation-copy").textContent, firstConfirmation);
  assert.equal(harness.calls.filter(([command]) => command === "run_desktop_data_backup_operation").length, 1);

  resolveRestore({
    kind: "desktop-data-backup-operation",
    schemaVersion: 1,
    ok: true,
    status: "success",
    operation: "restore",
    phase: "complete",
    errorCode: null,
    result: null,
  });
  await flushRecovery();
});
// @ts-nocheck -- browser DOM fakes intentionally model only the tested surface.

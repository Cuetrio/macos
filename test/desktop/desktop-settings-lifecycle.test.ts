import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { test } from "node:test";
import { setImmediate } from "node:timers/promises";
import ts from "typescript";
import type {
  DesktopManagedBackupCatalogResponse,
  DesktopPendingRestoreStatusResponse,
} from "../../src/shared/desktop/desktop-settings-bridge";

const projectRoot = path.resolve(__dirname, "../..");
type EffectSetup = () => void | (() => void);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function loadDataAndBackupPanel() {
  const catalog = deferred<DesktopManagedBackupCatalogResponse>();
  const pending = deferred<DesktopPendingRestoreStatusResponse>();
  const requests: string[] = [];
  const states: Array<{ value: unknown }> = [];
  const stateUpdates: unknown[] = [];
  const effects: EffectSetup[] = [];
  let cleanups: Array<() => void> = [];
  const react = {
    useState(initial: unknown) {
      const state = {
        value: typeof initial === "function" ? initial() : initial,
      };
      states.push(state);
      return [state.value, (next: unknown) => {
        state.value = typeof next === "function" ? next(state.value) : next;
        stateUpdates.push(state.value);
      }];
    },
    useRef(initial: unknown) {
      return { current: initial };
    },
    useCallback<T>(callback: T) {
      return callback;
    },
    useEffect(effect: EffectSetup) {
      effects.push(effect);
    },
  };
  const source = readFileSync(
    path.join(projectRoot, "src/app/_components/settings/settings-modal.tsx"),
    "utf8",
  );
  const output = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
      jsx: ts.JsxEmit.ReactJSX,
      esModuleInterop: true,
    },
  }).outputText;
  const dependencies: Record<string, unknown> = {
    react,
    "react/jsx-runtime": { jsx: () => null, jsxs: () => null },
    "react-dom": {},
    "@/shared/desktop/desktop-settings-bridge": {
      requestManagedBackupCatalog() {
        requests.push("catalog");
        return catalog.promise;
      },
      requestPendingRestoreStatus() {
        requests.push("pending");
        return pending.promise;
      },
    },
    "../app-chrome-parts": {},
    "../theme/theme-provider": {},
    "../theme/theme": {},
    "./settings-modal.module.css": {},
  };
  const moduleObject = { exports: {} as Record<string, unknown> };
  // Expose the existing private component only inside this test's VM.
  vm.runInNewContext(`${output}\nexports.DataAndBackupPanel = DataAndBackupPanel;`, {
    module: moduleObject,
    exports: moduleObject.exports,
    require(request: string) {
      assert.ok(Object.prototype.hasOwnProperty.call(dependencies, request),
        `Unexpected dependency: ${request}`);
      return dependencies[request];
    },
  });
  (moduleObject.exports.DataAndBackupPanel as () => unknown)();

  return {
    catalog,
    pending,
    requests,
    stateUpdates,
    get catalogState() {
      return states[0].value as {
        phase: string;
        backups: DesktopManagedBackupCatalogResponse["backups"];
      };
    },
    get pendingState() {
      return states[1].value as {
        phase: string;
        pending: DesktopPendingRestoreStatusResponse["pending"];
      };
    },
    // Initial effect replay preserves this render's state/ref cells and callbacks.
    setup() {
      assert.equal(cleanups.length, 0);
      for (const effect of effects) {
        const cleanup = effect();
        if (typeof cleanup === "function") {
          cleanups.push(cleanup);
        }
      }
    },
    cleanup() {
      for (const cleanup of cleanups) {
        cleanup();
      }
      cleanups = [];
    },
  };
}

const catalogResult: DesktopManagedBackupCatalogResponse = {
  kind: "desktop-managed-backup-catalog",
  schemaVersion: 1,
  status: "ready",
  phase: "catalog",
  errorCode: null,
  backups: [{
    backupId: "backup-1",
    fileName: "backup.sqlite",
    size: 1024,
    createdAt: "2026-10-24T00:00:00Z",
    recoveryOnly: false,
  }],
};
const pendingResult: DesktopPendingRestoreStatusResponse = {
  kind: "desktop-pending-restore-status",
  schemaVersion: 1,
  status: "available",
  phase: "status",
  operationId: null,
  errorCode: null,
  pending: {
    pendingId: "pending-1",
    manifestToken: "test-manifest",
    sourceKind: "external-file",
    createdAt: "2026-10-24T00:00:00Z",
    candidateDigest: "test-digest",
    candidateSize: 2048,
    candidateSchemaIdentity: "test-schema",
  },
};

test("Data and Backup accepts delayed reads after effect replay without duplicate bridge requests", async () => {
  const panel = loadDataAndBackupPanel();
  panel.setup();
  assert.deepEqual(panel.requests, ["catalog", "pending"]);
  panel.cleanup();
  panel.setup();

  assert.deepEqual(panel.requests, ["catalog", "pending"]);
  assert.equal(panel.catalogState.phase, "loading");
  assert.equal(panel.pendingState.phase, "loading");

  panel.catalog.resolve(catalogResult);
  await panel.catalog.promise;
  // Let the VM's async continuations finish before inspecting state.
  await setImmediate();
  assert.equal(panel.catalogState.phase, "ready");
  assert.equal(panel.catalogState.backups, catalogResult.backups);
  assert.equal(panel.pendingState.phase, "loading");

  panel.pending.resolve(pendingResult);
  await panel.pending.promise;
  await setImmediate();
  assert.equal(panel.pendingState.phase, "available");
  assert.equal(panel.pendingState.pending, pendingResult.pending);
  assert.deepEqual(panel.requests, ["catalog", "pending"]);
  panel.cleanup();
});

test("Data and Backup suppresses both late read results after true unmount following replay", async () => {
  const panel = loadDataAndBackupPanel();
  panel.setup();
  panel.cleanup();
  panel.setup();
  assert.deepEqual(panel.requests, ["catalog", "pending"]);
  panel.cleanup();
  const updatesBeforeUnmount = panel.stateUpdates.length;

  panel.pending.resolve(pendingResult);
  await panel.pending.promise;
  await setImmediate();
  assert.equal(panel.stateUpdates.length, updatesBeforeUnmount);
  assert.equal(panel.pendingState.phase, "loading");

  panel.catalog.resolve(catalogResult);
  await panel.catalog.promise;
  await setImmediate();
  assert.equal(panel.stateUpdates.length, updatesBeforeUnmount);
  assert.equal(panel.catalogState.phase, "loading");
  assert.deepEqual(panel.requests, ["catalog", "pending"]);
});

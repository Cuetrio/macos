const fs = require("node:fs");
const path = require("node:path");
const {
  CANDIDATE_ROOT,
  EXPECTED_BASELINE,
  actualToolchain,
  ensureOutputDirectories,
  getContext,
  readJson,
  relativeOutputPath,
  revisionProvenance,
  writeFailureSummary,
  writeJsonOwned,
} = require("./common.cjs");

function readIf(filePath) {
  try { return readJson(filePath); } catch { return null; }
}

function statusOf(report) {
  return report?.status ?? "UNVERIFIED";
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isMeasured(value) {
  return isObject(value) && (value.status === "PASS" || value.status === "FAIL");
}

function reportPath(context, fileName) {
  return relativeOutputPath(context, path.join(context.evidenceRoot, fileName));
}

function addProvenance(measurement, provenance) {
  return {
    ...(isObject(measurement) ? measurement : { status: "UNVERIFIED" }),
    source: provenance.source,
    provenance,
  };
}

function selectOperationResponse(context, smoke, runtimeHttp) {
  const candidates = [
    {
      source: "smoke",
      report: "smoke.json",
      measurement: smoke?.operations,
      measurementType: smoke?.uiSmoke?.status === "PASS"
        ? "renderer-webview-ui-operation-response"
        : "native-shell-smoke-operation-response",
      scope: smoke?.uiSmoke?.status === "PASS" ? "renderer/WebView UI" : "Tauri native shell smoke",
      rendererWebViewUi: smoke?.uiSmoke?.status === "PASS",
      fallbackUsed: false,
    },
    {
      source: "runtime-http",
      report: "runtime-http-smoke.json",
      measurement: runtimeHttp?.operations,
      measurementType: "production-runtime-http-api-operation-response",
      scope: "production Next.js runtime loopback HTTP/API",
      rendererWebViewUi: false,
      fallbackUsed: !isMeasured(smoke?.operations),
    },
  ];
  const selected = isMeasured(candidates[0].measurement)
    ? candidates[0]
    : isObject(candidates[1].measurement)
      ? candidates[1]
      : isObject(candidates[0].measurement)
        ? candidates[0]
        : null;
  if (!selected) {
    return addProvenance(
      { status: "UNVERIFIED", reason: "smoke and runtime HTTP operation reports are unavailable" },
      {
        source: "none",
        report: null,
        measurementType: "operation-response",
        scope: "not measured",
        rendererWebViewUi: false,
        fallbackUsed: false,
      },
    );
  }
  return addProvenance(selected.measurement, {
    source: selected.source,
    report: reportPath(context, selected.report),
    measurementType: selected.measurementType,
    scope: selected.scope,
    rendererWebViewUi: selected.rendererWebViewUi,
    fallbackUsed: selected.fallbackUsed,
  });
}

function uiSmokeMeasurement(context, smoke) {
  const measurement = smoke?.uiSmoke ?? (smoke
    ? { status: statusOf(smoke), reason: "smoke report did not include a uiSmoke result" }
    : { status: "UNVERIFIED", reason: "smoke report unavailable" });
  return addProvenance(measurement, {
    source: smoke ? "smoke" : "none",
    report: smoke ? reportPath(context, "smoke.json") : null,
    measurementType: "renderer-webview-ui-smoke",
    scope: "renderer/WebView UI",
    rendererWebViewUi: true,
  });
}

function nativeShellCleanupMeasurement(context, smoke, lifecycle) {
  const candidates = [
    {
      source: "smoke",
      report: "smoke.json",
      measurement: smoke?.cleanup,
      measurementType: "native-shell-cleanup",
      scope: "Tauri native shell process tree and loopback shutdown",
    },
    {
      source: "lifecycle",
      report: "lifecycle.json",
      measurement: lifecycle?.shutdown?.shellCleanup ?? lifecycle?.shellCleanup,
      measurementType: "native-shell-lifecycle-cleanup",
      scope: "Tauri native shell lifecycle process tree",
    },
  ];
  const selected = candidates.find((candidate) => isMeasured(candidate.measurement))
    ?? candidates.find((candidate) => isObject(candidate.measurement));
  if (!selected) {
    return addProvenance(
      {
        status: "UNVERIFIED",
        reason: "native shell cleanup requires smoke.json cleanup or lifecycle.json shellCleanup evidence",
      },
      {
        source: "none",
        report: null,
        measurementType: "native-shell-cleanup",
        scope: "Tauri native shell lifecycle",
        runtimeHttpSidecar: false,
      },
    );
  }
  return addProvenance(selected.measurement, {
    source: selected.source,
    report: reportPath(context, selected.report),
    measurementType: selected.measurementType,
    scope: selected.scope,
    runtimeHttpSidecar: false,
  });
}

function runtimeReadinessMeasurement(context, smoke, runtimeHttp) {
  const runtimeHttpReadiness = runtimeHttp?.runtime?.readinessMs;
  if (runtimeHttpReadiness != null) {
    return addProvenance(
      {
        status: runtimeHttp.runtime.status === "PASS"
          ? "PASS"
          : runtimeHttp.runtime.status === "FAIL" ? "FAIL" : "UNVERIFIED",
        readinessMs: runtimeHttpReadiness,
      },
      {
        source: "runtime-http",
        report: reportPath(context, "runtime-http-smoke.json"),
        measurementType: "production-runtime-readiness",
        scope: "production Next.js runtime loopback readiness",
        primaryWindowUsable: false,
      },
    );
  }
  const shellReadiness = smoke?.coldStart?.processLaunchToRuntimeReadyMs;
  if (shellReadiness != null) {
    return addProvenance(
      {
        status: "PASS",
        readinessMs: shellReadiness,
      },
      {
        source: "smoke",
        report: reportPath(context, "smoke.json"),
        measurementType: "native-shell-runtime-readiness",
        scope: "Tauri shell-launched runtime readiness",
        primaryWindowUsable: false,
      },
    );
  }
  return addProvenance(
    { status: "UNVERIFIED", reason: "runtime readiness was not measured" },
    {
      source: "none",
      report: null,
      measurementType: "runtime-readiness",
      scope: "runtime readiness separate from primary window usability",
      primaryWindowUsable: false,
    },
  );
}

function coldStartMeasurement(context, smoke, runtimeHttp) {
  const coldStart = smoke?.coldStart ?? {
    status: "UNVERIFIED",
    reason: "smoke report unavailable; primary window usability was not measured",
  };
  const primaryWindowUsableMs = coldStart.processLaunchToPrimaryWindowUsableMs;
  const primaryWindow = smoke?.primaryWindow;
  const primaryWindowReadinessVerified = primaryWindow?.usableStatus === "PASS"
    && primaryWindow?.usableObservationComplete === true
    && primaryWindow?.usablePageLoadEvent === "Finished"
    && primaryWindowUsableMs != null;
  const primaryWindowUsable = addProvenance(
    {
      status: primaryWindowReadinessVerified
        ? "PASS"
        : primaryWindow?.usableStatus === "BLOCKED" ? "BLOCKED" : "UNVERIFIED",
      durationMs: primaryWindowReadinessVerified ? primaryWindowUsableMs : null,
      reason: primaryWindowReadinessVerified
        ? undefined
        : "Tauri /notes PageLoadEvent::Finished evidence was not verified",
    },
    {
      source: smoke ? "smoke" : "none",
      report: smoke ? reportPath(context, "smoke.json") : null,
      measurementType: "primary-window-usable",
      scope: "Tauri primary window usability",
      primaryWindowUsable: true,
    },
  );
  const runtimeReadiness = runtimeReadinessMeasurement(context, smoke, runtimeHttp);
  return {
    ...coldStart,
    primaryWindowUsable,
    runtimeReadiness,
    provenance: {
      primaryWindowUsable: primaryWindowUsable.provenance,
      runtimeReadiness: runtimeReadiness.provenance,
    },
  };
}

function productionRuntimeHttpMeasurement(context, runtimeHttp) {
  if (!runtimeHttp) {
    return addProvenance(
      { status: "UNVERIFIED", reason: "runtime HTTP report unavailable" },
      {
        source: "none",
        report: null,
        measurementType: "production-runtime-http",
        scope: "production Next.js runtime loopback HTTP/API and sidecar cleanup",
        rendererWebViewUi: false,
      },
    );
  }
  const measurement = { ...runtimeHttp };
  if (runtimeHttp.cleanup != null) {
    measurement.cleanup = addProvenance(runtimeHttp.cleanup, {
      source: "runtime-http",
      report: reportPath(context, "runtime-http-smoke.json"),
      measurementType: "runtime-http-sidecar-cleanup",
      scope: "Node.js sidecar process tree and loopback shutdown",
      rendererWebViewUi: false,
      nativeShell: false,
    });
  }
  return addProvenance(measurement, {
    source: "runtime-http",
    report: reportPath(context, "runtime-http-smoke.json"),
    measurementType: "production-runtime-http",
    scope: "production Next.js runtime loopback HTTP/API and sidecar cleanup",
    rendererWebViewUi: false,
    nativeShell: false,
  });
}

function overallStatus(statuses) {
  if (statuses.includes("FAIL")) return "FAIL";
  if (statuses.includes("BLOCKED")) return "BLOCKED";
  if (statuses.includes("UNVERIFIED")) return "UNVERIFIED";
  return "PASS";
}

function cargoMetadata() {
  const cargoTomlPath = path.join(CANDIDATE_ROOT, "src-tauri", "Cargo.toml");
  const cargoLockPath = path.join(CANDIDATE_ROOT, "src-tauri", "Cargo.lock");
  const cargoToml = fs.existsSync(cargoTomlPath) ? fs.readFileSync(cargoTomlPath, "utf8") : "";
  return {
    manifest: cargoTomlPath,
    lockfile: fs.existsSync(cargoLockPath) ? cargoLockPath : null,
    lockfileStatus: fs.existsSync(cargoLockPath) ? "present" : "not-created-dependency-resolution-blocked",
    tauri: cargoToml.match(/^tauri\s*=\s*"=([^\"]+)"/m)?.[1] ?? null,
    tauriBuild: cargoToml.match(/^tauri-build\s*=\s*"=([^\"]+)"/m)?.[1] ?? null,
    plugin: "none",
  };
}

function buildManifest(context) {
  const baselineValidationFile = readIf(path.join(context.evidenceRoot, "baseline-validation.json"));
  const toolchain = actualToolchain();
  const currentRevisionProvenance = revisionProvenance(context.baseline, toolchain);
  const baselineValidation = baselineValidationFile
    ? {
      ...baselineValidationFile,
      observed: toolchain,
      revisionProvenance: currentRevisionProvenance,
    }
    : {
      status: "UNVERIFIED",
      reason: "baseline-validation.json unavailable",
      observed: toolchain,
      revisionProvenance: currentRevisionProvenance,
    };
  const preparation = readIf(path.join(context.evidenceRoot, "preparation.json"));
  const build = readIf(path.join(context.evidenceRoot, "build.json"));
  const runtimeHttp = readIf(path.join(context.evidenceRoot, "runtime-http-smoke.json"));
  const smoke = readIf(path.join(context.evidenceRoot, "smoke.json"));
  const lifecycle = readIf(path.join(context.evidenceRoot, "lifecycle.json"));
  const packaging = readIf(path.join(context.evidenceRoot, "package.json"));
  const updateManifest = readIf(path.join(context.evidenceRoot, "update-manifest.json"));
  const uiSmoke = uiSmokeMeasurement(context, smoke);
  const operationResponse = selectOperationResponse(context, smoke, runtimeHttp);
  const nativeCleanup = nativeShellCleanupMeasurement(context, smoke, lifecycle);
  const coldStart = coldStartMeasurement(context, smoke, runtimeHttp);
  const instanceRecovery = smoke?.instanceRecovery
    ?? lifecycle?.singleApplicationInstance?.instanceRecovery
    ?? null;
  const statuses = [
    baselineValidation,
    preparation,
    build,
    runtimeHttp,
    smoke,
    lifecycle,
    packaging,
    coldStart,
    coldStart.primaryWindowUsable,
    coldStart.runtimeReadiness,
    operationResponse,
    uiSmoke,
    nativeCleanup,
  ].map(statusOf);
  const baseline = baselineValidation?.status === "PASS"
    ? {
      baselineId: context.baseline.baseline_id,
      gitHead: context.baseline.git_head,
      baselineScopeSha256: context.baseline.baseline_scope_sha256,
      fixtureCount: context.baseline.fixture_count,
      fixtureSeed: context.baseline.fixture_seed,
      fixtureSha256: context.baseline.fixture_sha256,
      fixtureContentHash: context.baseline.fixture_content_hash,
      observedToolchain: baselineValidation.observed,
      fixtureReadBack: baselineValidation.fixtureReadBack,
    }
    : {
      baselineId: context.baseline.baseline_id ?? EXPECTED_BASELINE.baseline_id,
      gitHead: context.baseline.git_head ?? EXPECTED_BASELINE.git_head,
      baselineScopeSha256: context.baseline.baseline_scope_sha256 ?? EXPECTED_BASELINE.baseline_scope_sha256,
      fixtureCount: context.baseline.fixture_count ?? EXPECTED_BASELINE.fixture_count,
      fixtureSeed: context.baseline.fixture_seed ?? EXPECTED_BASELINE.fixture_seed,
      fixtureSha256: context.baseline.fixture_sha256 ?? EXPECTED_BASELINE.fixture_sha256,
      fixtureContentHash: context.baseline.fixture_content_hash ?? EXPECTED_BASELINE.fixture_content_hash,
      observedToolchain: toolchain,
      validationStatus: statusOf(baselineValidation),
    };
  const findings = [
    {
      id: "TAURI-001",
      status: packaging?.status === "PASS" ? "observed" : "BLOCKED",
      message: packaging?.status === "PASS" ? "Tauri .app/DMG packaging completed in the candidate output." : "Tauri native packaging is not verified because cargo tauri or Cargo dependency resolution is unavailable.",
    },
    {
      id: "TAURI-002",
      status: smoke?.uiSmoke?.status === "PASS" ? "observed" : "UNVERIFIED",
      message: smoke?.uiSmoke?.status === "PASS" ? "Renderer/WebView UI smoke completed." : "Renderer/WebView UI automation is separate from production runtime HTTP smoke and remains unverified.",
    },
    {
      id: "TAURI-003",
      status: "risk",
      message: "The unpackaged PoC launches the host Node binary; a complete distributable Node/native dependency tree is not claimed.",
    },
    {
      id: "TAURI-004",
      status: "boundary",
      message: "Developer ID, notarization, public distribution, and product updater implementation are outside this PoC.",
    },
  ];
  return {
    schemaVersion: 1,
    candidate: "tauri-node-sidecar",
    status: overallStatus(statuses),
    revisionProvenance: currentRevisionProvenance,
    baseline,
    candidateDependencies: {
      packageJson: path.join(CANDIDATE_ROOT, "package.json"),
      packageLock: path.join(CANDIDATE_ROOT, "package-lock.json"),
      packageLockStatus: fs.existsSync(path.join(CANDIDATE_ROOT, "package-lock.json")) ? "present" : "missing",
      nodeRuntime: "host Node v26.7.0 for unpackaged sidecar",
      rootNodeModulesUsedByRuntime: false,
      rust: cargoMetadata(),
    },
    conditions: {
      buildMode: "production Next.js runtime from the same webpack build, served on loopback",
      cacheState: build?.nextBuild?.cacheState ?? "UNVERIFIED",
      measurementStartCondition: "candidate launcher spawn with a clean candidate run directory; populated DB starts as byte-identical fixture",
      repetitions: { coldStart: 1, operationResponse: 1, memorySnapshot: 1, lifecycle: 1, packaging: 1 },
      thresholds: "none introduced by this PoC",
      runtime: {
        host: context.runtimeHost,
        port: context.runtimePort,
        portPolicy: "fixed candidate port; collision fails explicitly; no fallback",
        databaseUrl: `file:${context.populatedDatabasePath}`,
        databasePath: context.populatedDatabasePath,
        stagingPath: context.stagingRoot,
        appBundleDatabase: false,
      },
      userData: {
        cleanPath: context.cleanUserDataRoot,
        populatedPath: context.populatedUserDataRoot,
        liveDatabaseOutsideBundle: true,
        noteBodyIncludedInEvidence: false,
        queryIncludedInEvidence: false,
      },
    },
    measurements: {
      coldStart,
      primaryWindow: smoke?.primaryWindow ?? {
        count: 0,
        created: false,
        usableStatus: "UNVERIFIED",
        usableObservationComplete: false,
        reason: "native shell smoke report unavailable",
      },
      operationResponse,
      memory: smoke?.memory ?? { status: "UNVERIFIED", reason: "native shell smoke unavailable" },
      cleanup: nativeCleanup,
      shutdown: smoke?.shutdown ?? lifecycle?.shutdown ?? { status: "UNVERIFIED" },
      uiSmoke,
      instanceRecovery,
      productionRuntimeHttp: productionRuntimeHttpMeasurement(context, runtimeHttp),
      sourceReports: {
        build: build ? relativeOutputPath(context, path.join(context.evidenceRoot, "build.json")) : null,
        smoke: smoke ? relativeOutputPath(context, path.join(context.evidenceRoot, "smoke.json")) : null,
        runtimeHttp: runtimeHttp ? relativeOutputPath(context, path.join(context.evidenceRoot, "runtime-http-smoke.json")) : null,
      },
    },
    lifecycle: lifecycle ?? {
      status: "UNVERIFIED",
      singleApplicationInstance: "UNVERIFIED",
      primaryWindow: "UNVERIFIED",
      shutdownCleanup: "UNVERIFIED",
    },
    persistence: {
      status: preparation?.status === "PASS" && runtimeHttp?.persistence?.status === "PASS" ? "PASS" : overallStatus([statusOf(preparation), statusOf(runtimeHttp?.persistence)]),
      clean: preparation?.userData?.clean ?? { status: "UNVERIFIED" },
      populated: runtimeHttp?.persistence ?? { status: "UNVERIFIED", reason: "runtime HTTP report unavailable" },
      migrationContract: "existing Prisma SQLite migrations are applied only to candidate clean user-data; shared fixture is copied byte-for-byte",
      reopenContract: runtimeHttp?.operations?.reopen ?? { status: "UNVERIFIED" },
    },
    artifacts: packaging ?? { status: "UNVERIFIED", reason: "packaging report unavailable", developerIdRequired: false, notarizationRequired: false },
    updateOutlook: {
      status: updateManifest?.status ?? "UNVERIFIED",
      manifest: updateManifest,
      boundary: "static local metadata records artifact type, architecture, size, SHA-256, and explicit-restart application; background download and signature verification remain future work",
    },
    verification: {
      candidateSyntax: "npm run syntax",
      candidateTests: "npm test",
      baselineValidation: "npm run poc:validate",
      diffCheck: "git diff --check",
      rootLint: "not run by candidate task constraint",
      rootBuild: "not run by candidate task constraint",
      rootPackageAndLockfile: "not modified",
      rootSourcePrismaDbAndNextOutput: "not modified by candidate scripts",
    },
    findings,
    risks: [
      "Electron and Tauri have not been compared in a selection task; this manifest does not select a shell.",
      "Cargo.lock is absent until Cargo actually resolves the exact Tauri dependencies; no fake lockfile is accepted.",
      "Only one trial is specified; no threshold or candidate ranking is introduced.",
    ],
    reports: {
      baselineValidation: baselineValidation ? relativeOutputPath(context, path.join(context.evidenceRoot, "baseline-validation.json")) : null,
      preparation: preparation ? relativeOutputPath(context, path.join(context.evidenceRoot, "preparation.json")) : null,
      build: build ? relativeOutputPath(context, path.join(context.evidenceRoot, "build.json")) : null,
      smoke: smoke ? relativeOutputPath(context, path.join(context.evidenceRoot, "smoke.json")) : null,
      lifecycle: lifecycle ? relativeOutputPath(context, path.join(context.evidenceRoot, "lifecycle.json")) : null,
      packaging: packaging ? relativeOutputPath(context, path.join(context.evidenceRoot, "package.json")) : null,
      updateManifest: updateManifest ? relativeOutputPath(context, path.join(context.evidenceRoot, "update-manifest.json")) : null,
      runtimeHttp: runtimeHttp ? relativeOutputPath(context, path.join(context.evidenceRoot, "runtime-http-smoke.json")) : null,
    },
    measuredAt: new Date().toISOString(),
  };
}

function run() {
  const context = getContext();
  ensureOutputDirectories(context);
  try {
    const manifest = buildManifest(context);
    const canonicalPath = path.join(context.evidenceRoot, "tauri-evidence-manifest.json");
    try {
      writeJsonOwned(canonicalPath, manifest);
    } catch (error) {
      if (!fs.existsSync(canonicalPath)) throw error;
      writeJsonOwned(path.join(context.evidenceRoot, `tauri-evidence-manifest-retry-${Date.now()}.json`), {
        ...manifest,
        supersedes: path.basename(canonicalPath),
      });
    }
    console.log(JSON.stringify({ status: manifest.status, evidence: path.join(context.evidenceRoot, "tauri-evidence-manifest.json") }));
    return manifest;
  } catch (error) {
    const failurePath = writeFailureSummary(context, "evidence", error);
    console.error(`${error instanceof Error ? error.message : String(error)}\nsummary: ${failurePath}`);
    return { status: "BLOCKED", failurePath };
  }
}

if (require.main === module) {
  try { run(); } catch { process.exitCode = 1; }
}

module.exports = {
  buildManifest,
  coldStartMeasurement,
  nativeShellCleanupMeasurement,
  overallStatus,
  productionRuntimeHttpMeasurement,
  run,
  selectOperationResponse,
  statusOf,
};

require("node:fs");
const path = require("node:path");
const {
  CANDIDATE_ROOT,
  actualToolchain,
  ensureOutputDirectories,
  getContext,
  readJson,
  relativeOutputPath,
  revisionProvenance,
  validateBaseline,
  writeFailureSummary,
  writeJsonOwned,
} = require("./common.cjs");

function readIf(filePath) {
  try {
    return readJson(filePath);
  } catch {
    return null;
  }
}

function statusOf(report) {
  return report?.status ?? "UNVERIFIED";
}

function overallStatus(statuses) {
  if (statuses.includes("FAIL")) return "FAIL";
  if (statuses.includes("BLOCKED")) return "BLOCKED";
  if (statuses.includes("UNVERIFIED")) return "UNVERIFIED";
  return "PASS";
}

function packageVersions() {
  const packageJson = readJson(path.join(CANDIDATE_ROOT, "package.json"));
  const packageLock = readIf(path.join(CANDIDATE_ROOT, "package-lock.json"));
  const installedElectron = readIf(path.join(CANDIDATE_ROOT, "node_modules", "electron", "package.json"));
  const installedBuilder = readIf(path.join(CANDIDATE_ROOT, "node_modules", "electron-builder", "package.json"));
  return {
    declared: {
      electron: packageJson.devDependencies.electron,
      electronBuilder: packageJson.devDependencies["electron-builder"],
    },
    lockfile: {
      electron: packageLock?.packages?.["node_modules/electron"]?.version ?? null,
      electronBuilder: packageLock?.packages?.["node_modules/electron-builder"]?.version ?? null,
    },
    installed: {
      electron: installedElectron?.version ?? null,
      electronBuilder: installedBuilder?.version ?? null,
    },
  };
}

function addFinding(collection, status, message) {
  if (!message) return;
  if (status === "PASS") collection.successes.push(message);
  else if (status === "FAIL") collection.failures.push(message);
  else if (status === "BLOCKED") collection.blocked.push(message);
  else collection.unverified.push(message);
}

function buildManifest(context) {
  const preparation = readIf(path.join(context.evidenceRoot, "preparation.json"));
  const build = readIf(path.join(context.evidenceRoot, "build.json"));
  const smoke = readIf(path.join(context.evidenceRoot, "smoke.json"));
  const runtimeHttp = readIf(path.join(context.evidenceRoot, "runtime-http-smoke.json"));
  const lifecycle = readIf(path.join(context.evidenceRoot, "lifecycle.json"));
  const packaging = readIf(path.join(context.evidenceRoot, "package.json"));
  const updateManifest = readIf(path.join(context.evidenceRoot, "update-manifest.json"));
  const versions = packageVersions();
  const baselineValidation = (() => {
    try {
      const validation = validateBaseline(context);
      return {
        status: "PASS",
        reason: null,
        observed: validation.actual,
        fixtureReadBack: validation.fixtureReadBack,
        revisionProvenance: validation.revisionProvenance,
      };
    } catch (error) {
      const observed = error.validation?.actual ?? actualToolchain();
      return {
        status: error?.code === "BASELINE_MISMATCH" ? "BLOCKED" : "FAIL",
        reason: error instanceof Error ? error.message : String(error),
        observed,
        fixtureReadBack: error.validation?.fixtureReadBack ?? null,
        revisionProvenance: error.validation?.revisionProvenance
          ?? revisionProvenance(context.baseline, observed),
      };
    }
  })();
  const findings = { successes: [], failures: [], unverified: [], blocked: [] };
  addFinding(findings, baselineValidation.status, "shared baseline manifest と fixture の固定値を検証");
  addFinding(findings, statusOf(preparation), "candidate staging、clean migration、populated fixture copy を準備");
  addFinding(findings, statusOf(build), "staging source から production Next webpack build を生成");
  addFinding(findings, statusOf(smoke), "Electron primary window で list/search/detail/edit/explicit save/reopen を実行");
  addFinding(findings, statusOf(runtimeHttp), "Electron 外の production Next runtime で loopback HTTP/API persistence を確認");
  addFinding(findings, statusOf(lifecycle), "single instance、duplicate launch、primary window close、cleanup を観測");
  addFinding(findings, statusOf(packaging), ".app と DMG の Apple Silicon packaging を確認");

  if (packaging?.status === "BLOCKED") {
    findings.blocked.push(`packaging blocker: ${packaging.reason}`);
  }
  if (smoke?.status === "BLOCKED") {
    findings.blocked.push(`GUI/runtime smoke blocker: ${smoke.reason ?? "main result unavailable"}`);
  }
  findings.unverified.push("Developer ID signing、notarization、公開配布は PoC の対象外");
  findings.unverified.push("background download と explicit restart apply は静的 metadata 境界のみで、製品更新機能は未実装");
  findings.unverified.push("Electron artifact に Node runtime と完全な production dependency tree を同梱する配布設計は未確定");
  findings.unverified.push("Tauri + Node.js sidecar PoC 完了前の候補比較・shell 選定は未実施");

  const installStatus = versions.lockfile.electron === versions.declared.electron &&
    versions.lockfile.electronBuilder === versions.declared.electronBuilder &&
    versions.installed.electron === versions.declared.electron &&
    versions.installed.electronBuilder === versions.declared.electronBuilder
    ? "PASS"
    : "BLOCKED";
  if (installStatus === "BLOCKED") {
    findings.blocked.push("candidate npm install / package-lock generation: registry.npmjs.org was unreachable (ENOTFOUND); root dependency fallback was not used");
  }

  const statuses = [baselineValidation.status, installStatus, statusOf(preparation), statusOf(build), statusOf(smoke), statusOf(runtimeHttp), statusOf(lifecycle), statusOf(packaging)];
  const status = overallStatus(statuses);
  const toolchain = baselineValidation.observed ?? actualToolchain();
  return {
    schemaVersion: 1,
    candidate: "electron",
    candidateDecision: "not-selected; compare with Tauri PoC before Desktop Alpha",
    status,
    revisionProvenance: baselineValidation.revisionProvenance,
    baseline: {
      baseline_id: context.baseline.baseline_id,
      git_head: context.baseline.git_head,
      scope_sha256: context.baseline.baseline_scope_sha256,
      fixture: {
        count: context.baseline.fixture_count,
        seed: context.baseline.fixture_seed,
        sha256: context.baseline.fixture_sha256,
        contentHash: context.baseline.fixture_content_hash,
        sharedPath: context.fixturePath,
      },
      validation: baselineValidation,
    },
    toolchain: {
      architecture: context.baseline.architecture,
      macosVersion: context.baseline.macos_version,
      nodeVersion: context.baseline.node_version,
      npmVersion: context.baseline.npm_version,
      observed: toolchain,
      dependencies: versions,
      install: {
        status: installStatus,
        packageLockPresent: Boolean(versions.lockfile.electron && versions.lockfile.electronBuilder),
        reason: installStatus === "PASS" ? null : "npm install failed with ENOTFOUND registry.npmjs.org; no root fallback",
      },
    },
    conditions: {
      buildMode: "production Next.js runtime from the same webpack build, served on loopback",
      cacheState: build?.cacheState ?? "UNVERIFIED",
      measurementStartCondition: "candidate launcher spawn with clean candidate run directory; populated DB begins as byte-identical fixture",
      repetitions: {
        coldStart: 1,
        operationResponse: 1,
        memorySnapshot: 1,
        lifecycle: 1,
        packaging: 1,
      },
      thresholds: "none introduced by this PoC",
      runtime: {
        host: context.runtimeHost,
        port: context.runtimePort,
        portPolicy: "fixed candidate-specific port; collision fails explicitly; no fallback",
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
      },
    },
    measurements: {
      coldStart: smoke?.coldStart ?? { status: "UNVERIFIED", reason: "smoke report unavailable" },
      operationResponse: smoke?.operations ?? { status: "UNVERIFIED", reason: "smoke report unavailable" },
      memory: smoke?.memory ?? { status: "UNVERIFIED", reason: "smoke report unavailable" },
      cleanup: smoke?.cleanup ?? { status: "UNVERIFIED", reason: "smoke report unavailable" },
      shutdown: smoke?.shutdown ?? { status: "UNVERIFIED", reason: "smoke report unavailable" },
      productionRuntimeHttp: runtimeHttp ?? { status: "UNVERIFIED", reason: "runtime HTTP report unavailable" },
      sourceReport: smoke ? relativeOutputPath(context, path.join(context.evidenceRoot, "smoke.json")) : null,
    },
    lifecycle: lifecycle ?? {
      status: "UNVERIFIED",
      singleApplicationInstance: "UNVERIFIED",
      primaryWindow: "UNVERIFIED",
      duplicateLaunch: "UNVERIFIED",
      internalProcessAllowance: "not assessed",
      shutdownCleanup: "UNVERIFIED",
    },
    persistence: {
      status: preparation?.status === "PASS" && smoke?.persistence?.status === "PASS" ? "PASS" : overallStatus([statusOf(preparation), smoke?.persistence?.status ?? "UNVERIFIED"]),
      clean: preparation?.userData?.clean ?? { status: "UNVERIFIED", reason: "preparation report unavailable" },
      populated: smoke?.persistence ?? { status: "UNVERIFIED", reason: "smoke report unavailable" },
      productionRuntimeHttp: runtimeHttp?.persistence ?? { status: "UNVERIFIED", reason: "runtime HTTP report unavailable" },
      migrationContract: "existing Prisma SQLite migrations applied only to candidate clean user-data; shared fixture not migrated",
      reopenContract: smoke?.operations?.reopen ?? { status: "UNVERIFIED" },
    },
    artifacts: packaging ?? {
      status: "UNVERIFIED",
      reason: "packaging report unavailable",
      developerIdRequired: false,
      notarizationRequired: false,
    },
    updateOutlook: {
      status: updateManifest ? "PASS" : "UNVERIFIED",
      manifest: updateManifest ?? null,
      boundary: "static local manifest records version, artifact name, architecture, sha256, and explicit-restart application; background download and signature verification remain future work",
    },
    verification: {
      candidateSyntax: "run via npm run syntax and npm test",
      rootLint: "not run by task constraint",
      rootBuild: "not run by task constraint",
      rootPackageAndLockfile: "not modified",
      rootSourcePrismaDbAndNextOutput: "not modified by candidate scripts",
    },
    findings,
    risks: [
      "Electron and Tauri have not been compared; this manifest does not select a shell.",
      "PoC packaging intentionally omits Developer ID, notarization, and public distribution.",
      "Unpackaged smoke uses the same host Node binary for the local Next runtime; production sidecar bundling remains a Desktop Alpha design decision.",
      "Staging node_modules is a read-only symlink to root node_modules; root build output and generated client are not used as candidate write targets.",
      "Only one measurement trial is recorded; no threshold or candidate ranking is introduced.",
    ],
    reports: {
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
    writeJsonOwned(path.join(context.evidenceRoot, "electron-evidence-manifest.json"), manifest);
    console.log(JSON.stringify({ status: manifest.status, evidence: path.join(context.evidenceRoot, "electron-evidence-manifest.json") }));
    return manifest;
  } catch (error) {
    const failurePath = writeFailureSummary(context, "evidence", error);
    console.error(`${error instanceof Error ? error.message : String(error)}\nsummary: ${failurePath}`);
    if (error?.code === "BASELINE_MISMATCH") return { status: "BLOCKED", failurePath };
    throw error;
  }
}

if (require.main === module) {
  try {
    run();
  } catch {
    process.exitCode = 1;
  }
}

module.exports = { run, buildManifest };

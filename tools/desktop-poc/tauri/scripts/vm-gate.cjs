const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

const DEFAULT_BASELINE_MANIFEST = "/private/tmp/cornell-method-desktop-poc/mvp-gate0-20260812-dcc057d8/shared/baseline-manifest.json";
const DEFAULT_OUTPUT_ROOT_BASE = "/private/tmp/cornell-method-desktop-poc/mvp-gate0-20260812-dcc057d8";

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    env: options.env,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(`[vm-gate] ${command} failed to start: ${result.error.message}`);
    return 1;
  }
  return result.status == null ? 1 : result.status;
}

function readReport(outputRoot, relativePath) {
  const filePath = path.join(outputRoot, relativePath);
  if (!fs.existsSync(filePath)) {
    return { status: "BLOCKED", reason: `evidence がありません: ${filePath}`, evidence: filePath };
  }
  try {
    return { ...JSON.parse(fs.readFileSync(filePath, "utf8")), evidence: filePath };
  } catch (error) {
    return { status: "BLOCKED", reason: `evidence を JSON として読めません: ${filePath}: ${error.message}`, evidence: filePath };
  }
}

function readFailureSummary(outputRoot, name) {
  const filePath = path.join(outputRoot, "logs", `${name}-failure-summary.json`);
  if (!fs.existsSync(filePath)) return null;
  try {
    return { ...JSON.parse(fs.readFileSync(filePath, "utf8")), evidence: filePath };
  } catch (error) {
    return { status: "BLOCKED", reason: `failure summary を JSON として読めません: ${filePath}: ${error.message}`, evidence: filePath };
  }
}

function printFailureDiagnostics(outputRoot, name) {
  const summary = readFailureSummary(outputRoot, name);
  if (!summary?.diagnostics) return;
  console.log(`[vm-gate] ${name} diagnostics:`);
  console.log(JSON.stringify({
    summary: summary.evidence,
    binary: summary.diagnostics.binary,
    pid: summary.diagnostics.pid,
    childExit: summary.diagnostics.childExit,
    stdoutTail: summary.diagnostics.stdoutTail,
    stderrTail: summary.diagnostics.stderrTail,
  }, null, 2));
}

function copyDirectoryContents(source, destination) {
  fs.mkdirSync(destination, { recursive: true });
  for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
    fs.cpSync(path.join(source, entry.name), path.join(destination, entry.name), {
      force: true,
      recursive: true,
    });
  }
}

function sha256File(filePath) {
  const hash = crypto.createHash("sha256");
  const descriptor = fs.openSync(filePath, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    let bytesRead;
    do {
      bytesRead = fs.readSync(descriptor, buffer, 0, buffer.length, null);
      if (bytesRead > 0) hash.update(buffer.subarray(0, bytesRead));
    } while (bytesRead > 0);
  } finally {
    fs.closeSync(descriptor);
  }
  return hash.digest("hex");
}

function selectOutputRoot(defaultRoot) {
  if (process.env.POC_OUTPUT_ROOT) return path.resolve(process.env.POC_OUTPUT_ROOT);
  if (!fs.existsSync(defaultRoot)) return defaultRoot;
  for (let attempt = 1; attempt <= 100; attempt += 1) {
    const candidate = `${defaultRoot}-rerun${attempt}`;
    if (!fs.existsSync(candidate)) return candidate;
  }
  throw new Error(`既存 output root が多すぎます。POC_OUTPUT_ROOT を明示してください: ${defaultRoot}`);
}

function installSharedFile(source, destination) {
  if (!fs.existsSync(source)) throw new Error(`bundle shared file がありません: ${source}`);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const sourceHash = sha256File(source);
  if (fs.existsSync(destination)) {
    if (!fs.statSync(destination).isFile()) {
      throw new Error(`shared file の配置先が通常ファイルではありません: ${destination}`);
    }
    const destinationHash = sha256File(destination);
    if (destinationHash !== sourceHash) {
      throw new Error(`既存の shared file を上書きしません: ${destination}`);
    }
    return { status: "REUSED", source, destination, sha256: sourceHash };
  }
  fs.copyFileSync(source, destination, fs.constants.COPYFILE_EXCL);
  return { status: "INSTALLED", source, destination, sha256: sourceHash };
}

function installSharedAssets(bundleRoot, baselineManifest) {
  const bundleSharedRoot = path.join(bundleRoot, "shared");
  const bundleBaselineManifest = path.join(bundleSharedRoot, "baseline-manifest.json");
  if (!fs.existsSync(bundleBaselineManifest)) {
    throw new Error(`bundle baseline manifest がありません: ${bundleBaselineManifest}`);
  }
  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(bundleBaselineManifest, "utf8"));
  } catch (error) {
    throw new Error(`bundle baseline manifest を JSON として読めません: ${error.message}`);
  }
  const fixtureName = path.basename(manifest.fixture_path ?? "");
  if (!fixtureName || fixtureName === "." || fixtureName === ".." || fixtureName === path.sep) {
    throw new Error("bundle baseline manifest の fixture_path が不正です");
  }
  const bundleFixture = path.join(bundleSharedRoot, fixtureName);
  const sharedRoot = path.dirname(baselineManifest);
  const targetFixture = path.resolve(manifest.fixture_path);
  if (path.dirname(targetFixture) !== sharedRoot) {
    throw new Error("bundle baseline manifest の fixture_path は baseline manifest と同じ shared directory を指す必要があります");
  }
  const installed = [
    installSharedFile(bundleBaselineManifest, baselineManifest),
    installSharedFile(bundleFixture, targetFixture),
  ];
  console.log(`[vm-gate] shared assets: ${installed.map((item) => `${item.status} ${item.destination}`).join(", ")}`);
  return installed;
}

function stageResult(results, name, report) {
  const result = { name, status: report.status ?? "UNVERIFIED", evidence: report.evidence };
  results.push(result);
  console.log(`[vm-gate] ${name}: ${result.status}`);
  if (report.reason) console.log(`[vm-gate] ${name} reason: ${report.reason}`);
  return result;
}

function main() {
  const bundleRoot = path.resolve(process.env.BUNDLE_ROOT ?? path.resolve(__dirname, "../../../.."));
  const projectRoot = path.resolve(process.env.PROJECT_ROOT ?? "/Users/cornell/Projects/Cornell-Method");
  const candidateRoot = path.join(projectRoot, "tools", "desktop-poc", "tauri");
  const bundleCandidateRoot = path.join(bundleRoot, "tools", "desktop-poc", "tauri");
  const bundleName = path.basename(bundleRoot);
  const suffix = bundleName.startsWith("tauri-poc-sync-current-vm-")
    ? bundleName.slice("tauri-poc-sync-current-vm-".length)
    : `manual-${Date.now()}`;
  const outputRoot = selectOutputRoot(
    path.join(DEFAULT_OUTPUT_ROOT_BASE, `tauri-current-vm-${suffix}`),
  );
  const baselineManifest = path.resolve(process.env.POC_BASELINE_MANIFEST ?? DEFAULT_BASELINE_MANIFEST);
  const env = {
    ...process.env,
    POC_BASELINE_MANIFEST: baselineManifest,
    POC_OUTPUT_ROOT: outputRoot,
  };
  const results = [];
  const unexpected = [];

  if (!fs.existsSync(path.join(bundleRoot, "SHA256SUMS"))) throw new Error(`SHA256SUMS がありません: ${bundleRoot}`);
  if (!fs.existsSync(path.join(bundleRoot, "SHARED-SHA256SUMS"))) {
    throw new Error(`SHARED-SHA256SUMS がありません: ${bundleRoot}`);
  }
  if (!fs.existsSync(bundleCandidateRoot)) throw new Error(`bundle candidate がありません: ${bundleCandidateRoot}`);

  console.log(`[vm-gate] bundle: ${bundleRoot}`);
  console.log(`[vm-gate] project: ${projectRoot}`);
  console.log(`[vm-gate] output: ${outputRoot}`);
  if (run("shasum", ["-a", "256", "-c", "SHA256SUMS"], { cwd: bundleRoot })) throw new Error("bundle SHA256 検証に失敗しました");
  if (run("shasum", ["-a", "256", "-c", "SHARED-SHA256SUMS"], { cwd: bundleRoot })) {
    throw new Error("bundle shared SHA256 検証に失敗しました");
  }

  installSharedAssets(bundleRoot, baselineManifest);

  copyDirectoryContents(bundleCandidateRoot, candidateRoot);
  if (run("shasum", ["-a", "256", "-c", path.join(bundleRoot, "SHA256SUMS")], { cwd: projectRoot })) {
    throw new Error("candidate 配置後の SHA256 検証に失敗しました");
  }

  if (run("npm", ["test"], { cwd: candidateRoot, env })) throw new Error("candidate npm test に失敗しました");
  if (run("npm", ["run", "syntax"], { cwd: candidateRoot, env })) throw new Error("candidate syntax に失敗しました");

  const runStage = (name, relativeEvidence) => {
    const code = run("npm", ["run", name], { cwd: candidateRoot, env });
    const report = readReport(outputRoot, relativeEvidence);
    stageResult(results, name, report);
    printFailureDiagnostics(outputRoot, name.replace(/^poc:/, ""));
    if (code !== 0) unexpected.push(`${name}: npm exit ${code}`);
    return report;
  };

  const validate = runStage("poc:validate", "evidence/baseline-validation.json");
  if (validate.status !== "PASS") throw new Error(`baseline validation が PASS ではありません: ${validate.evidence}`);
  const prepare = runStage("poc:prepare", "evidence/preparation.json");
  if (prepare.status !== "PASS") throw new Error(`prepare が PASS ではありません: ${prepare.evidence}`);
  const build = runStage("poc:build", "evidence/build.json");
  if (build.status !== "PASS" || build.nextBuild?.status !== "PASS" || build.tauriCompilation?.status !== "PASS") {
    throw new Error(`build が PASS ではありません: ${build.evidence}`);
  }
  const runtimeHttp = runStage("poc:runtime-http", "evidence/runtime-http-smoke.json");
  if (runtimeHttp.status !== "PASS") throw new Error(`runtime-http が PASS ではありません: ${runtimeHttp.evidence}`);

  const smoke = runStage("poc:smoke", "evidence/smoke.json");
  const expectedUiBoundary = smoke.status === "BLOCKED"
    && smoke.nativeShell?.status === "PASS"
    && smoke.cleanup?.status === "PASS"
    && smoke.uiSmoke?.status === "BLOCKED";
  if (expectedUiBoundary) {
    console.log("[vm-gate] smoke: BLOCKED は native shell PASS / renderer UI automation unavailable の既知境界です");
  } else if (smoke.status !== "PASS") {
    unexpected.push(`poc:smoke: ${smoke.evidence}`);
  }

  const lifecycle = runStage("poc:lifecycle", "evidence/lifecycle.json");
  if (lifecycle.status !== "PASS") unexpected.push(`poc:lifecycle: ${lifecycle.evidence}`);
  const packaging = runStage("poc:package", "evidence/package.json");
  if (packaging.status !== "PASS") unexpected.push(`poc:package: ${packaging.evidence}`);
  const evidence = runStage("poc:evidence", "evidence/tauri-evidence-manifest.json");

  console.log(JSON.stringify({
    outputRoot,
    stages: results,
    finalEvidence: evidence.evidence,
    unexpected,
  }, null, 2));
  process.exitCode = unexpected.length > 0 ? 1 : 0;
}

if (require.main === module) {
  try {
    main();
  } catch (error) {
    console.error(`[vm-gate] ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}

module.exports = {
  copyDirectoryContents,
  installSharedAssets,
  installSharedFile,
  readFailureSummary,
  readReport,
  sha256File,
  selectOutputRoot,
};

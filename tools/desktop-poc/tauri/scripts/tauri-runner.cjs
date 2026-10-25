const fs = require("node:fs");
const crypto = require("node:crypto");
const net = require("node:net");
const path = require("node:path");
const { spawn } = require("node:child_process");
const {
  CANDIDATE_ROOT,
  ensureDirectory,
  relativeOutputPath,
} = require("./common.cjs");
const {
  canUseProcessGroupSignal,
  isPositivePid,
  observeDescendantClosure,
  sameProcessIdentity,
  PROCESS_GROUP_SIGNAL_METHOD,
  signalProcessGroup,
  signalProcessTree,
  waitForProcessTreeExit,
} = require("./process-tree.cjs");

const TAURI_BINARY_NAME = process.platform === "win32"
  ? "cornell-method-tauri-poc.exe"
  : "cornell-method-tauri-poc";

function tauriBinary(context) {
  if (process.env.POC_TAURI_BINARY) return path.resolve(process.env.POC_TAURI_BINARY);
  const candidates = [
    path.join(context.tauriTargetRoot, "aarch64-apple-darwin", "release", TAURI_BINARY_NAME),
    path.join(context.tauriTargetRoot, "release", TAURI_BINARY_NAME),
    path.join(context.tauriTargetRoot, "debug", TAURI_BINARY_NAME),
  ];
  return candidates.find((candidate) => fs.existsSync(candidate)) ?? candidates[0];
}

function uniqueRunDirectory(context, prefix) {
  ensureDirectory(context.evidenceRunsRoot);
  const base = `${prefix}-${Date.now()}-${process.pid}`;
  let target = path.join(context.evidenceRunsRoot, base);
  let suffix = 1;
  while (fs.existsSync(target)) {
    target = path.join(context.evidenceRunsRoot, `${base}-${suffix}`);
    suffix += 1;
  }
  ensureDirectory(target);
  return target;
}

function tauriInstanceSocketPath(context, mode) {
  const token = crypto
    .createHash("sha256")
    .update(`${context.outputRoot}\0${mode}`)
    .digest("hex")
    .slice(0, 24);
  return path.join("/tmp", `cornell-tauri-${token}.sock`);
}

function tailOutput(value, limit = 12) {
  return String(value).split(/\r?\n/).filter(Boolean).slice(-limit);
}

function waitForExit(child, timeoutMs) {
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve({ code: child.exitCode, signal: child.signalCode, timedOut: false });
      return;
    }
    let settled = false;
    let timer;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(value);
    };
    timer = setTimeout(() => finish({
      code: child.exitCode,
      signal: child.signalCode,
      timedOut: true,
    }), timeoutMs);
    child.once("exit", (code, signal) => finish({ code, signal, timedOut: false }));
    child.once("error", (error) => finish({ code: null, signal: null, timedOut: false, error: error.message }));
  });
}

function waitForJson(filePath, predicate, timeoutMs = 30_000, child = null) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (child && (child.exitCode !== null || child.signalCode !== null)) {
        reject(new Error(`Tauri process exited before state: ${filePath} (code=${child.exitCode}, signal=${child.signalCode})`));
        return;
      }
      if (Date.now() >= deadline) {
        reject(new Error(`Tauri state timeout: ${filePath}`));
        return;
      }
      if (fs.existsSync(filePath)) {
        try {
          const value = JSON.parse(fs.readFileSync(filePath, "utf8"));
          if (predicate(value)) {
            resolve(value);
            return;
          }
        } catch {
          // The Rust process can still be writing the state atomically.
        }
      }
      setTimeout(tick, 100);
    };
    tick();
  });
}

function portIsListening(context) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host: context.runtimeHost, port: context.runtimePort });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => {
      socket.destroy();
      resolve(false);
    });
    socket.setTimeout(1000, () => {
      socket.destroy();
      resolve(false);
    });
  });
}

function startTauri(context, mode, paths, showWindow = "0") {
  const binary = tauriBinary(context);
  const launchStartNs = process.hrtime.bigint();
  const child = spawn(binary, [], {
    cwd: CANDIDATE_ROOT,
    detached: process.platform !== "win32",
    env: {
      ...process.env,
      POC_BASELINE_MANIFEST: context.baselinePath,
      POC_STAGING_DIR: context.stagingRoot,
      POC_DATABASE_PATH: context.populatedDatabasePath,
      POC_RUNTIME_HOST: context.runtimeHost,
      POC_RUNTIME_PORT: String(context.runtimePort),
      POC_NODE_BINARY: process.execPath,
      POC_TAURI_USER_DATA: paths.tauriUserData,
      POC_TAURI_INSTANCE_LOCK: path.join(paths.tauriUserData, "instance.lock"),
      POC_TAURI_INSTANCE_SOCKET: tauriInstanceSocketPath(context, mode),
      POC_RESULT_FILE: paths.result,
      POC_LIFECYCLE_STATE_FILE: paths.state,
      POC_LIFECYCLE_COMMAND_FILE: paths.command,
      POC_LAUNCH_START_NS: launchStartNs.toString(),
      POC_TAURI_MODE: mode,
      POC_SHOW_WINDOW: showWindow,
      RUST_BACKTRACE: process.env.RUST_BACKTRACE ?? "full",
      CARGO_TARGET_DIR: context.tauriTargetRoot,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout?.on("data", (chunk) => { stdout += chunk; });
  child.stderr?.on("data", (chunk) => { stderr += chunk; });
  return {
    child,
    binary,
    launchStartNs,
    stdout: () => stdout,
    stderr: () => stderr,
  };
}

function processGroupFallbackReason(processGroup, before, rootIdentityMatches) {
  const reasons = [];
  if (!before?.rootObserved || before.observationStatus !== "PASS") reasons.push(before?.error ?? before?.reason ?? "shutdown 前の sidecar descendant closure を観測できません");
  else if (before.processGroupScoped !== true) reasons.push(before.processGroupScopeReason ?? "sidecar descendant closure 全体の process group scope を検証できません");
  if (processGroup?.validated !== true) reasons.push(processGroup?.reason ?? "dedicated process group が検証済みではありません");
  if (!isPositivePid(processGroup?.id)) reasons.push(`expected process group ID が正の整数ではありません: ${processGroup?.id}`);
  if (before?.expectedProcessGroupId !== processGroup?.id) reasons.push("shutdown 前の観測対象 group ID が runtime の expected group ID と一致しません");
  if (!rootIdentityMatches) reasons.push("shutdown 前の sidecar root identity が readiness 時の identity と一致しません");
  return reasons.join("; ");
}

async function stopOwnedProcess(child, readyState = null, timeoutMs = 5000) {
  const rootPid = child?.pid ?? readyState?.runtime?.rootPid ?? null;
  const expectedGroupId = readyState?.runtime?.processGroup?.id ?? rootPid;
  const before = observeDescendantClosure(rootPid, { expectedProcessGroupId: expectedGroupId });
  const readyRoot = readyState?.runtime?.processTreeAtReady?.processTree?.find((record) => record.pid === rootPid) ?? null;
  const beforeRoot = before.processTree?.find((record) => record.pid === rootPid) ?? null;
  const rootIdentityMatches = Boolean(!readyRoot ? beforeRoot : beforeRoot && sameProcessIdentity(readyRoot, beforeRoot));
  const groupUsable = canUseProcessGroupSignal(
    readyState?.runtime?.processGroup ?? { id: expectedGroupId, validated: false },
    before,
  ) && rootIdentityMatches;
  const fallbackReason = groupUsable ? null : processGroupFallbackReason(
    readyState?.runtime?.processGroup ?? { id: expectedGroupId, validated: false },
    before,
    rootIdentityMatches,
  );
  const term = groupUsable
    ? signalProcessGroup(expectedGroupId, "SIGTERM", { observedPids: before.pids ?? [] })
    : before.observationStatus === "PASS" && before.processTree.length > 0
      ? signalProcessTree(before, "SIGTERM")
      : { status: "UNVERIFIED", requested: false, method: "no-validated-process-tree", signal: "SIGTERM", targetPids: [], reason: fallbackReason };
  const graceful = before.observationStatus === "PASS" && before.processTree.length > 0
    ? await waitForProcessTreeExit(before, 3000)
    : { status: "UNVERIFIED", timedOut: true, after: { observationStatus: "UNVERIFIED", remainingPids: null }, reason: fallbackReason };
  let kill = { status: "PASS", requested: false, method: groupUsable ? PROCESS_GROUP_SIGNAL_METHOD : "explicit-pid-from-validated-descendant-closure", signal: "SIGKILL", targetPids: [] };
  let forcedObservation = graceful.after ?? before;
  let forced = false;
  if (graceful.status !== "PASS" || (Array.isArray(graceful.after?.remainingPids) && graceful.after.remainingPids.length > 0)) {
    forced = true;
    const latest = observeDescendantClosure(rootPid, { expectedProcessGroupId: expectedGroupId });
    const latestReadyRoot = latest.processTree?.find((record) => record.pid === rootPid) ?? null;
    const latestIdentityMatches = Boolean(!readyRoot ? latestReadyRoot : latestReadyRoot && sameProcessIdentity(readyRoot, latestReadyRoot));
    const latestGroupUsable = canUseProcessGroupSignal(
      readyState?.runtime?.processGroup ?? { id: expectedGroupId, validated: false },
      latest,
    ) && latestIdentityMatches;
    forcedObservation = latest;
    if (latest.observationStatus === "PASS" && latest.processTree.length > 0) {
      kill = latestGroupUsable
        ? signalProcessGroup(expectedGroupId, "SIGKILL", { observedPids: latest.pids ?? [] })
        : signalProcessTree(latest, "SIGKILL");
      if (!latestGroupUsable && !fallbackReason) kill.fallbackReason = "SIGKILL 直前の process group scope を検証できないため explicit PID tree fallback を使用しました";
      const forcedWait = await waitForProcessTreeExit(latest, 2000);
      forcedObservation = forcedWait.after ?? forcedObservation;
    } else {
      kill = { status: "UNVERIFIED", requested: false, method: "explicit-pid-from-validated-descendant-closure", signal: "SIGKILL", targetPids: [], reason: "SIGKILL 用の残存 descendant closure を観測できません。PID 再利用を避けて signal を送信しません" };
    }
  }
  const exit = await waitForExit(child, Math.min(timeoutMs, 500));
  const after = forcedObservation?.remainingPids ? forcedObservation : observeDescendantClosure(rootPid, { expectedProcessGroupId: expectedGroupId });
  const cleanupStatus = after.observationStatus === "PASS" && Array.isArray(after.remainingPids) && after.remainingPids.length === 0 && term.status === "PASS" && kill.status === "PASS" ? "PASS" : "UNVERIFIED";
  return {
    status: cleanupStatus,
    rootPid,
    processGroup: readyState?.runtime?.processGroup ?? null,
    processTreeBeforeShutdown: before,
    processTreeAfterShutdown: after,
    signalSelection: {
      processGroupEligible: groupUsable,
      selectedTermMethod: term.method,
      groupFallbackReason: fallbackReason,
      forced,
      selectedKillMethod: kill.method,
    },
    sigterm: term,
    sigkill: kill,
    gracefulWait: graceful,
    exit,
    observationWaitMs: 5000,
  };
}

function relativeRunPath(context, runPath) {
  return runPath ? relativeOutputPath(context, runPath) : null;
}

module.exports = {
  TAURI_BINARY_NAME,
  portIsListening,
  relativeRunPath,
  startTauri,
  stopOwnedProcess,
  tailOutput,
  tauriInstanceSocketPath,
  tauriBinary,
  uniqueRunDirectory,
  waitForExit,
  waitForJson,
};

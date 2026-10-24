const {
  app,
  BrowserWindow,
  session,
} = require("electron");
const fs = require("node:fs");
const http = require("node:http");
const net = require("node:net");
const path = require("node:path");
const { spawn } = require("node:child_process");
const {
  assessRuntimeCleanup,
  buildDescendantClosure,
  canUseProcessGroupSignal,
  isPositivePid,
  observeDescendantClosure,
  signalProcessGroup,
  signalProcessTree,
  readProcessTable,
  sameProcessIdentity,
  validateDedicatedProcessGroup,
  waitForProcessTreeExit,
} = require("../scripts/process-tree.cjs");

const MODE = process.argv.includes("--poc-mode=lifecycle")
  ? "lifecycle"
  : "smoke";
const IS_LIFECYCLE = MODE === "lifecycle";
const candidateRoot = path.resolve(__dirname, "..");
const stagingRoot = path.resolve(process.env.POC_STAGING_DIR ?? path.join(candidateRoot, "staging"));
const runtimeHost = process.env.POC_RUNTIME_HOST ?? "127.0.0.1";
const runtimePort = Number(process.env.POC_RUNTIME_PORT ?? 37821);
const databasePath = path.resolve(process.env.POC_DATABASE_PATH ?? path.join(stagingRoot, "user-data", "live.sqlite"));
const resultPath = process.env.POC_RESULT_FILE ? path.resolve(process.env.POC_RESULT_FILE) : null;
const lifecycleStatePath = process.env.POC_LIFECYCLE_STATE_FILE
  ? path.resolve(process.env.POC_LIFECYCLE_STATE_FILE)
  : null;
const lifecycleCommandPath = process.env.POC_LIFECYCLE_COMMAND_FILE
  ? path.resolve(process.env.POC_LIFECYCLE_COMMAND_FILE)
  : null;
const launchStartNs = process.env.POC_LAUNCH_START_NS
  ? BigInt(process.env.POC_LAUNCH_START_NS)
  : process.hrtime.bigint();
const runtimeNode = process.env.POC_NODE_BINARY ?? "node";
const nextBinary = path.join(
  stagingRoot,
  "node_modules",
  ".bin",
  process.platform === "win32" ? "next.cmd" : "next",
);
const appUserDataPath = process.env.POC_ELECTRON_USER_DATA
  ? path.resolve(process.env.POC_ELECTRON_USER_DATA)
  : undefined;

function createLoopbackApiRequestHeaderHook({ runtimeHost, runtimePort, getPrimaryWebContentsId }) {
  const runtimeOrigin = `http://${runtimeHost}:${runtimePort}`;
  const canonicalOrigin = runtimeHost === "127.0.0.1"
    ? `http://localhost:${runtimePort}`
    : runtimeOrigin;
  const stateChangingMethods = new Set(["POST", "PATCH", "DELETE"]);
  const diagnostics = {
    hookInstalled: false,
    matchedRequestCount: 0,
    lastMatchedRequest: null,
  };

  function readRequestHeader(requestHeaders, name) {
    const key = Object.keys(requestHeaders).find((candidate) => candidate.toLowerCase() === name.toLowerCase());
    return key ? requestHeaders[key] : null;
  }

  function summarizeOrigin(value) {
    return value === canonicalOrigin ? canonicalOrigin : null;
  }

  function summarizeReferer(value) {
    if (typeof value !== "string") return null;
    if (value !== value.trim()) return null;
    try {
      const parsed = new URL(value);
      if (
        (parsed.protocol !== "http:" && parsed.protocol !== "https:") ||
        parsed.username ||
        parsed.password
      ) {
        return null;
      }
      return parsed.origin === canonicalOrigin ? canonicalOrigin : null;
    } catch {
      return null;
    }
  }

  function removeRequestHeader(requestHeaders, name) {
    for (const key of Object.keys(requestHeaders)) {
      if (key.toLowerCase() === name.toLowerCase()) delete requestHeaders[key];
    }
  }

  function setRequestHeader(requestHeaders, name, value) {
    removeRequestHeader(requestHeaders, name);
    requestHeaders[name] = value;
  }

  const listener = (details, callback) => {
    const requestHeaders = { ...details.requestHeaders };
    let requestUrl;
    try {
      requestUrl = new URL(details.url);
    } catch {
      callback({ requestHeaders });
      return;
    }
    const isLoopbackApiRequest = requestUrl.origin === runtimeOrigin && requestUrl.pathname.startsWith("/api/");
    const hasWebContentsId = details.webContentsId !== undefined && details.webContentsId !== null;
    const isPrimaryRenderer = !hasWebContentsId || details.webContentsId === getPrimaryWebContentsId();
    const isStateChangingRequest = stateChangingMethods.has(String(details.method).toUpperCase());
    if (isLoopbackApiRequest && isPrimaryRenderer && isStateChangingRequest) {
      const incomingOrigin = summarizeOrigin(readRequestHeader(requestHeaders, "Origin"));
      const incomingReferer = summarizeReferer(readRequestHeader(requestHeaders, "Referer"));
      setRequestHeader(requestHeaders, "Origin", canonicalOrigin);
      setRequestHeader(requestHeaders, "Referer", `${canonicalOrigin}/`);
      const matchedRequest = {
        method: String(details.method).toUpperCase(),
        path: requestUrl.pathname,
        hasWebContentsId,
        incomingHeaders: {
          origin: incomingOrigin,
          referer: incomingReferer,
        },
        outgoingHeaders: {
          origin: summarizeOrigin(readRequestHeader(requestHeaders, "Origin")),
          referer: summarizeReferer(readRequestHeader(requestHeaders, "Referer")),
        },
        callbackCalled: false,
      };
      diagnostics.matchedRequestCount += 1;
      diagnostics.lastMatchedRequest = matchedRequest;
      try {
        callback({ requestHeaders });
      } finally {
        matchedRequest.callbackCalled = true;
      }
      return;
    }
    callback({ requestHeaders });
  };

  return {
    filter: { urls: [`${runtimeOrigin}/api/*`] },
    listener,
    diagnostics,
    markInstalled() {
      diagnostics.hookInstalled = true;
    },
  };
}

if (appUserDataPath) {
  fs.mkdirSync(appUserDataPath, { recursive: true });
  app.setPath("userData", appUserDataPath);
}

const hasSingleInstanceLock = app.requestSingleInstanceLock();
if (!hasSingleInstanceLock) {
  app.whenReady().then(() => app.quit());
} else {
  let primaryWindow;
  let runtimeProcess;
  let runtimeExitPromise;
  let runtimeExitRecord;
  let runtimeOutput = [];
  let shutdownPromise;
  let shutdownComplete = false;
  let lifecyclePoll;
  let duplicateLaunches = 0;
  let lifecycleEvents = [];
  const result = {
    schemaVersion: 1,
    candidate: "electron",
    mode: MODE,
    status: "UNVERIFIED",
    primaryWindow: {
      count: 0,
      created: false,
      userFacing: true,
      shellApiExposed: false,
    },
    runtime: {
      host: runtimeHost,
      port: runtimePort,
      databasePath,
      databaseUrl: `file:${databasePath}`,
      stagingPath: stagingRoot,
      bindBoundary: "127.0.0.1",
      pid: null,
      rootPid: null,
      processGroup: {
        id: null,
        detached: false,
        validated: false,
        method: "not-used",
      },
      processOwnership: {
        method: "Electron main spawned runtime root; descendants require parent-child closure",
        launcherPid: process.pid,
        rootPid: null,
      },
      readyStatus: "UNVERIFIED",
    },
    coldStart: {
      measuredFrom: "launcher process launch monotonic timestamp",
      processLaunchToRuntimeReadyMs: null,
      processLaunchToPrimaryWindowUsableMs: null,
    },
    operations: {
      list: { status: "UNVERIFIED", durationMs: null },
      search: { status: "UNVERIFIED", durationMs: null },
      detail: { status: "UNVERIFIED", durationMs: null },
      edit: { status: "UNVERIFIED", durationMs: null },
      explicitSave: { status: "UNVERIFIED", durationMs: null },
      reopen: { status: "UNVERIFIED", durationMs: null },
    },
    renderer: { pid: null },
    memory: { status: "UNVERIFIED", scope: [], processes: [], totalRssKb: null },
    uiSmoke: { status: "UNVERIFIED", editedNoteId: null, editedTitle: null },
    requestHookDiagnostics: {
      hookInstalled: false,
      matchedRequestCount: 0,
      lastMatchedRequest: null,
    },
    duplicateLaunches: 0,
    lifecycleEvents,
    shutdown: { status: "UNVERIFIED", observationWaitMs: 5000 },
    errors: [],
    measuredAt: null,
  };

  function nowMs() {
    return Number(process.hrtime.bigint() - launchStartNs) / 1_000_000;
  }

  function writeJson(filePath, value) {
    if (!filePath) return;
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
  }

  function writeResult() {
    result.duplicateLaunches = duplicateLaunches;
    result.lifecycleEvents = lifecycleEvents;
    result.measuredAt = new Date().toISOString();
    writeJson(resultPath, result);
    if (IS_LIFECYCLE) writeJson(lifecycleStatePath, result);
  }

  function rememberRuntimeOutput(chunk) {
    const lines = String(chunk).split(/\r?\n/).filter(Boolean);
    runtimeOutput = [...runtimeOutput, ...lines].slice(-20);
  }

  function portIsAvailable(port) {
    return new Promise((resolve, reject) => {
      const probe = net.createServer();
      probe.once("error", reject);
      probe.listen({ host: runtimeHost, port }, () => {
        probe.close(() => resolve(true));
      });
    });
  }

  function request(pathname) {
    return new Promise((resolve, reject) => {
      const requestStarted = process.hrtime.bigint();
      const requestHandle = http.get(
        { hostname: runtimeHost, port: runtimePort, path: pathname, timeout: 2000 },
        (response) => {
          response.resume();
          response.once("end", () => resolve({
            statusCode: response.statusCode ?? 0,
            durationMs: Number(process.hrtime.bigint() - requestStarted) / 1_000_000,
          }));
        },
      );
      requestHandle.once("error", reject);
      requestHandle.once("timeout", () => requestHandle.destroy(new Error("HTTP readiness timeout")));
    });
  }

  async function waitForRuntimeReady() {
    const deadline = Date.now() + 20_000;
    let lastError = "not attempted";
    while (Date.now() < deadline) {
      try {
        const response = await request("/notes");
        if (response.statusCode >= 200 && response.statusCode < 400) {
          result.runtime.readyStatus = "PASS";
          result.runtime.readyHttpStatus = response.statusCode;
          result.coldStart.processLaunchToRuntimeReadyMs = Math.round(nowMs());
          return response;
        }
        lastError = `HTTP ${response.statusCode}`;
      } catch (error) {
        lastError = error instanceof Error ? error.message : String(error);
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error(`loopback runtime readiness timeout: ${lastError}`);
  }

  async function startRuntime() {
    try {
      await portIsAvailable(runtimePort);
    } catch (error) {
      throw new Error(`固定 loopback port ${runtimePort} は使用中です。別 port へ fallback しません: ${error.message}`);
    }
    if (!fs.existsSync(nextBinary)) {
      throw new Error(`staging Next binary がありません: ${nextBinary}`);
    }
    runtimeProcess = spawn(
      runtimeNode,
      [nextBinary, "start", "--hostname", runtimeHost, "--port", String(runtimePort)],
      {
        cwd: stagingRoot,
        env: {
          ...process.env,
          DATABASE_URL: `file:${databasePath}`,
          PRISMA_PROVIDER: "sqlite",
          NODE_ENV: "production",
          CORNELL_FIXTURE_DIST_DIR: "next-dist",
          CORNELL_FIXTURE_TSCONFIG_PATH: "tsconfig.poc.json",
        },
        stdio: ["ignore", "pipe", "pipe"],
        detached: process.platform !== "win32",
      },
    );
    const runtimePid = runtimeProcess.pid ?? null;
    if (!isPositivePid(runtimePid)) {
      throw new Error(`runtime root PID が正の整数ではありません: ${runtimePid}`);
    }
    result.runtime.pid = runtimePid;
    result.runtime.rootPid = runtimePid;
    result.runtime.processOwnership.rootPid = runtimePid;
    const processGroupValidation = process.platform === "win32"
      ? {
        status: "UNVERIFIED",
        validated: false,
        method: "explicit-descendant-closure",
        reason: "Windows では process group signal を使用しません",
      }
      : validateDedicatedProcessGroup(runtimePid);
    result.runtime.processGroup = {
      id: process.platform === "win32" ? null : runtimePid,
      detached: process.platform !== "win32",
      ...processGroupValidation,
    };
    runtimeProcess.stdout?.on("data", rememberRuntimeOutput);
    runtimeProcess.stderr?.on("data", rememberRuntimeOutput);
    runtimeExitPromise = new Promise((resolve) => {
      runtimeProcess.once("exit", (code, signal) => {
        runtimeExitRecord = { code, signal };
        resolve(runtimeExitRecord);
      });
      runtimeProcess.once("error", (error) => {
        runtimeExitRecord = { code: null, signal: null, error: error.message };
        resolve(runtimeExitRecord);
      });
    });
    await waitForRuntimeReady();
    if (process.platform !== "win32" && !result.runtime.processGroup.validated) {
      const retryValidation = validateDedicatedProcessGroup(runtimePid);
      result.runtime.processGroup = {
        ...result.runtime.processGroup,
        ...retryValidation,
        id: runtimePid,
        detached: true,
        retryValidation: true,
      };
    }
    const runtimeAtReady = observeDescendantClosure(runtimePid, {
      expectedProcessGroupId: result.runtime.processGroup.id,
    });
    result.runtime.processTreeAtReady = runtimeAtReady;
    if (process.platform !== "win32" && runtimeAtReady.observationStatus === "PASS") {
      result.runtime.processGroup.observedRootProcessGroupId = runtimeAtReady.rootProcessGroupId;
    }
    if (process.platform !== "win32" && (
      runtimeAtReady.observationStatus !== "PASS" ||
      runtimeAtReady.processGroupScoped !== true
    )) {
      result.runtime.processGroup.validated = false;
      result.runtime.processGroup.status = "UNVERIFIED";
      result.runtime.processGroup.reason = runtimeAtReady.processGroupScopeReason ??
        "runtime descendant closure 全体の process group scope を検証できません";
    }
  }

  function executeRenderer(script) {
    if (!primaryWindow || primaryWindow.isDestroyed()) {
      throw new Error("primary window がありません");
    }
    return primaryWindow.webContents.executeJavaScript(`(${script})()`, true);
  }

  function waitForRendererFunction(predicate, label, timeoutMs = 20_000) {
    const deadline = Date.now() + timeoutMs;
    let lastRendererError = null;
    return new Promise((resolve, reject) => {
      const tick = async () => {
        if (Date.now() >= deadline) {
          const diagnostic = lastRendererError
            ? `; last renderer error: ${lastRendererError.message}`
            : "";
          reject(new Error(`renderer readiness timeout: ${label}${diagnostic}`));
          return;
        }
        try {
          const value = await executeRenderer(predicate);
          if (value) {
            resolve(value);
            return;
          }
        } catch (error) {
          lastRendererError = error instanceof Error ? error : new Error(String(error));
          // The document may still be navigating; keep polling until the deadline.
        }
        setTimeout(tick, 100);
      };
      void tick();
    });
  }

  function loadPrimaryWindow(url) {
    return new Promise((resolve, reject) => {
      const onLoad = () => {
        primaryWindow.webContents.removeListener("did-fail-load", onFail);
        resolve();
      };
      const onFail = (_event, errorCode, errorDescription) => {
        primaryWindow.webContents.removeListener("did-finish-load", onLoad);
        reject(new Error(`primary window load failed (${errorCode}): ${errorDescription}`));
      };
      primaryWindow.webContents.once("did-finish-load", onLoad);
      primaryWindow.webContents.once("did-fail-load", onFail);
      void primaryWindow.loadURL(url);
    });
  }

  function createPrimaryWindow() {
    primaryWindow = new BrowserWindow({
      width: 1280,
      height: 900,
      show: process.env.POC_SHOW_WINDOW === "1",
      webPreferences: {
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        preload: path.join(candidateRoot, "src", "preload.cjs"),
      },
    });
    result.primaryWindow.created = true;
    result.primaryWindow.count = BrowserWindow.getAllWindows().length;
    result.renderer.pid = primaryWindow.webContents.getOSProcessId();
    primaryWindow.on("closed", () => {
      primaryWindow = null;
      void shutdownAndQuit("primary-window-closed");
    });
    return primaryWindow;
  }

  function textButtonClickScript(label) {
    return `function () {
      const button = [...document.querySelectorAll('button')].find((element) => element.textContent?.trim() === ${JSON.stringify(label)});
      if (!button) return false;
      button.click();
      return true;
    }`;
  }

  function installPrimaryRendererRequestHook() {
    const hook = createLoopbackApiRequestHeaderHook({
      runtimeHost,
      runtimePort,
      getPrimaryWebContentsId: () => {
        if (!primaryWindow || primaryWindow.isDestroyed()) return null;
        return primaryWindow.webContents.id;
      },
    });
    session.defaultSession.webRequest.onBeforeSendHeaders(hook.filter, hook.listener);
    result.requestHookDiagnostics = hook.diagnostics;
    hook.markInstalled();
  }

  async function runUiSmoke() {
    const primaryUrl = `http://${runtimeHost}:${runtimePort}/notes`;
    nowMs();
    await loadPrimaryWindow(primaryUrl);
    const listStarted = nowMs();
    await waitForRendererFunction(
      function listIsReady() {
        return Boolean(
          document.querySelector("h1")?.textContent?.includes("ノート一覧") &&
          document.querySelector("form[role=search]") &&
          [...document.querySelectorAll("a[href^='/notes/']")].some((element) => !element.getAttribute("href")?.endsWith("/new")),
        );
      },
      "notes list",
    );
    result.operations.list = { status: "PASS", durationMs: Math.round(nowMs() - listStarted) };
    result.coldStart.processLaunchToPrimaryWindowUsableMs = Math.round(nowMs());

    const searchStarted = nowMs();
    const searchResponse = await executeRenderer(
      `async function () {
        const input = document.getElementById("notes-query");
        if (!input) return { ok: false, reason: "notes-query missing" };
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
        setter?.call(input, "schema");
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.dispatchEvent(new Event("change", { bubbles: true }));
        const response = await fetch("/api/notes?query=schema&page=1");
        const data = await response.json();
        return { ok: response.ok, status: response.status, totalCount: data.totalCount ?? null };
      }`,
    );
    await waitForRendererFunction(
      function searchIsReady() {
        return Boolean(
          document.body.textContent?.includes("検索結果") &&
          [...document.querySelectorAll("a[href^='/notes/']")].some((element) => {
            const href = element.getAttribute("href") || "";
            return /^\/notes\/[^/]+$/.test(href) && href !== "/notes/new";
          }),
        );
      },
      "notes search",
    );
    if (!searchResponse?.ok) {
      throw new Error(`search API failed: ${JSON.stringify(searchResponse)}`);
    }
    result.operations.search = {
      status: "PASS",
      durationMs: Math.round(nowMs() - searchStarted),
      query: "schema",
      totalCount: searchResponse.totalCount,
    };

    const detailStarted = nowMs();
    const detailHref = await executeRenderer(
      `function () {
        const link = [...document.querySelectorAll("a[href^='/notes/']")].find((element) => {
          const href = element.getAttribute("href") || "";
          return /^\\/notes\\/[^/]+$/.test(href) && href !== "/notes/new";
        });
        if (!link) return null;
        link.click();
        return link.getAttribute("href");
      }`,
    );
    if (!detailHref) throw new Error("検索結果から詳細リンクを取得できません");
    await waitForRendererFunction(
      function detailIsReady() {
        return Boolean(
          location.pathname.startsWith("/notes/") &&
          document.querySelector("h1.note-paper-title"),
        );
      },
      "note detail",
    );
    const detailId = detailHref.split("/").pop();
    result.operations.detail = { status: "PASS", durationMs: Math.round(nowMs() - detailStarted), noteId: detailId };

    const editStarted = nowMs();
    const editClicked = await executeRenderer(textButtonClickScript("編集"));
    if (!editClicked) throw new Error("詳細画面の編集ボタンが見つかりません");
    await waitForRendererFunction(
      function editorIsReady() {
        return Boolean(document.querySelector("#note-title"));
      },
      "note editor",
    );
    const originalTitle = await executeRenderer(
      `function () { return document.getElementById("note-title")?.value || ""; }`,
    );
    if (!originalTitle) throw new Error("編集画面のタイトルが空です");
    const editedTitle = `${originalTitle} [Electron PoC]`;
    const titleChanged = await executeRenderer(
      `function () {
        const input = document.getElementById("note-title");
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
        setter?.call(input, ${JSON.stringify(editedTitle)});
        input?.dispatchEvent(new Event("input", { bubbles: true }));
        input?.dispatchEvent(new Event("change", { bubbles: true }));
        return input?.value === ${JSON.stringify(editedTitle)};
      }`,
    );
    if (!titleChanged) throw new Error("タイトル入力の変更を renderer に適用できません");
    await waitForRendererFunction(
      `function titleStateIsReady() {
        const input = document.getElementById("note-title");
        return Boolean(
          input &&
          input.isConnected &&
          input.value === ${JSON.stringify(editedTitle)} &&
          input.defaultValue === ${JSON.stringify(editedTitle)},
        );
      }`,
      "note title state",
    );
    result.operations.edit = { status: "PASS", durationMs: Math.round(nowMs() - editStarted), noteId: detailId };

    const saveStarted = nowMs();
    const saveClicked = await executeRenderer(
      `function () {
        const form = document.querySelector("form.note-paper-editor");
        const button = form?.querySelector("button[type=submit]");
        if (!button) return false;
        button.click();
        return true;
      }`,
    );
    if (!saveClicked) throw new Error("明示保存ボタンが見つかりません");
    const saveState = await waitForRendererFunction(
      function saveIsComplete() {
        const editorError = document.querySelector("#note-editor-error-alert[role='alert']");
        if (editorError) {
          return {
            status: "error",
            message: editorError.textContent?.trim() || "保存エラーの内容が空です",
          };
        }
        const editor = document.querySelector("form.note-paper-editor");
        const detail = document.querySelector(".note-paper-detail");
        const title = detail?.querySelector("h1.note-paper-title");
        const editButton = [...(detail?.querySelectorAll("button") ?? [])].find(
          (element) => element.textContent?.trim() === "編集",
        );
        if (
          !editor &&
          detail &&
          title?.textContent?.includes("[Electron PoC]") &&
          editButton &&
          !editButton.disabled
        ) {
          return { status: "success" };
        }
        return false;
      },
      "explicit note save",
    );
    if (saveState?.status === "error") {
      throw new Error(`explicit note save failed in renderer: ${saveState.message}`);
    }
    result.operations.explicitSave = { status: "PASS", durationMs: Math.round(nowMs() - saveStarted), noteId: detailId };
    result.uiSmoke = { status: "PASS", editedNoteId: detailId, editedTitle };

    const reopenStarted = nowMs();
    await loadPrimaryWindow(`http://${runtimeHost}:${runtimePort}/notes/${detailId}`);
    await waitForRendererFunction(
      function detailReloaded() {
        return document.querySelector("h1.note-paper-title")?.textContent?.includes("[Electron PoC]");
      },
      "persisted note reload",
    );
    const persistedApiResult = await executeRenderer(
      `async function () {
        const response = await fetch(${JSON.stringify(`/api/notes/${detailId}`)});
        const data = await response.json();
        return { ok: response.ok, status: response.status, title: data.title ?? null };
      }`,
    );
    if (!persistedApiResult?.ok || persistedApiResult.title !== editedTitle) {
      throw new Error(`再読込後の persistence が不一致です: ${JSON.stringify(persistedApiResult)}`);
    }
    result.operations.reopen = { status: "PASS", durationMs: Math.round(nowMs() - reopenStarted), noteId: detailId };
    writeResult();
  }

  function collectMemorySnapshot() {
    try {
      const closure = buildDescendantClosure(readProcessTable(), process.pid);
      if (!closure.rootObserved) throw new Error(`Electron main PID ${process.pid} が process table にありません`);
      const rootPid = process.pid;
      const roleFor = (record) => {
        if (record.pid === rootPid) return "electron-main-shell";
        if (record.pid === result.runtime.pid) return "next-runtime";
        if (record.pid === result.renderer.pid) return "renderer-webview";
        if (record.commandLine.includes("--type=gpu-process")) return "framework-helper-gpu";
        if (record.commandLine.includes("--type=utility")) return "framework-helper-utility";
        if (record.commandLine.includes("Electron")) return "framework-helper";
        return "related-child-process";
      };
      const processes = closure.processes.map((record) => ({
        role: roleFor(record),
        pid: record.pid,
        parentPid: record.parentPid,
        processGroupId: record.processGroupId,
        rssKb: record.rssKb,
        footprintBytes: null,
        footprintStatus: "not-collected",
        commandName: record.commandName,
      }));
      const totalRssKb = processes.reduce((sum, processRecord) => sum + processRecord.rssKb, 0);
      result.memory = {
        status: "PASS",
        aggregation: "descendant closure rooted at Electron main PID; RSS from ps",
        scope: ["Electron main shell", "renderer/WebView", "Next.js runtime", "framework helpers", "related child processes"],
        processes,
        totalRssKb,
      };
    } catch (error) {
      result.memory = {
        status: "UNVERIFIED",
        scope: ["Electron main shell", "renderer/WebView", "Next.js runtime", "framework helpers", "related child processes"],
        processes: [],
        totalRssKb: null,
        reason: error instanceof Error ? error.message : String(error),
      };
    }
  }

  function waitForRuntimeExit(timeoutMs) {
    if (runtimeProcess && runtimeProcess.exitCode !== null) {
      return Promise.resolve({ status: "PASS", timedOut: false, timeoutMs, runtimeExit: runtimeExitRecord });
    }
    if (!runtimeExitPromise) {
      return Promise.resolve({ status: "UNVERIFIED", timedOut: false, timeoutMs, runtimeExit: runtimeExitRecord });
    }
    return new Promise((resolve) => {
      let settled = false;
      const finish = (value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        resolve(value);
      };
      const timeout = setTimeout(() => finish({
        status: "TIMEOUT",
        timedOut: true,
        timeoutMs,
        runtimeExit: runtimeExitRecord,
      }), timeoutMs);
      runtimeExitPromise.then((runtimeExit) => finish({
        status: "PASS",
        timedOut: false,
        timeoutMs,
        runtimeExit,
      }));
    });
  }

  function signalRuntimeRoot(signal) {
    const pid = result.runtime.rootPid;
    const response = {
      status: "PASS",
      requested: true,
      method: "validated-runtime-root-pid-only-fallback",
      signal,
      targetPids: [pid],
      sentPids: [],
      alreadyExitedPids: [],
      failedPids: [],
      errors: [],
    };
    try {
      process.kill(pid, signal);
      response.sentPids.push(pid);
    } catch (error) {
      if (error?.code === "ESRCH") response.alreadyExitedPids.push(pid);
      else {
        response.status = "FAIL";
        response.failedPids.push(pid);
        response.errors.push({ code: error?.code ?? null, message: error instanceof Error ? error.message : String(error) });
      }
    }
    return response;
  }

  async function stopRuntime() {
    const pid = result.runtime.rootPid ?? runtimeProcess?.pid ?? null;
    const processGroup = result.runtime.processGroup ?? {};
    const before = observeDescendantClosure(pid, {
      expectedProcessGroupId: processGroup.id,
    });
    const beforeUsable = before.observationStatus === "PASS" && Array.isArray(before.pids) && before.pids.length > 0;
    const readyRoot = result.runtime.processTreeAtReady?.processTree?.find((record) => record.pid === pid) ?? null;
    const beforeRoot = before.processTree?.find((record) => record.pid === pid) ?? null;
    const rootIdentityMatches = Boolean(readyRoot && beforeRoot && sameProcessIdentity(readyRoot, beforeRoot));
    const groupUsable = canUseProcessGroupSignal(processGroup, before) && rootIdentityMatches;
    const groupFallbackReason = (() => {
      if (groupUsable) return null;
      const reasons = [];
      if (!beforeUsable) {
        reasons.push(before.error ?? before.reason ?? "shutdown 前の runtime descendant closure を観測できません");
      } else if (before.processGroupScoped !== true) {
        reasons.push(before.processGroupScopeReason ?? "runtime descendant closure 全体の process group scope を検証できません");
      }
      if (processGroup.validated !== true) {
        reasons.push(processGroup.reason ?? "dedicated process group が検証済みではありません");
      }
      if (!isPositivePid(processGroup.id)) {
        reasons.push(`expected process group ID が正の整数ではありません: ${processGroup.id}`);
      }
      if (beforeUsable && before.expectedProcessGroupId !== processGroup.id) {
        reasons.push("shutdown 前の観測対象 group ID が runtime の expected group ID と一致しません");
      }
      if (beforeUsable && !rootIdentityMatches) {
        reasons.push("shutdown 前の runtime root identity が readiness 時の identity と一致しません");
      }
      if (beforeUsable && before.processGroupMatches !== true && before.processGroupScoped === true) {
        reasons.push("runtime root の process group ID が expected group ID と一致しません");
      }
      return reasons.join("; ");
    })();
    const term = groupUsable
      ? signalProcessGroup(processGroup.id, "SIGTERM", { observedPids: before.pids ?? [] })
      : beforeUsable
        ? signalProcessTree(before, "SIGTERM")
        : isPositivePid(pid)
          ? signalRuntimeRoot("SIGTERM")
          : {
            status: "FAIL",
            requested: false,
            method: "no-validated-runtime-pid",
            signal: "SIGTERM",
            targetPids: [],
            errors: [{ message: `runtime root PID が正の整数ではありません: ${pid}` }],
          };
    const graceful = beforeUsable
      ? await waitForProcessTreeExit(before, 3000)
      : await waitForRuntimeExit(3000);
    const knownRemaining = Array.isArray(graceful.after?.remainingPids)
      ? graceful.after.remainingPids.length
      : null;
    const shouldForce = groupUsable
      ? !beforeUsable || graceful.status !== "PASS" || knownRemaining > 0
      : beforeUsable
        ? graceful.status !== "PASS" || knownRemaining > 0
        : runtimeProcess?.exitCode === null;
    let forced = false;
    let forcedScopeObservation = null;
    let forcedGroupUsable = false;
    let forcedGroupFallbackReason = null;
    let kill = {
      status: "PASS",
      requested: false,
      method: groupUsable ? "validated-dedicated-process-group" : "explicit-pid-from-validated-descendant-closure",
      signal: "SIGKILL",
      targetPids: [],
    };
    let finalObservation = graceful.after ?? {
      status: "UNVERIFIED",
      observationStatus: "UNVERIFIED",
      rootPid: pid,
      rootObserved: false,
      processTree: [],
      remainingPids: null,
      reason: "shutdown 前の process tree が観測できていません",
    };
    if (shouldForce) {
      forced = true;
      if (groupUsable) {
        forcedScopeObservation = observeDescendantClosure(pid, {
          expectedProcessGroupId: processGroup.id,
        });
        const forcedRoot = forcedScopeObservation.processTree?.find((record) => record.pid === pid) ?? null;
        const forcedRootIdentityMatches = Boolean(readyRoot && forcedRoot && sameProcessIdentity(readyRoot, forcedRoot));
        forcedGroupUsable = canUseProcessGroupSignal(processGroup, forcedScopeObservation) && forcedRootIdentityMatches;
        if (forcedGroupUsable) {
          kill = signalProcessGroup(processGroup.id, "SIGKILL", {
            observedPids: forcedScopeObservation.pids,
          });
        } else {
          forcedGroupFallbackReason = !forcedRootIdentityMatches
            ? "SIGKILL 直前の runtime root identity が readiness 時の identity と一致しません。explicit PID tree fallback を使用します"
            : forcedScopeObservation.processGroupScopeReason ??
            "SIGKILL 直前の process group scope を検証できません。explicit PID tree fallback を使用します";
          const explicitObservation = forcedScopeObservation.observationStatus === "PASS" &&
            Array.isArray(forcedScopeObservation.processTree) &&
            forcedScopeObservation.processTree.length > 0
            ? forcedScopeObservation
            : finalObservation;
          if (Array.isArray(explicitObservation.processTree) && explicitObservation.processTree.length > 0) {
            kill = signalProcessTree(explicitObservation, "SIGKILL");
          } else {
            kill = {
              status: "UNVERIFIED",
              requested: false,
              method: "explicit-pid-from-validated-descendant-closure",
              signal: "SIGKILL",
              targetPids: [],
              reason: forcedGroupFallbackReason,
            };
          }
        }
      } else if (Array.isArray(finalObservation.processTree) && finalObservation.processTree.length > 0) {
        kill = signalProcessTree(finalObservation, "SIGKILL");
      } else if (!beforeUsable && isPositivePid(pid)) {
        kill = signalRuntimeRoot("SIGKILL");
      } else {
        kill = {
          status: "UNVERIFIED",
          requested: false,
          method: "explicit-pid-from-validated-descendant-closure",
          signal: "SIGKILL",
          targetPids: [],
          reason: "SIGKILL 用の残存 descendant closure を観測できません。PID 再利用を避けて signal を送信しません",
        };
      }
      const forcedWait = beforeUsable
        ? await waitForProcessTreeExit(before, 2000)
        : await waitForRuntimeExit(2000);
      finalObservation = forcedWait.after ?? finalObservation;
    }
    const runtimeExit = await waitForRuntimeExit(250);
    const cleanup = assessRuntimeCleanup({
      status: "PASS",
      runtimeRootPid: pid,
      processTreeBeforeShutdown: before,
      processTreeAfterShutdown: finalObservation,
    });
    const signalFailed = term.status === "FAIL" ||
      (shouldForce && kill.status !== "PASS") ||
      kill.explicitResidual?.status === "FAIL";
    const status = cleanup.status === "PASS" && runtimeExit.status === "PASS" && !signalFailed ? "PASS" : "FAIL";
    const signalSelection = {
      signal: "SIGTERM",
      groupSignalEligible: groupUsable,
      selectedMethod: term.method,
      fallbackReason: groupUsable ? null : groupFallbackReason,
      expectedProcessGroupId: processGroup.id ?? null,
      processGroupValidated: processGroup.validated === true,
      preShutdownScope: {
        observationStatus: before.observationStatus ?? "UNVERIFIED",
        rootObserved: before.rootObserved ?? false,
        rootProcessGroupId: before.rootProcessGroupId ?? null,
        processGroupMatches: before.processGroupMatches ?? null,
        processGroupScoped: before.processGroupScoped ?? null,
        processGroupScopeReason: before.processGroupScopeReason ?? null,
        rootIdentityMatches,
      },
    };
    const forcedSignalSelection = {
      signal: "SIGKILL",
      groupSignalEligible: forcedGroupUsable,
      selectedMethod: shouldForce ? kill.method : null,
      fallbackReason: shouldForce
        ? (forcedGroupUsable ? null : forcedGroupFallbackReason ?? groupFallbackReason)
        : null,
      scopeObservation: forcedScopeObservation,
    };
    return {
      status,
      runtimeRootPid: pid,
      runtimeExit: runtimeExit.runtimeExit ?? runtimeExitRecord,
      runtimeExited: runtimeExit.status === "PASS",
      signalMethod: term.method,
      signalSelection,
      forcedSignalSelection,
      processTreeBeforeShutdown: before,
      sigterm: term,
      gracefulWait: graceful,
      sigkill: kill,
      forcedTermination: Boolean(forced && (kill.delivered || kill.explicitResidual?.delivered)),
      processTreeAfterShutdown: finalObservation,
      remainingPids: finalObservation.remainingPids ?? null,
      timeoutMs: { graceful: 3000, forced: 2000, runtimeExitObservation: 250 },
      observationWaitMs: 5000,
      stderrTail: runtimeOutput.slice(-8),
      cleanup,
    };
  }

  async function shutdownAndQuit(reason) {
    if (shutdownPromise) return shutdownPromise;
    shutdownPromise = (async () => {
      const runtimeShutdown = await stopRuntime();
      let loopbackListenerRemaining = null;
      try {
        loopbackListenerRemaining = !(await portIsAvailable(runtimePort));
      } catch {
        loopbackListenerRemaining = true;
      }
      const shutdownStatus = runtimeShutdown.status === "PASS" && !loopbackListenerRemaining ? "PASS" : "FAIL";
      result.shutdown = {
        ...runtimeShutdown,
        status: shutdownStatus,
        reason,
        runtime: runtimeShutdown,
        loopbackListenerRemaining,
        observationWaitMs: 5000,
        appOwnedProcessCleanup: runtimeShutdown.status === "PASS" ? "requested-and-exited" : "failed",
      };
      shutdownComplete = true;
      writeResult();
      app.quit();
      return result.shutdown;
    })();
    return shutdownPromise;
  }

  async function runSmokeMode() {
    try {
      await startRuntime();
      const window = createPrimaryWindow();
      await runUiSmoke();
      collectMemorySnapshot();
      writeResult();
      if (window && !window.isDestroyed()) window.close();
      await shutdownAndQuit("smoke-complete");
      result.status = result.uiSmoke.status === "PASS" && result.shutdown.status === "PASS" ? "PASS" : "FAIL";
    } catch (error) {
      result.status = "BLOCKED";
      result.errors.push(error instanceof Error ? error.message : String(error));
      writeResult();
      if (primaryWindow && !primaryWindow.isDestroyed()) primaryWindow.close();
      await shutdownAndQuit("smoke-failure");
    } finally {
      writeResult();
    }
  }

  async function runLifecycleMode() {
    try {
      await startRuntime();
      const window = createPrimaryWindow();
      await loadPrimaryWindow(`http://${runtimeHost}:${runtimePort}/notes`);
      await waitForRendererFunction(
        function primaryWindowUsable() {
          return Boolean(document.querySelector("h1")?.textContent?.includes("ノート一覧"));
        },
        "lifecycle primary window",
      );
      result.coldStart.processLaunchToPrimaryWindowUsableMs = Math.round(nowMs());
      result.status = "UNVERIFIED";
      result.lifecycle = {
        singleApplicationInstance: "PASS",
        primaryWindowCount: BrowserWindow.getAllWindows().length,
        duplicateLaunches: "pending-second-instance-event",
        internalProcessesAllowed: true,
        closeCommand: "waiting",
      };
      writeResult();
      lifecyclePoll = setInterval(() => {
        if (!lifecycleCommandPath || !fs.existsSync(lifecycleCommandPath)) return;
        try {
          const command = JSON.parse(fs.readFileSync(lifecycleCommandPath, "utf8"));
          if (command.command === "close") {
            lifecycleEvents.push({ type: "primary-window-close-request", at: new Date().toISOString() });
            clearInterval(lifecyclePoll);
            if (primaryWindow && !primaryWindow.isDestroyed()) primaryWindow.close();
          }
        } catch (error) {
          result.errors.push(`lifecycle command read failed: ${error.message}`);
        }
      }, 100);
      void window;
    } catch (error) {
      result.status = "BLOCKED";
      result.errors.push(error instanceof Error ? error.message : String(error));
      writeResult();
      if (primaryWindow && !primaryWindow.isDestroyed()) primaryWindow.close();
      await shutdownAndQuit("lifecycle-start-failure");
    }
  }

  app.on("second-instance", () => {
    duplicateLaunches += 1;
    lifecycleEvents.push({ type: "second-instance", at: new Date().toISOString(), primaryWindowCount: BrowserWindow.getAllWindows().length });
    if (primaryWindow && !primaryWindow.isDestroyed()) {
      if (primaryWindow.isMinimized()) primaryWindow.restore();
      primaryWindow.show();
      primaryWindow.focus();
    }
    writeResult();
  });

  app.on("before-quit", (event) => {
    if (shutdownComplete) return;
    event.preventDefault();
    void shutdownAndQuit("before-quit");
  });

  app.whenReady().then(async () => {
    session.defaultSession.setPermissionRequestHandler((_webContents, _permission, callback) => callback(false));
    installPrimaryRendererRequestHook();
    if (IS_LIFECYCLE) {
      await runLifecycleMode();
    } else {
      await runSmokeMode();
    }
  }).catch(async (error) => {
    result.status = "BLOCKED";
    result.errors.push(error instanceof Error ? error.message : String(error));
    writeResult();
    await shutdownAndQuit("ready-handler-failure");
  });
}

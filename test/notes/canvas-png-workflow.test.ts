import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { build } from "esbuild";
import { chromium, type Page } from "playwright";

const root = path.resolve(import.meta.dirname, "../..");
const note = {
  id: "isolated-png-note", title: "保存済みタイトル", noteDate: "2026-10-16",
  sourceType: "other", sourceTitle: "", bodyMode: "canvas", body: "",
  summary: "- [ ] saved task", nextReviewDate: null, reviewedAt: null, cues: [], tags: [],
  canvas: { schemaVersion: 1, page: { width: 640, height: 480, background: "paper" }, elements: [
    { id: "saved-shape", type: "rect", x: 30, y: 30, width: 100, height: 100, rotation: 0, z: 0, style: { fill: "#4285c3", strokeWidth: 0 } },
  ] },
};

async function bundle() {
  const result = await build({
    stdin: {
      contents: `import React from 'react'; import {createRoot} from 'react-dom/client'; import {flushSync} from 'react-dom';
import {NoteDetailModes} from './src/modules/notes/ui/components/detail/modes';
import {ThemeProvider} from './src/app/_components/theme/theme-provider';
window.mountNote = (note, mode = 'view') => { window.appRoot?.unmount();
window.appRoot = createRoot(document.getElementById('root'));
// Complete mounting before count-based exclusion assertions can inspect stale DOM.
flushSync(() => window.appRoot.render(<ThemeProvider><NoteDetailModes initialNote={note} initialMode={mode}/></ThemeProvider>)); };`,
      resolveDir: root, loader: "tsx",
    },
    bundle: true, write: false, platform: "browser", format: "iife", jsx: "automatic",
    define: { "process.env.NODE_ENV": '"development"' },
    loader: { ".css": "empty" },
    plugins: [{ name: "isolated-next-router", setup(builder) {
      builder.onResolve({ filter: /^next\/(navigation|link)$/ }, (args) => ({ path: args.path, namespace: "fixture-next" }));
      builder.onLoad({ filter: /.*/, namespace: "fixture-next" }, (args) => ({ loader: "jsx", resolveDir: root, contents: args.path === "next/navigation"
        ? `import {useSyncExternalStore} from 'react';
const subscribe = callback => {window.addEventListener('fixture:navigation', callback); return () => window.removeEventListener('fixture:navigation', callback)};
export const useRouter = () => ({replace: url => {window.routeChanges.push(url); history.replaceState(null, '', url); window.dispatchEvent(new Event('fixture:navigation'))}, refresh: () => {}, push: () => {throw Error('unexpected navigation')}});
export const usePathname = () => location.pathname; export const useSearchParams = () => new URLSearchParams(useSyncExternalStore(subscribe, () => location.search));`
        : `import React from 'react'; export default function Link({children, ...props}) {return <a {...props}>{children}</a>}` }));
    } }],
  });
  return result.outputFiles[0].text;
}

async function fixture(page: Page, script: string, desktop = true, allowSave = false) {
  const errors: string[] = [];
  const mutations: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/api/") && request.method() !== "GET") mutations.push(request.method());
  });
  await page.addInitScript({ content: `window.__name = value => value; window.routeChanges = []; window.__CORNELL_PNG_DESKTOP__ = ${desktop};` });
  await page.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://127.0.0.1:47891") return route.abort();
    if (url.pathname === "/fixture.js") return route.fulfill({ contentType: "text/javascript", body: script });
    if (url.pathname === "/api/tags" && route.request().method() === "GET") return route.fulfill({ json: [] });
    if (allowSave && url.pathname === `/api/notes/${note.id}` && route.request().method() === "PATCH") {
      return route.fulfill({ json: { ...note, ...route.request().postDataJSON() } });
    }
    if (url.pathname.startsWith("/api")) return route.abort("blockedbyclient");
    return route.fulfill({ contentType: "text/html", body: '<!doctype html><html><body><div id="root"></div><script src="/fixture.js"></script></body></html>' });
  });
  await page.goto("http://127.0.0.1:47891/notes/isolated-png-note");
  await page.evaluate(() => {
    // Simulated fragment transport only: no native dialogs, DB, network or file writes.
    const state = { requests: [] as Record<string, unknown>[], chunks: [] as number[], published: [] as number[], fail: "", cancel: false, hold: false, holdDiscard: false, selectedPage: "", pending: null as null | (() => void) };
    (window as unknown as { native: typeof state }).native = state;
    window.addEventListener("hashchange", () => {
      const prefix = "#cornell-desktop-png=";
      if (!location.hash.startsWith(prefix)) return;
      const request = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(location.hash.slice(prefix.length)), (c) => c.charCodeAt(0))));
      state.requests.push(request);
      history.replaceState(null, "", location.pathname + location.search);
      const respond = () => {
        let status = "error";
        let code: string | null = null;
        let token: string | null = null;
        if (request.operation !== "select" && state.selectedPage !== location.pathname + location.search) {
          code = "unavailable";
        } else if (request.operation === "select") {
          state.selectedPage = location.pathname + location.search;
          state.chunks = [];
          status = state.cancel ? "cancelled" : "selected";
          token = state.cancel ? null : "a".repeat(48);
        } else if (request.operation === "append") {
          state.chunks.push(...Uint8Array.from(atob(request.data), (c) => c.charCodeAt(0)));
          status = "appended";
        } else if (request.operation === "publish") {
          status = state.fail ? "error" : "published";
          code = state.fail || null;
          if (!state.fail) state.published = [...state.chunks];
        } else if (request.operation === "discard") status = "discarded";
        window.dispatchEvent(new CustomEvent("cornell:desktop-png-result", { detail: { id: request.id, status, token, code } }));
      };
      if (state.hold && request.operation === "publish" || state.holdDiscard && request.operation === "discard") state.pending = respond;
      else respond();
    });
  });
  return { errors, mutations };
}
async function mount(page: Page, value: unknown, mode = "view") {
  await page.evaluate(({ value, mode }) => (window as unknown as { mountNote: (note: unknown, mode: string) => void }).mountNote(value, mode), { value, mode });
}
async function native(page: Page, patch: Record<string, unknown>) {
  await page.evaluate((patch) => Object.assign((window as unknown as { native: object }).native, patch), patch);
}
async function waitHeld(page: Page) {
  await page.waitForFunction(() => Boolean((window as unknown as { native: { pending: unknown } }).native.pending));
}
async function releaseHeld(page: Page) {
  await page.evaluate(() => {
    const state = (window as unknown as { native: { pending: null | (() => void) } }).native;
    const respond = state.pending;
    state.pending = null;
    respond?.();
  });
}
async function requestCount(page: Page, operation: string) {
  return page.evaluate((operation) => (window as unknown as { native: { requests: { operation: string }[] } }).native.requests.filter((request) => request.operation === operation).length, operation);
}

test("held PNG owns its query lifetime and completion feedback across detail modes", async (t) => {
  const script = await bundle();
  const browser = await chromium.launch({ headless: true });
  try {
    for (const mode of ["view", "edit"]) {
      await t.test(`pending export blocks ${mode === "view" ? "Edit" : "Cancel"} through publication`, async () => {
        const page = await browser.newPage();
        try {
          const evidence = await fixture(page, script);
          if (mode === "edit") await page.evaluate(() => history.replaceState(null, "", "?mode=edit&keep=1"));
          await mount(page, note, mode);
          const before = new URL(page.url()).search;
          await native(page, { hold: true });
          await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
          await waitHeld(page);
          const navigation = page.getByRole("button", { name: mode === "view" ? "編集" : "キャンセル", exact: true });
          await navigation.evaluate((button) => (button as HTMLButtonElement).click());
          assert.equal(new URL(page.url()).search, before, "query must remain bound to the selected native token");
          assert.equal(await navigation.isDisabled(), true);
          await releaseHeld(page);
          await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).waitFor({ timeout: 3000 });
          await navigation.click();
          await page.waitForURL((url) => mode === "view" ? url.searchParams.get("mode") === "edit" : !url.searchParams.has("mode"));
          await native(page, { hold: false, cancel: true });
          await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
          await page.waitForFunction(() => (window as unknown as { native: { requests: { operation: string }[] } }).native.requests.filter((r) => r.operation === "select").length === 2);
          assert.equal(await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).count(), 0, "navigation admission releases on the observed query; new cancellation is silent");
          assert.deepEqual(evidence.errors, []);
          assert.deepEqual(evidence.mutations, []);
        } finally { await page.close(); }
      });
    }
    for (const fail of ["", "/private/RAW STACK"]) {
      await t.test(`Review retains ${fail ? "sanitized failure and retry" : "publication success"}`, async () => {
        const page = await browser.newPage();
        try {
          const evidence = await fixture(page, script);
          await mount(page, note);
          await native(page, { hold: true, fail });
          await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
          await waitHeld(page);
          await page.getByRole("button", { name: "復習", exact: true }).click();
          assert.equal(new URL(page.url()).search, "");
          assert.equal(await page.getByRole("button", { name: /PNG/ }).count(), 0, "Review has no ordinary export entry");
          await releaseHeld(page);
          if (fail) {
            const modal = page.getByRole("alertdialog", { name: "PNGを保存できませんでした" });
            await modal.waitFor({ timeout: 3000 });
            assert.equal(await modal.locator("p").textContent(), "PNGを保存できませんでした。もう一度お試しください。");
            const selections = await requestCount(page, "select");
            await native(page, { hold: false, fail: "", cancel: true });
            await modal.getByRole("button", { name: "やりなおす", exact: true }).click();
            await page.waitForFunction((count) => (window as unknown as { native: { requests: { operation: string }[] } }).native.requests.filter((r) => r.operation === "select").length === count + 1, selections);
            assert.equal(await page.getByRole("alertdialog").count(), 0);
            assert.equal(await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).count(), 0);
          } else {
            await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).waitFor({ timeout: 3000 });
          }
          assert.equal(await page.getByRole("button", { name: /PNG/ }).count(), 0);
          assert.deepEqual(evidence.errors, []);
          assert.deepEqual(evidence.mutations, []);
        } finally { await page.close(); }
      });
    }
    await t.test("Cancel remains blocked until awaited discard cleanup settles", async () => {
      const page = await browser.newPage();
      try {
        const evidence = await fixture(page, script);
        await page.evaluate(() => history.replaceState(null, "", "?mode=edit"));
        await mount(page, note, "edit");
        await native(page, { fail: "permission", holdDiscard: true });
        await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
        await waitHeld(page);
        assert.equal(await requestCount(page, "discard"), 1);
        const cancel = page.getByRole("button", { name: "キャンセル", exact: true });
        await cancel.evaluate((button) => (button as HTMLButtonElement).click());
        assert.equal(new URL(page.url()).search, "?mode=edit");
        assert.equal(await cancel.isDisabled(), true);
        assert.equal(await page.getByRole("alertdialog").count(), 0, "failure is not settled before bridge cleanup");
        await releaseHeld(page);
        await page.getByRole("alertdialog").waitFor({ timeout: 3000 });
        await page.keyboard.press("Escape");
        await cancel.click();
        await page.waitForURL((url) => !url.searchParams.has("mode"));
        assert.deepEqual(evidence.errors, []);
        assert.deepEqual(evidence.mutations, []);
      } finally { await page.close(); }
    });
    await t.test("accepted editor save exits immediately while only URL waits for old export cleanup", async () => {
      const page = await browser.newPage();
      try {
        const evidence = await fixture(page, script, true, true);
        await page.evaluate(() => history.replaceState(null, "", "?mode=edit&keep=1"));
        await mount(page, note, "edit");
        await page.getByPlaceholder("タイトルを入力", { exact: true }).fill("NEW SAVED TITLE");
        await native(page, { hold: true, fail: "permission", holdDiscard: true });
        await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
        await waitHeld(page);
        await page.getByRole("button", { name: "保存", exact: true }).click();
        await page.getByRole("heading", { name: "NEW SAVED TITLE", exact: true }).waitFor({ timeout: 3000 });
        assert.equal(await page.getByPlaceholder("タイトルを入力", { exact: true }).count(), 0, "accepted save must unmount editor immediately");
        assert.equal(new URL(page.url()).search, "?mode=edit&keep=1", "only the query change is deferred");
        assert.equal(await page.getByRole("button", { name: "PNG保存中...", exact: true }).count(), 1, "operation owner survived the editor exit");
        await releaseHeld(page);
        await waitHeld(page);
        assert.equal(await requestCount(page, "discard"), 1);
        assert.equal(new URL(page.url()).search, "?mode=edit&keep=1", "defer includes awaited cleanup");
        await releaseHeld(page);
        await page.getByRole("alertdialog").waitFor({ timeout: 3000 });
        await page.waitForURL((url) => url.search === "?keep=1");
        await page.keyboard.press("Escape");
        await native(page, { hold: false, holdDiscard: false, fail: "" });
        await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
        await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).waitFor({ timeout: 3000 });
        const names = await page.evaluate(() => (window as unknown as { native: { requests: { operation: string; suggestion?: string }[] } }).native.requests.filter((r) => r.operation === "select").map((r) => r.suggestion));
        assert.deepEqual(names, ["保存済みタイトル_2026-10-16.png", "NEW SAVED TITLE_2026-10-16.png"]);
        assert.deepEqual(evidence.errors, []);
        assert.deepEqual(evidence.mutations, ["PATCH"], "only the explicit editor save mutates the isolated note");
      } finally { await page.close(); }
    });
  } finally { await browser.close(); }
});

test("isolated actual detail/edit PNG workflow preserves drafts, reports publication only, retries with a new dialog and traps/restores focus", async () => {
  const script = await bundle();
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const calls: string[] = [];
    page.on("console", (message) => { if (message.type() === "error") console.error("PNG fixture console:", message.text()); });
    page.on("request", (request) => { if (new URL(request.url()).pathname.startsWith("/api/") && request.method() !== "GET") calls.push(request.method()); });
    const evidence = await fixture(page, script);
    await mount(page, note, "edit");
    const title = page.getByPlaceholder("タイトルを入力", { exact: true });
    await title.fill("UNSAVED TITLE");
    await page.getByText("Canvasを編集できます。", { exact: false }).waitFor();
    // Change the actual editor document, not a separate stand-in draft.
    const draftWidth = page.getByRole("spinbutton", { name: "用紙の幅（px）", exact: true });
    await draftWidth.fill("800");
    await page.getByRole("button", { name: "用紙サイズを適用", exact: true }).click();
    await page.getByText("用紙サイズ: 800 x 480 px.", { exact: false }).waitFor();
    await native(page, { hold: true });
    const action = page.getByRole("button", { name: "PNGを書き出す", exact: true });
    await action.click();
    await page.waitForFunction(() => Boolean((window as unknown as { native: { pending: unknown } }).native.pending));
    assert.equal(await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).count(), 0);
    assert.equal(await requestCount(page, "select"), 1);
    // A disabled button plus the synchronous in-flight guard prevents duplicates.
    await page.getByRole("button", { name: "PNG保存中..." }).evaluate((button) => (button as HTMLButtonElement).click());
    assert.equal(await requestCount(page, "select"), 1);
    await page.evaluate(() => (window as unknown as { native: { pending: () => void } }).native.pending());
    await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).waitFor();
    assert.equal(await title.inputValue(), "UNSAVED TITLE");
    assert.equal(await draftWidth.inputValue(), "800", "unsaved Canvas document is retained");
    const exported = await page.evaluate(() => {
      const state = (window as unknown as { native: { requests: { operation: string; suggestion?: string }[]; published: number[] } }).native;
      return { name: state.requests.find((r) => r.operation === "select")?.suggestion, bytes: state.published };
    });
    assert.equal(exported.name, "保存済みタイトル_2026-10-16.png");
    const bytes = Buffer.from(exported.bytes);
    assert.equal(bytes.readUInt32BE(16), 640); assert.equal(bytes.readUInt32BE(20), 480);
    assert.equal(calls.length, 0, "export never saves or changes note data");

    await native(page, { hold: false, fail: "permission" });
    await action.click();
    const modal = page.getByRole("alertdialog", { name: "PNGを保存できませんでした" });
    await modal.waitFor();
    assert.equal(await modal.locator("p").textContent(), "この保存先には書き込めません。別の保存先を選んでください。");
    assert.deepEqual(await modal.getByRole("button").allTextContents(), ["やりなおす", "閉じる"]);
    assert.equal(await page.getByRole("button", { name: "閉じる", exact: true }).evaluate((button) => button === document.activeElement), true);
    await page.keyboard.press("Tab");
    assert.equal(await page.getByRole("button", { name: "やりなおす", exact: true }).evaluate((button) => button === document.activeElement), true);
    await page.keyboard.press("Shift+Tab");
    assert.equal(await page.getByRole("button", { name: "閉じる", exact: true }).evaluate((button) => button === document.activeElement), true);
    const beforeRetry = await requestCount(page, "select");
    await native(page, { fail: "", cancel: true });
    await page.getByRole("button", { name: "やりなおす", exact: true }).click();
    await action.waitFor();
    assert.equal(await requestCount(page, "select"), beforeRetry + 1);
    assert.equal(await page.getByRole("alertdialog").count(), 0);
    assert.equal(await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).count(), 0, "cancellation is silent");
    assert.equal(await title.inputValue(), "UNSAVED TITLE");
    assert.equal(await draftWidth.inputValue(), "800", "unsaved Canvas document is retained");

    for (const [code, message] of [
      ["space", "保存先の空き容量が不足しています。空き容量を増やすか、別の保存先を選んでください。"],
      ["unavailable", "保存先を利用できません。接続を確認するか、別の保存先を選んでください。"],
      ["changed", "保存先のファイルが変更されています。保存先を選び直してください。"],
      ["/private/internal/path RAW STACK", "PNGを保存できませんでした。もう一度お試しください。"],
    ]) {
      await native(page, { cancel: false, fail: code });
      await action.click(); await modal.waitFor();
      assert.equal(await modal.locator("p").textContent(), message);
      assert.equal(await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).count(), 0, "failure never reports publication success");
      const beforeClose = await requestCount(page, "select");
      if (code === "space") await page.getByRole("button", { name: "閉じる", exact: true }).click();
      else await page.keyboard.press("Escape");
      await modal.waitFor({ state: "hidden" });
      assert.equal(await requestCount(page, "select"), beforeClose);
      assert.equal(await action.evaluate((button) => button === document.activeElement), true);
    }
    assert.equal(await title.inputValue(), "UNSAVED TITLE");
    assert.equal(await draftWidth.inputValue(), "800", "unsaved Canvas document is retained");
    assert.equal(calls.length, 0);

    await mount(page, note);
    const summary = page.locator("section").filter({ has: page.getByRole("heading", { name: "Summary / 要約と次の一歩", exact: true }) });
    // MarkdownReadView names inputs by task index/state, not adjacent task text.
    const checkbox = summary.getByRole("checkbox");
    assert.equal(await checkbox.count(), 1);
    assert.equal(await checkbox.getAttribute("aria-label"), "タスク 1、未完了");
    await checkbox.check();
    await native(page, { fail: "", cancel: false });
    await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
    await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).waitFor();
    assert.equal(await checkbox.isChecked(), true, "publication retains the unsaved Summary draft");
    assert.equal(await checkbox.getAttribute("aria-label"), "タスク 1、完了済み");
    await native(page, { fail: "", cancel: true });
    await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
    await page.getByRole("button", { name: "PNGを書き出す", exact: true }).waitFor();
    assert.equal(await checkbox.isChecked(), true, "unsaved Summary checkbox state is retained");
    assert.equal(await page.getByRole("status").filter({ hasText: "PNGを保存しました" }).count(), 0, "Summary export cancellation is silent");
    assert.equal(calls.length, 0);
    await page.getByRole("button", { name: "復習", exact: true }).click();
    assert.equal(await page.getByRole("button", { name: "PNGを書き出す", exact: true }).count(), 0, "review never exposes hidden body through export");

    const beforeInvalid = await requestCount(page, "select");
    await mount(page, { ...note, noteDate: null });
    await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
    await modal.waitFor();
    assert.equal(await modal.locator("p").textContent(), "PNGを保存できませんでした。もう一度お試しください。");
    assert.equal(await requestCount(page, "select"), beforeInvalid, "invalid metadata fails before dialog/render/write");
    await mount(page, { ...note, canvas: null });
    await page.getByRole("button", { name: "PNGを書き出す", exact: true }).click();
    await modal.waitFor();
    assert.equal(await modal.locator("p").textContent(), "PNGを保存できませんでした。もう一度お試しください。");
    assert.equal(await requestCount(page, "select"), beforeInvalid);
    await mount(page, { ...note, bodyMode: "markdown", canvas: null });
    assert.equal(await page.getByRole("button", { name: "PNGを書き出す", exact: true }).count(), 0);
    await mount(page, { ...note, id: "" });
    assert.equal(await page.getByRole("button", { name: "PNGを書き出す", exact: true }).count(), 0);
    const webPage = await browser.newPage();
    const webEvidence = await fixture(webPage, script, false); await mount(webPage, note);
    assert.equal(await webPage.getByRole("button", { name: "PNGを書き出す", exact: true }).count(), 0, "browser-only development offers no desktop operation");
    await webPage.close();
    assert.equal(await requestCount(page, "select"), beforeInvalid, "excluded notes never open a save dialog");
    assert.equal(calls.length, 0, "the complete workflow never sends a note mutation");
    assert.deepEqual(evidence.errors, []);
    assert.deepEqual(webEvidence.errors, []);
    assert.deepEqual(evidence.mutations, []);
    assert.deepEqual(webEvidence.mutations, []);
  } finally { await browser.close(); }
});

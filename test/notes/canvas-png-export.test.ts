import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import path from "node:path";
import ts from "typescript";

type SnapshotModule = typeof import("../../src/shared/canvas/canvas-png-snapshot");
const modules = new Map<string, { exports: unknown }>();
function loadCanvasModule(name: string): unknown {
  const file = path.resolve(import.meta.dirname, "../../src/shared/canvas", name);
  if (!file.startsWith(path.resolve(import.meta.dirname, "../../src/shared/canvas") + path.sep)) {
    throw new Error("Unexpected PNG test dependency");
  }
  const cached = modules.get(file);
  if (cached) return cached.exports;
  const moduleObject = { exports: {} };
  modules.set(file, moduleObject);
  const compiled = ts.transpileModule(readFileSync(`${file}.ts`, "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const localRequire = (request: string) => loadCanvasModule(path.relative(path.resolve(import.meta.dirname, "../../src/shared/canvas"), path.resolve(path.dirname(file), request)));
  new Function("module", "exports", "require", compiled)(moduleObject, moduleObject.exports, localRequire);
  return moduleObject.exports;
}
const { captureSavedCanvasPngSnapshot, suggestedCanvasPngFileName } = loadCanvasModule("canvas-png-snapshot") as SnapshotModule;

const canvas = {
  schemaVersion: 1,
  page: { width: 640, height: 480, background: "paper" },
  elements: [{
    id: "saved-text", type: "text", x: 10, y: 20, width: 200,
    height: 50, rotation: 0, z: 0, text: "saved", style: {},
  }],
};

function savedNote() {
  return { id: "isolated-note", bodyMode: "canvas", title: "微分の復習", noteDate: "2026-10-16", canvas: structuredClone(canvas) };
}

test("PNG snapshot detaches Canvas and metadata together without changing saved or draft state", () => {
  const saved = savedNote();
  const draft = { ...structuredClone(saved), title: "UNSAVED", noteDate: "2020-01-01" };
  draft.canvas.elements[0].text = "UNSAVED";
  const before = structuredClone({ saved, draft });
  const snapshot = captureSavedCanvasPngSnapshot(saved);
  assert.ok(snapshot);
  assert.deepEqual({ saved, draft }, before);
  saved.canvas.elements[0].text = "subsequent save";
  saved.title = "subsequent title";
  assert.equal(snapshot.canvas.elements[0].text, "saved");
  assert.equal(snapshot.title, "微分の復習");
  assert.equal(snapshot.noteDate, "2026-10-16");
  assert.equal(snapshot.fileName, "微分の復習_2026-10-16.png");
  assert.deepEqual(draft, before.draft);
});

test("Markdown is excluded; required saved metadata and invalid Canvas fail without substitution", () => {
  assert.equal(captureSavedCanvasPngSnapshot({ bodyMode: "markdown" }), null);
  for (const patch of [
    { canvas: null }, { canvas: {} }, { title: null }, { noteDate: null },
    { noteDate: "2026-02-29" }, { noteDate: "2026-10-16T00:00:00Z" },
    { noteDate: "2026-00-01" }, { noteDate: "0000-01-01" }, { id: "" },
    { bodyMode: "unknown" },
  ]) {
    assert.throws(() => captureSavedCanvasPngSnapshot({ ...savedNote(), ...patch }));
  }
  assert.equal(captureSavedCanvasPngSnapshot({ ...savedNote(), noteDate: "2024-02-29" })?.noteDate, "2024-02-29");
});

test("suggestions sanitize only the filename, retain Unicode and use the saved date-only suffix", () => {
  assert.equal(suggestedCanvasPngFileName("  A/B\\C:D\u0000\u001f\u007f...  ", "2026-10-16"), "A_B_C_D____2026-10-16.png");
  for (const title of ["", " ", ".", "...", " . . "]) {
    assert.equal(suggestedCanvasPngFileName(title, "2026-10-16"), "無題_2026-10-16.png");
  }
  assert.equal(suggestedCanvasPngFileName("学習 👩🏽‍💻 é", "2026-10-16"), "学習 👩🏽‍💻 é_2026-10-16.png");
});

test("240-byte suggestions truncate only at grapheme boundaries and retain date and extension", () => {
  for (const grapheme of ["あ", "👩🏽‍💻", "é", "🇯🇵"]) {
    const result = suggestedCanvasPngFileName(grapheme.repeat(300), "2026-10-16");
    assert.ok(Buffer.byteLength(result) <= 240);
    assert.ok(result.endsWith("_2026-10-16.png"));
    const title = result.slice(0, -"_2026-10-16.png".length);
    assert.ok(title.length > 0);
    assert.equal(title, grapheme.repeat([...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(title)].length));
    assert.ok(Buffer.byteLength(title + grapheme + "_2026-10-16.png") > 240);
  }
});

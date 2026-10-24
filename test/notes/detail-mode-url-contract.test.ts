// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-nocheck -- VM-loaded production components use focused React mocks.
import assert from "node:assert/strict";
import vm from "node:vm";
import ts from "typescript";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";

const projectRoot = path.resolve(__dirname, "../..");

function readSource(relativePath: string): string {
  return readFileSync(path.join(projectRoot, relativePath), "utf8");
}

function loadUi(relativePath, dependencies) {
  const output = ts.transpileModule(readSource(relativePath), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  const moduleObject = { exports: {} };
  vm.runInNewContext(output, {
    module: moduleObject, exports: moduleObject.exports, URLSearchParams,
    require: (name) => {
      assert.ok(name in dependencies, `unexpected dependency: ${name}`);
      return dependencies[name];
    },
  });
  return moduleObject.exports;
}

// Execute the real modes, PNG and action functions, retaining hook slots between
// renders. Calling captured callbacks directly tests admission BEFORE rerender.
function modeHarness(initialMode) {
  const slots = new Map();
  let current;
  let cursor;
  let query = initialMode === "edit" ? "mode=edit&keep=1" : "keep=1";
  const replacements = [];
  const selections = [];
  let settle;
  const saved = { id: "note", title: "saved", bodyMode: "canvas", summary: "saved summary" };
  function slot(initial) {
    const values = slots.get(current);
    const index = cursor++;
    if (!(index in values)) values[index] = initial();
    return [values, index];
  }
  const react = {
    useState(initial) {
      const [values, index] = slot(() => typeof initial === "function" ? initial() : initial);
      return [values[index], (next) => { values[index] = typeof next === "function" ? next(values[index]) : next; }];
    },
    useRef(initial) { const [values, index] = slot(() => ({ current: initial })); return values[index]; },
    useEffect() {},
    useSyncExternalStore(_subscribe, snapshot) { return snapshot(); },
  };
  const jsx = (type, props) => ({ type, props });
  const shared = { react, "react/jsx-runtime": { jsx, jsxs: jsx, Fragment: "Fragment" }, "react-dom": { createPortal: (node) => node } };
  const png = loadUi("src/modules/notes/ui/components/detail/png-export.tsx", {
    ...shared,
    "@/shared/canvas/canvas-png-snapshot": { captureSavedCanvasPngSnapshot: (note) => ({ fileName: note.title, canvas: {} }) },
    "@/shared/canvas/canvas-png-render": { renderCanvasPng: () => Promise.resolve(new Uint8Array([1])) },
    "@/shared/desktop/desktop-png-bridge": {
      canExportDesktopPng: () => true,
      desktopPngFailureMessage: () => "sanitized",
      saveDesktopCanvasPng: (name) => { selections.push(name); return new Promise((resolve) => { settle = resolve; }); },
    },
  });
  const actions = loadUi("src/modules/notes/ui/components/detail/actions.tsx", {
    ...shared, "next/link": { default: "Link" }, "../date-picker": { openDatePicker() {} },
  });
  const modes = loadUi("src/modules/notes/ui/components/detail/modes.tsx", {
    ...shared, "./png-export": png, "./actions": actions,
    "next/navigation": { useRouter: () => ({ replace: (url) => { replacements.push(url); query = url.split("?")[1] ?? ""; } }), usePathname: () => "/notes/note", useSearchParams: () => new URLSearchParams(query) },
    "@/modules/notes/remote": {}, "@/modules/notes/model": { normalizeSourceType: (value) => value },
    "@/shared/date": {}, "@/shared/desktop/desktop-close-bridge": {},
    "@/modules/notes/ui/hooks/use-note-detail-summary-draft": { useNoteDetailSummaryDraft: ({ note, onSavedNote }) => ({ summaryDraft: note.summary, isSummarySaving: () => false, discardSummaryDraft() {}, acceptSavedNote: onSavedNote }) },
    "../editor/editor": { NoteEditor: "NoteEditor" }, "./read-view": { NoteDetailReadView: "ReadView" },
  });
  function invoke(component, props) {
    const previous = current; const previousCursor = cursor;
    current = component; cursor = 0;
    if (!slots.has(component)) slots.set(component, []);
    const result = component(props);
    current = previous; cursor = previousCursor;
    return result;
  }
  function expand(node) {
    if (Array.isArray(node)) return node.flatMap(expand);
    if (!node || typeof node !== "object" || !("props" in node)) return [];
    if (typeof node.type === "function") return expand(invoke(node.type, node.props));
    return [node, ...["children", "topActions", "modeActions"].flatMap((key) => expand(node.props[key]))];
  }
  let nodes;
  function render() {
    nodes = expand(invoke(modes.NoteDetailModes, { initialNote: saved, initialMode }));
  }
  render();
  const button = (label) => nodes.find((node) => node.type === "button" && node.props.children === label);
  return { replacements, selections, render, button, editor: () => nodes.find((node) => node.type === "NoteEditor"), read: () => nodes.find((node) => node.type === "ReadView"), settle: async () => { settle("cancelled"); await new Promise((resolve) => setImmediate(resolve)); } };
}

for (const mode of ["view", "edit"]) {
  test(`actual ${mode} callbacks admit export once and block competing query navigation before rerender`, async () => {
    const harness = modeHarness(mode);
    const exportClick = harness.button("PNGを書き出す").props.onClick;
    const navigate = harness.button(mode === "view" ? "編集" : "キャンセル").props.onClick;
    exportClick(); exportClick(); navigate();
    assert.deepEqual(harness.selections, ["saved"]);
    assert.deepEqual(harness.replacements, [], "active operation blocks captured navigation callback synchronously");
    await harness.settle();
    navigate();
    assert.equal(harness.replacements.length, 1, "navigation is admitted after settlement");
  });
  test(`actual ${mode} callbacks reject export when query navigation wins the same tick`, () => {
    const harness = modeHarness(mode);
    const exportClick = harness.button("PNGを書き出す").props.onClick;
    harness.button(mode === "view" ? "編集" : "キャンセル").props.onClick();
    exportClick();
    assert.equal(harness.replacements.length, 1);
    assert.deepEqual(harness.selections, [], "captured export callback cannot use the old page/query lifetime");
  });
}

test("accepted onSaved callback accepts saved note and exits edit before old export settlement", async () => {
  const harness = modeHarness("edit");
  harness.button("PNGを書き出す").props.onClick();
  harness.editor().props.onSaved({ id: "note", title: "new saved", bodyMode: "canvas", summary: "new summary" });
  harness.render();
  assert.equal(harness.editor(), undefined, "only URL navigation may wait, not the editor exit");
  assert.equal(harness.read().props.note.title, "new saved");
  assert.deepEqual(harness.replacements, []);
  await harness.settle();
  assert.deepEqual(harness.replacements, ["/notes/note?keep=1"]);
});

test("note detail accepts only mode=edit as the initial edit mode", () => {
  const page = readSource("src/app/notes/[id]/page.tsx");

  assert.match(page, /searchParams: Promise<NoteDetailSearchParams>/);
  assert.match(page, /const \[\{ id \}, \{ mode \}\] = await Promise\.all\(\[params, searchParams\]\);/);
  assert.match(
    page,
    /typeof mode === "string" && mode === "edit" \? "edit" : "view"/,
  );
  assert.match(
    page,
    /<NoteDetailModes initialNote=\{notebook\} initialMode=\{initialMode\} \/>/,
  );
});

test("note detail edit mode is synchronized through replaceable URL state", () => {
  const modes = readSource(
    "src/modules/notes/ui/components/detail/modes.tsx",
  );

  assert.match(modes, /usePathname/);
  assert.match(modes, /useSearchParams/);
  assert.match(modes, /const \[mode, setMode\] = useState<Mode>\(initialMode\);/);
  assert.match(modes, /new URLSearchParams\(searchParams\.toString\(\)\)/);
  assert.match(modes, /nextSearchParams\.set\("mode", "edit"\)/);
  assert.match(modes, /nextSearchParams\.delete\("mode"\)/);
  assert.match(modes, /router\.replace\(query \? `\$\{pathname\}\?\$\{query\}` : pathname\);/);
  assert.match(modes, /function enterEditMode\(\)[\s\S]*replaceModeUrl\("edit"\)[\s\S]*setMode\("edit"\)/);
  assert.match(modes, /function finishEditMode\([^)]*\)[\s\S]*setMode\("view"\)[\s\S]*replaceModeUrl\("view"\)/);
  assert.match(modes, /function leaveEditMode\(\)[\s\S]*finishEditMode\(note\.summary \?\? ""\)/);
  assert.match(modes, /onEdit=\{enterEditMode\}/);
  assert.match(
    modes,
    /<NoteDetailEditActions onCancel=\{\(\) => leaveEditMode\(\)\} pngAction=\{png\.action\} disabled=\{png\.busy\} \/>/,
    "the detail cancel click must not pass its React event into leaveEditMode",
  );
  assert.doesNotMatch(
    modes,
    /<NoteDetailEditActions onCancel=\{leaveEditMode\}/,
    "the detail cancel action must use a no-argument callback",
  );
  assert.match(modes, /onCancel=\{\(\) => leaveEditMode\(\)\}/);
  assert.match(modes, /onSaved=\{\(savedNote\) => \{[\s\S]*finishEditMode\([^)]*\);/);
});

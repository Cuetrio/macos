import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import ts from "typescript";
import { test } from "node:test";

const projectRoot = path.resolve(__dirname, "../..");

function readSource(relativePath: string): string {
  return readFileSync(path.join(projectRoot, relativePath), "utf8");
}

function loadTranspiledModule(
  relativePath: string,
  dependencies: Record<string, unknown> = {},
): Record<string, unknown> {
  const output = ts.transpileModule(readSource(relativePath), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  }).outputText;
  const moduleObject: { exports: Record<string, unknown> } = { exports: {} };

  vm.runInNewContext(output, {
    module: moduleObject,
    exports: moduleObject.exports,
    require: (request: string) => {
      if (Object.prototype.hasOwnProperty.call(dependencies, request)) {
        return dependencies[request];
      }
      throw new Error(`Unexpected dependency: ${request}`);
    },
  });

  return moduleObject.exports;
}

function sourceSection(source: string, startMarker: string, endMarker: string): string {
  const start = source.indexOf(startMarker);
  assert.notEqual(start, -1, `Missing source marker: ${startMarker}`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.notEqual(end, -1, `Missing source marker: ${endMarker}`);
  return source.slice(start, end);
}

type SummaryController = {
  handleSummaryTaskToggle: (index: number, checked: boolean) => void;
  saveSummary: () => Promise<boolean>;
};

type CloseOwner = {
  save: () => Promise<boolean>;
  isDirty: () => boolean;
};

function loadSummarySaveHarness() {
  const bridge = loadTranspiledModule("src/shared/desktop/desktop-close-bridge.ts");
  const requests: Array<{ id: string; payload: unknown }> = [];
  const stateUpdates: unknown[] = [];
  const effectCleanups: Array<() => void> = [];
  const layoutEffects: Array<() => (() => void) | undefined> = [];
  const effects: Array<() => (() => void) | undefined> = [];
  const savedNotes: unknown[] = [];
  let resolveRequest: ((value: unknown) => void) | undefined;
  let rejectRequest: ((error: unknown) => void) | undefined;

  const react = {
    useState(initial: unknown) {
      let value = typeof initial === "function" ? initial() : initial;
      return [
        value,
        (next: unknown) => {
          stateUpdates.push(next);
          value = typeof next === "function" ? next(value) : next;
        },
      ];
    },
    useRef(initial: unknown) {
      return { current: initial };
    },
    useLayoutEffect(effect: () => (() => void) | undefined) {
      layoutEffects.push(effect);
    },
    useEffect(effect: () => (() => void) | undefined) {
      effects.push(effect);
    },
  };
  const note = {
    id: "note-1",
    summary: "- [ ] Remember the main idea",
  };
  const summaryDraft = loadTranspiledModule(
    "src/modules/notes/ui/hooks/use-note-detail-summary-draft.ts",
    {
      react,
      "@/modules/notes/model": {
        noteDetailToSummaryUpdatePayload: (_note: unknown, summary: string) => ({ summary }),
      },
      "@/modules/notes/remote": {
        NotesRemoteError: class NotesRemoteError extends Error {},
        updateNote: (id: string, payload: unknown) => {
          requests.push({ id, payload });
          return new Promise<unknown>((resolve, reject) => {
            resolveRequest = resolve;
            rejectRequest = reject;
          });
        },
      },
      "@/shared/markdown": {
        updateMarkdownTaskMarker: () => "- [x] Remember the main idea",
      },
      "@/shared/desktop/desktop-close-bridge": bridge,
    },
  );
  const controller = (summaryDraft.useNoteDetailSummaryDraft as (
    options: Record<string, unknown>,
  ) => Record<string, unknown>)({
    mode: "view",
    note,
    onSavedNote: (savedNote: unknown) => savedNotes.push(savedNote),
  });
  for (const effect of [...layoutEffects, ...effects]) {
    const cleanup = effect();
    if (typeof cleanup === "function") {
      effectCleanups.push(cleanup);
    }
  }

  return {
    bridge,
    controller: controller as SummaryController,
    requests,
    savedNotes,
    stateUpdates,
    resolveRequest(value: unknown) {
      assert.ok(resolveRequest);
      resolveRequest(value);
    },
    rejectRequest(error: unknown) {
      assert.ok(rejectRequest);
      rejectRequest(error);
    },
    closeOwner: (bridge.getDesktopDirtyController as () => CloseOwner)(),
    cleanup() {
      for (const cleanup of effectCleanups) {
        cleanup();
      }
    },
  };
}

const readView = readSource(
  "src/modules/notes/ui/components/detail/read-view.tsx",
);
const modes = readSource("src/modules/notes/ui/components/detail/modes.tsx");
const summaryDraft = readSource(
  "src/modules/notes/ui/hooks/use-note-detail-summary-draft.ts",
);
const actions = readSource("src/modules/notes/ui/components/detail/actions.tsx");
const payload = readSource(
  "src/modules/notes/model/detail-summary-payload.ts",
);

test("detail Summary uses the interactive read renderer in view and review", () => {
  assert.match(readView, /MarkdownReadView/);
  assert.doesNotMatch(readView, /MarkdownPreview/);
  assert.equal(
    (readView.match(/onTaskToggle=\{onSummaryTaskToggle\}/g) ?? []).length,
    2,
    "view and review should both use the interactive Summary task toggle",
  );
  assert.match(readView, /summaryDraft/);
  assert.match(readView, /summaryDirty/);
  assert.match(readView, /NoteDetailSummaryActions/);
  assert.match(readView, /onSave=\{onSaveSummary\}/);
  assert.match(readView, /onDiscard=\{onDiscardSummary\}/);
});

test("detail Summary toggle is draft-only and explicit save uses the existing note update", () => {
  const toggleBody = sourceSection(
    summaryDraft,
    "function handleSummaryTaskToggle",
    "  async function performSummarySave",
  );
  const saveBody = sourceSection(
    summaryDraft,
    "async function performSummarySave(): Promise<boolean>",
    "  function saveSummary(): Promise<boolean>",
  );
  const saveWrapperBody = sourceSection(
    summaryDraft,
    "function saveSummary(): Promise<boolean>",
    "  function acceptSavedNote",
  );
  const dirtyOwnerBody = sourceSection(
    summaryDraft,
    'useEffect(() => {\n    if (mode === "edit")',
    "  return {",
  );

  assert.match(modes, /useNoteDetailSummaryDraft/);
  assert.match(
    modes,
    /onSavedNote: \(savedNote\) =>[\s\S]*mode === "edit"[\s\S]*reviewedAt: current\.reviewedAt[\s\S]*nextReviewDate: current\.nextReviewDate/,
  );
  assert.match(modes, /onSummaryTaskToggle=\{handleSummaryTaskToggle\}/);
  assert.match(modes, /onSaveSummary=\{\(\) => void saveSummary\(\)\}/);
  assert.match(modes, /onDiscardSummary=\{\(\) => discardSummaryDraft\(\)\}/);
  assert.match(modes, /registerDesktopDirtyController/);
  assert.doesNotMatch(modes, /updateMarkdownTaskMarker/);
  assert.doesNotMatch(modes, /noteDetailToSummaryUpdatePayload/);
  assert.match(summaryDraft, /const \[summaryDraft, setSummaryDraft\] = useState\(note\.summary \?\? ""\);/);
  assert.match(summaryDraft, /const summaryDirty = summaryDraft !== \(note\.summary \?\? ""\);/);
  assert.match(summaryDraft, /const noteRef = useRef\(note\);/);
  assert.match(summaryDraft, /const summaryDraftRef = useRef\(note\.summary \?\? ""\);/);
  assert.match(summaryDraft, /const summaryDirtyRef = useRef\(false\);/);
  assert.match(
    summaryDraft,
    /const summarySaveInFlightRef = useRef<Promise<boolean> \| null>\(null\);/,
  );
  assert.match(summaryDraft, /const mountedRef = useRef\(true\);/);
  assert.match(summaryDraft, /noteRef\.current = note;/);
  assert.match(summaryDraft, /summaryDraftRef\.current = summaryDraft;/);
  assert.match(
    toggleBody,
    /const nextSummary = updateMarkdownTaskMarker\(\s*summaryDraftRef\.current,\s*taskIndex,\s*checked,\s*\);/,
  );
  assert.match(
    toggleBody,
    /summaryDirtyRef\.current = true;[\s\S]*summaryDraftRef\.current = nextSummary;[\s\S]*setSummaryDraft\(nextSummary\);/,
  );
  assert.doesNotMatch(toggleBody, /updateNote\(/);
  assert.match(summaryDraft, /const summaryRevisionRef = useRef\(0\);/);
  const cleanSaveGuard = sourceSection(
    saveBody,
    "if (!summaryDirtyRef.current)",
    "    const saveRevision",
  );
  assert.match(cleanSaveGuard, /return true;/);
  assert.doesNotMatch(
    saveBody,
    /if \(summarySavingRef\.current\)[\s\S]*return false;/,
  );
  assert.match(saveBody, /const saveRevision = summaryRevisionRef\.current;/);
  const updateCall = sourceSection(
    saveBody,
    "const savedNote = await updateNote(",
    "      if (summaryRevisionRef.current !== saveRevision)",
  );
  assert.match(
    updateCall,
    /noteRef\.current\.id,[\s\S]*noteDetailToSummaryUpdatePayload\(\s*noteRef\.current,\s*summaryDraftRef\.current,\s*\)/,
  );
  const revisionGuard = sourceSection(
    saveBody,
    "if (summaryRevisionRef.current !== saveRevision)",
    "      noteRef.current = savedNote;",
  );
  assert.match(revisionGuard, /return false;/);
  const successfulSave = sourceSection(
    saveBody,
    "noteRef.current = savedNote;",
    "    } catch (caught)",
  );
  assert.match(
    successfulSave,
    /summaryDirtyRef\.current = false;[\s\S]*return true;/,
  );
  const failedSave = sourceSection(
    saveBody,
    "    } catch (caught)",
    "    } finally",
  );
  assert.match(failedSave, /caught instanceof NotesRemoteError/);
  assert.match(failedSave, /setSummaryError\(caught\.message\)/);
  assert.match(failedSave, /summaryDirtyRef\.current = true;/);
  assert.match(failedSave, /return false;/);
  assert.match(
    saveWrapperBody,
    /return shareInFlightSummarySave\(summarySaveInFlightRef, performSummarySave\);/,
  );
  assert.match(dirtyOwnerBody, /return registerDesktopDirtyController\(\{/);
  assert.match(dirtyOwnerBody, /isDirty: \(\) => summaryDirtyRef\.current/);
  assert.match(
    dirtyOwnerBody,
    /summarySaveInFlightRef\.current \?\? summarySaveRef\.current\(\)/,
  );
  assert.match(dirtyOwnerBody, /discard: \(\) => summaryDiscardRef\.current\(\)/);
  assert.match(dirtyOwnerBody, /\}, \[mode\]\);/);
  assert.match(summaryDraft, /mountedRef\.current = false;/);
  assert.match(readView, /taskToggleDisabled=\{summarySaving\}/);
  assert.match(actions, /disabled=\{saving\}/);
  assert.match(actions, /disabled=\{disabled\}/);
  assert.match(modes, /discardSummaryDraft/);
  assert.match(payload, /title: note\.title/);
  assert.match(payload, /noteDate: note\.noteDate/);
  assert.match(payload, /const sourceType = normalizeSourceType\(note\.sourceType\);/);
  assert.match(payload, /if \(note\.sourceType !== null && sourceType === null\)/);
  assert.match(payload, /対応していない値が保存されています/);
  assert.match(payload, /sourceType: sourceType \?\? undefined/);
  assert.doesNotMatch(payload, /sourceType: normalizeSourceType\(note\.sourceType\) \?\? undefined/);
  assert.match(payload, /nextReviewDate: note\.nextReviewDate/);
  assert.match(payload, /cues: note\.cues\.map/);
  assert.match(payload, /tags: note\.tags\.map/);
  assert.match(payload, /bodyMode: "canvas"/);
  assert.match(payload, /canvas: cloneCanvasDocument\(note\.canvas\)/);
  assert.match(payload, /bodyMode: "markdown"/);
  assert.match(payload, /body: note\.body \?\? ""/);
});

test("Summary explicit save and desktop close share one PATCH and await completion", async () => {
  const harness = loadSummarySaveHarness();
  assert.ok(harness.closeOwner);
  harness.controller.handleSummaryTaskToggle(0, true);

  const explicitSave = harness.controller.saveSummary();
  const closeSave = harness.closeOwner.save();
  let closeSettled = false;
  void closeSave.then(() => {
    closeSettled = true;
  });

  assert.equal(harness.requests.length, 1);
  await Promise.resolve();
  assert.equal(closeSettled, false);

  harness.resolveRequest({
    id: "note-1",
    summary: "- [x] Remember the main idea",
  });
  assert.equal(await explicitSave, true);
  assert.equal(await closeSave, true);
  assert.equal(harness.closeOwner.isDirty(), false);
  assert.equal(harness.savedNotes.length, 1);

  harness.cleanup();
});

test("Summary close save keeps dirty state after failure and can retry", async () => {
  const harness = loadSummarySaveHarness();
  assert.ok(harness.closeOwner);
  harness.controller.handleSummaryTaskToggle(0, true);

  const explicitSave = harness.controller.saveSummary();
  const closeSave = harness.closeOwner.save();
  harness.rejectRequest(new Error("network down"));

  assert.equal(await explicitSave, false);
  assert.equal(await closeSave, false);
  assert.equal(harness.closeOwner.isDirty(), true);

  const retrySave = harness.closeOwner.save();
  assert.equal(harness.requests.length, 2);
  harness.resolveRequest({
    id: "note-1",
    summary: "- [x] Remember the main idea",
  });
  assert.equal(await retrySave, true);
  assert.equal(harness.closeOwner.isDirty(), false);

  harness.cleanup();
});

test("Summary save does not update state or parent after unmount", async () => {
  const harness = loadSummarySaveHarness();
  assert.ok(harness.closeOwner);
  harness.controller.handleSummaryTaskToggle(0, true);
  const save = harness.controller.saveSummary();
  const stateUpdatesBeforeUnmount = harness.stateUpdates.length;

  harness.cleanup();
  harness.resolveRequest({
    id: "note-1",
    summary: "- [x] Remember the main idea",
  });

  assert.equal(await save, true);
  assert.equal(harness.stateUpdates.length, stateUpdatesBeforeUnmount);
  assert.equal(harness.savedNotes.length, 0);
});

test("Summary discard does not pass the click event into the draft in view or review", () => {
  assert.match(
    summaryDraft,
    /function discardSummaryDraft\(nextSummary = noteRef\.current\.summary \?\? ""\)/,
  );
  assert.match(summaryDraft, /setSummaryDraft\(nextSummary\)/);
  assert.match(modes, /onDiscardSummary=\{\(\) => discardSummaryDraft\(\)\}/);

  assert.equal(
    (readView.match(/<NoteDetailSummaryActions/g) ?? []).length,
    2,
    "view and review should share the Summary actions component",
  );
  assert.equal(
    (readView.match(/onDiscard=\{onDiscardSummary\}/g) ?? []).length,
    2,
    "view and review should pass the same event-free discard callback",
  );

  assert.match(actions, /onClick=\{\(\) => onDiscard\(\)\}/);
  assert.doesNotMatch(actions, /onClick=\{onDiscard\}/);
});

test("Summary payload preserves valid and null source types and rejects unknown values", () => {
  const noteDisplay = loadTranspiledModule("src/modules/notes/model/note-display.ts", {
    "@/shared/date": { todayDateString: () => "2026-08-09" },
  });
  const { noteDetailToSummaryUpdatePayload } = loadTranspiledModule(
    "src/modules/notes/model/detail-summary-payload.ts",
    {
      "@/shared/canvas": { cloneCanvasDocument: (canvas: unknown) => canvas },
      "./note-display": noteDisplay,
    },
  );
  const note = (sourceType: string | null) => ({
    id: "note-1",
    title: "Title",
    noteDate: "2026-08-08",
    sourceType,
    sourceTitle: "Source",
    bodyMode: "markdown",
    body: "Body",
    canvas: null,
    summary: "Summary",
    nextReviewDate: null,
    reviewedAt: null,
    cues: [],
    tags: [],
  });

  const updateSummary = noteDetailToSummaryUpdatePayload as (
    note: Record<string, unknown>,
    summary: string,
  ) => { sourceType?: string };
  assert.equal(
    updateSummary(note("book"), "Updated").sourceType,
    "book",
  );
  assert.equal(
    updateSummary(note(null), "Updated").sourceType,
    undefined,
  );
  assert.throws(
    () => updateSummary(note("legacy-book"), "Updated"),
    /対応していない値が保存されています/,
  );
});

test("review completion remains separate from Summary save and leaves unsaved Summary discarded", () => {
  const reviewBody = modes.slice(
    modes.indexOf("async function performReviewCompletion("),
    modes.indexOf("async function deleteNote()"),
  );

  assert.match(reviewBody, /completeReview\(note\.id/);
  assert.match(reviewBody, /discardSummaryDraft\(\)/);
  assert.doesNotMatch(reviewBody, /updateNote\(/);
  assert.match(actions, /未保存の変更があります/);
  assert.match(actions, />\s*破棄\s*</);
  assert.match(actions, /saving \? "保存中\.\.\." : "保存"/);
  assert.match(actions, /role="alert"/);
});

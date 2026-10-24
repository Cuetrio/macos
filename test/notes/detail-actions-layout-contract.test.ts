import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";

const projectRoot = path.resolve(__dirname, "../..");

function readSource(relativePath: string): string {
  return readFileSync(path.join(projectRoot, relativePath), "utf8");
}

test("view and review actions are placed in the title row without duplicating the old action bar", () => {
  const actions = readSource(
    "src/modules/notes/ui/components/detail/actions.tsx",
  );
  const display = readSource(
    "src/modules/notes/ui/components/detail/display.tsx",
  );
  const readView = readSource(
    "src/modules/notes/ui/components/detail/read-view.tsx",
  );
  const metadata = readSource(
    "src/modules/notes/ui/components/editor/metadata.tsx",
  );
  const modes = readSource(
    "src/modules/notes/ui/components/detail/modes.tsx",
  );
  const paper = readSource("src/app/styles/note-paper.css");

  assert.match(actions, /className="note-paper-heading-actions"/);
  assert.match(
    metadata,
    /<div className="note-paper-heading">[\s\S]*<div className="note-paper-heading-copy min-w-0 flex-1">[\s\S]*<TitleInput[\s\S]*<\/div>[\s\S]*\{actions\}[\s\S]*<\/div>/,
  );
  assert.doesNotMatch(metadata, /note-paper-heading[^>]*!border-b-0/);
  const editActionsStart = actions.indexOf(
    "export function NoteDetailEditActions",
  );
  const reviewModeActionsStart = actions.indexOf(
    "export function NoteDetailReviewModeActions",
  );
  const editActions = actions.slice(editActionsStart, reviewModeActionsStart);
  assert.match(editActions, /<NoteDetailHeadingActions>/);
  assert.doesNotMatch(editActions, /note-paper-mode-actions/);
  assert.match(editActions, /onClick=\{onCancel\}[\s\S]*キャンセル/);
  const viewActionsStart = actions.indexOf(
    "export function NoteDetailViewActions",
  );
  const reviewModeActions = actions.slice(reviewModeActionsStart, viewActionsStart);
  assert.match(reviewModeActions, /<NoteDetailHeadingActions>/);
  assert.doesNotMatch(reviewModeActions, /note-paper-mode-actions/);
  assert.match(
    reviewModeActions,
    /onClick=\{onBackToView\}[\s\S]*閲覧へ戻る/,
  );
  const reviewActionsStart = actions.indexOf(
    "type NoteDetailReviewActionsProps",
  );
  const viewActions = actions.slice(viewActionsStart, reviewActionsStart);
  assert.doesNotMatch(viewActions, /note-paper-mode-actions/);
  assert.match(viewActions, /onClick=\{onEdit\}[\s\S]*編集/);
  assert.match(viewActions, /onClick=\{onReview\}[\s\S]*復習/);

  assert.match(
    display,
    /export function NoteDetailHeading\(\{[\s\S]*actions\?: ReactNode[\s\S]*<div className="note-paper-heading">[\s\S]*\{actions\}/,
  );
  assert.match(readView, /<NoteDetailHeading[\s\S]*actions=\{modeActions\}/);
  assert.doesNotMatch(readView, /\{mode === "review" && modeActions\}/);
  assert.equal(
    (readView.match(/actions=\{modeActions\}/g) ?? []).length,
    1,
    "view and review actions should share the title row action slot",
  );

  assert.match(modes, /onEdit=\{enterEditMode\}/);
  assert.match(
    modes,
    /import \{ addDaysToDateString, todayDateString \} from "@\/shared\/date";/,
  );
  assert.match(
    modes,
    /onReview=\{\(\) => \{[\s\S]*setShowBody\(false\);[\s\S]*setShowSummary\(false\);[\s\S]*const reviewBaseline = addDaysToDateString\(todayDateString\(\), 7\);[\s\S]*setReviewNextDate\(reviewBaseline\);[\s\S]*setMode\("review"\);/,
  );
  const reviewTransitionStart = modes.indexOf("onReview={() => {");
  const reviewTransitionEnd = modes.indexOf("          />", reviewTransitionStart);
  assert.notEqual(reviewTransitionStart, -1);
  assert.notEqual(reviewTransitionEnd, -1);
  assert.doesNotMatch(
    modes.slice(reviewTransitionStart, reviewTransitionEnd),
    /note\.nextReviewDate/,
  );
  assert.match(modes, /nextReviewDate: submittedNextReviewDate/);
  assert.match(
    modes,
    /reviewNextDate=\{reviewNextDate\}[\s\S]*onReviewNextDateChange=\{\(value\) => \{[\s\S]*setReviewNextDate\(value\);/,
  );
  assert.match(modes, /setReviewNextDate\(confirmedNextReviewDate \?\? ""\)/);
  assert.match(modes, /<NoteDetailReviewModeActions[\s\S]*onBackToView=/);
  assert.match(
    modes,
    /<NoteDetailEditActions onCancel=\{\(\) => leaveEditMode\(\)\} pngAction=\{png\.action\} disabled=\{png\.busy\} \/>/,
  );

  assert.match(
    paper,
    /\.note-paper-heading-actions\s*\{[\s\S]*min-width:\s*0;[\s\S]*flex-shrink:\s*0;[\s\S]*flex-wrap:\s*wrap;/,
  );
  assert.match(
    paper,
    /\.note-paper-heading\s*\{[\s\S]*border-bottom:\s*1px solid var\(--paper-line\);/,
  );
  assert.doesNotMatch(paper, /\.note-paper-title[^}]*border-bottom/);
  assert.match(
    paper,
    /\.note-paper-editor \.note-paper-heading \.note-paper-title:focus-visible:not\(\[aria-invalid="true"\]\)\s*\{[\s\S]*outline:\s*2px solid var\(--app-focus\);/,
  );
  const headingActionsCssStart = paper.indexOf(".note-paper-heading-actions");
  const headingActionsCssEnd = paper.indexOf("\n}", headingActionsCssStart);
  assert.notEqual(headingActionsCssStart, -1);
  assert.notEqual(headingActionsCssEnd, -1);
  assert.doesNotMatch(
    paper.slice(headingActionsCssStart, headingActionsCssEnd),
    /border|background/,
  );
  assert.match(
    paper,
    /@media \(max-width: 640px\)[\s\S]*?\.note-paper-heading\s*\{[\s\S]*flex-direction:\s*column;[\s\S]*?\.note-paper-heading-copy,[\s\S]*?\.note-paper-heading-actions\s*\{[\s\S]*width:\s*100%;/,
  );
});

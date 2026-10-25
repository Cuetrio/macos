import type { CanvasDocumentV1 } from "./canvas-document-types";
import { cloneCanvasDocument } from "./canvas-document-serialization";
import { validateCanvasDocument } from "./canvas-document-validation";

export type SavedCanvasPngSnapshot = {
  title: string;
  noteDate: string;
  canvas: CanvasDocumentV1;
  fileName: string;
};

function validDateOnly(value: unknown): value is string {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const [year, month, day] = value.split("-").map(Number);
  if (year < 1 || month < 1 || month > 12 || day < 1) return false;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return day <= days[month - 1];
}

export function suggestedCanvasPngFileName(title: string, noteDate: string) {
  if (typeof title !== "string" || !validDateOnly(noteDate)) {
    throw new Error("invalid-png-metadata");
  }
  // These transformations apply only to the suggestion, never to the note.
  const sanitized = title
    .replace(/[/\\:\u0000-\u001f\u007f]/g, "_")
    .trim()
    .replace(/[.\s]+$/u, "") || "無題";
  const suffix = `_${noteDate}.png`;
  const encoder = new TextEncoder();
  const budget = 240 - encoder.encode(suffix).length;
  let truncated = "";
  let bytes = 0;
  for (const { segment } of new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(sanitized)) {
    const size = encoder.encode(segment).length;
    if (bytes + size > budget) break;
    truncated += segment;
    bytes += size;
  }
  return `${truncated || "無題"}${suffix}`;
}

/** Call synchronously with NoteDetailModes' saved note, never an editor draft. */
export function captureSavedCanvasPngSnapshot(value: unknown): SavedCanvasPngSnapshot | null {
  if (!value || typeof value !== "object") throw new Error("invalid-png-note");
  const note = value as Record<string, unknown>;
  if (note.bodyMode === "markdown") return null;
  if (
    note.bodyMode !== "canvas" ||
    typeof note.id !== "string" || !note.id.trim() ||
    typeof note.title !== "string" || !validDateOnly(note.noteDate)
  ) {
    throw new Error("invalid-png-metadata");
  }
  const title = note.title;
  const noteDate = note.noteDate;
  const canvas = cloneCanvasDocument(validateCanvasDocument(note.canvas));
  return { title, noteDate, canvas, fileName: suggestedCanvasPngFileName(title, noteDate) };
}

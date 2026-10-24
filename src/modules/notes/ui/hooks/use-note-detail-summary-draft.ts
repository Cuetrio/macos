"use client";

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { NoteDetailResponse } from "@/modules/notes/contracts";
import { noteDetailToSummaryUpdatePayload } from "@/modules/notes/model";
import { NotesRemoteError, updateNote } from "@/modules/notes/remote";
import { updateMarkdownTaskMarker } from "@/shared/markdown";
import { registerDesktopDirtyController } from "@/shared/desktop/desktop-close-bridge";

type NoteDetailMode = "view" | "edit" | "review";

type UseNoteDetailSummaryDraftOptions = {
  mode: NoteDetailMode;
  note: NoteDetailResponse;
  onSavedNote: (savedNote: NoteDetailResponse) => void;
};

type InFlightSummarySaveRef = {
  current: Promise<boolean> | null;
};

export function shareInFlightSummarySave(
  inFlightSaveRef: InFlightSummarySaveRef,
  save: () => Promise<boolean>,
): Promise<boolean> {
  if (inFlightSaveRef.current) {
    return inFlightSaveRef.current;
  }

  const nextSave = save();
  inFlightSaveRef.current = nextSave;
  const clearInFlightSave = () => {
    if (inFlightSaveRef.current === nextSave) {
      inFlightSaveRef.current = null;
    }
  };
  void nextSave.then(clearInFlightSave, clearInFlightSave);
  return nextSave;
}

export function useNoteDetailSummaryDraft({
  mode,
  note,
  onSavedNote,
}: UseNoteDetailSummaryDraftOptions) {
  const [summaryDraft, setSummaryDraft] = useState(note.summary ?? "");
  const [summarySaving, setSummarySaving] = useState(false);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const noteRef = useRef(note);
  const summaryDraftRef = useRef(note.summary ?? "");
  const summarySavingRef = useRef(false);
  const summarySaveInFlightRef = useRef<Promise<boolean> | null>(null);
  const summaryRevisionRef = useRef(0);
  const summaryDirtyRef = useRef(false);
  const mountedRef = useRef(true);
  const summarySaveRef = useRef<() => Promise<boolean>>(() =>
    Promise.resolve(false),
  );
  const summaryDiscardRef = useRef<() => boolean>(() => false);

  const summaryDirty = summaryDraft !== (note.summary ?? "");

  function discardSummaryDraft(nextSummary = noteRef.current.summary ?? "") {
    if (summarySavingRef.current) {
      return false;
    }

    summaryRevisionRef.current += 1;
    summaryDraftRef.current = nextSummary;
    setSummaryDraft(nextSummary);
    setSummaryError(null);
    summaryDirtyRef.current = nextSummary !== (noteRef.current.summary ?? "");
    return true;
  }

  function handleSummaryTaskToggle(taskIndex: number, checked: boolean) {
    if (summarySavingRef.current) {
      return;
    }

    summaryRevisionRef.current += 1;
    summaryDirtyRef.current = true;
    const nextSummary = updateMarkdownTaskMarker(
      summaryDraftRef.current,
      taskIndex,
      checked,
    );
    summaryDraftRef.current = nextSummary;
    setSummaryDraft(nextSummary);
    setSummaryError(null);
  }

  async function performSummarySave(): Promise<boolean> {
    if (!summaryDirtyRef.current) {
      return true;
    }

    const saveRevision = summaryRevisionRef.current;
    summarySavingRef.current = true;
    if (mountedRef.current) {
      setSummarySaving(true);
      setSummaryError(null);
    }

    try {
      const savedNote = await updateNote(
        noteRef.current.id,
        noteDetailToSummaryUpdatePayload(
          noteRef.current,
          summaryDraftRef.current,
        ),
      );

      if (summaryRevisionRef.current !== saveRevision) {
        summaryDirtyRef.current = true;
        return false;
      }

      noteRef.current = savedNote;
      summaryDraftRef.current = savedNote.summary ?? "";
      summaryRevisionRef.current += 1;
      summaryDirtyRef.current = false;
      if (mountedRef.current) {
        setSummaryDraft(savedNote.summary ?? "");
        setSummaryError(null);
        onSavedNote(savedNote);
      }
      return true;
    } catch (caught) {
      summaryDirtyRef.current = true;
      if (mountedRef.current) {
        if (caught instanceof NotesRemoteError) {
          setSummaryError(caught.message);
        } else if (caught instanceof Error) {
          setSummaryError(caught.message);
        } else {
          setSummaryError(
            "サマリーの保存に失敗しました。通信状態またはAPIを確認してください。",
          );
        }
      }
      return false;
    } finally {
      summarySavingRef.current = false;
      if (mountedRef.current) {
        setSummarySaving(false);
      }
    }
  }

  function saveSummary(): Promise<boolean> {
    return shareInFlightSummarySave(summarySaveInFlightRef, performSummarySave);
  }

  function acceptSavedNote(savedNote: NoteDetailResponse) {
    if (summarySavingRef.current) {
      return false;
    }

    noteRef.current = savedNote;
    summaryDraftRef.current = savedNote.summary ?? "";
    summaryRevisionRef.current += 1;
    summaryDirtyRef.current = false;
    setSummaryDraft(savedNote.summary ?? "");
    setSummaryError(null);
    onSavedNote(savedNote);
    return true;
  }

  useLayoutEffect(() => {
    noteRef.current = note;
    summaryDraftRef.current = summaryDraft;
    summaryDirtyRef.current = summaryDirty;
    summarySaveRef.current = saveSummary;
    summaryDiscardRef.current = discardSummaryDraft;
  });

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (mode === "edit") {
      return;
    }

    return registerDesktopDirtyController({
      isDirty: () => summaryDirtyRef.current,
      save: () =>
        summarySaveInFlightRef.current ?? summarySaveRef.current(),
      discard: () => summaryDiscardRef.current(),
    });
  }, [mode]);

  return {
    summaryDraft,
    summaryDirty,
    summarySaving,
    summaryError,
    handleSummaryTaskToggle,
    saveSummary,
    discardSummaryDraft,
    isSummarySaving: () => summarySavingRef.current,
    acceptSavedNote,
  };
}

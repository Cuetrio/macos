"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import type { NoteDetailResponse } from "@/modules/notes/contracts";
import { captureSavedCanvasPngSnapshot } from "@/shared/canvas/canvas-png-snapshot";
import { renderCanvasPng } from "@/shared/canvas/canvas-png-render";
import {
  canExportDesktopPng,
  desktopPngFailureMessage,
  saveDesktopCanvasPng,
} from "@/shared/desktop/desktop-png-bridge";

function PngFailureDialog({ message, onRetry, onClose }: {
  message: string;
  onRetry: () => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
      } else if (event.key === "Tab") {
        const buttons = Array.from(dialogRef.current?.querySelectorAll("button") ?? []);
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        if (index < 0 || event.shiftKey && index === 0 || !event.shiftKey && index === buttons.length - 1) {
          event.preventDefault();
          buttons[event.shiftKey ? buttons.length - 1 : 0]?.focus();
        }
      }
    };
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.body.style.overflow = previousOverflow;
      document.removeEventListener("keydown", onKeyDown, true);
      previousFocus?.focus();
    };
  }, [onClose]);

  return createPortal(
    <div className="fixed inset-0 z-[90] grid place-items-center bg-[var(--app-overlay)] p-4" role="presentation">
      <div ref={dialogRef} role="alertdialog" aria-modal="true" aria-labelledby="png-export-title" aria-describedby="png-export-description"
        className="w-full max-w-md rounded-xl border border-[var(--app-line)] bg-[var(--app-paper-surface)] p-5 text-[var(--app-ink)] shadow-[var(--app-shadow)]">
        <h2 id="png-export-title" className="text-lg font-semibold">PNGを保存できませんでした</h2>
        <p id="png-export-description" className="mt-2 text-sm leading-6 text-[var(--app-muted-ink)]">{message}</p>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" onClick={onRetry} className="rounded-lg border border-stone-300 px-4 py-2 text-sm">やりなおす</button>
          <button ref={closeRef} type="button" onClick={onClose} className="rounded-lg border border-stone-300 px-4 py-2 text-sm">閉じる</button>
        </div>
      </div>
    </div>, document.body,
  );
}

const subscribeDesktopAvailability = () => () => {};
const serverDesktopAvailability = () => false;

/** Owned by detail modes so operation/results outlive the heading action. */
export function useCanvasPngExport(note: NoteDetailResponse, {
  canStart,
  onSettled,
}: {
  canStart: () => boolean;
  onSettled: () => void;
}) {
  const available = useSyncExternalStore(subscribeDesktopAvailability, canExportDesktopPng, serverDesktopAvailability);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);
  const inFlight = useRef(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(false);
  useEffect(() => {
    if (!failure && restoreFocus.current) {
      restoreFocus.current = false;
      triggerRef.current?.focus();
    }
  }, [failure]);
  useEffect(() => {
    if (!success) return;
    const timeout = window.setTimeout(() => setSuccess(false), 4000);
    return () => window.clearTimeout(timeout);
  }, [success]);

  async function exportPng() {
    if (inFlight.current || !canStart() || !canExportDesktopPng()) return;
    inFlight.current = true;
    setBusy(true);
    setFailure(null);
    setSuccess(false);
    try {
      const snapshot = captureSavedCanvasPngSnapshot(note);
      if (!snapshot) return;
      const result = await saveDesktopCanvasPng(snapshot.fileName, () => renderCanvasPng(snapshot.canvas));
      if (result === "published") setSuccess(true);
    } catch (error) {
      setFailure(desktopPngFailureMessage(error));
    } finally {
      // The bridge promise includes awaited discard cleanup on failure.
      inFlight.current = false;
      setBusy(false);
      onSettled();
    }
  }

  return {
    busy,
    isBusy: () => inFlight.current,
    action: available && note.bodyMode === "canvas" && note.id ? (
      <button ref={triggerRef} type="button" disabled={busy || failure !== null} aria-busy={busy}
        onClick={() => void exportPng()}
        className="rounded-lg border border-stone-300 px-3 py-1.5 text-sm font-medium text-stone-700 transition hover:bg-stone-50/60 disabled:opacity-60">
        {busy ? "PNG保存中..." : "PNGを書き出す"}
      </button>
    ) : null,
    feedback: <>
      {success ? createPortal(<div role="status" className="fixed bottom-5 right-5 z-[80] rounded-lg bg-stone-800 px-4 py-3 text-sm text-white">PNGを保存しました</div>, document.body) : null}
      {failure ? <PngFailureDialog message={failure} onRetry={() => void exportPng()} onClose={() => { restoreFocus.current = true; setFailure(null); }} /> : null}
    </>,
  };
}

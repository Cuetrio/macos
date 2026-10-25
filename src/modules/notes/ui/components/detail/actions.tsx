"use client";

import Link from "next/link";
import { createPortal } from "react-dom";
import {
  useEffect,
  useRef,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { openDatePicker } from "../date-picker";

const noteDeleteFocusableSelector =
  "button:not([disabled]), [href], [tabindex]:not([tabindex='-1'])";

function getNoteDeleteFocusableElements(container: HTMLElement) {
  return Array.from(
    container.querySelectorAll<HTMLElement>(noteDeleteFocusableSelector),
  ).filter(
    (element) =>
      !element.hasAttribute("aria-hidden") && !element.closest("[hidden]"),
  );
}

function NoteDetailHeadingActions({ children }: { children: ReactNode }) {
  return (
    <div
      className="note-paper-heading-actions"
      role="group"
      aria-label="ノートモード操作"
    >
      {children}
    </div>
  );
}

export function NoteDetailEditActions({ onCancel, pngAction, disabled = false }: { onCancel: () => void; pngAction?: ReactNode; disabled?: boolean }) {
  return (
    <NoteDetailHeadingActions>
      {pngAction}
      <button
        type="button"
        onClick={onCancel}
        disabled={disabled}
        className="rounded-lg border border-stone-300 px-3 py-1.5 text-sm font-medium text-stone-700 transition hover:bg-stone-50/60 disabled:opacity-60"
      >
        キャンセル
      </button>
    </NoteDetailHeadingActions>
  );
}

export function NoteDetailReviewModeActions({
  onBackToView,
  disabled = false,
}: {
  onBackToView: () => void;
  disabled?: boolean;
}) {
  return (
    <NoteDetailHeadingActions>
      <button
        type="button"
        disabled={disabled}
        onClick={onBackToView}
        className="rounded-lg border border-stone-300 px-3 py-1.5 text-sm font-medium text-stone-700 transition hover:bg-stone-50/60"
      >
        閲覧へ戻る
      </button>
    </NoteDetailHeadingActions>
  );
}

type NoteDetailViewModeActionsProps = {
  onEdit: () => void;
  onReview: () => void;
  disabled?: boolean;
  editDisabled?: boolean;
  pngAction?: ReactNode;
};

export function NoteDetailViewActions({
  onEdit,
  onReview,
  disabled = false,
  editDisabled = false,
  pngAction,
}: NoteDetailViewModeActionsProps) {
  return (
    <NoteDetailHeadingActions>
      <div className="flex min-w-0 flex-wrap justify-end gap-2">
        {pngAction}
        <button
          type="button"
          disabled={disabled || editDisabled}
          onClick={onEdit}
          className="rounded-lg border border-stone-300 px-3 py-1.5 text-sm font-medium text-stone-700 transition hover:bg-stone-50/60"
        >
          編集
        </button>
        <button
          type="button"
          disabled={disabled}
          onClick={onReview}
          className="rounded-lg bg-amber-600 px-3 py-1.5 text-sm font-medium text-white transition hover:bg-amber-700"
        >
          復習
        </button>
      </div>
    </NoteDetailHeadingActions>
  );
}

type NoteDetailReviewActionsProps = {
  reviewNextDate: string;
  reviewing: boolean;
  disabled?: boolean;
  reviewConfirmationComplete: boolean;
  onReviewNextDateChange: (value: string) => void;
  onSubmitReview: () => void;
};

export function NoteDetailReviewActions({
  reviewNextDate,
  reviewing,
  disabled = false,
  reviewConfirmationComplete,
  onReviewNextDateChange,
  onSubmitReview,
}: NoteDetailReviewActionsProps) {
  const submitDisabled =
    reviewing || disabled || !reviewConfirmationComplete;
  const confirmationHint = !reviewConfirmationComplete
    ? "本文を確認後、Summaryを確認すると復習済みにできます。"
    : disabled
      ? "Summaryの保存が完了するまで、復習済みにできません。"
      : "本文とSummaryを確認済みです。復習済みにできます。";

  return (
    <div className="note-paper-footer">
      <section className="min-w-0 space-y-3">
        <h2 className="note-paper-section-title">復習記録</h2>
        <div className="grid gap-4 md:grid-cols-[220px_minmax(0,1fr)] md:items-end">
          <div className="min-w-0 space-y-2">
            <label
              htmlFor="review-next-date"
              className="block text-sm font-medium text-stone-700"
            >
              次回復習日
            </label>
            <input
              id="review-next-date"
              type="date"
              value={reviewNextDate}
              disabled={reviewing || disabled}
              onClick={openDatePicker}
              onChange={(event) => onReviewNextDateChange(event.target.value)}
              className="w-full min-w-0 rounded-lg border border-stone-300/80 bg-transparent px-3 py-2 text-sm text-stone-900 shadow-none outline-none transition focus:border-amber-500 focus:ring-2 focus:ring-amber-100"
            />
          </div>
          <div className="flex flex-wrap justify-end gap-2">
            <p
              id="review-confirmation-hint"
              className="basis-full text-sm leading-6 text-[var(--app-muted-ink)]"
            >
              {confirmationHint}
            </p>
            <button
              type="button"
              disabled={submitDisabled}
              aria-describedby="review-confirmation-hint"
              onClick={onSubmitReview}
              className="inline-flex min-h-10 items-center rounded-lg border border-[var(--app-accent-deep)] bg-[var(--app-accent-deep)] px-3 py-1.5 text-sm font-medium text-[var(--app-accent-contrast)] transition hover:bg-[var(--app-accent-deep)] focus-visible:outline-2 focus-visible:outline-[var(--app-focus)] focus-visible:outline-offset-2 disabled:cursor-not-allowed disabled:border-[var(--app-line-strong)] disabled:bg-[var(--app-line)] disabled:text-[var(--app-ink)] disabled:opacity-100 disabled:shadow-none"
            >
              {reviewing ? "更新中..." : "復習済みにする"}
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}

type NoteDetailSummaryActionsProps = {
  dirty: boolean;
  saving: boolean;
  error: string | null;
  onSave: () => void;
  onDiscard: () => void;
};

export function NoteDetailSummaryActions({
  dirty,
  saving,
  error,
  onSave,
  onDiscard,
}: NoteDetailSummaryActionsProps) {
  if (!dirty && !error) {
    return null;
  }

  return (
    <div
      className="rounded-lg border border-amber-200 bg-amber-50/70 px-3 py-3"
      aria-busy={saving}
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p role="status" aria-live="polite" className="text-sm text-amber-900">
          未保存の変更があります。
        </p>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={saving}
            onClick={() => onDiscard()}
            className="rounded-lg border border-stone-300 px-3 py-1.5 text-sm font-medium text-stone-700 transition hover:bg-white disabled:cursor-not-allowed disabled:text-stone-400"
          >
            破棄
          </button>
          <button
            type="button"
            disabled={saving}
            onClick={onSave}
            className="rounded-lg bg-amber-600 px-3 py-1.5 text-sm font-medium text-white transition hover:bg-amber-700 disabled:cursor-not-allowed disabled:bg-amber-300"
          >
            {saving ? "保存中..." : "保存"}
          </button>
        </div>
      </div>
      {error && (
        <p role="alert" className="mt-2 text-sm leading-6 text-red-700">
          {error}
        </p>
      )}
    </div>
  );
}

type NoteDetailViewFooterActionsProps = {
  deleting: boolean;
  deleteConfirmationOpen: boolean;
  onDeleteIntent: () => void;
  onDeleteConfirm: () => void;
  onDeleteCancel: () => void;
};

type NoteDetailDeleteConfirmationProps = {
  deleting: boolean;
  onConfirm: () => void;
  onCancel: () => void;
};

function NoteDetailDeleteConfirmation({
  deleting,
  onConfirm,
  onCancel,
}: NoteDetailDeleteConfirmationProps) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) {
      return;
    }

    const focusableElements = getNoteDeleteFocusableElements(dialog);
    (focusableElements[0] ?? dialog).focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) {
        return;
      }

      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (!deleting) {
          onCancel();
        }
        return;
      }

      if (event.key !== "Tab") {
        return;
      }

      const currentFocusableElements = getNoteDeleteFocusableElements(dialog);
      if (currentFocusableElements.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const firstFocusableElement = currentFocusableElements[0];
      const lastFocusableElement =
        currentFocusableElements[currentFocusableElements.length - 1];
      const activeElement = document.activeElement;

      if (!currentFocusableElements.includes(activeElement as HTMLElement)) {
        event.preventDefault();
        firstFocusableElement.focus();
      } else if (
        event.shiftKey &&
        activeElement === firstFocusableElement
      ) {
        event.preventDefault();
        lastFocusableElement.focus();
      } else if (
        !event.shiftKey &&
        activeElement === lastFocusableElement
      ) {
        event.preventDefault();
        firstFocusableElement.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown, true);
    return () =>
      document.removeEventListener("keydown", handleKeyDown, true);
  }, [deleting, onCancel]);

  const handleBackdropMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (!deleting && event.target === event.currentTarget) {
      onCancel();
    }
  };

  if (typeof document === "undefined" || !document.body) {
    return null;
  }

  return createPortal(
    <div
      className="fixed inset-0 z-[90] grid place-items-center bg-[var(--app-overlay)] p-4 overscroll-contain"
      role="presentation"
      onMouseDown={handleBackdropMouseDown}
    >
      <div
        ref={dialogRef}
        className="w-full max-w-md rounded-xl border border-[var(--app-line)] bg-[var(--app-paper-surface)] p-5 text-[var(--app-ink)] shadow-[var(--app-shadow)]"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="note-delete-confirmation-title"
        aria-describedby="note-delete-confirmation-description"
        tabIndex={-1}
      >
        <h2
          id="note-delete-confirmation-title"
          className="text-lg font-semibold"
        >
          ノートを削除
        </h2>
        <p
          id="note-delete-confirmation-description"
          className="mt-2 text-sm leading-6 text-[var(--app-muted-ink)]"
        >
          このノートを削除します。よろしいですか？
        </p>
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          <button
            type="button"
            disabled={deleting}
            onClick={onCancel}
            className="inline-flex min-h-11 items-center rounded-lg border border-[var(--app-line-strong)] bg-[var(--app-paper-surface)] px-4 py-2 text-sm font-medium text-[var(--app-ink)] transition hover:bg-[var(--app-hover)] focus-visible:outline-2 focus-visible:outline-[var(--app-focus)] focus-visible:outline-offset-2 disabled:cursor-not-allowed disabled:opacity-60"
          >
            キャンセル
          </button>
          <button
            type="button"
            disabled={deleting}
            aria-busy={deleting}
            onClick={onConfirm}
            className="inline-flex min-h-11 items-center rounded-lg border border-[var(--app-danger)] bg-[var(--app-danger)] px-4 py-2 text-sm font-medium text-[var(--app-accent-contrast)] transition hover:brightness-95 focus-visible:outline-2 focus-visible:outline-[var(--app-focus)] focus-visible:outline-offset-2 disabled:cursor-not-allowed disabled:opacity-70"
          >
            {deleting ? "削除中..." : "削除する"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export function NoteDetailViewFooterActions({
  deleting,
  deleteConfirmationOpen,
  onDeleteIntent,
  onDeleteConfirm,
  onDeleteCancel,
}: NoteDetailViewFooterActionsProps) {
  const deleteTriggerRef = useRef<HTMLButtonElement>(null);
  const previousDeleteConfirmationOpenRef = useRef(deleteConfirmationOpen);

  useEffect(() => {
    if (
      previousDeleteConfirmationOpenRef.current &&
      !deleteConfirmationOpen &&
      !deleting
    ) {
      deleteTriggerRef.current?.focus();
      previousDeleteConfirmationOpenRef.current = false;
      return;
    }
    if (deleteConfirmationOpen) {
      previousDeleteConfirmationOpenRef.current = true;
    }
  }, [deleteConfirmationOpen, deleting]);

  return (
    <>
      <div className="note-paper-footer flex flex-wrap items-center justify-end gap-2">
        <Link
          href="/notes"
          className="rounded-lg border border-stone-300 px-3 py-1.5 text-sm font-medium text-stone-700 transition hover:bg-stone-50/60"
        >
          一覧へ戻る
        </Link>
        <button
          ref={deleteTriggerRef}
          type="button"
          disabled={deleting || deleteConfirmationOpen}
          aria-haspopup="dialog"
          aria-expanded={deleteConfirmationOpen}
          onClick={onDeleteIntent}
          className="rounded-lg border border-red-200 px-3 py-1.5 text-sm font-medium text-red-700 transition hover:bg-red-50/60 disabled:cursor-not-allowed disabled:text-red-300"
        >
          {deleting ? "削除中..." : "削除"}
        </button>
      </div>
      {deleteConfirmationOpen ? (
        <NoteDetailDeleteConfirmation
          deleting={deleting}
          onConfirm={onDeleteConfirm}
          onCancel={onDeleteCancel}
        />
      ) : null}
    </>
  );
}

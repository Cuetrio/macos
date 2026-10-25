"use client";

import Link from "next/link";
import { useCallback, useEffect, useRef, useState } from "react";
import type { BackupEntryDto } from "@/modules/backup/contracts";
import { createBackup, fetchBackups } from "@/modules/backup/remote";
import { BackupRemoteError } from "@/modules/backup/remote";
import {
  requestDesktopBackupRecovery,
  type DesktopBackupRecoveryReason,
  type DesktopBackupRecoveryResult,
} from "@/shared/desktop/desktop-settings-bridge";

const BACKUP_RECOVERY_REASON_BY_API_CODE = {
  backup_configuration_invalid: "backup_configuration_invalid",
  backup_database_unavailable: "backup_database_unavailable",
  backup_storage_failure: "backup_storage_failure",
} as const;

const BACKUP_RECOVERY_NAVIGATION_PREFIX =
  "#cornell-desktop-backup-recovery=";

type BackupRecoveryNavigation =
  | { status: "ready"; reason: null }
  | { status: "not-recovered"; reason: DesktopBackupRecoveryReason };

type BackupOperation = "list" | "create";

export type BackupOperationRecoveryResult<T> =
  | { kind: "success"; value: T }
  | { kind: "recovery-required" }
  | { kind: "ready-no-retry" }
  | { kind: "error"; error: unknown }
  | { kind: "stale" };

function recoveryReasonForError(
  caught: unknown,
): DesktopBackupRecoveryReason | null {
  if (!(caught instanceof BackupRemoteError)) return null;

  const code = caught.body?.code;
  if (
    code !== "backup_configuration_invalid" &&
    code !== "backup_database_unavailable" &&
    code !== "backup_storage_failure"
  ) {
    return null;
  }

  return BACKUP_RECOVERY_REASON_BY_API_CODE[code];
}

export async function runBackupOperationWithRecovery<T>(
  operation: BackupOperation,
  execute: () => Promise<T>,
  options: {
    skipRecovery?: boolean;
    isCurrent: () => boolean;
    requestRecovery: (
      reason: DesktopBackupRecoveryReason,
    ) => Promise<DesktopBackupRecoveryResult>;
  },
): Promise<BackupOperationRecoveryResult<T>> {
  try {
    return { kind: "success", value: await execute() };
  } catch (caught) {
    if (!options.isCurrent()) return { kind: "stale" };

    const reason = recoveryReasonForError(caught);
    if (options.skipRecovery || reason === null) {
      return { kind: "error", error: caught };
    }

    let recovery: DesktopBackupRecoveryResult;
    try {
      recovery = await options.requestRecovery(reason);
    } catch {
      return { kind: "error", error: caught };
    }
    if (!options.isCurrent()) return { kind: "stale" };

    if (recovery.kind === "unsupported-web") {
      return { kind: "error", error: caught };
    }
    if (recovery.status === "recovery-required") {
      return { kind: "recovery-required" };
    }
    if (recovery.status !== "ready") {
      return { kind: "error", error: caught };
    }
    if (operation === "create") {
      return { kind: "ready-no-retry" };
    }

    try {
      return { kind: "success", value: await execute() };
    } catch {
      // Keep the original stable API error and never preflight or retry again.
      return { kind: "error", error: caught };
    }
  }
}

export function consumeBackupRecoveryNavigation(): BackupRecoveryNavigation | null {
  if (
    typeof window === "undefined" ||
    window.location.pathname !== "/backup" ||
    !window.location.hash.startsWith(BACKUP_RECOVERY_NAVIGATION_PREFIX)
  ) {
    return null;
  }

  const signal = window.location.hash.slice(
    BACKUP_RECOVERY_NAVIGATION_PREFIX.length,
  );
  let navigation: BackupRecoveryNavigation | null = null;
  if (signal === "ready") {
    navigation = { status: "ready", reason: null };
  } else if (signal.startsWith("not-recovered:")) {
    const reason = signal.slice("not-recovered:".length);
    if (
      reason === "backup_configuration_invalid" ||
      reason === "backup_database_unavailable" ||
      reason === "backup_storage_failure"
    ) {
      navigation = { status: "not-recovered", reason };
    }
  }

  if (navigation !== null) {
    window.history.replaceState(
      null,
      "",
      `${window.location.pathname}${window.location.search}`,
    );
  }
  return navigation;
}

function stableBackupErrorMessage(reason: DesktopBackupRecoveryReason) {
  if (reason === "backup_storage_failure") {
    return "バックアップを作成できませんでした。バックアップの保存先を利用できない状態です。";
  }
  return "バックアップを作成できませんでした。アプリのデータを利用できない状態です。";
}

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof Error ? caught.message : fallback;
}

function formatCreatedAt(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;

  return new Intl.DateTimeFormat("ja-JP", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function BackupPage() {
  const [backups, setBackups] = useState<BackupEntryDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const backupsRequestIdRef = useRef(0);
  const createRequestIdRef = useRef(0);
  const recoveryInFlightRef = useRef<Promise<DesktopBackupRecoveryResult> | null>(
    null,
  );
  const skipRecoveryForNavigationRef = useRef(false);
  const navigationErrorRef = useRef<string | null>(null);
  const isMountedRef = useRef(false);

  const requestBackupRecovery = useCallback(async function requestBackupRecovery(
    reason: DesktopBackupRecoveryReason,
  ): Promise<DesktopBackupRecoveryResult> {
    const existing = recoveryInFlightRef.current;
    if (existing !== null) return existing;

    const request = requestDesktopBackupRecovery(reason);
    recoveryInFlightRef.current = request;
    try {
      return await request;
    } finally {
      if (recoveryInFlightRef.current === request) {
        recoveryInFlightRef.current = null;
      }
    }
  }, []);

  const loadBackups = useCallback(async (options: { skipRecovery?: boolean } = {}) => {
    const requestId = ++backupsRequestIdRef.current;
    const skipRecovery =
      options.skipRecovery ?? skipRecoveryForNavigationRef.current;

    setLoading(true);
    setError(null);

    const result = await runBackupOperationWithRecovery("list", fetchBackups, {
      skipRecovery,
      isCurrent: () => requestId === backupsRequestIdRef.current,
      requestRecovery: requestBackupRecovery,
    });

    if (result.kind === "stale" || result.kind === "recovery-required") return;
    if (result.kind === "error") {
      if (requestId !== backupsRequestIdRef.current) return;
      setError(
        errorMessage(result.error, "バックアップ一覧の取得に失敗しました。"),
      );
    } else if (result.kind === "success") {
      if (requestId !== backupsRequestIdRef.current) return;
      setBackups(result.value);
    }

    if (requestId === backupsRequestIdRef.current) {
      setLoading(false);
      if (skipRecovery) {
        skipRecoveryForNavigationRef.current = false;
      }
      if (navigationErrorRef.current !== null) {
        setError(navigationErrorRef.current);
        navigationErrorRef.current = null;
      }
    }
  }, [requestBackupRecovery]);

  useEffect(() => {
    let disposed = false;
    isMountedRef.current = true;

    async function initialize() {
      // A cancelled mount must not consume the navigation signal or start a GET.
      await Promise.resolve();
      if (disposed) return;

      const navigation = consumeBackupRecoveryNavigation();
      if (navigation?.status === "ready") {
        skipRecoveryForNavigationRef.current = true;
        setSuccess("デスクトップの復旧を試しました。もう一度お試しください。");
      } else if (navigation?.status === "not-recovered") {
        skipRecoveryForNavigationRef.current = true;
        navigationErrorRef.current = stableBackupErrorMessage(navigation.reason);
      }

      await loadBackups();
    }

    void initialize();
    return () => {
      disposed = true;
      isMountedRef.current = false;
      backupsRequestIdRef.current += 1;
      createRequestIdRef.current += 1;
    };
  }, [loadBackups]);

  async function handleCreateBackup() {
    const requestId = ++createRequestIdRef.current;
    setCreating(true);
    setError(null);
    setSuccess(null);

    const result = await runBackupOperationWithRecovery("create", createBackup, {
      isCurrent: () =>
        isMountedRef.current && requestId === createRequestIdRef.current,
      requestRecovery: requestBackupRecovery,
    });

    if (result.kind === "stale" || !isMountedRef.current) return;
    if (result.kind === "recovery-required") {
      setCreating(false);
      return;
    }
    if (result.kind === "ready-no-retry") {
      setSuccess("デスクトップの復旧を試しました。もう一度お試しください。");
    } else if (result.kind === "error") {
      setError(errorMessage(result.error, "バックアップの作成に失敗しました。"));
    } else if (result.kind === "success") {
      setSuccess(`${result.value.backup.file} を作成しました。`);
      await loadBackups();
    }
    if (isMountedRef.current && requestId === createRequestIdRef.current) {
      setCreating(false);
    }
  }

  const isEmpty = !loading && backups.length === 0;

  return (
    <div className="space-y-6">
      <div className="app-page-header flex flex-col gap-4 border-b border-[var(--app-line)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div className="min-w-0 space-y-1.5">
          <h1 className="text-2xl font-semibold text-[var(--app-ink)]">
            バックアップ
          </h1>
        </div>
        <div className="flex w-full flex-col gap-2 sm:w-auto sm:flex-row sm:flex-wrap">
          <Link
            href="/notes"
            className="inline-flex min-h-10 w-full items-center justify-center rounded-md border border-[var(--app-line)] bg-[var(--app-surface)] px-3 py-2 text-sm font-medium text-[var(--app-ink)] transition hover:bg-[var(--app-accent-soft)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--app-focus)] sm:w-auto"
          >
            ノート一覧へ
          </Link>
          <button
            type="button"
            className="inline-flex min-h-10 w-full items-center justify-center rounded-md border border-[var(--app-accent-deep)] bg-[var(--app-accent-deep)] px-4 py-2 text-sm font-semibold text-[var(--app-paper-surface)] shadow-sm transition hover:border-[var(--app-accent-deep)] hover:bg-[var(--app-accent-deep)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--app-focus)] disabled:cursor-not-allowed disabled:border-[var(--app-line)] disabled:bg-[var(--app-line)] disabled:text-[var(--app-muted-ink)] sm:w-auto"
            onClick={handleCreateBackup}
            disabled={creating || loading}
          >
            {creating ? "作成中..." : "バックアップ作成"}
          </button>
        </div>
      </div>

      {success && (
        <div
          role="status"
          className="rounded-md border border-[var(--app-line)] border-l-2 border-l-[var(--app-accent)] bg-[var(--app-surface)] px-3 py-2 text-sm font-medium text-[var(--app-accent-deep)]"
        >
          {success}
        </div>
      )}

      {error && (
        <div
          role="alert"
          className="rounded-md border border-[var(--app-line)] border-l-2 border-l-[var(--paper-danger)] bg-[var(--app-surface)] px-3 py-2 text-sm font-medium text-[var(--paper-danger)]"
        >
          {error}
        </div>
      )}

      <section className="rounded-md border border-[var(--app-line)] bg-[var(--app-paper-surface)]">
        <div className="flex flex-col gap-3 border-b border-[var(--app-line)] px-4 py-3.5 sm:flex-row sm:items-center sm:justify-between sm:px-5">
          <div className="min-w-0">
            <h2 className="text-sm font-semibold text-[var(--app-ink)]">
              最新バックアップ
            </h2>
            <p className="mt-1 text-xs text-[var(--app-muted-ink)]">
              最新3世代を保持します。
            </p>
          </div>
          <button
            type="button"
            className="inline-flex min-h-10 w-full items-center justify-center rounded-md border border-[var(--app-line)] bg-[var(--app-surface)] px-3 py-2 text-sm font-medium text-[var(--app-ink)] transition hover:border-[var(--app-line-strong)] hover:bg-[var(--app-accent-soft)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--app-focus)] disabled:cursor-not-allowed disabled:text-[var(--app-muted-ink)] sm:w-auto"
            onClick={() => void loadBackups()}
            disabled={loading || creating}
          >
            一覧更新
          </button>
        </div>

        {loading && (
          <div className="px-4 py-8 text-sm text-[var(--app-muted-ink)] sm:px-5">
            バックアップ一覧を読み込み中...
          </div>
        )}

        {isEmpty && (
          <div className="px-4 py-10 text-center sm:px-5">
            <p className="text-sm font-medium text-[var(--app-ink)]">
              バックアップはまだありません。
            </p>
            <p className="mt-1 text-sm text-[var(--app-muted-ink)]">
              バックアップ作成から現在の SQLite DB を保存できます。
            </p>
          </div>
        )}

        {!loading && backups.length > 0 && (
          <div className="divide-y divide-[var(--app-line)]">
            {backups.map((backup) => (
              <article key={backup.file} className="min-w-0 px-4 py-4 sm:px-5">
                <div className="flex min-w-0 flex-col gap-2 sm:flex-row sm:items-start sm:justify-between sm:gap-4">
                  <div className="min-w-0">
                    <h3 className="break-all text-sm font-semibold text-[var(--app-ink)]">
                      {backup.file}
                    </h3>
                    <p className="mt-1 break-all text-xs leading-5 text-[var(--app-muted-ink)]">
                      {backup.path}
                    </p>
                  </div>
                  <p className="shrink-0 whitespace-nowrap text-sm font-medium text-[var(--app-muted-ink)] sm:text-right">
                    {formatCreatedAt(backup.createdAt)}
                  </p>
                </div>
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

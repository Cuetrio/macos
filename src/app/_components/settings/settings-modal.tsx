"use client";

import { createPortal } from "react-dom";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import {
  confirmPendingRestore,
  readUpdateStateSnapshot,
  requestDataBackupExternalSource,
  requestDataBackupOperation,
  requestDataBackupSaveDestination,
  requestManagedBackupCatalog,
  requestManualUpdateCheck,
  type DesktopManualUpdateCheckResult,
  requestPendingRestoreStatus,
  type DesktopDataBackupDialogErrorCode,
  type DesktopDataBackupOperationResponse,
  type DesktopManagedBackupCatalogEntry,
  type DesktopManagedBackupCatalogErrorCode,
  type DesktopPendingRestoreResumeErrorCode,
  type DesktopPendingRestoreStatusErrorCode,
  type DesktopPendingRestoreStatusSummary,
  type DesktopUpdateStateSnapshot,
} from "@/shared/desktop/desktop-settings-bridge";
import { AppChromeIcon } from "../app-chrome-parts";
import { useTheme } from "../theme/theme-provider";
import { THEME_MODES, type ThemeMode } from "../theme/theme";
import styles from "./settings-modal.module.css";

const settingsCategories = [
  { id: "general", label: "一般" },
  { id: "data-and-backup", label: "データとバックアップ" },
] as const;

type SettingsCategoryId = (typeof settingsCategories)[number]["id"];

const focusableSelector =
  "a[href], button:not([disabled]), [tabindex]:not([tabindex='-1'])";
const DELETE_CONFIRMATION = "削除します";

function getFocusableElements(container: HTMLElement) {
  return Array.from(
    container.querySelectorAll<HTMLElement>(focusableSelector),
  ).filter(
    (element) =>
      !element.hasAttribute("aria-hidden") && !element.closest("[hidden]"),
  );
}

type DataBackupNoticeRetry =
  | "catalog"
  | "pending-status"
  | "external-source"
  | null;

type DataBackupNotice = {
  role: "status" | "alert";
  message: string;
  retry: DataBackupNoticeRetry;
};

type DataBackupCatalogState = {
  phase: "loading" | "ready" | "empty" | "error" | "unsupported";
  backups: DesktopManagedBackupCatalogEntry[];
  errorCode: DesktopManagedBackupCatalogErrorCode | null;
};

type DataBackupPendingState = {
  phase: "loading" | "none" | "available" | "invalid" | "unsupported";
  pending: DesktopPendingRestoreStatusSummary | null;
  errorCode: DesktopPendingRestoreStatusErrorCode | null;
};

type DataBackupConfirmation =
  | {
      kind: "managed";
      backup: DesktopManagedBackupCatalogEntry;
    }
  | {
      kind: "external";
      selectionId: string;
      fileName: string;
    }
  | {
      kind: "pending";
      pendingId: string;
      manifestToken: string;
      pending: DesktopPendingRestoreStatusSummary;
    }
  | {
      kind: "delete";
    };

type DataBackupOperationContext =
  | "export"
  | "managed-restore"
  | "external-restore"
  | "delete";

function restoreRetryForContext(
  context: DataBackupOperationContext,
): DataBackupNoticeRetry {
  if (context === "managed-restore") {
    return "catalog";
  }
  if (context === "external-restore") {
    return "external-source";
  }
  return null;
}

const initialDataBackupCatalogState: DataBackupCatalogState = {
  phase: "loading",
  backups: [],
  errorCode: null,
};

const initialDataBackupPendingState: DataBackupPendingState = {
  phase: "loading",
  pending: null,
  errorCode: null,
};

function formatDataBackupSize(size: number) {
  if (size < 1024) {
    return `${size} B`;
  }

  const units = ["KB", "MB", "GB", "TB"];
  let value = size;
  let unitIndex = -1;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  const rounded = value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
  return `${rounded} ${units[unitIndex]}`;
}

function formatDataBackupDate(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "日時を表示できません";
  }

  return new Intl.DateTimeFormat("ja-JP", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function managedBackupErrorMessage(
  errorCode: DesktopManagedBackupCatalogErrorCode | null,
) {
  switch (errorCode) {
    case "invalid-catalog":
      return "バックアップ一覧を確認できませんでした。もう一度読み込んでください。";
    case "storage-unavailable":
      return "バックアップを読み込めませんでした。Desktop アプリの状態を確認してください。";
    default:
      return "バックアップ一覧を読み込めませんでした。";
  }
}

function pendingStatusErrorMessage(
  errorCode: DesktopPendingRestoreStatusErrorCode | null,
) {
  switch (errorCode) {
    case "pending-invalid":
    case "pending-extra-entry":
    case "pending-multiple":
      return "保留中の復元情報を確認できません。状態を確認してから再試行してください。";
    case "pending-manifest-mismatch":
      return "保留中の復元情報が一致しません。外部ファイルまたはバックアップを再選択してください。";
    case "pending-cleanup-required":
      return "保留中の復元を整理する必要があります。Desktop アプリを確認してから再試行してください。";
    default:
      return "保留中の復元状態を読み込めませんでした。";
  }
}

function dataBackupDialogErrorMessage(
  errorCode: DesktopDataBackupDialogErrorCode | null,
) {
  switch (errorCode) {
    case "destination-name-unavailable":
      return "新しいバックアップファイル名を確定できませんでした。既存ファイルは変更されていません。もう一度保存を試してください。";
    case "path-not-directory":
      return "選択した保存先フォルダを確認できません。もう一度選択してください。";
    case "path-not-found":
    case "path-not-file":
    case "path-unavailable":
      return "選択したファイルを確認できません。もう一度選択してください。";
    case "unsupported-platform":
      return "この操作は Desktop アプリでのみ利用できます。";
    default:
      return "ファイル選択を完了できませんでした。";
  }
}

function dataBackupOperationNotice(
  result: DesktopDataBackupOperationResponse,
  context: DataBackupOperationContext,
): DataBackupNotice {
  if (result.status === "cancelled") {
    return {
      role: "status",
      message: "操作をキャンセルしました。データは変更されていません。",
      retry: null,
    };
  }

  if (result.status === "success") {
    if (context === "export" && result.result !== null) {
      return {
        role: "status",
        message: `バックアップを ${result.result.fileName} に保存しました（${formatDataBackupSize(result.result.size)}）。`,
        retry: null,
      };
    }

    if (context !== "export") {
      if (context === "delete") {
        return {
          role: "status",
          message: "アプリデータを削除しました。アプリを再起動してください。",
          retry: null,
        };
      }
      return {
        role: "status",
        message: "復元が完了しました。現在のデータを確認してください。",
        retry: null,
      };
    }

    return {
      role: "alert",
      message: "書き出し結果を確認できませんでした。",
      retry: null,
    };
  }

  switch (result.errorCode) {
    case "newer-schema-pending-required":
      return {
        role: "alert",
        message:
          "このバックアップは現在のアプリより新しい形式です。互換性のある更新後に、保留中の復元を確認してください。",
        retry: restoreRetryForContext(context),
      };
    case "integrity-check-failed":
    case "foreign-key-check-failed":
    case "schema-read-back-failed":
    case "schema-mismatch":
    case "required-data-invalid":
    case "markdown-invalid":
    case "canvas-invalid":
    case "search-text-mismatch":
    case "source-invalid":
      return {
        role: "alert",
        message:
          "選択したバックアップを検証できませんでした。別のバックアップを選択してください。",
        retry: restoreRetryForContext(context),
      };
    case "quiesce-failed":
    case "switch-failed":
    case "reopen-failed":
    case "rollback-failed":
    case "restore-failed":
      return {
        role: "alert",
        message:
          "復元を完了できませんでした。現在のデータは安全のため保護されています。状態を確認してから再試行してください。",
        retry: restoreRetryForContext(context),
      };
    case "destination-exists":
    case "publish-race":
      return {
        role: "alert",
        message:
          "別の処理が先にファイルを作成したため、バックアップを書き出せませんでした。既存ファイルは変更されていません。もう一度保存を試してください。",
        retry: null,
      };
    case "selection-not-found":
    case "selection-kind-mismatch":
    case "invalid-selection":
      return {
        role: "alert",
        message: "選択を確認できませんでした。もう一度選択してください。",
        retry: restoreRetryForContext(context),
      };
    case "layout-invalid":
    case "symlink-path":
    case "unsafe-name":
    case "unexpected-directory":
    case "special-file":
    case "preflight-failed":
    case "permission-failed":
    case "staging-conflict":
      return {
        role: "alert",
        message:
          "削除対象の構成を安全に確認できなかったため、削除を開始しませんでした。状態を確認してから再試行してください。",
        retry: context === "delete" ? null : restoreRetryForContext(context),
      };
    case "partial-delete":
    case "cleanup-required":
    case "delete-failed":
      return {
        role: "alert",
        message:
          "アプリデータの削除を完了できませんでした。安全のため処理を停止しています。アプリを再起動して状態を確認してから再試行してください。",
        retry: null,
      };
    default:
      return {
        role: "alert",
        message: "操作を完了できませんでした。",
        retry: restoreRetryForContext(context),
      };
  }
}

function pendingResumeNotice(
  errorCode: DesktopPendingRestoreResumeErrorCode | null,
): DataBackupNotice {
  switch (errorCode) {
    case "newer-schema-pending-required":
      return {
        role: "alert",
        message:
          "この復元は現在のアプリより新しい形式です。互換性のある更新後に復元を再開してください。",
        retry: "pending-status",
      };
    case "pending-invalid":
      return {
        role: "alert",
        message: "保留中の復元情報を検証できませんでした。状態を確認してから再試行してください。",
        retry: "pending-status",
      };
    case "pending-manifest-mismatch":
    case "pending-id-mismatch":
      return {
        role: "alert",
        message: "保留中の復元情報が一致しません。状態を再確認してからお試しください。",
        retry: "pending-status",
      };
    case "pending-cleanup-required":
      return {
        role: "alert",
        message: "保留中の復元を整理する必要があります。自動では変更せず、状態を確認してください。",
        retry: "pending-status",
      };
    case "quiesce-failed":
    case "switch-failed":
    case "reopen-failed":
    case "rollback-failed":
    case "restore-failed":
      return {
        role: "alert",
        message:
          "保留中の復元を完了できませんでした。保留状態を保持しています。状態を確認してから再試行してください。",
        retry: "pending-status",
      };
    default:
      return {
        role: "alert",
        message: "保留中の復元を完了できませんでした。状態を確認してから再試行してください。",
        retry: "pending-status",
      };
  }
}

function pendingSourceLabel(sourceKind: DesktopPendingRestoreStatusSummary["sourceKind"]) {
  return sourceKind === "managed-backup" ? "保存済みバックアップ" : "バックアップファイル";
}

function confirmationDescription(confirmation: DataBackupConfirmation) {
  if (confirmation.kind === "managed") {
    return `「${confirmation.backup.fileName}」を使って、現在のデータを置き換えます。`;
  }

  if (confirmation.kind === "external") {
    return `「${confirmation.fileName}」を使って、現在のデータを置き換えます。`;
  }

  return "保留中の復元を再開すると、現在のデータが置き換わります。未保存の変更は自動保存・破棄されません。";
}

function DataAndBackupPanel() {
  const [catalogState, setCatalogState] = useState<DataBackupCatalogState>(
    initialDataBackupCatalogState,
  );
  const [pendingState, setPendingState] = useState<DataBackupPendingState>(
    initialDataBackupPendingState,
  );
  const [confirmation, setConfirmation] =
    useState<DataBackupConfirmation | null>(null);
  const [deleteConfirmationText, setDeleteConfirmationText] = useState("");
  const [dialogLoading, setDialogLoading] = useState<
    "export" | "external" | null
  >(null);
  const [operationBusy, setOperationBusy] = useState(false);
  const [operationMessage, setOperationMessage] = useState<string | null>(
    null,
  );
  const [notice, setNotice] = useState<DataBackupNotice | null>(null);
  const mountedRef = useRef(true);
  const actionInFlightRef = useRef(false);
  const dialogInFlightRef = useRef(false);
  const catalogRequestInFlightRef = useRef(false);
  const pendingRequestInFlightRef = useRef(false);
  const exportSelectionIdRef = useRef<string | null>(null);
  const deleteDialogRef = useRef<HTMLDivElement>(null);
  const deleteTriggerRef = useRef<HTMLButtonElement>(null);
  const userFacingManagedBackups = catalogState.backups
    .filter((backup) => !backup.recoveryOnly)
    .slice(0, 1);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const refreshCatalog = useCallback(async () => {
    if (catalogRequestInFlightRef.current) {
      return;
    }

    catalogRequestInFlightRef.current = true;
    if (mountedRef.current) {
      setCatalogState({
        phase: "loading",
        backups: [],
        errorCode: null,
      });
      setNotice(null);
    }

    try {
      const result = await requestManagedBackupCatalog();
      if (!mountedRef.current) {
        return;
      }

      if (result.kind === "unsupported-web") {
        setCatalogState({
          phase: "unsupported",
          backups: [],
          errorCode: null,
        });
      } else if (result.status === "ready") {
        setCatalogState({
          phase: "ready",
          backups: result.backups,
          errorCode: null,
        });
      } else if (result.status === "empty") {
        setCatalogState({
          phase: "empty",
          backups: [],
          errorCode: null,
        });
      } else {
        setCatalogState({
          phase: "error",
          backups: [],
          errorCode: result.errorCode,
        });
        setNotice({
          role: "alert",
          message: managedBackupErrorMessage(result.errorCode),
          retry: "catalog",
        });
      }
    } finally {
      catalogRequestInFlightRef.current = false;
    }
  }, []);

  const refreshPendingStatus = useCallback(async () => {
    if (pendingRequestInFlightRef.current) {
      return;
    }

    pendingRequestInFlightRef.current = true;
    if (mountedRef.current) {
      setPendingState({
        phase: "loading",
        pending: null,
        errorCode: null,
      });
    }

    try {
      const result = await requestPendingRestoreStatus();
      if (!mountedRef.current) {
        return;
      }

      if (result.kind === "unsupported-web") {
        setPendingState({
          phase: "unsupported",
          pending: null,
          errorCode: null,
        });
      } else if (result.status === "none") {
        setPendingState({
          phase: "none",
          pending: null,
          errorCode: null,
        });
      } else if (result.status === "available" && result.pending !== null) {
        setPendingState({
          phase: "available",
          pending: result.pending,
          errorCode: null,
        });
      } else {
        setPendingState({
          phase: "invalid",
          pending: null,
          errorCode: result.errorCode,
        });
        setNotice({
          role: "alert",
          message: pendingStatusErrorMessage(result.errorCode),
          retry: "pending-status",
        });
      }
    } finally {
      pendingRequestInFlightRef.current = false;
    }
  }, []);

  useEffect(() => {
    void refreshCatalog();
    void refreshPendingStatus();
  }, [refreshCatalog, refreshPendingStatus]);

  const isLoading =
    catalogState.phase === "loading" || pendingState.phase === "loading";
  const isBusy = isLoading || dialogLoading !== null || operationBusy;
  const isUnsupportedWeb =
    catalogState.phase === "unsupported" || pendingState.phase === "unsupported";
  const canStartAction = !isBusy && !isUnsupportedWeb && confirmation === null;

  const runDataBackupOperation = async (
    request: Parameters<typeof requestDataBackupOperation>[0],
    context: DataBackupOperationContext,
  ) => {
    if (actionInFlightRef.current || !mountedRef.current) {
      return null;
    }

    actionInFlightRef.current = true;
    setOperationBusy(true);
    setOperationMessage(
      context === "export"
        ? "バックアップを保存中…"
        : context === "delete"
          ? "アプリデータを削除中…"
          : "復元を実行中…",
    );
    setNotice(null);

    try {
      const result = await requestDataBackupOperation(request);
      if (mountedRef.current) {
        if (result.kind === "unsupported-web") {
          setNotice({
            role: "alert",
            message: "この操作は Desktop アプリでのみ利用できます。",
            retry: null,
          });
        } else {
          setNotice(dataBackupOperationNotice(result, context));
        }
      }
      return result;
    } finally {
      actionInFlightRef.current = false;
      if (mountedRef.current) {
        setOperationBusy(false);
        setOperationMessage(null);
      }
    }
  };

  const handleExport = async () => {
    if (
      !canStartAction ||
      actionInFlightRef.current ||
      dialogInFlightRef.current
    ) {
      return;
    }

    dialogInFlightRef.current = true;
    setDialogLoading("export");
    setNotice(null);

    try {
      const result = await requestDataBackupSaveDestination();
      if (!mountedRef.current) {
        return;
      }

      if (result.kind === "unsupported-web") {
        setNotice({
          role: "alert",
          message: "この操作は Desktop アプリでのみ利用できます。",
          retry: null,
        });
        return;
      }

      if (result.status === "cancelled") {
        return;
      }

      if (result.status !== "selected" || result.selection === null) {
        setNotice({
          role: "alert",
          message: dataBackupDialogErrorMessage(result.errorCode),
          retry: null,
        });
        return;
      }

      exportSelectionIdRef.current = result.selection.selectionId;
      const selectionId = exportSelectionIdRef.current;
      if (selectionId === null) {
        return;
      }

      await runDataBackupOperation(
        {
          schemaVersion: 1,
          operation: "export",
          source: null,
          destination: {
            kind: "external-selection",
            selectionId,
          },
        },
        "export",
      );
    } finally {
      dialogInFlightRef.current = false;
      exportSelectionIdRef.current = null;
      if (mountedRef.current) {
        setDialogLoading(null);
      }
    }
  };

  const handleExternalRestoreSource = async () => {
    if (
      !canStartAction ||
      actionInFlightRef.current ||
      dialogInFlightRef.current
    ) {
      return;
    }

    dialogInFlightRef.current = true;
    setDialogLoading("external");
    setNotice(null);

    try {
      const result = await requestDataBackupExternalSource();
      if (!mountedRef.current) {
        return;
      }

      if (result.kind === "unsupported-web") {
        setNotice({
          role: "alert",
          message: "この操作は Desktop アプリでのみ利用できます。",
          retry: null,
        });
        return;
      }

      if (result.status === "cancelled") {
        return;
      }

      if (result.status !== "selected" || result.selection === null) {
        setNotice({
          role: "alert",
          message: dataBackupDialogErrorMessage(result.errorCode),
          retry: "external-source",
        });
        return;
      }

      setConfirmation({
        kind: "external",
        selectionId: result.selection.selectionId,
        fileName: result.selection.fileName,
      });
    } finally {
      dialogInFlightRef.current = false;
      if (mountedRef.current) {
        setDialogLoading(null);
      }
    }
  };

  const handleManagedRestoreIntent = (
    backup: DesktopManagedBackupCatalogEntry,
  ) => {
    if (backup.recoveryOnly || !canStartAction || actionInFlightRef.current) {
      return;
    }

    setNotice(null);
    setConfirmation({ kind: "managed", backup });
  };

  const handlePendingRestoreIntent = () => {
    if (
      !canStartAction ||
      actionInFlightRef.current ||
      pendingState.phase !== "available" ||
      pendingState.pending === null
    ) {
      return;
    }

    setNotice(null);
    setConfirmation({
      kind: "pending",
      pendingId: pendingState.pending.pendingId,
      manifestToken: pendingState.pending.manifestToken,
      pending: pendingState.pending,
    });
  };

  const handleDeleteIntent = () => {
    if (!canStartAction || actionInFlightRef.current) {
      return;
    }

    setNotice(null);
    setDeleteConfirmationText("");
    setConfirmation({ kind: "delete" });
  };

  const handleCancelConfirmation = useCallback(() => {
    if (operationBusy || confirmation === null) {
      return;
    }

    const message =
      confirmation.kind === "pending"
        ? "復元の再開を取り消しました。保留中のデータは変更していません。"
        : confirmation.kind === "delete"
          ? "削除をキャンセルしました。データは変更されていません。"
        : "復元の確認を取り消しました。データは変更されていません。";
    const wasDeleteConfirmation = confirmation.kind === "delete";
    setConfirmation(null);
    setDeleteConfirmationText("");
    if (wasDeleteConfirmation) {
      deleteTriggerRef.current?.focus();
    }
    setNotice({ role: "status", message, retry: null });
  }, [confirmation, operationBusy]);

  const handleConfirm = async () => {
    if (
      confirmation === null ||
      operationBusy ||
      actionInFlightRef.current
    ) {
      return;
    }

    const selectedConfirmation = confirmation;
    if (selectedConfirmation.kind === "delete") {
      if (deleteConfirmationText !== DELETE_CONFIRMATION) {
        return;
      }
      const result = await runDataBackupOperation(
        {
          schemaVersion: 1,
          operation: "delete",
          source: null,
          destination: null,
          confirmed: true,
        },
        "delete",
      );
      if (
        mountedRef.current &&
        result !== null &&
        result.kind !== "unsupported-web" &&
        result.status === "success"
      ) {
        setConfirmation(null);
        setDeleteConfirmationText("");
      }
      return;
    }
    if (selectedConfirmation.kind === "pending") {
      actionInFlightRef.current = true;
      setOperationBusy(true);
      setOperationMessage("保留中の復元を再開中…");
      setNotice(null);

      try {
        const result = await confirmPendingRestore(
          selectedConfirmation.pendingId,
          selectedConfirmation.manifestToken,
        );
        if (!mountedRef.current) {
          return;
        }

        if (result.kind === "unsupported-web") {
          setConfirmation(null);
          setNotice({
            role: "alert",
            message: "この操作は Desktop アプリでのみ利用できます。",
            retry: null,
          });
        } else if (result.status === "success") {
          setConfirmation(null);
          await refreshPendingStatus();
          if (mountedRef.current) {
            setNotice({
              role: "status",
              message: "復元の再開が完了しました。保留状態を更新しました。",
              retry: null,
            });
          }
        } else {
          setConfirmation(null);
          setNotice(pendingResumeNotice(result.errorCode));
        }
      } finally {
        actionInFlightRef.current = false;
        if (mountedRef.current) {
          setOperationBusy(false);
          setOperationMessage(null);
        }
      }
      return;
    }

    const source =
      selectedConfirmation.kind === "managed"
        ? {
            kind: "managed-backup" as const,
            backupId: selectedConfirmation.backup.backupId,
          }
        : {
            kind: "external-selection" as const,
            selectionId: selectedConfirmation.selectionId,
          };
    const context: DataBackupOperationContext =
      selectedConfirmation.kind === "managed"
        ? "managed-restore"
        : "external-restore";

    const result = await runDataBackupOperation(
      {
        schemaVersion: 1,
        operation: "restore",
        source,
        destination: null,
        confirmed: true,
      },
      context,
    );

    if (mountedRef.current && result !== null) {
      setConfirmation(null);
    }
  };

  useEffect(() => {
    if (confirmation?.kind !== "delete") {
      return;
    }

    const dialog = deleteDialogRef.current;
    if (!dialog) {
      return;
    }

    const focusableElements = getFocusableElements(dialog);
    (focusableElements[0] ?? dialog).focus();

    const handleDeleteDialogKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        handleCancelConfirmation();
        return;
      }

      if (event.key !== "Tab") {
        return;
      }

      const currentFocusableElements = getFocusableElements(dialog);
      if (currentFocusableElements.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const firstFocusableElement = currentFocusableElements[0];
      const lastFocusableElement =
        currentFocusableElements[currentFocusableElements.length - 1];
      if (event.shiftKey && document.activeElement === firstFocusableElement) {
        event.preventDefault();
        lastFocusableElement.focus();
      } else if (!event.shiftKey && document.activeElement === lastFocusableElement) {
        event.preventDefault();
        firstFocusableElement.focus();
      }
    };

    document.addEventListener("keydown", handleDeleteDialogKeyDown, true);
    return () =>
      document.removeEventListener("keydown", handleDeleteDialogKeyDown, true);
  }, [confirmation, handleCancelConfirmation]);

  const handleRetry = () => {
    if (notice?.retry === "catalog") {
      void refreshCatalog();
    } else if (notice?.retry === "pending-status") {
      setNotice(null);
      void refreshPendingStatus();
    } else if (notice?.retry === "external-source") {
      void handleExternalRestoreSource();
    }
  };

  const noticeContent = notice ? (
    <div
      className={`${styles.dataBackupStatus} ${
        notice.role === "alert" ? styles.dataBackupStatusError : ""
      }`}
      role={notice.role}
      aria-live="polite"
    >
      <span>{notice.message}</span>
      {notice.retry !== null ? (
        <button
          type="button"
          className={styles.dataBackupInlineButton}
          disabled={isBusy || confirmation !== null}
          onClick={handleRetry}
        >
          {notice.retry === "catalog"
            ? "バックアップ一覧を再読み込み"
            : notice.retry === "pending-status"
              ? "保留状態を再確認"
              : "外部ファイルを再選択"}
        </button>
      ) : null}
    </div>
  ) : null;

  return (
    <div className={styles.panelStack}>
      <h3>データとバックアップ</h3>

      {operationMessage !== null ? (
        <div
          className={styles.dataBackupStatus}
          role="status"
          aria-busy="true"
        >
          {operationMessage}
        </div>
      ) : null}
      {noticeContent}

      {confirmation !== null && confirmation.kind !== "delete" ? (
        <section
          className={styles.dataBackupConfirmation}
          aria-labelledby="data-backup-confirmation-title"
        >
          <p className={styles.panelKicker}>確認</p>
          <h4 id="data-backup-confirmation-title">
            {confirmation.kind === "pending"
              ? "復元の再開を確認"
              : "復元を確認"}
          </h4>
          <p>{confirmationDescription(confirmation)}</p>
          {confirmation.kind === "managed" ? (
            <dl className={styles.dataBackupMeta}>
              <div>
                <dt>ファイル</dt>
                <dd>{confirmation.backup.fileName}</dd>
              </div>
              <div>
                <dt>サイズ</dt>
                <dd>{formatDataBackupSize(confirmation.backup.size)}</dd>
              </div>
              <div>
                <dt>作成日時</dt>
                <dd>{formatDataBackupDate(confirmation.backup.createdAt)}</dd>
              </div>
            </dl>
          ) : confirmation.kind === "external" ? (
            <dl className={styles.dataBackupMeta}>
              <div>
                <dt>ファイル</dt>
                <dd>{confirmation.fileName}</dd>
              </div>
            </dl>
          ) : confirmation.kind === "pending" ? (
            <dl className={styles.dataBackupMeta}>
              <div>
                <dt>種類</dt>
                <dd>{pendingSourceLabel(confirmation.pending.sourceKind)}</dd>
              </div>
              <div>
                <dt>作成日時</dt>
                <dd>{formatDataBackupDate(confirmation.pending.createdAt)}</dd>
              </div>
              <div>
                <dt>サイズ</dt>
                <dd>
                  {formatDataBackupSize(confirmation.pending.candidateSize)}
                </dd>
              </div>
            </dl>
          ) : null}
          <div className={styles.dataBackupConfirmationActions}>
            <button
              type="button"
              className={styles.dataBackupSecondaryButton}
              disabled={operationBusy}
              onClick={handleCancelConfirmation}
            >
              キャンセル
            </button>
            <button
              type="button"
              className={styles.dataBackupButton}
              disabled={operationBusy}
              aria-busy={operationBusy}
              onClick={() => void handleConfirm()}
            >
              {confirmation.kind === "pending"
                ? "復元を再開"
                : "復元を実行"}
            </button>
          </div>
        </section>
      ) : null}

      <section className={styles.dataBackupSection} aria-labelledby="data-backup-export-title">
        <div className={styles.dataBackupSectionHeading}>
          <h4 id="data-backup-export-title">バックアップを保存</h4>
        </div>
        <p>保存先フォルダを選ぶと、選択したフォルダに新しいSQLiteバックアップを作成します。</p>
        <button
          type="button"
          className={styles.dataBackupButton}
          disabled={!canStartAction}
          aria-busy={dialogLoading === "export" || operationBusy}
          onClick={() => void handleExport()}
        >
          {dialogLoading === "export" || operationBusy
            ? "バックアップを保存中…"
            : "バックアップを保存"}
        </button>
      </section>

      <section className={styles.dataBackupSection} aria-labelledby="data-backup-managed-title">
        <div className={styles.dataBackupSectionHeading}>
          <h4 id="data-backup-managed-title">保存済みバックアップから復元</h4>
        </div>
        {catalogState.phase === "loading" ? (
          <div className={styles.dataBackupStatus} role="status" aria-busy="true">
            バックアップ一覧を読み込み中…
          </div>
        ) : catalogState.phase === "unsupported" ? (
          <div className={styles.dataBackupStatus} role="status">
            Desktop アプリでのみ利用できます。
          </div>
        ) : catalogState.phase === "error" ? (
          <div className={`${styles.dataBackupStatus} ${styles.dataBackupStatusError}`} role="alert">
            <span>{managedBackupErrorMessage(catalogState.errorCode)}</span>
            <button
              type="button"
              className={styles.dataBackupInlineButton}
              disabled={isBusy || confirmation !== null}
              onClick={() => void refreshCatalog()}
            >
              一覧を再読み込み
            </button>
          </div>
        ) : catalogState.phase === "empty" ||
          (catalogState.phase === "ready" && userFacingManagedBackups.length === 0) ? (
          <p className={styles.dataBackupEmpty} role="status">
            保存済みバックアップはありません。
          </p>
        ) : (
          <ul className={styles.dataBackupList}>
            {userFacingManagedBackups.map((backup) => (
              <li className={styles.dataBackupListItem} key={backup.backupId}>
                <dl className={styles.dataBackupMeta}>
                  <div>
                    <dt>ファイル</dt>
                    <dd>{backup.fileName}</dd>
                  </div>
                  <div>
                    <dt>サイズ</dt>
                    <dd>{formatDataBackupSize(backup.size)}</dd>
                  </div>
                  <div>
                    <dt>作成日時</dt>
                    <dd>{formatDataBackupDate(backup.createdAt)}</dd>
                  </div>
                </dl>
                <button
                  type="button"
                  className={styles.dataBackupSecondaryButton}
                  disabled={!canStartAction}
                  onClick={() => handleManagedRestoreIntent(backup)}
                >
                  このバックアップを復元
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className={styles.dataBackupSection} aria-labelledby="data-backup-external-title">
        <div className={styles.dataBackupSectionHeading}>
          <h4 id="data-backup-external-title">バックアップから復元</h4>
        </div>
        <button
          type="button"
          className={styles.dataBackupButton}
          disabled={!canStartAction}
          aria-busy={dialogLoading === "external" || operationBusy}
          onClick={() => void handleExternalRestoreSource()}
        >
          {dialogLoading === "external" ? "ファイルを選択中…" : "バックアップから復元"}
        </button>
      </section>

      <section className={styles.dataBackupSection} aria-labelledby="data-backup-pending-title">
        <div className={styles.dataBackupSectionHeading}>
          <h4 id="data-backup-pending-title">保留中の復元</h4>
        </div>
        <p>
          現在のアプリより新しい形式のバックアップはすぐに適用せず保留されます。
          互換性のあるアプリに更新した後、ここから「復元を再開」できます。
        </p>
        {pendingState.phase === "loading" ? (
          <div className={styles.dataBackupStatus} role="status" aria-busy="true">
            保留中の復元状態を読み込み中…
          </div>
        ) : pendingState.phase === "unsupported" ? (
          <div className={styles.dataBackupStatus} role="status">
            Desktop アプリでのみ利用できます。
          </div>
        ) : pendingState.phase === "invalid" ? (
          <div className={`${styles.dataBackupStatus} ${styles.dataBackupStatusError}`} role="alert">
            <span>{pendingStatusErrorMessage(pendingState.errorCode)}</span>
            <button
              type="button"
              className={styles.dataBackupInlineButton}
              disabled={isBusy || confirmation !== null}
              onClick={() => {
                setNotice(null);
                void refreshPendingStatus();
              }}
            >
              状態を再確認
            </button>
          </div>
        ) : pendingState.phase === "available" && pendingState.pending !== null ? (
          <div className={styles.dataBackupPendingCard}>
            <dl className={styles.dataBackupMeta}>
              <div>
                <dt>種類</dt>
                <dd>{pendingSourceLabel(pendingState.pending.sourceKind)}</dd>
              </div>
              <div>
                <dt>作成日時</dt>
                <dd>{formatDataBackupDate(pendingState.pending.createdAt)}</dd>
              </div>
              <div>
                <dt>サイズ</dt>
                <dd>{formatDataBackupSize(pendingState.pending.candidateSize)}</dd>
              </div>
            </dl>
            <button
              type="button"
              className={styles.dataBackupSecondaryButton}
              disabled={!canStartAction}
              onClick={handlePendingRestoreIntent}
            >
              復元を再開
            </button>
          </div>
        ) : (
          <p className={styles.dataBackupEmpty} role="status">
            保留中の復元はありません。
          </p>
        )}
      </section>

      <section
        className={`${styles.dataBackupSection} ${styles.dataBackupDangerSection}`}
        aria-labelledby="data-backup-delete-title"
      >
        <div className={styles.dataBackupSectionHeading}>
          <h4 id="data-backup-delete-title">アプリデータを削除</h4>
        </div>
        <button
          type="button"
          ref={deleteTriggerRef}
          className={styles.dataBackupDangerButton}
          disabled={!canStartAction}
          aria-busy={operationBusy}
          onClick={handleDeleteIntent}
        >
          {operationBusy ? "削除中…" : "削除"}
        </button>
      </section>

      {confirmation?.kind === "delete" && typeof document !== "undefined"
        ? createPortal(
            <div className={styles.confirmationBackdrop} role="presentation">
              <div
                ref={deleteDialogRef}
                className={styles.dataBackupConfirmation}
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="data-backup-delete-confirmation-title"
                tabIndex={-1}
              >
                <p className={styles.panelKicker}>確認</p>
                <h4 id="data-backup-delete-confirmation-title">アプリデータを削除</h4>
                <label
                  className={styles.dataBackupConfirmationField}
                  htmlFor="data-backup-delete-confirmation"
                >
                  <span>確認のため「{DELETE_CONFIRMATION}」と入力してください</span>
                  <input
                    id="data-backup-delete-confirmation"
                    type="text"
                    value={deleteConfirmationText}
                    autoComplete="off"
                    spellCheck={false}
                    aria-invalid={
                      deleteConfirmationText.length > 0 &&
                      deleteConfirmationText !== DELETE_CONFIRMATION
                    }
                    onChange={(event) => setDeleteConfirmationText(event.target.value)}
                  />
                </label>
                <div className={styles.dataBackupConfirmationActions}>
                  <button
                    type="button"
                    className={styles.dataBackupSecondaryButton}
                    disabled={operationBusy}
                    onClick={handleCancelConfirmation}
                  >
                    キャンセル
                  </button>
                  <button
                    type="button"
                    className={styles.dataBackupDangerButton}
                    disabled={
                      operationBusy || deleteConfirmationText !== DELETE_CONFIRMATION
                    }
                    aria-busy={operationBusy}
                    onClick={() => void handleConfirm()}
                  >
                    削除する
                  </button>
                </div>
              </div>
            </div>,
            document.body,
          )
        : null}

    </div>
  );
}

function SettingsCategoryPanel({
  category,
}: {
  category: SettingsCategoryId;
}) {
  if (category === "general") {
    return <GeneralPanel />;
  }

  return <DataAndBackupPanel />;
}

const themeModeLabels: Record<ThemeMode, string> = {
  light: "ライト",
  dark: "ダーク",
  system: "システム",
};

function GeneralPanel() {
  const { mode, setThemeMode } = useTheme();

  return (
    <div className={styles.panelStack}>
      <h3>一般</h3>
      <div className={styles.themeControl}>
        <label htmlFor="settings-theme">テーマ</label>
        <select
          id="settings-theme"
          value={mode}
          onChange={(event) => setThemeMode(event.target.value as ThemeMode)}
        >
          {THEME_MODES.map((themeMode) => (
            <option key={themeMode} value={themeMode}>
              {themeModeLabels[themeMode]}
            </option>
          ))}
        </select>
      </div>
      <UpdatesPanel />
    </div>
  );
}

type UpdatePanelPhase = "loading" | "idle" | "checking" | "resolved";
type UpdatePanelResultKind = DesktopManualUpdateCheckResult["kind"] | null;

type UpdatePanelState = {
  phase: UpdatePanelPhase;
  resultKind: UpdatePanelResultKind;
  snapshot: DesktopUpdateStateSnapshot | null;
};

const initialUpdatePanelState: UpdatePanelState = {
  phase: "loading",
  resultKind: null,
  snapshot: null,
};

function resultKindForSnapshot(
  snapshot: DesktopUpdateStateSnapshot,
): UpdatePanelResultKind {
  switch (snapshot.status) {
    case "no-update":
      return "no-update";
    case "available":
      return "available";
    case "failed":
      return null;
    case "checking":
      return "already-checking";
    case "not-checked":
      return null;
  }
}

function verificationStatusMessage(
  verificationState: NonNullable<
    DesktopUpdateStateSnapshot["pendingUpdate"]
  >["verificationState"],
) {
  switch (verificationState) {
    case "verified":
      return "検証済みの更新候補です。";
    case "failed":
      return "更新候補の検証に失敗しました。";
    case "not-verified":
      return "署名検証前 / 未検証です。";
  }
}

function UpdatesPanel() {
  const [updateState, setUpdateState] = useState<UpdatePanelState>(
    initialUpdatePanelState,
  );

  useEffect(() => {
    let isMounted = true;

    void readUpdateStateSnapshot().then((result) => {
      if (!isMounted) {
        return;
      }

      if (result.kind === "snapshot") {
        setUpdateState({
          phase: "idle",
          resultKind: resultKindForSnapshot(result.snapshot),
          snapshot: result.snapshot,
        });
        return;
      }

      setUpdateState({
        phase: "idle",
        resultKind: result.kind,
        snapshot: null,
      });
    });

    return () => {
      isMounted = false;
    };
  }, []);

  const handleManualUpdateCheck = async () => {
    if (
      updateState.phase === "loading" ||
      updateState.phase === "checking" ||
      updateState.resultKind === "unsupported-web"
    ) {
      return;
    }

    setUpdateState((current) => ({
      phase: "checking",
      resultKind: null,
      snapshot: current.snapshot,
    }));

    const result = await requestManualUpdateCheck();

    if (result.kind === "unsupported-web") {
      setUpdateState((current) => ({
        ...current,
        phase: "resolved",
        resultKind: result.kind,
      }));
      return;
    }

    if ("response" in result) {
      setUpdateState({
        phase: "resolved",
        resultKind: result.kind,
        snapshot: result.response.state,
      });
      return;
    }

    setUpdateState((current) => ({
      ...current,
      phase: "resolved",
      resultKind: result.kind,
    }));
  };

  const isReading = updateState.phase === "loading";
  const isChecking = updateState.phase === "checking";
  const isUnsupported = updateState.resultKind === "unsupported-web";
  const isCheckDisabled = isReading || isChecking || isUnsupported;
  const pendingVersion = updateState.snapshot?.pendingUpdate?.version;
  const verificationState = updateState.snapshot?.pendingUpdate?.verificationState;

  let statusMessage: ReactNode = null;
  let statusRole: "status" | "alert" = "status";
  let statusClassName = styles.updateStatus;

  if (isReading) {
    statusMessage = "更新状態を読み込み中…";
  } else if (isChecking) {
    statusMessage = "確認中…";
  } else {
    switch (updateState.resultKind) {
      case "no-update":
        statusMessage = "利用可能な更新はありません。";
        break;
      case "available":
        statusMessage = (
          <>
            <p>利用可能な更新があります。</p>
            {pendingVersion ? (
              <p className={styles.updateVersion}>
                利用可能なバージョン: {pendingVersion}
              </p>
            ) : (
              <p>利用可能なバージョン情報を表示できません。</p>
            )}
            {verificationState ? (
              <p>{verificationStatusMessage(verificationState)}</p>
            ) : null}
          </>
        );
        break;
      case "failed":
        statusMessage = (
          <>
            <p>更新情報を確認できませんでした。</p>
            {pendingVersion ? (
              <p className={styles.updateVersion}>
                保留中のバージョン: {pendingVersion}
              </p>
            ) : null}
            {verificationState ? (
              <p>{verificationStatusMessage(verificationState)}</p>
            ) : null}
          </>
        );
        statusRole = "alert";
        statusClassName = `${styles.updateStatus} ${styles.updateStatusError}`;
        break;
      case "suppressed":
        statusMessage = "今回は確認を実行しませんでした。";
        break;
      case "already-checking":
        statusMessage = "別の更新確認が進行中です。";
        break;
      case "unsupported-web":
        statusMessage = "Desktop アプリでのみ利用できます。";
        break;
      case "command-error":
        statusMessage = "更新情報を確認できませんでした。";
        statusRole = "alert";
        statusClassName = `${styles.updateStatus} ${styles.updateStatusError}`;
        break;
      case "state-error":
        statusMessage = "更新情報を確認できませんでした。";
        statusRole = "alert";
        statusClassName = `${styles.updateStatus} ${styles.updateStatusError}`;
        break;
      default:
        break;
    }
  }

  return (
    <div className={styles.panelStack}>
      <h3>更新</h3>
      <div className={styles.updateControls}>
        <button
          type="button"
          className={styles.updateCheckButton}
          disabled={isCheckDisabled}
          aria-busy={isChecking}
          onClick={() => void handleManualUpdateCheck()}
        >
          更新を確認
        </button>
        {statusMessage !== null &&
          (statusRole === "alert" ? (
            <div className={statusClassName} role="alert">
              {statusMessage}
            </div>
          ) : (
            <div className={statusClassName} role="status">
              {statusMessage}
            </div>
          ))}
      </div>
    </div>
  );
}

export function SettingsModal({ onClose }: { onClose: () => void }) {
  const dialogRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const tabRefs = useRef<
    Record<SettingsCategoryId, HTMLButtonElement | null>
  >({
    general: null,
    "data-and-backup": null,
  });
  const [activeCategory, setActiveCategory] =
    useState<SettingsCategoryId>("general");

  useEffect(() => {
    const previousBodyOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";

    return () => {
      document.body.style.overflow = previousBodyOverflow;
    };
  }, []);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) {
      return;
    }

    const focusableElements = getFocusableElements(dialog);
    (closeButtonRef.current ?? focusableElements[0] ?? dialog).focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) {
        return;
      }

      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }

      if (event.key !== "Tab") {
        return;
      }

      const currentFocusableElements = getFocusableElements(dialog);
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

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const handleBackdropMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (event.target === event.currentTarget) {
      onClose();
    }
  };

  const handleTabKeyDown = (
    event: ReactKeyboardEvent<HTMLButtonElement>,
    categoryId: SettingsCategoryId,
  ) => {
    const currentIndex = settingsCategories.findIndex(
      (category) => category.id === categoryId,
    );
    let nextIndex = currentIndex;

    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % settingsCategories.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      nextIndex =
        (currentIndex - 1 + settingsCategories.length) %
        settingsCategories.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = settingsCategories.length - 1;
    } else {
      return;
    }

    event.preventDefault();
    const nextCategory = settingsCategories[nextIndex];
    setActiveCategory(nextCategory.id);
    tabRefs.current[nextCategory.id]?.focus();
  };

  const modal = (
    <div
      className={styles.backdrop}
      role="presentation"
      onMouseDown={handleBackdropMouseDown}
    >
      <section
        ref={dialogRef}
        className={styles.dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-modal-title"
        tabIndex={-1}
      >
        <header className={styles.header}>
          <div>
            <h2 id="settings-modal-title">設定</h2>
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            className={styles.closeButton}
            aria-label="設定を閉じる"
            onClick={onClose}
          >
            <AppChromeIcon name="close" className={styles.closeIcon} />
          </button>
        </header>

        <div className={styles.body}>
          <div
            className={styles.categoryNavigation}
            role="tablist"
            aria-label="設定カテゴリ"
          >
            {settingsCategories.map((category) => {
              const isActive = activeCategory === category.id;
              return (
                <button
                  key={category.id}
                  ref={(element) => {
                    tabRefs.current[category.id] = element;
                  }}
                  type="button"
                  className={styles.tab}
                  role="tab"
                  id={`settings-tab-${category.id}`}
                  aria-selected={isActive}
                  aria-controls={`settings-panel-${category.id}`}
                  tabIndex={isActive ? 0 : -1}
                  onClick={() => setActiveCategory(category.id)}
                  onKeyDown={(event) =>
                    handleTabKeyDown(event, category.id)
                  }
                >
                  {category.label}
                </button>
              );
            })}
          </div>

          <div className={styles.panelRegion}>
            {settingsCategories.map((category) => {
              const isActive = activeCategory === category.id;
              return (
                <div
                  key={category.id}
                  id={`settings-panel-${category.id}`}
                  className={styles.panel}
                  role="tabpanel"
                  aria-labelledby={`settings-tab-${category.id}`}
                  tabIndex={isActive ? 0 : -1}
                  hidden={!isActive}
                >
                  <SettingsCategoryPanel
                    category={category.id}
                  />
                </div>
              );
            })}
          </div>
        </div>
      </section>
    </div>
  );

  if (typeof document === "undefined" || !document.body) {
    return null;
  }

  return createPortal(modal, document.body);
}

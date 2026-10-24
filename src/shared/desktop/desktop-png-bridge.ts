"use client";

const FRAGMENT = "cornell-desktop-png=";
const RESULT_EVENT = "cornell:desktop-png-result";
const CHUNK_BYTES = 32 * 1024;
const MAX_PNG_BYTES = 64 * 1024 * 1024;

type PngResponse = {
  id: string;
  status: "selected" | "cancelled" | "appended" | "published" | "discarded" | "error";
  token: string | null;
  code: string | null;
};

export class DesktopPngError extends Error {
  constructor(readonly code: string) {
    super("PNG export failed");
  }
}

export function canExportDesktopPng() {
  return typeof window !== "undefined" &&
    (window as Window & { __CORNELL_PNG_DESKTOP__?: boolean }).__CORNELL_PNG_DESKTOP__ === true &&
    window.location.protocol === "http:" && window.location.hostname === "127.0.0.1" &&
    /^\/notes\/(?!new$)[^/]+$/.test(window.location.pathname);
}

function base64(bytes: Uint8Array) {
  return btoa(String.fromCharCode(...bytes));
}

let requesting = false;
function request(operation: Record<string, unknown>): Promise<PngResponse> {
  if (!canExportDesktopPng() || requesting) return Promise.reject(new DesktopPngError("unknown"));
  requesting = true;
  return new Promise((resolve, reject) => {
    const id = crypto.randomUUID();
    const fragment = FRAGMENT + base64(new TextEncoder().encode(JSON.stringify({ ...operation, id })));
    const finish = (response?: PngResponse) => {
      window.clearTimeout(timeout);
      window.removeEventListener(RESULT_EVENT, handleResult);
      if (window.location.hash === `#${fragment}`) {
        window.history.replaceState(null, "", window.location.pathname + window.location.search);
      }
      requesting = false;
      if (!response || response.status === "error") reject(new DesktopPngError(response?.code ?? "unknown"));
      else resolve(response);
    };
    const handleResult = (event: Event) => {
      const value: unknown = (event as CustomEvent).detail;
      if (!value || typeof value !== "object") return;
      const response = value as PngResponse;
      if (response.id !== id) return;
      if (!["selected", "cancelled", "appended", "published", "discarded", "error"].includes(response.status)) return;
      finish(response);
    };
    // Allow the user time to use the native save/replace dialogs.
    const timeout = window.setTimeout(() => finish(), 15 * 60 * 1000);
    window.addEventListener(RESULT_EVENT, handleResult);
    try {
      window.location.hash = fragment;
    } catch {
      finish();
    }
  });
}

let exportInFlight = false;

/** Paths and overwrite approval never cross into the renderer. */
export async function saveDesktopCanvasPng(
  suggestion: string,
  generate: () => Promise<Uint8Array<ArrayBuffer>>,
): Promise<"published" | "cancelled"> {
  if (exportInFlight) throw new DesktopPngError("unknown");
  exportInFlight = true;
  let token: string | null = null;
  try {
    const selection = await request({ operation: "select", suggestion });
    if (selection.status === "cancelled") return "cancelled";
    if (selection.status !== "selected" || !selection.token || !/^[a-f0-9]{48}$/.test(selection.token)) {
      throw new DesktopPngError("unknown");
    }
    token = selection.token;
    const bytes = await generate();
    if (!bytes.length || bytes.length > MAX_PNG_BYTES) throw new DesktopPngError("unknown");
    for (let offset = 0; offset < bytes.length; offset += CHUNK_BYTES) {
      const response = await request({ operation: "append", token, offset, data: base64(bytes.subarray(offset, offset + CHUNK_BYTES)) });
      if (response.status !== "appended") throw new DesktopPngError("unknown");
    }
    const response = await request({ operation: "publish", token });
    if (response.status !== "published") throw new DesktopPngError("unknown");
    return "published";
  } catch (error) {
    // Never keep a prior selection/approval for a retry, including generation failure.
    if (token) {
      try { await request({ operation: "discard", token }); } catch { /* Native tokens also expire. */ }
    }
    throw error;
  } finally {
    exportInFlight = false;
  }
}

export function desktopPngFailureMessage(error: unknown) {
  const code = error instanceof DesktopPngError ? error.code : "unknown";
  switch (code) {
    case "permission": return "この保存先には書き込めません。別の保存先を選んでください。";
    case "space": return "保存先の空き容量が不足しています。空き容量を増やすか、別の保存先を選んでください。";
    case "unavailable": return "保存先を利用できません。接続を確認するか、別の保存先を選んでください。";
    case "changed": return "保存先のファイルが変更されています。保存先を選び直してください。";
    default: return "PNGを保存できませんでした。もう一度お試しください。";
  }
}

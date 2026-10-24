"use client";

import { useEffect, useRef, useState } from "react";
import { useTheme } from "@/app/_components/theme/theme-provider";
import {
  cloneCanvasDocument,
  createEmptyCanvasDocument,
  type CanvasDocumentV1,
} from "@/shared/canvas";
import {
  applyCanvasSurfaceDimensions,
  fabricDocumentToCanvas,
  type FabricApiLike,
  type FabricCanvasLike,
} from "@/shared/canvas/adapters/fabric";
import { NoteCanvasSurface } from "./surface";

type NoteCanvasViewerProps = {
  document: CanvasDocumentV1 | null;
};

const EMPTY_CANVAS_DOCUMENT = createEmptyCanvasDocument();

function errorMessage(error: unknown) {
  return error instanceof Error && error.message
    ? error.message
    : "Canvasの初期化に失敗しました。保存済みデータは変更されていません。";
}

export function NoteCanvasViewer({ document }: NoteCanvasViewerProps) {
  const { resolvedTheme } = useTheme();
  const [initialDocumentSnapshot] = useState<CanvasDocumentV1 | null>(() => {
    if (!document) return null;
    try {
      return cloneCanvasDocument(document);
    } catch {
      return null;
    }
  });

  const validDocument = initialDocumentSnapshot ?? EMPTY_CANVAS_DOCUMENT;
  const pageWidth = validDocument.page.width;
  const pageHeight = validDocument.page.height;
  const viewportRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const canvasElementRef = useRef<HTMLCanvasElement>(null);
  const canvasRef = useRef<FabricCanvasLike | null>(null);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(
    !document
      ? "Canvas documentがありません。保存済みデータは変更されていません。"
      : initialDocumentSnapshot === null
        ? "Canvas documentが壊れているため表示できません。保存済みデータは変更されていません。"
        : null,
  );

  useEffect(() => {
    let disposed = false;
    let canvas: FabricCanvasLike | null = null;

    async function initialize() {
      if (initialDocumentSnapshot === null) return;

      try {
        const fabricModule = (await import("fabric")) as unknown as FabricApiLike;
        const element = canvasElementRef.current;
        if (!element || disposed) return;

        const nextCanvas = new fabricModule.Canvas(element, {
          preserveObjectStacking: true,
          selection: false,
          enablePointerEvents: false,
          allowTouchScrolling: true,
        });
        canvas = nextCanvas;
        canvasRef.current = nextCanvas;
        nextCanvas.backgroundColor = window
          .getComputedStyle(surfaceRef.current ?? element)
          .getPropertyValue("--app-paper-surface")
          .trim() || "#fffdf8";
        nextCanvas.selection = false;
        nextCanvas.isDrawingMode = false;
        const validated = cloneCanvasDocument(validDocument);
        fabricDocumentToCanvas(nextCanvas, fabricModule, validated);
        nextCanvas.getObjects().forEach((object) => {
          object.set({ selectable: false, evented: false });
        });
        nextCanvas.discardActiveObject();
        applyCanvasSurfaceDimensions(
          {
            canvas: nextCanvas,
            canvasElement: element,
            surface: surfaceRef.current,
          },
          validated.page,
        );
        nextCanvas.renderAll();
        setReady(true);
        setError(null);
      } catch (caught) {
        if (!disposed) setError(errorMessage(caught));
      }
    }

    void initialize();
    return () => {
      disposed = true;
      if (canvas) void canvas.dispose();
      canvas = null;
      canvasRef.current = null;
    };
  }, [initialDocumentSnapshot, pageHeight, pageWidth, validDocument]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const surface = surfaceRef.current;
    if (!canvas || !surface) {
      return;
    }

    canvas.backgroundColor = window
      .getComputedStyle(surface)
      .getPropertyValue("--app-paper-surface")
      .trim() || (resolvedTheme === "dark" ? "#2a302b" : "#fffdf8");
    canvas.requestRenderAll?.();
  }, [resolvedTheme]);

  useEffect(() => {
    const surface = surfaceRef.current;
    const canvas = canvasRef.current;
    if (!surface || !canvas) return;

    applyCanvasSurfaceDimensions(
      {
        canvas,
        canvasElement: canvasElementRef.current,
        surface,
      },
      { width: pageWidth, height: pageHeight },
    );
  }, [pageHeight, pageWidth, ready]);

  const textElements = validDocument.elements
    .filter(
      (element) =>
        (element.type === "text" ||
          element.type === "rect" ||
          element.type === "ellipse") &&
        element.text?.trim(),
    )
    .sort((a, b) => a.z - b.z);

  return (
    <div className="note-canvas-viewer" aria-label="保存済みCanvas本文">
      {error && (
        <p className="note-canvas-error" role="alert">
          {error}
        </p>
      )}

      {initialDocumentSnapshot !== null && (
        <NoteCanvasSurface
          mode="viewer"
          pageDimensions={{ width: pageWidth, height: pageHeight }}
          viewportRef={viewportRef}
          surfaceRef={surfaceRef}
          canvasElementRef={canvasElementRef}
          tabIndex={0}
          viewportAriaLabel={`保存済みCanvas本文。用紙サイズ ${pageWidth} x ${pageHeight} px。図形、線、ストローク、テキストを含みます。`}
          canvasAriaLabel={`保存済み${pageWidth} x ${pageHeight} Canvas`}
        />
      )}

      <div className="note-canvas-assistive-text">
        {ready ? "保存済みCanvasを表示しています。" : "Canvasを準備しています。"}
        {` 用紙サイズ: ${pageWidth} x ${pageHeight} px。`}
        {textElements.length > 0 ? (
          <ul>
            {textElements.map((element) => (
              <li key={element.id}>{element.text}</li>
            ))}
          </ul>
        ) : (
          <span> テキスト要素はありません。</span>
        )}
      </div>
    </div>
  );
}

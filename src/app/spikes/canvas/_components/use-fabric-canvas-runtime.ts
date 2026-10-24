"use client";

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MutableRefObject } from "react";
import { CANVAS_PAGE, type CanvasDocumentV1 } from "@/shared/canvas";
import {
  fabricCanvasToDocument,
  fabricDocumentToCanvas,
  type FabricApiLike,
  type FabricCanvasLike,
  type FabricObjectLike,
} from "../_lib/fabric-adapter";
import type { CanvasTool } from "./canvas-toolbar";
import { createFabricCanvasEventHandlers } from "./fabric-canvas-interactions";
import {
  type DragDraft,
} from "./fabric-canvas-panel-geometry";

type UseFabricCanvasRuntimeOptions = {
  initialDocument: CanvasDocumentV1;
  commitRef: MutableRefObject<(document: CanvasDocumentV1) => void>;
  tool: CanvasTool;
  text: string;
};

export type FabricCanvasRuntime = {
  surfaceRef: MutableRefObject<HTMLDivElement | null>;
  canvasElementRef: MutableRefObject<HTMLCanvasElement | null>;
  canvasRef: MutableRefObject<FabricCanvasLike | null>;
  fabricRef: MutableRefObject<FabricApiLike | null>;
  applyDocumentRef: MutableRefObject<(document: CanvasDocumentV1) => void>;
  applyZoomRef: MutableRefObject<() => void>;
  ready: boolean;
  deleteActiveObject: () => void;
};

export function useFabricCanvasRuntime({
  initialDocument,
  commitRef,
  tool,
  text,
}: UseFabricCanvasRuntimeOptions): FabricCanvasRuntime {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const canvasElementRef = useRef<HTMLCanvasElement>(null);
  const canvasRef = useRef<FabricCanvasLike | null>(null);
  const fabricRef = useRef<FabricApiLike | null>(null);
  const previewObjectRef = useRef<FabricObjectLike | null>(null);
  const toolRef = useRef<CanvasTool>("select");
  const textRef = useRef("New canvas text");
  const dragRef = useRef<DragDraft | null>(null);
  const draftPointsRef = useRef<Array<[number, number]>>([]);
  const applyDocumentRef = useRef<(document: CanvasDocumentV1) => void>(
    () => undefined,
  );
  const applyZoomRef = useRef<() => void>(() => undefined);
  const [ready, setReady] = useState(false);

  useLayoutEffect(() => {
    toolRef.current = tool;
    textRef.current = text;
  }, [tool, text]);

  const removePreview = useCallback(() => {
    const canvas = canvasRef.current;
    const preview = previewObjectRef.current;
    if (canvas && preview) {
      canvas.remove(preview);
      previewObjectRef.current = null;
      canvas.requestRenderAll?.();
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let canvas: FabricCanvasLike | null = null;
    let cleanup: (() => void) | undefined;

    async function initialize() {
      const fabricModule = (await import("fabric")) as unknown as FabricApiLike;
      const element = canvasElementRef.current;
      if (!element || disposed) return;

      const nextCanvas = new fabricModule.Canvas(element, {
        preserveObjectStacking: true,
        selection: true,
        enablePointerEvents: true,
      });
      canvas = nextCanvas;
      fabricRef.current = fabricModule;
      canvasRef.current = nextCanvas;
      nextCanvas.upperCanvasEl.style.touchAction = "none";
      nextCanvas.backgroundColor = "#fffdf8";
      nextCanvas.setDimensions({
        width: CANVAS_PAGE.width,
        height: CANVAS_PAGE.height,
      });

      const apply = (document: CanvasDocumentV1) => {
        if (!canvas || !fabricRef.current) return;
        removePreview();
        fabricDocumentToCanvas(canvas, fabricRef.current, document);
        applyZoomRef.current();
      };
      applyDocumentRef.current = apply;
      apply(initialDocument);

      const commitCurrent = () => {
        if (!canvas) return;
        commitRef.current(fabricCanvasToDocument(canvas));
      };
      const handlers = createFabricCanvasEventHandlers({
        canvas: nextCanvas,
        fabric: fabricModule,
        element,
        toolRef,
        textRef,
        dragRef,
        draftPointsRef,
        previewObjectRef,
        removePreview,
        commitCurrent,
      });

      nextCanvas.on("mouse:down", handlers.onMouseDown);
      nextCanvas.on("mouse:move", handlers.onMouseMove);
      nextCanvas.on("mouse:up", handlers.onMouseUp);
      nextCanvas.on("path:created", handlers.onPathCreated);
      nextCanvas.on("object:modified", handlers.onObjectModified);
      setReady(true);
      applyZoomRef.current();

      cleanup = () => {
        nextCanvas.off("mouse:down", handlers.onMouseDown);
        nextCanvas.off("mouse:move", handlers.onMouseMove);
        nextCanvas.off("mouse:up", handlers.onMouseUp);
        nextCanvas.off("path:created", handlers.onPathCreated);
        nextCanvas.off("object:modified", handlers.onObjectModified);
        void nextCanvas.dispose();
      };
    }

    void initialize();
    return () => {
      disposed = true;
      cleanup?.();
      canvas = null;
      canvasRef.current = null;
      fabricRef.current = null;
    };
  }, [commitRef, initialDocument, removePreview]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.isDrawingMode = tool === "pen";
    canvas.selection = tool === "select";
    canvas.getObjects().forEach((object) => {
      object.set({
        selectable: tool === "select",
        evented: true,
      });
    });
    if (tool === "pen" && fabricRef.current) {
      const brush = new fabricRef.current.PencilBrush(canvas);
      brush.width = 5;
      brush.color = "#2f5544";
      canvas.freeDrawingBrush = brush;
    }
    canvas.requestRenderAll?.();
  }, [tool, ready]);

  const deleteActiveObject = useCallback(() => {
    const canvas = canvasRef.current;
    const active = canvas?.getActiveObject();
    if (!canvas || !active) return;
    canvas.remove(active);
    canvas.discardActiveObject();
    commitRef.current(fabricCanvasToDocument(canvas));
  }, [commitRef]);

  return {
    surfaceRef,
    canvasElementRef,
    canvasRef,
    fabricRef,
    applyDocumentRef,
    applyZoomRef,
    ready,
    deleteActiveObject,
  };
}

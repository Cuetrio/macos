"use client";

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTheme } from "@/app/_components/theme/theme-provider";
import {
  cloneCanvasDocument,
  createElementId,
  getElementBounds,
  type CanvasDocumentV1,
} from "@/shared/canvas";
import {
  applyCanvasSurfaceDimensions,
  attachFabricMetadata,
  createFabricObject,
  fabricCanvasToDocument,
  isCanvasPreviewObject,
  isCanvasDrawingTarget,
  isCanvasShapeTextEditorTarget,
  isCanvasShapeTextEditorObject,
  markCanvasPreviewObject,
  readCanvasElementType,
  type FabricApiLike,
  type FabricCanvasLike,
  type FabricEventLike,
  type FabricObjectLike,
  fabricDocumentToCanvas,
} from "@/shared/canvas/adapters/fabric";
import type {
  CanvasRuntimeOptions,
  CanvasRuntimeResult,
  CanvasStyleChange,
} from "../canvas";
import {
  DRAW_DRAG_THRESHOLD,
  createDraggedElement,
  pointFromPointer,
} from "@/modules/notes/lib/canvas-editor-geometry";
import {
  DEFAULT_FONT_FAMILY,
  applyFabricStyleChange,
  getDrawingStyleTarget,
  isOneShotCanvasTool,
  isEditingStandaloneText,
  readFabricString,
  readSelectedCanvasStyle,
} from "../canvas";
import {
  createShapeTextEditorSessionController,
  type ShapeTextEditSession,
  type ShapeTextEditorSessionController,
} from "../canvas/shape-text-editor-session";
import type {
  CanvasNoteTool,
  CanvasStyleDefaults,
  DragDraft,
} from "@/modules/notes/lib/canvas-editor-types";

type EraseSession = {
  deletedObjects: Set<FabricObjectLike>;
};

function isCanvasPreviewTarget(event: FabricEventLike) {
  return [event.target, ...(event.subTargets ?? [])].some(
    isCanvasPreviewObject,
  );
}

function isCanvasPenDrawingTarget(event: FabricEventLike) {
  return (
    isCanvasDrawingTarget(event) &&
    !isCanvasPreviewTarget(event) &&
    !isCanvasShapeTextEditorTarget(event)
  );
}

function errorMessage(error: unknown, fallback: string) {
  return error instanceof Error && error.message ? error.message : fallback;
}

function enableTextEditing(canvas: FabricCanvasLike) {
  canvas.getObjects().forEach((object) => {
    if (readCanvasElementType(object) === "text") object.set({ editable: true });
  });
}

export function useNoteCanvasRuntime(
  options: CanvasRuntimeOptions,
): CanvasRuntimeResult {
  const { resolvedTheme } = useTheme();
  const surfaceRef = useRef<HTMLDivElement>(null);
  const canvasElementRef = useRef<HTMLCanvasElement>(null);
  const canvasRef = useRef<FabricCanvasLike | null>(null);
  const fabricRef = useRef<FabricApiLike | null>(null);
  const previewObjectRef = useRef<FabricObjectLike | null>(null);
  const dragRef = useRef<DragDraft | null>(null);
  const eraseSessionRef = useRef<EraseSession | null>(null);
  const shapeTextEditSessionRef = useRef<ShapeTextEditSession | null>(null);
  const shapeTextControllerRef = useRef<ShapeTextEditorSessionController | null>(null);
  const draftPointsRef = useRef<Array<[number, number]>>([]);
  const blockedPenGestureRef = useRef(false);
  const applyDocumentRef = useRef<(document: CanvasDocumentV1) => void>(() => undefined);
  const applyCanvasDimensionsRef = useRef<
    (pageDimensions?: CanvasRuntimeOptions["pageDimensions"]) => void
  >(() => undefined);
  const flushShapeTextEditRef = useRef<() => void>(() => undefined);
  const cancelShapeTextEditRef = useRef<() => void>(() => undefined);
  const callbacksRef = useRef(options);
  const [ready, setReady] = useState(false);
  const pageWidth = options.pageDimensions.width;
  const pageHeight = options.pageDimensions.height;
  const currentTool = options.tool;
  const strokeColor = options.styleDefaults.strokeColor;
  const strokeWidth = options.styleDefaults.strokeWidth;

  const removePreview = useCallback(() => {
    const canvas = canvasRef.current;
    const preview = previewObjectRef.current;
    if (!canvas || !preview) return;
    canvas.remove(preview);
    previewObjectRef.current = null;
    canvas.requestRenderAll?.();
  }, []);

  const applyCanvasDimensions = useCallback(
    (pageDimensions = callbacksRef.current.pageDimensions) => {
      const surface = surfaceRef.current;
      const canvas = canvasRef.current;
      if (!surface || !canvas) return;

      applyCanvasSurfaceDimensions(
        {
          canvas,
          canvasElement: canvasElementRef.current,
          surface,
        },
        pageDimensions,
      );
    },
    [],
  );
  useLayoutEffect(() => {
    callbacksRef.current = options;
    applyCanvasDimensionsRef.current = applyCanvasDimensions;
  }, [applyCanvasDimensions, options]);

  const syncCanvasToolState = useCallback(
    (canvas: FabricCanvasLike, tool: CanvasNoteTool) => {
      canvas.isDrawingMode = tool === "pen";
      canvas.selection = tool === "select";
      if (tool !== "select") {
        canvas.discardActiveObject();
        callbacksRef.current.setSelectedStyle(null);
      } else {
        const activeObject = canvas.getActiveObject();
        callbacksRef.current.setSelectedStyle(
          activeObject ? readSelectedCanvasStyle(activeObject) : null,
        );
      }
      canvas.getObjects().forEach((object) => {
        object.set({ selectable: tool === "select", evented: true });
      });
      if (tool === "pen" && fabricRef.current) {
        const currentStyleDefaults = callbacksRef.current.styleDefaultsRef.current;
        const brush = new fabricRef.current.PencilBrush(canvas);
        brush.width = currentStyleDefaults.strokeWidth;
        brush.color = currentStyleDefaults.strokeColor;
        canvas.freeDrawingBrush = brush;
      }
      canvas.requestRenderAll?.();
    },
    [],
  );

  useEffect(() => {
    let disposed = false;
    let canvas: FabricCanvasLike | null = null;
    let cleanup: (() => void) | undefined;

    async function initialize() {
      if (options.initialDocument === null) return;

      try {
        const fabricModule = (await import("fabric")) as unknown as FabricApiLike;
        const element = canvasElementRef.current;
        if (!element || disposed) return;

        const nextCanvas = new fabricModule.Canvas(element, {
          preserveObjectStacking: true,
          selection: true,
          enablePointerEvents: true,
          allowTouchScrolling: true,
        });
        if (disposed) {
          void nextCanvas.dispose();
          return;
        }
        canvas = nextCanvas;
        fabricRef.current = fabricModule;
        canvasRef.current = nextCanvas;
        nextCanvas.backgroundColor = window
          .getComputedStyle(surfaceRef.current ?? element)
          .getPropertyValue("--app-paper-surface")
          .trim() || "#fffdf8";
        const currentDocument = callbacksRef.current.getCurrentDocument();

        const apply = (document: CanvasDocumentV1) => {
          if (!canvas || !fabricRef.current) return;
          const validated = cloneCanvasDocument(document);
          removePreview();
          fabricDocumentToCanvas(canvas, fabricRef.current, validated);
          enableTextEditing(canvas);
          applyCanvasDimensionsRef.current(validated.page);
        };
        applyDocumentRef.current = apply;
        apply(currentDocument);

        const commitCurrent = () => {
          if (!canvas) return;
          callbacksRef.current.commitDocument(fabricCanvasToDocument(canvas));
        };

        const returnToSelectAfterPlacement = () => {
          const activeTool = callbacksRef.current.toolRef.current;
          if (!isOneShotCanvasTool(activeTool)) return;
          callbacksRef.current.toolRef.current = "select";
          callbacksRef.current.setTool("select");
          syncCanvasToolState(nextCanvas, "select");
        };

        const resetBlockedPenGesture = () => {
          const wasBlocked = blockedPenGestureRef.current;
          blockedPenGestureRef.current = false;
          draftPointsRef.current = [];
          if (wasBlocked) {
            nextCanvas.isDrawingMode =
              callbacksRef.current.toolRef.current === "pen";
          }
        };

        const controller = createShapeTextEditorSessionController({
          canvas: nextCanvas,
          fabric: fabricModule,
          sessionRef: shapeTextEditSessionRef,
          toolRef: callbacksRef.current.toolRef,
          styleDefaultsRef: callbacksRef.current.styleDefaultsRef,
          notifyDocument: (document) => callbacksRef.current.notifyDocument(document),
          commitDocument: (document) => callbacksRef.current.commitDocument(document),
          applyDocument: () =>
            applyDocumentRef.current(callbacksRef.current.getCurrentDocument()),
          setSelectedStyle: (style) => callbacksRef.current.setSelectedStyle(style),
          setShapeTextEditingStyle: (style) =>
            callbacksRef.current.setShapeTextEditingStyle(style),
          reportError: (message) => callbacksRef.current.reportError(message),
          shouldSkipCommit: () => eraseSessionRef.current !== null,
        });
        shapeTextControllerRef.current = controller;
        flushShapeTextEditRef.current = () => controller.flush();
        cancelShapeTextEditRef.current = () => controller.cancel();

        const eraseObject = (target?: FabricObjectLike) => {
          const session = eraseSessionRef.current;
          if (!canvas || !session || !target || session.deletedObjects.has(target)) return;
          if (
            isCanvasPreviewObject(target) ||
            !canvas.getObjects().includes(target)
          ) {
            return;
          }
          session.deletedObjects.add(target);
          canvas.remove(target);
          canvas.requestRenderAll?.();
        };

        const onMouseDownBefore = (event: FabricEventLike) => {
          // Finish the overlay before Fabric clears the active object.
          controller.finishFromPointer(event);
          resetBlockedPenGesture();

          if (
            callbacksRef.current.toolRef.current !== "pen" ||
            isCanvasPenDrawingTarget(event)
          ) {
            return;
          }

          // Fabric 7 starts the free-drawing brush after this event and before
          // mouse:down. Keep the brush disabled for this blocked gesture.
          blockedPenGestureRef.current = true;
          draftPointsRef.current = [];
          nextCanvas.isDrawingMode = false;
        };

        const onMouseDown = (event: FabricEventLike) => {
          const activeTool = callbacksRef.current.toolRef.current;
          const currentCanvas = canvas;
          if (!currentCanvas) return;
          // Fallback for synthetic/programmatic mouse:down events.
          controller.finishFromPointer(event);
          const pointer = pointFromPointer(
            event.e,
            element,
            callbacksRef.current.getCurrentDocument().page,
          );
          const isDetachedShapeTextEditor =
            event.target !== undefined &&
            !currentCanvas.getObjects().includes(event.target) &&
            isCanvasShapeTextEditorTarget(event);
          const isPreviewTarget = isCanvasPreviewTarget(event);
          const canStartCanvasElement =
            (!isPreviewTarget && isCanvasDrawingTarget(event)) ||
            isDetachedShapeTextEditor;

          if (activeTool === "erase") {
            dragRef.current = null;
            draftPointsRef.current = [];
            eraseSessionRef.current = { deletedObjects: new Set() };
            currentCanvas.discardActiveObject();
            eraseObject(event.target);
            return;
          }

          if (activeTool === "pen") {
            if (
              blockedPenGestureRef.current ||
              !isCanvasPenDrawingTarget(event)
            ) {
              draftPointsRef.current = [];
              return;
            }
            draftPointsRef.current = [[pointer.x, pointer.y]];
            return;
          }

          if (activeTool === "select") {
            dragRef.current = null;
            draftPointsRef.current = [];
            removePreview();
            return;
          }

          if (["line", "arrow", "rect", "ellipse"].includes(activeTool)) {
            if (!canStartCanvasElement) {
              dragRef.current = null;
              removePreview();
              return;
            }
            dragRef.current = {
              tool: activeTool as DragDraft["tool"],
              start: pointer,
              current: pointer,
              started: false,
            };
            return;
          }

          if (activeTool === "text") {
            if (!canStartCanvasElement || !fabricRef.current) {
              dragRef.current = null;
              removePreview();
              return;
            }
            const elementForText: CanvasDocumentV1["elements"][number] = {
              id: createElementId("text"),
              type: "text",
              x: pointer.x,
              y: pointer.y,
              width: 290,
              height: 58,
              rotation: 0,
              text: "",
              style: {
                fill: callbacksRef.current.styleDefaultsRef.current.textColor,
                fontSize: callbacksRef.current.styleDefaultsRef.current.fontSize,
                fontFamily: DEFAULT_FONT_FAMILY,
                textAlign: callbacksRef.current.styleDefaultsRef.current.textAlign,
              },
              z: currentCanvas.getObjects().length,
            };
            const textObject = createFabricObject(fabricRef.current, elementForText);
            textObject.set({ editable: true, selectable: true, evented: true });
            currentCanvas.add(textObject);
            currentCanvas.setActiveObject(textObject);
            textObject.enterEditing?.();
            textObject.selectAll?.();
            currentCanvas.requestRenderAll?.();
            returnToSelectAfterPlacement();
          }
        };

        const onMouseDoubleClick = (event: FabricEventLike) => {
          if (callbacksRef.current.toolRef.current === "erase") return;
          controller.start(event);
        };

        const onMouseMove = (event: FabricEventLike) => {
          const pointer = pointFromPointer(
            event.e,
            element,
            callbacksRef.current.getCurrentDocument().page,
          );
          if (callbacksRef.current.toolRef.current === "erase") {
            eraseObject(event.target);
            return;
          }

          if (
            callbacksRef.current.toolRef.current === "pen" &&
            draftPointsRef.current.length
          ) {
            const last = draftPointsRef.current.at(-1);
            if (!last || Math.hypot(pointer.x - last[0], pointer.y - last[1]) >= 2) {
              draftPointsRef.current.push([pointer.x, pointer.y]);
            }
            return;
          }

          const drag = dragRef.current;
          if (!drag || !fabricRef.current || !canvas) return;
          drag.current = pointer;
          if (
            !drag.started &&
            Math.hypot(pointer.x - drag.start.x, pointer.y - drag.start.y) <
              DRAW_DRAG_THRESHOLD
          ) {
            return;
          }
          drag.started = true;
          removePreview();
          const preview = createFabricObject(
            fabricRef.current,
            createDraggedElement(
              drag.tool,
              drag.start,
              drag.current,
              canvas.getObjects().length,
              callbacksRef.current.styleDefaultsRef.current,
            ),
          );
          markCanvasPreviewObject(preview);
          preview.set({ selectable: false, evented: false });
          previewObjectRef.current = preview;
          canvas.add(preview);
          canvas.requestRenderAll?.();
        };

        const onMouseUp = () => {
          if (blockedPenGestureRef.current) {
            resetBlockedPenGesture();
            canvas?.requestRenderAll?.();
            return;
          }

          if (callbacksRef.current.toolRef.current === "pen") {
            draftPointsRef.current = [];
          }

          if (callbacksRef.current.toolRef.current === "select") {
            dragRef.current = null;
            return;
          }

          const eraseSession = eraseSessionRef.current;
          if (eraseSession) {
            eraseSessionRef.current = null;
            removePreview();
            canvas?.discardActiveObject();
            if (eraseSession.deletedObjects.size > 0) commitCurrent();
            else canvas?.requestRenderAll?.();
            return;
          }

          const drag = dragRef.current;
          if (!drag || !fabricRef.current || !canvas) return;
          dragRef.current = null;
          removePreview();
          if (!drag.started) {
            canvas.requestRenderAll?.();
            return;
          }
          canvas.add(
            createFabricObject(
              fabricRef.current,
              createDraggedElement(
                drag.tool,
                drag.start,
                drag.current,
                canvas.getObjects().length,
                callbacksRef.current.styleDefaultsRef.current,
              ),
            ),
          );
          commitCurrent();
          returnToSelectAfterPlacement();
        };

        const onPathCreated = (event: FabricEventLike) => {
          const points = draftPointsRef.current;
          draftPointsRef.current = [];
          // Fabric 7 emits path:created with { path }, not { target }.
          const pathObject = event.path ?? event.target;
          if (blockedPenGestureRef.current) {
            if (pathObject && canvas?.getObjects().includes(pathObject)) {
              canvas.remove(pathObject);
              canvas.requestRenderAll?.();
            }
            return;
          }
          if (!pathObject || points.length < 2 || !canvas) return;
          const bounds = getElementBounds({
            x: Math.min(...points.map(([x]) => x)),
            y: Math.min(...points.map(([, y]) => y)),
            width: 1,
            height: 1,
            points,
          });
          const targetLeft = pathObject.get("left");
          const targetTop = pathObject.get("top");
          const currentStyleDefaults = callbacksRef.current.styleDefaultsRef.current;
          pathObject.set({
            stroke: currentStyleDefaults.strokeColor,
            strokeWidth: currentStyleDefaults.strokeWidth,
          });
          attachFabricMetadata(
            pathObject,
            {
              id: createElementId("stroke"),
              type: "stroke",
              x: bounds.x,
              y: bounds.y,
              width: bounds.width,
              height: bounds.height,
              rotation: 0,
              points,
              style: {
                stroke: currentStyleDefaults.strokeColor,
                strokeWidth: currentStyleDefaults.strokeWidth,
              },
              z: canvas.getObjects().length,
            },
            {
              left: typeof targetLeft === "number" ? targetLeft : bounds.x,
              top: typeof targetTop === "number" ? targetTop : bounds.y,
            },
          );
          commitCurrent();
        };

        const onPointerCancel = () => {
          if (
            callbacksRef.current.toolRef.current !== "pen" &&
            !blockedPenGestureRef.current
          ) {
            return;
          }
          resetBlockedPenGesture();
        };

        const onObjectModified = (event: FabricEventLike) => {
          if (
            eraseSessionRef.current ||
            isCanvasShapeTextEditorObject(event.target)
          ) {
            return;
          }
          commitCurrent();
        };
        const onTextChanged = (event: FabricEventLike) => {
          if (eraseSessionRef.current) return;
          controller.handleTextChanged(event);
        };
        const onTextEditingExited = (event: FabricEventLike) => {
          controller.handleTextEditingExited(event);
        };
        const syncSelection = (event?: FabricEventLike) => {
          const target = event?.target ?? canvas?.getActiveObject();
          callbacksRef.current.setSelectedStyle(
            target ? readSelectedCanvasStyle(target) : null,
          );
        };

        nextCanvas.on("mouse:down:before", onMouseDownBefore);
        nextCanvas.on("mouse:down", onMouseDown);
        nextCanvas.on("mouse:dblclick", onMouseDoubleClick);
        nextCanvas.on("mouse:move", onMouseMove);
        nextCanvas.on("mouse:up", onMouseUp);
        nextCanvas.on("path:created", onPathCreated);
        nextCanvas.on("object:modified", onObjectModified);
        nextCanvas.on("text:changed", onTextChanged);
        nextCanvas.on("text:editing:exited", onTextEditingExited);
        nextCanvas.on("selection:created", syncSelection);
        nextCanvas.on("selection:updated", syncSelection);
        nextCanvas.on("selection:cleared", syncSelection);
        nextCanvas.upperCanvasEl.addEventListener(
          "pointercancel",
          onPointerCancel,
        );
        nextCanvas.upperCanvasEl.addEventListener(
          "touchcancel",
          onPointerCancel,
        );
        setReady(true);
        callbacksRef.current.reportError(null);
        applyCanvasDimensionsRef.current();

        cleanup = () => {
          controller.dispose();
          flushShapeTextEditRef.current = () => undefined;
          cancelShapeTextEditRef.current = () => undefined;
          callbacksRef.current.setShapeTextEditingStyle(null);
          eraseSessionRef.current = null;
          dragRef.current = null;
          draftPointsRef.current = [];
          blockedPenGestureRef.current = false;
          previewObjectRef.current = null;
          nextCanvas.off("mouse:down:before", onMouseDownBefore);
          nextCanvas.off("mouse:down", onMouseDown);
          nextCanvas.off("mouse:dblclick", onMouseDoubleClick);
          nextCanvas.off("mouse:move", onMouseMove);
          nextCanvas.off("mouse:up", onMouseUp);
          nextCanvas.off("path:created", onPathCreated);
          nextCanvas.off("object:modified", onObjectModified);
          nextCanvas.off("text:changed", onTextChanged);
          nextCanvas.off("text:editing:exited", onTextEditingExited);
          nextCanvas.off("selection:created", syncSelection);
          nextCanvas.off("selection:updated", syncSelection);
          nextCanvas.off("selection:cleared", syncSelection);
          nextCanvas.upperCanvasEl.removeEventListener(
            "pointercancel",
            onPointerCancel,
          );
          nextCanvas.upperCanvasEl.removeEventListener(
            "touchcancel",
            onPointerCancel,
          );
          void nextCanvas.dispose();
        };
      } catch (error) {
        if (!disposed) {
          callbacksRef.current.reportError(
            errorMessage(
              error,
              "Canvasの初期化に失敗しました。保存済みデータは変更されていません。",
            ),
          );
        }
      }
    }

    void initialize();
    return () => {
      disposed = true;
      cleanup?.();
      setReady(false);
      shapeTextControllerRef.current = null;
      canvas = null;
      canvasRef.current = null;
      fabricRef.current = null;
    };
  }, [options.initialDocument, removePreview, syncCanvasToolState]);

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
    applyCanvasDimensionsRef.current({ width: pageWidth, height: pageHeight });
  }, [applyCanvasDimensionsRef, pageHeight, pageWidth]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    syncCanvasToolState(canvas, currentTool);
  }, [currentTool, ready, strokeColor, strokeWidth, syncCanvasToolState]);

  const applyStyleChange = useCallback((change: CanvasStyleChange) => {
    const currentOptions = callbacksRef.current;
    const canvas = canvasRef.current;
    const controller = shapeTextControllerRef.current;
    if (controller?.applyStyleChange(change)) return true;

    const shouldCommit = change.commit !== false;
    const activeObject = canvas?.getActiveObject();
    if (canvas && activeObject && isEditingStandaloneText(activeObject)) {
      const currentStyle = readSelectedCanvasStyle(activeObject);
      if (!currentStyle || currentStyle.elementType !== "text") return true;
      const applicableChange: CanvasStyleChange = {
        color: change.color,
        fontSize: change.fontSize,
        textAlign: change.textAlign,
      };
      if (
        applicableChange.color === undefined &&
        applicableChange.fontSize === undefined &&
        applicableChange.textAlign === undefined
      ) {
        return true;
      }
      applyFabricStyleChange(activeObject, "text", applicableChange);
      activeObject.setCoords?.();
      canvas.requestRenderAll?.();
      const nextSelectedStyle = readSelectedCanvasStyle(activeObject);
      if (currentOptions.toolRef.current === "select") {
        currentOptions.setSelectedStyle(nextSelectedStyle);
      } else {
        currentOptions.setStyleDefaults((current) => ({
          ...current,
          textColor: nextSelectedStyle?.color ?? current.textColor,
          fontSize: nextSelectedStyle?.fontSize ?? current.fontSize,
          textAlign: nextSelectedStyle?.textAlign ?? current.textAlign,
        }));
      }
      const nextDocument = fabricCanvasToDocument(canvas);
      const hasText = Boolean(readFabricString(activeObject, "text")?.trim());
      if (shouldCommit && hasText) currentOptions.commitDocument(nextDocument);
      else currentOptions.notifyDocument(nextDocument);
      currentOptions.reportError(null);
      return true;
    }

    const selected =
      currentOptions.toolRef.current === "select"
        ? currentOptions.selectedStyleRef.current
        : null;
    if (selected && canvas) {
      const active = canvas.getActiveObject();
      const currentStyle = active ? readSelectedCanvasStyle(active) : null;
      if (!active || !currentStyle || currentStyle.elementType !== selected.elementType) {
        currentOptions.setSelectedStyle(null);
        return true;
      }
      const applicableChange: CanvasStyleChange =
        currentStyle.elementType === "text"
          ? {
              color: change.color,
              fontSize: change.fontSize,
              textAlign: change.textAlign,
            }
          : { color: change.color, strokeWidth: change.strokeWidth };
      if (
        applicableChange.color === undefined &&
        applicableChange.fontSize === undefined &&
        applicableChange.strokeWidth === undefined &&
        applicableChange.textAlign === undefined
      ) {
        return true;
      }
      applyFabricStyleChange(active, currentStyle.elementType, applicableChange);
      active.setCoords?.();
      canvas.requestRenderAll?.();
      const nextSelectedStyle = readSelectedCanvasStyle(active);
      const nextDocument = fabricCanvasToDocument(canvas);
      if (!shouldCommit) {
        currentOptions.setSelectedStyle(nextSelectedStyle);
        currentOptions.notifyDocument(nextDocument);
        currentOptions.reportError(null);
        return true;
      }
      currentOptions.commitDocument(nextDocument);
      currentOptions.setSelectedStyle(nextSelectedStyle);
      return true;
    }

    const target = getDrawingStyleTarget(currentOptions.toolRef.current);
    if (!target) return false;
    currentOptions.setStyleDefaults((current) => {
      const next: CanvasStyleDefaults = { ...current };
      if (change.strokeWidth !== undefined && target === "stroke") {
        next.strokeWidth = change.strokeWidth;
      }
      if (change.fontSize !== undefined && target === "text") {
        next.fontSize = change.fontSize;
      }
      if (change.textAlign !== undefined && target === "text") {
        next.textAlign = change.textAlign;
      }
      if (change.color !== undefined) {
        if (target === "text") next.textColor = change.color;
        else next.strokeColor = change.color;
      }
      return next;
    });
    return true;
  }, []);

  const deleteActiveObject = useCallback(() => {
    flushShapeTextEditRef.current();
    const canvas = canvasRef.current;
    const active = canvas?.getActiveObject();
    if (!canvas || !active) return;
    canvas.remove(active);
    canvas.discardActiveObject();
    callbacksRef.current.commitDocument(fabricCanvasToDocument(canvas));
  }, []);

  const discardActiveObject = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.discardActiveObject();
    canvas.requestRenderAll?.();
  }, []);

  const isTextEditing = useCallback(
    () => canvasRef.current?.getActiveObject()?.get("isEditing") === true,
    [],
  );

  return {
    surfaceRef,
    canvasElementRef,
    canvasRef,
    fabricRef,
    applyDocumentRef,
    applyCanvasDimensionsRef,
    flushShapeTextEditRef,
    cancelShapeTextEditRef,
    ready,
    applyStyleChange,
    deleteActiveObject,
    discardActiveObject,
    isTextEditing,
  };
}

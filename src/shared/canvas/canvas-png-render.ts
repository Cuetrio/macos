import type { CanvasDocumentV1 } from "./canvas-document-types";
import { cloneCanvasDocument } from "./canvas-document-serialization";
import { createFabricObject } from "./adapters/fabric/fabric-object-factory";
import {
  DEFAULT_STANDALONE_TEXT_FONT_SIZE,
  DEFAULT_TEXT_FONT_FAMILY,
  DEFAULT_TEXT_FONT_SIZE,
} from "./adapters/fabric/fabric-style";
import type { FabricApiLike } from "./adapters/fabric/fabric-types";

const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

function crc32(bytes: Uint8Array) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** Only used after encoding pixels rendered in a verified sRGB context. */
function withSrgbMetadata(bytes: Uint8Array, width: number, height: number): Uint8Array<ArrayBuffer> {
  if (!PNG_SIGNATURE.every((byte, index) => bytes[index] === byte)) {
    throw new Error("invalid-png-encoding");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const chunks: Uint8Array[] = [bytes.subarray(0, 8)];
  const srgb = new Uint8Array(13);
  const srgbView = new DataView(srgb.buffer);
  srgbView.setUint32(0, 1);
  srgb.set([115, 82, 71, 66, 0], 4); // sRGB, rendering intent 0
  srgbView.setUint32(9, crc32(srgb.subarray(4, 9)));
  let offset = 8;
  let header = false;
  let data = false;
  let end = false;
  while (offset + 12 <= bytes.length) {
    const size = view.getUint32(offset);
    const next = offset + 12 + size;
    if (next > bytes.length) throw new Error("invalid-png-encoding");
    const type = String.fromCharCode(...bytes.subarray(offset + 4, offset + 8));
    if (view.getUint32(next - 4) !== crc32(bytes.subarray(offset + 4, next - 4))) {
      throw new Error("invalid-png-encoding");
    }
    if (!header && type !== "IHDR") throw new Error("invalid-png-encoding");
    if (type === "IHDR") {
      if (header || size !== 13 || view.getUint32(offset + 8) !== width || view.getUint32(offset + 12) !== height) {
        throw new Error("invalid-png-dimensions");
      }
      header = true;
      chunks.push(bytes.subarray(offset, next), srgb);
    } else if (!["sRGB", "iCCP", "gAMA", "cHRM"].includes(type)) {
      chunks.push(bytes.subarray(offset, next));
    }
    if (type === "IDAT") data = true;
    if (type === "IEND") {
      if (size !== 0 || next !== bytes.length) throw new Error("invalid-png-encoding");
      end = true;
      break;
    }
    offset = next;
  }
  if (!header || !data || !end) throw new Error("invalid-png-encoding");
  const output = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.length;
  }
  return output;
}

/** Detached, canonical Fabric rendering; no DOM screenshot or editor state. */
export async function renderCanvasPng(document: CanvasDocumentV1): Promise<Uint8Array<ArrayBuffer>> {
  // Detach before the first await, including when fonts or Fabric load slowly.
  const snapshot = cloneCanvasDocument(document);
  const fabric = await import("fabric");
  for (const element of snapshot.elements) {
    if (element.type === "text" || element.text?.trim()) {
      const style = element.type === "text" ? element.style : element.textStyle;
      const defaultSize = element.type === "text"
        ? DEFAULT_STANDALONE_TEXT_FONT_SIZE
        : DEFAULT_TEXT_FONT_SIZE;
      await window.document.fonts.load(
        `${style?.fontSize ?? defaultSize}px ${style?.fontFamily ?? DEFAULT_TEXT_FONT_FAMILY}`,
        element.text,
      );
    }
  }
  await window.document.fonts.ready;
  const element = window.document.createElement("canvas");
  element.width = snapshot.page.width;
  element.height = snapshot.page.height;
  const context = element.getContext("2d", { colorSpace: "srgb" });
  if (!context || context.getContextAttributes?.().colorSpace !== "srgb") {
    // Do not label unknown or wide-gamut pixels as sRGB on older engines.
    throw new Error("srgb-rendering-unavailable");
  }
  const canvas = new fabric.StaticCanvas(element, {
    width: snapshot.page.width,
    height: snapshot.page.height,
    enableRetinaScaling: false,
    renderOnAddRemove: false,
    backgroundColor: "#fffdf8",
  });
  try {
    for (const item of snapshot.elements.slice().sort((a, b) => a.z - b.z)) {
      canvas.add(createFabricObject(fabric as unknown as FabricApiLike, item) as unknown as InstanceType<typeof fabric.FabricObject>);
    }
    canvas.renderAll();
    const blob = await new Promise<Blob>((resolve, reject) => {
      element.toBlob((result) => result ? resolve(result) : reject(new Error("png-encoding-failed")), "image/png");
    });
    return withSrgbMetadata(new Uint8Array(await blob.arrayBuffer()), snapshot.page.width, snapshot.page.height);
  } finally {
    await canvas.dispose();
    element.remove();
  }
}

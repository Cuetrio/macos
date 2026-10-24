import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { chromium, type BrowserContext } from "playwright";
import ts from "typescript";

const root = path.resolve(import.meta.dirname, "../..");
const fixture = {
  schemaVersion: 1,
  page: { width: 640, height: 480, background: "paper" },
  elements: [
    { id: "text", type: "text", x: 20, y: 20, width: 220, height: 60, rotation: 0, z: 0, text: "Saved Canvas", style: { fill: "#000000", fontSize: 28, fontFamily: "sans-serif" } },
    { id: "rect", type: "rect", x: 20, y: 100, width: 100, height: 80, rotation: 0, z: 1, style: { fill: "#ff0000", strokeWidth: 0 } },
    { id: "ellipse", type: "ellipse", x: 160, y: 100, width: 100, height: 80, rotation: 0, z: 2, style: { fill: "#00ff00", strokeWidth: 0 } },
    { id: "line", type: "line", x: 20, y: 220, width: 200, height: 1, rotation: 0, z: 3, points: [[20, 220], [220, 220]], style: { stroke: "#0000ff", strokeWidth: 8 } },
    { id: "stroke", type: "stroke", x: 20, y: 270, width: 200, height: 30, rotation: 0, z: 4, points: [[20, 270], [120, 300], [220, 270]], style: { stroke: "#ff00ff", strokeWidth: 8 } },
    { id: "arrow", type: "arrow", x: 300, y: 220, width: 120, height: 1, rotation: 0, z: 5, points: [[300, 220], [420, 220]], style: { stroke: "#00ffff", strokeWidth: 8 } },
    { id: "clip", type: "rect", x: -20, y: 350, width: 60, height: 60, rotation: 0, z: 6, style: { fill: "#0000ff", strokeWidth: 0 } },
    { id: "outside", type: "rect", x: 700, y: 500, width: 100, height: 100, rotation: 0, z: 7, style: { fill: "#ff0000", strokeWidth: 0 } },
    { id: "srgb", type: "rect", x: 320, y: 100, width: 100, height: 80, rotation: 0, z: 8, style: { fill: "#4285c3", strokeWidth: 0 } },
    { id: "shape-text", type: "rect", x: 300, y: 320, width: 180, height: 80, rotation: 0, z: 9, text: "Shape text", style: { fill: "#ffffff", strokeWidth: 0 }, textStyle: { fill: "#000000", fontSize: 24, fontFamily: "sans-serif" } },
  ],
};

async function installModules(context: BrowserContext) {
  // tsx may retain inferred function names inside serialized evaluate callbacks.
  await context.addInitScript("window.__name = (value) => value;");
  // Fulfilled entirely in-process: no application server, database or external service.
  await context.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== "http://png-export.test") return route.abort();
    if (url.pathname === "/") {
      return route.fulfill({ contentType: "text/html", body: `<!doctype html><html><head><script type="importmap">{"imports":{"@/":"/src/","fabric":"/fabric.mjs"}}</script></head><body style="background:black;height:5000px"><h1>UI filename Cue Summary selection</h1></body></html>` });
    }
    if (url.pathname === "/fabric.mjs") {
      return route.fulfill({ contentType: "text/javascript", body: await readFile(path.join(root, "node_modules/fabric/dist/index.min.mjs"), "utf8") });
    }
    if (!/^\/src\/shared\/canvas\/[a-z0-9/.-]+$/i.test(url.pathname) || url.pathname.includes("..")) return route.abort();
    const file = path.join(root, url.pathname.endsWith(".ts") ? url.pathname : `${url.pathname}.ts`);
    const source = await readFile(file, "utf8");
    const compiled = ts.transpileModule(source, {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
    }).outputText;
    return route.fulfill({ contentType: "text/javascript", body: compiled });
  });
}

test("actual offscreen PNG pixels, dimensions, saved snapshot and sRGB metadata are independent of UI and Retina", async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const outputs: Buffer[] = [];
    for (const scale of [1, 2]) {
      const context = await browser.newContext({ viewport: { width: scale === 1 ? 900 : 400, height: 600 }, deviceScaleFactor: scale });
      await installModules(context);
      const page = await context.newPage();
      await page.goto("http://png-export.test/");
      const result = await page.evaluate(async ({ fixture, scale }) => {
        // Browser imports are served by the isolated route above.
        const renderer = await import(/* webpackIgnore: true */ "/src/shared/canvas/canvas-png-render" as string);
        const snapshots = await import(/* webpackIgnore: true */ "/src/shared/canvas/canvas-png-snapshot" as string);
        const saved = { id: "fixture", bodyMode: "canvas", title: "保存済み", noteDate: "2026-10-16", canvas: fixture };
        const draft = structuredClone(saved);
        draft.title = "UNSAVED";
        draft.canvas.elements[0].text = "UNSAVED";
        const before = JSON.stringify({ saved, draft });
        const snapshot = snapshots.captureSavedCanvasPngSnapshot(saved);
        document.body.style.zoom = String(scale / 2);
        document.body.style.background = scale === 1 ? "black" : "red";
        document.querySelector("h1")!.textContent = scale === 1 ? "Cue Summary toolbar" : "A DIFFERENT FILENAME AND UI";
        window.scrollTo(0, scale === 1 ? 0 : 1000);
        const rendering = renderer.renderCanvasPng(snapshot.canvas);
        // A later state change cannot leak into an in-flight export.
        snapshot.canvas.elements[0].text = "MUTATED AFTER START";
        const bytes: Uint8Array = await rendering;
        const image = new Image();
        const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/png" }));
        try {
          image.src = url;
          await image.decode();
          const canvas = document.createElement("canvas");
          canvas.width = image.naturalWidth;
          canvas.height = image.naturalHeight;
          const context = canvas.getContext("2d", { colorSpace: "srgb" })!;
          context.drawImage(image, 0, 0);
          const pixel = (x: number, y: number) => [...context.getImageData(x, y, 1, 1).data];
          const countInk = (x: number, y: number, width: number, height: number) => {
            const pixels = context.getImageData(x, y, width, height).data;
            let count = 0;
            for (let i = 0; i < pixels.length; i += 4) {
              if (pixels[i] < 50 && pixels[i + 1] < 50 && pixels[i + 2] < 50) count++;
            }
            return count;
          };
          const darkTextPixels = countInk(20, 20, 220, 60);
          const shapeTextPixels = countInk(300, 320, 180, 80);
          const clippedFixture = structuredClone(fixture);
          clippedFixture.elements = clippedFixture.elements.filter((item) => item.id !== "outside");
          const withoutOutside: Uint8Array = await renderer.renderCanvasPng(clippedFixture);
          return {
            bytes: [...bytes], withoutOutside: [...withoutOutside],
            width: image.naturalWidth, height: image.naturalHeight,
            paper: pixel(600, 450), rect: pixel(60, 140), ellipse: pixel(210, 140),
            line: pixel(100, 220), stroke: pixel(120, 299), arrow: pixel(350, 220), clip: pixel(0, 380),
            darkTextPixels, shapeTextPixels, srgb: pixel(360, 140),
            unchanged: before === JSON.stringify({ saved, draft }),
            fileName: snapshot.fileName,
          };
        } finally {
          URL.revokeObjectURL(url);
        }
      }, { fixture, scale });
      assert.equal(result.width, 640);
      assert.equal(result.height, 480);
      const bytes = Buffer.from(result.bytes);
      assert.equal(bytes.readUInt32BE(16), 640, "actual IHDR width");
      assert.equal(bytes.readUInt32BE(20), 480, "actual IHDR height");
      assert.deepEqual(result.paper, [255, 253, 248, 255]);
      assert.deepEqual(result.rect, [255, 0, 0, 255]);
      assert.deepEqual(result.ellipse, [0, 255, 0, 255]);
      assert.deepEqual(result.line, [0, 0, 255, 255]);
      assert.deepEqual(result.stroke, [255, 0, 255, 255]);
      assert.deepEqual(result.arrow, [0, 255, 255, 255]);
      assert.deepEqual(result.clip, [0, 0, 255, 255]);
      assert.ok(result.darkTextPixels > 100, "actual text ink in the PNG");
      assert.ok(result.shapeTextPixels > 100, "canonical shape-group text ink in the PNG");
      assert.deepEqual(result.srgb, [66, 133, 195, 255]);
      assert.equal(result.unchanged, true);
      assert.equal(result.fileName, "保存済み_2026-10-16.png");
      assert.deepEqual(bytes, Buffer.from(result.withoutOutside), "fully out-of-page content is clipped");
      const chunkNames: string[] = [];
      for (let offset = 8; offset < bytes.length;) {
        const size = bytes.readUInt32BE(offset);
        const type = bytes.toString("ascii", offset + 4, offset + 8);
        chunkNames.push(type);
        if (type === "sRGB") {
          assert.equal(size, 1);
          assert.equal(bytes[offset + 8], 0);
          assert.equal(bytes.readUInt32BE(offset + 9), 0xaece1ce9, "standard sRGB chunk CRC");
        }
        offset += 12 + size;
      }
      assert.equal(chunkNames.filter((name) => name === "sRGB").length, 1);
      assert.equal(chunkNames.includes("iCCP"), false);
      outputs.push(bytes);
      await context.close();
    }
    assert.deepEqual(outputs[0], outputs[1], "zoom, viewport, scroll and device scale do not change the PNG");
  } finally {
    await browser.close();
  }
});

test("PNG rendering fails closed if the browser cannot establish an sRGB context", async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext();
    await installModules(context);
    const page = await context.newPage();
    await page.goto("http://png-export.test/");
    const error = await page.evaluate(async (fixture) => {
      const renderer = await import(/* webpackIgnore: true */ "/src/shared/canvas/canvas-png-render" as string);
      CanvasRenderingContext2D.prototype.getContextAttributes = () => ({ colorSpace: "display-p3", alpha: true, desynchronized: false, willReadFrequently: false, colorType: "unorm8" });
      try {
        await renderer.renderCanvasPng(fixture);
        return null;
      } catch (error) {
        return error instanceof Error ? error.message : "unexpected";
      }
    }, fixture);
    assert.equal(error, "srgb-rendering-unavailable");
    await context.close();
  } finally {
    await browser.close();
  }
});

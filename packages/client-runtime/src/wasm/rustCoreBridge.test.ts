import { describe, expect, it } from "vite-plus/test";

import {
  abiVersion,
  emitWebAppBundleJson,
  initRustCoreBridge,
  isLegacyWebForced,
  isRustCoreWasmActive,
  sanitizeBracketedPaste,
  splitSocketFrameParts,
} from "./rustCoreBridge.ts";

function withLegacyWebEnv<T>(value: string | undefined, fn: () => Promise<T>): Promise<T> {
  const previous = process.env.T3CODE_LEGACY_WEB;
  if (value === undefined) {
    delete process.env.T3CODE_LEGACY_WEB;
  } else {
    process.env.T3CODE_LEGACY_WEB = value;
  }
  return fn().finally(() => {
    if (previous === undefined) {
      delete process.env.T3CODE_LEGACY_WEB;
    } else {
      process.env.T3CODE_LEGACY_WEB = previous;
    }
  });
}

describe("rustCoreBridge", () => {
  it("loads live WASM in Node/Vitest and preserves paste, framing, and web bundle parity", async () => {
    await withLegacyWebEnv(undefined, async () => {
      expect(isLegacyWebForced()).toBe(false);
      expect(await initRustCoreBridge()).toBe(true);
      expect(isRustCoreWasmActive()).toBe(true);
      expect(abiVersion()).toBe(1);

      const rawPaste = "echo ok\t\r\n\u001b[201~rm -rf /\u001b[000200~\u001b]0;pwn\u0007\u0000!";
      const wasmPaste = sanitizeBracketedPaste(rawPaste);
      expect(wasmPaste).toBe("echo ok\t\nrm -rf /!");

      const framePayload = "😀😀😀";
      const wasmParts = splitSocketFrameParts(framePayload, 6);
      expect(wasmParts).toEqual(["\u001e😀", "\u001e😀", "😀"]);

      const unpairedPayload = "\ud800中中";
      const unpairedWasmParts = splitSocketFrameParts(unpairedPayload, 5);
      expect(unpairedWasmParts).toEqual(["\u001e\ufffd", "\u001e中", "中"]);

      const bundle = JSON.parse(emitWebAppBundleJson("sv")) as {
        readonly indexHtml: string;
        readonly stylesCss: string;
        readonly appMjs: string;
      };
      expect(bundle.indexHtml).toContain('<html lang="sv">');
      expect(bundle.indexHtml).toContain("./styles.css");
      expect(bundle.indexHtml).toContain("./app.mjs");
      expect(bundle.stylesCss).toContain(".t3-shell");
      expect(bundle.appMjs).toContain("WasmWebApp");
    });
  });

  it("bypasses WASM and runs pure TypeScript fallbacks when T3CODE_LEGACY_WEB=1", async () => {
    const rawPaste = "a\u001b[20\u001b[201~1~b\u001b]0;unterminated\nc\u001b]52;c;dGVzdA==\u0007d";
    let fallbackPaste = "";
    let fallbackParts: ReadonlyArray<string> = [];

    await withLegacyWebEnv("1", async () => {
      expect(isLegacyWebForced()).toBe(true);
      expect(await initRustCoreBridge()).toBe(false);
      expect(isRustCoreWasmActive()).toBe(false);
      expect(abiVersion()).toBe(1);

      fallbackPaste = sanitizeBracketedPaste(rawPaste);
      expect(fallbackPaste).toBe("ab\ncd");

      fallbackParts = splitSocketFrameParts("😀😀😀", 6);
      expect(fallbackParts).toEqual(["\u001e😀", "\u001e😀", "😀"]);

      const unpairedFallbackParts = splitSocketFrameParts("\ud800中中", 5);
      expect(unpairedFallbackParts).toEqual(["\u001e\ufffd", "\u001e中", "中"]);

      const fallbackBundle = JSON.parse(emitWebAppBundleJson("de-DE")) as {
        readonly indexHtml: string;
        readonly stylesCss: string;
        readonly appMjs: string;
      };
      expect(fallbackBundle.indexHtml).toContain('<html lang="de">');
      expect(fallbackBundle.indexHtml).toContain("./styles.css");
      expect(fallbackBundle.stylesCss).toContain(".t3-shell");
      expect(fallbackBundle.appMjs).toContain("WasmWebApp");
    });

    await withLegacyWebEnv(undefined, async () => {
      expect(isRustCoreWasmActive()).toBe(true);
      expect(sanitizeBracketedPaste(rawPaste)).toBe(fallbackPaste);
      expect(splitSocketFrameParts("😀😀😀", 6)).toEqual(fallbackParts);
    });
  });
});

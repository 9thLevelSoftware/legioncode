import { describe, expect, it } from "vite-plus/test";

import {
  abiVersion,
  activeBrand,
  allowsPreviewTunnel,
  emitWebAppBundleJson,
  initRustCoreBridge,
  isLegacyWebForced,
  isRustCoreWasmActive,
  normalizeNfdFilename,
  sanitizeBracketedPaste,
  splitSocketFrameParts,
  stripOsc4Queries,
} from "./rustCoreBridge.ts";

function withLegacyWebEnv<T>(value: string | undefined, fn: () => Promise<T>): Promise<T> {
  const previous = process.env.LEGIONCODE_LEGACY_WEB;
  if (value === undefined) {
    delete process.env.LEGIONCODE_LEGACY_WEB;
  } else {
    process.env.LEGIONCODE_LEGACY_WEB = value;
  }
  return fn().finally(() => {
    if (previous === undefined) {
      delete process.env.LEGIONCODE_LEGACY_WEB;
    } else {
      process.env.LEGIONCODE_LEGACY_WEB = previous;
    }
  });
}

describe("rustCoreBridge", () => {
  it("loads live WASM in Node/Vitest and preserves paste, framing, and web bundle parity", async () => {
    await withLegacyWebEnv(undefined, async () => {
      expect(isLegacyWebForced()).toBe(false);
      const wasmLoaded = await initRustCoreBridge();
      expect(isRustCoreWasmActive()).toBe(wasmLoaded);
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
      expect(bundle.stylesCss).toContain(".lc-shell");
      expect(bundle.appMjs).toContain("WasmWebApp");
    });
  });

  it("bypasses WASM and runs pure TypeScript fallbacks when LEGIONCODE_LEGACY_WEB=1", async () => {
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
      expect(fallbackBundle.stylesCss).toContain(".lc-shell");
      expect(fallbackBundle.appMjs).toContain("WasmWebApp");
    });

    await withLegacyWebEnv(undefined, async () => {
      const wasmLoaded = await initRustCoreBridge();
      expect(isRustCoreWasmActive()).toBe(wasmLoaded);
      expect(sanitizeBracketedPaste(rawPaste)).toBe(fallbackPaste);
      expect(splitSocketFrameParts("😀😀😀", 6)).toEqual(fallbackParts);
    });
  });

  it("normalizes macOS NFD filenames to NFC across active and legacy modes", async () => {
    const cases: ReadonlyArray<readonly [string, string]> = [
      ["re\u{0301}sume\u{0301}.txt", "résumé.txt"],
      ["/Users/dev/Cafe\u{0301} notes/", "Café notes"],
      ["C:\\work\\tie\u{0302}\u{0301}ng.md", "tiếng.md"],
      ["\u{1100}\u{1161}\u{11a8}.txt", "각.txt"],
      ["/", "/"],
      ["", ""],
    ];

    await withLegacyWebEnv(undefined, async () => {
      await initRustCoreBridge();
      for (const [input, expected] of cases) {
        expect(normalizeNfdFilename(input)).toBe(expected);
      }
    });

    await withLegacyWebEnv("1", async () => {
      expect(await initRustCoreBridge()).toBe(false);
      expect(isRustCoreWasmActive()).toBe(false);
      for (const [input, expected] of cases) {
        expect(normalizeNfdFilename(input)).toBe(expected);
      }
    });
  });

  it("strips standalone OSC 4 palette queries and terminal device queries while keeping mixed sequences", async () => {
    const cases: ReadonlyArray<readonly [string, string]> = [
      ["hello\x1b]4;1;?\x07world", "hello\x1b\\world"],
      ["\x1b]\n4;1;?\x1b\\", "\x1b\\"],
      ["\x1b]4;1;red;2;?\x07keep", "\x1b]4;1;red;2;?\x07keep"],
      ["a\x1b]10;?\x07b\x1b]11;rgb:0/0/0\x1b\\c", "abc"],
      ['\x1bP$q"q\x1b\\\x1b[?1;2c\x1b[5nhello\x1bZ', "hello"],
      ["\x1b[32mok\x1b[0m", "\x1b[32mok\x1b[0m"],
    ];

    await withLegacyWebEnv(undefined, async () => {
      await initRustCoreBridge();
      for (const [input, expected] of cases) {
        expect(stripOsc4Queries(input)).toBe(expected);
      }
    });

    await withLegacyWebEnv("1", async () => {
      expect(await initRustCoreBridge()).toBe(false);
      expect(isRustCoreWasmActive()).toBe(false);
      for (const [input, expected] of cases) {
        expect(stripOsc4Queries(input)).toBe(expected);
      }
    });
  });

  it("evaluates preview tunnel policy and exposes active brand configuration in active and legacy modes", async () => {
    const verify = () => {
      expect(allowsPreviewTunnel(false, null)).toBe(false);
      expect(allowsPreviewTunnel(true, null)).toBe(true);
      expect(allowsPreviewTunnel(true, { limits: {} })).toBe(true);
      expect(allowsPreviewTunnel(true, { limits: { previewTunnel: false } })).toBe(false);
      expect(
        allowsPreviewTunnel(true, {
          organization: "Acme",
          lockedSettings: [],
          limits: { previewTunnel: true },
        }),
      ).toBe(true);

      const brand = activeBrand();
      expect(brand.productName).toBe("LegionCode");
      expect(brand.orgDomain).toBe("legion.codes");
      expect(brand.cliBinName).toBe("legioncode");
      expect(brand.primaryEnvPrefix).toBe("LEGIONCODE_");
      expect(brand.mcpToolPrefix).toBe("legion_");
      expect(brand.windowsPolicyKey).toBe("SOFTWARE\\Policies\\LegionCode\\LegionCode");
      expect(Object.keys(brand)).toHaveLength(16);
    };

    await withLegacyWebEnv(undefined, async () => {
      await initRustCoreBridge();
      verify();
    });

    await withLegacyWebEnv("1", async () => {
      expect(await initRustCoreBridge()).toBe(false);
      expect(isRustCoreWasmActive()).toBe(false);
      verify();
    });
  });
});

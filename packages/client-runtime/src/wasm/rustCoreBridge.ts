import { resolveLocale } from "../locales.ts";

const DEFAULT_SOCKET_FRAME_PART_BYTES = 48 * 1024;
const PART_PREFIX = "\u001e";
const FALLBACK_ABI_VERSION = 1;
const SUPPORTED_WEB_LOCALES = [
  "en",
  "de",
  "es",
  "fr",
  "ja",
  "ko",
  "pt-BR",
  "ru",
  "sv",
  "zh",
] as const;

interface WasmFfiExports {
  readonly t3_wasm_abi_version: () => number;
  readonly sanitize_bracketed_paste_str: (raw: string) => string;
  readonly split_utf16_frame_parts_json: (inputJson: string) => string;
  readonly emit_web_app_bundle_json: () => string;
  readonly translate_i18n_json?: (inputJson: string) => string;
  readonly default?: (moduleOrPath?: string) => Promise<unknown>;
}

interface NodeBuiltinModule {
  readonly createRequire: (filename: string) => (id: string) => unknown;
}

interface NodeBuiltinFS {
  readonly existsSync: (filePath: string) => boolean;
}

interface NodeBuiltinPath {
  readonly dirname: (filePath: string) => string;
  readonly resolve: (...segments: ReadonlyArray<string>) => string;
}

interface NodeBuiltinURL {
  readonly fileURLToPath: (url: string | URL) => string;
}

interface RuntimeProcessLike {
  readonly env?: Readonly<Record<string, string | undefined>>;
  readonly cwd?: () => string;
  readonly getBuiltinModule?: (id: string) => unknown;
}

interface WebAppBundleAssets {
  readonly indexHtml: string;
  readonly stylesCss: string;
  readonly appMjs: string;
}

let cachedNodeWasm: WasmFfiExports | null | undefined;
let cachedBrowserWasm: WasmFfiExports | undefined;

function getRuntimeProcess(): RuntimeProcessLike | undefined {
  return (globalThis as { readonly process?: RuntimeProcessLike }).process;
}

function isWasmFfiExports(value: unknown): value is WasmFfiExports {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<Record<keyof WasmFfiExports, unknown>>;
  return (
    typeof candidate.t3_wasm_abi_version === "function" &&
    typeof candidate.sanitize_bracketed_paste_str === "function" &&
    typeof candidate.split_utf16_frame_parts_json === "function" &&
    typeof candidate.emit_web_app_bundle_json === "function"
  );
}

export function isLegacyWebForced(env?: Readonly<Record<string, string | undefined>>): boolean {
  const proc = getRuntimeProcess();
  const raw =
    env?.T3CODE_LEGACY_WEB ??
    proc?.env?.T3CODE_LEGACY_WEB ??
    (globalThis as { readonly T3CODE_LEGACY_WEB?: unknown }).T3CODE_LEGACY_WEB;
  if (typeof raw !== "string" && typeof raw !== "number" && typeof raw !== "boolean") {
    return false;
  }
  const normalized = String(raw).trim().toLowerCase();
  return normalized === "1" || normalized === "true";
}

function loadNodeWasmExports(): WasmFfiExports | undefined {
  if (cachedNodeWasm !== undefined) {
    return cachedNodeWasm ?? undefined;
  }
  const proc = getRuntimeProcess();
  if (!proc || typeof proc.getBuiltinModule !== "function") {
    cachedNodeWasm = null;
    return undefined;
  }

  try {
    const nodeModule = proc.getBuiltinModule("node:module") as NodeBuiltinModule | undefined;
    const nodeFs = proc.getBuiltinModule("node:fs") as NodeBuiltinFS | undefined;
    const nodePath = proc.getBuiltinModule("node:path") as NodeBuiltinPath | undefined;
    const nodeUrl = proc.getBuiltinModule("node:url") as NodeBuiltinURL | undefined;
    if (!nodeModule || !nodeFs || !nodePath || !nodeUrl) {
      cachedNodeWasm = null;
      return undefined;
    }

    const candidates: Array<string> = [];
    if (import.meta.url.startsWith("file:")) {
      const currentDir = nodePath.dirname(nodeUrl.fileURLToPath(import.meta.url));
      candidates.push(
        nodePath.resolve(currentDir, "../../../../rust/dist/wasm-nodejs/t3_wasm_ffi.js"),
      );
    }
    if (typeof proc.cwd === "function") {
      const cwd = proc.cwd();
      candidates.push(
        nodePath.resolve(cwd, "rust/dist/wasm-nodejs/t3_wasm_ffi.js"),
        nodePath.resolve(cwd, "../../rust/dist/wasm-nodejs/t3_wasm_ffi.js"),
      );
    }

    const wasmEntry = candidates.find((candidate) => nodeFs.existsSync(candidate));
    if (!wasmEntry) {
      cachedNodeWasm = null;
      return undefined;
    }

    const requireFromHere = nodeModule.createRequire(import.meta.url);
    const loaded = requireFromHere(wasmEntry);
    if (!isWasmFfiExports(loaded)) {
      cachedNodeWasm = null;
      return undefined;
    }

    cachedNodeWasm = loaded;
    return loaded;
  } catch {
    cachedNodeWasm = null;
    return undefined;
  }
}

function resolveActiveWasmExports(): WasmFfiExports | undefined {
  if (isLegacyWebForced()) {
    return undefined;
  }
  return cachedBrowserWasm ?? loadNodeWasmExports();
}

export function isRustCoreWasmActive(): boolean {
  return resolveActiveWasmExports() !== undefined;
}

export async function initRustCoreBridge(options?: {
  readonly moduleUrl?: string;
  readonly wasmUrl?: string;
}): Promise<boolean> {
  if (isLegacyWebForced()) {
    return false;
  }
  if (loadNodeWasmExports()) {
    return true;
  }
  if (cachedBrowserWasm) {
    return true;
  }

  const moduleUrl = options?.moduleUrl ?? "./t3_wasm_ffi.js";
  try {
    const imported: unknown = await import(/* @vite-ignore */ moduleUrl);
    if (typeof imported !== "object" || imported === null) {
      return false;
    }
    const maybeDefault = (imported as { readonly default?: unknown }).default;
    if (typeof maybeDefault === "function") {
      await (maybeDefault as (wasmPath?: string) => Promise<unknown>)(options?.wasmUrl);
    }
    if (!isWasmFfiExports(imported)) {
      return false;
    }
    cachedBrowserWasm = imported;
    return true;
  } catch {
    return false;
  }
}

export function abiVersion(): number {
  const wasm = resolveActiveWasmExports();
  if (wasm) {
    return wasm.t3_wasm_abi_version();
  }
  return FALLBACK_ABI_VERSION;
}

function isUnsafePasteControl(codePoint: number): boolean {
  if (codePoint === 0x0a || codePoint === 0x09) return false;
  return (
    (codePoint >= 0x00 && codePoint <= 0x1f) ||
    codePoint === 0x7f ||
    (codePoint >= 0x80 && codePoint <= 0x9f)
  );
}

function isStructuralPasteEscapeOrSafe(codePoint: number): boolean {
  return (
    codePoint === 0x0a ||
    codePoint === 0x09 ||
    codePoint === 0x1b ||
    codePoint === 0x07 ||
    codePoint === 0x9b ||
    codePoint === 0x9c ||
    codePoint === 0x9d
  );
}

function matchBracketedMarkerLength(input: string, index: number): number {
  let cursor = index;
  if (input.startsWith("\u001b[", cursor)) {
    cursor += 2;
  } else if (input.startsWith("\u009b", cursor)) {
    cursor += 1;
  } else {
    return 0;
  }
  while (cursor < input.length && input.charCodeAt(cursor) === 0x30) {
    cursor += 1;
  }
  if (input.startsWith("201~", cursor) || input.startsWith("200~", cursor)) {
    return cursor + 4 - index;
  }
  return 0;
}

function matchOscSequenceLength(input: string, index: number): number {
  let cursor = index;
  if (input.startsWith("\u001b]", cursor)) {
    cursor += 2;
  } else if (input.startsWith("\u009d", cursor)) {
    cursor += 1;
  } else {
    return 0;
  }
  while (cursor < input.length) {
    const code = input.charCodeAt(cursor);
    if (code === 0x07 || code === 0x9c) {
      return cursor + 1 - index;
    }
    if (code === 0x1b) {
      if (input.charCodeAt(cursor + 1) === 0x5c) {
        return cursor + 2 - index;
      }
      return cursor - index;
    }
    if (code === 0x0a) {
      return cursor - index;
    }
    cursor += 1;
  }
  return input.length - index;
}

function stripPasteEscapesOnce(input: string): string {
  let out = "";
  let index = 0;
  while (index < input.length) {
    const markerLen = matchBracketedMarkerLength(input, index);
    if (markerLen > 0) {
      index += markerLen;
      continue;
    }
    const oscLen = matchOscSequenceLength(input, index);
    if (oscLen > 0) {
      index += oscLen;
      continue;
    }
    const codePoint = input.codePointAt(index)!;
    const width = codePoint > 0xffff ? 2 : 1;
    out += input.slice(index, index + width);
    index += width;
  }
  return out;
}

function sanitizeBracketedPasteFallback(input: string): string {
  let current = "";
  for (const ch of input) {
    const codePoint = ch.codePointAt(0)!;
    if (isStructuralPasteEscapeOrSafe(codePoint) || !isUnsafePasteControl(codePoint)) {
      current += ch;
    }
  }
  while (true) {
    const next = stripPasteEscapesOnce(current);
    if (next.length === current.length) {
      current = next;
      break;
    }
    current = next;
  }
  let clean = "";
  for (const ch of current) {
    const codePoint = ch.codePointAt(0)!;
    if (!isUnsafePasteControl(codePoint)) {
      clean += ch;
    }
  }
  return clean;
}

export function sanitizeBracketedPaste(input: string): string {
  const wasm = resolveActiveWasmExports();
  if (wasm) {
    return wasm.sanitize_bracketed_paste_str(input);
  }
  return sanitizeBracketedPasteFallback(input);
}

function toValidUtf8ScalarString(input: string): {
  readonly normalized: string;
  readonly hasUnpairedSurrogate: boolean;
} {
  let hasUnpairedSurrogate = false;
  let normalized = "";
  let segmentStart = 0;

  for (let index = 0; index < input.length; index += 1) {
    const code = input.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = index + 1 < input.length ? input.charCodeAt(index + 1) : 0;
      if (next >= 0xdc00 && next <= 0xdfff) {
        index += 1;
        continue;
      }
      hasUnpairedSurrogate = true;
      normalized += `${input.slice(segmentStart, index)}\ufffd`;
      segmentStart = index + 1;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      hasUnpairedSurrogate = true;
      normalized += `${input.slice(segmentStart, index)}\ufffd`;
      segmentStart = index + 1;
    }
  }

  if (!hasUnpairedSurrogate) {
    return { normalized: input, hasUnpairedSurrogate: false };
  }
  return { normalized: normalized + input.slice(segmentStart), hasUnpairedSurrogate: true };
}

function utf8LengthOfScalar(codePoint: number): number {
  if (codePoint <= 0x7f) return 1;
  if (codePoint <= 0x7ff) return 2;
  if (codePoint <= 0xffff) return 3;
  return 4;
}

function splitSocketFramePartsFallback(
  payload: string,
  partBytes = DEFAULT_SOCKET_FRAME_PART_BYTES,
): ReadonlyArray<string> {
  const { normalized } = toValidUtf8ScalarString(payload);
  const scalars: Array<{ readonly text: string; readonly bytes: number }> = [];
  let totalBytes = 0;

  for (let index = 0; index < normalized.length;) {
    const codePoint = normalized.codePointAt(index)!;
    const width = codePoint > 0xffff ? 2 : 1;
    const bytes = utf8LengthOfScalar(codePoint);
    scalars.push({ text: normalized.slice(index, index + width), bytes });
    totalBytes += bytes;
    index += width;
  }

  if (totalBytes <= partBytes) {
    return [normalized];
  }

  const prefixBytes = PART_PREFIX.length;
  if (partBytes <= prefixBytes) {
    const firstBytes = scalars[0]?.bytes ?? 1;
    throw new RangeError(
      `socket frame part limit ${partBytes} bytes is below required minimum ${prefixBytes + firstBytes} bytes`,
    );
  }

  const maxPayload = partBytes - prefixBytes;
  const parts: Array<string> = [];
  let cursor = 0;
  let remainingBytes = totalBytes;

  while (remainingBytes > partBytes) {
    let chunkText = "";
    let chunkBytes = 0;
    while (cursor < scalars.length) {
      const scalar = scalars[cursor]!;
      if (chunkBytes + scalar.bytes > maxPayload) {
        break;
      }
      chunkText += scalar.text;
      chunkBytes += scalar.bytes;
      cursor += 1;
    }
    if (chunkBytes === 0) {
      const nextBytes = scalars[cursor]?.bytes ?? 1;
      throw new RangeError(
        `socket frame part limit ${partBytes} bytes is below required minimum ${prefixBytes + nextBytes} bytes`,
      );
    }
    parts.push(`${PART_PREFIX}${chunkText}`);
    remainingBytes -= chunkBytes;
  }

  let tail = "";
  while (cursor < scalars.length) {
    tail += scalars[cursor]!.text;
    cursor += 1;
  }
  parts.push(tail);
  return parts;
}

export function splitSocketFrameParts(
  payload: string,
  partBytes = DEFAULT_SOCKET_FRAME_PART_BYTES,
): ReadonlyArray<string> {
  const wasm = resolveActiveWasmExports();
  if (wasm) {
    const { hasUnpairedSurrogate } = toValidUtf8ScalarString(payload);
    const request = hasUnpairedSurrogate
      ? {
          utf16Units: Array.from({ length: payload.length }, (_, idx) => payload.charCodeAt(idx)),
          partBytes,
        }
      : { text: payload, partBytes };
    const response = JSON.parse(wasm.split_utf16_frame_parts_json(JSON.stringify(request))) as {
      readonly parts: ReadonlyArray<string>;
    };
    return response.parts;
  }
  return splitSocketFramePartsFallback(payload, partBytes);
}

function resolveWasmBundleLocale(wasm: WasmFfiExports, locale?: string): string {
  if (!locale || !wasm.translate_i18n_json) {
    return "en";
  }
  try {
    const translated = JSON.parse(
      wasm.translate_i18n_json(JSON.stringify({ language: locale })),
    ) as { readonly locale?: string; readonly bcp47Tag?: string };
    return translated.bcp47Tag ?? translated.locale ?? "en";
  } catch {
    return "en";
  }
}

function fallbackWebAppBundleJson(locale?: string): string {
  const resolvedLocale = locale
    ? resolveLocale(locale, [locale], SUPPORTED_WEB_LOCALES, "en")
    : "en";
  const bundle: WebAppBundleAssets = {
    indexHtml: [
      "<!doctype html>",
      `<html lang="${resolvedLocale}">`,
      "  <head>",
      '    <meta charset="utf-8" />',
      '    <meta name="viewport" content="width=device-width, initial-scale=1" />',
      "    <title>T3 Code</title>",
      '    <link rel="stylesheet" href="./styles.css" />',
      "  </head>",
      "  <body>",
      '    <div id="app" data-testid="app-root"></div>',
      '    <script type="module" src="./app.mjs"></script>',
      "  </body>",
      "</html>",
      "",
    ].join("\n"),
    stylesCss: ":root { color-scheme: dark; }\n.t3-shell { display: grid; min-height: 100vh; }\n",
    appMjs: [
      'import init, { WasmWebApp } from "./t3_wasm_ffi.js";',
      "await init();",
      'const app = new WasmWebApp(navigator.language || "en", Date.now());',
      'app.dispatch_json(JSON.stringify({ type: "setLocale", language: "system" }));',
      "",
    ].join("\n"),
  };
  return JSON.stringify(bundle);
}

export function emitWebAppBundleJson(locale?: string): string {
  const wasm = resolveActiveWasmExports();
  if (wasm) {
    const rawJson = wasm.emit_web_app_bundle_json();
    if (!locale) {
      return rawJson;
    }
    const targetLang = resolveWasmBundleLocale(wasm, locale);
    if (targetLang === "en") {
      return rawJson;
    }
    const parsed = JSON.parse(rawJson) as WebAppBundleAssets;
    return JSON.stringify({
      ...parsed,
      indexHtml: parsed.indexHtml.replace('<html lang="en">', `<html lang="${targetLang}">`),
    });
  }
  return fallbackWebAppBundleJson(locale);
}

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
  readonly normalize_nfd_filename?: (raw: string) => string;
  readonly strip_osc4_queries?: (raw: string) => string;
  readonly allows_preview_tunnel_json?: (inputJson: string) => string;
  readonly active_brand_json?: () => string;
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

const TRAILING_SEPARATORS_RE = /[/\\]+$/;

export interface PreviewTunnelPolicySummary {
  readonly organization?: string;
  readonly lockedSettings?: ReadonlyArray<string>;
  readonly limits?: {
    readonly previewTunnel?: boolean;
    readonly [key: string]: unknown;
  };
  readonly [key: string]: unknown;
}

export interface ActiveBrandConfig {
  readonly productName: string;
  readonly orgDomain: string;
  readonly cliBinName: string;
  readonly desktopBinName: string;
  readonly primaryEnvPrefix: string;
  readonly fallbackEnvPrefix: string;
  readonly homeDirName: string;
  readonly fallbackHomeDirName: string;
  readonly urlScheme: string;
  readonly ipcPipePrefix: string;
  readonly httpHeaderPrefix: string;
  readonly fallbackHttpHeaderPrefix: string;
  readonly mcpToolPrefix: string;
  readonly fallbackMcpToolPrefix: string;
  readonly mcpNamespace: string;
  readonly fallbackMcpNamespace: string;
  readonly systemdUnitName: string;
  readonly launchdLabel: string;
  readonly windowsTaskName: string;
  readonly windowsPolicyKey: string;
  readonly macosPolicyDomain: string;
}

const ACTIVE_BRAND_FALLBACK: ActiveBrandConfig = {
  productName: "T3 Code",
  orgDomain: "t3.codes",
  cliBinName: "t3",
  desktopBinName: "t3-desktop",
  primaryEnvPrefix: "T3CODE_",
  fallbackEnvPrefix: "T3_",
  homeDirName: ".t3",
  fallbackHomeDirName: ".t3code",
  urlScheme: "t3code",
  ipcPipePrefix: "t3code",
  httpHeaderPrefix: "x-t3-",
  fallbackHttpHeaderPrefix: "t3-",
  mcpToolPrefix: "t3_",
  fallbackMcpToolPrefix: "legion_",
  mcpNamespace: "mcp__t3_code__",
  fallbackMcpNamespace: "mcp__legion_code__",
  systemdUnitName: "t3code.service",
  launchdLabel: "com.t3tools.t3code.service",
  windowsTaskName: "t3code",
  windowsPolicyKey: "SOFTWARE\\Policies\\T3Tools\\T3Code",
  macosPolicyDomain: "com.t3tools.t3code",
};

function normalizeNfdFilenameFallback(path: string): string {
  const trimmed = path.replace(TRAILING_SEPARATORS_RE, "");
  const basename =
    trimmed.length === 0
      ? path
      : (() => {
          const slash = trimmed.lastIndexOf("/");
          const backslash = trimmed.lastIndexOf("\\");
          const sepIndex = Math.max(slash, backslash);
          return sepIndex >= 0 ? trimmed.slice(sepIndex + 1) : trimmed;
        })();
  return basename.normalize("NFC");
}

/**
 * Extracts the trailing path segment across `/` and `\` separators and composes
 * macOS NFD filenames into canonical NFC.
 */
export function normalizeNfdFilename(raw: string): string {
  const wasm = resolveActiveWasmExports();
  if (wasm && typeof wasm.normalize_nfd_filename === "function") {
    return wasm.normalize_nfd_filename(raw);
  }
  return normalizeNfdFilenameFallback(raw);
}

interface C1Effect {
  readonly next: string;
  readonly query: boolean;
  readonly safe: string;
}

interface C1Hit {
  readonly effect: C1Effect;
  readonly index: number;
}

interface StringEnd {
  readonly end: number;
  readonly c1?: C1Hit;
}

function utf8BytesOfCodePoint(cp: number): readonly number[] {
  if (cp <= 0x7ff) {
    return [0xc0 | (cp >> 6), 0x80 | (cp & 0x3f)];
  }
  if (cp <= 0xffff) {
    return [0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f)];
  }
  return [
    0xf0 | (cp >> 18),
    0x80 | ((cp >> 12) & 0x3f),
    0x80 | ((cp >> 6) & 0x3f),
    0x80 | (cp & 0x3f),
  ];
}

function c1Effect(cp: number): C1Effect | undefined {
  let next: string | undefined;
  let query = false;
  let safe = "";
  for (const byte of utf8BytesOfCodePoint(cp)) {
    if (next === "") {
      safe += "\ufffd";
    } else if (next?.startsWith("\u001b]")) {
      next += "\ufffd";
    } else if (byte === 0x9b) {
      next = "\u001b[";
    } else if (byte === 0x9d) {
      next = "\u001b]";
    } else if (byte === 0x90) {
      next = "\u001bP";
    } else if (byte === 0x98 || byte === 0x9e || byte === 0x9f) {
      next = "\u001b_";
    } else if (byte >= 0x80 && byte <= 0x9f) {
      next = "";
      query = byte === 0x9a;
      safe = !query && byte !== 0x9c ? `\u001b${String.fromCharCode(byte - 0x40)}` : "";
    }
  }
  return next === undefined ? undefined : { next, query, safe };
}

function findStringEnd(input: string, start: number, isOsc: boolean): StringEnd | undefined {
  for (let pos = start; pos < input.length;) {
    const cp = input.codePointAt(pos)!;
    const width = cp > 0xffff ? 2 : 1;
    if ((isOsc && cp === 0x07) || cp === 0x18 || cp === 0x1a) {
      return { end: pos + 1 };
    }
    if (cp === 0x1b) {
      if (pos + 1 >= input.length) {
        return undefined;
      }
      const end = input.charCodeAt(pos + 1) === 0x5c ? pos + 2 : pos;
      return { end };
    }
    if (!isOsc && cp >= 0x80) {
      const effect = c1Effect(cp);
      if (effect) {
        return { end: pos + width, c1: { effect, index: pos } };
      }
    }
    pos += width;
  }
  return undefined;
}

function findEscapeSequenceEnd(input: string, start: number): number | undefined {
  let cursor = start;
  while (cursor < input.length) {
    const code = input.charCodeAt(cursor);
    if (code >= 0x20 && code <= 0x2f) {
      cursor += 1;
      continue;
    }
    if (code === 0x1b) {
      return cursor;
    }
    return code >= 0x30 && code <= 0x7e ? cursor + 1 : start + 1;
  }
  return undefined;
}

function stripStringTerminator(value: string): string {
  if (value.endsWith("\u001b\\")) {
    return value.slice(0, -2);
  }
  if (value.endsWith("\u0007") || value.endsWith("\u009c")) {
    return value.slice(0, -1);
  }
  return value;
}

function isTerminatedOscOrDcs(slice: string): boolean {
  return (
    slice.endsWith("\u0007") ||
    slice.endsWith("\u0018") ||
    slice.endsWith("\u001a") ||
    slice.endsWith("\u001b\\")
  );
}

function isOscPaletteQuery(content: string): boolean {
  let start = 0;
  while (start < content.length && content.charCodeAt(start) <= 0x1f) {
    start += 1;
  }
  if (!content.startsWith("4", start) || !content.includes("?")) {
    return false;
  }
  let cleaned = "";
  for (let i = 0; i < content.length; i += 1) {
    if (content.charCodeAt(i) > 0x1f) {
      cleaned += content[i]!;
    }
  }
  const fields = cleaned.split(";").filter((field) => field.length > 0);
  if (fields[0] !== "4") {
    return false;
  }
  let pairFieldCount = 0;
  for (let idx = 1; idx < fields.length; idx += 1) {
    const pairIdx = idx - 1;
    pairFieldCount += 1;
    if (pairIdx % 2 === 1 && fields[idx] !== "?") {
      return false;
    }
  }
  return pairFieldCount > 1;
}

function shouldStripOscSequence(content: string): boolean {
  const semi = content.indexOf(";");
  if (semi < 0) {
    return false;
  }
  const code = content.slice(0, semi);
  const rest = content.slice(semi + 1);
  return (
    (code === "10" || code === "11" || code === "12") &&
    (rest.startsWith("?") || rest.startsWith("rgb:"))
  );
}

function shouldStripDcsSequence(content: string): boolean {
  const rest = content.startsWith("0") || content.startsWith("1") ? content.slice(1) : content;
  return (rest.startsWith("$") || rest.startsWith("+")) && (rest[1] === "q" || rest[1] === "r");
}

function isAsciiDigitOrAny(body: string, allowedPunctuation: string): boolean {
  for (let i = 0; i < body.length; i += 1) {
    const ch = body[i]!;
    const code = body.charCodeAt(i);
    if ((code < 0x30 || code > 0x39) && !allowedPunctuation.includes(ch)) {
      return false;
    }
  }
  return true;
}

function shouldStripCsiSequence(body: string, finalCode: number): boolean {
  if (finalCode === 0x6e) {
    return true;
  }
  if (finalCode === 0x52) {
    return isAsciiDigitOrAny(body, ";?");
  }
  if (finalCode === 0x63) {
    return isAsciiDigitOrAny(body, ">=;?");
  }
  if (finalCode === 0x70 || finalCode === 0x79) {
    return body.endsWith("$") && isAsciiDigitOrAny(body.slice(0, -1), ";?");
  }
  if (finalCode === 0x71) {
    return body.startsWith(">") && isAsciiDigitOrAny(body.slice(1), ";");
  }
  if (finalCode === 0x75) {
    return body.startsWith("?");
  }
  return false;
}

function stripOsc4QueriesFallback(input: string): string {
  if (!input.includes("\u001b")) {
    return input;
  }
  let visibleText = "";
  let index = 0;
  let c1Introducer = "";

  while (index < input.length) {
    if (c1Introducer.length > 0 || input.charCodeAt(index) === 0x1b) {
      const prefix = c1Introducer;
      c1Introducer = "";
      const contentStart = prefix.length === 0 ? index + 2 : index;
      const nextCode =
        prefix.length === 0
          ? index + 1 < input.length
            ? input.charCodeAt(index + 1)
            : undefined
          : prefix.length > 1
            ? prefix.charCodeAt(1)
            : undefined;
      if (nextCode === undefined) {
        visibleText += input.slice(index);
        break;
      }

      if (nextCode === 0x5b) {
        let body = "";
        let executedControls = "";
        let advanced = false;
        for (let pos = contentStart; pos < input.length;) {
          const cp = input.codePointAt(pos)!;
          const width = cp > 0xffff ? 2 : 1;
          const cancelled = cp === 0x18 || cp === 0x1a;
          if (cancelled || (cp >= 0x40 && cp <= 0x7e)) {
            if (cancelled || !shouldStripCsiSequence(body, cp)) {
              visibleText += `${prefix}${input.slice(index, pos + width)}`;
            } else {
              visibleText += executedControls;
            }
            index = pos + width;
            advanced = true;
            break;
          }
          if (cp >= 0x80) {
            const end = pos + width;
            const effect = c1Effect(cp);
            if (effect) {
              if (effect.next.length === 0 && !effect.query) {
                visibleText += `${prefix}${input.slice(index, end)}`;
              } else {
                visibleText += `${executedControls}${effect.safe}`;
              }
              c1Introducer = effect.next;
              index = end;
              advanced = true;
              break;
            }
            pos = end;
            continue;
          }
          if (cp === 0x1b) {
            visibleText += executedControls;
            index = pos;
            advanced = true;
            break;
          }
          if (cp < 0x10) {
            executedControls += input.slice(pos, pos + width);
          } else if (cp >= 0x20 && cp !== 0x7f) {
            body += input.slice(pos, pos + width);
          }
          pos += width;
        }
        if (!advanced) {
          visibleText += `${prefix}${input.slice(index)}`;
          break;
        }
        continue;
      }

      if (nextCode === 0x5d || nextCode === 0x50 || nextCode === 0x5e || nextCode === 0x5f) {
        const isOsc = nextCode === 0x5d;
        const stringEnd = findStringEnd(input, contentStart, isOsc);
        if (!stringEnd) {
          visibleText += `${prefix}${input.slice(index)}`;
          break;
        }
        const end = stringEnd.end;
        const contentEnd = stringEnd.c1 ? stringEnd.c1.index : end;
        const prefixTail = prefix.length > 2 ? prefix.slice(2) : "";
        const rawContent = `${prefixTail}${input.slice(contentStart, contentEnd)}`;
        const content = stripStringTerminator(rawContent);

        if (isOsc && isOscPaletteQuery(content) && isTerminatedOscOrDcs(input.slice(index, end))) {
          visibleText += "\u001b\\";
          index = end;
          continue;
        }

        const strip =
          (isOsc && shouldStripOscSequence(content)) ||
          (nextCode === 0x50 && shouldStripDcsSequence(content));
        if (!stringEnd.c1) {
          if (!strip) {
            visibleText += `${prefix}${input.slice(index, end)}`;
          }
        } else {
          const hit = stringEnd.c1;
          if (!strip && hit.effect.next.length === 0 && !hit.effect.query) {
            visibleText += `${prefix}${input.slice(index, end)}`;
          } else {
            if (!strip) {
              visibleText += `${prefix}${input.slice(index, hit.index)}\u009c`;
            }
            visibleText += hit.effect.safe;
            c1Introducer = hit.effect.next;
          }
        }
        index = end;
        continue;
      }

      const escEnd = findEscapeSequenceEnd(input, index + 1);
      if (escEnd === undefined) {
        visibleText += input.slice(index);
        break;
      }
      if (nextCode !== 0x5a) {
        visibleText += input.slice(index, escEnd);
      }
      index = escEnd;
      continue;
    }

    const nextEsc = input.indexOf("\u001b", index + 1);
    const end = nextEsc >= 0 ? nextEsc : input.length;
    visibleText += input.slice(index, end);
    index = end;
  }

  return `${visibleText}${c1Introducer}`;
}

/**
 * Strips standalone `OSC 4` palette queries, `OSC 10/11/12` dynamic color queries,
 * and replayable `DCS`/`CSI` terminal queries from terminal history.
 */
export function stripOsc4Queries(raw: string): string {
  const wasm = resolveActiveWasmExports();
  if (wasm && typeof wasm.strip_osc4_queries === "function") {
    return wasm.strip_osc4_queries(raw);
  }
  return stripOsc4QueriesFallback(raw);
}

/**
 * Evaluates whether loopback preview tunnels are permitted by user settings
 * and any loaded managed policy.
 */
export function allowsPreviewTunnel(
  enablePreviewTunnel: boolean,
  managedPolicy?: PreviewTunnelPolicySummary | null,
): boolean {
  const wasm = resolveActiveWasmExports();
  if (wasm && typeof wasm.allows_preview_tunnel_json === "function") {
    const normalizedPolicy = managedPolicy
      ? {
          organization:
            typeof managedPolicy.organization === "string" && managedPolicy.organization.length > 0
              ? managedPolicy.organization
              : "Organization",
          lockedSettings: Array.isArray(managedPolicy.lockedSettings)
            ? managedPolicy.lockedSettings
            : [],
          ...managedPolicy,
        }
      : null;
    const response = JSON.parse(
      wasm.allows_preview_tunnel_json(
        JSON.stringify({
          enablePreviewTunnel,
          managedPolicy: normalizedPolicy,
        }),
      ),
    ) as { readonly allowed: boolean };
    return response.allowed;
  }
  return enablePreviewTunnel && managedPolicy?.limits?.previewTunnel !== false;
}

/**
 * Returns the compile-time active product brand configuration.
 */
export function activeBrand(): ActiveBrandConfig {
  const wasm = resolveActiveWasmExports();
  if (wasm && typeof wasm.active_brand_json === "function") {
    return JSON.parse(wasm.active_brand_json()) as ActiveBrandConfig;
  }
  return { ...ACTIVE_BRAND_FALLBACK };
}

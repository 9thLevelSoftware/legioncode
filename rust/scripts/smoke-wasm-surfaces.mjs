#!/usr/bin/env node
import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const rustRoot = path.resolve(scriptDir, "..");
const distDir = path.join(rustRoot, "dist");

const req = createRequire(import.meta.url);
const wasmNode = req(path.join(distDir, "wasm-nodejs", "legion_wasm_ffi.js"));

assert.equal(wasmNode.legion_wasm_abi_version(), 1);
assert.equal(wasmNode.sanitize_bracketed_paste_str("hi\x1b[201~evil\x00!"), "hievil!");

const client = new wasmNode.WasmClientCore(1_700_000_000_000);
try {
  assert.ok(client.next_backoff_ms(0) > 0, "WasmClientCore.next_backoff_ms(0) must be positive");
} finally {
  client.free?.();
}

const vt = new wasmNode.WasmTerminalVt(80, 24);
try {
  vt.feed("\x1b[32mok\x1b[0m");
  const snapshot = vt.export_lcvt();
  assert.ok(snapshot.length >= 36, `Expected LCVT snapshot >= 36 bytes, got ${snapshot.length}`);
} finally {
  vt.free?.();
}

const webAppDir = path.join(distDir, "web-app");
if (fs.existsSync(webAppDir)) {
  const webAppIndex = path.join(webAppDir, "index.html");
  assert.ok(fs.existsSync(webAppIndex), `Expected ${webAppIndex} to exist`);
  const html = fs.readFileSync(webAppIndex, "utf8");
  assert.ok(html.includes("<html") && html.includes("root"), "dist/web-app/index.html looks invalid");
  assert.ok(
    fs.existsSync(path.join(webAppDir, "legion_wasm_ffi.js")),
    "Expected dist/web-app/legion_wasm_ffi.js to exist",
  );
}

const runtimeUrl = pathToFileURL(path.join(distDir, "cloudflare-workers", "worker_runtime.mjs")).href;
const { createWorkerRuntime } = await import(runtimeUrl);
const { HookInboxObject, default: workerDefault } = createWorkerRuntime({
  initWasm: () => Promise.resolve(),
  wasmInput: null,
  WasmEdgeWorker: wasmNode.WasmEdgeWorker,
  WasmHookInboxObject: wasmNode.WasmHookInboxObject,
});

const scheduledReport = await workerDefault.scheduled({ cron: "* * * * *" }, {}, {});
assert.ok(
  Array.isArray(scheduledReport.d1Plans) && scheduledReport.d1Plans.length > 0,
  "Expected scheduled cron to produce d1Plans",
);

const storage = new Map();
const actor = new HookInboxObject(
  {
    storage: {
      get: async (key) => storage.get(key),
      put: async (key, value) => void storage.set(key, value),
      setAlarm: async () => {},
    },
  },
  {},
);
const summaryRes = await actor.fetch(new Request("https://do.internal/summary", { method: "GET" }));
assert.equal(summaryRes.status, 200);

process.stdout.write("[smoke-wasm-surfaces] All WASM and Worker surface checks passed.\n");

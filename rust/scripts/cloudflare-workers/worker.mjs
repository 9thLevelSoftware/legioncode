import init, { WasmEdgeWorker, WasmHookInboxObject } from "./legion_wasm_ffi.js";
import wasmModule from "./legion_wasm_ffi_bg.wasm";
import { createWorkerRuntime } from "./worker_runtime.mjs";

const { HookInboxObject, default: workerDefault } = createWorkerRuntime({
  initWasm: init,
  wasmInput: wasmModule,
  WasmEdgeWorker,
  WasmHookInboxObject,
});

export { HookInboxObject };
export default workerDefault;

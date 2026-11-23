import * as NodeFS from "node:fs";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";
import * as NodeUtil from "node:util";

const REQUIRED_WORKER_FILES = [
  "worker.mjs",
  "worker_runtime.mjs",
  "wrangler.toml",
  "legion_wasm_ffi.js",
  "legion_wasm_ffi_bg.wasm",
];

const scriptDir = NodePath.dirname(NodeURL.fileURLToPath(import.meta.url));
const packageDir = NodePath.resolve(scriptDir, "..");
const repoRoot = NodePath.resolve(packageDir, "../..");
const rustWorkersDistDir = NodePath.join(repoRoot, "rust", "dist", "cloudflare-workers");

const { values } = NodeUtil.parseArgs({
  options: {
    check: { type: "boolean", default: false },
    "out-dir": { type: "string" },
  },
});

const legacyWorkerFlag = process.env.LEGIONCODE_LEGACY_WORKER?.trim().toLowerCase();
const isLegacyWorker = legacyWorkerFlag === "1" || legacyWorkerFlag === "true";

function assertValidWorkerDist(sourceDir) {
  const missing = [];
  for (const fileName of REQUIRED_WORKER_FILES) {
    const fullPath = NodePath.join(sourceDir, fileName);
    try {
      const stat = NodeFS.statSync(fullPath);
      if (!stat.isFile() || stat.size === 0) {
        missing.push(fileName);
      }
    } catch {
      missing.push(fileName);
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `Missing or empty Rust Cloudflare Worker artifacts in ${sourceDir}:\n${missing.map((item) => `- ${item}`).join("\n")}`,
    );
  }
}

if (values.check) {
  assertValidWorkerDist(rustWorkersDistDir);
  if (values["out-dir"]) {
    assertValidWorkerDist(NodePath.resolve(values["out-dir"]));
  }
  process.stdout.write(
    `Verified ${REQUIRED_WORKER_FILES.length} Rust worker artifacts in ${rustWorkersDistDir}\n`,
  );
} else if (isLegacyWorker) {
  process.stdout.write(
    "Skipping Rust worker staging because LEGIONCODE_LEGACY_WORKER is enabled.\n",
  );
} else {
  assertValidWorkerDist(rustWorkersDistDir);
  const targetDir = values["out-dir"]
    ? NodePath.resolve(values["out-dir"])
    : NodePath.join(packageDir, "dist", "rust-worker");
  NodeFS.mkdirSync(targetDir, { recursive: true });
  NodeFS.cpSync(rustWorkersDistDir, targetDir, { recursive: true, force: true });
  assertValidWorkerDist(targetDir);
  process.stdout.write(`Staged Rust worker artifacts into ${targetDir}\n`);
}

import * as NodeFS from "node:fs";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";
import * as NodeUtil from "node:util";

const REQUIRED_MOBILE_FFI_FILES = [
  "include/t3_mobile_ffi.h",
  "swift/T3MobileCore.swift",
  "kotlin/T3MobileCore.kt",
  "android/T3MobileBridge.kt",
  "apple/Package.swift",
];

const scriptDir = NodePath.dirname(NodeURL.fileURLToPath(import.meta.url));
const mobileDir = NodePath.resolve(scriptDir, "..");
const repoRoot = NodePath.resolve(mobileDir, "../..");
const rustMobileDistDir = NodePath.join(repoRoot, "rust", "dist", "mobile");

const { values } = NodeUtil.parseArgs({
  options: {
    check: { type: "boolean", default: false },
    "out-dir": { type: "string" },
  },
});

function assertValidMobileDist(sourceDir) {
  const missing = [];
  for (const relativePath of REQUIRED_MOBILE_FFI_FILES) {
    const fullPath = NodePath.join(sourceDir, relativePath);
    try {
      const stat = NodeFS.statSync(fullPath);
      if (!stat.isFile() || stat.size === 0) {
        missing.push(relativePath);
      }
    } catch {
      missing.push(relativePath);
    }
  }
  if (missing.length > 0) {
    throw new Error(
      `Missing or empty Rust mobile FFI artifacts in ${sourceDir}:\n${missing.map((item) => `- ${item}`).join("\n")}`,
    );
  }
}

assertValidMobileDist(rustMobileDistDir);

if (values.check) {
  if (values["out-dir"]) {
    assertValidMobileDist(NodePath.resolve(values["out-dir"]));
  }
  process.stdout.write(
    `Verified ${REQUIRED_MOBILE_FFI_FILES.length} Rust mobile FFI artifacts in ${rustMobileDistDir}\n`,
  );
} else {
  const targetDir = values["out-dir"]
    ? NodePath.resolve(values["out-dir"])
    : NodePath.join(mobileDir, ".generated", "rust-mobile-ffi");
  NodeFS.mkdirSync(targetDir, { recursive: true });
  NodeFS.cpSync(rustMobileDistDir, targetDir, { recursive: true, force: true });
  assertValidMobileDist(targetDir);
  process.stdout.write(`Synced Rust mobile FFI artifacts to ${targetDir}\n`);
}

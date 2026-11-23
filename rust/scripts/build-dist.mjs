#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const rustRoot = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(rustRoot, "..");

const VALID_SURFACES = new Set([
  "all",
  "cli",
  "desktop",
  "mobile",
  "mobile-sdk",
  "wasm",
  "web-app",
  "cloudflare-workers",
]);

const WASM_WEB_FILES = ["legion_wasm_ffi.js", "legion_wasm_ffi.d.ts", "legion_wasm_ffi_bg.wasm"];

function run(command, args, cwd = repoRoot, options = {}) {
  const rendered = `${command} ${args.join(" ")}`;
  process.stdout.write(`[build-dist] ${rendered}\n`);
  const result = spawnSync(command, args, {
    cwd,
    stdio: "inherit",
    ...(options.shell ? { shell: true } : {}),
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`Command failed (${String(result.status)}): ${rendered}`);
  }
}

function readWorkspaceVersion() {
  const cargoToml = fs.readFileSync(path.join(rustRoot, "Cargo.toml"), "utf8");
  let inWorkspacePackage = false;
  for (const line of cargoToml.split(/\r?\n/u)) {
    const trimmed = line.trim();
    const section = /^\[([^\]]+)\]$/u.exec(trimmed);
    if (section) {
      inWorkspacePackage = section[1].trim() === "workspace.package";
      continue;
    }
    if (!inWorkspacePackage) continue;
    const match = /^version\s*=\s*"([^"]+)"/u.exec(trimmed);
    if (match) return match[1];
  }
  throw new Error("Could not find [workspace.package].version in rust/Cargo.toml");
}

function isHostCompatibleTarget(target) {
  if (!target) return true;
  if (process.platform === "win32") return target.includes("windows");
  if (process.platform === "darwin") return target.includes("apple-darwin");
  if (process.platform === "linux") return target.includes("linux");
  return false;
}

function copyFileEnsured(src, dest, executable = false) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
  if (executable && process.platform !== "win32") {
    fs.chmodSync(dest, 0o755);
  }
}

function copyDirContents(srcDir, destDir) {
  fs.mkdirSync(destDir, { recursive: true });
  fs.cpSync(srcDir, destDir, { recursive: true, force: true });
}

function replaceDirContents(srcDir, destDir) {
  fs.rmSync(destDir, { recursive: true, force: true });
  copyDirContents(srcDir, destDir);
}

function stageWebBundle(webDistDir, destDir, wasmSourceDir) {
  replaceDirContents(webDistDir, destDir);
  if (wasmSourceDir && fs.existsSync(path.join(wasmSourceDir, "legion_wasm_ffi.js"))) {
    for (const fileName of WASM_WEB_FILES) {
      copyFileEnsured(path.join(wasmSourceDir, fileName), path.join(destDir, fileName));
    }
  }
}

function writeWasmPackageJson(dir, name, version, moduleType) {
  const manifest = {
    name,
    version,
    type: moduleType,
    main: "legion_wasm_ffi.js",
    types: "legion_wasm_ffi.d.ts",
    files: ["legion_wasm_ffi_bg.wasm", "legion_wasm_ffi.js", "legion_wasm_ffi.d.ts"],
  };
  fs.writeFileSync(path.join(dir, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
}

function collectFiles(dir, baseDir = dir) {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectFiles(full, baseDir));
    } else if (entry.isFile()) {
      const rel = path.relative(baseDir, full).split(path.sep).join("/");
      if (rel !== "SHA256SUMS.txt") {
        files.push({ rel, full });
      }
    }
  }
  return files;
}

function writeSha256Sums(outDir) {
  const files = collectFiles(outDir).sort((a, b) => a.rel.localeCompare(b.rel));
  const lines = files.map(({ rel, full }) => {
    const hash = crypto.createHash("sha256").update(fs.readFileSync(full)).digest("hex");
    return `${hash}  ${rel}`;
  });
  fs.writeFileSync(path.join(outDir, "SHA256SUMS.txt"), `${lines.join("\n")}\n`);
  process.stdout.write(`[build-dist] Wrote SHA256SUMS.txt (${lines.length} artifacts)\n`);
}

const { values } = parseArgs({
  options: {
    surfaces: { type: "string", default: "all" },
    target: { type: "string" },
    "out-dir": { type: "string" },
    "skip-web-build": { type: "boolean", default: false },
  },
});

const requested = new Set(
  values.surfaces
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean),
);
for (const surface of requested) {
  if (!VALID_SURFACES.has(surface)) {
    throw new Error(`Unknown surface '${surface}'. Expected one of: ${[...VALID_SURFACES].join(", ")}`);
  }
}

const buildAll = requested.has("all");
const wantCli = buildAll || requested.has("cli");
const wantDesktop = buildAll || requested.has("desktop");
const wantMobileNative = buildAll || requested.has("mobile");
const wantMobileSdk = wantMobileNative || requested.has("mobile-sdk");
const wantWebApp = buildAll || requested.has("web-app");
const needWebBundle = wantWebApp || wantCli || wantDesktop;
const wantCloudflareWorkers = buildAll || requested.has("cloudflare-workers");
const wantWasm = buildAll || requested.has("wasm") || wantWebApp || wantCloudflareWorkers;

const target = values.target?.trim() || undefined;
const outDir = path.resolve(values["out-dir"] || path.join(rustRoot, "dist"));
const manifestPath = path.join(rustRoot, "Cargo.toml");
const version = readWorkspaceVersion();
const isWindowsTarget = target ? target.includes("windows") : process.platform === "win32";
const exeExt = isWindowsTarget ? ".exe" : "";
const targetReleaseDir = target
  ? path.join(rustRoot, "target", target, "release")
  : path.join(rustRoot, "target", "release");
const webDistDir = path.join(repoRoot, "apps", "web", "dist");
const webDistIndex = path.join(webDistDir, "index.html");
const skipWebBuild = Boolean(values["skip-web-build"]);

fs.mkdirSync(outDir, { recursive: true });

if (needWebBundle) {
  if (skipWebBuild && fs.existsSync(webDistIndex)) {
    process.stdout.write(`[build-dist] Reusing existing @legioncode/web build at ${webDistDir}\n`);
  } else {
    run("pnpm", ["--filter", "@legioncode/web", "build"], repoRoot, {
      shell: process.platform === "win32",
    });
  }
  if (!fs.existsSync(webDistIndex)) {
    throw new Error(`Missing built @legioncode/web bundle at ${webDistIndex}`);
  }
}

const nativePackages = [
  ...(wantCli ? ["legion-cli"] : []),
  ...(wantDesktop ? ["legion-desktop-shell"] : []),
  ...(wantMobileNative ? ["legion-mobile-ffi"] : []),
];

if (nativePackages.length > 0) {
  const cargoArgs = [
    "build",
    "--locked",
    "--release",
    "--manifest-path",
    manifestPath,
    ...(target ? ["--target", target] : []),
    ...nativePackages.flatMap((pkg) => ["-p", pkg]),
  ];
  run("cargo", cargoArgs, repoRoot);
}

if (wantCli) {
  const cliSrc = path.join(targetReleaseDir, `legioncode${exeExt}`);
  const cliDest = path.join(outDir, "bin", `legioncode${exeExt}`);
  copyFileEnsured(cliSrc, cliDest, !isWindowsTarget);
}

if (wantDesktop) {
  const desktopDir = path.join(outDir, "desktop");
  const desktopSrc = path.join(targetReleaseDir, `legioncode-desktop${exeExt}`);
  const desktopDest = path.join(desktopDir, `legioncode-desktop${exeExt}`);
  copyFileEnsured(desktopSrc, desktopDest, !isWindowsTarget);

  if (isHostCompatibleTarget(target)) {
    run(desktopDest, ["--emit-bundle", desktopDir], repoRoot);
    const desktopServicesDir = path.join(desktopDir, "services");
    if (fs.existsSync(desktopServicesDir)) {
      copyDirContents(desktopServicesDir, path.join(outDir, "services"));
    }
    const launcherSh = path.join(desktopDir, "linux", "legioncode-launcher.sh");
    if (fs.existsSync(launcherSh) && process.platform !== "win32") {
      fs.chmodSync(launcherSh, 0o755);
    }
  } else {
    process.stdout.write(
      `[build-dist] Skipping --emit-bundle because target '${target}' is not host-executable.\n`,
    );
  }
}

if (wantMobileSdk) {
  const mobileDir = path.join(outDir, "mobile");
  run(
    "cargo",
    [
      "run",
      "--locked",
      "--release",
      "--manifest-path",
      manifestPath,
      "-p",
      "legion-mobile-ffi",
      "--example",
      "emit_mobile_dist",
      "--",
      mobileDir,
    ],
    repoRoot,
  );
  if (process.platform !== "win32") {
    for (const shRel of ["apple/build-xcframework.sh", "android/build-jni.sh"]) {
      const shPath = path.join(mobileDir, shRel);
      if (fs.existsSync(shPath)) fs.chmodSync(shPath, 0o755);
    }
  }
  if (wantMobileNative) {
    const mobileArtifacts = [
      "legion_mobile_ffi.dll",
      "legion_mobile_ffi.dll.lib",
      "liblegion_mobile_ffi.so",
      "liblegion_mobile_ffi.dylib",
      "liblegion_mobile_ffi.a",
    ];
    for (const fileName of mobileArtifacts) {
      const candidate = path.join(targetReleaseDir, fileName);
      if (fs.existsSync(candidate)) {
        copyFileEnsured(candidate, path.join(mobileDir, fileName));
      }
    }
  }
}

const wasmWebDir = path.join(outDir, "wasm-web");

if (wantWasm) {
  run(
    "cargo",
    [
      "build",
      "--locked",
      "--release",
      "--manifest-path",
      manifestPath,
      "--target",
      "wasm32-unknown-unknown",
      "-p",
      "legion-wasm-ffi",
    ],
    repoRoot,
  );
  const wasmBinary = path.join(
    rustRoot,
    "target",
    "wasm32-unknown-unknown",
    "release",
    "legion_wasm_ffi.wasm",
  );
  if (!fs.existsSync(wasmBinary)) {
    throw new Error(`Missing built WASM binary at ${wasmBinary}`);
  }

  const wasmNodeDir = path.join(outDir, "wasm-nodejs");
  fs.mkdirSync(wasmWebDir, { recursive: true });
  fs.mkdirSync(wasmNodeDir, { recursive: true });

  run("wasm-bindgen", ["--target", "web", "--out-dir", wasmWebDir, wasmBinary], repoRoot);
  run("wasm-bindgen", ["--target", "nodejs", "--out-dir", wasmNodeDir, wasmBinary], repoRoot);

  for (const dir of [wasmWebDir, wasmNodeDir]) {
    const gitignore = path.join(dir, ".gitignore");
    if (fs.existsSync(gitignore)) fs.rmSync(gitignore);
  }

  writeWasmPackageJson(wasmWebDir, "@legioncode/legion-wasm-ffi", version, "module");
  writeWasmPackageJson(wasmNodeDir, "@legioncode/legion-wasm-ffi-node", version, "commonjs");

  if (wantCloudflareWorkers) {
    const workerDir = path.join(outDir, "cloudflare-workers");
    const workerTemplatesDir = path.join(scriptDir, "cloudflare-workers");
    fs.mkdirSync(workerDir, { recursive: true });
    for (const fileName of ["worker.mjs", "worker_runtime.mjs", "wrangler.toml"]) {
      copyFileEnsured(path.join(workerTemplatesDir, fileName), path.join(workerDir, fileName));
    }
    for (const fileName of WASM_WEB_FILES) {
      copyFileEnsured(path.join(wasmWebDir, fileName), path.join(workerDir, fileName));
    }
  }
}

if (wantWebApp) {
  stageWebBundle(webDistDir, path.join(outDir, "web-app"), wasmWebDir);
}
if (wantCli && (wantWebApp || fs.existsSync(webDistIndex))) {
  stageWebBundle(webDistDir, path.join(outDir, "bin", "client"), wasmWebDir);
}
if (wantDesktop && (wantWebApp || fs.existsSync(webDistIndex))) {
  stageWebBundle(webDistDir, path.join(outDir, "desktop", "web-app"), wasmWebDir);
}

writeSha256Sums(outDir);

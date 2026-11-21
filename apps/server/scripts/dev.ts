// @effect-diagnostics nodeBuiltinImport:off globalConsole:off globalDate:off globalTimers:off - Standalone Node watch launcher.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";

import { isLegacyNodeForced, resolveRustCliBinary } from "../src/rustCutover.ts";

const serverDir = NodePath.resolve(import.meta.dirname, "..");
const repoRoot = NodePath.resolve(serverDir, "../..");
const entry = "src/bin.ts";
const legacyForced = isLegacyNodeForced(process.env);
const packageDirs = [
  serverDir,
  ...NodeFS.globSync("../../packages/*/package.json", { cwd: serverDir }).map((file) =>
    NodePath.resolve(serverDir, NodePath.dirname(file)),
  ),
];

const resolveLaunchTarget = () => {
  if (!legacyForced) {
    const rustBinary = resolveRustCliBinary({
      repoRoot,
      // oxlint-disable-next-line t3code/no-global-process-runtime -- Standalone Node watch launcher has no Effect runtime.
      platform: process.platform,
      env: process.env,
    });
    if (rustBinary !== undefined) {
      return {
        command: rustBinary,
        args: process.argv.slice(2),
        label: NodePath.relative(repoRoot, rustBinary),
        isRust: true,
      };
    }
  }
  return {
    command: process.execPath,
    args: [entry, ...process.argv.slice(2)],
    label: entry,
    isRust: false,
  };
};

const initialTarget = resolveLaunchTarget();
let activeLabel = initialTarget.label;

let child: NodeChildProcess.ChildProcess | undefined;
let startedAt = 0;
let restartPending = false;
let stopping = false;
let debounce: NodeJS.Timeout | undefined;
const changedPaths = new Set<string>();

// `node --watch` cannot be used: on Windows, libuv reports NTFS last-access updates as changes,
// so merely reading a source file (or an installed dependency directory) restarted the server.
// A path counts as changed only when it is gone or written since the running server started.
const isWrittenSinceStart = (path: string) => {
  try {
    const stats = NodeFS.statSync(path);
    return !stats.isDirectory() && Math.max(stats.mtimeMs, stats.ctimeMs) >= startedAt;
  } catch {
    return true;
  }
};

const start = () => {
  const target = resolveLaunchTarget();
  activeLabel = target.label;
  startedAt = Date.now();
  restartPending = false;
  child = NodeChildProcess.spawn(target.command, target.args, {
    cwd: serverDir,
    stdio: "inherit",
  });
  child.on("error", (error) => console.error(error));
  child.on("exit", (code, signal) => {
    child = undefined;
    if (stopping) {
      process.exitCode = code ?? (signal === null ? 1 : 128 + NodeOS.constants.signals[signal]);
      return;
    }
    if (restartPending) return start();
    console.log(
      `[dev] '${activeLabel}' exited (${signal ?? code}). Waiting for file changes before restarting...`,
    );
  });
};

const flush = () => {
  debounce = undefined;
  const changed = [...changedPaths].find(isWrittenSinceStart);
  changedPaths.clear();
  if (changed === undefined || stopping || restartPending) return;
  console.log(
    `[dev] Restarting '${activeLabel}': ${NodePath.relative(serverDir, changed)} changed`,
  );
  if (!child) return start();
  restartPending = true;
  child.kill("SIGTERM");
};

const onChange = (path: string) => {
  changedPaths.add(path);
  debounce ??= setTimeout(flush, 200);
};

// Watch each workspace package's package.json, plus rust/dist/bin in Rust mode or src/ in legacy mode.
const rustDistBinDir = NodePath.join(repoRoot, "rust", "dist", "bin");
const rustTargetDir = NodePath.dirname(initialTarget.command);
const rustTargetName = NodePath.basename(initialTarget.command);
const watchers = [
  ...packageDirs.flatMap((dir) => {
    const src = NodePath.join(dir, "src");
    return [
      NodeFS.watch(dir, (_event, name) => {
        if (name === "package.json") onChange(NodePath.join(dir, name));
      }),
      ...(!initialTarget.isRust && NodeFS.existsSync(src)
        ? [
            NodeFS.watch(src, { recursive: true }, (_event, name) =>
              onChange(name ? NodePath.join(src, name) : src),
            ),
          ]
        : []),
    ];
  }),
  ...(initialTarget.isRust && NodeFS.existsSync(rustDistBinDir)
    ? [
        NodeFS.watch(rustDistBinDir, (_event, name) =>
          onChange(name ? NodePath.join(rustDistBinDir, name) : initialTarget.command),
        ),
      ]
    : []),
  ...(initialTarget.isRust && rustTargetDir !== rustDistBinDir && NodeFS.existsSync(rustTargetDir)
    ? [
        NodeFS.watch(rustTargetDir, (_event, name) => {
          if (!name || name === rustTargetName) {
            onChange(initialTarget.command);
          }
        }),
      ]
    : []),
];

const stop = (signal: NodeJS.Signals) => {
  stopping = true;
  clearTimeout(debounce);
  for (const watcher of watchers) watcher.close();
  if (child) child.kill(signal);
};
process.on("SIGINT", () => stop("SIGINT"));
process.on("SIGTERM", () => stop("SIGTERM"));
process.on("SIGHUP", () => stop("SIGTERM"));

start();

import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";

import { desktopDir, resolveElectronLaunchCommand } from "./electron-launcher.mjs";

// oxlint-disable-next-line t3code/no-global-process-runtime -- Standalone desktop launcher script has no Effect runtime.
const hostPlatform = NodeOS.platform();
const repoRoot = NodePath.resolve(desktopDir, "..", "..");

function isFile(filePath) {
  try {
    return NodeFS.statSync(filePath).isFile();
  } catch {
    return false;
  }
}

function resolveRustDesktopBinary() {
  const legacyFlag = process.env.T3CODE_LEGACY_ELECTRON?.trim().toLowerCase();
  if (legacyFlag === "1" || legacyFlag === "true") {
    return undefined;
  }

  const binaryName = hostPlatform === "win32" ? "t3-desktop.exe" : "t3-desktop";
  const candidates = [
    NodePath.join(repoRoot, "rust", "dist", "desktop", binaryName),
    NodePath.join(repoRoot, "rust", "target", "release", binaryName),
  ];
  return candidates.find(isFile);
}

function spawnAndMirrorExit(command, args, options) {
  const child = NodeChildProcess.spawn(command, args, options);
  child.on("error", (error) => {
    process.stderr.write(`Failed to launch ${command}: ${error.message}\n`);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 0);
  });
}

const rustDesktopBinary = resolveRustDesktopBinary();
if (rustDesktopBinary) {
  const passedArgs = process.argv.slice(2);
  const args = passedArgs.length > 0 ? passedArgs : ["--launch"];
  spawnAndMirrorExit(rustDesktopBinary, args, { stdio: "inherit" });
} else {
  NodeChildProcess.execFileSync(
    process.execPath,
    [NodePath.join(desktopDir, "scripts/build-browser-secret.mjs")],
    { stdio: "inherit" },
  );

  const childEnv = { ...process.env };
  delete childEnv.ELECTRON_RUN_AS_NODE;

  const electronCommand = resolveElectronLaunchCommand(["dist-electron/main.cjs"]);
  spawnAndMirrorExit(electronCommand.electronPath, electronCommand.args, {
    stdio: "inherit",
    cwd: desktopDir,
    env: childEnv,
  });
}

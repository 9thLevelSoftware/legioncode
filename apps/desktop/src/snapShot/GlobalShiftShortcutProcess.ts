// @effect-diagnostics globalTimers:off -- The child-process shutdown watchdog runs at a worker callback boundary outside any Effect fiber.
// @effect-diagnostics nodeBuiltinImport:off -- This desktop-only helper owns a Node child process for global shortcut capture.

import * as NodeChildProcess from "node:child_process";

import type { SnapShotModifier } from "@legioncode/contracts";

export function startGlobalShiftShortcutProcess(
  workerPath: string,
  modifier: SnapShotModifier,
  onTrigger: () => void,
  onFailure: (error: Error) => void,
  options?: { readonly signal?: AbortSignal; readonly startupTimeoutMs?: number },
): Promise<() => void> {
  const worker = NodeChildProcess.fork(workerPath, [modifier], {
    env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" },
    execArgv: [],
    stdio: ["ignore", "ignore", "inherit", "ipc"],
  });

  return new Promise((resolve, reject) => {
    let settled = false;
    let stopped = false;
    let startupTimer: ReturnType<typeof setTimeout> | undefined;
    const onAbort = () => fail(new Error("Snapshot shortcut helper startup aborted"));
    const clearStartup = () => {
      if (startupTimer !== undefined) {
        clearTimeout(startupTimer);
        startupTimer = undefined;
      }
      options?.signal?.removeEventListener("abort", onAbort);
    };
    const stop = () => {
      clearStartup();
      if (stopped) return;
      stopped = true;
      worker.kill();
      const forceKill = setTimeout(() => worker.kill("SIGKILL"), 1_000);
      forceKill.unref?.();
      worker.once("exit", () => clearTimeout(forceKill));
    };
    const fail = (error: Error) => {
      clearStartup();
      if (stopped) return;
      if (settled) {
        stop();
        onFailure(error);
        return;
      }
      settled = true;
      stop();
      reject(error);
    };

    if (options?.signal?.aborted) {
      fail(new Error("Snapshot shortcut helper startup aborted"));
      return;
    }
    options?.signal?.addEventListener("abort", onAbort, { once: true });
    startupTimer = setTimeout(
      () => fail(new Error("Snapshot shortcut helper timed out waiting for ready")),
      options?.startupTimeoutMs ?? 5_000,
    );
    startupTimer.unref?.();

    worker.on("message", (message) => {
      if (message === "ready" && !settled) {
        settled = true;
        clearStartup();
        resolve(stop);
        return;
      }
      if (message !== "trigger" || !settled || stopped) return;
      try {
        onTrigger();
      } catch {}
    });
    worker.once("error", (error) => {
      fail(error);
    });
    worker.once("exit", (code) => {
      fail(new Error(`Snapshot shortcut helper exited with code ${code}`));
    });
  });
}

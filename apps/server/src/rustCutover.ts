// @effect-diagnostics nodeBuiltinImport:off -- Pre-Effect CLI bootstrap delegates to the Rust binary without loading Effect.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeSea from "node:sea";
import * as NodeURL from "node:url";

export interface ResolveRustCliBinaryOptions {
  readonly repoRoot?: string | undefined;
  readonly platform?: NodeJS.Platform | undefined;
  readonly env?: NodeJS.ProcessEnv | undefined;
}

export interface TryRunRustCliCutoverOptions extends ResolveRustCliBinaryOptions {
  readonly argv?: ReadonlyArray<string> | undefined;
  readonly isSea?: (() => boolean) | undefined;
  readonly spawn?: typeof NodeChildProcess.spawn | undefined;
  readonly processEvents?: Pick<NodeJS.Process, "on" | "off"> | undefined;
}

function defaultRepoRoot(): string {
  return NodePath.resolve(NodePath.dirname(NodeURL.fileURLToPath(import.meta.url)), "../../..");
}

function isExistingFile(filePath: string): boolean {
  try {
    return NodeFS.statSync(filePath).isFile();
  } catch {
    return false;
  }
}

function exitCodeFromChildExit(code: number | null, signal: NodeJS.Signals | null): number {
  if (code !== null) return code;
  if (signal === null) return 1;
  const signalNumber = NodeOS.constants.signals[signal];
  return signalNumber !== undefined ? 128 + signalNumber : 1;
}

/** Returns true when `LEGIONCODE_LEGACY_NODE` forces execution through the legacy Node server. */
export function isLegacyNodeForced(env: NodeJS.ProcessEnv = process.env): boolean {
  const value = env.LEGIONCODE_LEGACY_NODE?.trim().toLowerCase();
  return value === "1" || value === "true";
}

/** Resolves the Rust `legioncode` executable from `LEGIONCODE_RUST_BIN`, `rust/dist/bin`, or `rust/target/release`. */
export function resolveRustCliBinary(
  options: ResolveRustCliBinaryOptions = {},
): string | undefined {
  const env = options.env ?? process.env;
  // oxlint-disable-next-line legioncode/no-global-process-runtime -- Pre-Effect CLI bootstrap cannot import HostProcess.
  const platform = options.platform ?? process.platform;
  const repoRoot = options.repoRoot ?? defaultRepoRoot();
  const binaryName = platform === "win32" ? "legioncode.exe" : "legioncode";
  const overrideBin = env.LEGIONCODE_RUST_BIN?.trim();

  const candidates = [
    ...(overrideBin ? [NodePath.resolve(overrideBin)] : []),
    NodePath.join(repoRoot, "rust", "dist", "bin", binaryName),
    NodePath.join(repoRoot, "rust", "target", "release", binaryName),
  ];
  return candidates.find(isExistingFile);
}

/**
 * Runs the Rust `legioncode` binary with `argv.slice(2)` unless running inside a Node SEA,
 * `LEGIONCODE_LEGACY_NODE` is forced, or no Rust binary is available.
 */
export async function tryRunRustCliCutover(
  options: TryRunRustCliCutoverOptions = {},
): Promise<number | undefined> {
  if ((options.isSea ?? NodeSea.isSea)()) {
    return undefined;
  }
  const env = options.env ?? process.env;
  if (isLegacyNodeForced(env)) {
    return undefined;
  }
  const binaryPath = resolveRustCliBinary({
    repoRoot: options.repoRoot,
    platform: options.platform,
    env,
  });
  if (binaryPath === undefined) {
    return undefined;
  }

  const args = (options.argv ?? process.argv).slice(2);
  const spawnFn = options.spawn ?? NodeChildProcess.spawn;
  const events = options.processEvents ?? process;

  return await new Promise<number | undefined>((resolve) => {
    let settled = false;
    let child: NodeChildProcess.ChildProcess;

    const cleanup = () => {
      events.off("SIGINT", onSigInt);
      events.off("SIGTERM", onSigTerm);
    };

    const forwardSignal = (signal: NodeJS.Signals) => {
      if (settled) return;
      try {
        child.kill(signal);
      } catch {
        // Child may already have exited before the forwarded signal was delivered.
      }
    };

    const onSigInt = () => forwardSignal("SIGINT");
    const onSigTerm = () => forwardSignal("SIGTERM");

    try {
      child = spawnFn(binaryPath, args, {
        stdio: "inherit",
        env,
      });
    } catch {
      resolve(undefined);
      return;
    }

    events.on("SIGINT", onSigInt);
    events.on("SIGTERM", onSigTerm);

    child.once("error", () => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(undefined);
    });

    child.once("exit", (code, signal) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(exitCodeFromChildExit(code, signal));
    });
  });
}

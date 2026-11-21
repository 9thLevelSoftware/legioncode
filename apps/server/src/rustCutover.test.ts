// @effect-diagnostics nodeBuiltinImport:off -- Exercises child process and EventEmitter cutover behavior directly.
import type * as NodeChildProcess from "node:child_process";
import * as NodeEvents from "node:events";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";

import * as NodeServices from "@effect/platform-node/NodeServices";
import { assert, it } from "@effect/vitest";
import * as HostProcess from "@t3tools/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as FileSystem from "effect/FileSystem";
import * as Path from "effect/Path";

import { isLegacyNodeForced, resolveRustCliBinary, tryRunRustCliCutover } from "./rustCutover.ts";

class FakeChildProcess extends NodeEvents.EventEmitter {
  readonly killedSignals: Array<NodeJS.Signals | number | undefined> = [];

  kill(signal?: NodeJS.Signals | number): boolean {
    this.killedSignals.push(signal);
    return true;
  }
}

it.layer(NodeServices.layer)("rustCutover", (it) => {
  it("parses T3CODE_LEGACY_NODE case-insensitively with whitespace trimming", () => {
    assert.isFalse(isLegacyNodeForced({}));
    assert.isFalse(isLegacyNodeForced({ T3CODE_LEGACY_NODE: "" }));
    assert.isFalse(isLegacyNodeForced({ T3CODE_LEGACY_NODE: "0" }));
    assert.isFalse(isLegacyNodeForced({ T3CODE_LEGACY_NODE: "false" }));
    assert.isTrue(isLegacyNodeForced({ T3CODE_LEGACY_NODE: "1" }));
    assert.isTrue(isLegacyNodeForced({ T3CODE_LEGACY_NODE: " 1 " }));
    assert.isTrue(isLegacyNodeForced({ T3CODE_LEGACY_NODE: "true" }));
    assert.isTrue(isLegacyNodeForced({ T3CODE_LEGACY_NODE: " True " }));
  });

  it.effect("resolves T3CODE_RUST_BIN, dist/bin, and target/release in priority order", () =>
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem;
      const path = yield* Path.Path;
      const root = yield* fs.makeTempDirectoryScoped({ prefix: "t3-rust-cutover-" });
      const customBin = path.join(root, "custom-t3");
      const distBinWin = path.join(root, "rust", "dist", "bin", "t3.exe");
      const releaseBinWin = path.join(root, "rust", "target", "release", "t3.exe");
      const distBinPosix = path.join(root, "rust", "dist", "bin", "t3");
      const releaseBinPosix = path.join(root, "rust", "target", "release", "t3");

      assert.isUndefined(resolveRustCliBinary({ repoRoot: root, platform: "win32", env: {} }));

      // Directories sharing the binary name must not be treated as executables.
      yield* fs.makeDirectory(distBinWin, { recursive: true });
      assert.isUndefined(resolveRustCliBinary({ repoRoot: root, platform: "win32", env: {} }));
      yield* fs.remove(distBinWin, { recursive: true });

      yield* fs.makeDirectory(path.dirname(releaseBinWin), { recursive: true });
      yield* fs.writeFileString(releaseBinWin, "release-win");
      yield* fs.writeFileString(releaseBinPosix, "release-posix");
      assert.strictEqual(
        resolveRustCliBinary({ repoRoot: root, platform: "win32", env: {} }),
        releaseBinWin,
      );
      assert.strictEqual(
        resolveRustCliBinary({ repoRoot: root, platform: "linux", env: {} }),
        releaseBinPosix,
      );

      yield* fs.makeDirectory(path.dirname(distBinWin), { recursive: true });
      yield* fs.writeFileString(distBinWin, "dist-win");
      yield* fs.writeFileString(distBinPosix, "dist-posix");
      assert.strictEqual(
        resolveRustCliBinary({ repoRoot: root, platform: "win32", env: {} }),
        distBinWin,
      );
      assert.strictEqual(
        resolveRustCliBinary({ repoRoot: root, platform: "darwin", env: {} }),
        distBinPosix,
      );

      yield* fs.writeFileString(customBin, "custom");
      assert.strictEqual(
        resolveRustCliBinary({
          repoRoot: root,
          platform: "win32",
          env: { T3CODE_RUST_BIN: customBin },
        }),
        customBin,
      );
      // Missing T3CODE_RUST_BIN falls through to the staged repository binary.
      assert.strictEqual(
        resolveRustCliBinary({
          repoRoot: root,
          platform: "win32",
          env: { T3CODE_RUST_BIN: path.join(root, "missing-bin") },
        }),
        distBinWin,
      );
    }).pipe(Effect.scoped),
  );

  it.effect("skips Rust cutover when running as SEA, legacy-forced, or without a binary", () =>
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem;
      const path = yield* Path.Path;
      const root = yield* fs.makeTempDirectoryScoped({ prefix: "t3-rust-cutover-skip-" });
      const distBin = path.join(root, "rust", "dist", "bin", "t3");
      yield* fs.makeDirectory(path.dirname(distBin), { recursive: true });
      yield* fs.writeFileString(distBin, "rust-binary");

      let spawnCalls = 0;
      const spawnStub = (() => {
        spawnCalls++;
        return new FakeChildProcess() as unknown as NodeChildProcess.ChildProcess;
      }) as unknown as typeof NodeChildProcess.spawn;

      assert.isUndefined(
        yield* Effect.promise(() =>
          tryRunRustCliCutover({
            repoRoot: root,
            platform: "linux",
            env: {},
            isSea: () => true,
            spawn: spawnStub,
          }),
        ),
      );
      assert.isUndefined(
        yield* Effect.promise(() =>
          tryRunRustCliCutover({
            repoRoot: root,
            platform: "linux",
            env: { T3CODE_LEGACY_NODE: "1" },
            isSea: () => false,
            spawn: spawnStub,
          }),
        ),
      );
      assert.isUndefined(
        yield* Effect.promise(() =>
          tryRunRustCliCutover({
            repoRoot: path.join(root, "empty"),
            platform: "linux",
            env: {},
            isSea: () => false,
            spawn: spawnStub,
          }),
        ),
      );
      assert.strictEqual(spawnCalls, 0);
    }).pipe(Effect.scoped),
  );

  it.effect("forwards args, stdio, and signals and returns the child exit status", () =>
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem;
      const path = yield* Path.Path;
      const root = yield* fs.makeTempDirectoryScoped({ prefix: "t3-rust-cutover-run-" });
      const distBin = path.join(root, "rust", "dist", "bin", "t3");
      yield* fs.makeDirectory(path.dirname(distBin), { recursive: true });
      yield* fs.writeFileString(distBin, "rust-binary");

      const processEvents = new NodeEvents.EventEmitter();
      const recorded: Array<{
        command: string;
        args: ReadonlyArray<string>;
        stdio: unknown;
      }> = [];
      let activeChild: FakeChildProcess | undefined;

      const spawnStub = ((
        command: string,
        args: ReadonlyArray<string>,
        options: NodeChildProcess.SpawnOptions,
      ) => {
        recorded.push({ command, args, stdio: options.stdio });
        const child = new FakeChildProcess();
        activeChild = child;
        queueMicrotask(() => {
          processEvents.emit("SIGINT");
          processEvents.emit("SIGTERM");
          child.emit("exit", 42, null);
        });
        return child as unknown as NodeChildProcess.ChildProcess;
      }) as unknown as typeof NodeChildProcess.spawn;

      const exitCode = yield* Effect.promise(() =>
        tryRunRustCliCutover({
          repoRoot: root,
          platform: "linux",
          env: { T3CODE_HOME: root },
          argv: ["node", "bin.ts", "status", "--json"],
          isSea: () => false,
          spawn: spawnStub,
          processEvents: processEvents as unknown as Pick<NodeJS.Process, "on" | "off">,
        }),
      );

      assert.strictEqual(exitCode, 42);
      assert.deepStrictEqual(recorded, [
        {
          command: distBin,
          args: ["status", "--json"],
          stdio: "inherit",
        },
      ]);
      assert.deepStrictEqual(activeChild?.killedSignals, ["SIGINT", "SIGTERM"]);
      assert.strictEqual(processEvents.listenerCount("SIGINT"), 0);
      assert.strictEqual(processEvents.listenerCount("SIGTERM"), 0);

      // Signal exits translate to 128 + signal number, and spawn errors fall back to undefined.
      const signalChild = ((_cmd: string) => {
        const child = new FakeChildProcess();
        queueMicrotask(() => child.emit("exit", null, "SIGTERM"));
        return child as unknown as NodeChildProcess.ChildProcess;
      }) as unknown as typeof NodeChildProcess.spawn;

      const signalExit = yield* Effect.promise(() =>
        tryRunRustCliCutover({
          repoRoot: root,
          platform: "linux",
          env: {},
          isSea: () => false,
          spawn: signalChild,
          processEvents: processEvents as unknown as Pick<NodeJS.Process, "on" | "off">,
        }),
      );
      assert.strictEqual(signalExit, 128 + NodeOS.constants.signals.SIGTERM);

      const errorChild = ((_cmd: string) => {
        const child = new FakeChildProcess();
        queueMicrotask(() => child.emit("error", new Error("ENOEXEC")));
        return child as unknown as NodeChildProcess.ChildProcess;
      }) as unknown as typeof NodeChildProcess.spawn;

      const fallbackExit = yield* Effect.promise(() =>
        tryRunRustCliCutover({
          repoRoot: root,
          platform: "linux",
          env: {},
          isSea: () => false,
          spawn: errorChild,
          processEvents: processEvents as unknown as Pick<NodeJS.Process, "on" | "off">,
        }),
      );
      assert.isUndefined(fallbackExit);
      assert.strictEqual(processEvents.listenerCount("SIGINT"), 0);
      assert.strictEqual(processEvents.listenerCount("SIGTERM"), 0);
    }).pipe(Effect.scoped),
  );
});

it.live("executes the staged Rust binary when present in the repository", () =>
  Effect.gen(function* () {
    const repoRoot = NodePath.resolve(
      NodePath.dirname(NodeURL.fileURLToPath(import.meta.url)),
      "../../..",
    );
    const platform = yield* HostProcess.Platform;
    const resolved = resolveRustCliBinary({ repoRoot, platform, env: {} });
    if (resolved === undefined) {
      return;
    }

    const exitCode = yield* Effect.promise(() =>
      tryRunRustCliCutover({
        repoRoot,
        platform,
        env: {},
        argv: ["node", "bin.ts", "--version"],
        isSea: () => false,
      }),
    );
    assert.strictEqual(exitCode, 0);
  }),
);

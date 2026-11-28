// @effect-diagnostics nodeBuiltinImport:off -- E2E smoke test verifies temp directory cleanup and isolated fixture files.
import * as NodeFS from "node:fs";
import * as NodeHttp from "node:http";
import * as NodePath from "node:path";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, it } from "@effect/vitest";
import { ServerConfig, ServerConfigStreamEvent } from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";

import { resolveRustCliBinary } from "./rustCutover.ts";
import {
  copyDisposableLegionCli,
  fetchEnvironmentInfo,
  initIsolatedGitRepo,
  resolveWorkflowBoardRepoRoot,
  startWorkflowBoardE2eHarness,
  WorkflowBoardRpcClient,
} from "./workflowBoardE2eHarness.ts";

const decodeServerConfig = Schema.decodeUnknownSync(Schema.toCodecJson(ServerConfig));
const decodeServerConfigEvent = Schema.decodeUnknownSync(
  Schema.toCodecJson(ServerConfigStreamEvent),
);

async function withMockWsClient(
  onConnection: (ws: NodeWS.WebSocket, wss: NodeWS.WebSocketServer) => void,
  run: (client: WorkflowBoardRpcClient) => Promise<void>,
): Promise<void> {
  const wss = new NodeWS.WebSocketServer({ host: "127.0.0.1", port: 0 });
  await new Promise<void>((resolve) => wss.once("listening", () => resolve()));
  const addr = wss.address();
  if (!addr || typeof addr === "string") {
    wss.close();
    throw new Error("Expected numeric WebSocketServer port");
  }
  wss.on("connection", (ws) => onConnection(ws, wss));
  const clientWs = new NodeWS.WebSocket(`ws://127.0.0.1:${addr.port}`);
  await new Promise<void>((resolve, reject) => {
    clientWs.once("open", () => resolve());
    clientWs.once("error", reject);
  });
  const client = new WorkflowBoardRpcClient(clientWs);
  try {
    await run(client);
  } finally {
    client.close();
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  }
}

it.live(
  "preserves raw capabilities.projectWorkflowBoard, buffers multi-value Chunks, reassembles frameParts, and rejects waiters on WS close",
  () =>
    Effect.promise(async () => {
      const httpServer = NodeHttp.createServer((req, res) => {
        if (req.url === "/.well-known/legioncode/environment") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(
            JSON.stringify({
              environmentId: "env-mock-1",
              label: "mock-env",
              platform: { os: "linux", arch: "x64" },
              serverVersion: "0.1.0-mock",
              orchestrationProtocolVersion: 2,
              capabilities: {
                repositoryIdentity: true,
                projectWorkflowBoard: true,
              },
            }),
          );
          return;
        }
        res.writeHead(404).end();
      });
      await new Promise<void>((resolve) => httpServer.listen(0, "127.0.0.1", () => resolve()));
      try {
        const addr = httpServer.address();
        if (!addr || typeof addr === "string") {
          throw new Error("Expected numeric HTTP server port");
        }
        const info = await fetchEnvironmentInfo(`http://127.0.0.1:${addr.port}`);
        assert.isTrue(info.projectWorkflowBoard);
      } finally {
        await new Promise<void>((resolve) => httpServer.close(() => resolve()));
      }

      await withMockWsClient(
        (ws) => {
          ws.on("message", (raw) => {
            const msg = JSON.parse(raw.toString("utf8")) as {
              _tag: string;
              id?: string;
              tag?: string;
            };
            if (msg._tag !== "Request") return;
            if (msg.tag === "stream.multi") {
              ws.send(
                JSON.stringify({
                  _tag: "Chunk",
                  requestId: msg.id,
                  values: [{ seq: 1 }, { seq: 2 }, { seq: 3 }],
                }),
              );
            } else if (msg.tag === "server.probe") {
              ws.send(
                JSON.stringify({
                  _tag: "Exit",
                  requestId: msg.id,
                  exit: { _tag: "Success", value: { serverTime: "2026-11-24T00:00:00Z" } },
                }),
              );
            } else if (msg.tag === "unary.split") {
              const full = JSON.stringify({
                _tag: "Exit",
                requestId: msg.id,
                exit: { _tag: "Success", value: { reassembled: true, size: 42 } },
              });
              const p1 = Math.floor(full.length / 3);
              const p2 = Math.floor((full.length * 2) / 3);
              ws.send(JSON.stringify({ _tag: "Pong" }));
              ws.send(`\u001e${full.slice(0, p1)}`);
              ws.send(`\u001e${full.slice(p1, p2)}`);
              ws.send(full.slice(p2));
            }
          });
        },
        async (client) => {
          const sub = client.subscribe("stream.multi", {});
          const first = await sub.nextChunk();
          assert.deepStrictEqual(first, { seq: 1 });
          const remaining = await sub.drainWithProbe();
          assert.deepStrictEqual(remaining, [{ seq: 2 }, { seq: 3 }]);

          const splitResult = await client.callUnary<{
            readonly reassembled: boolean;
            readonly size: number;
          }>("unary.split", {});
          assert.deepStrictEqual(splitResult, { reassembled: true, size: 42 });
        },
      );

      await withMockWsClient(
        (ws) => {
          ws.on("message", (raw) => {
            const msg = JSON.parse(raw.toString("utf8")) as { _tag: string };
            if (msg._tag === "Request") {
              ws.close(1011, "server crashed");
            }
          });
        },
        async (client) => {
          let errMessage = "";
          try {
            await client.callUnary("unary.abruptClose", {});
          } catch (err) {
            errMessage = err instanceof Error ? err.message : String(err);
          }
          assert.include(errMessage, "WebSocket closed");
          assert.include(errMessage, "1011");

          let secondErr = "";
          try {
            await client.callUnary("unary.afterClose", {});
          } catch (err) {
            secondErr = err instanceof Error ? err.message : String(err);
          }
          assert.include(secondErr, "WebSocket closed");
        },
      );
    }),
);

it.live(
  "boots isolated Rust server, verifies WS/HTTP/SQLite/worktrees/CLI-fixtures, restarts, and cleans up",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveWorkflowBoardRepoRoot();
      const platform = yield* HostProcess.Platform;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env: {} });
      assert.isDefined(binaryPath, "Expected built Rust legioncode binary");

      let capturedTempRoot = "";

      yield* Effect.promise(async () => {
        await using harness = await startWorkflowBoardE2eHarness({
          repoRoot,
          platform,
          binaryPath,
          seedGitWorkspace: true,
          requireBoardCapability: true,
        });
        capturedTempRoot = harness.tempRoot;

        assert.isTrue(harness.rpc instanceof WorkflowBoardRpcClient);
        assert.strictEqual(harness.environment.descriptor.orchestrationProtocolVersion, 2);
        assert.isTrue(
          harness.environment.projectWorkflowBoard,
          "Expected capabilities.projectWorkflowBoard === true",
        );

        const probedEnv = await harness.fetchEnvironment();
        assert.strictEqual(
          probedEnv.descriptor.environmentId,
          harness.environment.descriptor.environmentId,
        );

        const rawConfig = await harness.rpc.callUnary("server.getConfig", {});
        const serverConfig = decodeServerConfig(rawConfig);
        assert.isTrue(serverConfig.providers.some((p) => p.instanceId === "devReplay"));

        const sub = harness.rpc.subscribe("subscribeServerConfig", {});
        const firstEvent = decodeServerConfigEvent(await sub.nextChunk());
        assert.strictEqual(firstEvent.type, "snapshot");
        await sub.drainWithProbe();
        await sub.interrupt();

        assert.isDefined(harness.git);
        const worktreePath = harness.git!.createWorktree("feat/e2e-smoke");
        assert.isTrue(NodeFS.existsSync(NodePath.join(worktreePath, "README.md")));
        const commitSha = harness.git!.commitFile(
          worktreePath,
          "smoke.txt",
          "e2e smoke content\n",
          "smoke commit",
        );
        assert.isTrue(commitSha.length >= 7);

        const standaloneGitDir = NodePath.join(harness.tempRoot, "standalone-git");
        const standaloneGit = initIsolatedGitRepo(standaloneGitDir);
        assert.isTrue(NodeFS.existsSync(NodePath.join(standaloneGit.repoDir, ".git")));

        const cliDir = harness.stageDisposableCliProject("imported-cli");
        assert.isTrue(NodeFS.existsSync(NodePath.join(cliDir, "STATE.md")));
        assert.isTrue(NodeFS.existsSync(NodePath.join(cliDir, "workflow", "execution.yaml")));
        assert.isTrue(NodeFS.existsSync(NodePath.join(cliDir, "tasks", "TSK-0002.md")));
        assert.isFalse(NodeFS.existsSync(NodePath.join(cliDir, "map")));

        const fullCliDir = copyDisposableLegionCli(
          repoRoot,
          NodePath.join(harness.tempRoot, "cli-direct"),
        );
        assert.isTrue(NodeFS.existsSync(NodePath.join(fullCliDir, "tasks", "TSK-0008.md")));

        await harness.restart();

        const restartedEnv = await harness.fetchEnvironment();
        assert.strictEqual(restartedEnv.descriptor.orchestrationProtocolVersion, 2);

        const migrationHead = harness.readSqlMigrationHead();
        assert.isDefined(migrationHead);
        assert.isTrue((migrationHead ?? 0) >= 110);

        const db = harness.openReadOnlyDb();
        try {
          const row = db
            .prepare("SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table'")
            .get() as { readonly count: number };
          assert.isTrue(row.count > 0);
        } finally {
          db.close();
        }
      });

      assert.isFalse(NodeFS.existsSync(capturedTempRoot));
    }),
);

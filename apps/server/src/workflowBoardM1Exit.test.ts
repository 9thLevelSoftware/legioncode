// @effect-diagnostics nodeBuiltinImport:off -- Spawns staged Rust binary against isolated temp git repo/worktree for M1 exit verification.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, it } from "@effect/vitest";
import {
  AuthBrowserSessionResult,
  AuthWebSocketTicketResult,
  ExecutionEnvironmentDescriptor,
  OrchestrationV2RpcSchemas,
  ProjectWorkflowBoardSnapshot,
  ServerConfig,
  WorkflowBoardApproveSpecResult,
  WorkflowBoardArtifact,
  WorkflowBoardStartStageResult,
  WorkflowBoardUpdateSpecResult,
  WorkflowCardDetail,
  WS_METHODS,
} from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Redacted from "effect/Redacted";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";

interface RpcFrame {
  readonly _tag: string;
  readonly id?: string;
  readonly requestId?: string;
  readonly values?: ReadonlyArray<unknown>;
  readonly exit?: {
    readonly _tag: "Success" | "Failure";
    readonly value?: unknown;
    readonly cause?: unknown;
  };
}

const decodeDescriptor = Schema.decodeUnknownSync(
  Schema.toCodecJson(ExecutionEnvironmentDescriptor),
);
const decodeBrowserSession = Schema.decodeUnknownSync(Schema.toCodecJson(AuthBrowserSessionResult));
const decodeWsTicket = Schema.decodeUnknownSync(Schema.toCodecJson(AuthWebSocketTicketResult));
const decodeServerConfig = Schema.decodeUnknownSync(Schema.toCodecJson(ServerConfig));
const decodeBoardSnapshot = Schema.decodeUnknownSync(
  Schema.toCodecJson(ProjectWorkflowBoardSnapshot),
);
const decodeCardDetail = Schema.decodeUnknownSync(Schema.toCodecJson(WorkflowCardDetail));
const decodeUpdateSpecResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardUpdateSpecResult),
);
const decodeApproveSpecResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApproveSpecResult),
);
const decodeStartStageResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardStartStageResult),
);
const decodeArtifact = Schema.decodeUnknownSync(Schema.toCodecJson(WorkflowBoardArtifact));
const decodeThreadProjection = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.getThreadProjection.output),
);

function resolveRepoRoot(): string {
  return NodePath.resolve(NodePath.dirname(NodeURL.fileURLToPath(import.meta.url)), "../../..");
}

function runGit(args: ReadonlyArray<string>, cwd: string): string {
  return NodeChildProcess.execFileSync("git", [...args], {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function initIsolatedRepoWithWorktree(tempRoot: string): {
  readonly repoDir: string;
  readonly worktreeDir: string;
} {
  const repoDir = NodePath.join(tempRoot, "repo");
  const worktreeDir = NodePath.join(tempRoot, "repo-wt");
  NodeFS.mkdirSync(repoDir, { recursive: true });
  runGit(["init", "-b", "main"], repoDir);
  runGit(["config", "user.name", "M1 Exit Test"], repoDir);
  runGit(["config", "user.email", "m1-exit@legioncode.local"], repoDir);
  NodeFS.writeFileSync(NodePath.join(repoDir, "README.md"), "# Isolated Repo\n", "utf8");
  runGit(["add", "README.md"], repoDir);
  runGit(["commit", "-m", "init"], repoDir);
  runGit(["worktree", "add", "-b", "feat/m1-plan", worktreeDir, "main"], repoDir);
  return { repoDir, worktreeDir };
}

interface StartedServer {
  readonly origin: string;
  readonly pairingToken: string;
  readonly child: NodeChildProcess.ChildProcessWithoutNullStreams;
}

function startRustServer(binaryPath: string, homeDir: string, cwd: string): Promise<StartedServer> {
  return new Promise((resolve, reject) => {
    const cleanEnv: NodeJS.ProcessEnv = { ...process.env };
    delete cleanEnv.LEGIONCODE_DEV_AUTH_TOKEN;
    delete cleanEnv.LEGIONCODE_LEGACY_NODE;
    delete cleanEnv.LEGIONCODE_HOME;
    delete cleanEnv.LEGIONCODE_STATE_DIR;
    delete cleanEnv.VITE_DEV_SERVER_URL;

    const child = NodeChildProcess.spawn(
      binaryPath,
      ["start", "--host", "127.0.0.1", "--port", "0", "--home-dir", homeDir, "--no-browser", cwd],
      { cwd, env: cleanEnv, stdio: ["pipe", "pipe", "pipe"] },
    );

    let origin: string | undefined;
    let pairingToken: string | undefined;
    let stdoutBuf = "";
    let stderrBuf = "";
    let settled = false;

    const fail = (err: Error) => {
      if (settled) return;
      settled = true;
      child.kill();
      reject(err);
    };

    child.once("error", fail);
    child.once("exit", (code, signal) => {
      if (!settled) {
        fail(
          new Error(
            `Rust server exited early (code=${code}, signal=${signal}). stderr:\n${stderrBuf}`,
          ),
        );
      }
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      stderrBuf += chunk;
    });
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdoutBuf += chunk;
      const lines = stdoutBuf.split(/\r?\n/);
      stdoutBuf = lines.pop() ?? "";
      for (const line of lines) {
        const listenMatch = /LegionCode server listening on (https?:\/\/\S+)/.exec(line);
        if (listenMatch?.[1]) {
          origin = listenMatch[1].trim();
        }
        const pairMatch = /pairingUrl:\s*(https?:\/\/\S+)/.exec(line);
        if (pairMatch?.[1]) {
          const parsedUrl = new URL(pairMatch[1].trim());
          const tokenParam = new URLSearchParams(parsedUrl.hash.replace(/^#/, "")).get("token");
          if (tokenParam) {
            pairingToken = tokenParam;
          }
        }
        if (origin && pairingToken && !settled) {
          settled = true;
          resolve({ origin, pairingToken, child });
        }
      }
    });
  });
}

function stopRustServer(child: NodeChildProcess.ChildProcessWithoutNullStreams): Promise<void> {
  return new Promise((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) {
      resolve();
      return;
    }
    child.once("exit", () => resolve());
    child.kill();
  });
}

class RpcClient {
  private readonly ws: NodeWS.WebSocket;
  private readonly waiters: Array<(frame: RpcFrame) => boolean> = [];
  private readonly buffer: Array<RpcFrame> = [];

  constructor(ws: NodeWS.WebSocket) {
    this.ws = ws;
    ws.on("message", (raw) => {
      const parsed = JSON.parse(raw.toString("utf8")) as RpcFrame | ReadonlyArray<RpcFrame>;
      const frames = Array.isArray(parsed) ? parsed : [parsed];
      for (const frame of frames) {
        const idx = this.waiters.findIndex((w) => w(frame));
        if (idx >= 0) {
          this.waiters.splice(idx, 1);
        } else {
          this.buffer.push(frame);
        }
      }
    });
  }

  sendRequest(id: string, tag: string, payload: unknown): void {
    this.ws.send(JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] }));
  }

  sendAck(requestId: string): void {
    this.ws.send(JSON.stringify({ _tag: "Ack", requestId }));
  }

  sendInterrupt(requestId: string): void {
    this.ws.send(JSON.stringify({ _tag: "Interrupt", requestId }));
  }

  nextFrame(predicate: (frame: RpcFrame) => boolean): Promise<RpcFrame> {
    const idx = this.buffer.findIndex(predicate);
    if (idx >= 0) {
      return Promise.resolve(this.buffer.splice(idx, 1)[0]!);
    }
    return new Promise((resolve) => {
      this.waiters.push((frame) => {
        if (!predicate(frame)) return false;
        resolve(frame);
        return true;
      });
    });
  }

  async callUnary(id: string, tag: string, payload: unknown): Promise<unknown> {
    this.sendRequest(id, tag, payload);
    const frame = await this.nextFrame((f) => f._tag === "Exit" && f.requestId === id);
    if (frame.exit?._tag !== "Success") {
      throw new Error(`RPC ${tag} failed: ${JSON.stringify(frame.exit)}`);
    }
    return frame.exit.value;
  }

  async nextChunk(requestId: string): Promise<unknown> {
    const frame = await this.nextFrame(
      (f) => f._tag === "Chunk" && f.requestId === requestId && Array.isArray(f.values),
    );
    this.sendAck(requestId);
    return frame.values?.[0];
  }

  close(): void {
    this.ws.close();
  }
}

async function pairClient(origin: string, pairingToken: string): Promise<RpcClient> {
  const pairRes = await fetch(`${origin}/api/auth/browser-session`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ credential: pairingToken }),
  });
  assert.strictEqual(pairRes.status, 200);
  const session = decodeBrowserSession(await pairRes.json());
  assert.isTrue(session.authenticated);
  const cookiePair = pairRes.headers.get("set-cookie")!.split(";")[0]!;

  const ticketRes = await fetch(`${origin}/api/auth/websocket-ticket/rpc`, {
    method: "POST",
    headers: { cookie: cookiePair },
  });
  assert.strictEqual(ticketRes.status, 200);
  const wsTicket = decodeWsTicket(await ticketRes.json());
  const wsOrigin = origin.replace(/^http/, "ws");
  const ws = new NodeWS.WebSocket(
    `${wsOrigin}/ws?orchestrationProtocol=2&wsTicket=${encodeURIComponent(Redacted.value(wsTicket.ticket))}`,
  );
  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
  return new RpcClient(ws);
}

it.live(
  "passes full M1 exit flow across host restart, second paired client, and idempotent Start Plan",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const platform = yield* HostProcess.Platform;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env: {} });
      assert.isDefined(binaryPath, "Expected staged rust/dist/bin/legioncode binary");

      const tempRoot = yield* Effect.acquireRelease(
        Effect.sync(() => NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-m1-exit-"))),
        (dir) =>
          Effect.sync(() =>
            NodeFS.rmSync(dir, {
              recursive: true,
              force: true,
              maxRetries: 20,
              retryDelay: 25,
            }),
          ),
      );
      const homeDir = NodePath.join(tempRoot, "home");
      NodeFS.mkdirSync(homeDir, { recursive: true });
      const { repoDir, worktreeDir } = initIsolatedRepoWithWorktree(tempRoot);

      const projectId = "proj-m1-exit";
      let cardId = "";
      let approvedSpecDigest = "";

      // Phase 1: Boot Host 1, pair Client 1, create card, draft & approve spec.
      yield* Effect.gen(function* () {
        const host1 = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopRustServer(child)),
        );

        yield* Effect.promise(async () => {
          const indexRes = await fetch(`${host1.origin}/`);
          assert.strictEqual(indexRes.status, 200);
          const indexHtml = await indexRes.text();
          assert.isTrue(indexHtml.includes('<div id="root">'));

          const envRes = await fetch(`${host1.origin}/.well-known/legioncode/environment`);
          assert.strictEqual(envRes.status, 200);
          const descriptor = decodeDescriptor(await envRes.json());
          assert.strictEqual(descriptor.capabilities.projectWorkflowBoard, true);

          const client1 = await pairClient(host1.origin, host1.pairingToken);
          try {
            const config = decodeServerConfig(
              await client1.callUnary("cfg", "server.getConfig", {}),
            );
            const devReplay = config.providers.find((p) => p.instanceId === "devReplay");
            assert.isDefined(devReplay);
            assert.isTrue(devReplay!.models.some((m) => m.slug === "dev-replay"));

            await client1.callUnary("proj", "orchestration.dispatchCommand", {
              type: "project.create",
              commandId: "cmd-proj-m1",
              projectId,
              title: "M1 Exit Repo",
              workspaceRoot: repoDir,
            });
            await client1.callUnary("settings", "server.updateSettings", {
              patch: { workflowsEnabled: true },
            });

            client1.sendRequest("sub-board", WS_METHODS.workflowBoardSubscribe, { projectId });
            const initialSnap = decodeBoardSnapshot(await client1.nextChunk("sub-board"));
            assert.strictEqual(initialSnap.cards.length, 0);

            const created = decodeCardDetail(
              await client1.callUnary("create", WS_METHODS.workflowBoardCreateCard, {
                projectId,
                title: "M1 One Handoff",
                brief: "Initial brief",
                sourceType: "idea",
                assistanceMode: "balanced",
                workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
                modelSelection: { instanceId: "devReplay", model: "dev-replay" },
                runtimeMode: "full-access",
                interactionMode: "default",
              }),
            );
            cardId = created.cardId;
            assert.strictEqual(created.revision, 1);

            const updated = decodeUpdateSpecResult(
              await client1.callUnary("update", WS_METHODS.workflowBoardUpdateSpec, {
                cardId,
                expectedRevision: 1,
                title: "M1 One Handoff Refined",
                brief: "Produce Plan brief and connect Replay thread in worktree.",
                acceptanceCriteria: [{ id: "AC-1", text: "Single Replay thread in worktree" }],
                constraints: ["No auto-commits"],
                sourceRefs: ["README.md"],
              }),
            );
            assert.strictEqual(updated.revision, 2);
            assert.isNotNull(updated.specDigest);
            approvedSpecDigest = updated.specDigest!;

            const approved = decodeApproveSpecResult(
              await client1.callUnary("approve", WS_METHODS.workflowBoardApproveSpec, {
                cardId,
                expectedRevision: 2,
              }),
            );
            assert.strictEqual(approved.revision, 3);
            assert.strictEqual(approved.specApproval.status, "valid");
            assert.strictEqual(approved.specApproval.specDigest, approvedSpecDigest);
            assert.strictEqual(approved.card.stage, "plan");

            client1.sendInterrupt("sub-board");
          } finally {
            client1.close();
          }
        });
      }).pipe(Effect.scoped);

      // Phase 2: Boot Host 2 against same homeDir, pair Client 2, verify persisted revision & Start Plan twice.
      yield* Effect.gen(function* () {
        const host2 = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopRustServer(child)),
        );

        yield* Effect.promise(async () => {
          const client2 = await pairClient(host2.origin, host2.pairingToken);
          try {
            const persisted = decodeCardDetail(
              await client2.callUnary("get-card", WS_METHODS.workflowBoardGetCard, {
                cardId,
                projectId,
              }),
            );
            assert.strictEqual(persisted.revision, 3);
            assert.strictEqual(persisted.stage, "plan");
            assert.strictEqual(persisted.specApproval?.status, "valid");
            assert.strictEqual(persisted.specApproval?.specDigest, approvedSpecDigest);

            const started1 = decodeStartStageResult(
              await client2.callUnary("start-1", WS_METHODS.workflowBoardStartStage, {
                cardId,
                expectedRevision: 3,
                stage: "plan",
                modelSelection: { instanceId: "devReplay", model: "dev-replay" },
                runtimeMode: "full-access",
                interactionMode: "default",
                workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
              }),
            );
            assert.strictEqual(started1.resumed, false);
            assert.strictEqual(started1.revision, 4);
            assert.isNotNull(started1.runId);

            const brief = decodeArtifact(
              await client2.callUnary("brief", WS_METHODS.workflowBoardGetArtifact, {
                cardId,
                key: "plan-brief.md",
              }),
            );
            assert.isTrue(brief.content.includes(approvedSpecDigest));
            assert.isTrue(brief.content.includes("M1 One Handoff Refined"));

            const threadProj = decodeThreadProjection(
              await client2.callUnary("proj-thread", "orchestration.getThreadProjection", {
                threadId: started1.threadId,
              }),
            );
            assert.strictEqual(threadProj.thread.projectId, projectId);
            assert.strictEqual(
              NodePath.resolve(threadProj.thread.worktreePath ?? ""),
              NodePath.resolve(worktreeDir),
            );

            // Second Start Plan yields the exact same thread (resumed: true, stageRuns.length === 1).
            const started2 = decodeStartStageResult(
              await client2.callUnary("start-2", WS_METHODS.workflowBoardStartStage, {
                cardId,
                expectedRevision: 4,
                stage: "plan",
                modelSelection: { instanceId: "devReplay", model: "dev-replay" },
                runtimeMode: "full-access",
                interactionMode: "default",
                workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
              }),
            );
            assert.strictEqual(started2.resumed, true);
            assert.strictEqual(started2.threadId, started1.threadId);
            assert.strictEqual(started2.stageRunId, started1.stageRunId);

            const afterDouble = decodeCardDetail(
              await client2.callUnary("get-card-2", WS_METHODS.workflowBoardGetCard, {
                cardId,
                projectId,
              }),
            );
            assert.strictEqual(afterDouble.stageRuns.length, 1);
            assert.strictEqual(afterDouble.activeThreadId, started1.threadId);
          } finally {
            client2.close();
          }
        });
      }).pipe(Effect.scoped);

      assert.strictEqual(runGit(["rev-list", "--count", "HEAD"], repoDir), "1");
      assert.isTrue(
        NodeFS.existsSync(
          NodePath.join(worktreeDir, ".legion", "workflows", cardId, "plan-brief.md"),
        ),
      );
    }).pipe(Effect.scoped),
);

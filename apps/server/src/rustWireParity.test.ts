// @effect-diagnostics nodeBuiltinImport:off -- Spawns the staged Rust binary directly to validate live wire schemas.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeSqlite from "node:sqlite";
import * as NodeURL from "node:url";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, it } from "@effect/vitest";
import {
  AuthBrowserSessionResult,
  AuthSessionState,
  AuthWebSocketTicketResult,
  ExecutionEnvironmentDescriptor,
  OrchestrationV2RpcSchemas,
  OrchestrationV2ShellSnapshot,
  ServerConfig,
  ServerConfigStreamEvent,
  TerminalSessionSnapshot,
} from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Redacted from "effect/Redacted";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";
import { runFullRpcAndProviderParityChecks } from "./rustWireParitySuite.ts";

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
const decodeAuthSession = Schema.decodeUnknownSync(Schema.toCodecJson(AuthSessionState));
const decodeWsTicket = Schema.decodeUnknownSync(Schema.toCodecJson(AuthWebSocketTicketResult));
const decodeShellSnapshot = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2ShellSnapshot),
);
const decodeServerConfig = Schema.decodeUnknownSync(Schema.toCodecJson(ServerConfig));
const decodeServerConfigEvent = Schema.decodeUnknownSync(
  Schema.toCodecJson(ServerConfigStreamEvent),
);
const decodeShellStreamItem = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.subscribeShell.output),
);
const decodeThreadStreamItem = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.subscribeThread.output),
);
const decodeTerminalSnapshot = Schema.decodeUnknownSync(
  Schema.toCodecJson(TerminalSessionSnapshot),
);

function resolveRepoRoot(): string {
  return NodePath.resolve(NodePath.dirname(NodeURL.fileURLToPath(import.meta.url)), "../../..");
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

class RpcSocketHarness {
  private readonly ws: NodeWS.WebSocket;
  private readonly pendingWaiters: Array<(frames: ReadonlyArray<RpcFrame>) => boolean> = [];
  private readonly bufferedFrames: Array<RpcFrame> = [];

  constructor(ws: NodeWS.WebSocket) {
    this.ws = ws;
    ws.on("message", (data) => {
      const parsed = JSON.parse(data.toString("utf8")) as RpcFrame | ReadonlyArray<RpcFrame>;
      const frames = Array.isArray(parsed) ? parsed : [parsed];
      for (const frame of frames) {
        const idx = this.pendingWaiters.findIndex((waiter) => waiter([frame]));
        if (idx >= 0) {
          this.pendingWaiters.splice(idx, 1);
        } else {
          this.bufferedFrames.push(frame);
        }
      }
    });
  }

  sendRequest(id: string, tag: string, payload: unknown): void {
    this.ws.send(
      JSON.stringify({
        _tag: "Request",
        id,
        tag,
        payload,
        headers: [],
      }),
    );
  }

  sendAck(requestId: string): void {
    this.ws.send(JSON.stringify({ _tag: "Ack", requestId }));
  }

  sendInterrupt(requestId: string): void {
    this.ws.send(JSON.stringify({ _tag: "Interrupt", requestId }));
  }

  nextFrameMatching(predicate: (frame: RpcFrame) => boolean): Promise<RpcFrame> {
    const existingIdx = this.bufferedFrames.findIndex(predicate);
    if (existingIdx >= 0) {
      return Promise.resolve(this.bufferedFrames.splice(existingIdx, 1)[0]!);
    }
    return new Promise((resolve) => {
      this.pendingWaiters.push((frames) => {
        const match = frames.find(predicate);
        if (!match) return false;
        resolve(match);
        return true;
      });
    });
  }

  async callUnary(id: string, tag: string, payload: unknown): Promise<unknown> {
    this.sendRequest(id, tag, payload);
    const frame = await this.nextFrameMatching((f) => f._tag === "Exit" && f.requestId === id);
    if (frame.exit?._tag !== "Success") {
      throw new Error(`RPC ${tag} failed: ${JSON.stringify(frame.exit)}`);
    }
    return frame.exit.value;
  }

  async nextChunk(requestId: string, predicate?: (chunk: unknown) => boolean): Promise<unknown> {
    while (true) {
      const frame = await this.nextFrameMatching(
        (f) => f._tag === "Chunk" && f.requestId === requestId && Array.isArray(f.values),
      );
      this.sendAck(requestId);
      for (const value of frame.values ?? []) {
        if (!predicate || predicate(value)) {
          return value;
        }
      }
    }
  }

  close(): void {
    this.ws.close();
  }
}

function connectWs(url: string): Promise<RpcSocketHarness> {
  return new Promise((resolve, reject) => {
    const ws = new NodeWS.WebSocket(url);
    ws.once("open", () => resolve(new RpcSocketHarness(ws)));
    ws.once("error", reject);
  });
}

async function runWireParityChecks(
  origin: string,
  pairingToken: string,
  workspaceDir: string,
  tempRoot: string,
  isWindows: boolean,
) {
  // 1. ExecutionEnvironmentDescriptor
  const envRes = await fetch(`${origin}/.well-known/legioncode/environment`);
  assert.strictEqual(envRes.status, 200);
  const descriptor = decodeDescriptor(await envRes.json());
  assert.strictEqual(descriptor.orchestrationProtocolVersion, 2);
  assert.strictEqual(descriptor.capabilities.projectWorkflowBoard, true);

  // 2. Pair browser session cookie & verify AuthSessionState
  const pairRes = await fetch(`${origin}/api/auth/browser-session`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ credential: pairingToken }),
  });
  assert.strictEqual(pairRes.status, 200);
  const browserSession = decodeBrowserSession(await pairRes.json());
  assert.isTrue(browserSession.authenticated);

  const setCookie = pairRes.headers.get("set-cookie");
  assert.isDefined(setCookie);
  const cookiePair = setCookie!.split(";")[0]!;

  const sessionRes = await fetch(`${origin}/api/auth/session`, {
    headers: { cookie: cookiePair },
  });
  assert.strictEqual(sessionRes.status, 200);
  const authState = decodeAuthSession(await sessionRes.json());
  assert.isTrue(authState.authenticated);

  // 3. HTTP Shell Snapshot
  const httpShellRes = await fetch(`${origin}/api/orchestration/shell`, {
    headers: { cookie: cookiePair },
  });
  assert.strictEqual(httpShellRes.status, 200);
  const httpShell = decodeShellSnapshot(await httpShellRes.json());
  assert.strictEqual(httpShell.schemaVersion, 2);

  // 4. Single-use WebSocket RPC ticket + upgrade
  const ticketRes = await fetch(`${origin}/api/auth/websocket-ticket/rpc`, {
    method: "POST",
    headers: { cookie: cookiePair },
  });
  assert.strictEqual(ticketRes.status, 200);
  const wsTicket = decodeWsTicket(await ticketRes.json());

  const wsOrigin = origin.replace(/^http/, "ws");
  const rpc = await connectWs(
    `${wsOrigin}/ws?orchestrationProtocol=2&wsTicket=${encodeURIComponent(Redacted.value(wsTicket.ticket))}`,
  );

  try {
    // 5. Unary & streaming ServerConfig
    const configVal = await rpc.callUnary("cfg-1", "server.getConfig", {});
    const serverConfig = decodeServerConfig(configVal);
    assert.isTrue(serverConfig.providers.some((p) => p.instanceId === "devReplay"));

    rpc.sendRequest("sub-cfg", "subscribeServerConfig", {});
    const cfgChunk = decodeServerConfigEvent(await rpc.nextChunk("sub-cfg"));
    assert.strictEqual(cfgChunk.type, "snapshot");
    if (cfgChunk.type === "snapshot") {
      assert.isTrue(cfgChunk.config.providers.some((p) => p.instanceId === "devReplay"));
    }

    // 6. Shell stream snapshot -> project.create -> launchThread (Replay dev)
    rpc.sendRequest("sub-shell", "orchestration.subscribeShell", {});
    const initialSnap = decodeShellStreamItem(await rpc.nextChunk("sub-shell"));
    assert.strictEqual(initialSnap.kind, "snapshot");
    const initialSync = decodeShellStreamItem(await rpc.nextChunk("sub-shell"));
    assert.strictEqual(initialSync.kind, "synchronized");

    const projectId = "proj-wire-1";
    await rpc.callUnary("cmd-proj", "orchestration.dispatchCommand", {
      type: "project.create",
      commandId: "cmd-proj-1",
      projectId,
      title: "Wire Parity Project",
      workspaceRoot: workspaceDir,
    });
    const projEvent = decodeShellStreamItem(
      await rpc.nextChunk("sub-shell", (c) => {
        const decoded = decodeShellStreamItem(c);
        return decoded.kind === "project.updated" && decoded.project.id === projectId;
      }),
    );
    assert.strictEqual(projEvent.kind, "project.updated");

    const threadId = "thread-wire-1";
    await rpc.callUnary("cmd-launch", "orchestration.launchThread", {
      commandId: "cmd-launch-1",
      creationSource: "web",
      threadId,
      projectId,
      title: "Replay Wire Parity",
      modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      runtimeMode: "full-access",
      interactionMode: "default",
      workspaceStrategy: { type: "root", branch: null },
      initialMessage: {
        messageId: "msg-wire-1",
        text: "Hello Replay Parity",
        attachments: [],
      },
    });

    const threadEvent = decodeShellStreamItem(
      await rpc.nextChunk("sub-shell", (c) => {
        const decoded = decodeShellStreamItem(c);
        return decoded.kind === "thread.updated" && decoded.thread.id === threadId;
      }),
    );
    assert.strictEqual(threadEvent.kind, "thread.updated");

    // 7. Thread stream snapshot + synchronized + turn event
    rpc.sendRequest("sub-thread", "orchestration.subscribeThread", { threadId });
    const threadSnap = decodeThreadStreamItem(await rpc.nextChunk("sub-thread"));
    assert.strictEqual(threadSnap.kind, "snapshot");
    const threadSync = decodeThreadStreamItem(await rpc.nextChunk("sub-thread"));
    assert.strictEqual(threadSync.kind, "synchronized");

    // 8. Terminal RPC lifecycle
    const openedTerm = decodeTerminalSnapshot(
      await rpc.callUnary("term-open", "terminal.open", {
        threadId,
        terminalId: "term-wire-1",
        cwd: workspaceDir,
        cols: 80,
        rows: 24,
      }),
    );
    assert.strictEqual(openedTerm.terminalId, "term-wire-1");

    await rpc.callUnary("term-clear", "terminal.clear", {
      threadId,
      terminalId: "term-wire-1",
    });
    await rpc.callUnary("term-close", "terminal.close", {
      threadId,
      terminalId: "term-wire-1",
    });

    rpc.sendInterrupt("sub-cfg");
    rpc.sendInterrupt("sub-shell");
    rpc.sendInterrupt("sub-thread");

    await runFullRpcAndProviderParityChecks(
      rpc,
      workspaceDir,
      tempRoot,
      origin,
      "proj-wire-1",
      "thread-parity-sweep",
      isWindows,
    );
  } finally {
    rpc.close();
  }
}

it.live(
  "decodes live Rust HTTP, Auth, WS Config, Shell, Thread & Terminal payloads via @legioncode/contracts",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const platform = yield* HostProcess.Platform;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env: {} });
      assert.isDefined(binaryPath, "Expected built rust/dist/bin/legioncode binary");

      const tempRoot = yield* Effect.acquireRelease(
        Effect.sync(() =>
          NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-wire-parity-")),
        ),
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
      const workspaceDir = NodePath.join(tempRoot, "workspace");
      NodeFS.mkdirSync(homeDir, { recursive: true });
      NodeFS.mkdirSync(workspaceDir, { recursive: true });

      yield* Effect.gen(function* () {
        const started = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, workspaceDir)),
          ({ child }) =>
            Effect.promise(
              () =>
                new Promise<void>((resolve) => {
                  if (child.exitCode !== null || child.signalCode !== null) {
                    resolve();
                    return;
                  }
                  child.once("exit", () => resolve());
                  child.kill();
                }),
            ),
        );

        yield* Effect.promise(() =>
          runWireParityChecks(
            started.origin,
            started.pairingToken,
            workspaceDir,
            tempRoot,
            platform === "win32",
          ),
        );
      }).pipe(Effect.scoped);

      // Verify SQLite v111 migrations & tables persisted cleanly in statev2.sqlite.
      const dbPath = NodePath.join(homeDir, "userdata", "statev2.sqlite");
      assert.isTrue(NodeFS.existsSync(dbPath), `Expected statev2.sqlite at ${dbPath}`);
      const db = new NodeSqlite.DatabaseSync(dbPath, { readOnly: true });
      try {
        const headRow = db
          .prepare("SELECT MAX(migration_id) AS head FROM effect_sql_migrations")
          .get() as { readonly head?: number } | undefined;
        assert.strictEqual(headRow?.head, 114);

        const tables = new Set(
          (
            db
              .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
              .all() as unknown as ReadonlyArray<{ readonly name: string }>
          ).map((row) => row.name),
        );
        for (const requiredTable of [
          "agent_feedback",
          "issue_tracker_accounts",
          "workflow_runs",
          "workflow_steps",
          "workflow_waits",
          "workflow_mail",
          "workflow_marks",
          "workflow_signals",
          "workflow_triggers",
          "workflow_hooks",
          "workflow_store",
          "workflow_run_references",
          "workflow_engine_journal",
          "workflow_board_cards",
          "workflow_board_revisions",
          "workflow_board_approvals",
          "workflow_board_stage_runs",
          "workflow_board_write_journal",
          "workflow_board_exploration_rounds",
          "workflow_board_decisions",
          "workflow_board_challenges",
          "workflow_board_plan_designs",
          "workflow_board_tasks",
          "workflow_board_planned_checks",
          "workflow_board_ac_evidence",
          "workflow_board_contract_violations",
          "workflow_board_check_receipts",
          "workflow_board_review_runs",
          "workflow_board_review_findings",
          "workflow_board_amendments",
          "workflow_board_external_actions",
          "workflow_board_delivery_receipts",
          "workflow_board_cli_imports",
        ]) {
          assert.isTrue(tables.has(requiredTable), `Missing SQLite v114 table ${requiredTable}`);
        }
      } finally {
        db.close();
      }
    }).pipe(Effect.scoped),
);

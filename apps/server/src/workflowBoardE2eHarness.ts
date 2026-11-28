// @effect-diagnostics nodeBuiltinImport:off -- E2E harness spawns isolated Rust server, git worktrees, and SQLite inspections.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeSqlite from "node:sqlite";
import * as NodeURL from "node:url";

import * as NodeWS from "@effect/platform-node/NodeWS";
import {
  AuthBrowserSessionResult,
  AuthSessionState,
  AuthWebSocketTicketResult,
  ExecutionEnvironmentDescriptor,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Redacted from "effect/Redacted";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";

export interface WorkflowBoardRpcFrame {
  readonly _tag: string;
  readonly id?: string | undefined;
  readonly requestId?: string | undefined;
  readonly values?: ReadonlyArray<unknown> | undefined;
  readonly defect?: unknown;
  readonly exit?:
    | {
        readonly _tag: "Success" | "Failure";
        readonly value?: unknown;
        readonly cause?: unknown;
      }
    | undefined;
}

export interface WorkflowBoardRpcSubscription {
  readonly requestId: string;
  nextChunk(predicate?: (chunk: unknown) => boolean): Promise<unknown>;
  drainWithProbe(): Promise<ReadonlyArray<unknown>>;
  interrupt(): Promise<void>;
}

export interface WorkflowBoardEnvironmentInfo {
  readonly descriptor: ExecutionEnvironmentDescriptor;
  readonly projectWorkflowBoard: boolean;
  readonly databaseMigration: number | undefined;
}

export interface WorkflowBoardGitWorkspace {
  readonly repoDir: string;
  readonly worktreesDir: string;
  createWorktree(branchName: string, dirName?: string): string;
  commitFile(dir: string, relPath: string, content: string, message?: string): string;
}

export interface WorkflowBoardE2eHarnessOptions {
  readonly repoRoot?: string | undefined;
  readonly platform?: NodeJS.Platform | undefined;
  readonly binaryPath?: string | undefined;
  readonly env?: NodeJS.ProcessEnv | undefined;
  readonly seedGitWorkspace?: boolean | undefined;
  readonly workspaceTitle?: string | undefined;
  readonly requireBoardCapability?: boolean | undefined;
}

export interface WorkflowBoardE2eHarness extends AsyncDisposable {
  readonly origin: string;
  readonly homeDir: string;
  readonly dbPath: string;
  readonly workspaceDir: string;
  readonly tempRoot: string;
  readonly pairingToken: string;
  readonly cookiePair: string;
  readonly environment: WorkflowBoardEnvironmentInfo;
  readonly rpc: WorkflowBoardRpcClient;
  readonly git: WorkflowBoardGitWorkspace | undefined;
  restart(): Promise<void>;
  openReadOnlyDb(): NodeSqlite.DatabaseSync;
  readSqlMigrationHead(): number | undefined;
  stageDisposableCliProject(destSubdir?: string): string;
  fetchEnvironment(): Promise<WorkflowBoardEnvironmentInfo>;
  close(): Promise<void>;
  [Symbol.asyncDispose](): Promise<void>;
}

const decodeDescriptor = Schema.decodeUnknownSync(
  Schema.toCodecJson(ExecutionEnvironmentDescriptor),
);
const decodeBrowserSession = Schema.decodeUnknownSync(Schema.toCodecJson(AuthBrowserSessionResult));
const decodeAuthSession = Schema.decodeUnknownSync(Schema.toCodecJson(AuthSessionState));
const decodeWsTicket = Schema.decodeUnknownSync(Schema.toCodecJson(AuthWebSocketTicketResult));

const GIT_ENV: NodeJS.ProcessEnv = {
  ...process.env,
  GIT_TERMINAL_PROMPT: "0",
  GIT_ASKPASS: "",
};

function runGit(cwd: string, args: ReadonlyArray<string>): string {
  return NodeChildProcess.execFileSync("git", [...args], {
    cwd,
    env: GIT_ENV,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

interface PendingWaiter {
  readonly match: (frame: WorkflowBoardRpcFrame) => boolean;
  readonly reject: (err: Error) => void;
}

const FRAME_PART_PREFIX = "\u001e";

export class WorkflowBoardRpcClient {
  private readonly ws: NodeWS.WebSocket;
  private readonly pendingWaiters: Array<PendingWaiter> = [];
  private readonly bufferedFrames: Array<WorkflowBoardRpcFrame> = [];
  private readonly pendingChunkValues = new Map<string, Array<unknown>>();
  private pendingFrameParts = "";
  private terminalError: Error | null = null;
  private nextSeq = 0;

  constructor(ws: NodeWS.WebSocket) {
    this.ws = ws;
    ws.on("message", (data) => {
      const rawText = typeof data === "string" ? data : data.toString("utf8");
      let text = rawText;
      if (rawText.startsWith(FRAME_PART_PREFIX)) {
        this.pendingFrameParts += rawText.slice(FRAME_PART_PREFIX.length);
        return;
      }
      if (this.pendingFrameParts !== "") {
        text = this.pendingFrameParts + rawText;
        this.pendingFrameParts = "";
      }

      let parsed: WorkflowBoardRpcFrame | ReadonlyArray<WorkflowBoardRpcFrame>;
      try {
        parsed = JSON.parse(text) as WorkflowBoardRpcFrame | ReadonlyArray<WorkflowBoardRpcFrame>;
      } catch (err) {
        this.pendingFrameParts = "";
        this.rejectAllWaiters(
          err instanceof Error
            ? new Error(`WebSocket RPC JSON parse error: ${err.message}`)
            : new Error(`WebSocket RPC JSON parse error: ${String(err)}`),
        );
        return;
      }

      const frames = Array.isArray(parsed) ? parsed : [parsed];
      for (const frame of frames) {
        if (frame._tag === "Pong") {
          continue;
        }
        const idx = this.pendingWaiters.findIndex((waiter) => waiter.match(frame));
        if (idx >= 0) {
          this.pendingWaiters.splice(idx, 1);
        } else {
          this.bufferedFrames.push(frame);
        }
      }
    });

    ws.on("close", (code, reason) => {
      const reasonText = reason.toString("utf8");
      this.rejectAllWaiters(
        new Error(`WebSocket closed (code=${code}${reasonText ? `, reason=${reasonText}` : ""})`),
      );
    });

    ws.on("error", (err) => {
      this.rejectAllWaiters(err instanceof Error ? err : new Error(String(err)));
    });
  }

  private rejectAllWaiters(err: Error): void {
    if (this.terminalError === null) {
      this.terminalError = err;
    }
    const waiters = this.pendingWaiters.splice(0, this.pendingWaiters.length);
    for (const waiter of waiters) {
      waiter.reject(this.terminalError);
    }
  }

  private allocateRequestId(prefix = "req"): string {
    this.nextSeq += 1;
    return `${prefix}-${this.nextSeq}`;
  }

  sendRequest(id: string, tag: string, payload: unknown): void {
    if (this.terminalError !== null) {
      throw this.terminalError;
    }
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
    if (this.terminalError !== null) {
      return;
    }
    this.ws.send(JSON.stringify({ _tag: "Ack", requestId }));
  }

  sendInterrupt(requestId: string): void {
    if (this.terminalError !== null) {
      return;
    }
    this.ws.send(JSON.stringify({ _tag: "Interrupt", requestId }));
  }

  nextFrameMatching(
    predicate: (frame: WorkflowBoardRpcFrame) => boolean,
  ): Promise<WorkflowBoardRpcFrame> {
    const existingIdx = this.bufferedFrames.findIndex(predicate);
    if (existingIdx >= 0) {
      return Promise.resolve(this.bufferedFrames.splice(existingIdx, 1)[0]!);
    }
    if (this.terminalError !== null) {
      return Promise.reject(this.terminalError);
    }
    return new Promise((resolve, reject) => {
      this.pendingWaiters.push({
        match: (frame) => {
          if (!predicate(frame)) return false;
          resolve(frame);
          return true;
        },
        reject,
      });
    });
  }

  async callUnary<T = unknown>(tag: string, payload: unknown): Promise<T> {
    if (this.terminalError !== null) {
      throw this.terminalError;
    }
    const id = this.allocateRequestId("unary");
    this.sendRequest(id, tag, payload);
    const frame = await this.nextFrameMatching(
      (f) => f._tag === "Defect" || (f._tag === "Exit" && f.requestId === id),
    );
    if (frame._tag === "Defect") {
      throw new Error(`RPC "${tag}" triggered connection Defect: ${JSON.stringify(frame.defect)}`);
    }
    if (frame.exit?._tag !== "Success") {
      throw new Error(`RPC "${tag}" failed: ${JSON.stringify(frame.exit)}`);
    }
    return frame.exit.value as T;
  }

  subscribe(tag: string, payload: unknown): WorkflowBoardRpcSubscription {
    const requestId = this.allocateRequestId("sub");
    let exited = false;
    this.sendRequest(requestId, tag, payload);

    const enqueueChunkValues = (values: ReadonlyArray<unknown>): void => {
      if (values.length === 0) return;
      let queue = this.pendingChunkValues.get(requestId);
      if (!queue) {
        queue = [];
        this.pendingChunkValues.set(requestId, queue);
      }
      queue.push(...values);
    };

    const nextChunk = async (predicate?: (chunk: unknown) => boolean): Promise<unknown> => {
      while (true) {
        const queued = this.pendingChunkValues.get(requestId);
        if (queued) {
          while (queued.length > 0) {
            const value = queued.shift()!;
            if (!predicate || predicate(value)) {
              return value;
            }
          }
        }
        if (exited) {
          throw new Error(`Stream "${tag}" already exited before next chunk`);
        }

        const frame = await this.nextFrameMatching(
          (f) => f._tag === "Defect" || f.requestId === requestId,
        );
        if (frame._tag === "Defect") {
          exited = true;
          throw new Error(`Stream "${tag}" defect: ${JSON.stringify(frame.defect)}`);
        }
        if (frame._tag === "Exit") {
          exited = true;
          throw new Error(
            `Stream "${tag}" exited before matching chunk: ${JSON.stringify(frame.exit)}`,
          );
        }
        if (frame._tag === "Chunk" && Array.isArray(frame.values)) {
          this.sendAck(requestId);
          enqueueChunkValues(frame.values);
        }
      }
    };

    const drainWithProbe = async (): Promise<ReadonlyArray<unknown>> => {
      const collected: Array<unknown> = [];
      const queued = this.pendingChunkValues.get(requestId);
      if (queued && queued.length > 0) {
        collected.push(...queued.splice(0, queued.length));
      }
      if (exited) return collected;

      const probeId = this.allocateRequestId("probe");
      this.sendRequest(probeId, "server.probe", {});
      while (true) {
        const frame = await this.nextFrameMatching(
          (f) =>
            f._tag === "Defect" ||
            f.requestId === requestId ||
            (f._tag === "Exit" && f.requestId === probeId),
        );
        if (frame._tag === "Defect") {
          exited = true;
          throw new Error(`Stream "${tag}" defect during probe: ${JSON.stringify(frame.defect)}`);
        }
        if (frame.requestId === probeId) {
          return collected;
        }
        if (frame._tag === "Chunk" && Array.isArray(frame.values)) {
          this.sendAck(requestId);
          collected.push(...frame.values);
        } else if (frame._tag === "Exit") {
          exited = true;
          return collected;
        }
      }
    };

    const interrupt = async (): Promise<void> => {
      this.pendingChunkValues.delete(requestId);
      if (exited) return;
      this.sendInterrupt(requestId);
      while (!exited) {
        const frame = await this.nextFrameMatching(
          (f) => f._tag === "Defect" || f.requestId === requestId,
        );
        if (frame._tag === "Defect" || frame._tag === "Exit") {
          exited = true;
        } else if (frame._tag === "Chunk") {
          this.sendAck(requestId);
        }
      }
    };

    return { requestId, nextChunk, drainWithProbe, interrupt };
  }

  close(): void {
    this.ws.close();
  }
}

export async function awaitWorkflowBoardStageSettlement(
  rpc: WorkflowBoardRpcClient,
  cardId: string,
  stageRunId: string,
): Promise<WorkflowCardDetail> {
  const initial = await rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", { cardId });
  const sub = rpc.subscribe("workflowBoard.subscribe", { projectId: initial.projectId });
  try {
    await sub.nextChunk();
    const readAfterUpdate = async (): Promise<WorkflowCardDetail> => {
      const card = await rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", { cardId });
      const status = card.stageRuns.find((run) => run.id === stageRunId)?.status;
      if (status === "completed" || status === "failed" || status === "cancelled") {
        return card;
      }
      await sub.nextChunk();
      return await readAfterUpdate();
    };
    return await readAfterUpdate();
  } finally {
    await sub.interrupt();
  }
}

export async function installWorkflowBoardTestProvider(
  harness: WorkflowBoardE2eHarness,
  instanceId: string,
  output: string,
  beforeResult = "",
) {
  const scriptPath = NodePath.join(harness.tempRoot, `${instanceId}.mjs`);
  const windows = harness.environment.descriptor.platform.os === "windows";
  const binaryPath = NodePath.join(harness.tempRoot, windows ? `${instanceId}.cmd` : instanceId);
  NodeFS.writeFileSync(
    scriptPath,
    [
      "process.stdin.resume();",
      beforeResult,
      `console.log(JSON.stringify({ type: "system", subtype: "init", session_id: ${JSON.stringify(instanceId)} }));`,
      `console.log(JSON.stringify({ type: "assistant", message: { id: "response", role: "assistant", content: [{ type: "text", text: ${JSON.stringify(output)} }], usage: { input_tokens: 10, output_tokens: 8 } } }));`,
      'console.log(JSON.stringify({ type: "result", subtype: "success", is_error: false, usage: { input_tokens: 10, output_tokens: 8 } }));',
      "process.exit(0);",
    ].join("\n"),
    "utf8",
  );
  if (windows) {
    NodeFS.writeFileSync(binaryPath, `@echo off\r\nnode "%~dp0${instanceId}.mjs" %*\r\n`, "utf8");
  } else {
    NodeFS.writeFileSync(
      binaryPath,
      `#!/bin/sh\nexec node "$(dirname "$0")/${instanceId}.mjs" "$@"\n`,
      {
        encoding: "utf8",
        mode: 0o755,
      },
    );
  }
  await harness.rpc.callUnary("server.updateSettings", {
    patch: {
      providerInstances: {
        [instanceId]: { driver: "claudeAgent", enabled: true, config: { binaryPath } },
      },
    },
  });
  return { instanceId, model: "claude-sonnet-4-5" };
}

export function resolveWorkflowBoardRepoRoot(): string {
  return NodePath.resolve(NodePath.dirname(NodeURL.fileURLToPath(import.meta.url)), "../../..");
}

export function initIsolatedGitRepo(
  dir: string,
  title = "# Workflow Board E2E Workspace\n",
): WorkflowBoardGitWorkspace {
  NodeFS.mkdirSync(dir, { recursive: true });
  if (!NodeFS.existsSync(NodePath.join(dir, ".git"))) {
    NodeFS.writeFileSync(NodePath.join(dir, "README.md"), title, "utf8");
    runGit(dir, ["init", "-b", "main"]);
    runGit(dir, ["config", "user.name", "Workflow E2E"]);
    runGit(dir, ["config", "user.email", "e2e@legion.codes"]);
    runGit(dir, ["config", "core.autocrlf", "false"]);
    runGit(dir, ["add", "README.md"]);
    runGit(dir, ["commit", "-m", "init"]);
  }

  const worktreesDir = NodePath.join(NodePath.dirname(dir), `${NodePath.basename(dir)}-worktrees`);
  NodeFS.mkdirSync(worktreesDir, { recursive: true });

  const createWorktree = (branchName: string, dirName?: string): string => {
    const safeName = dirName ?? branchName.replaceAll(/[^a-zA-Z0-9._-]/g, "-");
    const wtPath = NodePath.join(worktreesDir, safeName);
    runGit(dir, ["worktree", "add", "-b", branchName, wtPath, "HEAD"]);
    return wtPath;
  };

  const commitFile = (
    targetDir: string,
    relPath: string,
    content: string,
    message = `update ${relPath}`,
  ): string => {
    const absPath = NodePath.join(targetDir, relPath);
    NodeFS.mkdirSync(NodePath.dirname(absPath), { recursive: true });
    NodeFS.writeFileSync(absPath, content, "utf8");
    runGit(targetDir, ["add", relPath]);
    runGit(targetDir, ["commit", "-m", message]);
    return runGit(targetDir, ["rev-parse", "HEAD"]);
  };

  return { repoDir: dir, worktreesDir, createWorktree, commitFile };
}

export function copyDisposableLegionCli(
  repoRoot: string,
  destDir: string,
  options?: { readonly includeMap?: boolean | undefined },
): string {
  const sourceCliDir = NodePath.join(repoRoot, ".legion-cli");
  const destCliDir = NodePath.join(destDir, ".legion-cli");
  const includeMap = options?.includeMap ?? false;
  NodeFS.mkdirSync(destDir, { recursive: true });
  NodeFS.cpSync(sourceCliDir, destCliDir, {
    recursive: true,
    force: true,
    filter: (srcPath) => {
      const rel = NodePath.relative(sourceCliDir, srcPath);
      if (!rel) return true;
      const top = rel.split(NodePath.sep)[0];
      if (top === "cache") return false;
      if (!includeMap && top === "map") return false;
      return true;
    },
  });
  return destCliDir;
}

interface StartedServer {
  readonly origin: string;
  readonly pairingToken: string;
  readonly child: NodeChildProcess.ChildProcessWithoutNullStreams;
}

function buildIsolatedServerEnv(homeDir: string, extraEnv?: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const cleanEnv: NodeJS.ProcessEnv = { ...process.env };
  for (const key of Object.keys(cleanEnv)) {
    if (key.startsWith("LEGIONCODE_") || key.startsWith("RUST_ANYTHING_UNIT_")) {
      delete cleanEnv[key];
    }
  }
  delete cleanEnv.RUST_ANYTHING_FALLBACK_LOG;
  delete cleanEnv.SSH_CONNECTION;
  delete cleanEnv.SSH_TTY;
  delete cleanEnv.VITE_DEV_SERVER_URL;
  Object.assign(cleanEnv, extraEnv);
  cleanEnv.LOCALAPPDATA = homeDir;
  cleanEnv.USERPROFILE = homeDir;
  cleanEnv.HOME = homeDir;
  cleanEnv.XDG_CONFIG_HOME = homeDir;
  cleanEnv.GIT_TERMINAL_PROMPT = "0";
  cleanEnv.GIT_ASKPASS = "";
  return cleanEnv;
}

function spawnRustServer(
  binaryPath: string,
  homeDir: string,
  workspaceDir: string,
  extraEnv?: NodeJS.ProcessEnv,
): Promise<StartedServer> {
  return new Promise((resolve, reject) => {
    const cleanEnv = buildIsolatedServerEnv(homeDir, extraEnv);

    const child = NodeChildProcess.spawn(
      binaryPath,
      [
        "start",
        "--host",
        "127.0.0.1",
        "--port",
        "0",
        "--home-dir",
        homeDir,
        "--no-browser",
        workspaceDir,
      ],
      { cwd: workspaceDir, env: cleanEnv, stdio: ["pipe", "pipe", "pipe"] },
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
            `Rust server exited early (code=${String(code)}, signal=${String(signal)}). stderr:\n${stderrBuf}`,
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
          const parsedUrl = new NodeURL.URL(pairMatch[1].trim());
          const tokenParam = new NodeURL.URLSearchParams(parsedUrl.hash.replace(/^#/, "")).get(
            "token",
          );
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

function connectWs(url: string): Promise<WorkflowBoardRpcClient> {
  return new Promise((resolve, reject) => {
    const ws = new NodeWS.WebSocket(url);
    ws.once("open", () => resolve(new WorkflowBoardRpcClient(ws)));
    ws.once("error", reject);
  });
}

async function stopChildProcess(
  child: NodeChildProcess.ChildProcessWithoutNullStreams,
): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  await new Promise<void>((resolve) => {
    child.once("exit", () => resolve());
    child.kill();
  });
}

export async function fetchEnvironmentInfo(origin: string): Promise<WorkflowBoardEnvironmentInfo> {
  const envRes = await fetch(`${origin}/.well-known/legioncode/environment`);
  if (envRes.status !== 200) {
    throw new Error(`Environment descriptor failed with status ${envRes.status}`);
  }
  const rawJson = (await envRes.json()) as {
    readonly capabilities?: Record<string, unknown> | undefined;
  };
  const descriptor = decodeDescriptor(rawJson);
  return {
    descriptor,
    projectWorkflowBoard: rawJson.capabilities?.projectWorkflowBoard === true,
    databaseMigration: descriptor.databaseMigration,
  };
}

async function authenticateAndConnect(
  origin: string,
  pairingToken: string,
): Promise<{
  readonly cookiePair: string;
  readonly environment: WorkflowBoardEnvironmentInfo;
  readonly rpc: WorkflowBoardRpcClient;
}> {
  const environment = await fetchEnvironmentInfo(origin);

  const pairRes = await fetch(`${origin}/api/auth/browser-session`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ credential: pairingToken }),
  });
  if (pairRes.status !== 200) {
    throw new Error(`Browser session pairing failed with status ${pairRes.status}`);
  }
  const browserSession = decodeBrowserSession(await pairRes.json());
  if (!browserSession.authenticated) {
    throw new Error("Expected authenticated browser session");
  }

  const setCookie = pairRes.headers.get("set-cookie");
  if (!setCookie) {
    throw new Error("Missing set-cookie header from browser-session");
  }
  const cookiePair = setCookie.split(";")[0]!;

  const sessionRes = await fetch(`${origin}/api/auth/session`, {
    headers: { cookie: cookiePair },
  });
  if (sessionRes.status !== 200) {
    throw new Error(`Auth session check failed with status ${sessionRes.status}`);
  }
  const authState = decodeAuthSession(await sessionRes.json());
  if (!authState.authenticated) {
    throw new Error("Expected authenticated session state");
  }

  const ticketRes = await fetch(`${origin}/api/auth/websocket-ticket/rpc`, {
    method: "POST",
    headers: { cookie: cookiePair },
  });
  if (ticketRes.status !== 200) {
    throw new Error(`WebSocket ticket mint failed with status ${ticketRes.status}`);
  }
  const wsTicket = decodeWsTicket(await ticketRes.json());
  const wsOrigin = origin.replace(/^http/, "ws");
  const rpc = await connectWs(
    `${wsOrigin}/ws?orchestrationProtocol=2&frameParts=1&wsTicket=${encodeURIComponent(Redacted.value(wsTicket.ticket))}`,
  );

  return { cookiePair, environment, rpc };
}

export async function startWorkflowBoardE2eHarness(
  options: WorkflowBoardE2eHarnessOptions = {},
): Promise<WorkflowBoardE2eHarness> {
  const repoRoot = options.repoRoot ?? resolveWorkflowBoardRepoRoot();
  const binaryPath =
    options.binaryPath ??
    resolveRustCliBinary({
      repoRoot,
      platform: options.platform,
      env: options.env ?? {},
    });
  if (!binaryPath) {
    throw new Error("Expected built Rust legioncode binary for WorkflowBoardE2eHarness");
  }

  const tempRoot = NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-wf-board-e2e-"));
  const homeDir = NodePath.join(tempRoot, "home");
  const workspaceDir = NodePath.join(tempRoot, "workspace");
  const dbPath = NodePath.join(homeDir, "userdata", "statev2.sqlite");
  NodeFS.mkdirSync(homeDir, { recursive: true });
  NodeFS.mkdirSync(workspaceDir, { recursive: true });

  let git: WorkflowBoardGitWorkspace | undefined;
  let current: StartedServer | undefined;
  let session:
    | {
        readonly cookiePair: string;
        readonly environment: WorkflowBoardEnvironmentInfo;
        readonly rpc: WorkflowBoardRpcClient;
      }
    | undefined;
  let closed = false;

  const close = async (): Promise<void> => {
    if (closed) return;
    closed = true;
    try {
      session?.rpc.close();
      if (current) {
        await stopChildProcess(current.child);
      }
    } finally {
      NodeFS.rmSync(tempRoot, { recursive: true, force: true, maxRetries: 20, retryDelay: 25 });
    }
  };

  try {
    git = options.seedGitWorkspace
      ? initIsolatedGitRepo(workspaceDir, options.workspaceTitle)
      : undefined;

    current = await spawnRustServer(binaryPath, homeDir, workspaceDir, options.env);
    session = await authenticateAndConnect(current.origin, current.pairingToken);

    if (options.requireBoardCapability && !session.environment.projectWorkflowBoard) {
      throw new Error("Server does not advertise capabilities.projectWorkflowBoard");
    }
  } catch (err) {
    await close();
    throw err;
  }

  const restart = async (): Promise<void> => {
    session?.rpc.close();
    session = undefined;
    if (current) {
      await stopChildProcess(current.child);
      current = undefined;
    }
    current = await spawnRustServer(binaryPath, homeDir, workspaceDir, options.env);
    session = await authenticateAndConnect(current.origin, current.pairingToken);
  };

  const openReadOnlyDb = (): NodeSqlite.DatabaseSync => {
    return new NodeSqlite.DatabaseSync(dbPath, { readOnly: true });
  };

  const readSqlMigrationHead = (): number | undefined => {
    if (!NodeFS.existsSync(dbPath)) return undefined;
    const db = openReadOnlyDb();
    try {
      const row = db
        .prepare("SELECT MAX(migration_id) AS head FROM effect_sql_migrations")
        .get() as { readonly head?: number | null } | undefined;
      return typeof row?.head === "number" ? row.head : undefined;
    } finally {
      db.close();
    }
  };

  return {
    get origin() {
      if (!current) throw new Error("Harness server is closed");
      return current.origin;
    },
    homeDir,
    dbPath,
    workspaceDir,
    tempRoot,
    get pairingToken() {
      if (!current) throw new Error("Harness server is closed");
      return current.pairingToken;
    },
    get cookiePair() {
      if (!session) throw new Error("Harness session is closed");
      return session.cookiePair;
    },
    get environment() {
      if (!session) throw new Error("Harness session is closed");
      return session.environment;
    },
    get rpc() {
      if (!session) throw new Error("Harness session is closed");
      return session.rpc;
    },
    git,
    restart,
    openReadOnlyDb,
    readSqlMigrationHead,
    stageDisposableCliProject: (destSubdir) => {
      const dest = destSubdir ? NodePath.join(workspaceDir, destSubdir) : workspaceDir;
      return copyDisposableLegionCli(repoRoot, dest);
    },
    fetchEnvironment: () => {
      if (!current) throw new Error("Harness server is closed");
      return fetchEnvironmentInfo(current.origin);
    },
    close,
    [Symbol.asyncDispose]: close,
  };
}

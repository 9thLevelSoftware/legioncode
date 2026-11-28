// @effect-diagnostics nodeBuiltinImport:off -- Spawns staged Rust binary against isolated temp git repo/worktree + bare origin remote for M3 exit verification.
import * as NodeChildProcess from "node:child_process";
import * as NodeCrypto from "node:crypto";
import * as NodeFS from "node:fs";
import * as NodeHttp from "node:http";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, it } from "@effect/vitest";
import {
  AuthBrowserSessionResult,
  AuthWebSocketTicketResult,
  ExecutionEnvironmentDescriptor,
  GitActionProgressEvent,
  ProjectWorkflowBoardSnapshot,
  WorkflowBoardApproveDeliveryResult,
  WorkflowBoardApprovePlanResult,
  WorkflowBoardApproveSpecResult,
  WorkflowBoardChallengeStepResult,
  WorkflowBoardGetCardDiffResult,
  WorkflowBoardPauseCardResult,
  WorkflowBoardProposeAmendmentResult,
  WorkflowBoardRecordManualAcceptanceResult,
  WorkflowBoardResolveFindingResult,
  WorkflowBoardResumeCardResult,
  WorkflowBoardRetryStepResult,
  WorkflowBoardStartStageResult,
  WorkflowBoardUpdatePlanResult,
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
const decodeBoardSnapshot = Schema.decodeUnknownSync(
  Schema.toCodecJson(ProjectWorkflowBoardSnapshot),
);
const decodeCardDetail = Schema.decodeUnknownSync(Schema.toCodecJson(WorkflowCardDetail));
const decodeUpdateSpecResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardUpdateSpecResult),
);
const decodeChallengeStepResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardChallengeStepResult),
);
const decodeApproveSpecResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApproveSpecResult),
);
const decodeUpdatePlanResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardUpdatePlanResult),
);
const decodeApprovePlanResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApprovePlanResult),
);
const decodeStartStageResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardStartStageResult),
);
const decodeGetCardDiffResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardGetCardDiffResult),
);
const decodePauseCardResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardPauseCardResult),
);
const decodeResumeCardResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardResumeCardResult),
);
const decodeRetryStepResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardRetryStepResult),
);
const decodeProposeAmendmentResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardProposeAmendmentResult),
);
const decodeResolveFindingResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardResolveFindingResult),
);
const decodeRecordManualAcceptanceResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardRecordManualAcceptanceResult),
);
const decodeApproveDeliveryResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApproveDeliveryResult),
);
const decodeGitActionProgressEvent = Schema.decodeUnknownSync(
  Schema.toCodecJson(GitActionProgressEvent),
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

function writeExecutableShim(
  dir: string,
  name: string,
  scriptPath: string,
  isWindows: boolean,
): string {
  if (isWindows) {
    // Windows resolve_program checks extensionless `<name>` before `<name>.cmd`,
    // so only emit `<name>.cmd` on Windows.
    const shimPath = NodePath.join(dir, `${name}.cmd`);
    NodeFS.writeFileSync(shimPath, `@echo off\r\n"${process.execPath}" "${scriptPath}" %*\r\n`);
    return shimPath;
  }
  const shimPath = NodePath.join(dir, name);
  NodeFS.writeFileSync(shimPath, `#!/bin/sh\nexec "${process.execPath}" "${scriptPath}" "$@"\n`);
  NodeFS.chmodSync(shimPath, 0o755);
  return shimPath;
}

function writeM3StubTools(
  dir: string,
  isWindows: boolean,
  gatePort: number,
): {
  readonly binDir: string;
  readonly execClaudeBinaryPath: string;
  readonly gateClaudeBinaryPath: string;
  readonly reviewClaudeBinaryPath: string;
} {
  const binDir = NodePath.join(dir, "bin");
  NodeFS.mkdirSync(binDir, { recursive: true });

  // Custom mock-exec provider (claudeAgent stream-json) that writes allowed `src/second.ts`,
  // unexpected `unexpected.txt`, and (on first attempt) forbidden `secrets.env` into worktree cwd.
  const execScriptPath = NodePath.join(binDir, "mock-exec-claude.mjs");
  NodeFS.writeFileSync(
    execScriptPath,
    `import * as fs from "node:fs";
import * as path from "node:path";
process.stdin.resume();
const cwd = process.cwd();
fs.mkdirSync(path.join(cwd, "src"), { recursive: true });
fs.writeFileSync(path.join(cwd, "src", "second.ts"), "export const ready = false;\\n", "utf8");
fs.writeFileSync(path.join(cwd, "unexpected.txt"), "preserved unexpected edit\\n", "utf8");
if (!fs.existsSync(path.join(cwd, "secrets.env"))) {
  fs.writeFileSync(path.join(cwd, "secrets.env"), "TOKEN=forbidden\\n", "utf8");
}
const reply = "Implemented TSK-02 in worktree.";
console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-exec-1" }));
console.log(JSON.stringify({
  type: "assistant",
  message: {
    id: "msg-exec-1",
    role: "assistant",
    content: [{ type: "text", text: reply }],
    usage: { input_tokens: 15, output_tokens: 10 },
  },
}));
console.log(JSON.stringify({
  type: "result",
  subtype: "success",
  is_error: false,
  usage: { input_tokens: 15, output_tokens: 10 },
}));
process.exit(0);
`,
  );

  // Deterministic gated provider that holds an OS HTTP request open until released by the test.
  const gateScriptPath = NodePath.join(binDir, "mock-gate-claude.mjs");
  NodeFS.writeFileSync(
    gateScriptPath,
    `process.stdin.resume();
await fetch("http://127.0.0.1:${gatePort}/wait");
console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-gate-1" }));
console.log(JSON.stringify({
  type: "assistant",
  message: {
    id: "msg-gate-1",
    role: "assistant",
    content: [{ type: "text", text: "Completed TSK-01 after gate release." }],
    usage: { input_tokens: 10, output_tokens: 8 },
  },
}));
console.log(JSON.stringify({
  type: "result",
  subtype: "success",
  is_error: false,
  usage: { input_tokens: 10, output_tokens: 8 },
}));
process.exit(0);
`,
  );

  // Custom mock-review provider emitting strict JSON parsed by workflow_board_review_parse.rs.
  const reviewScriptPath = NodePath.join(binDir, "mock-review-claude.mjs");
  NodeFS.writeFileSync(
    reviewScriptPath,
    `process.stdin.resume();
const reviewPayload = JSON.stringify({
  verdict: "findings",
  summary: "Found one minor issue in src/second.ts.",
  findings: [
    {
      id: "FND-01",
      severity: "minor",
      criterionId: "AC-01",
      file: "src/second.ts",
      line: 1,
      message: "Document exported ready flag for callers.",
    },
  ],
});
console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-review-1" }));
console.log(JSON.stringify({
  type: "assistant",
  message: {
    id: "msg-review-1",
    role: "assistant",
    content: [{ type: "text", text: reviewPayload }],
    usage: { input_tokens: 20, output_tokens: 18 },
  },
}));
console.log(JSON.stringify({
  type: "result",
  subtype: "success",
  is_error: false,
  usage: { input_tokens: 20, output_tokens: 18 },
}));
process.exit(0);
`,
  );

  // Isolated `gh` stub handling auth status, pr list, pr create, and pr view.
  const prStatePath = NodePath.join(binDir, "gh-pr-state.json");
  const ghScriptPath = NodePath.join(binDir, "gh-stub.mjs");
  NodeFS.writeFileSync(
    ghScriptPath,
    `import * as fs from "node:fs";
const args = process.argv.slice(2);
const statePath = ${JSON.stringify(prStatePath)};
const readFlag = (name) => {
  const idx = args.indexOf(name);
  return idx >= 0 ? args[idx + 1] : undefined;
};
if (args[0] === "--version") {
  console.log("gh version 2.50.0");
  process.exit(0);
}
if (args[0] === "auth" && args[1] === "status") {
  console.log("Logged in to github.com as stub");
  process.exit(0);
}
if (args[0] === "pr" && args[1] === "list") {
  const head = readFlag("--head");
  if (fs.existsSync(statePath)) {
    const saved = JSON.parse(fs.readFileSync(statePath, "utf8"));
    if (!head || saved.headRefName === head) {
      console.log(JSON.stringify([saved]));
      process.exit(0);
    }
  }
  console.log("[]");
  process.exit(0);
}
if (args[0] === "pr" && args[1] === "create") {
  const headRefName = readFlag("--head") ?? "feat/m3-exec";
  const baseRefName = readFlag("--base") ?? "main";
  const title = readFlag("--title") ?? "M3 exit feature";
  const url = "https://github.com/org/repo/pull/42";
  fs.writeFileSync(statePath, JSON.stringify({
    number: 42,
    url,
    title,
    baseRefName,
    headRefName,
  }), "utf8");
  console.log(url);
  process.exit(0);
}
if (args[0] === "pr" && args[1] === "view") {
  if (!fs.existsSync(statePath)) {
    console.error("no pull requests found");
    process.exit(1);
  }
  console.log(fs.readFileSync(statePath, "utf8"));
  process.exit(0);
}
console.error("Unsupported gh stub args: " + JSON.stringify(args));
process.exit(1);
`,
  );

  writeExecutableShim(binDir, "gh", ghScriptPath, isWindows);
  return {
    binDir,
    execClaudeBinaryPath: writeExecutableShim(
      binDir,
      "mock-exec-claude",
      execScriptPath,
      isWindows,
    ),
    gateClaudeBinaryPath: writeExecutableShim(
      binDir,
      "mock-gate-claude",
      gateScriptPath,
      isWindows,
    ),
    reviewClaudeBinaryPath: writeExecutableShim(
      binDir,
      "mock-review-claude",
      reviewScriptPath,
      isWindows,
    ),
  };
}

interface HttpGate {
  readonly port: number;
  readonly gateEntered: Promise<void>;
  readonly releaseGate: () => void;
  readonly close: () => Promise<void>;
}

function createHttpGate(): Promise<HttpGate> {
  return new Promise((resolve, reject) => {
    let resolveEntered!: () => void;
    const gateEntered = new Promise<void>((res) => {
      resolveEntered = res;
    });
    let releaseResponse: (() => void) | undefined;
    let released = false;

    const server = NodeHttp.createServer((_req, res) => {
      resolveEntered();
      if (released) {
        res.writeHead(200);
        res.end("ok");
        return;
      }
      releaseResponse = () => {
        res.writeHead(200);
        res.end("ok");
      };
    });

    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        server.close();
        reject(new Error("Expected numeric port for HTTP gate server"));
        return;
      }
      resolve({
        port: address.port,
        gateEntered,
        releaseGate: () => {
          released = true;
          releaseResponse?.();
        },
        close: () =>
          new Promise<void>((res) => {
            server.closeAllConnections();
            server.close(() => res());
          }),
      });
    });
  });
}

function initBrownfieldRepoWithRemoteAndWorktree(tempRoot: string): {
  readonly bareRemoteDir: string;
  readonly repoDir: string;
  readonly worktreeDir: string;
  readonly initialHead: string;
} {
  const bareRemoteDir = NodePath.join(tempRoot, "origin.git");
  const repoDir = NodePath.join(tempRoot, "repo");
  const worktreeDir = NodePath.join(tempRoot, "repo-wt");

  NodeFS.mkdirSync(bareRemoteDir, { recursive: true });
  runGit(["init", "--bare", bareRemoteDir], tempRoot);
  runGit(["symbolic-ref", "HEAD", "refs/heads/main"], bareRemoteDir);

  NodeFS.mkdirSync(NodePath.join(repoDir, "src"), { recursive: true });
  runGit(["init", "-b", "main"], repoDir);
  runGit(["config", "user.name", "M3 Exit Test"], repoDir);
  runGit(["config", "user.email", "m3-exit@legioncode.local"], repoDir);
  NodeFS.writeFileSync(
    NodePath.join(repoDir, "package.json"),
    JSON.stringify({ name: "m3-exit-fixture", version: "1.0.0" }, null, 2),
    "utf8",
  );
  NodeFS.writeFileSync(
    NodePath.join(repoDir, "src", "feature.ts"),
    "export const version = 1;\n",
    "utf8",
  );
  runGit(["add", "."], repoDir);
  runGit(["commit", "-m", "init m3 fixture"], repoDir);
  runGit(["remote", "add", "origin", bareRemoteDir], repoDir);
  runGit(["push", "-u", "origin", "main"], repoDir);

  runGit(["worktree", "add", "-b", "feat/m3-exec", worktreeDir, "main"], repoDir);
  const initialHead = runGit(["rev-parse", "HEAD"], worktreeDir);
  return { bareRemoteDir, repoDir, worktreeDir, initialHead };
}

interface StartedServer {
  readonly origin: string;
  readonly pairingToken: string;
  readonly child: NodeChildProcess.ChildProcessWithoutNullStreams;
}

function startRustServer(
  binaryPath: string,
  homeDir: string,
  cwd: string,
  isWindows: boolean,
  extraPathDir?: string,
): Promise<StartedServer> {
  return new Promise((resolve, reject) => {
    const cleanEnv: NodeJS.ProcessEnv = { ...process.env };
    delete cleanEnv.LEGIONCODE_DEV_AUTH_TOKEN;
    delete cleanEnv.LEGIONCODE_LEGACY_NODE;
    delete cleanEnv.LEGIONCODE_HOME;
    delete cleanEnv.LEGIONCODE_STATE_DIR;
    delete cleanEnv.VITE_DEV_SERVER_URL;
    if (extraPathDir) {
      const currentPath = cleanEnv.PATH ?? cleanEnv.Path ?? "";
      cleanEnv.PATH = `${extraPathDir}${NodePath.delimiter}${currentPath}`;
      if (isWindows) {
        cleanEnv.Path = cleanEnv.PATH;
      }
    }

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
  readonly cookiePair: string;

  constructor(ws: NodeWS.WebSocket, cookiePair = "") {
    this.ws = ws;
    this.cookiePair = cookiePair;
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

  async callUnaryFailure(id: string, tag: string, payload: unknown): Promise<unknown> {
    this.sendRequest(id, tag, payload);
    const frame = await this.nextFrame((f) => f._tag === "Exit" && f.requestId === id);
    if (frame.exit?._tag !== "Failure") {
      throw new Error(`Expected RPC ${tag} to fail, got: ${JSON.stringify(frame.exit)}`);
    }
    return frame.exit.cause;
  }

  async nextChunk(requestId: string): Promise<unknown> {
    const frame = await this.nextFrame(
      (f) => f._tag === "Chunk" && f.requestId === requestId && Array.isArray(f.values),
    );
    this.sendAck(requestId);
    return frame.values?.[0];
  }

  async collectStreamUntilExit(
    requestId: string,
    tag: string,
    payload: unknown,
  ): Promise<unknown[]> {
    const chunks: unknown[] = [];
    this.sendRequest(requestId, tag, payload);
    for (;;) {
      const frame = await this.nextFrame(
        (f) => (f._tag === "Chunk" || f._tag === "Exit") && f.requestId === requestId,
      );
      if (frame._tag === "Chunk" && Array.isArray(frame.values)) {
        chunks.push(...frame.values);
        this.sendAck(requestId);
        continue;
      }
      if (frame._tag === "Exit") {
        if (frame.exit?._tag !== "Success") {
          throw new Error(`Stream RPC ${tag} failed: ${JSON.stringify(frame.exit)}`);
        }
        return chunks;
      }
    }
  }

  close(): void {
    this.ws.close();
  }
}

async function waitForBoardSnapshot(
  client: RpcClient,
  subId: string,
  predicate: (snapshot: ProjectWorkflowBoardSnapshot) => boolean,
): Promise<ProjectWorkflowBoardSnapshot> {
  for (;;) {
    const snapshot = decodeBoardSnapshot(await client.nextChunk(subId));
    if (predicate(snapshot)) {
      return snapshot;
    }
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
  return new RpcClient(ws, cookiePair);
}

function extractFailureError(cause: unknown): Record<string, unknown> | undefined {
  const entries = Array.isArray(cause) ? cause : [cause];
  for (const entry of entries) {
    if (entry !== null && typeof entry === "object" && "error" in entry) {
      const err = (entry as { readonly error?: unknown }).error;
      if (err !== null && typeof err === "object") {
        return err as Record<string, unknown>;
      }
    }
  }
  return undefined;
}

async function connectMcpOAuthAgentClient(origin: string, cookiePair: string): Promise<RpcClient> {
  const verifier = NodeCrypto.randomBytes(32).toString("base64url");
  const challenge = NodeCrypto.createHash("sha256").update(verifier).digest("base64url");
  const redirectUri = "http://127.0.0.1:9999/cb";

  const regRes = await fetch(`${origin}/oauth/mcp/register`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_name: "M3 Exit Agent Token",
      redirect_uris: [redirectUri],
      token_endpoint_auth_method: "none",
    }),
  });
  assert.strictEqual(regRes.status, 201);
  const { client_id: clientId } = (await regRes.json()) as { readonly client_id: string };

  const authParams = {
    response_type: "code",
    client_id: clientId,
    redirect_uri: redirectUri,
    code_challenge: challenge,
    code_challenge_method: "S256",
  };
  const approvalRes = await fetch(`${origin}/oauth/mcp/approval`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: cookiePair },
    body: JSON.stringify(authParams),
  });
  assert.strictEqual(approvalRes.status, 200);
  const { csrfToken } = (await approvalRes.json()) as { readonly csrfToken: string };

  const decisionRes = await fetch(`${origin}/oauth/mcp/decision`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: cookiePair },
    body: JSON.stringify({
      authorization: authParams,
      decision: { _tag: "browser-session", access: "full-access", csrfToken },
    }),
  });
  assert.strictEqual(decisionRes.status, 200);
  const { redirectTo } = (await decisionRes.json()) as { readonly redirectTo: string };
  const code = new URL(redirectTo).searchParams.get("code");
  assert.isString(code);

  const tokenRes = await fetch(`${origin}/oauth/mcp/token`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      grant_type: "authorization_code",
      code,
      redirect_uri: redirectUri,
      client_id: clientId,
      code_verifier: verifier,
    }),
  });
  assert.strictEqual(tokenRes.status, 200);
  const { access_token: accessToken } = (await tokenRes.json()) as {
    readonly access_token: string;
  };

  const wsOrigin = origin.replace(/^http/, "ws");
  const ws = new NodeWS.WebSocket(
    `${wsOrigin}/ws?orchestrationProtocol=2&wsTicket=${encodeURIComponent(accessToken)}`,
  );
  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
  return new RpcClient(ws);
}

it.live(
  "passes M3 exit criteria across execute contracts, pause/resume/amendment, planned checks, fresh-thread review, ship gates, and discrete git actions",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const platform = yield* HostProcess.Platform;
      const isWindows = platform === "win32";
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env: {} });
      assert.isDefined(binaryPath, "Expected staged rust/dist/bin/legioncode binary");

      const stagedIndexHtml = NodeFS.readFileSync(
        NodePath.join(repoRoot, "rust", "dist", "bin", "client", "index.html"),
        "utf8",
      );
      const webDistIndexHtml = NodeFS.readFileSync(
        NodePath.join(repoRoot, "apps", "web", "dist", "index.html"),
        "utf8",
      );
      assert.strictEqual(stagedIndexHtml, webDistIndexHtml);

      const tempRoot = yield* Effect.acquireRelease(
        Effect.sync(() => NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-m3-exit-"))),
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
      const { bareRemoteDir, repoDir, worktreeDir, initialHead } =
        initBrownfieldRepoWithRemoteAndWorktree(tempRoot);
      const gate = yield* Effect.acquireRelease(
        Effect.promise(() => createHttpGate()),
        (g) => Effect.promise(() => g.close()),
      );
      const stubs = writeM3StubTools(tempRoot, isWindows, gate.port);

      const host = yield* Effect.acquireRelease(
        Effect.promise(() =>
          startRustServer(binaryPath!, homeDir, repoDir, isWindows, stubs.binDir),
        ),
        ({ child }) => Effect.promise(() => stopRustServer(child)),
      );

      yield* Effect.promise(async () => {
        const indexRes = await fetch(`${host.origin}/`);
        assert.strictEqual(indexRes.status, 200);
        assert.strictEqual(await indexRes.text(), stagedIndexHtml);

        const envRes = await fetch(`${host.origin}/.well-known/legioncode/environment`);
        assert.strictEqual(envRes.status, 200);
        const descriptor = decodeDescriptor(await envRes.json());
        assert.strictEqual(descriptor.capabilities.projectWorkflowBoard, true);

        const client = await pairClient(host.origin, host.pairingToken);
        try {
          const projectId = "proj-m3-exit";
          await client.callUnary("proj", "orchestration.dispatchCommand", {
            type: "project.create",
            commandId: "cmd-proj-m3",
            projectId,
            title: "M3 Exit Repo",
            workspaceRoot: repoDir,
          });
          await client.callUnary("settings", "server.updateSettings", {
            patch: {
              workflowsEnabled: true,
              providerInstances: {
                "mock-exec": {
                  driver: "claudeAgent",
                  enabled: true,
                  config: { binaryPath: stubs.execClaudeBinaryPath },
                },
                "mock-gate": {
                  driver: "claudeAgent",
                  enabled: true,
                  config: { binaryPath: stubs.gateClaudeBinaryPath },
                },
                "mock-review": {
                  driver: "claudeAgent",
                  enabled: true,
                  config: { binaryPath: stubs.reviewClaudeBinaryPath },
                },
              },
            },
          });

          client.sendRequest("sub-board", WS_METHODS.workflowBoardSubscribe, { projectId });
          const initialSnap = decodeBoardSnapshot(await client.nextChunk("sub-board"));
          assert.strictEqual(initialSnap.cards.length, 0);

          // 1. Create card in feature worktree, approve spec, build DAG plan with checks + manual AC, approve plan.
          const created = decodeCardDetail(
            await client.callUnary("create-card", WS_METHODS.workflowBoardCreateCard, {
              projectId,
              title: "M3 End-to-End Execute & Ship",
              brief: "Execute tasks cleanly, review independently, gate ship, commit discretely.",
              sourceType: "codebase",
              intakeGoal: "change",
              assistanceMode: "direct",
              wireframesEnabled: false,
              wireframesIndex: null,
              workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
            }),
          );
          const cardId = created.cardId;

          const updatedSpec = decodeUpdateSpecResult(
            await client.callUnary("upd-spec", WS_METHODS.workflowBoardUpdateSpec, {
              cardId,
              expectedRevision: created.revision,
              acceptanceCriteria: [
                { id: "AC-01", text: "Exported ready flag in src/second.ts passes check" },
                { id: "AC-02", text: "Operator manually verifies shipped worktree output" },
              ],
              sourceRefs: ["src/feature.ts"],
            }),
          );

          const challengeDone = decodeChallengeStepResult(
            await client.callUnary("ch-fallback", WS_METHODS.workflowBoardChallengeStep, {
              cardId,
              expectedRevision: updatedSpec.revision,
              action: "manualFallback",
              acknowledgementToken: "I acknowledge",
              unresolvedConcernIds: [],
              answers: ["Scope confined.", "Worktree isolated.", "Checks automated."],
            }),
          );

          const specApproved = decodeApproveSpecResult(
            await client.callUnary("appr-spec", WS_METHODS.workflowBoardApproveSpec, {
              cardId,
              expectedRevision: challengeDone.revision,
            }),
          );

          const planned = decodeUpdatePlanResult(
            await client.callUnary("upd-plan", WS_METHODS.workflowBoardUpdatePlan, {
              cardId,
              expectedRevision: specApproved.revision,
              lenses: [
                { lens: "ux", applicable: false, reason: "Library module" },
                { lens: "functional", applicable: true, summary: "Implements ready flag" },
                { lens: "architecture", applicable: true, summary: "Scoped to src/second.ts" },
                { lens: "quality", applicable: true, summary: "Automated check + review" },
                { lens: "infrastructure", applicable: false, reason: "No infra changes" },
                { lens: "delivery", applicable: true, summary: "Sequential DAG tasks" },
              ],
              alternatives: [
                {
                  id: "alt-1",
                  title: "Two-task DAG",
                  summary: "Separate preparation from implementation",
                  tradeoffs: "Clear isolation",
                },
                {
                  id: "alt-2",
                  title: "Monolithic task",
                  summary: "Single edit step",
                  tradeoffs: "Coarser diff",
                },
              ],
              selectedAlternativeId: "alt-1",
              selectionRationale: "Clean dependency ordering across tasks.",
              strategy: "outcomes",
              granularity: "fine",
              tasks: [
                {
                  taskId: "TSK-01",
                  title: "Prepare feature module",
                  filesAllowed: ["src/feature.ts"],
                  testingMethod: "test-first",
                },
                {
                  taskId: "TSK-02",
                  title: "Add second module",
                  dependsOn: ["TSK-01"],
                  filesAllowed: ["src/second.ts"],
                  filesForbidden: ["secrets.env"],
                  testingMethod: "existing-checks",
                },
                {
                  taskId: "TSK-03",
                  title: "Add third module",
                  filesAllowed: ["src/third.ts"],
                  testingMethod: "existing-checks",
                },
              ],
              checks: [
                {
                  id: "CHK-01",
                  argv: [
                    "node",
                    "-e",
                    "process.exit(require('node:fs').readFileSync('src/second.ts','utf8').includes('ready = true') ? 0 : 1)",
                  ],
                },
              ],
              acMappings: [
                { criterionId: "AC-01", method: "automated", checkIds: ["CHK-01"] },
                {
                  criterionId: "AC-02",
                  method: "manual",
                  procedure: "Inspect worktree changes and verify manually.",
                },
              ],
            }),
          );

          const planApproved = decodeApprovePlanResult(
            await client.callUnary("appr-plan", WS_METHODS.workflowBoardApprovePlan, {
              cardId,
              expectedRevision: planned.revision,
            }),
          );
          assert.strictEqual(planApproved.card.stage, "execute");

          // 2a. DAG dependency rejection: TSK-02 depends on TSK-01 (still todo) -> contract_conflict.
          const dagBlockedCause = await client.callUnaryFailure(
            "start-tsk2-dag-blocked",
            WS_METHODS.workflowBoardStartStage,
            {
              cardId,
              expectedRevision: planApproved.revision,
              stage: "execute",
              taskId: "TSK-02",
              modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
              runtimeMode: "full-access",
            },
          );
          assert.strictEqual(extractFailureError(dagBlockedCause)?.code, "contract_conflict");

          // 2b. Run TSK-01 via mock-gate, verify single-mutating-task lock blocks TSK-03 & review, then release.
          const startTsk1 = decodeStartStageResult(
            await client.callUnary("start-tsk1", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: planApproved.revision,
              stage: "execute",
              taskId: "TSK-01",
              modelSelection: {
                instanceId: "mock-gate",
                model: "claude-sonnet-4-5",
                options: [],
              },
              runtimeMode: "full-access",
            }),
          );
          assert.strictEqual(startTsk1.taskId, "TSK-01");

          await gate.gateEntered;
          const duringTsk1 = decodeCardDetail(
            await client.callUnary("get-during-tsk1", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(
            duringTsk1.taskExecutions?.find((t) => t.taskId === "TSK-01")?.status,
            "in_progress",
          );

          const tsk3LockedCause = await client.callUnaryFailure(
            "start-tsk3-locked",
            WS_METHODS.workflowBoardStartStage,
            {
              cardId,
              expectedRevision: duringTsk1.revision,
              stage: "execute",
              taskId: "TSK-03",
              modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
              runtimeMode: "full-access",
            },
          );
          assert.strictEqual(extractFailureError(tsk3LockedCause)?.code, "contract_conflict");

          const reviewLockedCause = await client.callUnaryFailure(
            "start-review-locked",
            WS_METHODS.workflowBoardStartStage,
            {
              cardId,
              expectedRevision: duringTsk1.revision,
              stage: "review",
              modelSelection: {
                instanceId: "mock-review",
                model: "claude-sonnet-4-5",
                options: [],
              },
              runtimeMode: "full-access",
            },
          );
          assert.strictEqual(extractFailureError(reviewLockedCause)?.code, "contract_conflict");

          gate.releaseGate();
          await waitForBoardSnapshot(client, "sub-board", (snap) => {
            const card = snap.cards.find((c) => c.cardId === cardId);
            return card !== undefined && card.revision > startTsk1.revision;
          });
          const afterTsk1 = decodeCardDetail(
            await client.callUnary("get-after-tsk1", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          const tsk1Exec = afterTsk1.taskExecutions?.find((t) => t.taskId === "TSK-01");
          assert.strictEqual(tsk1Exec?.status, "completed");

          const startTsk2 = decodeStartStageResult(
            await client.callUnary("start-tsk2", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: afterTsk1.revision,
              stage: "execute",
              taskId: "TSK-02",
              modelSelection: {
                instanceId: "mock-exec",
                model: "claude-sonnet-4-5",
                options: [],
              },
              runtimeMode: "full-access",
            }),
          );

          // Wait reactively for mock-exec turn settlement + contract violation detection.
          await waitForBoardSnapshot(client, "sub-board", (snap) => {
            const card = snap.cards.find((c) => c.cardId === cardId);
            return card !== undefined && card.status === "blocked";
          });

          const blockedCard = decodeCardDetail(
            await client.callUnary("get-blocked", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(blockedCard.status, "blocked");
          const violationReport = blockedCard.fileContractReports?.find(
            (r) => r.taskId === "TSK-02" && r.status === "violation_pending",
          );
          assert.isDefined(violationReport);
          assert.isTrue(violationReport!.preservedInWorktree);
          assert.include(violationReport!.unexpectedFiles, "unexpected.txt");
          assert.include(violationReport!.forbiddenFiles, "secrets.env");
          assert.isTrue(NodeFS.existsSync(NodePath.join(worktreeDir, "unexpected.txt")));
          assert.isTrue(NodeFS.existsSync(NodePath.join(worktreeDir, "secrets.env")));

          const diffWhileBlocked = decodeGetCardDiffResult(
            await client.callUnary("diff-blocked", WS_METHODS.workflowBoardGetCardDiff, {
              cardId,
              includePatches: true,
            }),
          );
          assert.strictEqual(diffWhileBlocked.worktreePath, worktreeDir);
          const secondDiff = diffWhileBlocked.files.find((f) => f.path === "src/second.ts");
          const unexpectedDiff = diffWhileBlocked.files.find((f) => f.path === "unexpected.txt");
          const forbiddenDiff = diffWhileBlocked.files.find((f) => f.path === "secrets.env");
          assert.strictEqual(secondDiff?.contractStatus, "allowed");
          assert.strictEqual(unexpectedDiff?.contractStatus, "unexpected");
          assert.strictEqual(forbiddenDiff?.contractStatus, "forbidden");

          // 3. Pause, resume, propose plan amendment incorporating unexpected.txt, verify selective invalidation, re-approve plan.
          const paused = decodePauseCardResult(
            await client.callUnary("pause-card", WS_METHODS.workflowBoardPauseCard, {
              cardId,
              expectedRevision: blockedCard.revision,
              reason: "Inspect unexpected edit",
            }),
          );
          assert.strictEqual(paused.status, "paused");

          const resumed = decodeResumeCardResult(
            await client.callUnary("resume-card", WS_METHODS.workflowBoardResumeCard, {
              cardId,
              expectedRevision: paused.revision,
              reconcileInterrupted: true,
            }),
          );

          const amended = decodeProposeAmendmentResult(
            await client.callUnary("propose-amend", WS_METHODS.workflowBoardProposeAmendment, {
              cardId,
              expectedRevision: resumed.revision,
              targetStage: "plan",
              reason: "Incorporate unexpected.txt into TSK-02 allowed contract",
              sourceTaskId: "TSK-02",
              unexpectedFiles: ["unexpected.txt"],
              tasks: [
                {
                  taskId: "TSK-01",
                  title: "Prepare feature module",
                  filesAllowed: ["src/feature.ts"],
                  testingMethod: "test-first",
                },
                {
                  taskId: "TSK-02",
                  title: "Add second module and helper note",
                  dependsOn: ["TSK-01"],
                  filesAllowed: ["src/second.ts", "unexpected.txt"],
                  filesForbidden: ["secrets.env"],
                  testingMethod: "existing-checks",
                },
                {
                  taskId: "TSK-03",
                  title: "Add third module",
                  filesAllowed: ["src/third.ts"],
                  testingMethod: "existing-checks",
                },
              ],
            }),
          );
          assert.strictEqual(amended.specApproval?.status, "valid");
          assert.strictEqual(amended.planApproval?.status, "stale");

          const reapprovedPlan = decodeApprovePlanResult(
            await client.callUnary("reappr-plan", WS_METHODS.workflowBoardApprovePlan, {
              cardId,
              expectedRevision: amended.revision,
            }),
          );
          assert.strictEqual(reapprovedPlan.planApproval.status, "valid");

          // Complete TSK-02 under the amended contract (`unexpected.txt` now allowed).
          const retryTsk2 = decodeStartStageResult(
            await client.callUnary("retry-tsk2", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: reapprovedPlan.revision,
              stage: "execute",
              taskId: "TSK-02",
              modelSelection: {
                instanceId: "mock-exec",
                model: "claude-sonnet-4-5",
                options: [],
              },
              runtimeMode: "full-access",
            }),
          );
          await waitForBoardSnapshot(client, "sub-board", (snap) => {
            const card = snap.cards.find((c) => c.cardId === cardId);
            return card !== undefined && card.revision > retryTsk2.revision;
          });
          const afterTsk2Retry = decodeCardDetail(
            await client.callUnary("get-after-tsk2-retry", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(
            afterTsk2Retry.taskExecutions?.find((t) => t.taskId === "TSK-02")?.status,
            "completed",
          );

          const startTsk3 = decodeStartStageResult(
            await client.callUnary("start-tsk3", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: afterTsk2Retry.revision,
              stage: "execute",
              taskId: "TSK-03",
              modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
              runtimeMode: "full-access",
            }),
          );
          await waitForBoardSnapshot(client, "sub-board", (snap) => {
            const card = snap.cards.find((c) => c.cardId === cardId);
            return card !== undefined && card.revision > startTsk3.revision;
          });
          const afterTsk3 = decodeCardDetail(
            await client.callUnary("get-after-tsk3", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(
            afterTsk3.taskExecutions?.find((t) => t.taskId === "TSK-03")?.status,
            "completed",
          );

          // 4. Planned argv check lifecycle: fail -> pass -> snapshot mutation makes receipt stale -> re-run passes.
          const failedCheck = decodeRetryStepResult(
            await client.callUnary("chk-fail", WS_METHODS.workflowBoardRetryStep, {
              cardId,
              expectedRevision: afterTsk3.revision,
              stepKind: "check",
              targetId: "CHK-01",
            }),
          );
          assert.strictEqual(failedCheck.card.status, "blocked");
          const failedReceipt = failedCheck.card.checkReceipts?.find((r) => r.checkId === "CHK-01");
          assert.strictEqual(failedReceipt?.status, "failed");
          assert.strictEqual(failedReceipt?.exitCode, 1);

          NodeFS.writeFileSync(
            NodePath.join(worktreeDir, "src", "second.ts"),
            "export const ready = true;\n",
            "utf8",
          );
          const passedCheck = decodeRetryStepResult(
            await client.callUnary("chk-pass", WS_METHODS.workflowBoardRetryStep, {
              cardId,
              expectedRevision: failedCheck.revision,
              stepKind: "check",
              targetId: "CHK-01",
            }),
          );
          assert.strictEqual(passedCheck.card.status, "active");
          const passedReceipt = passedCheck.card.checkReceipts?.find((r) => r.checkId === "CHK-01");
          assert.strictEqual(passedReceipt?.status, "passed");
          assert.strictEqual(passedReceipt?.exitCode, 0);
          assert.isString(passedReceipt?.productSnapshotDigest);

          // Mutating product file makes previous check receipt stale.
          NodeFS.writeFileSync(
            NodePath.join(worktreeDir, "src", "second.ts"),
            "export const ready = true;\nexport const version = 2;\n",
            "utf8",
          );
          const staleView = decodeCardDetail(
            await client.callUnary("get-stale-chk", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(
            staleView.checkReceipts?.find((r) => r.checkId === "CHK-01")?.status,
            "stale",
          );

          const freshCheck = decodeRetryStepResult(
            await client.callUnary("chk-repass", WS_METHODS.workflowBoardRetryStep, {
              cardId,
              expectedRevision: staleView.revision,
              stepKind: "check",
              targetId: "CHK-01",
            }),
          );
          assert.strictEqual(
            freshCheck.card.checkReceipts?.find((r) => r.checkId === "CHK-01")?.status,
            "passed",
          );

          // 5. Fresh-thread independent review (distinct threadId, no stableLaunchKey) -> findings.
          const reviewStart = decodeStartStageResult(
            await client.callUnary("start-review", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: freshCheck.revision,
              stage: "review",
              modelSelection: {
                instanceId: "mock-review",
                model: "claude-sonnet-4-5",
                options: [],
              },
              runtimeMode: "full-access",
            }),
          );
          assert.notStrictEqual(reviewStart.threadId, startTsk1.threadId);
          assert.notStrictEqual(reviewStart.threadId, startTsk2.threadId);

          await waitForBoardSnapshot(client, "sub-board", (snap) => {
            const card = snap.cards.find((c) => c.cardId === cardId);
            return card !== undefined && card.revision > reviewStart.revision;
          });
          const afterReview = decodeCardDetail(
            await client.callUnary("get-after-review", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(afterReview.reviewReport?.status, "completed");
          assert.strictEqual(afterReview.reviewReport?.verdict, "findings");
          assert.isAtLeast(afterReview.reviewReport?.findings.length ?? 0, 1);
          const findingId = afterReview.reviewReport!.findings[0]!.id;
          const reviewId = afterReview.reviewReport!.reviewId;
          assert.strictEqual(afterReview.reviewReport!.findings[0]!.disposition, "unresolved");

          // Verify Ship blocker pointer before resolution.
          const acBlocker = afterReview.shipReadiness?.blockers.find(
            (b) => b.kind === "ac_unverified",
          );
          assert.deepStrictEqual(acBlocker?.pointer, {
            stage: "ship",
            tab: "acceptance",
            targetId: "AC-02",
          });

          // 6. Agent MCP OAuth token is refused with EnvironmentAuthorizationError on human-only M3 gates.
          const agentClient = await connectMcpOAuthAgentClient(host.origin, client.cookiePair);
          try {
            for (const [reqId, method, payload] of [
              [
                "agent-resolve-finding",
                WS_METHODS.workflowBoardResolveFinding,
                {
                  cardId,
                  expectedRevision: afterReview.revision,
                  reviewId,
                  findingId,
                  disposition: "accept_risk",
                  reason: "Agent bypass attempt",
                },
              ],
              [
                "agent-manual-ac",
                WS_METHODS.workflowBoardRecordManualAcceptance,
                {
                  cardId,
                  expectedRevision: afterReview.revision,
                  criterionId: "AC-02",
                  verdict: "pass",
                },
              ],
              [
                "agent-approve-delivery",
                WS_METHODS.workflowBoardApproveDelivery,
                {
                  cardId,
                  expectedRevision: afterReview.revision,
                },
              ],
            ] as const) {
              const cause = await agentClient.callUnaryFailure(reqId, method, payload);
              assert.strictEqual(extractFailureError(cause)?._tag, "EnvironmentAuthorizationError");
            }
          } finally {
            agentClient.close();
          }

          // Mismatched reviewId fails with invalid_input.
          const badReviewCause = await client.callUnaryFailure(
            "bad-rev-id",
            WS_METHODS.workflowBoardResolveFinding,
            {
              cardId,
              expectedRevision: afterReview.revision,
              reviewId: "rev-does-not-exist",
              findingId,
              disposition: "accept_risk",
              reason: "Should fail",
            },
          );
          assert.strictEqual(extractFailureError(badReviewCause)?.code, "invalid_input");

          // Blocked approveDelivery is rejected while blockers remain.
          await client.callUnaryFailure("blocked-ship", WS_METHODS.workflowBoardApproveDelivery, {
            cardId,
            expectedRevision: afterReview.revision,
          });

          // `fix` disposition leaves review_unresolved blocker; `accept_risk` clears it.
          const fixedFinding = decodeResolveFindingResult(
            await client.callUnary("resolve-fix", WS_METHODS.workflowBoardResolveFinding, {
              cardId,
              expectedRevision: afterReview.revision,
              reviewId,
              findingId,
              disposition: "fix",
            }),
          );
          assert.isTrue(
            fixedFinding.card.shipReadiness?.blockers.some((b) => b.kind === "review_unresolved"),
          );

          const acceptedRisk = decodeResolveFindingResult(
            await client.callUnary("resolve-accept", WS_METHODS.workflowBoardResolveFinding, {
              cardId,
              expectedRevision: fixedFinding.revision,
              reviewId,
              findingId,
              disposition: "accept_risk",
              reason: "Accepted for release",
            }),
          );
          assert.isFalse(
            acceptedRisk.card.shipReadiness?.blockers.some((b) => b.kind === "review_unresolved"),
          );

          // 7. Record manual AC acceptance -> Ship ready -> Approve delivery (zero auto-commit).
          const snapDigest = acceptedRisk.card.shipReadiness?.productSnapshotDigest ?? undefined;
          assert.isString(snapDigest);
          const acceptedAc = decodeRecordManualAcceptanceResult(
            await client.callUnary("manual-ac", WS_METHODS.workflowBoardRecordManualAcceptance, {
              cardId,
              expectedRevision: acceptedRisk.revision,
              criterionId: "AC-02",
              verdict: "pass",
              note: "Manually verified in M3 exit",
              evidenceRef: "manual://m3-exit",
              expectedProductSnapshotDigest: snapDigest,
            }),
          );
          assert.isTrue(acceptedAc.shipReadiness.ready);
          assert.strictEqual(acceptedAc.shipReadiness.blockers.length, 0);

          const shipped = decodeApproveDeliveryResult(
            await client.callUnary("approve-ship", WS_METHODS.workflowBoardApproveDelivery, {
              cardId,
              expectedRevision: acceptedAc.revision,
              expectedProductSnapshotDigest: snapDigest,
              note: "Approved M3 delivery",
            }),
          );
          assert.strictEqual(shipped.status, "completed");
          assert.strictEqual(shipped.deliveryStatus, "shipped");
          assert.strictEqual(shipped.deliveryReceipt.status, "shipped");
          assert.strictEqual(shipped.deliveryApproval.status, "valid");

          // Approve delivery must NOT auto-commit anything.
          assert.strictEqual(runGit(["rev-parse", "HEAD"], worktreeDir), initialHead);
          assert.strictEqual(runGit(["rev-list", "--count", "HEAD"], repoDir), "1");

          // 8. Individual discrete Git actions via git.runStackedAction (`commit`, `push`, `create_pr`).
          const commitEvents = (
            await client.collectStreamUntilExit("git-commit", WS_METHODS.gitRunStackedAction, {
              actionId: "act-m3-commit",
              cwd: worktreeDir,
              action: "commit",
              commitMessage: "feat(m3): implement ready flag",
              filePaths: ["src/second.ts"],
            })
          ).map((e) => decodeGitActionProgressEvent(e));
          const commitFinished = commitEvents.find((e) => e.kind === "action_finished");
          assert.isDefined(commitFinished);
          if (commitFinished?.kind === "action_finished") {
            assert.strictEqual(commitFinished.result.commit.status, "created");
            assert.strictEqual(commitFinished.result.push.status, "skipped_not_requested");
            assert.strictEqual(commitFinished.result.pr.status, "skipped_not_requested");
          }
          // `unexpected.txt` and `secrets.env` were NOT selected so they remain untracked in the worktree.
          const porcelainAfterCommit = runGit(["status", "--porcelain"], worktreeDir);
          assert.include(porcelainAfterCommit, "?? unexpected.txt");
          assert.include(porcelainAfterCommit, "?? secrets.env");

          const pushEvents = (
            await client.collectStreamUntilExit("git-push", WS_METHODS.gitRunStackedAction, {
              actionId: "act-m3-push",
              cwd: worktreeDir,
              action: "push",
            })
          ).map((e) => decodeGitActionProgressEvent(e));
          const pushFinished = pushEvents.find((e) => e.kind === "action_finished");
          assert.isDefined(pushFinished);
          if (pushFinished?.kind === "action_finished") {
            assert.strictEqual(pushFinished.result.push.status, "pushed");
            assert.strictEqual(pushFinished.result.pr.status, "skipped_not_requested");
          }
          const worktreeHead = runGit(["rev-parse", "HEAD"], worktreeDir);
          assert.notStrictEqual(worktreeHead, initialHead);
          assert.strictEqual(
            runGit(["rev-parse", "refs/heads/feat/m3-exec"], bareRemoteDir),
            worktreeHead,
          );

          const pr1Events = (
            await client.collectStreamUntilExit("git-pr-1", WS_METHODS.gitRunStackedAction, {
              actionId: "act-m3-pr-1",
              cwd: worktreeDir,
              action: "create_pr",
            })
          ).map((e) => decodeGitActionProgressEvent(e));
          const pr1Finished = pr1Events.find((e) => e.kind === "action_finished");
          assert.isDefined(pr1Finished);
          if (pr1Finished?.kind === "action_finished") {
            assert.strictEqual(pr1Finished.result.pr.status, "created");
            assert.strictEqual(pr1Finished.result.pr.number, 42);
            assert.strictEqual(pr1Finished.result.pr.url, "https://github.com/org/repo/pull/42");
          }

          const pr2Events = (
            await client.collectStreamUntilExit("git-pr-2", WS_METHODS.gitRunStackedAction, {
              actionId: "act-m3-pr-2",
              cwd: worktreeDir,
              action: "create_pr",
            })
          ).map((e) => decodeGitActionProgressEvent(e));
          const pr2Finished = pr2Events.find((e) => e.kind === "action_finished");
          assert.isDefined(pr2Finished);
          if (pr2Finished?.kind === "action_finished") {
            assert.strictEqual(pr2Finished.result.pr.status, "opened_existing");
            assert.strictEqual(pr2Finished.result.pr.number, 42);
          }

          // 9. Second card external deliverable edit detection (.legion/workflows/<card2Id>/plan.md).
          const card2 = decodeCardDetail(
            await client.callUnary("create-card-2", WS_METHODS.workflowBoardCreateCard, {
              projectId,
              title: "M3 External Edit Reconciliation Check",
              brief: "Detect external edits to plan.md before starting Execute.",
              sourceType: "codebase",
              intakeGoal: "change",
              assistanceMode: "direct",
              wireframesEnabled: false,
              wireframesIndex: null,
              workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
            }),
          );
          const card2Id = card2.cardId;

          const card2Spec = decodeUpdateSpecResult(
            await client.callUnary("upd-spec-2", WS_METHODS.workflowBoardUpdateSpec, {
              cardId: card2Id,
              expectedRevision: card2.revision,
              acceptanceCriteria: [{ id: "AC-01", text: "External plan edit invalidates plan" }],
              sourceRefs: ["src/feature.ts"],
            }),
          );
          const card2Challenge = decodeChallengeStepResult(
            await client.callUnary("ch-fallback-2", WS_METHODS.workflowBoardChallengeStep, {
              cardId: card2Id,
              expectedRevision: card2Spec.revision,
              action: "manualFallback",
              acknowledgementToken: "I acknowledge",
              unresolvedConcernIds: [],
              answers: ["Scoped.", "Isolated.", "Verified."],
            }),
          );
          const card2SpecApproved = decodeApproveSpecResult(
            await client.callUnary("appr-spec-2", WS_METHODS.workflowBoardApproveSpec, {
              cardId: card2Id,
              expectedRevision: card2Challenge.revision,
            }),
          );
          const card2Planned = decodeUpdatePlanResult(
            await client.callUnary("upd-plan-2", WS_METHODS.workflowBoardUpdatePlan, {
              cardId: card2Id,
              expectedRevision: card2SpecApproved.revision,
              lenses: [
                { lens: "ux", applicable: false, reason: "Backend check" },
                { lens: "functional", applicable: true, summary: "External edit check" },
                { lens: "architecture", applicable: true, summary: "Single file" },
                { lens: "quality", applicable: true, summary: "Check digest" },
                { lens: "infrastructure", applicable: false, reason: "None" },
                { lens: "delivery", applicable: true, summary: "One task" },
              ],
              alternatives: [
                {
                  id: "alt-1",
                  title: "Direct task",
                  summary: "Single task",
                  tradeoffs: "Minimal",
                },
                {
                  id: "alt-2",
                  title: "Split task",
                  summary: "Two tasks",
                  tradeoffs: "Extra overhead",
                },
              ],
              selectedAlternativeId: "alt-1",
              selectionRationale: "Minimal surface for reconciliation test.",
              strategy: "outcomes",
              granularity: "coarse",
              tasks: [
                {
                  taskId: "TSK-01",
                  title: "Modify feature",
                  filesAllowed: ["src/feature.ts"],
                  testingMethod: "existing-checks",
                },
              ],
              checks: [{ id: "CHK-01", argv: ["node", "-e", "process.exit(0)"] }],
              acMappings: [{ criterionId: "AC-01", method: "automated", checkIds: ["CHK-01"] }],
            }),
          );
          const card2PlanApproved = decodeApprovePlanResult(
            await client.callUnary("appr-plan-2", WS_METHODS.workflowBoardApprovePlan, {
              cardId: card2Id,
              expectedRevision: card2Planned.revision,
            }),
          );
          assert.strictEqual(card2PlanApproved.planApproval.status, "valid");

          NodeFS.appendFileSync(
            NodePath.join(worktreeDir, ".legion", "workflows", card2Id, "plan.md"),
            "\n<!-- external edit -->\n",
            "utf8",
          );
          const card2AfterExternalEdit = decodeCardDetail(
            await client.callUnary("get-card2-external", WS_METHODS.workflowBoardGetCard, {
              cardId: card2Id,
              projectId,
            }),
          );
          assert.strictEqual(card2AfterExternalEdit.planApproval?.status, "stale");
          assert.isTrue(card2AfterExternalEdit.hasUnreconciledEdits);
          assert.isTrue(
            card2AfterExternalEdit.shipReadiness?.blockers.some((b) => b.kind === "external_edit"),
          );

          const card2StartErr = await client.callUnaryFailure(
            "start-card2-external-edit",
            WS_METHODS.workflowBoardStartStage,
            {
              cardId: card2Id,
              expectedRevision: card2AfterExternalEdit.revision,
              stage: "execute",
              taskId: "TSK-01",
              modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
              runtimeMode: "full-access",
            },
          );
          assert.strictEqual(extractFailureError(card2StartErr)?.code, "plan_not_approved");
        } finally {
          client.close();
        }
      });
    }).pipe(Effect.scoped),
);
